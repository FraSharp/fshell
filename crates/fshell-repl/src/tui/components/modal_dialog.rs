// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Floating modal dialog helper with backdrop clearing, centered positioning, and rounded framing.

use crate::tui::theme;
use fshell_core::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};

/// Computes a centered rectangle within `r` using percentage dimensions.
pub fn centered_percent(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Computes a centered rectangle with fixed width and height (clamped to terminal dimensions).
pub fn centered_fixed(width: u16, height: u16, r: Rect) -> Rect {
    let w = width.min(r.width.saturating_sub(2)).max(1);
    let h = height.min(r.height.saturating_sub(2)).max(1);
    let x = r.x + (r.width.saturating_sub(w)) / 2;
    let y = r.y + (r.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

/// Renders a clear backdrop and a rounded block frame for a modal dialog.
/// Returns the inner `Rect` available for dialog contents.
pub fn render_modal_frame(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    title: &str,
) -> Rect {
    // 1. Wipe out any characters beneath the dialog
    Clear.render(area, buf);

    // 2. Draw styled rounded block
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border_focused_style(theme))
        .title(format!(" {title} "))
        .title_style(theme::title_style(theme));

    let inner = block.inner(area);
    block.render(area, buf);
    inner
}
