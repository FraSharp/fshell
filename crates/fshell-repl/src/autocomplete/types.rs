// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Native completion types and traits for fshell-repl.

/// Category tag for rich, structured completions in the FTUI popover grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CompletionKind {
    Builtin,
    UserFunction,
    ExternalCommand,
    Keyword,
    PipeOperator,
    Variable,
    File,
    Directory,
    Flag,
    HelpTopic,
    GitBranch,
    Job,
    Custom(&'static str),
}

impl CompletionKind {
    /// Concise badge label rendered in the completion popup menu.
    pub fn badge(self) -> &'static str {
        match self {
            CompletionKind::Builtin => "builtin",
            CompletionKind::UserFunction => "fn",
            CompletionKind::ExternalCommand => "cmd",
            CompletionKind::Keyword => "keyword",
            CompletionKind::PipeOperator => "pipe",
            CompletionKind::Variable => "var",
            CompletionKind::File => "file",
            CompletionKind::Directory => "dir",
            CompletionKind::Flag => "flag",
            CompletionKind::HelpTopic => "help",
            CompletionKind::GitBranch => "branch",
            CompletionKind::Job => "job",
            CompletionKind::Custom(s) => s,
        }
    }

    pub fn label(self) -> &'static str {
        self.badge()
    }

    pub fn append_whitespace(self) -> bool {
        match self {
            CompletionKind::Directory => false,
            CompletionKind::File => true,
            CompletionKind::Builtin
            | CompletionKind::UserFunction
            | CompletionKind::ExternalCommand
            | CompletionKind::Keyword
            | CompletionKind::Flag => true,
            _ => false,
        }
    }
}

/// Byte replacement span within the input buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextSpan {
    pub start: usize,
    pub end: usize,
}

impl TextSpan {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// Rich candidate item emitted by the completion engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionCandidate {
    pub value: String,
    pub display: Option<String>,
    pub description: Option<String>,
    pub kind: CompletionKind,
    pub span: TextSpan,
    pub match_indices: Option<Vec<usize>>,
}

impl CompletionCandidate {
    pub fn new(value: impl Into<String>, kind: CompletionKind, span: TextSpan) -> Self {
        Self {
            value: value.into(),
            display: None,
            description: None,
            kind,
            span,
            match_indices: None,
        }
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn with_display(mut self, display: impl Into<String>) -> Self {
        self.display = Some(display.into());
        self
    }
}

/// Request generation published by the input owner, independently of workers.
#[derive(Debug, Default)]
pub struct CompletionGeneration {
    current: std::sync::atomic::AtomicU64,
    changed: tokio::sync::Notify,
}

impl CompletionGeneration {
    pub fn advance(&self, generation: u64) {
        self.current
            .store(generation, std::sync::atomic::Ordering::Release);
        self.changed.notify_waiters();
    }
}

/// Cooperative cancellation for scans and notification-driven provider waits.
/// A blocking scan cannot be aborted; it must observe the generation published
/// at submission rather than waiting for its own worker to read the next request.
#[derive(Debug, Clone)]
pub struct CompletionCancel {
    generation: u64,
    current: std::sync::Arc<CompletionGeneration>,
}

impl CompletionCancel {
    pub fn new(generation: u64, current: std::sync::Arc<CompletionGeneration>) -> Self {
        Self {
            generation,
            current,
        }
    }

    /// A token that never cancels, for synchronous/test computes.
    pub fn never() -> Self {
        Self {
            generation: 0,
            current: std::sync::Arc::new(CompletionGeneration::default()),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.current
            .current
            .load(std::sync::atomic::Ordering::Acquire)
            != self.generation
    }

    pub async fn cancelled(&self) {
        loop {
            let notified = self.current.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// A provider may return fallback candidates before its asynchronous source is
/// ready. Those are display-only until a later, final batch is published.
#[derive(Default)]
pub(crate) struct CompletionBatch {
    pub candidates: Vec<CompletionCandidate>,
    pub pending: bool,
}

/// Core completer trait implemented by `FshellCompleter`.
pub trait Completer: Send + Sync {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<CompletionCandidate>;
}
