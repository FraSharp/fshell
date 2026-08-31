// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Modal popup dialogs for config TUI (text inputs, dual-field inputs, choices, help).

use crate::theme_ext::ThemeColorRatatui;
use fshell_core::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget};

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

pub struct TextInputProps<'a> {
    pub title: &'a str,
    pub label: &'a str,
    pub value: &'a str,
    pub details: Option<&'a [String]>,
    pub error: Option<&'a str>,
}

pub fn render_text_input(area: Rect, buf: &mut Buffer, theme: &Theme, props: TextInputProps<'_>) {
    let height_percent = if props.details.is_some() { 50 } else { 25 };
    let popup_area = centered_rect(68, height_percent, area);
    Clear.render(popup_area, buf);

    let border_style = Style::default().fg(theme.widgets.title.to_ratatui_color());
    let block = Block::default()
        .title(format!(" {} ", props.title))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let inner = block.inner(popup_area);
    block.render(popup_area, buf);

    let display_val = if props.value.is_empty() {
        " "
    } else {
        props.value
    };
    let mut lines = vec![
        Line::from(Span::styled(
            props.label,
            theme.status.info.to_style().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("  Value: "),
            Span::styled(
                format!(" {display_val} "),
                Style::default()
                    .fg(theme.widgets.item_selected_fg.to_ratatui_color())
                    .bg(theme.widgets.item_selected_bg.to_ratatui_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    if let Some(detail_lines) = props.details {
        lines.push(Line::from(""));
        for d in detail_lines {
            lines.push(Line::from(Span::styled(
                format!("  {d}"),
                theme.status.muted.to_style(),
            )));
        }
    }

    if let Some(err) = props.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  Error: {err}"),
            theme.status.error.to_style().add_modifier(Modifier::BOLD),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  [Enter] Confirm   [Esc] Cancel",
        theme.status.muted.to_style(),
    )));

    Paragraph::new(lines).render(inner, buf);
}

pub struct TwoFieldModalProps<'a> {
    pub title: &'a str,
    pub field1_label: &'a str,
    pub field1_val: &'a str,
    pub field2_label: &'a str,
    pub field2_val: &'a str,
    pub active_field: usize,
    pub error: Option<&'a str>,
}

pub fn render_two_field_modal(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    props: TwoFieldModalProps<'_>,
) {
    let popup_area = centered_rect(65, 38, area);
    Clear.render(popup_area, buf);

    let border_style = Style::default().fg(theme.widgets.title.to_ratatui_color());
    let block = Block::default()
        .title(format!(" {} ", props.title))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let inner = block.inner(popup_area);
    block.render(popup_area, buf);

    // Field 1
    let f1_style = if props.active_field == 0 {
        theme.widgets.title.to_style().add_modifier(Modifier::BOLD)
    } else {
        theme.status.muted.to_style()
    };
    let val1_disp = if props.field1_val.is_empty() {
        "<empty>"
    } else {
        props.field1_val
    };
    let val1_style = if props.active_field == 0 {
        Style::default()
            .fg(theme.widgets.item_selected_fg.to_ratatui_color())
            .bg(theme.widgets.item_selected_bg.to_ratatui_color())
    } else {
        Style::default()
    };

    // Field 2
    let f2_style = if props.active_field == 1 {
        theme.widgets.title.to_style().add_modifier(Modifier::BOLD)
    } else {
        theme.status.muted.to_style()
    };
    let val2_disp = if props.field2_val.is_empty() {
        "<empty>"
    } else {
        props.field2_val
    };
    let val2_style = if props.active_field == 1 {
        Style::default()
            .fg(theme.widgets.item_selected_fg.to_ratatui_color())
            .bg(theme.widgets.item_selected_bg.to_ratatui_color())
    } else {
        Style::default()
    };

    let mut lines = vec![
        Line::from(Span::styled(format!("  {}:", props.field1_label), f1_style)),
        Line::from(vec![Span::raw("    "), Span::styled(val1_disp, val1_style)]),
        Line::from(""),
        Line::from(Span::styled(format!("  {}:", props.field2_label), f2_style)),
        Line::from(vec![Span::raw("    "), Span::styled(val2_disp, val2_style)]),
    ];

    if let Some(err) = props.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  Error: {err}"),
            theme.status.error.to_style().add_modifier(Modifier::BOLD),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  [Tab] Next Field   [Enter] Save   [Esc] Cancel",
        theme.status.muted.to_style(),
    )));

    Paragraph::new(lines).render(inner, buf);
}

pub fn render_confirm_dialog(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    title: &str,
    message: &str,
) {
    let popup_area = centered_rect(50, 20, area);
    Clear.render(popup_area, buf);

    let border_style = Style::default().fg(theme.status.warning.to_ratatui_color());
    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let inner = block.inner(popup_area);
    block.render(popup_area, buf);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {message}"),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("    "),
            Span::styled(
                " [y] Yes / Save ",
                theme
                    .widgets
                    .item_selected_fg
                    .to_style()
                    .bg(theme.status.ok.to_ratatui_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(
                " [n] No / Discard ",
                theme
                    .widgets
                    .item_selected_fg
                    .to_style()
                    .bg(theme.status.error.to_ratatui_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled("[Esc] Cancel", theme.status.muted.to_style()),
        ]),
    ];

    Paragraph::new(lines).render(inner, buf);
}

pub fn render_help_modal(area: Rect, buf: &mut Buffer, theme: &Theme) {
    let popup_area = centered_rect(70, 70, area);
    Clear.render(popup_area, buf);

    let border_style = Style::default().fg(theme.widgets.title.to_ratatui_color());
    let block = Block::default()
        .title(" Keyboard Navigation & Help ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let inner = block.inner(popup_area);
    block.render(popup_area, buf);

    let key_style = theme.status.info.to_style().add_modifier(Modifier::BOLD);
    let desc_style = Style::default();

    let lines = vec![
        Line::from(Span::styled(
            "Navigation",
            theme.widgets.title.to_style().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  Tab / Shift-Tab / h / l", key_style),
            Span::styled("  Switch between Sidebar and Content pane", desc_style),
        ]),
        Line::from(vec![
            Span::styled("  ↑ / ↓ / j / k          ", key_style),
            Span::styled(
                "  Navigate items in active pane with auto-scrolling",
                desc_style,
            ),
        ]),
        Line::from(vec![
            Span::styled("  Home / End             ", key_style),
            Span::styled("  Jump to top / bottom of list", desc_style),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Actions & Editing",
            theme.widgets.title.to_style().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  Space / Enter          ", key_style),
            Span::styled(
                "  Toggle boolean option / cycle choice / apply theme",
                desc_style,
            ),
        ]),
        Line::from(vec![
            Span::styled("  e / Enter              ", key_style),
            Span::styled(
                "  Edit selected value / alias / hook in modal dialog",
                desc_style,
            ),
        ]),
        Line::from(vec![
            Span::styled("  a                      ", key_style),
            Span::styled("  Add new alias or hook", desc_style),
        ]),
        Line::from(vec![
            Span::styled("  d                      ", key_style),
            Span::styled("  Delete selected alias or hook", desc_style),
        ]),
        Line::from(vec![
            Span::styled("  /                      ", key_style),
            Span::styled("  Live search & filter across current tab", desc_style),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Persistence & Session",
            theme.widgets.title.to_style().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  s                      ", key_style),
            Span::styled(
                "  Save & persist settings to ~/.config/fsh/init.fsh",
                desc_style,
            ),
        ]),
        Line::from(vec![
            Span::styled("  p                      ", key_style),
            Span::styled(
                "  Open Prompt Customizer Studio (from Prompt tab)",
                desc_style,
            ),
        ]),
        Line::from(vec![
            Span::styled("  ?                      ", key_style),
            Span::styled("  Toggle this help dialog", desc_style),
        ]),
        Line::from(vec![
            Span::styled("  q / Esc                ", key_style),
            Span::styled(
                "  Exit (prompts confirmation if unsaved changes)",
                desc_style,
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Press any key to close this help window.",
            theme.status.muted.to_style(),
        )),
    ];

    Paragraph::new(lines).render(inner, buf);
}
