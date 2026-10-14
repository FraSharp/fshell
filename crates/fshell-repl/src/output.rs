// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! One blocking display consumer per pipeline, with bounded transport buffering.
//! Only a homogeneous table retains rows for aggregate formatting; values already
//! displayed are never accumulated. Raw byte chunks are not text records.

use fshell_core::{Pipeline, Val};
use fshell_engine::{Env, PipelinePayload};
use std::io::{self, BufWriter, Write};
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Default)]
pub(crate) struct OutputSummary {
    pub rows: Vec<Val>,
    pub has_errors: bool,
    pub item_count: usize,
    pub byte_count: usize,
}

pub(crate) async fn run(pipeline: &Pipeline, env: &Env, source: &str) -> OutputSummary {
    let (tx, mut rx) = mpsc::channel(fshell_engine::pipeline_channel_size(env));
    let producer_env = env.clone();
    let pipeline = pipeline.clone();
    let producer = tokio::spawn(async move {
        let tx_error = tx.clone();
        if let Err(error) = fshell_engine::execute_pipeline(&pipeline, &producer_env, tx).await {
            let _ = fshell_engine::send_with_backpressure(
                &producer_env,
                &tx_error,
                PipelinePayload::Structured(error.into()),
            )
            .await;
        }
    });

    // Commands that produce no payload need no formatter task or output buffer.
    let result = if let Some(first) = rx.recv().await {
        let worker_env = env.clone();
        let source = source.to_owned();
        tokio::task::spawn_blocking(move || {
            drain(rx, first, &worker_env, &source, io::stdout(), io::stderr())
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|result| result.map_err(|error| error.to_string()))
    } else {
        Ok(OutputSummary::default())
    };
    let mut summary = result.unwrap_or_else(|error| {
        env.report_stage_error();
        eprintln!("output formatter error: {error}");
        OutputSummary {
            has_errors: true,
            ..OutputSummary::default()
        }
    });
    // Even on a failed display write, command completion must not race a live
    // producer's terminal/job guards. Closing rx makes backpressured sends stop.
    if let Err(error) = producer.await {
        env.report_stage_error();
        summary.has_errors = true;
        eprintln!("pipeline task failed: {error}");
    }
    summary
}

fn drain<W: Write, E: Write>(
    mut rx: mpsc::Receiver<PipelinePayload>,
    first: PipelinePayload,
    env: &Env,
    source: &str,
    output: W,
    mut errors: E,
) -> io::Result<OutputSummary> {
    let theme = env.active_theme();
    let mut output = BufWriter::with_capacity(64 * 1024, output);
    let mut summary = OutputSummary::default();
    let mut table_only = true;
    let mut last_stdout_byte = None;
    let mut next = Some(first);
    while let Some(payload) = next {
        match payload {
            PipelinePayload::Data(value) => {
                summary.item_count = summary.item_count.saturating_add(1);
                if table_only && matches!(&*value, Val::Map(_)) {
                    summary.rows.push(Arc::unwrap_or_clone(value));
                } else {
                    if table_only {
                        table_only = false;
                        for row in summary.rows.drain(..) {
                            crate::format::write_item_streaming(&mut output, &row, &theme)?;
                        }
                    }
                    if matches!(&*value, Val::List(_)) {
                        // Lists may open the fullscreen pager. Flush buffered
                        // records before it takes terminal ownership.
                        output.flush()?;
                        crate::format::print_item_streaming(&value, &theme);
                        fshell_tty::mark_cursor_state_unknown();
                    } else {
                        crate::format::write_item_streaming(&mut output, &value, &theme)?;
                    }
                    last_stdout_byte = match &*value {
                        Val::Blob(bytes) => bytes.last().copied().or(last_stdout_byte),
                        Val::String(text) if text.starts_with('\0') => last_stdout_byte,
                        Val::String(text) if text.ends_with('\0') => text.as_bytes()
                            [..text.len() - 1]
                            .last()
                            .copied()
                            .or(last_stdout_byte),
                        _ => Some(b'\n'),
                    };
                }
            }
            PipelinePayload::Bytes(bytes) => {
                summary.byte_count = summary.byte_count.saturating_add(bytes.len());
                if table_only {
                    table_only = false;
                    for row in summary.rows.drain(..) {
                        crate::format::write_item_streaming(&mut output, &row, &theme)?;
                    }
                }
                output.write_all(&bytes)?;
                last_stdout_byte = bytes.last().copied().or(last_stdout_byte);
            }
            PipelinePayload::Structured(diag) => {
                output.flush()?;
                if let Some(text) = diag.stderr_text() {
                    write_stderr_record(&mut errors, &text)?;
                } else {
                    summary.has_errors = true;
                    if !fshell_engine::is_condition_false_diag(&diag) {
                        let config = {
                            let options = env.options.read();
                            fshell_render::RenderConfig {
                                format: options.error_format,
                                color: options.error_color,
                                is_interactive: true,
                            }
                        };
                        env.set_last_error_with_source(
                            diag.clone(),
                            source.to_owned(),
                            "repl".into(),
                        );
                        writeln!(
                            errors,
                            "{}",
                            fshell_render::render(diag, None, "pipeline", &config)
                        )?;
                    }
                }
                errors.flush()?;
            }
        }
        next = match rx.try_recv() {
            Ok(payload) => Some(payload),
            Err(mpsc::error::TryRecvError::Empty) => {
                // Show partial/progress output before waiting for another item;
                // a quiet producer must never leave bytes stranded in a buffer.
                output.flush()?;
                rx.blocking_recv()
            }
            Err(mpsc::error::TryRecvError::Disconnected) => None,
        };
    }
    output.flush()?;
    if let Some(byte) = last_stdout_byte {
        fshell_tty::mark_cursor_state_unknown_for_stdout_output(&[byte]);
    }
    Ok(summary)
}

fn write_stderr_record(writer: &mut impl Write, text: &str) -> io::Result<()> {
    if text.starts_with('\0') {
        return Ok(());
    }
    if let Some(text) = text.strip_suffix('\0') {
        fshell_tty::mark_cursor_state_unknown_for_stderr_output(text.as_bytes());
        writer.write_all(text.as_bytes())
    } else {
        writer.write_all(text.as_bytes())?;
        if !text.ends_with('\n') {
            writer.write_all(b"\n")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(payloads: Vec<PipelinePayload>) -> (OutputSummary, Vec<u8>, Vec<u8>) {
        let (tx, mut rx) = mpsc::channel(payloads.len().max(1));
        for payload in payloads {
            tx.try_send(payload).unwrap();
        }
        drop(tx);
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let summary = match rx.blocking_recv() {
            Some(first) => drain(rx, first, &Env::new(), "test", &mut output, &mut errors).unwrap(),
            None => OutputSummary::default(),
        };
        (summary, output, errors)
    }

    #[test]
    fn bytes_preserve_chunk_boundaries_utf8_and_missing_final_newline() {
        let (summary, output, _) = collect(vec![
            PipelinePayload::Bytes(vec![0xc3].into()),
            PipelinePayload::Bytes(vec![0xa9, 0, 0xff].into()),
            PipelinePayload::Bytes(b"tail".as_slice().into()),
        ]);
        assert_eq!(output, b"\xc3\xa9\0\xfftail");
        assert_eq!(summary.byte_count, 8);
        assert!(summary.rows.is_empty());
    }

    #[test]
    fn displayed_records_are_not_retained() {
        let (summary, output, _) = collect(
            (0..1000)
                .map(|_| PipelinePayload::Data(Arc::new(Val::String("line".into()))))
                .collect(),
        );
        assert_eq!(summary.item_count, 1000);
        assert!(summary.rows.is_empty());
        assert_eq!(output, b"line\n".repeat(1000));
    }

    #[test]
    fn quiet_producer_flushes_before_the_consumer_waits() {
        struct NotifyWriter {
            bytes: Vec<u8>,
            written: std::sync::mpsc::Sender<()>,
        }
        impl Write for NotifyWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.bytes.extend_from_slice(bytes);
                let _ = self.written.send(());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (written, waiting) = std::sync::mpsc::channel();
        let (tx, rx) = mpsc::channel(1);
        let producer = std::thread::spawn(move || {
            waiting
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("early output was buffered while waiting");
            tx.blocking_send(PipelinePayload::Bytes(b"late".as_slice().into()))
                .unwrap();
        });
        let mut output = NotifyWriter {
            bytes: Vec::new(),
            written,
        };
        drain(
            rx,
            PipelinePayload::Bytes(b"early".as_slice().into()),
            &Env::new(),
            "test",
            &mut output,
            Vec::new(),
        )
        .unwrap();
        producer.join().unwrap();
        assert_eq!(output.bytes, b"earlylate");
    }

    #[test]
    fn stderr_is_output_not_an_execution_error() {
        let (summary, _, errors) = collect(vec![PipelinePayload::Structured(
            fshell_core::FshDiag::new(fshell_core::StderrStream("progress\r\0".into())),
        )]);
        assert!(!summary.has_errors);
        assert_eq!(errors, b"progress\r");
    }

    #[test]
    fn homogeneous_tables_remain_available_for_aggregate_formatting() {
        let (summary, output, _) = collect(vec![PipelinePayload::Data(Arc::new(Val::Map(
            Default::default(),
        )))]);
        assert_eq!(summary.rows.len(), 1);
        assert!(output.is_empty());
        let (summary, output, _) = collect(vec![
            PipelinePayload::Data(Arc::new(Val::Map(Default::default()))),
            PipelinePayload::Data(Arc::new(Val::String("tail".into()))),
        ]);
        assert!(summary.rows.is_empty());
        assert!(output.ends_with(b"tail\n"));
        assert!(output.starts_with(b"{ "));
    }
}
