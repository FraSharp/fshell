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
    let hooks = app.env.hooks.registry.read();
    hooks.values().map(|v| v.len()).sum()
}

const HOOK_DESCS: &[(&str, &str)] = &[
    ("precmd", "Runs before each prompt display"),
    ("preexec", "Runs before each command execution"),
    ("chpwd", "Runs when directory changes"),
];

pub fn render(area: Rect, buf: &mut Buffer, app: &App) {
    let theme = app.env.active_theme();
    let hooks = app.env.hooks.registry.read();
    let is_content_focused = matches!(app.focus, Focus::Content);

    let mut lines: Vec<Line> = Vec::new();
    let mut global_idx = 0;

    if hooks.is_empty() {
        lines.push(Line::from("  No hooks registered."));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Events: precmd, preexec, chpwd",
            theme.status.muted.to_style(),
        )));
        lines.push(Line::from(Span::styled(
            "  Press 'a' to add. Format: event fn_name",
            theme.status.muted.to_style(),
        )));
    } else {
        for (event, handlers) in hooks.iter() {
            let desc = HOOK_DESCS
                .iter()
                .find(|(e, _)| *e == *event)
                .map(|(_, d)| *d)
                .unwrap_or("");
            lines.push(Line::from(vec![
                Span::styled(format!("  {}:", event), theme.status.info.to_style()),
                Span::raw("  "),
                Span::styled(desc, theme.status.muted.to_style()),
            ]));
            for handler in handlers {
                let is_highlighted = is_content_focused && global_idx == app.content_selected;
                let style = if is_highlighted {
                    theme.widgets.title.to_style().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(
                    format!("    -> {}", handler),
                    style,
                )));
                global_idx += 1;
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  'a' add  'd' delete",
            theme.status.muted.to_style(),
        )));
    }

    let paragraph =
        Paragraph::new(lines).block(Block::default().title(" Hooks ").borders(Borders::ALL));

    paragraph.render(area, buf);
}
