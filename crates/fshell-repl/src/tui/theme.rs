// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Theme adapter providing unified Ratatui styling tokens from `fshell_core::theme::Theme`.

use crate::theme_ext::ThemeColorRatatui;
use fshell_core::theme::{Theme, ThemeColor};
use ratatui::style::{Color, Modifier, Style};

/// Helper to convert a `ThemeColor` directly to a Ratatui `Color`.
pub fn to_ratatui_color(color: &ThemeColor) -> Color {
    color.to_ratatui_color()
}

/// Helper to convert a `ThemeColor` directly to a foreground `Style`.
pub fn to_style(color: &ThemeColor) -> Style {
    color.to_style()
}

/// Helper to convert a `ThemeColor` to a bold foreground `Style`.
pub fn to_style_bold(color: &ThemeColor) -> Style {
    color.to_style_bold()
}

/// Standardized border style for panels and containers.
pub fn border_style(theme: &Theme) -> Style {
    theme.status.muted.to_style_dim()
}

/// Focused border style for active panels.
pub fn border_focused_style(theme: &Theme) -> Style {
    theme.widgets.title.to_style()
}

/// Standardized panel title style.
pub fn title_style(theme: &Theme) -> Style {
    theme.widgets.title.to_style_bold()
}

/// Standardized selection style for list items and table rows.
pub fn selected_style(theme: &Theme) -> Style {
    Style::default()
        .bg(theme.widgets.item_selected_bg.to_ratatui_color())
        .fg(theme.widgets.item_selected_fg.to_ratatui_color())
        .add_modifier(Modifier::BOLD)
}

/// High-contrast match character highlight style for fuzzy searching.
pub fn match_highlight_style(theme: &Theme) -> Style {
    theme
        .syntax
        .operator
        .to_style_bold()
        .add_modifier(Modifier::UNDERLINED)
}

/// Success / OK indicator style.
pub fn status_ok_style(theme: &Theme) -> Style {
    theme.status.ok.to_style_bold()
}

/// Error / failure indicator style.
pub fn status_error_style(theme: &Theme) -> Style {
    theme.status.error.to_style_bold()
}

/// Warning indicator style.
pub fn status_warn_style(theme: &Theme) -> Style {
    theme.status.warning.to_style_bold()
}

/// Muted / dim text style for secondary metadata.
pub fn muted_style(theme: &Theme) -> Style {
    theme.status.muted.to_style_dim()
}

/// Keybinding shortcut bracket/key highlight style.
pub fn key_hint_key_style(theme: &Theme) -> Style {
    theme.status.info.to_style_bold()
}

/// Keybinding shortcut description label style.
pub fn key_hint_label_style(theme: &Theme) -> Style {
    theme.status.muted.to_style()
}

/// Standardized keyword style.
pub fn keyword_style(theme: &Theme) -> Style {
    theme.syntax.keyword.to_style_bold()
}

/// Standardized foreground text style.
pub fn foreground_style(theme: &Theme) -> Style {
    theme.widgets.foreground.to_style()
}

/// Standardized error style.
pub fn error_style(theme: &Theme) -> Style {
    theme.status.error.to_style_bold()
}

/// Standardized ok/success style.
pub fn ok_style(theme: &Theme) -> Style {
    theme.status.ok.to_style_bold()
}
