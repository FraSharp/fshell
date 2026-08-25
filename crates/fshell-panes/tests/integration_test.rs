// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_panes::grid::{Grid, parser::GridParser, widget::GridWidget};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

#[test]
fn full_pipeline_parse_render() {
    let mut grid = Grid::new(40, 10, 500);
    let mut parser = GridParser::new();

    // Simulate shell output with ANSI codes (multi-param SGR)
    parser.process(&mut grid, b"\x1b[1;33mBold yellow\x1b[0m\n");
    parser.process(&mut grid, b"\x1b[32;40mGreen on black\x1b[0m\n");

    let widget = GridWidget::new(&grid);
    let mut buf = Buffer::empty(Rect::new(0, 0, 40, 10));
    widget.render(Rect::new(0, 0, 40, 10), &mut buf);

    // Verify rendering
    assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "B");
    assert_eq!(buf.cell((1, 0)).unwrap().symbol(), "o");
}

#[test]
fn scrollback_preserves_history() {
    let mut grid = Grid::new(10, 3, 100);
    let mut parser = GridParser::new();

    // Write more lines than viewport height
    for i in 0..10 {
        let line = format!("line {}\n", i);
        parser.process(&mut grid, line.as_bytes());
    }

    // Should have scrollback history
    assert!(grid.scrollback_len() > 0);
}

#[test]
fn parser_pen_state_across_chunks() {
    let mut grid = Grid::new(20, 3, 100);
    let mut parser = GridParser::new();

    // Set bold in first chunk, write text in second
    parser.process(&mut grid, b"\x1b[1m");
    parser.process(&mut grid, b"bold text");

    let vp = grid.viewport();
    assert!(vp[0].cells()[0].pen.bold);
    assert_eq!(vp[0].cells()[0].character, 'b');
}

#[test]
fn grid_dirty_tracking_through_full_pipeline() {
    let mut grid = Grid::new(20, 3, 100);
    let mut parser = GridParser::new();

    parser.process(&mut grid, b"hello");

    // The row containing "hello" should be dirty
    let vp = grid.viewport();
    assert!(vp[0].is_dirty());
}
