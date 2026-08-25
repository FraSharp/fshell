// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::time::{Duration, Instant};

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use crate::theme_ext::ThemeColorRatatui;

#[derive(Clone)]
pub struct StatusBar {
    pub last_exit_code: Option<i64>,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
    pub git_ahead: usize,
    pub git_behind: usize,
    pub job_count: usize,
    pub mode_indicator: String,
    pub hostname: String,
    pub username: String,
    pub last_command_elapsed: Option<Duration>,
    pub visible: bool,
    command_start: Option<Instant>,
}

impl StatusBar {
    pub fn is_enabled(env: &fshell_engine::Env) -> bool {
        // 1. Check ShellOptions
        if env.options.read().status_bar {
            return true;
        }
        // 2. Check shell env vars ($FSH_STATUS_BAR)
        if let Some(val) = env.vars.read().get("FSH_STATUS_BAR") {
            match val {
                fshell_core::Val::Bool(b) => return *b,
                fshell_core::Val::Int(i) => return *i != 0,
                fshell_core::Val::String(s) => {
                    let s_lower = s.trim().to_lowercase();
                    return s_lower == "1"
                        || s_lower == "true"
                        || s_lower == "yes"
                        || s_lower == "on";
                }
                _ => {}
            }
        }
        // 3. Check process env vars (FSH_STATUS_BAR)
        if let Ok(val) = std::env::var("FSH_STATUS_BAR") {
            let val_lower = val.trim().to_lowercase();
            return val_lower == "1"
                || val_lower == "true"
                || val_lower == "yes"
                || val_lower == "on";
        }
        false
    }

    pub fn new(hostname: String, username: String) -> Self {
        Self {
            last_exit_code: None,
            git_branch: None,
            git_dirty: false,
            git_ahead: 0,
            git_behind: 0,
            job_count: 0,
            mode_indicator: "I".to_string(),
            hostname,
            username,
            last_command_elapsed: None,
            visible: false,
            command_start: None,
        }
    }

    pub fn with_env(mut self, env: &fshell_engine::Env) -> Self {
        self.visible = Self::is_enabled(env);
        self
    }

    pub fn start_command_timer(&mut self) {
        if !self.visible {
            return;
        }
        self.command_start = Some(Instant::now());
    }

    pub fn end_command_timer(&mut self) {
        if !self.visible {
            return;
        }
        if let Some(start) = self.command_start.take() {
            self.last_command_elapsed = Some(start.elapsed());
        }
    }

    pub fn update_git(&mut self, branch: Option<String>, dirty: bool, ahead: usize, behind: usize) {
        if !self.visible {
            return;
        }
        self.git_branch = branch;
        self.git_dirty = dirty;
        self.git_ahead = ahead;
        self.git_behind = behind;
    }

    pub fn set_exit_code(&mut self, code: i64) {
        if !self.visible {
            return;
        }
        self.last_exit_code = Some(code);
    }

    pub fn set_mode(&mut self, mode: &str) {
        if !self.visible {
            return;
        }
        self.mode_indicator = mode.to_string();
    }

    pub fn set_job_count(&mut self, count: usize) {
        if !self.visible {
            return;
        }
        self.job_count = count;
    }

    fn format_elapsed(&self) -> String {
        match self.last_command_elapsed {
            Some(d) if d < Duration::from_micros(1) => {
                format!("{}ns", d.as_nanos())
            }
            Some(d) if d < Duration::from_millis(1) => {
                format!("{}µs", d.as_micros())
            }
            Some(d) if d < Duration::from_secs(1) => {
                let ms = d.as_secs_f64() * 1000.0;
                if ms < 10.0 {
                    format!("{:.2}ms", ms)
                } else {
                    format!("{:.1}ms", ms)
                }
            }
            Some(d) if d < Duration::from_secs(60) => {
                format!("{:.2}s", d.as_secs_f64())
            }
            Some(d) => {
                let secs = d.as_secs();
                format!("{}m{:02}s", secs / 60, secs % 60)
            }
            None => String::new(),
        }
    }

    pub(crate) fn format_timestamp() -> String {
        let now = chrono::Local::now();
        now.format("%H:%M:%S").to_string()
    }
}

pub struct StatusBarWidget<'a> {
    pub status_bar: &'a StatusBar,
    pub theme: &'a fshell_core::theme::Theme,
}

impl Widget for StatusBarWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.status_bar.visible {
            return;
        }

        let chunks = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);

        let muted_style = self.theme.status.muted.to_style();

        // Row 0: separator
        let sep = "─".repeat(chunks[0].width as usize);
        Paragraph::new(Line::from(Span::styled(sep, muted_style))).render(chunks[0], buf);

        // Row 1: content
        let divider = Span::styled("  │  ", muted_style);

        // --- Left side ---
        let mut left_spans: Vec<Span> = Vec::new();

        // 1. Mode badge
        let mode_bg = match self.status_bar.mode_indicator.as_str() {
            "I" | "INSERT" => self.theme.status.info.to_ratatui_color(),
            "N" | "NORMAL" => self.theme.status.warning.to_ratatui_color(),
            "V" | "VISUAL" => self.theme.status.ok.to_ratatui_color(),
            _ => self.theme.status.muted.to_ratatui_color(),
        };
        let mode_label = match self.status_bar.mode_indicator.as_str() {
            "I" | "INSERT" => " INSERT ",
            "N" | "NORMAL" => " NORMAL ",
            "V" | "VISUAL" => " VISUAL ",
            other => other,
        };
        let badge_style = Style::default()
            .bg(mode_bg)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD);
        left_spans.push(Span::styled(mode_label, badge_style));
        left_spans.push(Span::raw("  "));

        // 2. Exit status
        let (exit_style, exit_text) = match self.status_bar.last_exit_code {
            Some(0) | None => (
                self.theme.status.ok.to_style().add_modifier(Modifier::BOLD),
                "✔".to_string(),
            ),
            Some(code) => (
                self.theme
                    .status
                    .error
                    .to_style()
                    .add_modifier(Modifier::BOLD),
                format!("✘ {}", code),
            ),
        };
        left_spans.push(Span::styled(exit_text, exit_style));

        // 3. Git
        if let Some(ref branch) = self.status_bar.git_branch {
            left_spans.push(divider.clone());
            left_spans.push(Span::styled(
                branch.clone(),
                self.theme
                    .syntax
                    .keyword
                    .to_style()
                    .add_modifier(Modifier::BOLD),
            ));
            if self.status_bar.git_dirty {
                left_spans.push(Span::styled(
                    " ●",
                    self.theme
                        .status
                        .warning
                        .to_style()
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if self.status_bar.git_ahead > 0 {
                left_spans.push(Span::styled(
                    format!(" ⇡{}", self.status_bar.git_ahead),
                    self.theme.status.info.to_style(),
                ));
            }
            if self.status_bar.git_behind > 0 {
                left_spans.push(Span::styled(
                    format!(" ⇣{}", self.status_bar.git_behind),
                    self.theme.status.error.to_style(),
                ));
            }
        }

        // 4. Job count
        if self.status_bar.job_count > 0 {
            left_spans.push(divider.clone());
            left_spans.push(Span::styled(
                format!("⚙ {}", self.status_bar.job_count),
                self.theme.status.info.to_style(),
            ));
        }

        // --- Right side ---
        let mut right_spans: Vec<Span> = Vec::new();

        // 1. User@host
        right_spans.push(Span::styled(
            self.status_bar.username.clone(),
            self.theme
                .syntax
                .keyword
                .to_style()
                .add_modifier(Modifier::BOLD),
        ));
        right_spans.push(Span::styled("@", muted_style));
        right_spans.push(Span::styled(self.status_bar.hostname.clone(), muted_style));

        // 2. Elapsed time
        let elapsed = self.status_bar.format_elapsed();
        if !elapsed.is_empty() {
            let elapsed_style = if let Some(d) = self.status_bar.last_command_elapsed {
                if d >= Duration::from_secs(10) {
                    self.theme
                        .status
                        .error
                        .to_style()
                        .add_modifier(Modifier::BOLD)
                } else if d >= Duration::from_secs(2) {
                    self.theme
                        .status
                        .warning
                        .to_style()
                        .add_modifier(Modifier::BOLD)
                } else {
                    muted_style
                }
            } else {
                muted_style
            };
            right_spans.push(divider.clone());
            right_spans.push(Span::styled(elapsed, elapsed_style));
        }

        // 3. Timestamp
        right_spans.push(divider.clone());
        right_spans.push(Span::styled(StatusBar::format_timestamp(), muted_style));

        // Build full line: left + padding + right
        let left_width: u16 = left_spans.iter().map(|s| s.width() as u16).sum();
        let right_width: u16 = right_spans.iter().map(|s| s.width() as u16).sum();
        let pad = chunks[1].width.saturating_sub(left_width + right_width);

        let mut all_spans = left_spans;
        if pad > 0 {
            all_spans.push(Span::raw(" ".repeat(pad as usize)));
        }
        all_spans.extend(right_spans);

        Paragraph::new(Line::from(all_spans)).render(chunks[1], buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fshell_engine::Env;

    #[test]
    fn test_status_bar_default_disabled() {
        let env = Env::new();
        // By default status_bar is disabled (false) unless FSH_STATUS_BAR=1 is set
        if std::env::var("FSH_STATUS_BAR").is_err() {
            assert!(!StatusBar::is_enabled(&env));
            let sb = StatusBar::new("host".into(), "user".into()).with_env(&env);
            assert!(!sb.visible);
        }
    }

    #[test]
    fn test_status_bar_enabled_via_shell_options() {
        let env = Env::new();
        {
            let mut opts = env.options.write();
            opts.status_bar = true;
        }
        assert!(StatusBar::is_enabled(&env));
        let sb = StatusBar::new("host".into(), "user".into()).with_env(&env);
        assert!(sb.visible);
    }
}
