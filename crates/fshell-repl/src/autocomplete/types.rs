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

/// Core completer trait implemented by `FshellCompleter`.
pub trait Completer: Send + Sync {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<CompletionCandidate>;
}
