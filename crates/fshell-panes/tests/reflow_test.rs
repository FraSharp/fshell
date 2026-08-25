// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_panes::grid::{Grid, reflow::Reflow};

#[test]
fn reflow_preserves_content_on_widen() {
    let mut grid = Grid::new(10, 3, 100);
    grid.write_str("hello"); // Fits on one line, no wrap
    let mut reflow = Reflow::new(&mut grid);
    reflow.reflow(20);
    let vp = grid.viewport();
    // "hello" should still be on first line
    assert_eq!(vp[0].cells()[0].character, 'h');
    assert_eq!(vp[0].cells()[4].character, 'o');
}

#[test]
fn reflow_handles_narrowing() {
    let mut grid = Grid::new(20, 5, 100);
    grid.write_str("hello world foo bar");
    let mut reflow = Reflow::new(&mut grid);
    reflow.reflow(10);
    assert_eq!(grid.width(), 10);
    // Content may wrap, but should not be lost
}

#[test]
fn reflow_widening_updates_dimensions() {
    let mut grid = Grid::new(10, 3, 100);
    grid.write_str("hello world"); // 11 chars, wraps at col 10
    let mut reflow = Reflow::new(&mut grid);
    reflow.reflow(20);
    assert_eq!(grid.width(), 20);
    assert_eq!(grid.height(), 3);
}
