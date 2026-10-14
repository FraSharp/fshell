// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Incremental stderr transport. Retention limits never close the child's pipe.

use fshell_core::diagnostic::{FshDiag, StderrBytes};
use fshell_engine::{PipeSender, PipelinePayload};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

async fn send(tx: &PipeSender, bytes: Vec<u8>) -> io::Result<()> {
    tx.send(PipelinePayload::Structured(FshDiag::new(StderrBytes(
        bytes,
    ))))
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "stderr consumer closed"))
}

pub(super) async fn forward(
    mut reader: impl AsyncRead + Unpin,
    tx: PipeSender,
    limit: usize,
) -> io::Result<()> {
    let mut buffer = [0u8; 8192];
    let mut remaining = limit;
    let mut truncated = false;
    loop {
        let count = match reader.read(&mut buffer).await {
            Ok(0) => return Ok(()),
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let retained = count.min(remaining);
        if retained > 0 {
            send(&tx, buffer[..retained].to_vec()).await?;
            remaining -= retained;
        }
        if count > retained && !truncated {
            truncated = true;
            if limit > 0 {
                send(
                    &tx,
                    format!("\n...(stderr truncated at {limit} bytes)\n").into_bytes(),
                )
                .await?;
            }
        }
        // Continue draining after the retention limit. Dropping this pipe early
        // gives the child SIGPIPE or changes its status merely for being verbose.
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "transport fault assertions intentionally panic on unexpected shape"
)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::sync::mpsc;

    fn bytes(payload: PipelinePayload) -> Vec<u8> {
        let PipelinePayload::Structured(diag) = payload else {
            panic!("expected stderr transport");
        };
        assert!(diag.is_stderr_stream());
        assert!(diag.reported_status().is_none());
        diag.stderr_bytes().unwrap().to_vec()
    }

    #[tokio::test]
    async fn forwards_before_eof_and_preserves_split_and_invalid_utf8() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let (tx, mut rx) = mpsc::channel(16);
        let task = tokio::spawn(forward(reader, tx, 1024));
        writer.write_all(b"early\xc3").await.unwrap();
        let mut output = bytes(
            tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .unwrap()
                .unwrap(),
        );
        assert_eq!(output, b"early\xc3");
        assert!(
            !task.is_finished(),
            "first chunk must precede writer closure"
        );
        writer.write_all(b"\xa9\0\xfflate\r").await.unwrap();
        drop(writer);
        while let Some(payload) = rx.recv().await {
            output.extend(bytes(payload));
        }
        task.await.unwrap().unwrap();
        assert_eq!(output, b"early\xc3\xa9\0\xfflate\r");
    }

    #[tokio::test]
    async fn truncation_drains_excess_without_failing_the_writer() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let (tx, mut rx) = mpsc::channel(16);
        let task = tokio::spawn(forward(reader, tx, 7));
        tokio::time::timeout(Duration::from_secs(1), writer.write_all(&[b'x'; 10000]))
            .await
            .unwrap()
            .unwrap();
        drop(writer);
        task.await.unwrap().unwrap();
        let mut output = Vec::new();
        while let Some(payload) = rx.recv().await {
            output.extend(bytes(payload));
        }
        assert_eq!(output, b"xxxxxxx\n...(stderr truncated at 7 bytes)\n");
    }

    #[tokio::test]
    async fn exact_limit_is_not_truncation_and_zero_limit_is_silent() {
        for (input, limit, expected) in [
            (b"exact".as_slice(), 5, b"exact".as_slice()),
            (b"discard".as_slice(), 0, b"".as_slice()),
        ] {
            let (mut writer, reader) = tokio::io::duplex(64);
            let (tx, mut rx) = mpsc::channel(16);
            writer.write_all(input).await.unwrap();
            drop(writer);
            forward(reader, tx, limit).await.unwrap();
            let mut output = Vec::new();
            while let Some(payload) = rx.recv().await {
                output.extend(bytes(payload));
            }
            assert_eq!(output, expected);
        }
    }
}
