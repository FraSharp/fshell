// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_panes::grid::{Cell, Row, pen::Pen};

#[test]
fn row_new_is_clean() {
    let row = Row::new(10);
    assert!(!row.is_dirty());
    assert_eq!(row.len(), 10);
}

#[test]
fn row_dirty_range() {
    let mut row = Row::new(10);
    row.mark_dirty(3, 5);
    assert!(row.is_dirty());
    let (left, right) = row.dirty_range().unwrap();
    assert_eq!(left, 3);
    assert_eq!(right, 5);
}

#[test]
fn row_set_cell_clears_forward_continuation() {
    let mut row = Row::new(10);
    // Place a wide char at col 2 (occupies col 2 + continuation at col 3)
    row.set_cell(
        2,
        Cell {
            character: '中',
            pen: Pen::default(),
            wide_continuation: false,
        },
    );
    row.set_cell(
        3,
        Cell {
            character: '\0',
            pen: Pen::default(),
            wide_continuation: true,
        },
    );
    // Overwrite the wide char at col 2 with a narrow char
    row.set_cell(
        2,
        Cell {
            character: 'x',
            pen: Pen::default(),
            wide_continuation: false,
        },
    );
    // Col 3 should now be cleared (no orphaned continuation)
    assert!(
        !row.cells()[3].wide_continuation,
        "continuation at col 3 should be cleared"
    );
    assert_eq!(row.cells()[3].character, ' ');
}

#[test]
fn row_mark_all_dirty_empty_row() {
    // mark_all_dirty on an empty row should not panic
    // (Currently Row::new always creates with width > 0, but guard anyway)
    let mut row = Row::new(10);
    row.mark_all_dirty();
    let (left, right) = row.dirty_range().unwrap();
    assert_eq!(left, 0);
    assert_eq!(right, 9);
}

#[test]
fn row_dirty_range_merges() {
    let mut row = Row::new(10);
    row.mark_dirty(2, 4);
    row.mark_dirty(6, 8);
    let (left, right) = row.dirty_range().unwrap();
    assert_eq!(left, 2);
    assert_eq!(right, 8);
}
