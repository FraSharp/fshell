// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Hooks management pane with event grouping, function existence validation, and scrolling.

use crate::config_tui::app::{App, Focus};
use crate::theme_ext::ThemeColorRatatui;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Widget};

const HOOK_DESCS: &[(&str, &str)] = &[
    ("precmd", "Executes before each prompt is rendered"),
    (
        "preexec",
        "Executes immediately before each command line runs",
    ),
    ("chpwd", "Executes whenever working directory is changed"),
];

pub fn render(area: Rect, buf: &mut Buffer, app: &App) {
    let theme = app.env.active_theme();
    let is_focused = matches!(app.focus, Focus::Content);

    let border_color = if is_focused {
        theme.widgets.title.to_ratatui_color()
    } else {
        theme.status.muted.to_ratatui_color()
    };

    let block = Block::default()
        .title(" Shell Hooks (precmd / preexec / chpwd) ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let flat_hooks = app.flat_hooks_list();
    let hooks_reg = app.env.hooks.registry.read();
    let fns = app.env.fns.read();

    let mut lines = Vec::new();

    for &(event, desc) in HOOK_DESCS {
        lines.push(Line::from(vec![
            Span::styled(
                format!("── {event} ──"),
                theme.status.info.to_style().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(desc, theme.status.muted.to_style()),
        ]));

        let handlers = hooks_reg.get(event);
        if let Some(list) = handlers
            && !list.is_empty()
        {
            for handler_fn in list {
                let global_idx = flat_hooks
                    .iter()
                    .position(|(e, f)| *e == event && f == handler_fn)
                    .unwrap_or(0);
                let is_selected = is_focused && global_idx == app.content_selected;

                let (style, prefix) = if is_selected {
                    (
                        Style::default()
                            .fg(theme.widgets.item_selected_fg.to_ratatui_color())
                            .bg(theme.widgets.item_selected_bg.to_ratatui_color())
                            .add_modifier(Modifier::BOLD),
                        "   ▸ ",
                    )
                } else {
                    (Style::default(), "     ")
                };

                let exists = fns.contains_key(handler_fn.as_str());
                let warn_badge = if !exists {
                    Span::styled(
                        " [function not defined yet]",
                        theme.status.warning.to_style(),
                    )
                } else {
                    Span::styled(" [ok]", theme.status.ok.to_style())
                };

                lines.push(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(format!("fn: {:<25}", handler_fn), style),
                    warn_badge,
                ]));
            }
        } else {
            lines.push(Line::from(Span::styled(
                "     (no handlers registered)",
                theme.status.muted.to_style(),
            )));
        }
        lines.push(Line::from(""));
    }

    drop(hooks_reg);
    drop(fns);

    lines.push(Line::from(Span::styled(
        "  [a] Add Hook   [d] Delete Selected Hook",
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
