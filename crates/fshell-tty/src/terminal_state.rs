// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Cross-crate notifications for terminal cursor knowledge.
//!
//! The REPL owns the current origin estimate. TTY-owning execution paths use
//! this generation to report that they changed the terminal outside that
//! renderer, so the next prompt can establish a fresh origin.

use std::sync::atomic::{AtomicU64, Ordering};

static CURSOR_STATE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Current generation of terminal changes that make the cursor column opaque.
pub fn cursor_state_generation() -> u64 {
    CURSOR_STATE_GENERATION.load(Ordering::Relaxed)
}

/// Report that output or a child process may have moved the terminal cursor in
/// a way fshell did not track.
pub fn mark_cursor_state_unknown() {
    CURSOR_STATE_GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Invalidate cursor knowledge when stdout output does not end at column zero.
///
/// A trailing line feed resets the terminal column under the cooked TTY mode
/// used for command output. Non-terminal and empty writes leave the state
/// unchanged.
pub fn mark_cursor_state_unknown_for_stdout_output(bytes: &[u8]) {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() && !bytes.is_empty() && !bytes.ends_with(b"\n") {
        mark_cursor_state_unknown();
    }
}

/// Invalidate cursor knowledge when stderr output does not end at column zero.
pub fn mark_cursor_state_unknown_for_stderr_output(bytes: &[u8]) {
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() && !bytes.is_empty() && !bytes.ends_with(b"\n") {
        mark_cursor_state_unknown();
    }
}
