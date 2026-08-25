// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Color palette and style constants for the TUI.
//!
//! Uses a dark, muted palette inspired by Tokyo Night — low contrast,
//! easy on the eyes, with soft accent colors.

use ratatui::style::{Color, Modifier, Style};

// Palette

/// Near-black background.
pub const BG: Color = Color::Rgb(26, 27, 38);
/// Dark background for panels/overlays.
pub const BG_DARK: Color = Color::Rgb(22, 22, 30);
/// Slightly lighter dark for subtle fills.
pub const BG_HIGHLIGHT: Color = Color::Rgb(36, 40, 59);

/// Primary text — light gray, readable but not harsh.
pub const TEXT: Color = Color::Rgb(169, 177, 214);
/// Dimmed text for secondary information.
pub const TEXT_DIM: Color = Color::Rgb(86, 95, 137);
/// Bright text for emphasis.
pub const TEXT_BRIGHT: Color = Color::Rgb(192, 202, 245);

/// Muted blue — primary accent for focused elements.
pub const ACCENT: Color = Color::Rgb(122, 162, 247);
/// Muted green — secondary accent.
pub const GREEN: Color = Color::Rgb(158, 206, 106);
/// Muted red — errors / destructive.
pub const RED: Color = Color::Rgb(247, 118, 142);
/// Muted purple — prefix mode / special state.
pub const PURPLE: Color = Color::Rgb(187, 154, 247);
/// Muted cyan — help overlay / info.
pub const CYAN: Color = Color::Rgb(125, 207, 255);
/// Muted yellow — warnings.
pub const YELLOW: Color = Color::Rgb(224, 175, 104);

/// Border color for unfocused panes.
pub const BORDER_DIM: Color = Color::Rgb(59, 66, 97);

// Styles

/// Focused pane border.
pub fn border_focused() -> Style {
    Style::default().fg(ACCENT)
}

/// Unfocused pane border.
pub fn border_unfocused() -> Style {
    Style::default().fg(BORDER_DIM)
}

/// Focused pane border when prefix mode is active.
pub fn border_focused_prefix() -> Style {
    Style::default().fg(PURPLE)
}

/// Focused pane title.
pub fn title_focused() -> Style {
    Style::default().fg(ACCENT)
}

/// Unfocused pane title.
pub fn title_unfocused() -> Style {
    Style::default().fg(TEXT_DIM)
}

/// Status bar background style.
pub fn statusbar_bg() -> Style {
    Style::default().fg(TEXT).bg(BG_DARK)
}

/// Status bar background when prefix mode is active (purple tint).
pub fn statusbar_bg_prefix() -> Style {
    Style::default().fg(TEXT).bg(Color::Rgb(40, 30, 60))
}

/// Status bar session name.
pub fn statusbar_session() -> Style {
    Style::default()
        .fg(ACCENT)
        .bg(BG_DARK)
        .add_modifier(Modifier::BOLD)
}

/// Status bar session name when prefix mode is active.
pub fn statusbar_session_prefix() -> Style {
    Style::default()
        .fg(PURPLE)
        .bg(Color::Rgb(40, 30, 60))
        .add_modifier(Modifier::BOLD)
}

/// Status bar separator.
pub fn statusbar_sep() -> Style {
    Style::default().fg(TEXT_DIM).bg(BG_DARK)
}

/// Status bar separator when prefix mode is active.
pub fn statusbar_sep_prefix() -> Style {
    Style::default().fg(PURPLE).bg(Color::Rgb(40, 30, 60))
}

/// Status bar pane info (focused pane title).
pub fn statusbar_pane_info() -> Style {
    Style::default().fg(GREEN).bg(BG_DARK)
}

/// Status bar pane info when prefix mode is active.
pub fn statusbar_pane_info_prefix() -> Style {
    Style::default().fg(GREEN).bg(Color::Rgb(40, 30, 60))
}

/// Status bar active window tab.
pub fn statusbar_window_active() -> Style {
    Style::default()
        .fg(TEXT_BRIGHT)
        .bg(BG_DARK)
        .add_modifier(Modifier::BOLD)
}

/// Status bar inactive window tab.
pub fn statusbar_window_inactive() -> Style {
    Style::default().fg(TEXT_DIM).bg(BG_DARK)
}

/// Status bar active window tab when prefix mode is active.
pub fn statusbar_window_active_prefix() -> Style {
    Style::default()
        .fg(TEXT_BRIGHT)
        .bg(Color::Rgb(40, 30, 60))
        .add_modifier(Modifier::BOLD)
}

/// Status bar inactive window tab when prefix mode is active.
pub fn statusbar_window_inactive_prefix() -> Style {
    Style::default().fg(TEXT_DIM).bg(Color::Rgb(40, 30, 60))
}

/// Help overlay border.
pub fn help_border() -> Style {
    Style::default().fg(CYAN)
}

/// Help section title.
pub fn help_section() -> Style {
    Style::default().fg(TEXT_DIM).add_modifier(Modifier::BOLD)
}

/// Help key binding.
pub fn help_key() -> Style {
    Style::default().fg(CYAN).add_modifier(Modifier::BOLD)
}

/// Help footer text.
pub fn help_footer() -> Style {
    Style::default().fg(TEXT_DIM).add_modifier(Modifier::ITALIC)
}

/// Scroll indicator background.
pub fn scroll_indicator() -> Style {
    Style::default().fg(BG_DARK).bg(TEXT_DIM)
}
