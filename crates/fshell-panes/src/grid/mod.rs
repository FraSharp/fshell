// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Terminal Grid & State Engine
//!
//! Core data structures for representing terminal state, including:
//!
//! - [`Cell`]: A single character with styling (`Copy`, wide-char aware)
//! - [`Row`]: A horizontal line of cells with column-range dirty tracking
//! - [`Grid`]: The full terminal buffer with circular scrollback
//! - [`Pen`]: SGR attributes (`Copy`, all standard colors)
//! - [`parser::GridParser`]: VTE-powered parser with full SGR support
//! - [`widget::GridWidget`]: Ratatui widget with zero-allocation rendering
//! - [`reflow::Reflow`]: Resize handling foundation

pub mod cell;
pub mod parser;
pub mod pen;
pub mod reflow;
pub mod row;
pub mod scrollback;
pub mod widget;

pub use cell::Cell;
pub use pen::Pen;
pub use row::Row;
pub use scrollback::{Grid, ScrollPosition};
