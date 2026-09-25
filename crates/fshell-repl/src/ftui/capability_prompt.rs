// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Session-owned capability prompts rendering non-destructive floating modals.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::terminal_mode::FullscreenTerminalGuard;
use crate::tui::components::modal_dialog;
use crate::tui::theme;
use fshell_engine::{CapAction, CapPromptRequest, CapPromptResponse, Env};
use fshell_terminal::input::{CrosstermEventSource, InputEvent, InputPoll, Key, Modifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tokio::task::JoinHandle;

pub struct CapabilityPromptTask {
    handle: Option<JoinHandle<()>>,
    session_active: Arc<AtomicBool>,
}

impl CapabilityPromptTask {
    /// Start the single prompt worker for this shell session.
    pub fn spawn(env: &Env) -> Self {
        let receiver = env.caps.cap_prompt_rx.lock().take();
        let env = env.clone();
        let session_active = Arc::new(AtomicBool::new(true));
        let worker_session_active = session_active.clone();
        let handle = tokio::spawn(async move {
            let Some(mut receiver) = receiver else {
                return;
            };
            while let Some(request) = receiver.recv().await {
                let response = handle_request(&env, &request, worker_session_active.clone()).await;
                let _ = request.response_tx.send(response);
            }
        });
        Self {
            handle: Some(handle),
            session_active,
        }
    }

    /// Stop the worker before the terminal session is destroyed.
    pub async fn shutdown(mut self) {
        self.session_active.store(false, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

async fn handle_request(
    env: &Env,
    request: &CapPromptRequest,
    session_active: Arc<AtomicBool>,
) -> CapPromptResponse {
    if fshell_engine::is_test_mode() {
        return CapPromptResponse::Deny;
    }

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let stdin_fd = std::io::stdin().as_raw_fd();
        if unsafe { libc::isatty(stdin_fd) } == 0 {
            return CapPromptResponse::Deny;
        }
    }

    let (action_name, target_desc) = match &request.action {
        CapAction::ReadDir(p) => ("Read Directory", p.display().to_string()),
        CapAction::WriteDir(p) => ("Write Directory", p.display().to_string()),
        CapAction::ReadFile(p) => ("Read File", p.display().to_string()),
        CapAction::WriteFile(p) => ("Write File", p.display().to_string()),
        CapAction::Network(host) => ("Network Access", host.clone()),
        CapAction::ReadEnv(var) => ("Read Env Var", var.clone()),
        CapAction::WriteEnv(var) => ("Write Env Var", var.clone()),
        CapAction::ProcessSpawn => ("Spawn Subprocess", "Child process execution".to_string()),
    };

    let theme = env.active_theme();
    let cmd_name = request.cmd_name.clone();
    let input_active = session_active.clone();

    let response = tokio::task::spawn_blocking(move || {
        if !input_active.load(Ordering::Acquire) {
            return CapPromptResponse::Deny;
        }

        let _guard = match FullscreenTerminalGuard::enter(true) {
            Ok(g) => g,
            Err(_) => return CapPromptResponse::Deny,
        };

        let backend = CrosstermBackend::new(std::io::stdout());
        let mut terminal = match Terminal::new(backend) {
            Ok(t) => t,
            Err(_) => return CapPromptResponse::Deny,
        };
        let _ = terminal.clear();

        let mut input = CrosstermEventSource::new();

        loop {
            if !input_active.load(Ordering::Acquire) {
                return CapPromptResponse::Deny;
            }

            let draw_res = terminal.draw(|frame| {
                let size = frame.area();
                let modal_area = modal_dialog::centered_fixed(62, 13, size);
                let inner = modal_dialog::render_modal_frame(
                    modal_area,
                    frame.buffer_mut(),
                    &theme,
                    "Security Capability Request",
                );

                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Length(2),
                        Constraint::Length(1),
                        Constraint::Length(1),
                    ])
                    .split(inner);

                let label_style = theme::muted_style(&theme);
                let val_style = theme::title_style(&theme);

                let cmd_line = Line::from(vec![
                    Span::styled("  Command:  ", label_style),
                    Span::styled(&cmd_name, val_style),
                ]);
                let act_line = Line::from(vec![
                    Span::styled("  Action:   ", label_style),
                    Span::styled(action_name, theme::status_warn_style(&theme)),
                ]);
                let tgt_line = Line::from(vec![
                    Span::styled("  Target:   ", label_style),
                    Span::styled(target_desc.as_str(), theme::to_style(&theme.syntax.string)),
                ]);

                let expl_line1 = Line::from(Span::styled(
                    "  This command attempted an action not permitted",
                    label_style,
                ));
                let expl_line2 = Line::from(Span::styled(
                    "  in strict mode or without granted capability.",
                    label_style,
                ));

                let hotkey_line = Line::from(vec![
                    Span::raw("  "),
                    Span::styled("[y] ", theme::key_hint_key_style(&theme)),
                    Span::raw("Grant Once   "),
                    Span::styled("[a] ", theme::key_hint_key_style(&theme)),
                    Span::raw("Always Grant   "),
                    Span::styled("[Esc/n] ", theme::key_hint_key_style(&theme)),
                    Span::raw("Deny"),
                ]);

                frame.render_widget(Paragraph::new(cmd_line), chunks[1]);
                frame.render_widget(Paragraph::new(act_line), chunks[2]);
                frame.render_widget(Paragraph::new(tgt_line), chunks[3]);
                frame.render_widget(Paragraph::new(vec![expl_line1, expl_line2]), chunks[5]);
                frame.render_widget(Paragraph::new(hotkey_line), chunks[7]);
            });

            if draw_res.is_err() {
                return CapPromptResponse::Deny;
            }

            let key = match input.poll(std::time::Duration::from_millis(100)) {
                Ok(InputPoll::Event(InputEvent::Key(key))) => key,
                Ok(InputPoll::Event(_) | InputPoll::Timeout) => continue,
                Ok(InputPoll::Closed) | Err(_) => return CapPromptResponse::Deny,
            };

            match key.key {
                Key::Character('y') | Key::Character('Y') => {
                    return CapPromptResponse::GrantOnce;
                }
                Key::Character('a') | Key::Character('A') => {
                    return CapPromptResponse::GrantAlways;
                }
                Key::Character('n') | Key::Character('N') | Key::Escape => {
                    return CapPromptResponse::Deny;
                }
                Key::Character('c') if key.modifiers.contains(Modifiers::CONTROL) => {
                    return CapPromptResponse::Deny;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap_or(CapPromptResponse::Deny);

    if response == CapPromptResponse::GrantAlways {
        env.caps
            .caps
            .write()
            .grant(request.action.to_resource_handle());
    }

    response
}
