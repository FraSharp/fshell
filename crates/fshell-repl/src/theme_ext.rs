// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Extension traits for converting `ThemeColor` to UI-specific color types.
//!
//! These conversions live in fshell-repl because that's where `nu_ansi_term`
//! and `ratatui` are available. fshell-core's `ThemeColor` stays pure data.

use fshell_core::theme::ThemeColor;
use nu_ansi_term::{Color as NuColor, Style};
use ratatui::style::{Color as RatatuiColor, Modifier, Style as RatatuiStyle};

/// Extension trait for ThemeColor → nu_ansi_term conversions.
pub trait ThemeColorNu {
    fn to_nu_color(&self) -> NuColor;
    fn to_style(&self) -> Style;
    fn to_style_bold(&self) -> Style;
    fn to_style_italic(&self) -> Style;
}

impl ThemeColorNu for ThemeColor {
    fn to_nu_color(&self) -> NuColor {
        let (r, g, b) = self.to_rgb();
        NuColor::Rgb(r, g, b)
    }

    fn to_style(&self) -> Style {
        Style::new().fg(self.to_nu_color())
    }

    fn to_style_bold(&self) -> Style {
        Style::new().fg(self.to_nu_color()).bold()
    }

    fn to_style_italic(&self) -> Style {
        Style::new().fg(self.to_nu_color()).italic()
    }
}

/// Extension trait for ThemeColor → ratatui conversions.
pub trait ThemeColorRatatui {
    fn to_ratatui_color(&self) -> RatatuiColor;
    fn to_style(&self) -> RatatuiStyle;
    fn to_style_bold(&self) -> RatatuiStyle;
    fn to_style_dim(&self) -> RatatuiStyle;
}

impl ThemeColorRatatui for ThemeColor {
    fn to_ratatui_color(&self) -> RatatuiColor {
        let (r, g, b) = self.to_rgb();
        RatatuiColor::Rgb(r, g, b)
    }

    fn to_style(&self) -> RatatuiStyle {
        RatatuiStyle::default().fg(self.to_ratatui_color())
    }

    fn to_style_bold(&self) -> RatatuiStyle {
        RatatuiStyle::default()
            .fg(self.to_ratatui_color())
            .add_modifier(Modifier::BOLD)
    }

    fn to_style_dim(&self) -> RatatuiStyle {
        RatatuiStyle::default()
            .fg(self.to_ratatui_color())
            .add_modifier(Modifier::DIM)
    }
}
