// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptConfig {
    #[serde(default = "default_left_separator")]
    pub left_separator: String,
    #[serde(default = "default_right_separator")]
    pub right_separator: String,
    #[serde(default)]
    pub left: Vec<SegmentConfig>,
    #[serde(default)]
    pub right: Vec<SegmentConfig>,
    #[serde(default)]
    pub separator_style: SeparatorStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
}

fn default_left_separator() -> String {
    " ".to_string()
}

fn default_right_separator() -> String {
    " ".to_string()
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            left_separator: " ".to_string(),
            right_separator: " ".to_string(),
            left: vec![
                SegmentConfig::new(
                    SegmentType::CargoRun,
                    Some(ColorSpec::Named("warning".into())),
                    true,
                ),
                SegmentConfig::new(
                    SegmentType::User,
                    Some(ColorSpec::Named("keyword".into())),
                    true,
                ),
                SegmentConfig {
                    shorten: true,
                    ..SegmentConfig::new(
                        SegmentType::Pwd,
                        Some(ColorSpec::Named("string".into())),
                        true,
                    )
                },
                SegmentConfig {
                    show_only_in_repo: true,
                    ..SegmentConfig::new(
                        SegmentType::GitBranch,
                        Some(ColorSpec::Named("ok".into())),
                        true,
                    )
                },
                SegmentConfig {
                    hide_when_clean: true,
                    ..SegmentConfig::new(
                        SegmentType::GitStatus,
                        Some(ColorSpec::Named("ok".into())),
                        false,
                    )
                },
                SegmentConfig {
                    fg: Some(ColorSpec::Conditional {
                        ok: "ok".into(),
                        err: "error".into(),
                    }),
                    ..SegmentConfig::new(SegmentType::Char, None, true)
                },
            ],
            right: vec![
                SegmentConfig {
                    hide_on_zero: true,
                    ..SegmentConfig::new(
                        SegmentType::ExitCode,
                        Some(ColorSpec::Named("error".into())),
                        true,
                    )
                },
                SegmentConfig {
                    hide_under_ms: 1000,
                    ..SegmentConfig::new(
                        SegmentType::Duration,
                        Some(ColorSpec::Named("muted".into())),
                        false,
                    )
                },
                SegmentConfig {
                    hide_on_zero: true,
                    ..SegmentConfig::new(
                        SegmentType::Jobs,
                        Some(ColorSpec::Named("builtin".into())),
                        false,
                    )
                },
            ],
            separator_style: SeparatorStyle::None,
            preset: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SeparatorStyle {
    Arrow,
    Chevron,
    Flame,
    Pipe,
    Slash,
    Dots,
    #[default]
    None,
    Custom(String),
}

impl SeparatorStyle {
    pub fn glyph(&self) -> &str {
        match self {
            Self::Arrow => "\u{e0b0}",
            Self::Chevron => "\u{e0b1}",
            Self::Flame => "\u{e0b2}",
            Self::Pipe => "│",
            Self::Slash => "/",
            Self::Dots => "·",
            Self::None => " ",
            Self::Custom(s) => s,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SegmentType {
    CargoRun,
    User,
    Host,
    Pwd,
    GitBranch,
    GitStatus,
    ExitCode,
    Duration,
    Jobs,
    Char,
    Time,
    Date,
    Timestamp,
    Shlvl,
    Shell,
    Line,
    Aws,
    Kube,
    Venv,
    Ssh,
    Text,
    Separator,
    Newline,
    Custom,
}

impl SegmentType {
    pub fn name(&self) -> &'static str {
        match self {
            Self::CargoRun => "cargo_run",
            Self::User => "user",
            Self::Host => "host",
            Self::Pwd => "pwd",
            Self::GitBranch => "git_branch",
            Self::GitStatus => "git_status",
            Self::ExitCode => "exit_code",
            Self::Duration => "duration",
            Self::Jobs => "jobs",
            Self::Char => "char",
            Self::Time => "time",
            Self::Date => "date",
            Self::Timestamp => "timestamp",
            Self::Shlvl => "shlvl",
            Self::Shell => "shell",
            Self::Line => "line",
            Self::Aws => "aws",
            Self::Kube => "kube",
            Self::Venv => "venv",
            Self::Ssh => "ssh",
            Self::Text => "text",
            Self::Separator => "separator",
            Self::Newline => "newline",
            Self::Custom => "custom",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::CargoRun => "Cargo Run Badge",
            Self::User => "User",
            Self::Host => "Host",
            Self::Pwd => "Current Directory",
            Self::GitBranch => "Git Branch",
            Self::GitStatus => "Git Status",
            Self::ExitCode => "Exit Code",
            Self::Duration => "Duration",
            Self::Jobs => "Jobs",
            Self::Char => "Prompt Symbol",
            Self::Time => "Time",
            Self::Date => "Date",
            Self::Timestamp => "Timestamp",
            Self::Shlvl => "Shell Level",
            Self::Shell => "Shell Name",
            Self::Line => "Line Number",
            Self::Aws => "AWS Profile",
            Self::Kube => "Kube Context",
            Self::Venv => "Virtual Env",
            Self::Ssh => "SSH Session",
            Self::Text => "Text",
            Self::Separator => "Separator",
            Self::Newline => "Newline",
            Self::Custom => "Custom Command",
        }
    }

    pub fn fields_for_type(&self) -> &'static [&'static str] {
        match self {
            Self::Char => &["fg", "bg", "bold"],
            Self::Pwd => &["fg", "bg", "shorten", "prefix", "suffix"],
            Self::GitBranch => &["fg", "bg", "show_only_in_repo", "prefix", "suffix"],
            Self::GitStatus => &["fg", "bg", "hide_when_clean", "prefix", "suffix"],
            Self::Text => &["fg", "bg", "bold", "italic", "text", "prefix", "suffix"],
            Self::Custom => &["fg", "bg", "prefix", "suffix"],
            Self::Separator => &["fg", "bg", "separator_style"],
            Self::Newline => &[],
            Self::ExitCode | Self::Duration | Self::Jobs => {
                &["fg", "bg", "bold", "hide_on_zero", "prefix", "suffix"]
            }
            _ => &["fg", "bg", "bold", "italic", "prefix", "suffix"],
        }
    }

    pub fn all() -> Vec<(&'static str, &'static str, &'static str)> {
        vec![
            ("user", "Current username", "ariel"),
            ("host", "Hostname", "macbook"),
            ("pwd", "Current directory", "~/src/fshell"),
            ("git_branch", "Git branch name", "main"),
            ("git_status", "Git dirty/ahead/behind", "±2 ⇡1"),
            ("exit_code", "Last exit code", "[!] 127"),
            ("duration", "Command runtime", "2.5s"),
            ("jobs", "Background job count", "3 jobs"),
            ("char", "Prompt symbol", "> or #"),
            ("time", "Current time (HH:MM:SS)", "14:32:05"),
            ("date", "Current date (YYYY-MM-DD)", "2026-06-09"),
            ("timestamp", "Short time (HH:MM)", "14:32"),
            ("shlvl", "Shell nesting level", "sh2"),
            ("shell", "Shell name", "fsh"),
            ("line", "History line number", "42"),
            ("aws", "AWS profile name", "dev"),
            ("kube", "kubectl context", "prod-cluster"),
            ("venv", "Python virtualenv", ".venv"),
            ("ssh", "SSH session indicator", "connected"),
            ("text", "Literal text", "your text"),
            ("separator", "Visual separator", "│"),
            ("newline", "Line break", ""),
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentConfig {
    #[serde(rename = "type")]
    pub r#type: SegmentType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<ColorSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<ColorSpec>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub separator: Option<String>,
    #[serde(default)]
    pub hide_on_zero: bool,
    #[serde(default)]
    pub hide_when_clean: bool,
    #[serde(default)]
    pub hide_under_ms: u64,
    #[serde(default)]
    pub show_only_in_repo: bool,
    #[serde(default)]
    pub shorten: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub refresh_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub separator_style: Option<SeparatorStyle>,
}

impl SegmentConfig {
    pub fn new(r#type: SegmentType, fg: Option<ColorSpec>, bold: bool) -> Self {
        Self {
            r#type,
            fg,
            bold,
            ..Default::default()
        }
    }
}

impl Default for SegmentConfig {
    fn default() -> Self {
        Self {
            r#type: SegmentType::Text,
            text: None,
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            prefix: String::new(),
            suffix: String::new(),
            separator: None,
            hide_on_zero: false,
            hide_when_clean: false,
            hide_under_ms: 0,
            show_only_in_repo: false,
            shorten: false,
            command: None,
            refresh_ms: 0,
            separator_style: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ColorSpec {
    Named(String),
    Hex(String),
    Conditional { ok: String, err: String },
}

impl ColorSpec {
    /// Resolve this ColorSpec into RGB using the active Theme's semantic palette and dictionary.
    pub fn resolve_to_rgb(&self, exit_ok: bool, theme: &crate::theme::Theme) -> (u8, u8, u8) {
        match self {
            ColorSpec::Named(name) => theme.resolve_color(name),
            ColorSpec::Hex(hex) => theme.resolve_color(hex),
            ColorSpec::Conditional { ok, err } => {
                if exit_ok {
                    theme.resolve_color(ok)
                } else {
                    theme.resolve_color(err)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn test_color_spec_resolution() {
        let theme = Theme::default_theme();
        let spec_named = ColorSpec::Named("ok".into());
        assert_eq!(
            spec_named.resolve_to_rgb(true, &theme),
            theme.status.ok.to_rgb()
        );

        let spec_hex = ColorSpec::Hex("#abcdef".into());
        assert_eq!(spec_hex.resolve_to_rgb(true, &theme), (0xab, 0xcd, 0xef));

        let spec_cond = ColorSpec::Conditional {
            ok: "ok".into(),
            err: "error".into(),
        };
        assert_eq!(
            spec_cond.resolve_to_rgb(true, &theme),
            theme.status.ok.to_rgb()
        );
        assert_eq!(
            spec_cond.resolve_to_rgb(false, &theme),
            theme.status.error.to_rgb()
        );
    }
}
