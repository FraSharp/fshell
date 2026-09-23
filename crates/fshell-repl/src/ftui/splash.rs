// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Interactive first-run startup splash screen and environment status overview.

use crate::terminal_mode::FullscreenTerminalGuard;
use fshell_terminal::input::{CrosstermEventSource, InputEvent, InputPoll, Key};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};
use std::io::IsTerminal;

/// Renders the first-run splash banner with environment checks and capability mode status.
pub fn show_splash(
    env: &fshell_engine::Env,
    config_ok: bool,
    config_msg: &str,
    shell_ok: bool,
    shell_msg: &str,
) {
    if fshell_engine::is_test_mode() {
        return;
    }
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        return;
    }

    let _guard = match FullscreenTerminalGuard::enter(false) {
        Ok(g) => g,
        Err(_) => return,
    };

    let mut stdout = std::io::stdout();
    let backend = CrosstermBackend::new(&mut stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(t) => t,
        Err(_) => return,
    };

    let strict_mode = Some(env.caps.caps.read())
        .map(|c| c.strict_mode)
        .unwrap_or(false);

    let version = fshell_engine::exe::full_version();
    let version = format!("v{version}");

    let mut render_span = env.trace.span(
        env.trace_context,
        "startup.splash_render",
        env.trace_mode,
        serde_json::Map::new(),
    );
    let draw_result = terminal.draw(|f| {
        let area = f.area();
        let w = area.width.min(64);
        let h = 14u16;
        let x = area.width.saturating_sub(w) / 2;
        let y = area.height.saturating_sub(h) / 2;
        let centered = Rect::new(x, y, w, h);

        let outer_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Line::from(Span::styled(
                format!(" fshell {version} "),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));

        let inner = outer_block.inner(centered);
        f.render_widget(outer_block, centered);

        let cap_style = if strict_mode {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Rgb(255, 204, 0))
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

        let cap_symbol = if strict_mode { "*" } else { "!" };
        let cap_lines: Vec<Line> = if strict_mode {
            vec![Line::from(vec![
                Span::raw("  "),
                Span::styled(cap_symbol, cap_style.add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled("capability enforcement is active", cap_style),
            ])]
        } else {
            vec![
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(cap_symbol, cap_style.add_modifier(Modifier::BOLD)),
                    Span::raw("  "),
                    Span::styled("running without capability enforcement", cap_style),
                ]),
                Line::from(vec![
                    Span::raw("     "),
                    Span::styled(
                        "use --strict or 'setopt strict' to enable",
                        Style::default().fg(Color::DarkGray),
                    ),
                ]),
            ]
        };
        f.render_widget(Paragraph::new(cap_lines), chunks[1]);

        let config_line = Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "*",
                if config_ok {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Red)
                },
            ),
            Span::raw("  config:  "),
            Span::styled(
                config_msg,
                if config_ok {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::Red)
                },
            ),
        ]);

        let shell_line = Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "*",
                if shell_ok {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Red)
                },
            ),
            Span::raw("  shell:   "),
            Span::styled(
                shell_msg,
                if shell_ok {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::Red)
                },
            ),
        ]);

        f.render_widget(Paragraph::new(vec![config_line, shell_line]), chunks[3]);

        let key_p = Paragraph::new("press any key to continue  |  d = don't show again")
            .style(Style::default().fg(Color::Rgb(120, 120, 120)))
            .alignment(Alignment::Center);
        f.render_widget(key_p, chunks[5]);
    });
    if let Some(span) = render_span.take() {
        span.finish(if draw_result.is_ok() {
            fshell_engine::trace::SpanOutcome::Ok
        } else {
            fshell_engine::trace::SpanOutcome::Error
        });
    }

    let mut input = CrosstermEventSource::new();
    let mut wait_span = env.trace.span(
        env.trace_context,
        "startup.splash_wait",
        env.trace_mode,
        serde_json::Map::new(),
    );
    let outcome = loop {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let stdin_fd = std::io::stdin().as_raw_fd();
            if unsafe { libc::isatty(stdin_fd) } == 0 {
                break fshell_engine::trace::SpanOutcome::Ok;
            }
        }
        match input.poll(std::time::Duration::from_millis(100)) {
            Ok(InputPoll::Event(InputEvent::Key(key))) => {
                if matches!(key.key, Key::Character('d' | 'D')) {
                    persist_disable_splash();
                }
                break fshell_engine::trace::SpanOutcome::Ok;
            }
            Ok(InputPoll::Event(InputEvent::Resize { .. } | InputEvent::Paste(_)))
            | Ok(InputPoll::Timeout) => {}
            Ok(InputPoll::Event(InputEvent::Mouse(_))) | Ok(InputPoll::Closed) | Err(_) => {
                break fshell_engine::trace::SpanOutcome::Cancelled;
            }
        }
    };
    if let Some(span) = wait_span.take() {
        span.finish(outcome);
    }
}

fn persist_disable_splash() {
    let Some(cfg_dir) = fshell_engine::config_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&cfg_dir);
    let _ = std::fs::write(cfg_dir.join(".splash_disabled"), "");
}
