// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Scoped terminal mode for fullscreen sub-interfaces.
//!
//! Fullscreen helpers can be entered from either a cooked shell or the FTUI
//! session. They must restore the state they found instead of assuming that
//! they own raw mode for the entire process.

use std::io;

use fshell_terminal::session::{TerminalDevice, TerminalMode, TerminalSession};

pub struct FullscreenTerminalGuard {
    _session: TerminalSession,
}

impl FullscreenTerminalGuard {
    pub fn enter(hide_cursor: bool) -> io::Result<Self> {
        let device = TerminalDevice::auto()?;
        let options = fshell_terminal::session::TerminalSessionOptions {
            mode: TerminalMode::Fullscreen,
            hide_cursor,
            ..Default::default()
        };
        let session = TerminalSession::enter(device, options).map_err(io::Error::other)?;
        Ok(Self { _session: session })
    }
}
