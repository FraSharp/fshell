// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Visual theme system for fshell.
//!
//! Controls every visual aspect: syntax highlighting, completion headers,
//! widget chrome, status colors, prompt/cursor, and general chrome.
//!
//! # Architecture
//!
//! - [`ThemeColor`] is a pure data enum (Named or Hex) with [`to_rgb()`](ThemeColor::to_rgb)
//!   conversion. UI-specific conversions (`to_nu()`, `to_ratatui()`) live in fshell-repl
//!   via the [`ThemeColorExt`](fshell_repl::theme_ext::ThemeColorExt) extension trait.
//! - [`Theme`] holds all sub-structs and is wrapped in `Arc<Theme>` for shared ownership.
//! - Built-in presets are const functions; user themes load from TOML with merge/inheritance.

use serde::{Deserialize, Serialize};
use std::path::Path;
// ThemeColor
/// A color that can be specified by name or hex value.
///
/// Named colors use a global dictionary (CSS/X11 + Catppuccin + Gruvbox aliases).
/// Hex colors are `#rrggbb` format.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ThemeColor {
    Named(String),
    Hex(String),
}

impl std::fmt::Display for ThemeColor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeColor::Named(s) => write!(f, "{}", s),
            ThemeColor::Hex(s) => write!(f, "{}", s),
        }
    }
}

impl Serialize for ThemeColor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            ThemeColor::Named(s) => serializer.serialize_str(s),
            ThemeColor::Hex(s) => serializer.serialize_str(s),
        }
    }
}

impl<'de> Deserialize<'de> for ThemeColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        if s.starts_with('#') {
            Ok(ThemeColor::Hex(s))
        } else {
            Ok(ThemeColor::Named(s))
        }
    }
}

impl ThemeColor {
    /// Parse a hex color string (`#rrggbb`) into RGB components.
    fn from_hex(hex: &str) -> Option<(u8, u8, u8)> {
        let hex = hex.trim_start_matches('#');
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some((r, g, b))
    }

    /// Convert to RGB. Named colors map to the closest ANSI-256 color.
    pub fn to_rgb(&self) -> (u8, u8, u8) {
        match self {
            ThemeColor::Hex(hex) => Self::from_hex(hex).unwrap_or((255, 255, 255)),
            ThemeColor::Named(name) => color_name_to_rgb(name),
        }
    }

    /// Create a ThemeColor from RGB values.
    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        ThemeColor::Hex(format!("#{:02x}{:02x}{:02x}", r, g, b))
    }
}

/// Global named color dictionary. Maps color names to RGB.
/// Supports CSS/X11 names, Catppuccin ecosystem, and Gruvbox ecosystem.
fn color_name_to_rgb(name: &str) -> (u8, u8, u8) {
    let lower = name.to_lowercase();
    match lower.as_str() {
        // CSS/X11 standard colors
        "black" => (0, 0, 0),
        "white" => (255, 255, 255),
        "red" => (205, 49, 49),
        "green" => (13, 188, 121),
        "blue" => (36, 114, 200),
        "cyan" => (17, 168, 205),
        "yellow" => (229, 192, 123),
        "magenta" | "purple" => (188, 63, 188),
        "gray" | "grey" => (128, 128, 128),
        "darkgray" | "darkgrey" => (160, 160, 160),
        "lightred" => (241, 76, 76),
        "lightgreen" => (84, 217, 140),
        "lightblue" => (130, 170, 255),
        "lightcyan" => (104, 228, 255),
        "lightyellow" => (255, 223, 186),
        "lightmagenta" | "lightpurple" => (228, 130, 228),
        "lightgray" | "lightgrey" | "lightblack" => (192, 192, 192),
        "orange" => (255, 165, 0),
        "pink" => (255, 192, 203),

        // Gruvbox colors
        "dark0_hard" => (29, 32, 33),
        "dark0" | "bg0" | "fg0" => (40, 40, 40),
        "dark0_soft" => (50, 48, 47),
        "dark1" | "bg1" | "fg1" => (60, 56, 54),
        "dark2" | "bg2" | "fg2" => (80, 73, 69),
        "dark3" | "bg3" | "fg3" => (102, 92, 84),
        "dark4" | "bg4" | "fg4" => (124, 111, 100),
        "gray_245" | "gray_244" => (146, 131, 116),
        "light0_hard" => (249, 245, 215),
        "light0" => (251, 241, 199),
        "light0_soft" => (242, 229, 188),
        "light1" => (235, 219, 178),
        "light2" => (213, 196, 161),
        "light3" => (189, 174, 147),
        "light4" => (168, 153, 132),
        "bright_red" => (251, 73, 52),
        "bright_green" => (184, 187, 38),
        "bright_yellow" => (250, 189, 47),
        "bright_blue" => (131, 165, 152),
        "bright_purple" => (211, 134, 155),
        "bright_aqua" | "aqua" => (142, 192, 124),
        "bright_orange" => (254, 128, 25),
        "neutral_red" => (204, 36, 29),
        "neutral_green" => (152, 151, 26),
        "neutral_yellow" => (215, 153, 33),
        "neutral_blue" => (69, 133, 136),
        "neutral_purple" => (177, 98, 134),
        "neutral_aqua" => (104, 157, 106),
        "neutral_orange" => (214, 93, 14),
        "faded_red" => (157, 0, 6),
        "faded_green" => (121, 116, 14),
        "faded_yellow" => (181, 118, 20),
        "faded_blue" => (7, 102, 120),
        "faded_purple" => (143, 63, 113),
        "faded_aqua" => (66, 123, 88),
        "faded_orange" => (175, 58, 3),

        // Catppuccin Latte
        "latte_rosewater" => (220, 138, 120),
        "latte_flamingo" => (221, 120, 120),
        "latte_pink" => (234, 118, 203),
        "latte_mauve" => (136, 57, 239),
        "latte_red" => (210, 15, 57),
        "latte_maroon" => (230, 69, 83),
        "latte_peach" => (254, 100, 11),
        "latte_yellow" => (223, 142, 29),
        "latte_green" => (64, 160, 43),
        "latte_teal" => (23, 146, 153),
        "latte_sky" => (4, 165, 229),
        "latte_sapphire" => (32, 159, 181),
        "latte_blue" => (30, 102, 245),
        "latte_lavender" => (114, 135, 253),
        "latte_text" => (76, 79, 105),
        "latte_subtext1" => (92, 95, 119),
        "latte_subtext0" => (108, 111, 133),
        "latte_overlay2" => (124, 127, 147),
        "latte_overlay1" => (140, 143, 161),
        "latte_overlay0" => (156, 160, 176),
        "latte_surface2" => (172, 176, 190),
        "latte_surface1" => (188, 192, 204),
        "latte_surface0" => (204, 208, 218),
        "latte_base" => (239, 241, 245),
        "latte_mantle" => (230, 233, 239),
        "latte_crust" => (220, 224, 232),

        // Catppuccin Frappé
        "frappe_rosewater" => (242, 213, 207),
        "frappe_flamingo" => (238, 190, 190),
        "frappe_pink" => (244, 184, 228),
        "frappe_mauve" => (202, 158, 230),
        "frappe_red" => (231, 130, 132),
        "frappe_maroon" => (234, 153, 156),
        "frappe_peach" => (239, 159, 118),
        "frappe_yellow" => (229, 200, 144),
        "frappe_green" => (166, 209, 137),
        "frappe_teal" => (129, 200, 190),
        "frappe_sky" => (153, 209, 219),
        "frappe_sapphire" => (133, 193, 220),
        "frappe_blue" => (140, 170, 238),
        "frappe_lavender" => (186, 187, 241),
        "frappe_text" => (198, 208, 245),
        "frappe_subtext1" => (181, 191, 226),
        "frappe_subtext0" => (165, 173, 206),
        "frappe_overlay2" => (148, 156, 187),
        "frappe_overlay1" => (131, 139, 167),
        "frappe_overlay0" => (115, 121, 148),
        "frappe_surface2" => (98, 104, 128),
        "frappe_surface1" => (81, 87, 109),
        "frappe_surface0" => (65, 69, 89),
        "frappe_base" => (48, 52, 70),
        "frappe_mantle" => (41, 44, 60),
        "frappe_crust" => (35, 38, 52),

        // Catppuccin Macchiato
        "macchiato_rosewater" => (244, 219, 214),
        "macchiato_flamingo" => (240, 198, 198),
        "macchiato_pink" => (245, 189, 230),
        "macchiato_mauve" => (198, 160, 246),
        "macchiato_red" => (237, 135, 150),
        "macchiato_maroon" => (238, 153, 160),
        "macchiato_peach" => (245, 169, 127),
        "macchiato_yellow" => (238, 212, 159),
        "macchiato_green" => (166, 218, 149),
        "macchiato_teal" => (139, 213, 202),
        "macchiato_sky" => (145, 215, 227),
        "macchiato_sapphire" => (125, 196, 228),
        "macchiato_blue" => (138, 173, 244),
        "macchiato_lavender" => (183, 189, 248),
        "macchiato_text" => (202, 211, 245),
        "macchiato_subtext1" => (184, 192, 224),
        "macchiato_subtext0" => (165, 173, 203),
        "macchiato_overlay2" => (147, 154, 183),
        "macchiato_overlay1" => (128, 135, 162),
        "macchiato_overlay0" => (110, 115, 141),
        "macchiato_surface2" => (91, 96, 120),
        "macchiato_surface1" => (73, 77, 100),
        "macchiato_surface0" => (54, 58, 79),
        "macchiato_base" => (36, 39, 58),
        "macchiato_mantle" => (30, 32, 48),
        "macchiato_crust" => (24, 25, 38),

        // Catppuccin Mocha / prefix-less defaults
        "mocha_rosewater" | "rosewater" => (245, 224, 220),
        "mocha_flamingo" | "flamingo" => (242, 205, 205),
        "mocha_pink" => (245, 194, 231),
        "mocha_mauve" | "mauve" => (203, 166, 247),
        "mocha_red" => (243, 139, 168),
        "mocha_maroon" | "maroon" => (235, 160, 172),
        "mocha_peach" | "peach" => (250, 179, 135),
        "mocha_yellow" => (249, 226, 175),
        "mocha_green" => (166, 227, 161),
        "mocha_teal" | "teal" => (148, 226, 213),
        "mocha_sky" | "sky" => (137, 220, 235),
        "mocha_sapphire" | "sapphire" => (116, 199, 236),
        "mocha_blue" => (137, 180, 250),
        "mocha_lavender" | "lavender" => (180, 190, 254),
        "mocha_text" | "text" => (205, 214, 244),
        "mocha_subtext1" | "subtext1" => (186, 194, 222),
        "mocha_subtext0" | "subtext0" => (166, 173, 200),
        "mocha_overlay2" | "overlay2" => (147, 153, 178),
        "mocha_overlay1" | "overlay1" => (127, 132, 156),
        "mocha_overlay0" | "overlay0" => (108, 112, 134),
        "mocha_surface2" | "surface2" => (88, 91, 112),
        "mocha_surface1" | "surface1" => (69, 71, 90),
        "mocha_surface0" | "surface0" => (49, 50, 68),
        "mocha_base" | "base" => (30, 30, 46),
        "mocha_mantle" | "mantle" => (24, 24, 37),
        "mocha_crust" | "crust" => (17, 17, 27),

        // Nord palette
        "nord0" => (46, 52, 64),
        "nord1" => (59, 66, 82),
        "nord2" => (67, 76, 94),
        "nord3" => (76, 86, 106),
        "nord4" => (216, 222, 233),
        "nord5" => (229, 233, 240),
        "nord6" => (236, 239, 244),
        "nord7" => (143, 188, 187),
        "nord8" => (136, 192, 208),
        "nord9" => (129, 161, 193),
        "nord10" => (94, 129, 172),
        "nord11" => (191, 97, 106),
        "nord12" => (208, 135, 112),
        "nord13" => (235, 203, 139),
        "nord14" => (163, 190, 140),
        "nord15" => (180, 142, 173),

        // Solarized palette
        "solarized_base03" => (0, 43, 54),
        "solarized_base02" => (7, 54, 66),
        "solarized_base01" => (88, 110, 117),
        "solarized_base00" => (101, 123, 131),
        "solarized_base0" => (131, 148, 150),
        "solarized_base1" => (147, 161, 161),
        "solarized_base2" => (238, 232, 213),
        "solarized_base3" => (253, 246, 227),
        "solarized_yellow" => (181, 137, 0),
        "solarized_orange" => (203, 75, 22),
        "solarized_red" => (220, 50, 47),
        "solarized_magenta" => (211, 54, 130),
        "solarized_violet" => (108, 113, 196),
        "solarized_blue" => (38, 139, 210),
        "solarized_cyan" => (42, 161, 152),
        "solarized_green" => (133, 153, 0),

        // Tokyo Night palette
        "tokyo_bg" => (26, 27, 38),
        "tokyo_fg" => (192, 202, 245),
        "tokyo_black" => (21, 22, 30),
        "tokyo_red" => (247, 118, 142),
        "tokyo_green" => (158, 206, 106),
        "tokyo_yellow" => (224, 175, 104),
        "tokyo_blue" => (122, 162, 247),
        "tokyo_magenta" => (187, 154, 247),
        "tokyo_cyan" => (125, 207, 255),
        "tokyo_white" => (169, 177, 214),
        "tokyo_comment" => (86, 95, 137),

        // Fallback: try to parse as hex (without #)
        other => {
            if let Some((r, g, b)) = ThemeColor::from_hex(other) {
                (r, g, b)
            } else {
                // Unknown color: white fallback
                (255, 255, 255)
            }
        }
    }
}
// BorderStyle, CursorStyle
/// Border style for TUI widgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum BorderStyle {
    #[default]
    Rounded,
    Single,
    Double,
    Thick,
    None,
}

/// Cursor display style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum CursorStyle {
    #[default]
    Block,
    Underline,
    Bar,
    Reverse,
}
// Sub-structs
/// Syntax highlighting colors.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SyntaxTheme {
    pub keyword: ThemeColor,
    pub builtin: ThemeColor,
    pub function: ThemeColor,
    pub variable: ThemeColor,
    pub string: ThemeColor,
    pub number: ThemeColor,
    pub operator: ThemeColor,
    pub pipe: ThemeColor,
    pub comment: ThemeColor,
    pub escape: ThemeColor,
    pub alias: ThemeColor,
    pub unknown_command: ThemeColor,
    pub normal_text: ThemeColor,
    pub separator: ThemeColor,
    pub punctuation: ThemeColor,
    pub type_name: ThemeColor,
}

impl SyntaxTheme {
    pub fn merge(&mut self, other: SyntaxThemeToml) {
        if let Some(v) = other.keyword {
            self.keyword = v;
        }
        if let Some(v) = other.builtin {
            self.builtin = v;
        }
        if let Some(v) = other.function {
            self.function = v;
        }
        if let Some(v) = other.variable {
            self.variable = v;
        }
        if let Some(v) = other.string {
            self.string = v;
        }
        if let Some(v) = other.number {
            self.number = v;
        }
        if let Some(v) = other.operator {
            self.operator = v;
        }
        if let Some(v) = other.pipe {
            self.pipe = v;
        }
        if let Some(v) = other.comment {
            self.comment = v;
        }
        if let Some(v) = other.escape {
            self.escape = v;
        }
        if let Some(v) = other.alias {
            self.alias = v;
        }
        if let Some(v) = other.unknown_command {
            self.unknown_command = v;
        }
        if let Some(v) = other.normal_text {
            self.normal_text = v;
        }
        if let Some(v) = other.separator {
            self.separator = v;
        }
        if let Some(v) = other.punctuation {
            self.punctuation = v;
        }
        if let Some(v) = other.type_name {
            self.type_name = v;
        }
    }
}

/// Completion menu colors per category.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CompletionsTheme {
    pub header_directory: ThemeColor,
    pub header_file: ThemeColor,
    pub header_command: ThemeColor,
    pub header_builtin: ThemeColor,
    pub header_alias: ThemeColor,
    pub header_function: ThemeColor,
    pub header_variable: ThemeColor,
    pub header_flag: ThemeColor,
    pub header_pipeline: ThemeColor,
    pub header_keyword: ThemeColor,
    pub header_job: ThemeColor,
    pub header_history: ThemeColor,
    pub header_ref: ThemeColor,
    pub header_default: ThemeColor,
    pub description: ThemeColor,
    pub icon: ThemeColor,
}

impl CompletionsTheme {
    pub fn merge(&mut self, other: CompletionsThemeToml) {
        if let Some(v) = other.header_directory {
            self.header_directory = v;
        }
        if let Some(v) = other.header_file {
            self.header_file = v;
        }
        if let Some(v) = other.header_command {
            self.header_command = v;
        }
        if let Some(v) = other.header_builtin {
            self.header_builtin = v;
        }
        if let Some(v) = other.header_alias {
            self.header_alias = v;
        }
        if let Some(v) = other.header_function {
            self.header_function = v;
        }
        if let Some(v) = other.header_variable {
            self.header_variable = v;
        }
        if let Some(v) = other.header_flag {
            self.header_flag = v;
        }
        if let Some(v) = other.header_pipeline {
            self.header_pipeline = v;
        }
        if let Some(v) = other.header_keyword {
            self.header_keyword = v;
        }
        if let Some(v) = other.header_job {
            self.header_job = v;
        }
        if let Some(v) = other.header_history {
            self.header_history = v;
        }
        if let Some(v) = other.header_ref {
            self.header_ref = v;
        }
        if let Some(v) = other.header_default {
            self.header_default = v;
        }
        if let Some(v) = other.description {
            self.description = v;
        }
        if let Some(v) = other.icon {
            self.icon = v;
        }
    }
}

/// Widget chrome colors (borders, titles, backgrounds, selection).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WidgetTheme {
    pub border: ThemeColor,
    pub border_focus: ThemeColor,
    pub title: ThemeColor,
    pub title_focus: ThemeColor,
    pub background: ThemeColor,
    pub foreground: ThemeColor,
    pub item_selected_bg: ThemeColor,
    pub item_selected_fg: ThemeColor,
    pub border_style: BorderStyle,
}

impl WidgetTheme {
    pub fn merge(&mut self, other: WidgetThemeToml) {
        if let Some(v) = other.border {
            self.border = v;
        }
        if let Some(v) = other.border_focus {
            self.border_focus = v;
        }
        if let Some(v) = other.title {
            self.title = v;
        }
        if let Some(v) = other.title_focus {
            self.title_focus = v;
        }
        if let Some(v) = other.background {
            self.background = v;
        }
        if let Some(v) = other.foreground {
            self.foreground = v;
        }
        if let Some(v) = other.item_selected_bg {
            self.item_selected_bg = v;
        }
        if let Some(v) = other.item_selected_fg {
            self.item_selected_fg = v;
        }
        if let Some(v) = other.border_style {
            self.border_style = v;
        }
    }
}

/// Status indicator colors.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StatusTheme {
    pub ok: ThemeColor,
    pub error: ThemeColor,
    pub warning: ThemeColor,
    pub info: ThemeColor,
    pub muted: ThemeColor,
}

impl StatusTheme {
    pub fn merge(&mut self, other: StatusThemeToml) {
        if let Some(v) = other.ok {
            self.ok = v;
        }
        if let Some(v) = other.error {
            self.error = v;
        }
        if let Some(v) = other.warning {
            self.warning = v;
        }
        if let Some(v) = other.info {
            self.info = v;
        }
        if let Some(v) = other.muted {
            self.muted = v;
        }
    }
}

/// Prompt and cursor colors.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PromptTheme {
    pub cursor_color: ThemeColor,
    pub cursor_unfocused: ThemeColor,
    pub cursor_style: CursorStyle,
    pub input_fg: ThemeColor,
    pub selection_bg: ThemeColor,
    pub selection_fg: ThemeColor,

    // First-class prompt segment and decorator colors
    #[serde(default = "default_user_color")]
    pub user: ThemeColor,
    #[serde(default = "default_host_color")]
    pub host: ThemeColor,
    #[serde(default = "default_pwd_color")]
    pub pwd: ThemeColor,
    #[serde(default = "default_git_branch_color")]
    pub git_branch: ThemeColor,
    #[serde(default = "default_git_status_clean_color")]
    pub git_status_clean: ThemeColor,
    #[serde(default = "default_git_status_dirty_color")]
    pub git_status_dirty: ThemeColor,
    #[serde(default = "default_exit_ok_color")]
    pub exit_ok: ThemeColor,
    #[serde(default = "default_exit_error_color")]
    pub exit_error: ThemeColor,
    #[serde(default = "default_duration_color")]
    pub duration: ThemeColor,
    #[serde(default = "default_job_count_color")]
    pub job_count: ThemeColor,
    #[serde(default = "default_prompt_symbol_color")]
    pub prompt_symbol: ThemeColor,
    #[serde(default = "default_prompt_symbol_root_color")]
    pub prompt_symbol_root: ThemeColor,
}

fn default_user_color() -> ThemeColor {
    ThemeColor::Hex("#d2a8ff".into())
}
fn default_host_color() -> ThemeColor {
    ThemeColor::Hex("#79c0ff".into())
}
fn default_pwd_color() -> ThemeColor {
    ThemeColor::Hex("#a5d6ff".into())
}
fn default_git_branch_color() -> ThemeColor {
    ThemeColor::Hex("#7ee787".into())
}
fn default_git_status_clean_color() -> ThemeColor {
    ThemeColor::Hex("#7ee787".into())
}
fn default_git_status_dirty_color() -> ThemeColor {
    ThemeColor::Hex("#d29922".into())
}
fn default_exit_ok_color() -> ThemeColor {
    ThemeColor::Hex("#7ee787".into())
}
fn default_exit_error_color() -> ThemeColor {
    ThemeColor::Hex("#ff7b72".into())
}
fn default_duration_color() -> ThemeColor {
    ThemeColor::Hex("#6e7681".into())
}
fn default_job_count_color() -> ThemeColor {
    ThemeColor::Hex("#79c0ff".into())
}
fn default_prompt_symbol_color() -> ThemeColor {
    ThemeColor::Hex("#7ee787".into())
}
fn default_prompt_symbol_root_color() -> ThemeColor {
    ThemeColor::Hex("#ff7b72".into())
}

impl PromptTheme {
    pub fn merge(&mut self, other: PromptThemeToml) {
        if let Some(v) = other.cursor_color {
            self.cursor_color = v;
        }
        if let Some(v) = other.cursor_unfocused {
            self.cursor_unfocused = v;
        }
        if let Some(v) = other.cursor_style {
            self.cursor_style = v;
        }
        if let Some(v) = other.input_fg {
            self.input_fg = v;
        }
        if let Some(v) = other.selection_bg {
            self.selection_bg = v;
        }
        if let Some(v) = other.selection_fg {
            self.selection_fg = v;
        }
        if let Some(v) = other.user {
            self.user = v;
        }
        if let Some(v) = other.host {
            self.host = v;
        }
        if let Some(v) = other.pwd {
            self.pwd = v;
        }
        if let Some(v) = other.git_branch {
            self.git_branch = v;
        }
        if let Some(v) = other.git_status_clean {
            self.git_status_clean = v;
        }
        if let Some(v) = other.git_status_dirty {
            self.git_status_dirty = v;
        }
        if let Some(v) = other.exit_ok {
            self.exit_ok = v;
        }
        if let Some(v) = other.exit_error {
            self.exit_error = v;
        }
        if let Some(v) = other.duration {
            self.duration = v;
        }
        if let Some(v) = other.job_count {
            self.job_count = v;
        }
        if let Some(v) = other.prompt_symbol {
            self.prompt_symbol = v;
        }
        if let Some(v) = other.prompt_symbol_root {
            self.prompt_symbol_root = v;
        }
    }
}

/// General chrome (background, foreground, border style).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChromeTheme {
    pub background: ThemeColor,
    pub foreground: ThemeColor,
    pub border_style: BorderStyle,
}

impl ChromeTheme {
    pub fn merge(&mut self, other: ChromeThemeToml) {
        if let Some(v) = other.background {
            self.background = v;
        }
        if let Some(v) = other.foreground {
            self.foreground = v;
        }
        if let Some(v) = other.border_style {
            self.border_style = v;
        }
    }
}
// Theme
/// Complete visual theme for fshell.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub inherits: Option<String>,
    pub syntax: SyntaxTheme,
    pub completions: CompletionsTheme,
    pub widgets: WidgetTheme,
    pub prompt: PromptTheme,
    pub status: StatusTheme,
    pub chrome: ChromeTheme,
}

impl Theme {
    /// Check if this theme has a dark background.
    pub fn is_dark(&self) -> bool {
        let (r, g, b) = self.chrome.background.to_rgb();
        // Relative luminance formula approximation
        (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000 < 128
    }

    /// Resolve a color name, hex code, or semantic theme role into RGB.
    pub fn resolve_color(&self, spec: &str) -> (u8, u8, u8) {
        let s = spec.trim();
        if s.is_empty() {
            return self.chrome.foreground.to_rgb();
        }

        // 1. Direct hex (`#RRGGBB` or `#RGB`)
        if s.starts_with('#') {
            return ThemeColor::Hex(s.to_string()).to_rgb();
        }

        // 2. Semantic theme roles
        let normalized = s.to_lowercase();
        let semantic_match = match normalized.as_str() {
            "keyword" => Some(&self.syntax.keyword),
            "builtin" => Some(&self.syntax.builtin),
            "function" => Some(&self.syntax.function),
            "variable" => Some(&self.syntax.variable),
            "string" => Some(&self.syntax.string),
            "number" => Some(&self.syntax.number),
            "operator" => Some(&self.syntax.operator),
            "pipe" => Some(&self.syntax.pipe),
            "comment" => Some(&self.syntax.comment),
            "escape" => Some(&self.syntax.escape),
            "alias" => Some(&self.syntax.alias),
            "unknown_command" => Some(&self.syntax.unknown_command),
            "normal_text" | "text" => Some(&self.syntax.normal_text),
            "separator" => Some(&self.syntax.separator),
            "punctuation" => Some(&self.syntax.punctuation),
            "type_name" => Some(&self.syntax.type_name),
            "ok" | "success" => Some(&self.status.ok),
            "error" | "err" | "fail" => Some(&self.status.error),
            "warning" | "warn" => Some(&self.status.warning),
            "info" => Some(&self.status.info),
            "muted" => Some(&self.status.muted),
            "user" => Some(&self.prompt.user),
            "host" => Some(&self.prompt.host),
            "pwd" => Some(&self.prompt.pwd),
            "git_branch" | "branch" => Some(&self.prompt.git_branch),
            "git_clean" => Some(&self.prompt.git_status_clean),
            "git_dirty" => Some(&self.prompt.git_status_dirty),
            "duration" => Some(&self.prompt.duration),
            "jobs" => Some(&self.prompt.job_count),
            "prompt_symbol" | "char" => Some(&self.prompt.prompt_symbol),
            "cursor" => Some(&self.prompt.cursor_color),
            "border" => Some(&self.widgets.border),
            "title" => Some(&self.widgets.title),
            "background" | "bg" => Some(&self.chrome.background),
            "foreground" | "fg" => Some(&self.chrome.foreground),
            _ => None,
        };

        if let Some(tc) = semantic_match {
            return tc.to_rgb();
        }

        // 3. Global color dictionary
        ThemeColor::Named(s.to_string()).to_rgb()
    }

    /// Load a theme by name. Checks user themes first, then built-in presets.
    pub fn load(name: &str, config_dir: &Path) -> Result<Self, ThemeError> {
        // 1. Check user theme directory: ~/.config/fsh/themes/<name>.toml or ~/.config/fshell/themes/<name>.toml
        let user_theme_path = config_dir.join("themes").join(format!("{}.toml", name));
        if user_theme_path.exists() {
            let toml_str = std::fs::read_to_string(&user_theme_path)
                .map_err(|e| ThemeError::Io(e.to_string()))?;
            let user_theme: ThemeToml =
                toml_edit::de::from_str(&toml_str).map_err(|e| ThemeError::Parse(e.to_string()))?;
            return Self::from_toml(user_theme, config_dir);
        }

        // 2. Check built-in presets
        match name {
            "default" | "github-dark" => Ok(Self::default_theme()),
            "github-light" => Ok(Self::github_light()),
            "catppuccin" | "catppuccin-mocha" => Ok(Self::catppuccin_mocha()),
            "catppuccin-latte" => Ok(Self::catppuccin_latte()),
            "dracula" => Ok(Self::dracula()),
            "gruvbox" | "gruvbox-dark" => Ok(Self::gruvbox_dark()),
            "gruvbox-light" => Ok(Self::gruvbox_light()),
            "nord" => Ok(Self::nord()),
            "tokyo-night" => Ok(Self::tokyo_night()),
            "solarized-dark" => Ok(Self::solarized_dark()),
            "solarized-light" => Ok(Self::solarized_light()),
            _ => Err(ThemeError::NotFound(name.into())),
        }
    }

    /// List all available theme names (built-in + user).
    pub fn available(config_dir: &Path) -> Vec<String> {
        let mut names = vec![
            "default".into(),
            "github-dark".into(),
            "github-light".into(),
            "catppuccin".into(),
            "catppuccin-mocha".into(),
            "catppuccin-latte".into(),
            "dracula".into(),
            "gruvbox".into(),
            "gruvbox-dark".into(),
            "gruvbox-light".into(),
            "nord".into(),
            "tokyo-night".into(),
            "solarized-dark".into(),
            "solarized-light".into(),
        ];

        // Scan user theme directory
        let themes_dir = config_dir.join("themes");
        if let Ok(entries) = std::fs::read_dir(&themes_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if !names.contains(&stem.to_string()) {
                            names.push(stem.to_string());
                        }
                    }
                }
            }
        }

        names.sort();
        names.dedup();
        names
    }

    /// Merge a TOML overlay on top of a base theme.
    fn from_toml(toml: ThemeToml, config_dir: &Path) -> Result<Self, ThemeError> {
        // Start with base default
        let mut theme = Self::default_theme();

        // If inherits is set, overlay the preset on top of base
        if let Some(ref base_name) = toml.inherits {
            let base = Self::load(base_name, config_dir)?;
            theme = base;
        }

        // Override with explicit TOML values
        if let Some(name) = toml.name {
            theme.name = name;
        }
        if let Some(syntax) = toml.syntax {
            theme.syntax.merge(syntax);
        }
        if let Some(completions) = toml.completions {
            theme.completions.merge(completions);
        }
        if let Some(widgets) = toml.widgets {
            theme.widgets.merge(widgets);
        }
        if let Some(status) = toml.status {
            theme.status.merge(status);
        }
        if let Some(prompt) = toml.prompt {
            theme.prompt.merge(prompt);
        }
        if let Some(chrome) = toml.chrome {
            theme.chrome.merge(chrome);
        }

        Ok(theme)
    }
}
// TOML deserialization types
#[derive(Debug, Deserialize)]
pub struct ThemeToml {
    pub name: Option<String>,
    pub inherits: Option<String>,
    pub syntax: Option<SyntaxThemeToml>,
    pub completions: Option<CompletionsThemeToml>,
    pub widgets: Option<WidgetThemeToml>,
    pub prompt: Option<PromptThemeToml>,
    pub status: Option<StatusThemeToml>,
    pub chrome: Option<ChromeThemeToml>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SyntaxThemeToml {
    pub keyword: Option<ThemeColor>,
    pub builtin: Option<ThemeColor>,
    pub function: Option<ThemeColor>,
    pub variable: Option<ThemeColor>,
    pub string: Option<ThemeColor>,
    pub number: Option<ThemeColor>,
    pub operator: Option<ThemeColor>,
    pub pipe: Option<ThemeColor>,
    pub comment: Option<ThemeColor>,
    pub escape: Option<ThemeColor>,
    pub alias: Option<ThemeColor>,
    pub unknown_command: Option<ThemeColor>,
    pub normal_text: Option<ThemeColor>,
    pub separator: Option<ThemeColor>,
    pub punctuation: Option<ThemeColor>,
    pub type_name: Option<ThemeColor>,
}

#[derive(Debug, Default, Deserialize)]
pub struct CompletionsThemeToml {
    pub header_directory: Option<ThemeColor>,
    pub header_file: Option<ThemeColor>,
    pub header_command: Option<ThemeColor>,
    pub header_builtin: Option<ThemeColor>,
    pub header_alias: Option<ThemeColor>,
    pub header_function: Option<ThemeColor>,
    pub header_variable: Option<ThemeColor>,
    pub header_flag: Option<ThemeColor>,
    pub header_pipeline: Option<ThemeColor>,
    pub header_keyword: Option<ThemeColor>,
    pub header_job: Option<ThemeColor>,
    pub header_history: Option<ThemeColor>,
    pub header_ref: Option<ThemeColor>,
    pub header_default: Option<ThemeColor>,
    pub description: Option<ThemeColor>,
    pub icon: Option<ThemeColor>,
}

#[derive(Debug, Default, Deserialize)]
pub struct WidgetThemeToml {
    pub border: Option<ThemeColor>,
    pub border_focus: Option<ThemeColor>,
    pub title: Option<ThemeColor>,
    pub title_focus: Option<ThemeColor>,
    pub background: Option<ThemeColor>,
    pub foreground: Option<ThemeColor>,
    pub item_selected_bg: Option<ThemeColor>,
    pub item_selected_fg: Option<ThemeColor>,
    pub border_style: Option<BorderStyle>,
}

#[derive(Debug, Default, Deserialize)]
pub struct StatusThemeToml {
    pub ok: Option<ThemeColor>,
    pub error: Option<ThemeColor>,
    pub warning: Option<ThemeColor>,
    pub info: Option<ThemeColor>,
    pub muted: Option<ThemeColor>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PromptThemeToml {
    pub cursor_color: Option<ThemeColor>,
    pub cursor_unfocused: Option<ThemeColor>,
    pub cursor_style: Option<CursorStyle>,
    pub input_fg: Option<ThemeColor>,
    pub selection_bg: Option<ThemeColor>,
    pub selection_fg: Option<ThemeColor>,
    pub user: Option<ThemeColor>,
    pub host: Option<ThemeColor>,
    pub pwd: Option<ThemeColor>,
    pub git_branch: Option<ThemeColor>,
    pub git_status_clean: Option<ThemeColor>,
    pub git_status_dirty: Option<ThemeColor>,
    pub exit_ok: Option<ThemeColor>,
    pub exit_error: Option<ThemeColor>,
    pub duration: Option<ThemeColor>,
    pub job_count: Option<ThemeColor>,
    pub prompt_symbol: Option<ThemeColor>,
    pub prompt_symbol_root: Option<ThemeColor>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ChromeThemeToml {
    pub background: Option<ThemeColor>,
    pub foreground: Option<ThemeColor>,
    pub border_style: Option<BorderStyle>,
}
// Errors
#[derive(Debug, Clone)]
pub enum ThemeError {
    NotFound(String),
    Io(String),
    Parse(String),
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeError::NotFound(name) => write!(f, "theme not found: {}", name),
            ThemeError::Io(msg) => write!(f, "IO error loading theme: {}", msg),
            ThemeError::Parse(msg) => write!(f, "error parsing theme TOML: {}", msg),
        }
    }
}

impl std::error::Error for ThemeError {}
// Built-in presets
impl Theme {
    /// Default fshell theme — GitHub Dark inspired.
    pub fn default_theme() -> Self {
        Self {
            name: "default".into(),
            inherits: None,
            syntax: SyntaxTheme {
                keyword: ThemeColor::Hex("#d2a8ff".into()),
                builtin: ThemeColor::Hex("#79c0ff".into()),
                function: ThemeColor::Hex("#d2a8ff".into()),
                variable: ThemeColor::Hex("#ffa657".into()),
                string: ThemeColor::Hex("#a5d6ff".into()),
                number: ThemeColor::Hex("#ffa657".into()),
                operator: ThemeColor::Hex("#e1e4e8".into()),
                pipe: ThemeColor::Hex("#79c0ff".into()),
                comment: ThemeColor::Hex("#6e7681".into()),
                escape: ThemeColor::Hex("#79c0ff".into()),
                alias: ThemeColor::Hex("#d2a8ff".into()),
                unknown_command: ThemeColor::Hex("#ff7b72".into()),
                normal_text: ThemeColor::Hex("#e1e4e8".into()),
                separator: ThemeColor::Hex("#e1e4e8".into()),
                punctuation: ThemeColor::Hex("#8b949e".into()),
                type_name: ThemeColor::Hex("#ffa657".into()),
            },
            completions: CompletionsTheme {
                header_directory: ThemeColor::Hex("#7ee787".into()),
                header_file: ThemeColor::Hex("#7ee787".into()),
                header_command: ThemeColor::Hex("#79c0ff".into()),
                header_builtin: ThemeColor::Hex("#d2a8ff".into()),
                header_alias: ThemeColor::Hex("#d2a8ff".into()),
                header_function: ThemeColor::Hex("#d2a8ff".into()),
                header_variable: ThemeColor::Hex("#ffa657".into()),
                header_flag: ThemeColor::Hex("#79c0ff".into()),
                header_pipeline: ThemeColor::Hex("#79c0ff".into()),
                header_keyword: ThemeColor::Hex("#d2a8ff".into()),
                header_job: ThemeColor::Hex("#ffa657".into()),
                header_history: ThemeColor::Hex("#6e7681".into()),
                header_ref: ThemeColor::Hex("#6e7681".into()),
                header_default: ThemeColor::Hex("#6e7681".into()),
                description: ThemeColor::Hex("#6e7681".into()),
                icon: ThemeColor::Hex("#6e7681".into()),
            },
            widgets: WidgetTheme {
                border: ThemeColor::Hex("#30363d".into()),
                border_focus: ThemeColor::Hex("#58a6ff".into()),
                title: ThemeColor::Hex("#58a6ff".into()),
                title_focus: ThemeColor::Hex("#58a6ff".into()),
                background: ThemeColor::Hex("#0d1117".into()),
                foreground: ThemeColor::Hex("#e1e4e8".into()),
                item_selected_bg: ThemeColor::Hex("#264f78".into()),
                item_selected_fg: ThemeColor::Hex("#e1e4e8".into()),
                border_style: BorderStyle::Rounded,
            },
            prompt: PromptTheme {
                cursor_color: ThemeColor::Hex("#58a6ff".into()),
                cursor_unfocused: ThemeColor::Hex("#6e7681".into()),
                cursor_style: CursorStyle::Block,
                input_fg: ThemeColor::Hex("#e1e4e8".into()),
                selection_bg: ThemeColor::Hex("#264f78".into()),
                selection_fg: ThemeColor::Hex("#e1e4e8".into()),
                user: ThemeColor::Hex("#d2a8ff".into()),
                host: ThemeColor::Hex("#79c0ff".into()),
                pwd: ThemeColor::Hex("#a5d6ff".into()),
                git_branch: ThemeColor::Hex("#7ee787".into()),
                git_status_clean: ThemeColor::Hex("#7ee787".into()),
                git_status_dirty: ThemeColor::Hex("#d29922".into()),
                exit_ok: ThemeColor::Hex("#7ee787".into()),
                exit_error: ThemeColor::Hex("#ff7b72".into()),
                duration: ThemeColor::Hex("#6e7681".into()),
                job_count: ThemeColor::Hex("#79c0ff".into()),
                prompt_symbol: ThemeColor::Hex("#7ee787".into()),
                prompt_symbol_root: ThemeColor::Hex("#ff7b72".into()),
            },
            status: StatusTheme {
                ok: ThemeColor::Hex("#7ee787".into()),
                error: ThemeColor::Hex("#ff7b72".into()),
                warning: ThemeColor::Hex("#d29922".into()),
                info: ThemeColor::Hex("#58a6ff".into()),
                muted: ThemeColor::Hex("#6e7681".into()),
            },
            chrome: ChromeTheme {
                background: ThemeColor::Hex("#0d1117".into()),
                foreground: ThemeColor::Hex("#e1e4e8".into()),
                border_style: BorderStyle::Rounded,
            },
        }
    }

    /// GitHub Light — crisp and modern light theme.
    pub fn github_light() -> Self {
        Self {
            name: "github-light".into(),
            inherits: None,
            syntax: SyntaxTheme {
                keyword: ThemeColor::Hex("#8250df".into()),
                builtin: ThemeColor::Hex("#0550ae".into()),
                function: ThemeColor::Hex("#8250df".into()),
                variable: ThemeColor::Hex("#953800".into()),
                string: ThemeColor::Hex("#0a3069".into()),
                number: ThemeColor::Hex("#0550ae".into()),
                operator: ThemeColor::Hex("#24292f".into()),
                pipe: ThemeColor::Hex("#0550ae".into()),
                comment: ThemeColor::Hex("#6e7781".into()),
                escape: ThemeColor::Hex("#0550ae".into()),
                alias: ThemeColor::Hex("#8250df".into()),
                unknown_command: ThemeColor::Hex("#cf222e".into()),
                normal_text: ThemeColor::Hex("#24292f".into()),
                separator: ThemeColor::Hex("#24292f".into()),
                punctuation: ThemeColor::Hex("#57606a".into()),
                type_name: ThemeColor::Hex("#953800".into()),
            },
            completions: CompletionsTheme {
                header_directory: ThemeColor::Hex("#1a7f37".into()),
                header_file: ThemeColor::Hex("#1a7f37".into()),
                header_command: ThemeColor::Hex("#0550ae".into()),
                header_builtin: ThemeColor::Hex("#8250df".into()),
                header_alias: ThemeColor::Hex("#8250df".into()),
                header_function: ThemeColor::Hex("#8250df".into()),
                header_variable: ThemeColor::Hex("#953800".into()),
                header_flag: ThemeColor::Hex("#0969da".into()),
                header_pipeline: ThemeColor::Hex("#0969da".into()),
                header_keyword: ThemeColor::Hex("#8250df".into()),
                header_job: ThemeColor::Hex("#953800".into()),
                header_history: ThemeColor::Hex("#6e7781".into()),
                header_ref: ThemeColor::Hex("#6e7781".into()),
                header_default: ThemeColor::Hex("#6e7781".into()),
                description: ThemeColor::Hex("#6e7781".into()),
                icon: ThemeColor::Hex("#6e7781".into()),
            },
            widgets: WidgetTheme {
                border: ThemeColor::Hex("#d0d7de".into()),
                border_focus: ThemeColor::Hex("#0969da".into()),
                title: ThemeColor::Hex("#0969da".into()),
                title_focus: ThemeColor::Hex("#0969da".into()),
                background: ThemeColor::Hex("#ffffff".into()),
                foreground: ThemeColor::Hex("#24292f".into()),
                item_selected_bg: ThemeColor::Hex("#ddf4ff".into()),
                item_selected_fg: ThemeColor::Hex("#0969da".into()),
                border_style: BorderStyle::Rounded,
            },
            prompt: PromptTheme {
                cursor_color: ThemeColor::Hex("#0969da".into()),
                cursor_unfocused: ThemeColor::Hex("#6e7781".into()),
                cursor_style: CursorStyle::Block,
                input_fg: ThemeColor::Hex("#24292f".into()),
                selection_bg: ThemeColor::Hex("#ddf4ff".into()),
                selection_fg: ThemeColor::Hex("#0969da".into()),
                user: ThemeColor::Hex("#8250df".into()),
                host: ThemeColor::Hex("#0550ae".into()),
                pwd: ThemeColor::Hex("#0a3069".into()),
                git_branch: ThemeColor::Hex("#1a7f37".into()),
                git_status_clean: ThemeColor::Hex("#1a7f37".into()),
                git_status_dirty: ThemeColor::Hex("#9a6700".into()),
                exit_ok: ThemeColor::Hex("#1a7f37".into()),
                exit_error: ThemeColor::Hex("#cf222e".into()),
                duration: ThemeColor::Hex("#6e7781".into()),
                job_count: ThemeColor::Hex("#0550ae".into()),
                prompt_symbol: ThemeColor::Hex("#1a7f37".into()),
                prompt_symbol_root: ThemeColor::Hex("#cf222e".into()),
            },
            status: StatusTheme {
                ok: ThemeColor::Hex("#1a7f37".into()),
                error: ThemeColor::Hex("#cf222e".into()),
                warning: ThemeColor::Hex("#9a6700".into()),
                info: ThemeColor::Hex("#0969da".into()),
                muted: ThemeColor::Hex("#6e7781".into()),
            },
            chrome: ChromeTheme {
                background: ThemeColor::Hex("#ffffff".into()),
                foreground: ThemeColor::Hex("#24292f".into()),
                border_style: BorderStyle::Rounded,
            },
        }
    }

    /// Catppuccin Mocha — warm pastels on creamy dark background.
    pub fn catppuccin_mocha() -> Self {
        Self {
            name: "catppuccin".into(),
            inherits: None,
            syntax: SyntaxTheme {
                keyword: ThemeColor::Hex("#cba6f7".into()),
                builtin: ThemeColor::Hex("#89b4fa".into()),
                function: ThemeColor::Hex("#cba6f7".into()),
                variable: ThemeColor::Hex("#fab387".into()),
                string: ThemeColor::Hex("#a6e3a1".into()),
                number: ThemeColor::Hex("#fab387".into()),
                operator: ThemeColor::Hex("#cdd6f4".into()),
                pipe: ThemeColor::Hex("#89dceb".into()),
                comment: ThemeColor::Hex("#6c7086".into()),
                escape: ThemeColor::Hex("#89b4fa".into()),
                alias: ThemeColor::Hex("#f5c2e7".into()),
                unknown_command: ThemeColor::Hex("#f38ba8".into()),
                normal_text: ThemeColor::Hex("#cdd6f4".into()),
                separator: ThemeColor::Hex("#cdd6f4".into()),
                punctuation: ThemeColor::Hex("#9399b2".into()),
                type_name: ThemeColor::Hex("#f9e2af".into()),
            },
            completions: CompletionsTheme {
                header_directory: ThemeColor::Hex("#a6e3a1".into()),
                header_file: ThemeColor::Hex("#a6e3a1".into()),
                header_command: ThemeColor::Hex("#89b4fa".into()),
                header_builtin: ThemeColor::Hex("#cba6f7".into()),
                header_alias: ThemeColor::Hex("#f5c2e7".into()),
                header_function: ThemeColor::Hex("#cba6f7".into()),
                header_variable: ThemeColor::Hex("#fab387".into()),
                header_flag: ThemeColor::Hex("#89dceb".into()),
                header_pipeline: ThemeColor::Hex("#89dceb".into()),
                header_keyword: ThemeColor::Hex("#cba6f7".into()),
                header_job: ThemeColor::Hex("#fab387".into()),
                header_history: ThemeColor::Hex("#6c7086".into()),
                header_ref: ThemeColor::Hex("#6c7086".into()),
                header_default: ThemeColor::Hex("#6c7086".into()),
                description: ThemeColor::Hex("#6c7086".into()),
                icon: ThemeColor::Hex("#6c7086".into()),
            },
            widgets: WidgetTheme {
                border: ThemeColor::Hex("#45475a".into()),
                border_focus: ThemeColor::Hex("#cba6f7".into()),
                title: ThemeColor::Hex("#cba6f7".into()),
                title_focus: ThemeColor::Hex("#cba6f7".into()),
                background: ThemeColor::Hex("#1e1e2e".into()),
                foreground: ThemeColor::Hex("#cdd6f4".into()),
                item_selected_bg: ThemeColor::Hex("#45475a".into()),
                item_selected_fg: ThemeColor::Hex("#cdd6f4".into()),
                border_style: BorderStyle::Rounded,
            },
            prompt: PromptTheme {
                cursor_color: ThemeColor::Hex("#cba6f7".into()),
                cursor_unfocused: ThemeColor::Hex("#6c7086".into()),
                cursor_style: CursorStyle::Block,
                input_fg: ThemeColor::Hex("#cdd6f4".into()),
                selection_bg: ThemeColor::Hex("#45475a".into()),
                selection_fg: ThemeColor::Hex("#cdd6f4".into()),
                user: ThemeColor::Hex("#cba6f7".into()),
                host: ThemeColor::Hex("#89b4fa".into()),
                pwd: ThemeColor::Hex("#a6e3a1".into()),
                git_branch: ThemeColor::Hex("#a6e3a1".into()),
                git_status_clean: ThemeColor::Hex("#a6e3a1".into()),
                git_status_dirty: ThemeColor::Hex("#f9e2af".into()),
                exit_ok: ThemeColor::Hex("#a6e3a1".into()),
                exit_error: ThemeColor::Hex("#f38ba8".into()),
                duration: ThemeColor::Hex("#6c7086".into()),
                job_count: ThemeColor::Hex("#89b4fa".into()),
                prompt_symbol: ThemeColor::Hex("#a6e3a1".into()),
                prompt_symbol_root: ThemeColor::Hex("#f38ba8".into()),
            },
            status: StatusTheme {
                ok: ThemeColor::Hex("#a6e3a1".into()),
                error: ThemeColor::Hex("#f38ba8".into()),
                warning: ThemeColor::Hex("#f9e2af".into()),
                info: ThemeColor::Hex("#89b4fa".into()),
                muted: ThemeColor::Hex("#6c7086".into()),
            },
            chrome: ChromeTheme {
                background: ThemeColor::Hex("#1e1e2e".into()),
                foreground: ThemeColor::Hex("#cdd6f4".into()),
                border_style: BorderStyle::Rounded,
            },
        }
    }

    /// Catppuccin Latte — light palette with soft pastel colors.
    pub fn catppuccin_latte() -> Self {
        Self {
            name: "catppuccin-latte".into(),
            inherits: None,
            syntax: SyntaxTheme {
                keyword: ThemeColor::Hex("#8839ef".into()),
                builtin: ThemeColor::Hex("#1e66f5".into()),
                function: ThemeColor::Hex("#8839ef".into()),
                variable: ThemeColor::Hex("#fe640b".into()),
                string: ThemeColor::Hex("#40a02b".into()),
                number: ThemeColor::Hex("#fe640b".into()),
                operator: ThemeColor::Hex("#4c4f69".into()),
                pipe: ThemeColor::Hex("#04a5e5".into()),
                comment: ThemeColor::Hex("#9ca0b0".into()),
                escape: ThemeColor::Hex("#1e66f5".into()),
                alias: ThemeColor::Hex("#ea76cb".into()),
                unknown_command: ThemeColor::Hex("#d20f39".into()),
                normal_text: ThemeColor::Hex("#4c4f69".into()),
                separator: ThemeColor::Hex("#4c4f69".into()),
                punctuation: ThemeColor::Hex("#7c7f93".into()),
                type_name: ThemeColor::Hex("#df8e1d".into()),
            },
            completions: CompletionsTheme {
                header_directory: ThemeColor::Hex("#40a02b".into()),
                header_file: ThemeColor::Hex("#40a02b".into()),
                header_command: ThemeColor::Hex("#1e66f5".into()),
                header_builtin: ThemeColor::Hex("#8839ef".into()),
                header_alias: ThemeColor::Hex("#ea76cb".into()),
                header_function: ThemeColor::Hex("#8839ef".into()),
                header_variable: ThemeColor::Hex("#fe640b".into()),
                header_flag: ThemeColor::Hex("#04a5e5".into()),
                header_pipeline: ThemeColor::Hex("#04a5e5".into()),
                header_keyword: ThemeColor::Hex("#8839ef".into()),
                header_job: ThemeColor::Hex("#fe640b".into()),
                header_history: ThemeColor::Hex("#9ca0b0".into()),
                header_ref: ThemeColor::Hex("#9ca0b0".into()),
                header_default: ThemeColor::Hex("#9ca0b0".into()),
                description: ThemeColor::Hex("#9ca0b0".into()),
                icon: ThemeColor::Hex("#9ca0b0".into()),
            },
            widgets: WidgetTheme {
                border: ThemeColor::Hex("#ccd0da".into()),
                border_focus: ThemeColor::Hex("#8839ef".into()),
                title: ThemeColor::Hex("#8839ef".into()),
                title_focus: ThemeColor::Hex("#8839ef".into()),
                background: ThemeColor::Hex("#eff1f5".into()),
                foreground: ThemeColor::Hex("#4c4f69".into()),
                item_selected_bg: ThemeColor::Hex("#ccd0da".into()),
                item_selected_fg: ThemeColor::Hex("#4c4f69".into()),
                border_style: BorderStyle::Rounded,
            },
            prompt: PromptTheme {
                cursor_color: ThemeColor::Hex("#8839ef".into()),
                cursor_unfocused: ThemeColor::Hex("#9ca0b0".into()),
                cursor_style: CursorStyle::Block,
                input_fg: ThemeColor::Hex("#4c4f69".into()),
                selection_bg: ThemeColor::Hex("#ccd0da".into()),
                selection_fg: ThemeColor::Hex("#4c4f69".into()),
                user: ThemeColor::Hex("#8839ef".into()),
                host: ThemeColor::Hex("#1e66f5".into()),
                pwd: ThemeColor::Hex("#40a02b".into()),
                git_branch: ThemeColor::Hex("#40a02b".into()),
                git_status_clean: ThemeColor::Hex("#40a02b".into()),
                git_status_dirty: ThemeColor::Hex("#df8e1d".into()),
                exit_ok: ThemeColor::Hex("#40a02b".into()),
                exit_error: ThemeColor::Hex("#d20f39".into()),
                duration: ThemeColor::Hex("#9ca0b0".into()),
                job_count: ThemeColor::Hex("#1e66f5".into()),
                prompt_symbol: ThemeColor::Hex("#40a02b".into()),
                prompt_symbol_root: ThemeColor::Hex("#d20f39".into()),
            },
            status: StatusTheme {
                ok: ThemeColor::Hex("#40a02b".into()),
                error: ThemeColor::Hex("#d20f39".into()),
                warning: ThemeColor::Hex("#df8e1d".into()),
                info: ThemeColor::Hex("#1e66f5".into()),
                muted: ThemeColor::Hex("#9ca0b0".into()),
            },
            chrome: ChromeTheme {
                background: ThemeColor::Hex("#eff1f5".into()),
                foreground: ThemeColor::Hex("#4c4f69".into()),
                border_style: BorderStyle::Rounded,
            },
        }
    }

    /// Dracula — vibrant neon on dark.
    pub fn dracula() -> Self {
        Self {
            name: "dracula".into(),
            inherits: None,
            syntax: SyntaxTheme {
                keyword: ThemeColor::Hex("#ff79c6".into()),
                builtin: ThemeColor::Hex("#bd93f9".into()),
                function: ThemeColor::Hex("#ff79c6".into()),
                variable: ThemeColor::Hex("#f8f8f2".into()),
                string: ThemeColor::Hex("#f1fa8c".into()),
                number: ThemeColor::Hex("#bd93f9".into()),
                operator: ThemeColor::Hex("#ff79c6".into()),
                pipe: ThemeColor::Hex("#8be9fd".into()),
                comment: ThemeColor::Hex("#6272a4".into()),
                escape: ThemeColor::Hex("#8be9fd".into()),
                alias: ThemeColor::Hex("#ff79c6".into()),
                unknown_command: ThemeColor::Hex("#ff5555".into()),
                normal_text: ThemeColor::Hex("#f8f8f2".into()),
                separator: ThemeColor::Hex("#f8f8f2".into()),
                punctuation: ThemeColor::Hex("#6272a4".into()),
                type_name: ThemeColor::Hex("#8be9fd".into()),
            },
            completions: CompletionsTheme {
                header_directory: ThemeColor::Hex("#50fa7b".into()),
                header_file: ThemeColor::Hex("#50fa7b".into()),
                header_command: ThemeColor::Hex("#bd93f9".into()),
                header_builtin: ThemeColor::Hex("#ff79c6".into()),
                header_alias: ThemeColor::Hex("#ff79c6".into()),
                header_function: ThemeColor::Hex("#bd93f9".into()),
                header_variable: ThemeColor::Hex("#f8f8f2".into()),
                header_flag: ThemeColor::Hex("#8be9fd".into()),
                header_pipeline: ThemeColor::Hex("#8be9fd".into()),
                header_keyword: ThemeColor::Hex("#ff79c6".into()),
                header_job: ThemeColor::Hex("#ffb86c".into()),
                header_history: ThemeColor::Hex("#6272a4".into()),
                header_ref: ThemeColor::Hex("#6272a4".into()),
                header_default: ThemeColor::Hex("#6272a4".into()),
                description: ThemeColor::Hex("#6272a4".into()),
                icon: ThemeColor::Hex("#6272a4".into()),
            },
            widgets: WidgetTheme {
                border: ThemeColor::Hex("#6272a4".into()),
                border_focus: ThemeColor::Hex("#bd93f9".into()),
                title: ThemeColor::Hex("#bd93f9".into()),
                title_focus: ThemeColor::Hex("#bd93f9".into()),
                background: ThemeColor::Hex("#282a36".into()),
                foreground: ThemeColor::Hex("#f8f8f2".into()),
                item_selected_bg: ThemeColor::Hex("#44475a".into()),
                item_selected_fg: ThemeColor::Hex("#f8f8f2".into()),
                border_style: BorderStyle::Rounded,
            },
            prompt: PromptTheme {
                cursor_color: ThemeColor::Hex("#f8f8f2".into()),
                cursor_unfocused: ThemeColor::Hex("#6272a4".into()),
                cursor_style: CursorStyle::Block,
                input_fg: ThemeColor::Hex("#f8f8f2".into()),
                selection_bg: ThemeColor::Hex("#44475a".into()),
                selection_fg: ThemeColor::Hex("#f8f8f2".into()),
                user: ThemeColor::Hex("#ff79c6".into()),
                host: ThemeColor::Hex("#bd93f9".into()),
                pwd: ThemeColor::Hex("#f1fa8c".into()),
                git_branch: ThemeColor::Hex("#50fa7b".into()),
                git_status_clean: ThemeColor::Hex("#50fa7b".into()),
                git_status_dirty: ThemeColor::Hex("#ffb86c".into()),
                exit_ok: ThemeColor::Hex("#50fa7b".into()),
                exit_error: ThemeColor::Hex("#ff5555".into()),
                duration: ThemeColor::Hex("#6272a4".into()),
                job_count: ThemeColor::Hex("#bd93f9".into()),
                prompt_symbol: ThemeColor::Hex("#50fa7b".into()),
                prompt_symbol_root: ThemeColor::Hex("#ff5555".into()),
            },
            status: StatusTheme {
                ok: ThemeColor::Hex("#50fa7b".into()),
                error: ThemeColor::Hex("#ff5555".into()),
                warning: ThemeColor::Hex("#f1fa8c".into()),
                info: ThemeColor::Hex("#8be9fd".into()),
                muted: ThemeColor::Hex("#6272a4".into()),
            },
            chrome: ChromeTheme {
                background: ThemeColor::Hex("#282a36".into()),
                foreground: ThemeColor::Hex("#f8f8f2".into()),
                border_style: BorderStyle::Rounded,
            },
        }
    }

    /// Gruvbox Dark — warm, earthy tones.
    pub fn gruvbox_dark() -> Self {
        Self {
            name: "gruvbox".into(),
            inherits: None,
            syntax: SyntaxTheme {
                keyword: ThemeColor::Hex("#fb4934".into()),
                builtin: ThemeColor::Hex("#83a598".into()),
                function: ThemeColor::Hex("#d3869b".into()),
                variable: ThemeColor::Hex("#ebdbb2".into()),
                string: ThemeColor::Hex("#b8bb26".into()),
                number: ThemeColor::Hex("#d3869b".into()),
                operator: ThemeColor::Hex("#fe8019".into()),
                pipe: ThemeColor::Hex("#8ec07c".into()),
                comment: ThemeColor::Hex("#928374".into()),
                escape: ThemeColor::Hex("#8ec07c".into()),
                alias: ThemeColor::Hex("#d3869b".into()),
                unknown_command: ThemeColor::Hex("#fb4934".into()),
                normal_text: ThemeColor::Hex("#ebdbb2".into()),
                separator: ThemeColor::Hex("#ebdbb2".into()),
                punctuation: ThemeColor::Hex("#a89984".into()),
                type_name: ThemeColor::Hex("#8ec07c".into()),
            },
            completions: CompletionsTheme {
                header_directory: ThemeColor::Hex("#b8bb26".into()),
                header_file: ThemeColor::Hex("#b8bb26".into()),
                header_command: ThemeColor::Hex("#83a598".into()),
                header_builtin: ThemeColor::Hex("#d3869b".into()),
                header_alias: ThemeColor::Hex("#d3869b".into()),
                header_function: ThemeColor::Hex("#d3869b".into()),
                header_variable: ThemeColor::Hex("#ebdbb2".into()),
                header_flag: ThemeColor::Hex("#8ec07c".into()),
                header_pipeline: ThemeColor::Hex("#8ec07c".into()),
                header_keyword: ThemeColor::Hex("#fb4934".into()),
                header_job: ThemeColor::Hex("#fe8019".into()),
                header_history: ThemeColor::Hex("#928374".into()),
                header_ref: ThemeColor::Hex("#928374".into()),
                header_default: ThemeColor::Hex("#928374".into()),
                description: ThemeColor::Hex("#928374".into()),
                icon: ThemeColor::Hex("#928374".into()),
            },
            widgets: WidgetTheme {
                border: ThemeColor::Hex("#504945".into()),
                border_focus: ThemeColor::Hex("#d3869b".into()),
                title: ThemeColor::Hex("#d3869b".into()),
                title_focus: ThemeColor::Hex("#d3869b".into()),
                background: ThemeColor::Hex("#282828".into()),
                foreground: ThemeColor::Hex("#ebdbb2".into()),
                item_selected_bg: ThemeColor::Hex("#504945".into()),
                item_selected_fg: ThemeColor::Hex("#ebdbb2".into()),
                border_style: BorderStyle::Rounded,
            },
            prompt: PromptTheme {
                cursor_color: ThemeColor::Hex("#ebdbb2".into()),
                cursor_unfocused: ThemeColor::Hex("#928374".into()),
                cursor_style: CursorStyle::Block,
                input_fg: ThemeColor::Hex("#ebdbb2".into()),
                selection_bg: ThemeColor::Hex("#504945".into()),
                selection_fg: ThemeColor::Hex("#ebdbb2".into()),
                user: ThemeColor::Hex("#d3869b".into()),
                host: ThemeColor::Hex("#83a598".into()),
                pwd: ThemeColor::Hex("#b8bb26".into()),
                git_branch: ThemeColor::Hex("#b8bb26".into()),
                git_status_clean: ThemeColor::Hex("#b8bb26".into()),
                git_status_dirty: ThemeColor::Hex("#fabd2f".into()),
                exit_ok: ThemeColor::Hex("#b8bb26".into()),
                exit_error: ThemeColor::Hex("#fb4934".into()),
                duration: ThemeColor::Hex("#928374".into()),
                job_count: ThemeColor::Hex("#83a598".into()),
                prompt_symbol: ThemeColor::Hex("#b8bb26".into()),
                prompt_symbol_root: ThemeColor::Hex("#fb4934".into()),
            },
            status: StatusTheme {
                ok: ThemeColor::Hex("#b8bb26".into()),
                error: ThemeColor::Hex("#fb4934".into()),
                warning: ThemeColor::Hex("#fabd2f".into()),
                info: ThemeColor::Hex("#83a598".into()),
                muted: ThemeColor::Hex("#928374".into()),
            },
            chrome: ChromeTheme {
                background: ThemeColor::Hex("#282828".into()),
                foreground: ThemeColor::Hex("#ebdbb2".into()),
                border_style: BorderStyle::Rounded,
            },
        }
    }

    /// Gruvbox Light — warm, retro light palette.
    pub fn gruvbox_light() -> Self {
        Self {
            name: "gruvbox-light".into(),
            inherits: None,
            syntax: SyntaxTheme {
                keyword: ThemeColor::Hex("#9d0006".into()),
                builtin: ThemeColor::Hex("#076678".into()),
                function: ThemeColor::Hex("#8f3f71".into()),
                variable: ThemeColor::Hex("#3c3836".into()),
                string: ThemeColor::Hex("#79740e".into()),
                number: ThemeColor::Hex("#8f3f71".into()),
                operator: ThemeColor::Hex("#af3a03".into()),
                pipe: ThemeColor::Hex("#427b58".into()),
                comment: ThemeColor::Hex("#928374".into()),
                escape: ThemeColor::Hex("#427b58".into()),
                alias: ThemeColor::Hex("#8f3f71".into()),
                unknown_command: ThemeColor::Hex("#9d0006".into()),
                normal_text: ThemeColor::Hex("#3c3836".into()),
                separator: ThemeColor::Hex("#3c3836".into()),
                punctuation: ThemeColor::Hex("#7c6f64".into()),
                type_name: ThemeColor::Hex("#b57614".into()),
            },
            completions: CompletionsTheme {
                header_directory: ThemeColor::Hex("#79740e".into()),
                header_file: ThemeColor::Hex("#79740e".into()),
                header_command: ThemeColor::Hex("#076678".into()),
                header_builtin: ThemeColor::Hex("#8f3f71".into()),
                header_alias: ThemeColor::Hex("#8f3f71".into()),
                header_function: ThemeColor::Hex("#8f3f71".into()),
                header_variable: ThemeColor::Hex("#3c3836".into()),
                header_flag: ThemeColor::Hex("#427b58".into()),
                header_pipeline: ThemeColor::Hex("#427b58".into()),
                header_keyword: ThemeColor::Hex("#9d0006".into()),
                header_job: ThemeColor::Hex("#af3a03".into()),
                header_history: ThemeColor::Hex("#928374".into()),
                header_ref: ThemeColor::Hex("#928374".into()),
                header_default: ThemeColor::Hex("#928374".into()),
                description: ThemeColor::Hex("#928374".into()),
                icon: ThemeColor::Hex("#928374".into()),
            },
            widgets: WidgetTheme {
                border: ThemeColor::Hex("#d5c4a1".into()),
                border_focus: ThemeColor::Hex("#8f3f71".into()),
                title: ThemeColor::Hex("#8f3f71".into()),
                title_focus: ThemeColor::Hex("#8f3f71".into()),
                background: ThemeColor::Hex("#fbf1c7".into()),
                foreground: ThemeColor::Hex("#3c3836".into()),
                item_selected_bg: ThemeColor::Hex("#ebdbb2".into()),
                item_selected_fg: ThemeColor::Hex("#3c3836".into()),
                border_style: BorderStyle::Rounded,
            },
            prompt: PromptTheme {
                cursor_color: ThemeColor::Hex("#3c3836".into()),
                cursor_unfocused: ThemeColor::Hex("#928374".into()),
                cursor_style: CursorStyle::Block,
                input_fg: ThemeColor::Hex("#3c3836".into()),
                selection_bg: ThemeColor::Hex("#ebdbb2".into()),
                selection_fg: ThemeColor::Hex("#3c3836".into()),
                user: ThemeColor::Hex("#8f3f71".into()),
                host: ThemeColor::Hex("#076678".into()),
                pwd: ThemeColor::Hex("#79740e".into()),
                git_branch: ThemeColor::Hex("#79740e".into()),
                git_status_clean: ThemeColor::Hex("#79740e".into()),
                git_status_dirty: ThemeColor::Hex("#b57614".into()),
                exit_ok: ThemeColor::Hex("#79740e".into()),
                exit_error: ThemeColor::Hex("#9d0006".into()),
                duration: ThemeColor::Hex("#928374".into()),
                job_count: ThemeColor::Hex("#076678".into()),
                prompt_symbol: ThemeColor::Hex("#79740e".into()),
                prompt_symbol_root: ThemeColor::Hex("#9d0006".into()),
            },
            status: StatusTheme {
                ok: ThemeColor::Hex("#79740e".into()),
                error: ThemeColor::Hex("#9d0006".into()),
                warning: ThemeColor::Hex("#b57614".into()),
                info: ThemeColor::Hex("#076678".into()),
                muted: ThemeColor::Hex("#928374".into()),
            },
            chrome: ChromeTheme {
                background: ThemeColor::Hex("#fbf1c7".into()),
                foreground: ThemeColor::Hex("#3c3836".into()),
                border_style: BorderStyle::Rounded,
            },
        }
    }

    /// Nord — arctic, north-bluish clean dark palette.
    pub fn nord() -> Self {
        Self {
            name: "nord".into(),
            inherits: None,
            syntax: SyntaxTheme {
                keyword: ThemeColor::Hex("#81a1c1".into()),
                builtin: ThemeColor::Hex("#88c0d0".into()),
                function: ThemeColor::Hex("#88c0d0".into()),
                variable: ThemeColor::Hex("#d8dee9".into()),
                string: ThemeColor::Hex("#a3be8c".into()),
                number: ThemeColor::Hex("#b48ead".into()),
                operator: ThemeColor::Hex("#81a1c1".into()),
                pipe: ThemeColor::Hex("#88c0d0".into()),
                comment: ThemeColor::Hex("#616e88".into()),
                escape: ThemeColor::Hex("#ebcb8b".into()),
                alias: ThemeColor::Hex("#b48ead".into()),
                unknown_command: ThemeColor::Hex("#bf616a".into()),
                normal_text: ThemeColor::Hex("#eceff4".into()),
                separator: ThemeColor::Hex("#eceff4".into()),
                punctuation: ThemeColor::Hex("#4c566a".into()),
                type_name: ThemeColor::Hex("#8fbcbb".into()),
            },
            completions: CompletionsTheme {
                header_directory: ThemeColor::Hex("#a3be8c".into()),
                header_file: ThemeColor::Hex("#a3be8c".into()),
                header_command: ThemeColor::Hex("#88c0d0".into()),
                header_builtin: ThemeColor::Hex("#81a1c1".into()),
                header_alias: ThemeColor::Hex("#b48ead".into()),
                header_function: ThemeColor::Hex("#88c0d0".into()),
                header_variable: ThemeColor::Hex("#d8dee9".into()),
                header_flag: ThemeColor::Hex("#8fbcbb".into()),
                header_pipeline: ThemeColor::Hex("#88c0d0".into()),
                header_keyword: ThemeColor::Hex("#81a1c1".into()),
                header_job: ThemeColor::Hex("#d08770".into()),
                header_history: ThemeColor::Hex("#616e88".into()),
                header_ref: ThemeColor::Hex("#616e88".into()),
                header_default: ThemeColor::Hex("#616e88".into()),
                description: ThemeColor::Hex("#616e88".into()),
                icon: ThemeColor::Hex("#616e88".into()),
            },
            widgets: WidgetTheme {
                border: ThemeColor::Hex("#434c5e".into()),
                border_focus: ThemeColor::Hex("#88c0d0".into()),
                title: ThemeColor::Hex("#88c0d0".into()),
                title_focus: ThemeColor::Hex("#88c0d0".into()),
                background: ThemeColor::Hex("#2e3440".into()),
                foreground: ThemeColor::Hex("#eceff4".into()),
                item_selected_bg: ThemeColor::Hex("#3b4252".into()),
                item_selected_fg: ThemeColor::Hex("#88c0d0".into()),
                border_style: BorderStyle::Rounded,
            },
            prompt: PromptTheme {
                cursor_color: ThemeColor::Hex("#88c0d0".into()),
                cursor_unfocused: ThemeColor::Hex("#4c566a".into()),
                cursor_style: CursorStyle::Block,
                input_fg: ThemeColor::Hex("#eceff4".into()),
                selection_bg: ThemeColor::Hex("#3b4252".into()),
                selection_fg: ThemeColor::Hex("#eceff4".into()),
                user: ThemeColor::Hex("#81a1c1".into()),
                host: ThemeColor::Hex("#88c0d0".into()),
                pwd: ThemeColor::Hex("#a3be8c".into()),
                git_branch: ThemeColor::Hex("#8fbcbb".into()),
                git_status_clean: ThemeColor::Hex("#a3be8c".into()),
                git_status_dirty: ThemeColor::Hex("#ebcb8b".into()),
                exit_ok: ThemeColor::Hex("#a3be8c".into()),
                exit_error: ThemeColor::Hex("#bf616a".into()),
                duration: ThemeColor::Hex("#616e88".into()),
                job_count: ThemeColor::Hex("#81a1c1".into()),
                prompt_symbol: ThemeColor::Hex("#88c0d0".into()),
                prompt_symbol_root: ThemeColor::Hex("#bf616a".into()),
            },
            status: StatusTheme {
                ok: ThemeColor::Hex("#a3be8c".into()),
                error: ThemeColor::Hex("#bf616a".into()),
                warning: ThemeColor::Hex("#ebcb8b".into()),
                info: ThemeColor::Hex("#88c0d0".into()),
                muted: ThemeColor::Hex("#616e88".into()),
            },
            chrome: ChromeTheme {
                background: ThemeColor::Hex("#2e3440".into()),
                foreground: ThemeColor::Hex("#eceff4".into()),
                border_style: BorderStyle::Rounded,
            },
        }
    }

    /// Tokyo Night — rich dark theme inspired by Tokyo nightlife.
    pub fn tokyo_night() -> Self {
        Self {
            name: "tokyo-night".into(),
            inherits: None,
            syntax: SyntaxTheme {
                keyword: ThemeColor::Hex("#bb9af7".into()),
                builtin: ThemeColor::Hex("#7aa2f7".into()),
                function: ThemeColor::Hex("#7aa2f7".into()),
                variable: ThemeColor::Hex("#c0caf5".into()),
                string: ThemeColor::Hex("#9ece6a".into()),
                number: ThemeColor::Hex("#ff9e64".into()),
                operator: ThemeColor::Hex("#89ddff".into()),
                pipe: ThemeColor::Hex("#7dcfff".into()),
                comment: ThemeColor::Hex("#565f89".into()),
                escape: ThemeColor::Hex("#7dcfff".into()),
                alias: ThemeColor::Hex("#bb9af7".into()),
                unknown_command: ThemeColor::Hex("#f7768e".into()),
                normal_text: ThemeColor::Hex("#c0caf5".into()),
                separator: ThemeColor::Hex("#c0caf5".into()),
                punctuation: ThemeColor::Hex("#565f89".into()),
                type_name: ThemeColor::Hex("#2ac3de".into()),
            },
            completions: CompletionsTheme {
                header_directory: ThemeColor::Hex("#9ece6a".into()),
                header_file: ThemeColor::Hex("#9ece6a".into()),
                header_command: ThemeColor::Hex("#7aa2f7".into()),
                header_builtin: ThemeColor::Hex("#bb9af7".into()),
                header_alias: ThemeColor::Hex("#bb9af7".into()),
                header_function: ThemeColor::Hex("#7aa2f7".into()),
                header_variable: ThemeColor::Hex("#c0caf5".into()),
                header_flag: ThemeColor::Hex("#7dcfff".into()),
                header_pipeline: ThemeColor::Hex("#7dcfff".into()),
                header_keyword: ThemeColor::Hex("#bb9af7".into()),
                header_job: ThemeColor::Hex("#ff9e64".into()),
                header_history: ThemeColor::Hex("#565f89".into()),
                header_ref: ThemeColor::Hex("#565f89".into()),
                header_default: ThemeColor::Hex("#565f89".into()),
                description: ThemeColor::Hex("#565f89".into()),
                icon: ThemeColor::Hex("#565f89".into()),
            },
            widgets: WidgetTheme {
                border: ThemeColor::Hex("#29a4bd".into()),
                border_focus: ThemeColor::Hex("#7aa2f7".into()),
                title: ThemeColor::Hex("#7aa2f7".into()),
                title_focus: ThemeColor::Hex("#7aa2f7".into()),
                background: ThemeColor::Hex("#1a1b26".into()),
                foreground: ThemeColor::Hex("#c0caf5".into()),
                item_selected_bg: ThemeColor::Hex("#283457".into()),
                item_selected_fg: ThemeColor::Hex("#c0caf5".into()),
                border_style: BorderStyle::Rounded,
            },
            prompt: PromptTheme {
                cursor_color: ThemeColor::Hex("#c0caf5".into()),
                cursor_unfocused: ThemeColor::Hex("#565f89".into()),
                cursor_style: CursorStyle::Block,
                input_fg: ThemeColor::Hex("#c0caf5".into()),
                selection_bg: ThemeColor::Hex("#283457".into()),
                selection_fg: ThemeColor::Hex("#c0caf5".into()),
                user: ThemeColor::Hex("#bb9af7".into()),
                host: ThemeColor::Hex("#7aa2f7".into()),
                pwd: ThemeColor::Hex("#9ece6a".into()),
                git_branch: ThemeColor::Hex("#7dcfff".into()),
                git_status_clean: ThemeColor::Hex("#9ece6a".into()),
                git_status_dirty: ThemeColor::Hex("#e0af68".into()),
                exit_ok: ThemeColor::Hex("#9ece6a".into()),
                exit_error: ThemeColor::Hex("#f7768e".into()),
                duration: ThemeColor::Hex("#565f89".into()),
                job_count: ThemeColor::Hex("#7aa2f7".into()),
                prompt_symbol: ThemeColor::Hex("#7aa2f7".into()),
                prompt_symbol_root: ThemeColor::Hex("#f7768e".into()),
            },
            status: StatusTheme {
                ok: ThemeColor::Hex("#9ece6a".into()),
                error: ThemeColor::Hex("#f7768e".into()),
                warning: ThemeColor::Hex("#e0af68".into()),
                info: ThemeColor::Hex("#7aa2f7".into()),
                muted: ThemeColor::Hex("#565f89".into()),
            },
            chrome: ChromeTheme {
                background: ThemeColor::Hex("#1a1b26".into()),
                foreground: ThemeColor::Hex("#c0caf5".into()),
                border_style: BorderStyle::Rounded,
            },
        }
    }

    /// Solarized Dark — classic palette with precision colors on deep blue-green.
    pub fn solarized_dark() -> Self {
        Self {
            name: "solarized-dark".into(),
            inherits: None,
            syntax: SyntaxTheme {
                keyword: ThemeColor::Hex("#859900".into()),
                builtin: ThemeColor::Hex("#268bd2".into()),
                function: ThemeColor::Hex("#268bd2".into()),
                variable: ThemeColor::Hex("#b58900".into()),
                string: ThemeColor::Hex("#2aa198".into()),
                number: ThemeColor::Hex("#d33682".into()),
                operator: ThemeColor::Hex("#839496".into()),
                pipe: ThemeColor::Hex("#2aa198".into()),
                comment: ThemeColor::Hex("#586e75".into()),
                escape: ThemeColor::Hex("#cb4b16".into()),
                alias: ThemeColor::Hex("#6c71c4".into()),
                unknown_command: ThemeColor::Hex("#dc322f".into()),
                normal_text: ThemeColor::Hex("#839496".into()),
                separator: ThemeColor::Hex("#839496".into()),
                punctuation: ThemeColor::Hex("#657b83".into()),
                type_name: ThemeColor::Hex("#b58900".into()),
            },
            completions: CompletionsTheme {
                header_directory: ThemeColor::Hex("#859900".into()),
                header_file: ThemeColor::Hex("#859900".into()),
                header_command: ThemeColor::Hex("#268bd2".into()),
                header_builtin: ThemeColor::Hex("#6c71c4".into()),
                header_alias: ThemeColor::Hex("#6c71c4".into()),
                header_function: ThemeColor::Hex("#268bd2".into()),
                header_variable: ThemeColor::Hex("#b58900".into()),
                header_flag: ThemeColor::Hex("#2aa198".into()),
                header_pipeline: ThemeColor::Hex("#2aa198".into()),
                header_keyword: ThemeColor::Hex("#859900".into()),
                header_job: ThemeColor::Hex("#cb4b16".into()),
                header_history: ThemeColor::Hex("#586e75".into()),
                header_ref: ThemeColor::Hex("#586e75".into()),
                header_default: ThemeColor::Hex("#586e75".into()),
                description: ThemeColor::Hex("#586e75".into()),
                icon: ThemeColor::Hex("#586e75".into()),
            },
            widgets: WidgetTheme {
                border: ThemeColor::Hex("#073642".into()),
                border_focus: ThemeColor::Hex("#268bd2".into()),
                title: ThemeColor::Hex("#268bd2".into()),
                title_focus: ThemeColor::Hex("#268bd2".into()),
                background: ThemeColor::Hex("#002b36".into()),
                foreground: ThemeColor::Hex("#839496".into()),
                item_selected_bg: ThemeColor::Hex("#073642".into()),
                item_selected_fg: ThemeColor::Hex("#93a1a1".into()),
                border_style: BorderStyle::Rounded,
            },
            prompt: PromptTheme {
                cursor_color: ThemeColor::Hex("#839496".into()),
                cursor_unfocused: ThemeColor::Hex("#586e75".into()),
                cursor_style: CursorStyle::Block,
                input_fg: ThemeColor::Hex("#839496".into()),
                selection_bg: ThemeColor::Hex("#073642".into()),
                selection_fg: ThemeColor::Hex("#93a1a1".into()),
                user: ThemeColor::Hex("#6c71c4".into()),
                host: ThemeColor::Hex("#268bd2".into()),
                pwd: ThemeColor::Hex("#2aa198".into()),
                git_branch: ThemeColor::Hex("#859900".into()),
                git_status_clean: ThemeColor::Hex("#859900".into()),
                git_status_dirty: ThemeColor::Hex("#b58900".into()),
                exit_ok: ThemeColor::Hex("#859900".into()),
                exit_error: ThemeColor::Hex("#dc322f".into()),
                duration: ThemeColor::Hex("#586e75".into()),
                job_count: ThemeColor::Hex("#268bd2".into()),
                prompt_symbol: ThemeColor::Hex("#859900".into()),
                prompt_symbol_root: ThemeColor::Hex("#dc322f".into()),
            },
            status: StatusTheme {
                ok: ThemeColor::Hex("#859900".into()),
                error: ThemeColor::Hex("#dc322f".into()),
                warning: ThemeColor::Hex("#b58900".into()),
                info: ThemeColor::Hex("#268bd2".into()),
                muted: ThemeColor::Hex("#586e75".into()),
            },
            chrome: ChromeTheme {
                background: ThemeColor::Hex("#002b36".into()),
                foreground: ThemeColor::Hex("#839496".into()),
                border_style: BorderStyle::Rounded,
            },
        }
    }

    /// Solarized Light — sunny, warm light palette.
    pub fn solarized_light() -> Self {
        Self {
            name: "solarized-light".into(),
            inherits: None,
            syntax: SyntaxTheme {
                keyword: ThemeColor::Hex("#859900".into()),
                builtin: ThemeColor::Hex("#268bd2".into()),
                function: ThemeColor::Hex("#268bd2".into()),
                variable: ThemeColor::Hex("#b58900".into()),
                string: ThemeColor::Hex("#2aa198".into()),
                number: ThemeColor::Hex("#d33682".into()),
                operator: ThemeColor::Hex("#657b83".into()),
                pipe: ThemeColor::Hex("#2aa198".into()),
                comment: ThemeColor::Hex("#93a1a1".into()),
                escape: ThemeColor::Hex("#cb4b16".into()),
                alias: ThemeColor::Hex("#6c71c4".into()),
                unknown_command: ThemeColor::Hex("#dc322f".into()),
                normal_text: ThemeColor::Hex("#657b83".into()),
                separator: ThemeColor::Hex("#657b83".into()),
                punctuation: ThemeColor::Hex("#93a1a1".into()),
                type_name: ThemeColor::Hex("#b58900".into()),
            },
            completions: CompletionsTheme {
                header_directory: ThemeColor::Hex("#859900".into()),
                header_file: ThemeColor::Hex("#859900".into()),
                header_command: ThemeColor::Hex("#268bd2".into()),
                header_builtin: ThemeColor::Hex("#6c71c4".into()),
                header_alias: ThemeColor::Hex("#6c71c4".into()),
                header_function: ThemeColor::Hex("#268bd2".into()),
                header_variable: ThemeColor::Hex("#b58900".into()),
                header_flag: ThemeColor::Hex("#2aa198".into()),
                header_pipeline: ThemeColor::Hex("#2aa198".into()),
                header_keyword: ThemeColor::Hex("#859900".into()),
                header_job: ThemeColor::Hex("#cb4b16".into()),
                header_history: ThemeColor::Hex("#93a1a1".into()),
                header_ref: ThemeColor::Hex("#93a1a1".into()),
                header_default: ThemeColor::Hex("#93a1a1".into()),
                description: ThemeColor::Hex("#93a1a1".into()),
                icon: ThemeColor::Hex("#93a1a1".into()),
            },
            widgets: WidgetTheme {
                border: ThemeColor::Hex("#eee8d5".into()),
                border_focus: ThemeColor::Hex("#268bd2".into()),
                title: ThemeColor::Hex("#268bd2".into()),
                title_focus: ThemeColor::Hex("#268bd2".into()),
                background: ThemeColor::Hex("#fdf6e3".into()),
                foreground: ThemeColor::Hex("#657b83".into()),
                item_selected_bg: ThemeColor::Hex("#eee8d5".into()),
                item_selected_fg: ThemeColor::Hex("#586e75".into()),
                border_style: BorderStyle::Rounded,
            },
            prompt: PromptTheme {
                cursor_color: ThemeColor::Hex("#657b83".into()),
                cursor_unfocused: ThemeColor::Hex("#93a1a1".into()),
                cursor_style: CursorStyle::Block,
                input_fg: ThemeColor::Hex("#657b83".into()),
                selection_bg: ThemeColor::Hex("#eee8d5".into()),
                selection_fg: ThemeColor::Hex("#586e75".into()),
                user: ThemeColor::Hex("#6c71c4".into()),
                host: ThemeColor::Hex("#268bd2".into()),
                pwd: ThemeColor::Hex("#2aa198".into()),
                git_branch: ThemeColor::Hex("#859900".into()),
                git_status_clean: ThemeColor::Hex("#859900".into()),
                git_status_dirty: ThemeColor::Hex("#b58900".into()),
                exit_ok: ThemeColor::Hex("#859900".into()),
                exit_error: ThemeColor::Hex("#dc322f".into()),
                duration: ThemeColor::Hex("#93a1a1".into()),
                job_count: ThemeColor::Hex("#268bd2".into()),
                prompt_symbol: ThemeColor::Hex("#859900".into()),
                prompt_symbol_root: ThemeColor::Hex("#dc322f".into()),
            },
            status: StatusTheme {
                ok: ThemeColor::Hex("#859900".into()),
                error: ThemeColor::Hex("#dc322f".into()),
                warning: ThemeColor::Hex("#b58900".into()),
                info: ThemeColor::Hex("#268bd2".into()),
                muted: ThemeColor::Hex("#93a1a1".into()),
            },
            chrome: ChromeTheme {
                background: ThemeColor::Hex("#fdf6e3".into()),
                foreground: ThemeColor::Hex("#657b83".into()),
                border_style: BorderStyle::Rounded,
            },
        }
    }
}

// Tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_color_hex_to_rgb() {
        let c = ThemeColor::Hex("#ff0000".into());
        assert_eq!(c.to_rgb(), (255, 0, 0));

        let c = ThemeColor::Hex("#cba6f7".into());
        assert_eq!(c.to_rgb(), (203, 166, 247));
    }

    #[test]
    fn test_theme_color_name_to_rgb() {
        let c = ThemeColor::Named("red".into());
        assert_eq!(c.to_rgb(), (205, 49, 49));

        let c = ThemeColor::Named("mauve".into());
        assert_eq!(c.to_rgb(), (203, 166, 247));

        let c = ThemeColor::Named("aqua".into());
        assert_eq!(c.to_rgb(), (142, 192, 124));

        let c = ThemeColor::Named("nord8".into());
        assert_eq!(c.to_rgb(), (136, 192, 208));

        let c = ThemeColor::Named("solarized_base03".into());
        assert_eq!(c.to_rgb(), (0, 43, 54));

        let c = ThemeColor::Named("tokyo_blue".into());
        assert_eq!(c.to_rgb(), (122, 162, 247));
    }

    #[test]
    fn test_theme_color_case_insensitive() {
        let c = ThemeColor::Named("Red".into());
        assert_eq!(c.to_rgb(), (205, 49, 49));

        let c = ThemeColor::Named("MAUVE".into());
        assert_eq!(c.to_rgb(), (203, 166, 247));
    }

    #[test]
    fn test_theme_color_hex_without_hash() {
        let c = ThemeColor::Named("ff0000".into());
        assert_eq!(c.to_rgb(), (255, 0, 0));
    }

    #[test]
    fn test_theme_color_unknown_falls_back_to_white() {
        let c = ThemeColor::Named("notacolor".into());
        assert_eq!(c.to_rgb(), (255, 255, 255));
    }

    #[test]
    fn test_all_presets_load() {
        let presets = [
            "default",
            "github-dark",
            "github-light",
            "catppuccin",
            "catppuccin-mocha",
            "catppuccin-latte",
            "dracula",
            "gruvbox",
            "gruvbox-dark",
            "gruvbox-light",
            "nord",
            "tokyo-night",
            "solarized-dark",
            "solarized-light",
        ];
        let tmp = tempfile::tempdir().unwrap();
        for p in presets {
            let loaded = Theme::load(p, tmp.path()).unwrap();
            assert!(!loaded.name.is_empty());
        }
    }

    #[test]
    fn test_theme_is_dark() {
        assert!(Theme::default_theme().is_dark());
        assert!(Theme::catppuccin_mocha().is_dark());
        assert!(Theme::dracula().is_dark());
        assert!(Theme::gruvbox_dark().is_dark());
        assert!(Theme::nord().is_dark());
        assert!(Theme::tokyo_night().is_dark());
        assert!(Theme::solarized_dark().is_dark());

        assert!(!Theme::github_light().is_dark());
        assert!(!Theme::catppuccin_latte().is_dark());
        assert!(!Theme::gruvbox_light().is_dark());
        assert!(!Theme::solarized_light().is_dark());
    }

    #[test]
    fn test_theme_resolve_color() {
        let theme = Theme::default_theme();
        // Semantic roles
        assert_eq!(
            theme.resolve_color("keyword"),
            theme.syntax.keyword.to_rgb()
        );
        assert_eq!(theme.resolve_color("ok"), theme.status.ok.to_rgb());
        assert_eq!(theme.resolve_color("error"), theme.status.error.to_rgb());
        assert_eq!(theme.resolve_color("user"), theme.prompt.user.to_rgb());
        assert_eq!(theme.resolve_color("pwd"), theme.prompt.pwd.to_rgb());

        // Hex
        assert_eq!(theme.resolve_color("#123456"), (0x12, 0x34, 0x56));

        // Named
        assert_eq!(theme.resolve_color("red"), (205, 49, 49));
    }

    #[test]
    fn test_merge_syntax_theme() {
        let mut base = Theme::default_theme();
        let overlay = SyntaxThemeToml {
            keyword: Some(ThemeColor::Hex("#ff0000".into())),
            ..Default::default()
        };
        base.syntax.merge(overlay);
        assert_eq!(base.syntax.keyword, ThemeColor::Hex("#ff0000".into()));
        // Other fields unchanged
        assert_eq!(base.syntax.builtin, ThemeColor::Hex("#79c0ff".into()));
    }

    #[test]
    fn test_merge_widget_theme() {
        let mut base = Theme::default_theme();
        let overlay = WidgetThemeToml {
            border_focus: Some(ThemeColor::Hex("#ff00ff".into())),
            ..Default::default()
        };
        base.widgets.merge(overlay);
        assert_eq!(base.widgets.border_focus, ThemeColor::Hex("#ff00ff".into()));
        assert_eq!(base.widgets.border, ThemeColor::Hex("#30363d".into()));
    }

    #[test]
    fn test_merge_status_theme() {
        let mut base = Theme::default_theme();
        let overlay = StatusThemeToml {
            error: Some(ThemeColor::Hex("#ff0000".into())),
            ..Default::default()
        };
        base.status.merge(overlay);
        assert_eq!(base.status.error, ThemeColor::Hex("#ff0000".into()));
        assert_eq!(base.status.ok, ThemeColor::Hex("#7ee787".into()));
    }

    #[test]
    fn test_merge_prompt_theme() {
        let mut base = Theme::default_theme();
        let overlay = PromptThemeToml {
            cursor_color: Some(ThemeColor::Hex("#ff0000".into())),
            cursor_style: Some(CursorStyle::Bar),
            ..Default::default()
        };
        base.prompt.merge(overlay);
        assert_eq!(base.prompt.cursor_color, ThemeColor::Hex("#ff0000".into()));
        assert_eq!(base.prompt.cursor_style, CursorStyle::Bar);
        assert_eq!(base.prompt.input_fg, ThemeColor::Hex("#e1e4e8".into()));
    }

    #[test]
    fn test_merge_chrome_theme() {
        let mut base = Theme::default_theme();
        let overlay = ChromeThemeToml {
            background: Some(ThemeColor::Hex("#ffffff".into())),
            ..Default::default()
        };
        base.chrome.merge(overlay);
        assert_eq!(base.chrome.background, ThemeColor::Hex("#ffffff".into()));
        assert_eq!(base.chrome.foreground, ThemeColor::Hex("#e1e4e8".into()));
    }

    #[test]
    fn test_toml_deserialization_partial() {
        let toml_str = r##"
inherits = "catppuccin"

[syntax]
keyword = "#ff0000"

[widgets]
border_focus = "#ff79c6"
"##;
        let theme_toml: ThemeToml = toml_edit::de::from_str(toml_str).unwrap();
        assert_eq!(theme_toml.inherits.as_deref(), Some("catppuccin"));
        assert!(theme_toml.syntax.is_some());
        assert!(theme_toml.completions.is_none());
    }

    #[test]
    fn test_toml_deserialization_complete() {
        let toml_str = r##"
name = "custom"
inherits = "dracula"

[syntax]
keyword = "#ff0000"
builtin = "#00ff00"
function = "#0000ff"
variable = "#ffff00"
string = "#00ffff"
number = "#ff00ff"
operator = "#ffffff"
pipe = "#888888"
comment = "#444444"
escape = "#aaaaaa"
alias = "#bbbbbb"
unknown_command = "#cccccc"
normal_text = "#dddddd"
separator = "#eeeeee"
punctuation = "#111111"
type_name = "#222222"

[completions]
header_directory = "#333333"
header_file = "#444444"
header_command = "#555555"
header_builtin = "#666666"
header_alias = "#777777"
header_function = "#888888"
header_variable = "#999999"
header_flag = "#aaaaaa"
header_pipeline = "#bbbbbb"
header_keyword = "#cccccc"
header_job = "#dddddd"
header_history = "#eeeeee"
header_ref = "#111111"
header_default = "#222222"
match_highlight = "#333333"
description = "#444444"
icon = "#555555"

[widgets]
border = "#666666"
border_focus = "#777777"
border_help = "#888888"
border_error = "#999999"
title = "#aaaaaa"
title_focus = "#bbbbbb"
background = "#cccccc"
foreground = "#dddddd"
hint_fg = "#eeeeee"
continuation_fg = "#111111"
footer_fg = "#222222"
item_selected_bg = "#333333"
item_selected_fg = "#444444"
border_style = "Double"

[status]
ok = "#555555"
error = "#666666"
warning = "#777777"
info = "#888888"
muted = "#999999"

[prompt]
cursor_color = "#aaaaaa"
cursor_unfocused = "#bbbbbb"
cursor_style = "Bar"
input_fg = "#cccccc"
selection_bg = "#dddddd"
selection_fg = "#eeeeee"
syntax_error_underline = "#111111"

[chrome]
background = "#222222"
foreground = "#333333"
border_style = "Thick"
prompt_separator_fg = "#444444"
"##;
        let theme_toml: ThemeToml = toml_edit::de::from_str(toml_str).unwrap();
        assert_eq!(theme_toml.name.as_deref(), Some("custom"));
        assert_eq!(theme_toml.inherits.as_deref(), Some("dracula"));
        assert!(theme_toml.syntax.is_some());
        assert!(theme_toml.completions.is_some());
        assert!(theme_toml.widgets.is_some());
        assert!(theme_toml.status.is_some());
        assert!(theme_toml.prompt.is_some());
        assert!(theme_toml.chrome.is_some());
    }

    #[test]
    fn test_border_style_default() {
        assert_eq!(BorderStyle::default(), BorderStyle::Rounded);
    }

    #[test]
    fn test_cursor_style_default() {
        assert_eq!(CursorStyle::default(), CursorStyle::Block);
    }

    #[test]
    fn test_theme_serialize_roundtrip() {
        let theme = Theme::default_theme();
        let json = serde_json::to_string(&theme).unwrap();
        let deserialized: Theme = serde_json::from_str(&json).unwrap();
        assert_eq!(theme, deserialized);
    }

    #[test]
    fn test_available_themes_includes_builtins() {
        let tmp = tempfile::tempdir().unwrap();
        let mut names = Theme::available(tmp.path());
        names.sort();
        assert!(names.contains(&"default".to_string()));
        assert!(names.contains(&"catppuccin".to_string()));
        assert!(names.contains(&"dracula".to_string()));
        assert!(names.contains(&"gruvbox".to_string()));
    }

    #[test]
    fn test_available_themes_discovers_user_themes() {
        let tmp = tempfile::tempdir().unwrap();
        let themes_dir = tmp.path().join("themes");
        std::fs::create_dir_all(&themes_dir).unwrap();
        std::fs::write(themes_dir.join("ocean.toml"), "name = \"ocean\"\n").unwrap();

        let mut names = Theme::available(tmp.path());
        names.sort();
        assert!(names.contains(&"ocean".to_string()));
    }
}
