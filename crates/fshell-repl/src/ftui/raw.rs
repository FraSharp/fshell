// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! REPL terminal-session policy.
//!
//! The mechanics live in `fshell-tty`; this module fixes the REPL's policy
//! (refuse raw mode under the test harness, keep every auxiliary mode on for
//! the whole session) and re-exports the guards `ftui` installs.

use std::io;

use fshell_terminal::{RawSession, RawSessionModes};

pub use fshell_terminal::{PanicHookGuard, SignalGuard};

/// The REPL keeps every auxiliary mode on for the whole session.
const MODES: RawSessionModes = RawSessionModes {
    bracketed_paste: true,
    focus_change: true,
    mouse: true,
    disable_blinking: true,
};

/// Enter the session-owned raw state, unless the test harness forbids it.
pub fn enter_session() -> io::Result<RawSession> {
    if fshell_engine::is_test_mode() {
        return Err(io::Error::other("refusing raw mode in test mode"));
    }
    RawSession::enter(MODES)
}
