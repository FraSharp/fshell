// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(clippy::unwrap_used, clippy::panic)]

use futures::stream;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::{Frame, Terminal};

use fshell_terminal::input::{InputEvent, Key, KeyEvent, Modifiers};
use fshell_terminal::raw;
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
    // Simulate REPL environment where raw mode is ALREADY enabled.
    // Headless CI (stdin = /dev/null, no controlling tty) cannot enter raw
    // mode; skip gracefully instead of failing the suite.
    if raw::enable_raw_mode().is_err() {
        eprintln!("skip: no tty available for raw-mode ownership test");
        return;
    }
    let raw_initial = raw::is_raw_mode_enabled();

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
        let session = match TerminalSession::enter(device, options) {
            Ok(session) => session,
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    || matches!(e.raw_os_error(), Some(6) | Some(25)) =>
            {
                eprintln!("skip: no tty available for nested session: {e}");
                let _ = raw::disable_raw_mode();
                return;
            }
            Err(e) => {
                let _ = raw::disable_raw_mode();
                panic!("enter nested session: {e}");
            }
        };
        assert_eq!(session.mode(), TerminalMode::Fullscreen);
        // Session dropped here
    }

    // After nested session dropped, raw mode MUST STILL BE ENABLED because the session
    // did not enable it itself!
    let raw_after = raw::is_raw_mode_enabled();
    // Cleanup for test harness
    let _ = raw::disable_raw_mode();

    if raw_initial {
        assert!(
            raw_after,
            "nested session must NOT disable outer raw mode on drop!"
        );
    }
}
