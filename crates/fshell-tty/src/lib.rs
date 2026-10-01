// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Fshell-owned Unix terminal primitives.
//!
//! Owns input decoding and normalization, raw-mode and termios handling, the
//! ANSI byte vocabulary, cursor and size queries, and the process-level
//! lifecycle guards. Rendering on top of these primitives (the ratatui
//! backend, scoped sessions, the TUI runner) lives in `fshell-terminal`,
//! which re-exports this crate.

pub mod ansi;
mod inbox;
pub mod input;
pub mod lifecycle;
pub mod parse;
pub mod raw;
pub mod session;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod test_support;
#[cfg(unix)]
pub mod unix;

pub use ansi::*;
pub use input::*;
pub use lifecycle::*;
pub use raw::*;
pub use session::*;
