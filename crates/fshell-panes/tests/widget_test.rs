// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_panes::grid::{Grid, widget::GridWidget};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

#[test]
fn widget_renders_to_buffer() {
    let mut grid = Grid::new(10, 3, 100);
    grid.write_str("Hello");
    let widget = GridWidget::new(&grid);
    let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
    widget.render(Rect::new(0, 0, 10, 3), &mut buf);
    assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "H");
}
