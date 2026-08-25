// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use super::Cell;

#[derive(Debug, Clone)]
pub struct Row {
    cells: Vec<Cell>,
    /// Column range of dirty cells: (left, right) inclusive
    /// None = clean, Some((l, r)) = columns l..=r need re-render
    dirty_range: Option<(u16, u16)>,
}

impl Row {
    pub fn new(width: usize) -> Self {
        Self {
            cells: vec![Cell::default(); width],
            dirty_range: None,
        }
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty_range.is_some()
    }

    /// Get the dirty column range (inclusive)
    pub fn dirty_range(&self) -> Option<(u16, u16)> {
        self.dirty_range
    }

    /// Mark a column range as dirty, merging with existing dirty range
    pub fn mark_dirty(&mut self, left: u16, right: u16) {
        match self.dirty_range {
            Some((existing_left, existing_right)) => {
                let new_left = existing_left.min(left);
                let new_right = existing_right.max(right);
                self.dirty_range = Some((new_left, new_right));
            }
            None => {
                self.dirty_range = Some((left, right));
            }
        }
    }

    /// Mark entire row as dirty
    pub fn mark_all_dirty(&mut self) {
        if !self.cells.is_empty() {
            self.dirty_range = Some((0, (self.cells.len() - 1) as u16));
        }
    }

    pub fn clear_dirty(&mut self) {
        self.dirty_range = None;
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn cells_mut(&mut self, left: usize, right: usize) -> &mut [Cell] {
        assert!(
            left <= right,
            "cells_mut: left ({left}) must be <= right ({right})"
        );
        assert!(
            right < self.cells.len(),
            "cells_mut: right ({right}) out of bounds (len={})",
            self.cells.len()
        );
        self.mark_dirty(left as u16, right as u16);
        &mut self.cells[left..=right]
    }

    /// Set a cell, handling wide character overwrite protection.
    ///
    /// When overwriting a wide character, clears both the preceding
    /// continuation (if overwriting a continuation cell) AND the
    /// following continuation (if overwriting the wide char itself).
    pub fn set_cell(&mut self, col: usize, cell: Cell) {
        if col < self.cells.len() {
            // If overwriting a continuation cell, clear the preceding wide char
            if col > 0 && self.cells[col].wide_continuation {
                self.cells[col - 1] = Cell::default();
            }

            // If overwriting a wide char, clear the following continuation
            if col + 1 < self.cells.len() && self.cells[col].is_wide() {
                self.cells[col + 1] = Cell::default();
            }

            self.cells[col] = cell;
            self.mark_dirty(col as u16, col as u16);
        }
    }

    /// Resize the row to a new width, padding with default cells or truncating.
    pub fn resize(&mut self, new_width: usize) {
        let old_width = self.cells.len();
        if new_width == old_width {
            return;
        }
        self.cells.resize(new_width, Cell::default());

        if let Some((l, r)) = self.dirty_range {
            let limit = (new_width.saturating_sub(1)) as u16;
            if l > limit {
                self.dirty_range = None;
            } else {
                self.dirty_range = Some((l, r.min(limit)));
            }
        }

        if new_width > old_width {
            self.mark_dirty(old_width as u16, (new_width - 1) as u16);
        }
    }
}
