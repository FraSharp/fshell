// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_panes::grid::{Grid, parser::GridParser};

#[test]
fn test_alt_buffer_switching() {
    let mut grid = Grid::new(10, 3, 100);
    let mut parser = GridParser::new();

    // Write normal text
    parser.process(&mut grid, b"normal");

    // Switch to alt buffer (CSI ? 1049 h)
    parser.process(&mut grid, b"\x1b[?1049h");

    // Alt buffer should start blank
    let vp = grid.viewport();
    assert_eq!(vp[0].cells()[0].character, ' ');

    // Write alt text
    parser.process(&mut grid, b"alt");
    let vp_alt = grid.viewport();
    assert_eq!(vp_alt[0].cells()[0].character, 'a');

    // Switch back to normal (CSI ? 1049 l)
    parser.process(&mut grid, b"\x1b[?1049l");
    let vp_norm = grid.viewport();
    assert_eq!(vp_norm[0].cells()[0].character, 'n');
}

#[test]
fn test_scroll_margins() {
    let mut grid = Grid::new(5, 4, 10);
    let mut parser = GridParser::new();

    // Set scroll margins to rows 2 to 3 (1-indexed: CSI 2;3 r)
    parser.process(&mut grid, b"\x1b[2;3r");
    assert_eq!(grid.top_margin, 1);
    assert_eq!(grid.bottom_margin, 2);

    // Fill row 1 and row 2 (0-indexed)
    parser.process(&mut grid, b"\x1b[2;1Hrow1\nrow2");

    // Cursor is now at row 2 col 4. Printing newlines should trigger sub-region scroll
    parser.process(&mut grid, b"\nnew");

    let vp = grid.viewport();
    // Row 1 (index 1) should have scrolled up to Row 2, replacing "row1" with "row2"
    let row1_str: String = vp[1].cells().iter().map(|c| c.character).collect();
    // Row 2 (index 2) should contain the "new" text
    let row2_str: String = vp[2].cells().iter().map(|c| c.character).collect();

    assert!(row1_str.starts_with("row2"));
    assert!(row2_str.starts_with("new"));
}

#[test]
fn test_line_insert_delete() {
    let mut grid = Grid::new(10, 4, 10);
    let mut parser = GridParser::new();

    // Write 3 lines
    parser.process(&mut grid, b"line1\nline2\nline3");

    // Move cursor to line 2 (CSI 2;1 H) and insert 1 line (CSI L)
    parser.process(&mut grid, b"\x1b[2;1H\x1b[L");

    let vp = grid.viewport();
    let r0: String = vp[0].cells().iter().map(|c| c.character).collect();
    let r1: String = vp[1].cells().iter().map(|c| c.character).collect();
    let r2: String = vp[2].cells().iter().map(|c| c.character).collect();

    assert!(r0.starts_with("line1"));
    assert!(r1.starts_with(" ")); // blank line inserted
    assert!(r2.starts_with("line2")); // shifted down
}

#[test]
fn test_char_insert_delete() {
    let mut grid = Grid::new(10, 3, 10);
    let mut parser = GridParser::new();

    parser.process(&mut grid, b"hello");

    // Move to col 3 (col 2 0-indexed: CSI 1;3 H) and delete 2 chars (CSI 2 P)
    parser.process(&mut grid, b"\x1b[1;3H\x1b[2P");

    let vp = grid.viewport();
    let r0: String = vp[0].cells().iter().map(|c| c.character).collect();
    assert!(r0.starts_with("heo")); // "ll" deleted

    // Insert 2 spaces at col 3 (CSI 2 @)
    parser.process(&mut grid, b"\x1b[1;3H\x1b[2@");
    let vp2 = grid.viewport();
    let r0_ins: String = vp2[0].cells().iter().map(|c| c.character).collect();
    assert!(r0_ins.starts_with("he  o"));
}
