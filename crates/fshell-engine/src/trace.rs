// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Opt-in, per-process timing trace output.
//!
//! Traces are JSONL so they can be inspected with ordinary tools or converted
//! to a timeline format. This is intentionally separate from `ProfilerState`:
//! spans have explicit identities and parentage, so concurrent Tokio tasks do
//! not share an implicit call stack.

use fshell_core::Mutex;
use serde::Serialize;
use serde_json::{Map, Value};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

macro_rules! trace_id {
    ($name:ident, $inner:ty) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(pub $inner);
    };
}

trace_id!(RunId, u64);
trace_id!(CommandRunId, u64);
trace_id!(PipelineId, u64);
trace_id!(StageId, u32);
trace_id!(SpanId, u64);

/// Stable classification of how a measured operation completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanOutcome {
    Ok,
    Error,
    Cancelled,
    Exit,
    ExecReplaced,
    Signal,
    /// The guard left scope without an explicit completion classification.
    Abandoned,
}

/// Explicit task context. Pipeline and stage identity are span attributes so
/// ordinary child spans need carry only run/command correlation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpanContext {
    pub run_id: RunId,
    pub parent_span_id: Option<SpanId>,
    pub command_run_id: Option<CommandRunId>,
}

impl SpanContext {
    pub fn child_of(self, parent_span_id: SpanId) -> Self {
        Self {
            parent_span_id: Some(parent_span_id),
            ..self
        }
    }

    pub fn for_command(self, command_run_id: CommandRunId) -> Self {
        Self {
            command_run_id: Some(command_run_id),
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceMode {
    Startup,
    Command,
    Interactive,
    Script,
    Posix,
    Utility,
}

#[derive(Serialize)]
struct TraceRecord {
    schema_version: u32,
    run_id: RunId,
    span_id: SpanId,
    parent_id: Option<SpanId>,
    command_run_id: Option<CommandRunId>,
    name: String,
    start_ns: u64,
    duration_ns: u64,
    mode: TraceMode,
    outcome: SpanOutcome,
    attrs: Map<String, Value>,
}

struct SinkState {
    writer: Option<BufWriter<File>>,
}

/// Shared writer and process-relative monotonic clock for one shell process.
pub struct TraceSink {
    run_id: RunId,
    started: Instant,
    enabled: AtomicBool,
    next_span_id: AtomicU64,
    next_command_run_id: AtomicU64,
    next_pipeline_id: AtomicU64,
    state: Mutex<SinkState>,
}

impl TraceSink {
    /// Open the opt-in JSONL sink named by `FSH_TRACE_FILE`.
    ///
    /// An invalid path disables tracing and emits one startup diagnostic. Any
    /// later write/flush error disables tracing silently so a failure cannot
    /// leak into FTUI or affect shell command outcomes.
    pub fn from_env() -> Arc<Self> {
        let path = std::env::var_os("FSH_TRACE_FILE").map(PathBuf::from);
        let writer = path.as_ref().and_then(|path| match open_trace_file(path) {
            Ok(writer) => Some(writer),
            Err(error) => {
                eprintln!(
                    "fsh: timing trace disabled: cannot open {}: {error}",
                    path.display()
                );
                None
            }
        });
        Self::new(writer)
    }

    /// Disabled sink for environments constructed outside the binary entry
    /// point (including tests and embedders).
    pub fn disabled() -> Arc<Self> {
        Self::new(None)
    }

    fn new(writer: Option<BufWriter<File>>) -> Arc<Self> {
        let clock = Instant::now();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let run_id = RunId(nanos ^ (u64::from(std::process::id()) << 32));
        Arc::new(Self {
            run_id,
            started: clock,
            enabled: AtomicBool::new(writer.is_some()),
            next_span_id: AtomicU64::new(1),
            next_command_run_id: AtomicU64::new(1),
            next_pipeline_id: AtomicU64::new(1),
            state: Mutex::new(SinkState { writer }),
        })
    }

    pub fn root_context(&self) -> SpanContext {
        SpanContext {
            run_id: self.run_id,
            parent_span_id: None,
            command_run_id: None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn next_command_run_id(&self) -> Option<CommandRunId> {
        self.is_enabled()
            .then(|| CommandRunId(self.next_command_run_id.fetch_add(1, Ordering::Relaxed)))
    }

    pub fn next_pipeline_id(&self) -> Option<PipelineId> {
        self.is_enabled()
            .then(|| PipelineId(self.next_pipeline_id.fetch_add(1, Ordering::Relaxed)))
    }

    pub fn span(
        self: &Arc<Self>,
        context: SpanContext,
        name: impl Into<String>,
        mode: TraceMode,
        attrs: Map<String, Value>,
    ) -> Option<TraceSpan> {
        if !self.is_enabled() {
            return None;
        }
        let started = Instant::now();
        Some(TraceSpan {
            sink: Arc::clone(self),
            context,
            span_id: SpanId(self.next_span_id.fetch_add(1, Ordering::Relaxed)),
            name: name.into(),
            mode,
            attrs,
            started,
            start_ns: self.started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
            finished: false,
        })
    }

    pub fn command_span(
        self: &Arc<Self>,
        context: SpanContext,
        name: impl Into<String>,
        mode: TraceMode,
        attrs: Map<String, Value>,
    ) -> Option<TraceSpan> {
        let command_run_id = self.next_command_run_id()?;
        self.span(context.for_command(command_run_id), name, mode, attrs)
    }

    /// Flush buffered records at process lifecycle boundaries. A flush error
    /// disables the sink and is deliberately not returned to shell code.
    pub fn flush(&self) {
        let mut state = self.state.lock();
        if let Some(writer) = &mut state.writer
            && writer.flush().is_err()
        {
            state.writer = None;
            self.enabled.store(false, Ordering::Release);
        }
    }

    /// Record and flush a process exit before using `process::exit`.
    pub fn exit_process(
        trace: &Arc<Self>,
        context: SpanContext,
        mode: TraceMode,
        code: i32,
        outcome: SpanOutcome,
    ) -> ! {
        if let Some(mut span) = trace.span(context, "process.exit", mode, Map::new()) {
            span.add_attr("exit_code", code);
            span.finish(outcome);
        }
        trace.flush();
        std::process::exit(code)
    }

    fn record(&self, record: &TraceRecord) {
        if !self.is_enabled() {
            return;
        }
        let mut state = self.state.lock();
        let Some(writer) = state.writer.as_mut() else {
            self.enabled.store(false, Ordering::Release);
            return;
        };
        if serde_json::to_writer(&mut *writer, record).is_err() || writer.write_all(b"\n").is_err()
        {
            state.writer = None;
            self.enabled.store(false, Ordering::Release);
        }
    }
}

fn open_trace_file(path: &PathBuf) -> std::io::Result<BufWriter<File>> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map(BufWriter::new)
}

/// A completed-duration span. If a caller exits early without classifying
/// the span, `Drop` records `abandoned` rather than losing its duration.
pub struct TraceSpan {
    sink: Arc<TraceSink>,
    context: SpanContext,
    span_id: SpanId,
    name: String,
    mode: TraceMode,
    attrs: Map<String, Value>,
    started: Instant,
    start_ns: u64,
    finished: bool,
}

impl TraceSpan {
    pub fn id(&self) -> SpanId {
        self.span_id
    }

    pub fn context(&self) -> SpanContext {
        self.context.child_of(self.span_id)
    }

    pub fn context_for_command(&self, command_run_id: CommandRunId) -> SpanContext {
        self.context().for_command(command_run_id)
    }

    pub fn add_attr(&mut self, key: impl Into<String>, value: impl Serialize) {
        if let Ok(value) = serde_json::to_value(value) {
            self.attrs.insert(key.into(), value);
        }
    }

    pub fn finish(mut self, outcome: SpanOutcome) {
        self.record(outcome);
        self.finished = true;
    }

    fn record(&self, outcome: SpanOutcome) {
        let record = TraceRecord {
            schema_version: 1,
            run_id: self.context.run_id,
            span_id: self.span_id,
            parent_id: self.context.parent_span_id,
            command_run_id: self.context.command_run_id,
            name: self.name.clone(),
            start_ns: self.start_ns,
            duration_ns: self.started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
            mode: self.mode,
            outcome,
            attrs: self.attrs.clone(),
        };
        self.sink.record(&record);
    }
}

impl Drop for TraceSpan {
    fn drop(&mut self) {
        if !self.finished {
            self.record(SpanOutcome::Abandoned);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    #[test]
    fn writes_versioned_span_with_explicit_parent_and_typed_outcome() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("trace.jsonl");
        let sink = Arc::new(TraceSink {
            run_id: RunId(7),
            started: Instant::now(),
            enabled: AtomicBool::new(true),
            next_span_id: AtomicU64::new(1),
            next_command_run_id: AtomicU64::new(1),
            next_pipeline_id: AtomicU64::new(1),
            state: Mutex::new(SinkState {
                writer: Some(open_trace_file(&path).unwrap()),
            }),
        });
        let root = sink.root_context();
        let parent = sink
            .span(root, "command", TraceMode::Interactive, Map::new())
            .unwrap();
        let parent_id = parent.id();
        let child = sink
            .span(
                parent.context(),
                "parse",
                TraceMode::Interactive,
                Map::new(),
            )
            .unwrap();
        child.finish(SpanOutcome::Ok);
        parent.finish(SpanOutcome::Error);
        sink.flush();

        let records: Vec<Value> = BufReader::new(File::open(path).unwrap())
            .lines()
            .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["schema_version"], 1);
        assert_eq!(records[0]["outcome"], "ok");
        assert_eq!(records[1]["parent_id"], parent_id.0);
        assert_eq!(records[1]["outcome"], "error");
        assert!(records.iter().all(|r| r["duration_ns"].as_u64().is_some()));
    }

    #[test]
    fn disabled_sink_does_not_allocate_spans_or_ids() {
        let sink = TraceSink::disabled();
        assert!(!sink.is_enabled());
        assert!(sink.next_command_run_id().is_none());
        assert!(
            sink.span(
                sink.root_context(),
                "ignored",
                TraceMode::Startup,
                Map::new()
            )
            .is_none()
        );
    }
}
