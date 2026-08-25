// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use super::{Grid, pen::Color};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;

/// A ratatui widget that renders the visible viewport of a Grid.
///
/// Uses zero-allocation character rendering via `char::encode_utf8`
/// into a stack-allocated `[u8; 4]` buffer.
pub struct GridWidget<'a> {
    grid: &'a Grid,
}

impl<'a> GridWidget<'a> {
    pub fn new(grid: &'a Grid) -> Self {
        Self { grid }
    }
}

/// Map internal Color to ratatui Color (zero-alloc, no heap).
fn map_color(color: Option<Color>) -> ratatui::style::Color {
    match color {
        Some(Color::Indexed(i)) => ratatui::style::Color::Indexed(i),
        Some(Color::Rgb(r, g, b)) => ratatui::style::Color::Rgb(r, g, b),
        Some(Color::Default) | None => ratatui::style::Color::Reset,
    }
}

impl Widget for GridWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Clear the entire area first to remove stale cells from previous renders
        // (e.g. orphaned wide-char continuation markers, deleted text).
        // We set every cell to a space with default style, so any cell not
        // overwritten by grid content will appear as a clean blank.
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol(" ").set_style(Style::default());
                }
            }
        }
        let viewport = self.grid.viewport_iter();
        for (row_idx, row) in viewport.enumerate() {
            if row_idx as u16 >= area.height {
                break;
            }
            for (col_idx, cell) in row.cells().iter().enumerate() {
                if col_idx as u16 >= area.width {
                    break;
                }

                // Skip wide-continuation cells (they take no display space)
                if cell.wide_continuation {
                    continue;
                }

                let x = area.x + col_idx as u16;
                let y = area.y + row_idx as u16;

                let mut style = Style::default();

                // Map Pen attributes to ratatui Style modifiers
                if cell.pen.bold {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if cell.pen.italic {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if cell.pen.underline {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if cell.pen.dim {
                    style = style.add_modifier(Modifier::DIM);
                }
                if cell.pen.inverse {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                if cell.pen.hidden {
                    style = style.add_modifier(Modifier::HIDDEN);
                }
                if cell.pen.strikethrough {
                    style = style.add_modifier(Modifier::CROSSED_OUT);
                }

                // Map colors
                style.fg = Some(map_color(cell.pen.fg));
                style.bg = Some(map_color(cell.pen.bg));

                // Zero-allocation character rendering:
                // Encode char to stack-allocated UTF-8 buffer
                let mut char_bytes = [0u8; 4];
                let symbol_str = cell.character.encode_utf8(&mut char_bytes);

                if let Some(ratatui_cell) = buf.cell_mut((x, y)) {
                    ratatui_cell.set_symbol(symbol_str).set_style(style);
                }
            }
        }
    }
}
