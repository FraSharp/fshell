// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::app::App;
use crate::theme_ext::ThemeColorRatatui;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

pub fn render(area: Rect, buf: &mut Buffer, app: &App) {
    let theme = app.env.active_theme();
    let prompt = app.env.prompt_config.read();

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        "── Separator ──",
        theme.status.info.to_style(),
    )));
    lines.push(Line::from(format!("  style: {:?}", prompt.separator_style)));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "── Left Prompt Segments ──",
        theme.status.info.to_style(),
    )));
    for (i, seg) in prompt.left.iter().enumerate() {
        lines.push(Line::from(format!(
            "  {}. {:?} (fg: {:?}, bg: {:?}, bold: {})",
            i + 1,
            seg.r#type,
            seg.fg,
            seg.bg,
            seg.bold,
        )));
    }
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "── Right Prompt Segments ──",
        theme.status.info.to_style(),
    )));
    for (i, seg) in prompt.right.iter().enumerate() {
        lines.push(Line::from(format!(
            "  {}. {:?} (fg: {:?}, bg: {:?}, bold: {})",
            i + 1,
            seg.r#type,
            seg.fg,
            seg.bg,
            seg.bold,
        )));
    }

    if let Some(ref preset) = prompt.preset {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("  preset: {}", preset)));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Use `prompt tui` to edit prompt config.",
        theme.status.muted.to_style(),
    )));

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .title(" Prompt Config ")
            .borders(Borders::ALL),
    );

    paragraph.render(area, buf);
}
