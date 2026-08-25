// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use super::Grid;

/// Handles line reflow when the grid is resized.
///
/// This is the foundation for resize handling. A full implementation would
/// track wrap points per line and reflow content across line boundaries.
/// For now, it delegates to `Grid::resize` which adjusts dimensions
/// without reflowing wrapped content.
pub struct Reflow<'a> {
    grid: &'a mut Grid,
}

impl<'a> Reflow<'a> {
    pub fn new(grid: &'a mut Grid) -> Self {
        Self { grid }
    }

    /// Reflow content to the new width.
    ///
    /// Currently a thin wrapper around `Grid::resize`. Future versions
    /// will unwrap wrapped lines and re-wrap them at the new width.
    pub fn reflow(&mut self, new_width: usize) {
        self.grid.resize(new_width, self.grid.height());
    }
}
