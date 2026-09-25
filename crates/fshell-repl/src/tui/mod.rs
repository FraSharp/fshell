// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Unified TUI design system and component architecture for fshell.

pub mod components;
pub mod theme;

pub use crate::terminal_mode::FullscreenTerminalGuard;
pub use components::*;
pub use theme::*;
