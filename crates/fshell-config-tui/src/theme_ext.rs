// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Theme color conversion traits for config-tui.
//!
//! This is a duplicate of the trait in fshell-repl to avoid circular dependencies.
//! Both modules provide identical functionality.

use fshell_core::theme::ThemeColor;
use ratatui::style::{Color, Modifier, Style};

/// Extension trait for ThemeColor → ratatui conversions.
pub trait ThemeColorRatatui {
    fn to_ratatui_color(&self) -> Color;
    fn to_style(&self) -> Style;
    fn to_style_bold(&self) -> Style;
    fn to_style_dim(&self) -> Style;
}

impl ThemeColorRatatui for ThemeColor {
    fn to_ratatui_color(&self) -> Color {
        let (r, g, b) = self.to_rgb();
        Color::Rgb(r, g, b)
    }

    fn to_style(&self) -> Style {
        Style::default().fg(self.to_ratatui_color())
    }

    fn to_style_bold(&self) -> Style {
        Style::default()
            .fg(self.to_ratatui_color())
            .add_modifier(Modifier::BOLD)
    }

    fn to_style_dim(&self) -> Style {
        Style::default()
            .fg(self.to_ratatui_color())
            .add_modifier(Modifier::DIM)
    }
}
