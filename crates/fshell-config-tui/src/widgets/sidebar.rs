// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::app::{App, Focus};
use crate::theme_ext::ThemeColorRatatui;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, StatefulWidget};

pub fn render(area: Rect, buf: &mut Buffer, app: &App) {
    let theme = app.env.active_theme();
    let items: Vec<ListItem> = app
        .categories
        .iter()
        .enumerate()
        .map(|(i, cat)| {
            let is_selected = i == app.sidebar_selected;
            let style = if is_selected && matches!(app.focus, Focus::Sidebar) {
                Style::default()
                    .fg(theme.widgets.item_selected_fg.to_ratatui_color())
                    .bg(theme.widgets.item_selected_bg.to_ratatui_color())
                    .add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default()
                    .fg(theme.widgets.item_selected_fg.to_ratatui_color())
                    .bg(theme.status.muted.to_ratatui_color())
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(cat.label(), style)))
        })
        .collect();

    let list = List::new(items).block(Block::default().title(" Categories ").borders(Borders::ALL));

    let mut state = ListState::default();
    state.select(Some(app.sidebar_selected));

    StatefulWidget::render(list, area, buf, &mut state);
}
