// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Standardized interactive TUI components for fshell.

pub mod data_grid;
pub mod modal_dialog;
pub mod scrollable_pane;
pub mod search_bar;
pub mod status_footer;

pub use data_grid::{DataGrid, DataGridState, SortDirection};
pub use modal_dialog::{centered_fixed, centered_percent, render_modal_frame};
pub use scrollable_pane::ScrollState;
pub use search_bar::SearchBarState;
pub use status_footer::{KeyHint, StatusFooter};
