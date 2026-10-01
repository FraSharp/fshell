// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Fshell-owned terminal runtime boundary.
//!
//! Owns the ratatui backend, scoped terminal sessions (fullscreen and
//! inline), and the unified TUI runner. The underlying terminal primitives
//! live in `fshell-tty` and are re-exported here for callers.

pub mod backend;
pub mod runner;
pub mod session;

pub use backend::*;
pub use fshell_tty::*;
pub use runner::*;
pub use session::*;
