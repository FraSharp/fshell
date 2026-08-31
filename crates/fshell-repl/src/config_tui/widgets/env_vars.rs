// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Environment & special shell variables inspector with real-time search and type badges.

use crate::config_tui::app::{App, Focus};
use crate::theme_ext::ThemeColorRatatui;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget,
};

pub fn render(area: Rect, buf: &mut Buffer, app: &App) {
    let theme = app.env.active_theme();
    let is_focused = matches!(app.focus, Focus::Content);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    let border_color = if is_focused {
        theme.widgets.title.to_ratatui_color()
    } else {
        theme.status.muted.to_ratatui_color()
    };

    let title = if app.search_query.is_empty() {
        " Variables ".to_string()
    } else {
        format!(" Variables [Filter: \"{}\"] ", app.search_query)
    };

    let list_block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));

    let filtered = app.filtered_vars();
    let list_items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(idx, (name, _val, kind))| {
            let is_selected = is_focused && idx == app.content_selected;

            let (style, prefix) = if is_selected {
                (
                    Style::default()
                        .fg(theme.widgets.item_selected_fg.to_ratatui_color())
                        .bg(theme.widgets.item_selected_bg.to_ratatui_color())
                        .add_modifier(Modifier::BOLD),
                    "▸ ",
                )
            } else {
                (Style::default(), "  ")
            };

            let kind_badge = Span::styled(format!(" [{kind}]"), theme.status.muted.to_style());

            ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(format!("{:<22}", name), style),
                kind_badge,
            ]))
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(app.content_selected));
    StatefulWidget::render(
        List::new(list_items).block(list_block),
        chunks[0],
        buf,
        &mut state,
    );

    // Detail Pane on Right
    let detail_block = Block::default()
        .title(" Variable Detail ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.status.info.to_ratatui_color()));

    let inner = detail_block.inner(chunks[1]);
    detail_block.render(chunks[1], buf);

    if let Some((name, val, kind)) = filtered.get(app.content_selected) {
        let mut lines = vec![
            Line::from(vec![
                Span::styled(
                    "Name: ",
                    theme.widgets.title.to_style().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    name.as_str(),
                    theme.status.info.to_style().add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(
                    "Type: ",
                    theme.widgets.title.to_style().add_modifier(Modifier::BOLD),
                ),
                Span::styled(kind.as_str(), theme.syntax.keyword.to_style()),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Value:",
                theme.widgets.title.to_style().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ];

        // Value text formatting
        for val_line in val.lines() {
            lines.push(Line::from(format!("  {val_line}")));
        }

        Paragraph::new(lines).render(inner, buf);
    } else {
        let empty = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No variable selected.",
                theme.status.muted.to_style(),
            )),
        ];
        Paragraph::new(empty).render(inner, buf);
    }
}
