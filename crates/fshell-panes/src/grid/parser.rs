// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use super::{
    Cell, Grid, Row,
    pen::{Color, Pen},
};
use vte::{Parser, Perform};

/// Performs VTE actions on a Grid, updating an active pen style.
struct Performer<'a> {
    grid: &'a mut Grid,
    pen: &'a mut Pen,
    replies: &'a mut Vec<Vec<u8>>,
}

/// VTE parser that feeds escape sequences into a Grid.
///
/// Owns the VTE `Parser` and the active `Pen` style. The `process` method
/// borrows the grid separately, avoiding the double-mutable-borrow that
/// would occur if both lived in the same struct.
pub struct GridParser {
    parser: Parser,
    pen: Pen,
}

impl Default for GridParser {
    fn default() -> Self {
        Self::new()
    }
}

impl GridParser {
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
            pen: Pen::default(),
        }
    }

    /// Feed raw bytes through the VTE parser.  Escape sequences update the
    /// active pen; printable characters are written to the grid at the
    /// current cursor position. Returns any PTY response bytes (e.g. DSR).
    pub fn process(&mut self, grid: &mut Grid, data: &[u8]) -> Vec<Vec<u8>> {
        let GridParser { parser, pen } = self;
        let mut replies = Vec::new();
        let mut performer = Performer {
            grid,
            pen,
            replies: &mut replies,
        };
        parser.advance(&mut performer, data);
        replies
    }

    /// Current pen style (updated by SGR sequences).
    pub fn pen(&self) -> &Pen {
        &self.pen
    }

    /// Mutable access to the pen (e.g. for manual resets between parses).
    pub fn pen_mut(&mut self) -> &mut Pen {
        &mut self.pen
    }
}

// Perform impl

impl Perform for Performer<'_> {
    fn print(&mut self, ch: char) {
        use unicode_width::UnicodeWidthChar;

        let char_width = UnicodeWidthChar::width(ch).unwrap_or(1);
        if char_width == 0 {
            return;
        }

        let (row, col) = self.grid.cursor();
        let width = self.grid.width();

        // If the cursor is at/past the right edge or char doesn't fit, wrap to next line
        let (actual_row, actual_col) = if col >= width || col + char_width > width {
            let next_row = row + 1;
            let bottom = self.grid.bottom_margin;
            if row == bottom {
                self.grid.scroll_down();
                (row, 0)
            } else if next_row >= self.grid.height() {
                self.grid
                    .scroll_down_region(0, self.grid.height().saturating_sub(1));
                (row, 0)
            } else {
                (next_row, 0)
            }
        } else {
            (row, col)
        };

        // Write character
        let w = self.grid.width();
        let pen = *self.pen;
        if let Some(r) = self.grid.row_mut(actual_row) {
            r.set_cell(
                actual_col,
                Cell {
                    character: ch,
                    pen,
                    wide_continuation: false,
                },
            );

            // Double-width: write continuation marker
            if char_width > 1 && actual_col + 1 < w {
                r.set_cell(
                    actual_col + 1,
                    Cell {
                        character: '\0',
                        pen,
                        wide_continuation: true,
                    },
                );
            }
        }

        // Advance cursor
        self.grid.set_cursor(actual_row, actual_col + char_width);
    }

    fn execute(&mut self, byte: u8) {
        let (row, _col) = self.grid.cursor();
        match byte {
            b'\n' => {
                let bottom = self.grid.bottom_margin;
                if row == bottom {
                    self.grid.scroll_down();
                    self.grid.set_cursor(row, 0);
                } else if row + 1 >= self.grid.height() {
                    self.grid
                        .scroll_down_region(0, self.grid.height().saturating_sub(1));
                    self.grid.set_cursor(row, 0);
                } else {
                    self.grid.set_cursor(row + 1, 0);
                }
            }
            b'\r' => {
                self.grid.set_cursor(row, 0);
            }
            b'\t' => {
                let (r, col) = self.grid.cursor();
                let new_col = (col / 8 + 1) * 8;
                if new_col >= self.grid.width() {
                    // Tab wraps past end of line — scroll if at bottom of margins
                    let bottom = self.grid.bottom_margin;
                    if r == bottom {
                        self.grid.scroll_down();
                        self.grid.set_cursor(r, 0);
                    } else if r + 1 >= self.grid.height() {
                        self.grid
                            .scroll_down_region(0, self.grid.height().saturating_sub(1));
                        self.grid.set_cursor(r, 0);
                    } else {
                        self.grid.set_cursor(r + 1, 0);
                    }
                } else {
                    self.grid.set_cursor(r, new_col);
                }
            }
            b'\x08' => {
                // Backspace
                let (r, c) = self.grid.cursor();
                if c > 0 {
                    self.grid.set_cursor(r, c - 1);
                }
            }
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        // Collect flattened parameter values for easy cursor/erase checking
        let mut values = Vec::new();
        for param_group in params {
            if !param_group.is_empty() {
                values.push(param_group[0]);
            }
        }

        match action {
            // SGR (Select Graphic Rendition) - Style formatting
            'm' => {
                if !intermediates.is_empty() {
                    return;
                }
                // Flatten all parameters and sub-parameters
                let mut sgr_params = Vec::new();
                for group in params {
                    for &val in group {
                        sgr_params.push(val);
                    }
                }

                if sgr_params.is_empty() {
                    *self.pen = Pen::default();
                    return;
                }

                let mut i = 0;
                while i < sgr_params.len() {
                    let param = sgr_params[i];
                    match param {
                        0 => {
                            *self.pen = Pen::default();
                            i += 1;
                        }
                        1 => {
                            self.pen.bold = true;
                            i += 1;
                        }
                        2 => {
                            self.pen.dim = true;
                            i += 1;
                        }
                        3 => {
                            self.pen.italic = true;
                            i += 1;
                        }
                        4 => {
                            self.pen.underline = true;
                            i += 1;
                        }
                        5 => {
                            self.pen.blink = true;
                            i += 1;
                        }
                        7 => {
                            self.pen.inverse = true;
                            i += 1;
                        }
                        8 => {
                            self.pen.hidden = true;
                            i += 1;
                        }
                        9 => {
                            self.pen.strikethrough = true;
                            i += 1;
                        }
                        // Reset normal intensity (turns off bold and dim)
                        22 => {
                            self.pen.bold = false;
                            self.pen.dim = false;
                            i += 1;
                        }
                        23 => {
                            self.pen.italic = false;
                            i += 1;
                        }
                        24 => {
                            self.pen.underline = false;
                            i += 1;
                        }
                        25 => {
                            self.pen.blink = false;
                            i += 1;
                        }
                        27 => {
                            self.pen.inverse = false;
                            i += 1;
                        }
                        28 => {
                            self.pen.hidden = false;
                            i += 1;
                        }
                        29 => {
                            self.pen.strikethrough = false;
                            i += 1;
                        }
                        // Foreground colors 30-37
                        30..=37 => {
                            self.pen.fg = Some(Color::Indexed((param - 30) as u8));
                            i += 1;
                        }
                        38 => {
                            // Extended foreground: 38;5;N or 38;2;R;G;B
                            if i + 2 < sgr_params.len() && sgr_params[i + 1] == 5 {
                                self.pen.fg = Some(Color::Indexed(sgr_params[i + 2] as u8));
                                i += 3;
                            } else if i + 4 < sgr_params.len() && sgr_params[i + 1] == 2 {
                                self.pen.fg = Some(Color::Rgb(
                                    sgr_params[i + 2] as u8,
                                    sgr_params[i + 3] as u8,
                                    sgr_params[i + 4] as u8,
                                ));
                                i += 5;
                            } else {
                                i += 1;
                            }
                        }
                        39 => {
                            self.pen.fg = None;
                            i += 1;
                        }
                        // Background colors 40-47
                        40..=47 => {
                            self.pen.bg = Some(Color::Indexed((param - 40) as u8));
                            i += 1;
                        }
                        48 => {
                            // Extended background: 48;5;N or 48;2;R;G;B
                            if i + 2 < sgr_params.len() && sgr_params[i + 1] == 5 {
                                self.pen.bg = Some(Color::Indexed(sgr_params[i + 2] as u8));
                                i += 3;
                            } else if i + 4 < sgr_params.len() && sgr_params[i + 1] == 2 {
                                self.pen.bg = Some(Color::Rgb(
                                    sgr_params[i + 2] as u8,
                                    sgr_params[i + 3] as u8,
                                    sgr_params[i + 4] as u8,
                                ));
                                i += 5;
                            } else {
                                i += 1;
                            }
                        }
                        49 => {
                            self.pen.bg = None;
                            i += 1;
                        }
                        // Bright foreground 90-97
                        90..=97 => {
                            self.pen.fg = Some(Color::Indexed((param - 90 + 8) as u8));
                            i += 1;
                        }
                        // Bright background 100-107
                        100..=107 => {
                            self.pen.bg = Some(Color::Indexed((param - 100 + 8) as u8));
                            i += 1;
                        }
                        _ => {
                            i += 1;
                        }
                    }
                }
            }
            // CUP (Cursor Position) or HVP (Horizontal Vertical Position)
            'H' | 'f' => {
                let r = values.first().copied().unwrap_or(1).max(1) as usize;
                let c = values.get(1).copied().unwrap_or(1).max(1) as usize;
                self.grid.set_cursor(r - 1, c - 1);
            }
            // CUU (Cursor Up)
            'A' => {
                let n = values.first().copied().unwrap_or(1).max(1) as usize;
                let (r, col) = self.grid.cursor();
                self.grid.set_cursor(r.saturating_sub(n), col);
            }
            // CUD (Cursor Down)
            'B' => {
                let n = values.first().copied().unwrap_or(1).max(1) as usize;
                let (r, col) = self.grid.cursor();
                self.grid.set_cursor(r + n, col);
            }
            // CUF (Cursor Forward)
            'C' => {
                let n = values.first().copied().unwrap_or(1).max(1) as usize;
                let (r, col) = self.grid.cursor();
                self.grid.set_cursor(r, col + n);
            }
            // CUB (Cursor Backward)
            'D' => {
                let n = values.first().copied().unwrap_or(1).max(1) as usize;
                let (r, col) = self.grid.cursor();
                self.grid.set_cursor(r, col.saturating_sub(n));
            }
            // CHA (Cursor Character Absolute)
            'G' => {
                let col = values.first().copied().unwrap_or(1).max(1) as usize;
                let (r, _) = self.grid.cursor();
                self.grid.set_cursor(r, col - 1);
            }
            // VPA (Line Position Absolute)
            'd' => {
                let row = values.first().copied().unwrap_or(1).max(1) as usize;
                let (_, col) = self.grid.cursor();
                self.grid.set_cursor(row - 1, col);
            }
            // ED (Erase in Display)
            'J' => {
                let mode = values.first().copied().unwrap_or(0);
                let (cursor_row, cursor_col) = self.grid.cursor();
                let height = self.grid.height();
                let width = self.grid.width();

                match mode {
                    0 => {
                        if let Some(row) = self.grid.row_mut(cursor_row) {
                            for x in cursor_col..width {
                                row.set_cell(x, Cell::default());
                            }
                        }
                        for y in (cursor_row + 1)..height {
                            if let Some(row) = self.grid.row_mut(y) {
                                for x in 0..width {
                                    row.set_cell(x, Cell::default());
                                }
                            }
                        }
                    }
                    1 => {
                        for y in 0..cursor_row {
                            if let Some(row) = self.grid.row_mut(y) {
                                for x in 0..width {
                                    row.set_cell(x, Cell::default());
                                }
                            }
                        }
                        if let Some(row) = self.grid.row_mut(cursor_row) {
                            let limit = cursor_col.min(width.saturating_sub(1));
                            for x in 0..=limit {
                                row.set_cell(x, Cell::default());
                            }
                        }
                    }
                    2 | 3 => {
                        for y in 0..height {
                            if let Some(row) = self.grid.row_mut(y) {
                                for x in 0..width {
                                    row.set_cell(x, Cell::default());
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            // EL (Erase in Line)
            'K' => {
                let mode = values.first().copied().unwrap_or(0);
                let (cursor_row, cursor_col) = self.grid.cursor();
                let width = self.grid.width();

                if let Some(row) = self.grid.row_mut(cursor_row) {
                    match mode {
                        0 => {
                            for x in cursor_col..width {
                                row.set_cell(x, Cell::default());
                            }
                        }
                        1 => {
                            let limit = cursor_col.min(width.saturating_sub(1));
                            for x in 0..=limit {
                                row.set_cell(x, Cell::default());
                            }
                        }
                        2 => {
                            for x in 0..width {
                                row.set_cell(x, Cell::default());
                            }
                        }
                        _ => {}
                    }
                }
            }
            // IL (Insert Line)
            'L' => {
                let n = values.first().copied().unwrap_or(1).max(1) as usize;
                let (cursor_row, _) = self.grid.cursor();
                let height = self.grid.height();
                let width = self.grid.width();

                let top = self.grid.top_margin;
                let bottom = self.grid.bottom_margin;
                if cursor_row >= top && cursor_row <= bottom {
                    let base = self.grid.rows_len().saturating_sub(height);
                    for _ in 0..n {
                        let physical_bottom = base + bottom;
                        self.grid.remove_row(physical_bottom);
                        let physical_cursor = base + cursor_row;
                        let mut new_row = Row::new(width);
                        new_row.mark_all_dirty();
                        self.grid.insert_row(physical_cursor, new_row);
                    }
                }
            }
            // DL (Delete Line)
            'M' => {
                let n = values.first().copied().unwrap_or(1).max(1) as usize;
                let (cursor_row, _) = self.grid.cursor();
                let height = self.grid.height();
                let width = self.grid.width();

                let top = self.grid.top_margin;
                let bottom = self.grid.bottom_margin;
                if cursor_row >= top && cursor_row <= bottom {
                    let base = self.grid.rows_len().saturating_sub(height);
                    for _ in 0..n {
                        let physical_cursor = base + cursor_row;
                        self.grid.remove_row(physical_cursor);
                        let physical_bottom = base + bottom;
                        let mut new_row = Row::new(width);
                        new_row.mark_all_dirty();
                        self.grid.insert_row(physical_bottom, new_row);
                    }
                }
            }
            // DCH (Delete Character)
            'P' => {
                let n = values.first().copied().unwrap_or(1).max(1) as usize;
                let (cursor_row, cursor_col) = self.grid.cursor();
                let width = self.grid.width();

                if let Some(row) = self.grid.row_mut(cursor_row) {
                    let mut new_cells = row.cells().to_vec();
                    if cursor_col < width {
                        new_cells.drain(cursor_col..(cursor_col + n).min(width));
                        while new_cells.len() < width {
                            new_cells.push(Cell::default());
                        }
                        for (x, cell) in new_cells.into_iter().enumerate() {
                            row.set_cell(x, cell);
                        }
                    }
                }
            }
            // ICH (Insert Character)
            '@' => {
                let n = values.first().copied().unwrap_or(1).max(1) as usize;
                let (cursor_row, cursor_col) = self.grid.cursor();
                let width = self.grid.width();

                if let Some(row) = self.grid.row_mut(cursor_row) {
                    let mut new_cells = row.cells().to_vec();
                    if cursor_col < width {
                        for _ in 0..n {
                            if cursor_col < new_cells.len() {
                                new_cells.insert(cursor_col, Cell::default());
                            }
                        }
                        new_cells.truncate(width);
                        for (x, cell) in new_cells.into_iter().enumerate() {
                            row.set_cell(x, cell);
                        }
                    }
                }
            }
            // SU (Scroll Up)
            'S' => {
                let n = values.first().copied().unwrap_or(1).max(1) as usize;
                for _ in 0..n {
                    self.grid.scroll_down();
                }
            }
            // SD (Scroll Down)
            'T' => {
                let n = values.first().copied().unwrap_or(1).max(1) as usize;
                for _ in 0..n {
                    self.grid.scroll_up();
                }
            }
            // DECSTBM (Set Scroll Margins)
            'r' => {
                let height = self.grid.height();
                let top = match values.first().copied() {
                    None | Some(0) => 1,
                    Some(v) => v,
                } as usize;
                let bottom = match values.get(1).copied() {
                    None | Some(0) => height,
                    Some(v) => v as usize,
                };
                self.grid.set_margins(top - 1, bottom - 1);
                self.grid.set_cursor(0, 0);
            }
            // SM (Set Mode)
            'h' if intermediates == b"?" => {
                for val in values {
                    match val {
                        1049 | 47 | 1047 => {
                            self.grid.set_alt_buffer(true);
                        }
                        25 => {
                            self.grid.cursor_visible = true;
                        }
                        _ => {}
                    }
                }
            }
            // RM (Reset Mode)
            'l' if intermediates == b"?" => {
                for val in values {
                    match val {
                        1049 | 47 | 1047 => {
                            self.grid.set_alt_buffer(false);
                        }
                        25 => {
                            self.grid.cursor_visible = false;
                        }
                        _ => {}
                    }
                }
            }
            // DSR (Device Status Report)
            'n' => {
                let mode = values.first().copied().unwrap_or(0);
                if mode == 6 {
                    let (row, col) = self.grid.cursor();
                    let resp = format!("\x1b[{};{}R", row + 1, col + 1);
                    self.replies.push(resp.into_bytes());
                }
            }
            _ => {}
        }
    }
}
