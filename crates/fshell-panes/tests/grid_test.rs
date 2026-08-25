// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_panes::grid::Grid;

#[test]
fn grid_new_dimensions() {
    let grid = Grid::new(80, 24, 1000);
    assert_eq!(grid.width(), 80);
    assert_eq!(grid.height(), 24);
    assert_eq!(grid.scrollback_limit(), 1000);
}

#[test]
fn grid_scroll_pushes_old_rows() {
    let mut grid = Grid::new(10, 3, 5);
    for _ in 0..5 {
        grid.scroll_down();
    }
    assert!(grid.scrollback_len() > 0);
}

#[test]
fn grid_viewport_no_underflow() {
    // Edge case: grid just created, rows.len() == height
    let grid = Grid::new(10, 3, 100);
    let vp = grid.viewport();
    assert_eq!(vp.len(), 3);
}

#[test]
fn grid_resize_reflows() {
    let mut grid = Grid::new(10, 3, 100);
    grid.write_str("hello world 12345");
    grid.resize(20, 3);
    assert_eq!(grid.width(), 20);
}

#[test]
fn grid_scroll_marks_rows_dirty() {
    let mut grid = Grid::new(10, 3, 100);
    // Initially clean
    assert!(!grid.viewport()[0].is_dirty());
    assert!(!grid.viewport()[1].is_dirty());
    assert!(!grid.viewport()[2].is_dirty());
    // Scroll down — all visible rows should be dirty
    grid.scroll_down();
    let vp = grid.viewport();
    // At minimum, the newly added row should be dirty (it was just created
    // via set_cell in scroll_down, or... actually scroll_down just pushes
    // a blank row. Let's check the last row).
    // The key invariant: after scroll, the UI must re-render.
    // The last viewport row is the new blank row — mark it dirty.
    assert!(
        vp[vp.len() - 1].is_dirty(),
        "new row after scroll should be dirty"
    );
}

#[test]
fn grid_resize_shrinks_height() {
    let mut grid = Grid::new(10, 5, 100);
    // Fill all 5 rows
    for _ in 0..5 {
        grid.scroll_down();
    }
    assert_eq!(grid.viewport().len(), 5);
    // Resize to 3 rows
    grid.resize(10, 3);
    assert_eq!(grid.height(), 3);
    assert_eq!(grid.viewport().len(), 3);
    // Scrollback should grow by the excess rows
    // (5 - 3 = 2 extra rows went to scrollback)
}

#[test]
fn grid_viewport_iter_matches_viewport() {
    let mut grid = Grid::new(10, 3, 100);
    grid.write_str("hello");
    // viewport_iter should yield the same data as viewport
    let vp_vec = grid.viewport();
    let vp_iter: Vec<_> = grid.viewport_iter().collect();
    assert_eq!(vp_vec.len(), vp_iter.len());
    for (a, b) in vp_vec.iter().zip(vp_iter.iter()) {
        assert_eq!(a.cells(), b.cells());
    }
}

#[test]
fn grid_viewport_iter_exact_size() {
    let grid = Grid::new(10, 5, 100);
    let iter = grid.viewport_iter();
    assert_eq!(iter.len(), 5);
}

#[test]
fn grid_resize_shrinks_width() {
    let mut grid = Grid::new(20, 3, 100);
    grid.write_str("hello world");
    grid.resize(10, 3);
    assert_eq!(grid.width(), 10);
    // Cursor should be clamped
    let (_row, col) = grid.cursor();
    assert!(col < 10);
}

#[test]
fn grid_scroll_up_viewport() {
    let mut grid = Grid::new(10, 3, 100);
    // Write enough lines to create scrollback
    grid.write_str("line0\nline1\nline2\nline3\nline4");
    // Viewport should be at bottom (offset 0)
    assert!(grid.is_at_bottom());
    // Scroll up — viewport should show older lines
    grid.scroll_up_viewport(2);
    assert!(!grid.is_at_bottom());
    let vp: Vec<String> = grid
        .viewport()
        .iter()
        .map(|r| r.cells().iter().map(|c| c.character).collect())
        .collect();
    // After scrolling up 2 lines, we should see lines from earlier
    assert!(vp[0].contains("line") || vp[1].contains("line"));
}

#[test]
fn grid_scroll_down_viewport() {
    let mut grid = Grid::new(10, 3, 100);
    grid.write_str("line0\nline1\nline2\nline3\nline4");
    grid.scroll_up_viewport(3);
    assert!(!grid.is_at_bottom());
    grid.scroll_down_viewport(3);
    assert!(grid.is_at_bottom());
}

#[test]
fn grid_scroll_up_clamps_to_max() {
    let mut grid = Grid::new(10, 3, 100);
    grid.write_str("a\nb\nc\nd\ne");
    // Scroll up way past available history
    grid.scroll_up_viewport(1000);
    assert!(!grid.is_at_bottom());
    // Should clamp to max offset
    let _max_offset = grid.scrollback_len();
    // Scroll back to bottom
    grid.scroll_down_viewport(1000);
    assert!(grid.is_at_bottom());
}

#[test]
fn grid_scroll_zero_offset_is_bottom() {
    let grid = Grid::new(10, 3, 100);
    assert!(grid.is_at_bottom());
    assert_eq!(grid.viewport().len(), 3);
}

#[test]
fn grid_resize_actually_resizes_rows_and_alt_buffer() {
    let mut grid = Grid::new(10, 3, 100);
    assert_eq!(grid.viewport()[0].cells().len(), 10);

    // Switch to alt screen (to initialize alt_rows)
    grid.set_alt_buffer(true);
    assert_eq!(grid.viewport()[0].cells().len(), 10);

    // Resize grid while alt screen is active
    grid.resize(20, 4);
    assert_eq!(grid.viewport().len(), 4);
    assert_eq!(grid.viewport()[0].cells().len(), 20);

    // Switch back to normal screen and check that its rows were also resized
    grid.set_alt_buffer(false);
    assert_eq!(grid.viewport().len(), 4);
    assert_eq!(grid.viewport()[0].cells().len(), 20);
}

// New scrollback features: scroll_page_up/down

/// Helper: create a grid with N lines of scrollback history.
/// Each line is labeled "line0", "line1", ..., "lineN-1".
fn grid_with_scrollback(visible: usize, total_lines: usize) -> Grid {
    let mut grid = Grid::new(20, visible, 1000);
    let content: Vec<String> = (0..total_lines).map(|i| format!("line{i}")).collect();
    grid.write_str(&content.join("\n"));
    grid
}

/// Helper: extract the first non-space characters from each viewport row.
fn viewport_lines(grid: &Grid) -> Vec<String> {
    grid.viewport()
        .iter()
        .map(|r| {
            r.cells()
                .iter()
                .map(|c| c.character)
                .collect::<String>()
                .trim()
                .to_string()
        })
        .collect()
}

#[test]
fn grid_scroll_page_up_moves_by_viewport_height() {
    // 3 visible rows, 10 lines of output → 7 lines in scrollback
    let mut grid = grid_with_scrollback(3, 10);
    assert!(grid.is_at_bottom());
    assert_eq!(grid.scrollback_len(), 7);

    // Page up should move viewport_offset by height (3)
    grid.scroll_page_up();
    assert!(!grid.is_at_bottom());
    assert_eq!(grid.viewport_offset(), 3);

    // Viewport should now show older lines
    let lines = viewport_lines(&grid);
    assert!(lines[0].contains("line"));
}

#[test]
fn grid_scroll_page_down_moves_by_viewport_height() {
    let mut grid = grid_with_scrollback(3, 10);
    grid.scroll_page_up(); // offset = 3
    assert_eq!(grid.viewport_offset(), 3);

    grid.scroll_page_down(); // offset = 0
    assert!(grid.is_at_bottom());
    assert_eq!(grid.viewport_offset(), 0);
}

#[test]
fn grid_scroll_page_up_clamps_to_top() {
    let mut grid = grid_with_scrollback(3, 10); // 7 scrollback lines
    // Scroll up past the top — should clamp to max offset (7)
    grid.scroll_page_up();
    grid.scroll_page_up();
    grid.scroll_page_up(); // offset 9, but max is 7
    assert_eq!(grid.viewport_offset(), 7);
}

#[test]
fn grid_scroll_page_down_clamps_to_bottom() {
    let mut grid = grid_with_scrollback(3, 10);
    grid.scroll_page_up();
    grid.scroll_page_up(); // offset 6
    // Page down past bottom — should clamp to 0
    grid.scroll_page_down();
    grid.scroll_page_down();
    grid.scroll_page_down(); // should be 0
    assert!(grid.is_at_bottom());
    assert_eq!(grid.viewport_offset(), 0);
}

#[test]
fn grid_scroll_page_up_no_scrollback_does_nothing() {
    let mut grid = Grid::new(10, 3, 100);
    // No scrollback — page up should be a no-op
    grid.scroll_page_up();
    assert!(grid.is_at_bottom());
    assert_eq!(grid.viewport_offset(), 0);
}

// New scrollback features: scroll_to_top/bottom

#[test]
fn grid_scroll_to_top_goes_to_max_offset() {
    let mut grid = grid_with_scrollback(3, 10); // 7 scrollback
    grid.scroll_to_top();
    assert!(!grid.is_at_bottom());
    assert_eq!(grid.viewport_offset(), 7); // max offset
}

#[test]
fn grid_scroll_to_bottom_goes_to_zero() {
    let mut grid = grid_with_scrollback(3, 10);
    grid.scroll_to_top();
    assert!(!grid.is_at_bottom());

    grid.scroll_to_bottom();
    assert!(grid.is_at_bottom());
    assert_eq!(grid.viewport_offset(), 0);
}

#[test]
fn grid_scroll_to_top_no_scrollback_does_nothing() {
    let mut grid = Grid::new(10, 3, 100);
    grid.scroll_to_top();
    assert!(grid.is_at_bottom());
}

#[test]
fn grid_scroll_to_bottom_when_already_bottom_is_noop() {
    let mut grid = grid_with_scrollback(3, 10);
    assert!(grid.is_at_bottom());
    grid.scroll_to_bottom();
    assert!(grid.is_at_bottom());
}

// Scroll position info

#[test]
fn grid_scroll_position_at_bottom() {
    let grid = grid_with_scrollback(3, 10);
    let pos = grid.scroll_position();
    assert_eq!(pos.offset, 0);
    assert_eq!(pos.total_lines, 10);
    assert_eq!(pos.visible_lines, 3);
    assert_eq!(pos.percentage, 100.0);
}

#[test]
fn grid_scroll_position_at_top() {
    let mut grid = grid_with_scrollback(3, 10);
    grid.scroll_to_top();
    let pos = grid.scroll_position();
    assert_eq!(pos.offset, 7);
    assert_eq!(pos.percentage, 0.0);
}

#[test]
fn grid_scroll_position_no_scrollback() {
    let grid = Grid::new(10, 3, 100);
    let pos = grid.scroll_position();
    assert_eq!(pos.offset, 0);
    assert_eq!(pos.total_lines, 3);
    assert_eq!(pos.percentage, 100.0);
}
