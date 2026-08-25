// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::app::{App, Focus};
use crate::theme_ext::ThemeColorRatatui;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

pub fn item_count(app: &App) -> usize {
    app.env.scope.aliases.read().len()
}

pub fn render(area: Rect, buf: &mut Buffer, app: &App) {
    let theme = app.env.active_theme();
    let aliases = app.env.scope.aliases.read();
    let is_content_focused = matches!(app.focus, Focus::Content);

    let mut lines: Vec<Line> = Vec::new();

    if aliases.is_empty() {
        lines.push(Line::from("  No aliases defined."));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Press 'a' to add. Format: name = expansion",
            theme.status.muted.to_style(),
        )));
    } else {
        for (idx, (name, expansion)) in aliases.iter().enumerate() {
            let is_highlighted = is_content_focused && idx == app.content_selected;
            let style = if is_highlighted {
                theme.widgets.title.to_style().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![Span::styled(
                format!("  {:<20} = {}", name, expansion),
                style,
            )]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  'a' add  'd' delete  'Enter' edit",
            theme.status.muted.to_style(),
        )));
    }

    let paragraph =
        Paragraph::new(lines).block(Block::default().title(" Aliases ").borders(Borders::ALL));

    paragraph.render(area, buf);
}
