// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Standardized footer status bar rendering status info on the left and key hints on the right.

use crate::tui::theme;
use fshell_core::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

/// A single keybinding hint pair, e.g. ("Tab", "Switch Pane").
#[derive(Debug, Clone)]
pub struct KeyHint {
    pub key: &'static str,
    pub label: &'static str,
}

impl KeyHint {
    pub const fn new(key: &'static str, label: &'static str) -> Self {
        Self { key, label }
    }
}

/// Standardized status footer widget.
pub struct StatusFooter<'a> {
    pub status: Option<Span<'a>>,
    pub hints: &'a [KeyHint],
    pub theme: &'a Theme,
}

impl<'a> StatusFooter<'a> {
    pub fn new(theme: &'a Theme, hints: &'a [KeyHint]) -> Self {
        Self {
            status: None,
            hints,
            theme,
        }
    }

    pub fn with_status(mut self, status: Span<'a>) -> Self {
        self.status = Some(status);
        self
    }

    pub fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 4 || area.height < 1 {
            return;
        }

        let key_style = theme::key_hint_key_style(self.theme);
        let label_style = theme::key_hint_label_style(self.theme);
        let sep_style = theme::muted_style(self.theme);

        let mut spans = Vec::new();

        // 1. Left status
        let mut left_width = 0;
        if let Some(st) = self.status {
            left_width = st.width() + 2;
            spans.push(Span::raw(" "));
            spans.push(st);
            spans.push(Span::raw(" "));
        }

        // 2. Right key hints
        let mut right_spans = Vec::new();
        let mut right_width = 0;

        for (i, hint) in self.hints.iter().enumerate() {
            let item_width = hint.key.width() + hint.label.width() + 4; // "[key] label "
            if right_width + item_width + left_width + 4 > area.width as usize {
                // Skip further hints if they overflow screen width
                break;
            }

            if i > 0 {
                right_spans.push(Span::styled(" ", sep_style));
                right_width += 1;
            }

            right_spans.push(Span::styled(format!("[{}]", hint.key), key_style));
            right_spans.push(Span::styled(format!(" {} ", hint.label), label_style));
            right_width += hint.key.width() + 2 + hint.label.width() + 1;
        }

        // Calculate padding between left and right
        let available_space = (area.width as usize).saturating_sub(left_width + right_width);
        if available_space > 0 {
            spans.push(Span::raw(" ".repeat(available_space)));
        }

        spans.extend(right_spans);

        Paragraph::new(Line::from(spans)).render(area, buf);
    }
}
