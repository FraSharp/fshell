// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use super::{Cell, Row, pen::Pen};
use std::collections::VecDeque;
use std::collections::vec_deque;

/// Scroll position information for the viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollPosition {
    /// Current viewport offset from bottom (0 = at bottom)
    pub offset: usize,
    /// Total number of lines in the grid (visible + scrollback)
    pub total_lines: usize,
    /// Number of visible lines in the viewport
    pub visible_lines: usize,
    /// Number of lines in scrollback (total_lines - visible_lines)
    pub scrollback_len: usize,
    /// Scroll percentage (0.0 = at top, 100.0 = at bottom)
    pub percentage: f64,
}

/// Zero-allocation iterator over the visible viewport rows.
/// Implements `Iterator` and `ExactSizeIterator` for use in hot render paths.
pub struct Viewport<'a> {
    iter: vec_deque::Iter<'a, Row>,
}

impl<'a> Iterator for Viewport<'a> {
    type Item = &'a Row;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl ExactSizeIterator for Viewport<'_> {}

#[derive(Debug)]
pub struct Grid {
    /// Visible rows + scrollback history
    rows: VecDeque<Row>,
    /// Number of visible rows
    height: usize,
    /// Number of columns
    width: usize,
    /// Maximum scrollback lines
    scrollback_limit: usize,
    /// Viewport offset from bottom (0 = most recent)
    viewport_offset: usize,
    /// Parser cursor position (viewport-relative)
    cursor_row: usize,
    cursor_col: usize,
    pub cursor_visible: bool,
    pub top_margin: usize,
    pub bottom_margin: usize,
    alt_rows: Option<VecDeque<Row>>,
    is_alt: bool,
    saved_cursor: (usize, usize),
    saved_alt_cursor: (usize, usize),
}

impl Grid {
    pub fn new(width: usize, height: usize, scrollback_limit: usize) -> Self {
        let mut rows = VecDeque::with_capacity(height + scrollback_limit);
        for _ in 0..height {
            rows.push_back(Row::new(width));
        }
        Self {
            rows,
            height,
            width,
            scrollback_limit,
            viewport_offset: 0,
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            top_margin: 0,
            bottom_margin: height.saturating_sub(1),
            alt_rows: None,
            is_alt: false,
            saved_cursor: (0, 0),
            saved_alt_cursor: (0, 0),
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn scrollback_limit(&self) -> usize {
        self.scrollback_limit
    }

    pub fn scrollback_len(&self) -> usize {
        self.rows.len().saturating_sub(self.height)
    }

    /// Get cursor position (viewport-relative)
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    /// Set cursor position (viewport-relative, col can equal width for deferred wrap)
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor_row = row.min(self.height.saturating_sub(1));
        self.cursor_col = col.min(self.width);
    }

    /// Scroll up into scrollback history by `n` lines.
    /// Viewport offset increases (further from the bottom).
    pub fn scroll_up_viewport(&mut self, n: usize) {
        let max_offset = self.rows.len().saturating_sub(self.height);
        self.viewport_offset = (self.viewport_offset + n).min(max_offset);
    }

    /// Scroll down toward the bottom of the viewport.
    /// Viewport offset decreases (closer to the bottom).
    pub fn scroll_down_viewport(&mut self, n: usize) {
        self.viewport_offset = self.viewport_offset.saturating_sub(n);
    }

    /// Whether the viewport is at the bottom (no scrollback visible).
    pub fn is_at_bottom(&self) -> bool {
        self.viewport_offset == 0
    }

    /// Get the current viewport offset from bottom.
    pub fn viewport_offset(&self) -> usize {
        self.viewport_offset
    }

    /// Scroll up by one full page (viewport height).
    /// Clamps to the maximum available scrollback.
    pub fn scroll_page_up(&mut self) {
        let max_offset = self.rows.len().saturating_sub(self.height);
        self.viewport_offset = (self.viewport_offset + self.height).min(max_offset);
    }

    /// Scroll down by one full page (viewport height).
    /// Clamps to zero (bottom of scrollback).
    pub fn scroll_page_down(&mut self) {
        self.viewport_offset = self.viewport_offset.saturating_sub(self.height);
    }

    /// Scroll to the very top of scrollback history.
    pub fn scroll_to_top(&mut self) {
        self.viewport_offset = self.rows.len().saturating_sub(self.height);
    }

    /// Scroll to the very bottom (live output).
    pub fn scroll_to_bottom(&mut self) {
        self.viewport_offset = 0;
    }

    /// Get current scroll position information.
    pub fn scroll_position(&self) -> ScrollPosition {
        let total_lines = self.rows.len();
        let scrollback_len = total_lines.saturating_sub(self.height);
        let max_offset = scrollback_len;
        let percentage = if max_offset == 0 {
            100.0
        } else {
            ((max_offset - self.viewport_offset) as f64 / max_offset as f64) * 100.0
        };
        ScrollPosition {
            offset: self.viewport_offset,
            total_lines,
            visible_lines: self.height,
            scrollback_len,
            percentage,
        }
    }

    /// Zero-allocation iterator over the visible viewport rows.
    /// Use this in hot render paths to avoid heap allocation.
    pub fn viewport_iter(&self) -> Viewport<'_> {
        let base = self.rows.len().saturating_sub(self.height);
        let start = base.saturating_sub(self.viewport_offset);
        let end = (start + self.height).min(self.rows.len());
        Viewport {
            iter: self.rows.range(start..end),
        }
    }

    /// Get a reference to the visible viewport rows (allocating).
    /// Prefer `viewport_iter()` in performance-sensitive code.
    pub fn viewport(&self) -> Vec<&Row> {
        let base = self.rows.len().saturating_sub(self.height);
        let start = base.saturating_sub(self.viewport_offset);
        let end = (start + self.height).min(self.rows.len());
        self.rows.range(start..end).collect()
    }

    /// Get mutable reference to a viewport row (0 = top of visible area)
    pub fn row_mut(&mut self, viewport_row: usize) -> Option<&mut Row> {
        if viewport_row < self.height {
            let physical = self.rows.len().saturating_sub(self.height) + viewport_row;
            self.rows.get_mut(physical)
        } else {
            None
        }
    }

    /// Scroll down: adds blank row at bottom of margins.
    /// Pushes text UP (top line scrolls out).
    pub fn scroll_down(&mut self) {
        self.scroll_down_region(self.top_margin, self.bottom_margin);
    }

    /// Scroll down a specific region (from top row to bottom row).
    pub fn scroll_down_region(&mut self, top: usize, bottom: usize) {
        let top = top.min(self.height.saturating_sub(1));
        let bottom = bottom.min(self.height.saturating_sub(1));
        if top > bottom {
            return;
        }

        if top == 0 && bottom == self.height.saturating_sub(1) {
            // Full screen scroll: push/pop deque to preserve history
            if self.rows.len() >= self.height + self.scrollback_limit {
                self.rows.pop_front();
            }
            let mut new_row = Row::new(self.width);
            new_row.mark_all_dirty();
            self.rows.push_back(new_row);
        } else {
            // Sub-region scroll: shift rows in place, no scrollback history
            let base = self.rows.len().saturating_sub(self.height);
            let physical_top = base + top;
            let physical_bottom = base + bottom;

            if physical_top < self.rows.len()
                && physical_bottom < self.rows.len()
                && physical_top <= physical_bottom
            {
                self.rows.remove(physical_top);
                let mut new_row = Row::new(self.width);
                new_row.mark_all_dirty();
                self.rows.insert(physical_bottom, new_row);
            }
        }
    }

    /// Scroll up: adds blank row at top of margins.
    /// Pushes text DOWN (bottom line scrolls out).
    pub fn scroll_up(&mut self) {
        self.scroll_up_region(self.top_margin, self.bottom_margin);
    }

    /// Scroll up a specific region (from top row to bottom row).
    pub fn scroll_up_region(&mut self, top: usize, bottom: usize) {
        let top = top.min(self.height.saturating_sub(1));
        let bottom = bottom.min(self.height.saturating_sub(1));
        if top > bottom {
            return;
        }

        let base = self.rows.len().saturating_sub(self.height);
        let physical_top = base + top;
        let physical_bottom = base + bottom;

        if physical_top < self.rows.len()
            && physical_bottom < self.rows.len()
            && physical_top <= physical_bottom
        {
            self.rows.remove(physical_bottom);
            let mut new_row = Row::new(self.width);
            new_row.mark_all_dirty();
            self.rows.insert(physical_top, new_row);
        }
    }

    pub fn set_margins(&mut self, top: usize, bottom: usize) {
        self.top_margin = top.min(self.height.saturating_sub(1));
        self.bottom_margin = bottom.min(self.height.saturating_sub(1));
        if self.top_margin > self.bottom_margin {
            std::mem::swap(&mut self.top_margin, &mut self.bottom_margin);
        }
    }

    pub fn rows_len(&self) -> usize {
        self.rows.len()
    }

    pub fn remove_row(&mut self, physical_idx: usize) {
        if physical_idx < self.rows.len() {
            self.rows.remove(physical_idx);
        }
    }

    pub fn insert_row(&mut self, physical_idx: usize, row: Row) {
        if physical_idx <= self.rows.len() {
            self.rows.insert(physical_idx, row);
        }
    }

    pub fn set_alt_buffer(&mut self, enabled: bool) {
        if enabled == self.is_alt {
            return;
        }

        if enabled {
            self.saved_cursor = (self.cursor_row, self.cursor_col);
            if self.alt_rows.is_none() {
                let mut alt = VecDeque::with_capacity(self.height);
                for _ in 0..self.height {
                    alt.push_back(Row::new(self.width));
                }
                self.alt_rows = Some(alt);
            }
            std::mem::swap(
                &mut self.rows,
                self.alt_rows
                    .as_mut()
                    .expect("alt_rows just initialized is None"),
            );
            self.cursor_row = self.saved_alt_cursor.0;
            self.cursor_col = self.saved_alt_cursor.1;
            self.is_alt = true;
        } else {
            self.saved_alt_cursor = (self.cursor_row, self.cursor_col);
            if let Some(ref mut alt) = self.alt_rows {
                std::mem::swap(&mut self.rows, alt);
            }
            self.cursor_row = self.saved_cursor.0;
            self.cursor_col = self.saved_cursor.1;
            self.is_alt = false;
        }
    }

    /// Insert a blank row at the current cursor position.
    /// All visible rows from cursor down are marked dirty.
    pub fn insert_line(&mut self) {
        let physical = self.rows.len().saturating_sub(self.height) + self.cursor_row;
        self.rows.insert(physical, Row::new(self.width));
        if self.rows.len() > self.height + self.scrollback_limit {
            self.rows.pop_front();
        }
        // Mark all visible rows from cursor down as dirty
        for i in self.cursor_row..self.height {
            if let Some(row) = self.row_mut(i) {
                row.mark_all_dirty();
            }
        }
    }

    /// Write a string at the current cursor position, advancing cursor.
    ///
    /// Uses `Pen::default()` for all characters (no styling).
    /// For PTY output with active SGR styling, use `GridParser::process()` instead.
    pub fn write_str(&mut self, s: &str) {
        use unicode_width::UnicodeWidthChar;

        for ch in s.chars() {
            match ch {
                '\n' => {
                    self.cursor_col = 0;
                    if self.cursor_row + 1 >= self.height {
                        self.scroll_down();
                    } else {
                        self.cursor_row += 1;
                    }
                }
                '\r' => {
                    self.cursor_col = 0;
                }
                '\t' => {
                    // Tab stops every 8 columns
                    self.cursor_col = (self.cursor_col / 8 + 1) * 8;
                    if self.cursor_col >= self.width {
                        self.cursor_col = 0;
                        self.scroll_down();
                        self.cursor_row = (self.cursor_row + 1).min(self.height.saturating_sub(1));
                    }
                }
                _ => {
                    let char_width = UnicodeWidthChar::width(ch).unwrap_or(1);
                    if char_width == 0 {
                        continue; // Zero-width chars skip
                    }

                    // Wrap if needed
                    if self.cursor_col + char_width > self.width {
                        self.cursor_col = 0;
                        self.scroll_down();
                        self.cursor_row = (self.cursor_row + 1).min(self.height.saturating_sub(1));
                    }

                    // Write the character — copy values to avoid borrow conflicts
                    let col = self.cursor_col;
                    let row_idx = self.cursor_row;
                    let w = self.width;
                    if let Some(row) = self.row_mut(row_idx) {
                        row.set_cell(
                            col,
                            Cell {
                                character: ch,
                                pen: Pen::default(),
                                wide_continuation: false,
                            },
                        );

                        // For double-width characters, write continuation marker
                        if char_width > 1 && col + 1 < w {
                            row.set_cell(
                                col + 1,
                                Cell {
                                    character: '\0',
                                    pen: Pen::default(),
                                    wide_continuation: true,
                                },
                            );
                        }
                    }

                    self.cursor_col += char_width;
                }
            }
        }
    }

    /// Resize the grid to new dimensions.
    ///
    /// - Width change: adjusts column count in all existing rows.
    /// - Height shrink: excess visible rows become scrollback history for primary screen.
    /// - Height grow: adds new blank rows at the bottom.
    pub fn resize(&mut self, new_width: usize, new_height: usize) {
        let new_width = new_width.max(1);
        let new_height = new_height.max(1);

        self.width = new_width;
        self.height = new_height;

        // Resize all existing rows in active buffer
        for row in &mut self.rows {
            row.resize(new_width);
        }

        // Resize all existing rows in alternate buffer if it exists
        if let Some(ref mut alt) = self.alt_rows {
            for row in alt {
                row.resize(new_width);
            }
        }

        // Adjust heights of buffers
        if self.is_alt {
            // Active buffer is alt buffer: should be exactly new_height rows
            while self.rows.len() < new_height {
                self.rows.push_back(Row::new(new_width));
            }
            self.rows.truncate(new_height);

            // Alternate buffer is primary buffer: ensure at least new_height rows
            if let Some(ref mut primary) = self.alt_rows {
                while primary.len() < new_height {
                    primary.push_back(Row::new(new_width));
                }
            }
        } else {
            // Active buffer is primary buffer: ensure at least new_height rows
            while self.rows.len() < new_height {
                self.rows.push_back(Row::new(new_width));
            }

            // Alternate buffer is alt buffer: should be exactly new_height rows
            if let Some(ref mut alt) = self.alt_rows {
                while alt.len() < new_height {
                    alt.push_back(Row::new(new_width));
                }
                alt.truncate(new_height);
            }
        }

        // Ensure margins are kept valid after resize
        self.top_margin = self.top_margin.min(self.height.saturating_sub(1));
        self.bottom_margin = self.bottom_margin.min(self.height.saturating_sub(1));
        if self.top_margin > self.bottom_margin {
            std::mem::swap(&mut self.top_margin, &mut self.bottom_margin);
        }

        // Ensure cursor stays in bounds
        self.cursor_row = self.cursor_row.min(self.height.saturating_sub(1));
        self.cursor_col = self.cursor_col.min(self.width.saturating_sub(1));
    }
}
