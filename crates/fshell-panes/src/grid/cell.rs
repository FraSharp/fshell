// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use super::pen::Pen;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub character: char,
    pub pen: Pen,
    /// For double-width characters (CJK, emoji), the next cell is a continuation marker
    pub wide_continuation: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            character: ' ',
            pen: Pen::default(),
            wide_continuation: false,
        }
    }
}

impl Cell {
    pub fn is_wide(&self) -> bool {
        unicode_width::UnicodeWidthChar::width(self.character).unwrap_or(1) > 1
    }

    pub fn width(&self) -> usize {
        if self.wide_continuation {
            0 // Continuation cells take no display space
        } else {
            unicode_width::UnicodeWidthChar::width(self.character).unwrap_or(1)
        }
    }
}
