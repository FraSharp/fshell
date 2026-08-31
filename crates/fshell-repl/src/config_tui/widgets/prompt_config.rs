// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Prompt overview pane and bridge to full interactive Prompt Studio.

use crate::config_tui::app::App;
use crate::theme_ext::ThemeColorRatatui;
use fshell_core::Val;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Widget};

pub fn render(area: Rect, buf: &mut Buffer, app: &App) {
    let theme = app.env.active_theme();

    let block = Block::default()
        .title(" Prompt Configuration & Studio ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.widgets.title.to_ratatui_color()));

    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let vars = app.env.vars.read();
    let prompt_left = vars
        .get("FSH_PROMPT")
        .and_then(|v| match v {
            Val::String(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "default dynamic prompt".into());

    let prompt_right = vars
        .get("FSH_PROMPT_RIGHT")
        .and_then(|v| match v {
            Val::String(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "<none>".into());
    drop(vars);

    let lines = vec![
        Line::from(Span::styled(
            "── Live Prompt Preview ──",
            theme.status.info.to_style().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                " user ",
                theme
                    .prompt
                    .user
                    .to_style()
                    .bg(theme.chrome.background.to_ratatui_color()),
            ),
            Span::styled(
                " ~/dev/fshell ",
                theme
                    .prompt
                    .pwd
                    .to_style()
                    .bg(theme.chrome.background.to_ratatui_color()),
            ),
            Span::styled(
                " main * ",
                theme
                    .prompt
                    .git_branch
                    .to_style()
                    .bg(theme.chrome.background.to_ratatui_color()),
            ),
            Span::styled("❯ ", theme.prompt.prompt_symbol.to_style()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "── Prompt Variables ──",
            theme.status.info.to_style().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  $FSH_PROMPT:       ", theme.widgets.title.to_style()),
            Span::styled(prompt_left, Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("  $FSH_PROMPT_RIGHT: ", theme.widgets.title.to_style()),
            Span::styled(prompt_right, Style::default()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "── Interactive Prompt Studio ──",
            theme.status.info.to_style().add_modifier(Modifier::BOLD),
        )),
        Line::from("  fshell includes a dedicated visual Prompt Customizer Studio for designing"),
        Line::from("  multi-segment powerline prompts, git status badges, and transient prompts."),
        Line::from(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                " [p] Launch Prompt Customizer Studio ",
                theme
                    .widgets
                    .item_selected_fg
                    .to_style()
                    .bg(theme.widgets.item_selected_bg.to_ratatui_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    Paragraph::new(lines).render(inner, buf);
}
