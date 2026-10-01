// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Session-owned raw mode with suspend and resume for child processes.
//!
//! An interactive application holds raw input plus its auxiliary modes for
//! its whole lifetime, and drops back to cooked input around commands that
//! need a real terminal (editors, pagers, ssh). Keeping the transitions here
//! means every caller restores exactly what it changed, in one order.

use std::io::Write;

use crate::ansi;
use crate::lifecycle::emergency_restore_terminal;
use crate::raw::{disable_raw_mode, enable_raw_mode};

/// Auxiliary modes an interactive session keeps enabled alongside raw input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawSessionModes {
    pub bracketed_paste: bool,
    pub focus_change: bool,
    pub mouse: bool,
    pub disable_blinking: bool,
}

impl Default for RawSessionModes {
    fn default() -> Self {
        Self {
            bracketed_paste: true,
            focus_change: true,
            mouse: true,
            disable_blinking: true,
        }
    }
}

/// Owner of raw mode for the life of an interactive session.
///
/// Dropping it restores the terminal even if the task panics. Nothing else
/// should toggle raw mode while a session is alive.
pub struct RawSession {
    modes: RawSessionModes,
}

/// Borrowed guard that temporarily drops the session back to cooked.
///
/// While this guard is alive a child process sees a normal cooked terminal
/// (echo, icanon, onlcr). When it drops, including on panic/unwind, raw mode
/// and the session's auxiliary modes are reinstalled in order and flushed.
pub struct SuspendGuard<'a> {
    session: &'a RawSession,
    armed: bool,
}

impl RawSession {
    /// Enter raw mode plus the requested auxiliary modes.
    ///
    /// Returns an error instead of panicking so callers can exit gracefully
    /// when the terminal cannot enter raw mode (for example, not a tty).
    pub fn enter(modes: RawSessionModes) -> std::io::Result<Self> {
        enter_raw_mode(modes)?;
        Ok(Self { modes })
    }

    /// Suspend raw for the duration of a command that needs a real PTY
    /// (vim, less, ssh, fzf, …). The returned guard re-enables on drop.
    pub fn suspend(&self) -> std::io::Result<SuspendGuard<'_>> {
        enter_cooked_mode()?;
        Ok(SuspendGuard {
            session: self,
            armed: true,
        })
    }

    /// Re-enter the session's raw state after a suspend or a SIGTSTP resume,
    /// where the kernel may have reset termios behind us. Idempotent.
    pub fn reenter(&self) {
        let _ = enter_raw_mode(self.modes);
    }
}

impl Drop for RawSession {
    fn drop(&mut self) {
        emergency_restore_terminal();
    }
}

impl Drop for SuspendGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Re-enter exactly the state `RawSession::enter` established.
        self.session.reenter();
    }
}

impl SuspendGuard<'_> {
    /// Disarm: do not re-enter raw on drop. Used when the process is exiting
    /// and the session itself will do the final restore.
    #[allow(dead_code)]
    pub fn disarm(mut self) {
        self.armed = false;
    }
}

/// Enter the cooked input state for command execution without changing the
/// session's steady cursor policy. Full-screen children may set their own
/// cursor mode; the session restores its state when they return.
pub fn enter_cooked_mode() -> std::io::Result<()> {
    let mut out = std::io::stdout();
    out.flush()?;
    out.write_all(ansi::RESET_KEYBOARD_ENHANCEMENTS.as_bytes())?;
    out.write_all(ansi::DISABLE_BRACKETED_PASTE.as_bytes())?;
    out.write_all(ansi::DISABLE_FOCUS_CHANGE.as_bytes())?;
    out.write_all(ansi::DISABLE_MOUSE_CAPTURE.as_bytes())?;
    out.write_all(ansi::SHOW_CURSOR.as_bytes())?;
    out.flush()?;
    disable_raw_mode()
}

/// Enter raw mode plus `modes`, restoring the terminal if any write fails.
fn enter_raw_mode(modes: RawSessionModes) -> std::io::Result<()> {
    enable_raw_mode()?;
    let mut out = std::io::stdout();
    let result = (|| {
        // Drop any keyboard-enhancement state a previous program left on.
        out.write_all(ansi::RESET_KEYBOARD_ENHANCEMENTS.as_bytes())?;
        if modes.disable_blinking {
            out.write_all(ansi::DISABLE_BLINKING.as_bytes())?;
        }
        if modes.bracketed_paste {
            out.write_all(ansi::ENABLE_BRACKETED_PASTE.as_bytes())?;
        }
        if modes.focus_change {
            out.write_all(ansi::ENABLE_FOCUS_CHANGE.as_bytes())?;
        }
        if modes.mouse {
            out.write_all(ansi::ENABLE_MOUSE_CAPTURE.as_bytes())?;
        }
        out.flush()
    })();
    if result.is_err() {
        emergency_restore_terminal();
    }
    result
}
