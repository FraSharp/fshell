// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(clippy::unwrap_used)]

use futures::stream;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::{Frame, Terminal};

use fshell_terminal::input::{InputEvent, Key, KeyEvent, Modifiers};
use fshell_terminal::runner::{AppFlow, ShellTuiApp, run_tui_terminal};
use fshell_terminal::session::{
    TerminalDevice, TerminalMode, TerminalSession, TerminalSessionOptions,
};

struct TestCounterApp {
    count: i32,
    rendered_frames: usize,
    target: i32,
}

impl ShellTuiApp for TestCounterApp {
    type Message = InputEvent;
    type Output = i32;

    fn handle_message(&mut self, msg: Self::Message) -> AppFlow<Self::Output> {
        if let InputEvent::Key(key) = msg
            && key.key == Key::Enter
        {
            self.count += 1;
            if self.count >= self.target {
                return AppFlow::Break(self.count);
            }
            return AppFlow::Continue;
        }
        AppFlow::Ignore
    }

    fn render(&mut self, _frame: &mut Frame<'_>, _area: Rect) {
        self.rendered_frames += 1;
    }
}

#[tokio::test]
async fn test_run_tui_app_lifecycle_headless() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("create test terminal");

    let events = stream::iter(vec![
        InputEvent::Key(KeyEvent::new(Key::Enter, Modifiers::empty())),
        InputEvent::Key(KeyEvent::new(Key::Enter, Modifiers::empty())),
        InputEvent::Key(KeyEvent::new(Key::Enter, Modifiers::empty())),
    ]);

    let mut app = TestCounterApp {
        count: 0,
        rendered_frames: 0,
        target: 3,
    };

    let result = run_tui_terminal(&mut app, &mut terminal, events)
        .await
        .expect("run_tui_terminal");
    assert_eq!(result, Some(3));
    assert!(app.rendered_frames >= 3);
}

#[tokio::test]
async fn test_run_tui_eof_returns_none_headless() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("create test terminal");

    // Empty event stream simulates EOF / terminal closed
    let events = stream::empty::<InputEvent>();

    let mut app = TestCounterApp {
        count: 0,
        rendered_frames: 0,
        target: 10,
    };

    let result = run_tui_terminal(&mut app, &mut terminal, events)
        .await
        .expect("run_tui_terminal");
    assert_eq!(result, None);
}

#[test]
fn test_explicit_ownership_nested_raw_mode() {
    // Simulate REPL environment where raw mode is ALREADY enabled
    let _ = crossterm::terminal::enable_raw_mode();
    let raw_initial = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);

    {
        // Enter a nested session using stdio in Fullscreen mode (no DSR query)
        let options = TerminalSessionOptions {
            mode: TerminalMode::Fullscreen,
            enable_mouse: false,
            enable_bracketed_paste: false,
            hide_cursor: false,
            install_signal_guard: false,
            install_panic_hook: false,
        };

        let device = TerminalDevice::stdio();
        let session = TerminalSession::enter(device, options).expect("enter nested session");
        assert_eq!(session.mode(), TerminalMode::Fullscreen);
        // Session dropped here
    }

    // After nested session dropped, raw mode MUST STILL BE ENABLED because the session
    // did not enable it itself!
    let raw_after = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    // Cleanup for test harness
    let _ = crossterm::terminal::disable_raw_mode();

    if raw_initial {
        assert!(
            raw_after,
            "nested session must NOT disable outer raw mode on drop!"
        );
    }
}
