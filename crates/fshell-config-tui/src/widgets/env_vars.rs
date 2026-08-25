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
    let vars = app.env.scope.vars.read();

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        "── FSH Environment Variables ──",
        theme.status.info.to_style(),
    )));
    lines.push(Line::from(""));

    let fsh_vars = [
        "FSH_CONFIG_DIR",
        "FSH_CACHE_DIR",
        "FSH_HOME",
        "FSH_PROMPT",
        "FSH_PROMPT_RIGHT",
        "FSH_KEYBINDING_MODE",
        "FSH_SESSION_ID",
        "FSH_TUI_MODE",
        "FSH_DURATION_COLOR",
    ];

    for var in &fsh_vars {
        let value = vars
            .get(*var)
            .map(|v| format!("{:?}", v))
            .unwrap_or_else(|| "(not set)".into());
        lines.push(Line::from(format!("  {:<25} {}", var, value)));
    }

    let paragraph =
        Paragraph::new(lines).block(Block::default().title(" Env Vars ").borders(Borders::ALL));

    paragraph.render(area, buf);
}
