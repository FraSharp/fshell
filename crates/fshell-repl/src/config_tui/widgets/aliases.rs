// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Aliases management pane with shadow detection, search, and viewport scrolling.

use crate::config_tui::app::{App, Focus};
use crate::theme_ext::ThemeColorRatatui;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Widget};

pub fn render(area: Rect, buf: &mut Buffer, app: &App) {
    let theme = app.env.active_theme();
    let is_focused = matches!(app.focus, Focus::Content);

    let border_color = if is_focused {
        theme.widgets.title.to_ratatui_color()
    } else {
        theme.status.muted.to_ratatui_color()
    };

    let title = if app.search_query.is_empty() {
        " Aliases ".to_string()
    } else {
        format!(" Aliases [Filter: \"{}\"] ", app.search_query)
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let filtered = app.filtered_aliases();
    if filtered.is_empty() {
        let msg = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No aliases defined or matching filter.",
                theme.status.muted.to_style(),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press 'a' to add a new alias.",
                theme.status.info.to_style(),
            )),
        ];
        Paragraph::new(msg).render(inner, buf);
        return;
    }

    let mut lines = Vec::new();
    // Header
    lines.push(Line::from(vec![
        Span::styled("    ", theme.status.muted.to_style()),
        Span::styled(
            format!("{:<20}", "ALIAS"),
            theme.widgets.title.to_style().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "EXPANSION",
            theme.widgets.title.to_style().add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));

    let builtins = app.env.scope.builtins.read();

    for (idx, (name, expansion)) in filtered.iter().enumerate() {
        let is_selected = is_focused && idx == app.content_selected;
        let is_shadowing = builtins.contains_key(name.as_str());

        let (style, prefix) = if is_selected {
            (
                Style::default()
                    .fg(theme.widgets.item_selected_fg.to_ratatui_color())
                    .bg(theme.widgets.item_selected_bg.to_ratatui_color())
                    .add_modifier(Modifier::BOLD),
                " ▸ ",
            )
        } else {
            (Style::default(), "   ")
        };

        let shadow_tag = if is_shadowing {
            Span::styled(" [shadows builtin]", theme.status.warning.to_style())
        } else {
            Span::raw("")
        };

        lines.push(Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(format!("{:<20}", name), style),
            Span::raw(" = "),
            Span::styled(expansion.as_str(), style),
            shadow_tag,
        ]));
    }

    drop(builtins);

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  [a] Add   [e/Enter] Edit   [d] Delete   [/] Filter",
        theme.status.muted.to_style(),
    )));

    // Viewport scrolling
    let visible_rows = inner.height as usize;
    let rendered_lines: Vec<Line> = if lines.len() > visible_rows {
        let max_offset = lines.len().saturating_sub(visible_rows);
        let offset = app
            .content_selected
            .saturating_sub(visible_rows / 2)
            .min(max_offset);
        lines.into_iter().skip(offset).take(visible_rows).collect()
    } else {
        lines
    };

    Paragraph::new(rendered_lines).render(inner, buf);
}
