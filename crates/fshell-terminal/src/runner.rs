// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Unified TUI runner for interactive shell interfaces.
//!
//! Provides the generic `run_tui` event loop that connects a `ShellTuiApp`
//! with an asynchronous message stream and an active `TerminalSession`.

use futures::StreamExt;
use ratatui::Frame;
use ratatui::layout::Rect;
use std::io;

use crate::session::TerminalSession;

/// Action returned by a `ShellTuiApp` after handling a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppFlow<T> {
    /// State changed, schedule a terminal redraw.
    Continue,
    /// Message was handled but UI state did not change; skip redraw.
    Ignore,
    /// Application finished; exit the event loop and yield output.
    Break(T),
}

/// An interactive TUI application runnable by `run_tui`.
pub trait ShellTuiApp {
    /// Type of message consumed by this application.
    type Message;
    /// Type of output emitted on exit.
    type Output;

    /// Process an incoming message and decide control flow.
    fn handle_message(&mut self, msg: Self::Message) -> AppFlow<Self::Output>;

    /// Render the application into the provided viewport area.
    fn render(&mut self, frame: &mut Frame<'_>, area: Rect);
}

/// Run an interactive TUI application with any Ratatui terminal backend.
pub async fn run_tui_terminal<A, S, B>(
    app: &mut A,
    terminal: &mut ratatui::Terminal<B>,
    mut events: S,
) -> io::Result<Option<A::Output>>
where
    A: ShellTuiApp,
    S: futures::Stream<Item = A::Message> + Unpin,
    B: ratatui::backend::Backend,
{
    let mut dirty = true;

    loop {
        if dirty {
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    app.render(frame, area);
                })
                .map_err(|e| io::Error::other(e.to_string()))?;
            dirty = false;
        }

        match events.next().await {
            Some(msg) => match app.handle_message(msg) {
                AppFlow::Continue => {
                    dirty = true;
                }
                AppFlow::Ignore => {}
                AppFlow::Break(output) => {
                    return Ok(Some(output));
                }
            },
            None => {
                // Event stream ended (EOF / terminal input closed)
                return Ok(None);
            }
        }
    }
}

/// Run an interactive TUI application to completion using an active `TerminalSession`.
pub async fn run_tui<A, S>(
    app: &mut A,
    session: &mut TerminalSession,
    events: S,
) -> io::Result<Option<A::Output>>
where
    A: ShellTuiApp,
    S: futures::Stream<Item = A::Message> + Unpin,
{
    run_tui_terminal(app, session.terminal_mut(), events).await
}
