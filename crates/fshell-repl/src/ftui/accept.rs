// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! The Enter-key decision: submit the buffer, or continue the multi-line edit.
//!
//! One function owns the rule so every path that accepts a line — the
//! `accept-line` widget, the fallback Enter arm, and the tests — makes the same
//! choice with the same indentation. Soft-newline paths share
//! [`insert_newline_and_indent`] too.

use super::buffer::TextBuffer;
use fshell_engine::frontend::{InputClass, classify_input};

/// Columns added for each open brace when a newline is inserted.
pub(crate) const INDENT_WIDTH: usize = 4;

/// What the editor should do after an Enter press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AcceptOutcome {
    /// Run this command and leave the editor.
    Execute(String),
    /// A newline was inserted; stay in the editor.
    Continue,
    /// The buffer was cleared because a blank line cancelled a continuation.
    ClearContinuation,
    /// The line was empty: commit the prompt and start a fresh one.
    EmptySubmit,
}

/// Insert a newline at the cursor and indent to the current block depth.
///
/// The depth is measured at the cursor, not over the whole buffer: in a
/// multi-line edit the text after the cursor must not affect the indentation.
pub(crate) fn insert_newline_and_indent(text_buf: &mut TextBuffer) {
    let indent = fshell_core::compute_indent_depth_at(&text_buf.text(), Some(text_buf.cursor()));
    text_buf.insert_char('\n');
    for _ in 0..(indent * INDENT_WIDTH) {
        text_buf.insert_char(' ');
    }
}

/// Decide what Enter does for the current buffer, updating the continuation
/// flag and buffer exactly as the decision requires.
pub(crate) fn accept_line(text_buf: &mut TextBuffer, in_continuation: &mut bool) -> AcceptOutcome {
    text_buf.commit_transaction();
    let command_line = text_buf.text();

    if command_line.trim().is_empty() {
        text_buf.clear();
        if *in_continuation {
            *in_continuation = false;
            return AcceptOutcome::ClearContinuation;
        }
        return AcceptOutcome::EmptySubmit;
    }

    let is_last_line = {
        let chars = text_buf.chars();
        let cursor = text_buf.cursor();
        !chars.iter().skip(cursor).any(|&c| c == '\n')
    };

    match classify_input(&command_line) {
        // Incomplete input continues. This is the only place that decides it.
        InputClass::Incomplete { .. } => {
            insert_newline_and_indent(text_buf);
            *in_continuation = true;
            AcceptOutcome::Continue
        }
        // Inside a multi-line buffer, Enter on an earlier line splits the line
        // at the cursor instead of submitting the whole buffer.
        _ if !is_last_line && command_line.contains('\n') => {
            insert_newline_and_indent(text_buf);
            *in_continuation = true;
            AcceptOutcome::Continue
        }
        // Complete or definitively invalid: submit it so the diagnostic is
        // shown against the user's text.
        _ => {
            *in_continuation = false;
            AcceptOutcome::Execute(command_line.trim().to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> TextBuffer {
        let mut buf = TextBuffer::new();
        buf.insert_str(text);
        buf
    }

    #[test]
    fn incomplete_block_continues_with_indented_newline() {
        let mut buf = buffer("if true {");
        let mut in_continuation = false;

        let outcome = accept_line(&mut buf, &mut in_continuation);

        assert_eq!(outcome, AcceptOutcome::Continue);
        assert!(in_continuation);
        assert_eq!(buf.text(), "if true {\n    ");
    }

    #[test]
    fn heredoc_continues_without_indentation() {
        let mut buf = buffer("cat <<EOF");
        let mut in_continuation = false;

        let outcome = accept_line(&mut buf, &mut in_continuation);

        assert_eq!(outcome, AcceptOutcome::Continue);
        assert_eq!(buf.text(), "cat <<EOF\n");
    }

    #[test]
    fn triple_quoted_string_continues() {
        let mut buf = buffer("let q = \"\"\"");
        let mut in_continuation = false;

        let outcome = accept_line(&mut buf, &mut in_continuation);

        assert_eq!(outcome, AcceptOutcome::Continue);
        assert_eq!(buf.text(), "let q = \"\"\"\n");
    }

    #[test]
    fn complete_line_executes() {
        let mut buf = buffer("echo hi");
        let mut in_continuation = false;

        assert_eq!(
            accept_line(&mut buf, &mut in_continuation),
            AcceptOutcome::Execute("echo hi".to_string())
        );
        assert!(!in_continuation);
    }

    #[test]
    fn enter_before_the_last_line_splits_instead_of_submitting() {
        let mut buf = buffer("echo one\necho two");
        buf.set_cursor(4);
        let mut in_continuation = false;

        let outcome = accept_line(&mut buf, &mut in_continuation);

        assert_eq!(outcome, AcceptOutcome::Continue);
        assert_eq!(buf.text(), "echo\n one\necho two");
        assert!(in_continuation);
    }

    #[test]
    fn empty_line_submits_a_fresh_prompt() {
        let mut buf = TextBuffer::new();
        let mut in_continuation = false;

        assert_eq!(
            accept_line(&mut buf, &mut in_continuation),
            AcceptOutcome::EmptySubmit
        );
    }

    #[test]
    fn empty_line_cancels_a_continuation() {
        let mut buf = TextBuffer::new();
        let mut in_continuation = true;

        assert_eq!(
            accept_line(&mut buf, &mut in_continuation),
            AcceptOutcome::ClearContinuation
        );
        assert!(!in_continuation);
    }
}
