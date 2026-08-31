// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Theme browser widget with palette inspection and live syntax preview.

use crate::config_tui::app::{App, Focus};
use crate::theme_ext::ThemeColorRatatui;
use fshell_core::theme::Theme;
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
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let border_color = if is_focused {
        theme.widgets.title.to_ratatui_color()
    } else {
        theme.status.muted.to_ratatui_color()
    };

    // Left pane: Theme List
    let active_theme_name = &app.env.options.read().theme;
    let list_items: Vec<ListItem> = app
        .themes
        .iter()
        .enumerate()
        .map(|(idx, name)| {
            let is_selected = is_focused && idx == app.content_selected;
            let is_active = name == active_theme_name;

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

            let badge = if is_active {
                Span::styled(
                    " (active)",
                    theme.status.ok.to_style().add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("")
            };

            ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(name.as_str(), style),
                badge,
            ]))
        })
        .collect();

    let list_block = Block::default()
        .title(" Available Themes ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));

    let mut state = ListState::default();
    state.select(Some(app.content_selected));
    StatefulWidget::render(
        List::new(list_items).block(list_block),
        chunks[0],
        buf,
        &mut state,
    );

    // Right pane: Theme Preview
    let selected_theme_name = app
        .themes
        .get(app.content_selected)
        .cloned()
        .unwrap_or_else(|| "default".into());

    let config_dir = fshell_engine::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let preview_theme =
        Theme::load(&selected_theme_name, &config_dir).unwrap_or_else(|_| Theme::default_theme());

    let preview_block = Block::default()
        .title(format!(" Preview: {selected_theme_name} "))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.status.info.to_ratatui_color()));

    let inner = preview_block.inner(chunks[1]);
    preview_block.render(chunks[1], buf);

    let lines = vec![
        Line::from(Span::styled(
            "── Syntax Highlighting Preview ──",
            preview_theme
                .status
                .info
                .to_style()
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        // Code sample
        Line::from(vec![
            Span::styled("let ", preview_theme.syntax.keyword.to_style()),
            Span::styled("files", preview_theme.syntax.variable.to_style()),
            Span::raw(" = "),
            Span::styled("ls ", preview_theme.syntax.builtin.to_style()),
            Span::styled("/tmp ", preview_theme.syntax.string.to_style()),
            Span::styled("| ", preview_theme.syntax.operator.to_style()),
            Span::styled("filter ", preview_theme.syntax.builtin.to_style()),
            Span::styled(".size ", preview_theme.syntax.variable.to_style()),
            Span::styled("> ", preview_theme.syntax.operator.to_style()),
            Span::styled("1024", preview_theme.syntax.number.to_style()),
        ]),
        Line::from(vec![
            Span::styled("if ", preview_theme.syntax.keyword.to_style()),
            Span::styled("count ", preview_theme.syntax.builtin.to_style()),
            Span::styled("$files ", preview_theme.syntax.variable.to_style()),
            Span::styled("> ", preview_theme.syntax.operator.to_style()),
            Span::styled("0 ", preview_theme.syntax.number.to_style()),
            Span::raw("{"),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("echo ", preview_theme.syntax.builtin.to_style()),
            Span::styled("\"Found match: \" ", preview_theme.syntax.string.to_style()),
            Span::styled("$files", preview_theme.syntax.variable.to_style()),
        ]),
        Line::from("}"),
        Line::from(""),
        // Color Palette Swatches
        Line::from(Span::styled(
            "── Color Palette ──",
            preview_theme
                .status
                .info
                .to_style()
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(" ■ Keyword  ", preview_theme.syntax.keyword.to_style()),
            Span::styled(" ■ Builtin  ", preview_theme.syntax.builtin.to_style()),
            Span::styled(" ■ Function ", preview_theme.syntax.function.to_style()),
            Span::styled(" ■ Variable ", preview_theme.syntax.variable.to_style()),
        ]),
        Line::from(vec![
            Span::styled(" ■ String   ", preview_theme.syntax.string.to_style()),
            Span::styled(" ■ Number   ", preview_theme.syntax.number.to_style()),
            Span::styled(" ■ OK       ", preview_theme.status.ok.to_style()),
            Span::styled(" ■ Error    ", preview_theme.status.error.to_style()),
        ]),
        Line::from(""),
        // Prompt preview
        Line::from(Span::styled(
            "── Prompt Preview ──",
            preview_theme
                .status
                .info
                .to_style()
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                " user ",
                preview_theme
                    .prompt
                    .user
                    .to_style()
                    .bg(preview_theme.chrome.background.to_ratatui_color()),
            ),
            Span::styled(
                " ~/dev/fshell ",
                preview_theme
                    .prompt
                    .pwd
                    .to_style()
                    .bg(preview_theme.chrome.background.to_ratatui_color()),
            ),
            Span::styled(
                " main * ",
                preview_theme
                    .prompt
                    .git_branch
                    .to_style()
                    .bg(preview_theme.chrome.background.to_ratatui_color()),
            ),
            Span::styled("❯ ", preview_theme.prompt.prompt_symbol.to_style()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  [Space / Enter] Apply Theme Immediately",
            theme.status.muted.to_style(),
        )),
    ];

    Paragraph::new(lines).render(inner, buf);
}
