// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_panes::grid::{Grid, parser::GridParser};

#[test]
fn parser_writes_plain_text() {
    let mut grid = Grid::new(10, 3, 100);
    let mut parser = GridParser::new();
    parser.process(&mut grid, b"hello");
    let vp = grid.viewport();
    assert_eq!(vp[0].cells()[0].character, 'h');
    assert_eq!(vp[0].cells()[4].character, 'o');
}

#[test]
fn parser_handles_newline() {
    let mut grid = Grid::new(10, 3, 100);
    let mut parser = GridParser::new();
    parser.process(&mut grid, b"line1\nline2");
    let vp = grid.viewport();
    assert_eq!(vp[0].cells()[0].character, 'l');
    assert_eq!(vp[1].cells()[0].character, 'l');
}

#[test]
fn parser_handles_multi_sgr() {
    let mut grid = Grid::new(20, 3, 100);
    let mut parser = GridParser::new();
    // Bold + blue: \x1b[1;34m
    parser.process(&mut grid, b"\x1b[1;34mhello\x1b[0m");
    let vp = grid.viewport();
    assert!(vp[0].cells()[0].pen.bold);
    // SGR 34 = blue foreground
    assert!(vp[0].cells()[0].pen.fg.is_some());
}

#[test]
fn parser_tab_wraps_and_scrolls() {
    // Tab at end of line should wrap AND scroll when at bottom row
    let mut grid = Grid::new(10, 2, 100); // 2 rows, 10 cols
    let mut parser = GridParser::new();
    // Fill row 0 with 10 chars, then tab (should wrap to row 1 col 0)
    parser.process(&mut grid, b"0123456789\t");
    let vp = grid.viewport();
    // After tab wrap: cursor should be at row 1, col 0 (or 8 if tab stops align)
    // The key assertion: row 0 content should have scrolled up
    assert_eq!(vp[0].cells()[0].character, '0'); // row 0 still visible
    // Now fill row 1 and tab again — should trigger scroll
    parser.process(&mut grid, b"0123456789\t");
    let vp2 = grid.viewport();
    // After second scroll, row 0 should be the first "0123456789" line
    // and row 1 should be the second line (or tabbed)
    assert_eq!(vp2[0].cells()[0].character, '0');
    assert!(vp2[0].cells()[9].character == '9' || vp2[0].cells()[9].character == '\t');
}

#[test]
fn parser_tab_at_bottom_scrolls() {
    // Regression: tab at bottom row without scroll caused overwrite
    let mut grid = Grid::new(5, 2, 100); // 5 cols, 2 rows
    let mut parser = GridParser::new();
    // Fill both rows, then tab
    parser.process(&mut grid, b"abcde\n"); // row 0: "abcde", cursor at row 1
    parser.process(&mut grid, b"fghij"); // row 1: "fghij", cursor at row 1 col 5
    // Now tab — should scroll, not overwrite
    parser.process(&mut grid, b"\t");
    let vp = grid.viewport();
    // After scroll: row 0 should be "fghij", row 1 should be blank (or tabbed)
    assert_eq!(vp[0].cells()[0].character, 'f');
    assert_eq!(vp[0].cells()[4].character, 'j');
}

#[test]
fn parser_handles_double_width_char() {
    let mut grid = Grid::new(20, 3, 100);
    let mut parser = GridParser::new();
    parser.process(&mut grid, b"ab");
    let vp = grid.viewport();
    assert_eq!(vp[0].cells()[0].character, 'a');
    assert_eq!(vp[0].cells()[1].character, 'b');
}

#[test]
fn parser_handles_cursor_positioning() {
    let mut grid = Grid::new(10, 3, 100);
    let mut parser = GridParser::new();
    // Move to row 2, col 3 (1-indexed) and print 'X' -> should be row 1, col 2 (0-indexed)
    parser.process(&mut grid, b"\x1b[2;3HX");
    let vp = grid.viewport();
    assert_eq!(vp[1].cells()[2].character, 'X');
}

#[test]
fn parser_handles_clear_screen() {
    let mut grid = Grid::new(10, 3, 100);
    let mut parser = GridParser::new();
    parser.process(&mut grid, b"hello\nworld");
    // Clear screen
    parser.process(&mut grid, b"\x1b[2J");
    let vp = grid.viewport();
    for row in vp {
        for cell in row.cells() {
            assert_eq!(cell.character, ' ');
        }
    }
}

#[test]
fn parser_handles_clear_line() {
    let mut grid = Grid::new(10, 3, 100);
    let mut parser = GridParser::new();
    parser.process(&mut grid, b"hello");
    // Move cursor back to col 3 (0-indexed col 2) and clear line from cursor to end
    parser.process(&mut grid, b"\x1b[1;3H\x1b[K");
    let vp = grid.viewport();
    assert_eq!(vp[0].cells()[0].character, 'h');
    assert_eq!(vp[0].cells()[1].character, 'e');
    assert_eq!(vp[0].cells()[2].character, ' ');
    assert_eq!(vp[0].cells()[3].character, ' ');
}

#[test]
fn parser_handles_decstbm_defaults() {
    let mut grid = Grid::new(10, 5, 100);
    let mut parser = GridParser::new();

    // Default top and bottom (CSI r)
    parser.process(&mut grid, b"\x1b[r");
    assert_eq!(grid.top_margin, 0);
    assert_eq!(grid.bottom_margin, 4);

    // Default bottom (CSI 2;0 r or CSI 2 r)
    parser.process(&mut grid, b"\x1b[2;0r");
    assert_eq!(grid.top_margin, 1);
    assert_eq!(grid.bottom_margin, 4);

    // Default top (CSI 0;3 r)
    parser.process(&mut grid, b"\x1b[0;3r");
    assert_eq!(grid.top_margin, 0);
    assert_eq!(grid.bottom_margin, 2);
}

#[test]
fn parser_handles_extended_sgr_colors() {
    use fshell_panes::grid::pen::Color;

    let mut grid = Grid::new(10, 3, 100);
    let mut parser = GridParser::new();

    // 1. Semicolon-separated 256 foreground color (CSI 38;5;123m)
    parser.process(&mut grid, b"\x1b[38;5;123mX");
    let vp = grid.viewport();
    assert_eq!(vp[0].cells()[0].pen.fg, Some(Color::Indexed(123)));

    // 2. Colon-separated 256 foreground color (CSI 38:5:45m)
    parser.process(&mut grid, b"\x1b[38:5:45mY");
    let vp = grid.viewport();
    assert_eq!(vp[0].cells()[1].pen.fg, Some(Color::Indexed(45)));

    // 3. Semicolon-separated true-color background (CSI 48;2;10;20;30m)
    parser.process(&mut grid, b"\x1b[48;2;10;20;30mZ");
    let vp = grid.viewport();
    assert_eq!(vp[0].cells()[2].pen.bg, Some(Color::Rgb(10, 20, 30)));
}

#[test]
fn parser_handles_out_of_margin_scroll() {
    let mut grid = Grid::new(10, 4, 100);
    let mut parser = GridParser::new();

    // Set scroll margins to rows 2 to 3 (0-indexed row 1 to 2: CSI 2;3 r)
    parser.process(&mut grid, b"\x1b[2;3r");
    assert_eq!(grid.top_margin, 1);
    assert_eq!(grid.bottom_margin, 2);

    // Place cursor below margins (row 3, col 0)
    parser.process(&mut grid, b"\x1b[4;1Hhello");

    // Send a newline (LF) at row 3 (bottom of screen).
    // This should scroll the *entire screen* (since it is outside margins),
    // shifting "hello" to row 2, and leaving row 3 blank.
    parser.process(&mut grid, b"\n");
    let vp = grid.viewport();

    let r2: String = vp[2].cells().iter().map(|c| c.character).collect();
    let r3: String = vp[3].cells().iter().map(|c| c.character).collect();
    assert!(r2.starts_with("hello"));
    assert!(r3.starts_with(" "));
}
