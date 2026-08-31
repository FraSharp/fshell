// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Category sidebar widget with status badges and focus highlighting.

use crate::config_tui::app::{App, Category, Focus};
use crate::theme_ext::ThemeColorRatatui;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, StatefulWidget};

pub fn render(area: Rect, buf: &mut Buffer, app: &App) {
    let theme = app.env.active_theme();
    let is_focused = matches!(app.focus, Focus::Sidebar);

    let items: Vec<ListItem> = app
        .categories
        .iter()
        .enumerate()
        .map(|(i, cat)| {
            let is_selected = i == app.sidebar_selected;
            let icon = match cat {
                Category::ShellOptions => "[#]",
                Category::Themes => "[~]",
                Category::Aliases => "[=]",
                Category::Hooks => "[!]",
                Category::EnvVars => "[$]",
                Category::Prompt => "[>]",
            };

            let label = cat.label();

            let (style, prefix) = if is_selected && is_focused {
                (
                    Style::default()
                        .fg(theme.widgets.item_selected_fg.to_ratatui_color())
                        .bg(theme.widgets.item_selected_bg.to_ratatui_color())
                        .add_modifier(Modifier::BOLD),
                    "▸ ",
                )
            } else if is_selected {
                (
                    Style::default()
                        .fg(theme.widgets.item_selected_fg.to_ratatui_color())
                        .bg(theme.status.muted.to_ratatui_color())
                        .add_modifier(Modifier::BOLD),
                    "▸ ",
                )
            } else {
                (Style::default(), "  ")
            };

            ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(format!("{icon} {label}"), style),
            ]))
        })
        .collect();

    let title = if app.dirty {
        " Settings [*] "
    } else {
        " Settings "
    };

    let border_color = if is_focused {
        theme.widgets.title.to_ratatui_color()
    } else {
        theme.status.muted.to_ratatui_color()
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));

    let mut state = ListState::default();
    state.select(Some(app.sidebar_selected));

    StatefulWidget::render(List::new(items).block(block), area, buf, &mut state);
}
