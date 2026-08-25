// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Session-owned raw mode.
//!
//! Long-term A2: raw mode is a session property, not a per-command toggle.
//! The terminal spends the whole interactive session in raw + mouse +
//! bracketed-paste. Only a [`SuspendGuard`] — held for the duration of a
//! command that actually needs a PTY — temporarily drops back to cooked.
//!
//! Everything else (ls, cat, builtins, piped output) runs while raw
//! stays on, with output flowing through `CaptureGuard` into the anchored
//! pane. That eliminates the `\n` → missing `\r\n` smear and the 5
//! scattered `enable/disable_raw_mode` callsites.

use std::io::Write;

/// Owner of raw mode for the life of `run_ftui_repl`.
///
/// Exactly one exists per REPL session. Dropping it restores the terminal
/// even if the task panics. Nothing else in `ftui` should call
/// `enable_raw_mode`/`disable_raw_mode` directly.
pub struct Session {
    // Private so only this module can construct / drop.
    _private: (),
}

/// Borrowed guard that temporarily drops the session back to cooked.
///
/// Created by [`Session::suspend`]. While this guard is alive the child
/// process sees a normal cooked terminal (echo, icannon, onlcr). When the
/// guard drops — including on panic/unwind — raw mode and all auxiliary
/// modes are reinstalled in the correct order and flushed.
pub struct SuspendGuard<'a> {
    session: &'a Session,
    armed: bool,
}

impl Session {
    /// Enter raw + auxiliary modes once for the session.
    ///
    /// Returns an error instead of panicking so `run_ftui_repl` can exit
    /// gracefully if the terminal cannot enter raw mode (e.g. not a tty).
    pub fn enter() -> std::io::Result<Self> {
        if fshell_engine::is_test_mode() {
            return Err(std::io::Error::other("refusing raw mode in test mode"));
        }
        crossterm::terminal::enable_raw_mode()?;
        let mut out = std::io::stdout();
        crossterm::execute!(
            out,
            crossterm::cursor::DisableBlinking,
            crossterm::event::EnableBracketedPaste,
            crossterm::event::EnableFocusChange,
            crossterm::event::EnableMouseCapture,
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            ),
        )?;
        out.flush()?;
        Ok(Self { _private: () })
    }

    /// Suspend raw for the duration of a command that needs a real PTY
    /// (vim, less, ssh, fzf, …). The returned guard re-enables on drop.
    pub fn suspend(&self) -> std::io::Result<SuspendGuard<'_>> {
        // Order matters: flush any pending ratatui draws, leave auxiliary
        // modes before leaving raw, then flush so the child's first
        // `tcsetattr(TCSAFLUSH)` sees a clean queue.
        let mut out = std::io::stdout();
        out.flush()?;
        // These three are no-ops if the terminal doesn't support them, but
        // leaving them on leaks mouse escapes into the child's stdin.
        let _ = crossterm::execute!(
            out,
            crossterm::event::PopKeyboardEnhancementFlags,
            crossterm::event::DisableBracketedPaste,
            crossterm::event::DisableFocusChange,
            crossterm::event::DisableMouseCapture,
            crossterm::cursor::Show,
            crossterm::cursor::EnableBlinking,
        );
        out.flush()?;
        crossterm::terminal::disable_raw_mode()?;
        Ok(SuspendGuard {
            session: self,
            armed: true,
        })
    }

    /// Explicit re-arm without a suspend — used after SIGTSTP resume where
    /// the kernel may have reset termios behind us. Idempotent.
    pub(crate) fn reenter_raw(&self) {
        let _ = crossterm::terminal::enable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::cursor::DisableBlinking,
            crossterm::event::EnableBracketedPaste,
            crossterm::event::EnableFocusChange,
            crossterm::event::EnableMouseCapture,
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            ),
        );
        let _ = std::io::stdout().flush();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Best-effort even during panic unwind. Order mirrors `TuiGuard`
        // but with a final flush so the shell that regains the tty isn't
        // left with pending escape sequences.
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown),
            crossterm::event::PopKeyboardEnhancementFlags,
            crossterm::event::DisableBracketedPaste,
            crossterm::event::DisableFocusChange,
            crossterm::event::DisableMouseCapture,
            crossterm::cursor::Show,
            crossterm::cursor::EnableBlinking,
        );
        let _ = std::io::stdout().flush();
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

impl<'a> Drop for SuspendGuard<'a> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Re-enter exactly the state `Session::enter` established.
        self.session.reenter_raw();
    }
}

impl<'a> SuspendGuard<'a> {
    /// Disarm — don't re-enter raw on drop. Used when the repl is exiting
    /// and `Session` itself will do the final restore.
    #[allow(dead_code)]
    pub fn disarm(mut self) {
        self.armed = false;
    }
}
