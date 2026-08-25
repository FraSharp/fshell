// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::ftui::statusbar::{StatusBar, StatusBarWidget};
use fshell_core::theme::Theme;
use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend};
use std::io::Write;
use std::time::Duration;

/// RAII guard that locks terminal scrolling margins using DECSTBM (\x1b[1;limit_row r)
/// and restores full-screen scrolling (\x1b[r) when dropped.
pub struct MarginGuard {
    active: bool,
}

impl MarginGuard {
    pub fn new(term_height: u16) -> Self {
        if term_height > 4 {
            let limit_row = term_height - 2;
            let mut stdout = std::io::stdout();
            // ESC 7 (save cursor), set scroll region 1..limit_row, ESC 8 (restore cursor)
            let _ = write!(stdout, "\x1b7\x1b[1;{}r\x1b8", limit_row);
            let _ = stdout.flush();
            Self { active: true }
        } else {
            Self { active: false }
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }
}

impl Drop for MarginGuard {
    fn drop(&mut self) {
        if self.active {
            let mut stdout = std::io::stdout();
            // ESC 7 (save cursor), reset scroll region, ESC 8 (restore cursor)
            let _ = write!(stdout, "\x1b7\x1b[r\x1b8");
            let _ = stdout.flush();
        }
    }
}

/// Checks if a command line represents a full-screen TTY application
/// (e.g. vim, htop, less, fzf) that manages its own alternate screen.
pub fn is_fullscreen_app(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return false;
    }
    let first_word = trimmed.split_whitespace().next().unwrap_or("");
    let name = std::path::Path::new(first_word)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(first_word);

    if matches!(
        name,
        "vim"
            | "vi"
            | "nvim"
            | "nano"
            | "emacs"
            | "micro"
            | "hx"
            | "helix"
            | "less"
            | "more"
            | "moar"
            | "ov"
            | "view"
            | "rview"
            | "htop"
            | "top"
            | "btop"
            | "bottom"
            | "btm"
            | "glances"
            | "fzf"
            | "peco"
            | "sk"
            | "tmux"
            | "screen"
            | "zellij"
            | "man"
            | "yazi"
            | "ranger"
            | "nnn"
            | "lf"
            | "vifm"
            | "joshuto"
            | "broot"
            | "lazygit"
            | "lazydocker"
            | "tig"
            | "ncdu"
            | "gdu"
            | "bat"
            | "delta"
            | "page"
            | "nmtui"
            | "k9s"
            | "mprocs"
            | "ssh"
            | "mosh"
            | "gh"
            | "claude"
            | "ipython"
            | "python"
            | "python3"
            | "node"
            | "irb"
            | "ruby"
            | "gdb"
            | "lldb"
            | "iex"
            | "erl"
            | "ghci"
            | "julia"
    ) {
        return true;
    }

    // Git subcommands that launch pagers (git diff, git log, git show)
    if name == "git" {
        let sub = trimmed.split_whitespace().nth(1).unwrap_or("");
        if matches!(sub, "diff" | "log" | "show" | "blame" | "reflog") {
            return true;
        }
    }

    false
}

/// Renders the status bar at the locked bottom area (term_h - 2) while preserving cursor position.
pub fn render_persistent_status_bar(
    status_terminal: &mut Option<Terminal<CrosstermBackend<std::io::Stdout>>>,
    status_bar: &StatusBar,
    theme: &Theme,
    term_w: u16,
    term_h: u16,
) {
    if term_h <= 4 || !status_bar.visible {
        return;
    }

    if status_terminal.is_none() {
        let stdout = std::io::stdout();
        let backend = CrosstermBackend::new(stdout);
        if let Ok(st) = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(ratatui::layout::Rect::new(0, term_h - 2, term_w, 2)),
            },
        ) {
            *status_terminal = Some(st);
        }
    }

    if let Some(st) = status_terminal.as_mut() {
        let mut stdout = std::io::stdout();
        // ESC 7: Save cursor position
        let _ = write!(stdout, "\x1b7");
        let _ = stdout.flush();

        let _ = st.draw(|f| {
            f.render_widget(StatusBarWidget { status_bar, theme }, f.area());
        });

        // ESC 8: Restore cursor position
        let _ = write!(stdout, "\x1b8");
        let _ = stdout.flush();
    }
}

/// Real-time tick render for background status updates during long command execution.
pub fn render_status_bar_live_tick(
    status_bar: &StatusBar,
    theme: &Theme,
    elapsed: Duration,
    _init_term_w: u16,
    _init_term_h: u16,
) {
    let (term_w, term_h) = crossterm::terminal::size().unwrap_or((_init_term_w, _init_term_h));
    if term_h <= 4 || !status_bar.visible {
        return;
    }

    // Dynamic window resize check: re-issue DECSTBM scroll region if height changed.
    // Re-issuing it on *every* tick (200 ms) during a long-running command that is
    // actively printing is the source of the "status bar garbles command output"
    // interleave — the scroll-region rewrite lands mid-output. Only re-issue when
    // the terminal height actually changed since the last tick.
    static LAST_SCROLL_H: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
    if term_h > 4 && LAST_SCROLL_H.swap(term_h, std::sync::atomic::Ordering::Relaxed) != term_h {
        let limit_row = term_h - 2;
        let mut stdout = std::io::stdout();
        let _ = write!(stdout, "\x1b7\x1b[1;{}r\x1b8", limit_row);
        let _ = stdout.flush();
    }

    let mut updated_sb = status_bar.clone();
    updated_sb.last_command_elapsed = Some(elapsed);

    let stdout = std::io::stdout();
    let backend = CrosstermBackend::new(stdout);
    if let Ok(mut st) = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Fixed(ratatui::layout::Rect::new(0, term_h - 2, term_w, 2)),
        },
    ) {
        let mut stdout = std::io::stdout();
        let _ = write!(stdout, "\x1b7");
        let _ = stdout.flush();

        let _ = st.draw(|f| {
            f.render_widget(
                StatusBarWidget {
                    status_bar: &updated_sb,
                    theme,
                },
                f.area(),
            );
        });

        let _ = write!(stdout, "\x1b8");
        let _ = stdout.flush();
    }
}
