// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Shell options pane with categorization, toggle badges, choice indicators,
//! detail inspector pane for numeric explanations/examples, and auto-scrolling viewport.

use crate::config_tui::app::{App, Focus};
use crate::config_tui::schema::OptionKind;
use crate::theme_ext::ThemeColorRatatui;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
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
        " Shell Options ".to_string()
    } else {
        format!(" Shell Options [Filter: \"{}\"] ", app.search_query)
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

    let filtered_indices = app.filtered_option_indices();
    if filtered_indices.is_empty() {
        let empty_msg = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No options matched your search.",
                theme.status.muted.to_style(),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press '/' to search or Esc to clear.",
                theme.status.muted.to_style(),
            )),
        ];
        Paragraph::new(empty_msg).render(inner, buf);
        return;
    }

    // Split into list pane and detail inspector pane if enough height
    let (list_area, detail_area) = if inner.height >= 14 {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(6), Constraint::Length(5)])
            .split(inner);
        (chunks[0], Some(chunks[1]))
    } else {
        (inner, None)
    };

    // Build all lines with metadata to handle auto-scrolling
    let mut item_lines: Vec<(usize, Line)> = Vec::new();
    let mut current_section = "";

    for (filter_idx, &opt_idx) in filtered_indices.iter().enumerate() {
        let opt = &app.options[opt_idx];
        if opt.section != current_section {
            current_section = opt.section;
            item_lines.push((
                usize::MAX,
                Line::from(vec![
                    Span::raw("── "),
                    Span::styled(
                        current_section,
                        theme.status.info.to_style().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" ──"),
                ]),
            ));
        }

        let is_selected = is_focused && filter_idx == app.content_selected;

        let badge = match &opt.kind {
            OptionKind::Bool(true) => Span::styled(
                " [ON] ",
                theme.status.ok.to_style().add_modifier(if is_selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            OptionKind::Bool(false) => Span::styled(
                " [OFF] ",
                theme.status.error.to_style().add_modifier(if is_selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            OptionKind::Choice { current, .. } => Span::styled(
                format!(" ({current}) "),
                theme
                    .syntax
                    .keyword
                    .to_style()
                    .add_modifier(if is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            OptionKind::Theme { current } => Span::styled(
                format!(" [{current}] "),
                theme
                    .syntax
                    .function
                    .to_style()
                    .add_modifier(if is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            OptionKind::Keybinding { current } => Span::styled(
                format!(" ({current}) "),
                theme
                    .syntax
                    .keyword
                    .to_style()
                    .add_modifier(if is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            OptionKind::Integer { current, unit, .. } => Span::styled(
                format!(" [{current} {unit}] "),
                theme.syntax.number.to_style().add_modifier(if is_selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
        };

        let label_style = if is_selected {
            Style::default()
                .fg(theme.widgets.item_selected_fg.to_ratatui_color())
                .bg(theme.widgets.item_selected_bg.to_ratatui_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let line = Line::from(vec![
            Span::styled(if is_selected { " ▸ " } else { "   " }, label_style),
            Span::styled(format!("{:<26}", opt.label), label_style),
            badge,
            Span::raw("  "),
            Span::styled(opt.description, theme.status.muted.to_style()),
        ]);

        item_lines.push((filter_idx, line));
    }

    // Viewport scrolling
    let visible_rows = list_area.height as usize;
    let selected_line_idx = item_lines
        .iter()
        .position(|(idx, _)| *idx == app.content_selected)
        .unwrap_or(0);

    let scroll_offset = if selected_line_idx < app.scroll_offset {
        selected_line_idx
    } else if selected_line_idx >= app.scroll_offset + visible_rows {
        selected_line_idx.saturating_sub(visible_rows.saturating_sub(1))
    } else {
        app.scroll_offset
    };

    let rendered_lines: Vec<Line> = item_lines
        .into_iter()
        .skip(scroll_offset)
        .take(visible_rows)
        .map(|(_, l)| l)
        .collect();

    Paragraph::new(rendered_lines).render(list_area, buf);

    // Detail Inspector Pane
    if let Some(detail_box) = detail_area
        && let Some(&selected_opt_idx) = filtered_indices.get(app.content_selected)
    {
        let opt = &app.options[selected_opt_idx];

        let mut detail_lines: Vec<Line> = Vec::new();
        match &opt.kind {
            OptionKind::Integer {
                min,
                max,
                unit,
                examples,
                higher_meaning,
                lower_meaning,
                ..
            } => {
                let examples_str = examples.join("  |  ");
                detail_lines.push(Line::from(vec![
                    Span::styled(
                        "  Range: ",
                        theme.widgets.title.to_style().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("[{min} .. {max}] {unit}  "),
                        theme.syntax.number.to_style(),
                    ),
                    Span::styled("Examples: ", theme.widgets.title.to_style()),
                    Span::styled(examples_str, theme.status.info.to_style()),
                ]));
                detail_lines.push(Line::from(vec![
                    Span::styled(
                        "  [+] Higher: ",
                        theme.status.ok.to_style().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(*higher_meaning, theme.status.muted.to_style()),
                ]));
                detail_lines.push(Line::from(vec![
                    Span::styled(
                        "  [-] Lower:  ",
                        theme.status.warning.to_style().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(*lower_meaning, theme.status.muted.to_style()),
                ]));
            }
            OptionKind::Choice { choices, .. } => {
                let choices_str = choices.join(" | ");
                detail_lines.push(Line::from(vec![
                    Span::styled(
                        "  Choices: ",
                        theme.widgets.title.to_style().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(choices_str, theme.syntax.keyword.to_style()),
                    Span::styled(
                        "  (Press Space or Enter to cycle)",
                        theme.status.muted.to_style(),
                    ),
                ]));
                detail_lines.push(Line::from(vec![
                    Span::styled("  Description: ", theme.widgets.title.to_style()),
                    Span::styled(opt.description, theme.status.muted.to_style()),
                ]));
            }
            OptionKind::Bool(_) => {
                detail_lines.push(Line::from(vec![
                    Span::styled(
                        "  Key: ",
                        theme.widgets.title.to_style().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("$options.{}", opt.key),
                        theme.syntax.variable.to_style(),
                    ),
                    Span::styled(
                        "  (Press Space to toggle, 's' to persist)",
                        theme.status.muted.to_style(),
                    ),
                ]));
                detail_lines.push(Line::from(vec![
                    Span::styled("  Description: ", theme.widgets.title.to_style()),
                    Span::styled(opt.description, theme.status.muted.to_style()),
                ]));
            }
            OptionKind::Theme { .. } | OptionKind::Keybinding { .. } => {
                detail_lines.push(Line::from(vec![
                    Span::styled(
                        "  Setting: ",
                        theme.widgets.title.to_style().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(opt.label, theme.syntax.keyword.to_style()),
                    Span::styled("  (Press Space to cycle)", theme.status.muted.to_style()),
                ]));
                detail_lines.push(Line::from(vec![
                    Span::styled("  Description: ", theme.widgets.title.to_style()),
                    Span::styled(opt.description, theme.status.muted.to_style()),
                ]));
            }
        }

        let detail_block = Block::default()
            .title(format!(" Option Inspector: {} ({}) ", opt.label, opt.key))
            .borders(Borders::TOP)
            .border_type(BorderType::Plain)
            .border_style(Style::default().fg(theme.status.info.to_ratatui_color()));

        let inner_detail = detail_block.inner(detail_box);
        detail_block.render(detail_box, buf);
        Paragraph::new(detail_lines).render(inner_detail, buf);
    }
}
