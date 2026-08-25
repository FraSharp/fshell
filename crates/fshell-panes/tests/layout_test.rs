// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_panes::layout::bsp::{BspLayout, Split};
use ratatui::layout::Rect;

#[test]
fn bsp_single_pane_fills_area() {
    let layout = BspLayout::new();
    let result = layout.compute_layout(Rect::new(0, 0, 80, 24));
    assert_eq!(result.len(), 1);
    let (_id, rect) = result[0];
    assert_eq!(rect.width, 80);
    assert_eq!(rect.height, 24);
}

#[test]
fn bsp_horizontal_split() {
    let mut layout = BspLayout::new();
    let _new_id = layout.split(0, Split::Horizontal, 0.5);
    let result = layout.compute_layout(Rect::new(0, 0, 80, 24));
    assert_eq!(result.len(), 2);
    // Both panes should have width ~40
    let widths: Vec<u16> = result.iter().map(|(_, r)| r.width).collect();
    assert!(widths.iter().all(|w| *w >= 39 && *w <= 41));
}

#[test]
fn bsp_vertical_split() {
    let mut layout = BspLayout::new();
    let _new_id = layout.split(0, Split::Vertical, 0.5);
    let result = layout.compute_layout(Rect::new(0, 0, 80, 24));
    assert_eq!(result.len(), 2);
    let heights: Vec<u16> = result.iter().map(|(_, r)| r.height).collect();
    assert!(heights.iter().all(|h| *h >= 11 && *h <= 13));
}

#[test]
fn bsp_nested_splits() {
    let mut layout = BspLayout::new();
    // Split root horizontally
    let left_id = layout.split(0, Split::Horizontal, 0.5);
    // Split left pane vertically
    layout.split(left_id, Split::Vertical, 0.5);
    let result = layout.compute_layout(Rect::new(0, 0, 80, 24));
    assert_eq!(result.len(), 3);
}

#[test]
fn bsp_remove_pane() {
    let mut layout = BspLayout::new();
    let new_id = layout.split(0, Split::Horizontal, 0.5);
    assert_eq!(layout.pane_count(), 2);
    layout.remove(new_id);
    assert_eq!(layout.pane_count(), 1);
    // Remaining pane should fill the area
    let result = layout.compute_layout(Rect::new(0, 0, 80, 24));
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].1.width, 80);
}

#[test]
fn bsp_split_returns_unique_id() {
    let mut layout = BspLayout::new();
    let id1 = layout.split(0, Split::Horizontal, 0.5);
    let id2 = layout.split(0, Split::Vertical, 0.5);
    assert_ne!(id1, id2);
}

#[test]
fn bsp_configurable_ratio() {
    let mut layout = BspLayout::new();
    layout.split(0, Split::Horizontal, 0.7);
    let result = layout.compute_layout(Rect::new(0, 0, 100, 24));
    assert_eq!(result.len(), 2);
    let widths: Vec<u16> = result.iter().map(|(_, r)| r.width).collect();
    // 70/30 split of 100 cols
    assert!(widths[0] >= 69 && widths[0] <= 71);
}

#[test]
fn bsp_remove_preserves_other_split_directions() {
    let mut layout = BspLayout::new();
    // Split 0 vertical (top / bottom)
    let bottom_id = layout.split(0, Split::Vertical, 0.5);
    // Split 0 horizontal (left / right)
    let top_right_id = layout.split(0, Split::Horizontal, 0.5);

    // Now remove top_right_id
    layout.remove(top_right_id);

    // The vertical split between 0 and bottom_id should be preserved.
    let result = layout.compute_layout(Rect::new(0, 0, 80, 24));
    // Since top_right_id is removed, we have 2 panes remaining (0 and bottom_id)
    assert_eq!(result.len(), 2);
    // Find rects
    let rect_0 = result.iter().find(|(id, _)| *id == 0).unwrap().1;
    let rect_bottom = result.iter().find(|(id, _)| *id == bottom_id).unwrap().1;

    // They should be vertically split, so their x/width should be identical, but y/height different.
    assert_eq!(rect_0.x, rect_bottom.x);
    assert_eq!(rect_0.width, rect_bottom.width);
    assert_ne!(rect_0.y, rect_bottom.y);
}
