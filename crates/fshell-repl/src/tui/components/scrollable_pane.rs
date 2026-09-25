// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Scrollable viewport component with automatic bounds clamping and Ratatui scrollbar rendering.

use crate::tui::theme;
use fshell_core::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget};

/// State managing vertical scroll position and bounds.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollState {
    pub offset: usize,
    pub total: usize,
    pub visible: usize,
}

impl ScrollState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, total: usize, visible: usize) {
        self.total = total;
        self.visible = visible;
        let max_offset = total.saturating_sub(visible);
        if self.offset > max_offset {
            self.offset = max_offset;
        }
    }

    pub fn scroll_down(&mut self, lines: usize) {
        let max_offset = self.total.saturating_sub(self.visible);
        self.offset = (self.offset + lines).min(max_offset);
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.offset = self.offset.saturating_sub(lines);
    }

    pub fn page_down(&mut self) {
        let page = self.visible.saturating_sub(2).max(1);
        self.scroll_down(page);
    }

    pub fn page_up(&mut self) {
        let page = self.visible.saturating_sub(2).max(1);
        self.scroll_up(page);
    }

    pub fn scroll_to_top(&mut self) {
        self.offset = 0;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.offset = self.total.saturating_sub(self.visible);
    }

    pub fn ensure_visible(&mut self, item_idx: usize) {
        if item_idx < self.offset {
            self.offset = item_idx;
        } else if item_idx >= self.offset + self.visible && self.visible > 0 {
            self.offset = item_idx + 1 - self.visible;
        }
    }

    /// Renders a vertical scrollbar on the rightmost column of `area` if content overflows.
    pub fn render_scrollbar(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        if self.total <= self.visible || area.width < 1 || area.height < 1 {
            return;
        }

        let scrollbar_area = Rect::new(
            area.x + area.width.saturating_sub(1),
            area.y,
            1,
            area.height,
        );

        let mut state = ScrollbarState::new(self.total).position(self.offset);
        let track_style = theme::muted_style(theme);
        let thumb_style = theme::title_style(theme);

        Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("┃")
            .style(track_style)
            .thumb_style(thumb_style)
            .render(scrollbar_area, buf, &mut state);
    }
}
