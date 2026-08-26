// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::Val;

/// Outcome of evaluating a statement.
///
/// Control-flow effects (`break`, `continue`, `return`, `exit`, logical
/// false) are values, not errors: they travel through the `Ok` channel of
/// `Result<Flow, EngineError>` so error handling (try/catch, rendering,
/// `$last_error`) can never confuse them with failures, and so control
/// decisions never inspect error text.
#[derive(Debug, Clone, PartialEq)]
pub enum Flow {
    /// Statement completed; execution continues.
    Normal,
    /// Logical `false` (a failed `test`, a filter that dropped everything):
    /// exit code 1, never rendered as an error line. Consumed by `&&`/`||`,
    /// `if`, and `$?` plumbing.
    ConditionFalse,
    /// `break` — caught by the innermost enclosing loop.
    Break,
    /// `continue` — caught by the innermost enclosing loop.
    Continue,
    /// `return <val>` — caught by the enclosing function body.
    Return(Val),
    /// `exit <code>` — propagates to the REPL or script driver.
    Exit(i32),
}

impl Flow {
    pub fn is_normal(&self) -> bool {
        matches!(self, Flow::Normal)
    }

    /// Display text for a control-flow signal that reached a context where
    /// it has no meaning (e.g. `break` at the top level of a script).
    /// `None` for outcomes that are not strays.
    pub fn stray_message(&self) -> Option<String> {
        match self {
            Flow::Normal | Flow::ConditionFalse => None,
            Flow::Break => Some("break".to_string()),
            Flow::Continue => Some("continue".to_string()),
            Flow::Return(_) => Some("return".to_string()),
            Flow::Exit(_) => Some("exit".to_string()),
        }
    }
}
