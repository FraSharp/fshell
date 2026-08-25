// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_panes::grid::{Cell, Pen};

#[test]
fn cell_default_is_space() {
    let cell = Cell::default();
    assert_eq!(cell.character, ' ');
    assert_eq!(cell.pen, Pen::default());
}

#[test]
fn pen_default_is_reset() {
    let pen = Pen::default();
    assert!(!pen.bold);
    assert!(!pen.italic);
    assert_eq!(pen.fg, None);
}

#[test]
fn cell_and_pen_are_copy() {
    let cell = Cell::default();
    let cell2 = cell; // Copy, not move
    assert_eq!(cell, cell2);
}
