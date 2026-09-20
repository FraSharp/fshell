// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Scoped terminal mode for fullscreen sub-interfaces.
//!
//! Fullscreen helpers can be entered from either a cooked shell or the FTUI
//! session. They must restore the state they found instead of assuming that
//! they own raw mode for the entire process.

use std::io;

use crossterm::{
    cursor::{Hide, Show},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};

pub(crate) struct FullscreenTerminalGuard {
    raw_mode_was_enabled: bool,
    cursor_was_hidden: bool,
}

impl FullscreenTerminalGuard {
    pub(crate) fn enter(hide_cursor: bool) -> io::Result<Self> {
        let raw_mode_was_enabled = terminal::is_raw_mode_enabled()?;
        if !raw_mode_was_enabled {
            terminal::enable_raw_mode()?;
        }

        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen) {
            if !raw_mode_was_enabled {
                let _ = terminal::disable_raw_mode();
            }
            return Err(error);
        }

        if hide_cursor && let Err(error) = execute!(io::stdout(), Hide) {
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            if !raw_mode_was_enabled {
                let _ = terminal::disable_raw_mode();
            }
            return Err(error);
        }

        Ok(Self {
            raw_mode_was_enabled,
            cursor_was_hidden: hide_cursor,
        })
    }
}

impl Drop for FullscreenTerminalGuard {
    fn drop(&mut self) {
        if self.cursor_was_hidden {
            let _ = execute!(io::stdout(), Show);
        }
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        if !self.raw_mode_was_enabled {
            let _ = terminal::disable_raw_mode();
        }
    }
}
