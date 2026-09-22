// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Errors produced by the semantic layer.

/// An error raised while parsing, validating, lowering or rendering a semantic action.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SemanticError {
    /// A value could not be parsed (byte size, time span, ...).
    #[error("invalid value: {0}")]
    InvalidValue(String),
    /// The action kind is not known to the semantic layer.
    #[error("unknown action kind: {0}")]
    UnknownKind(String),
    /// The action cannot be lowered for the requested target/platform.
    #[error("cannot lower {kind} for target {target}: {reason}")]
    Unsupported {
        kind: String,
        target: String,
        reason: String,
    },
}
