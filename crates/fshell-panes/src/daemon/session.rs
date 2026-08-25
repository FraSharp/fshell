// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Session management for the daemon.
//!
//! A session represents a group of windows, each containing panes.
//! Sessions persist across client detaches — the daemon keeps PTY actors
//! running even when no client is attached.

use std::time::Instant;

use tokio::sync::mpsc;

use crate::layout::bsp::Split;
use crate::proto::SessionInfo;
use crate::pty::async_pty::AsyncPty;
use crate::pty::get_default_shell;
use crate::pty::grid_manager::{GridManager, PtyCommand};
use crate::pty::pty_actor::PtyActor;

use super::DaemonEvent;
use super::window::Window;

// Re-export for callers that still reference session::PaneState.
pub use super::window::PaneState;

/// Extract the basename from a shell path (e.g. "/bin/bash" → "bash").
pub fn shell_name_from_path(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// A terminal session containing windows.
pub struct Session {
    pub name: String,
    pub created_at: Instant,
    /// All windows in this session, ordered by creation.
    pub windows: Vec<Window>,
    /// Index of the currently active window in `windows`.
    pub active_window: usize,
    /// Monotonically increasing pane ID counter (session-scoped).
    pub next_pane_id: u32,
    /// Monotonically increasing window ID counter.
    pub next_window_id: u32,
    /// The sender half of the currently attached client's channel.
    /// `None` when the session is detached (running in background).
    pub attached_client: Option<mpsc::Sender<Vec<u8>>>,
    /// Current terminal dimensions (cols, rows).
    pub terminal_size: (u16, u16),
    /// Channel to send daemon events (e.g. PaneExited).
    daemon_event_tx: Option<mpsc::Sender<DaemonEvent>>,
    /// Session-wide lock-free dirty flag for rendering.
    pub dirty: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Session {
    /// Create a new session with no windows.
    pub fn new(name: String, initial_window_id: u32) -> Self {
        Self {
            name,
            created_at: Instant::now(),
            windows: Vec::new(),
            active_window: 0,
            next_pane_id: 0,
            next_window_id: initial_window_id + 1,
            attached_client: None,
            terminal_size: (80, 24),
            daemon_event_tx: None,
            dirty: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        }
    }

    /// Set the daemon event sender for this session.
    pub fn set_daemon_event_tx(&mut self, tx: mpsc::Sender<DaemonEvent>) {
        self.daemon_event_tx = Some(tx);
    }

    /// Get the active window.
    pub fn active_window(&self) -> Option<&Window> {
        if self.windows.is_empty() {
            None
        } else {
            let idx = self.active_window.min(self.windows.len() - 1);
            Some(&self.windows[idx])
        }
    }

    /// Get a mutable reference to the active window.
    pub fn active_window_mut(&mut self) -> Option<&mut Window> {
        if self.windows.is_empty() {
            None
        } else {
            if self.active_window >= self.windows.len() {
                self.active_window = self.windows.len() - 1;
            }
            let idx = self.active_window;
            Some(&mut self.windows[idx])
        }
    }

    /// Check if the session has any windows.
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Get total pane count across all windows.
    pub fn total_pane_count(&self) -> usize {
        self.windows.iter().map(|w| w.panes.len()).sum()
    }

    /// Get a lightweight info struct for listing.
    pub fn info(&self) -> SessionInfo {
        SessionInfo {
            name: self.name.clone(),
            window_count: self.windows.len(),
            pane_count: self.total_pane_count(),
            attached: self.attached_client.is_some(),
            created_at_secs: self.created_at.elapsed().as_secs(),
        }
    }

    // Window operations

    /// Add a pre-built window to the session.
    pub fn add_window(&mut self, window: Window) {
        window.resize_to_terminal(self.terminal_size.0, self.terminal_size.1);
        self.windows.push(window);
    }

    /// Create and spawn a new window with a fresh shell.
    pub fn new_window(&mut self) {
        let window_id = self.next_window_id;
        self.next_window_id += 1;

        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;

        let mut window = Window::new(window_id, pane_id);

        // Spawn the initial PTY pipeline for this window.
        let shell = get_default_shell();
        let (cols, rows) = self.terminal_size;
        let pane_rows = rows.saturating_sub(1);
        let init_cols = cols.saturating_sub(2).max(1);
        let init_rows = pane_rows.saturating_sub(2).max(1);

        let (pty_tx, pty_rx) = mpsc::channel(256);
        let (grid_tx, grid_rx) = mpsc::channel(256);

        let (mut manager, grid_ref) = GridManager::new(
            init_cols as usize,
            init_rows as usize,
            crate::proto::SCROLLBACK_LIMIT,
            grid_rx,
            self.dirty.clone(),
        );
        manager.set_pty_tx(pty_tx.clone());
        tokio::spawn(async move { manager.run().await });

        if let Ok(pty) = AsyncPty::spawn(&shell, init_cols, init_rows) {
            let session_name = self.name.clone();
            let event_tx = self.daemon_event_tx.clone();
            let actor = PtyActor::new(pty, grid_tx, pty_rx, pane_id, None);
            let actor_id = pane_id;
            tokio::spawn(async move {
                actor.run().await;
                if let Some(tx) = event_tx {
                    let _ = tx
                        .send(DaemonEvent::PaneExited {
                            session_name,
                            pane_id: actor_id,
                        })
                        .await;
                }
            });
        }

        window.name = shell_name_from_path(&shell);
        window.add_pane(pane_id, grid_ref, pty_tx, window.name.clone());
        window.resize_to_terminal(cols, rows);

        self.windows.push(window);
        self.active_window = self.windows.len() - 1;
    }

    /// Switch to the next window (wraps around).
    pub fn next_window(&mut self) {
        if !self.windows.is_empty() {
            self.active_window = (self.active_window + 1) % self.windows.len();
        }
    }

    /// Switch to the previous window (wraps around).
    pub fn previous_window(&mut self) {
        if !self.windows.is_empty() {
            self.active_window = if self.active_window == 0 {
                self.windows.len() - 1
            } else {
                self.active_window - 1
            };
        }
    }

    /// Switch to a window by index. No-op if index is out of bounds.
    pub fn switch_window(&mut self, index: usize) {
        if index < self.windows.len() {
            self.active_window = index;
        }
    }

    /// Close the active window. Returns true if the session is now empty.
    pub fn close_active_window(&mut self) -> bool {
        if self.windows.is_empty() {
            return true;
        }

        if self.active_window >= self.windows.len() {
            self.active_window = self.windows.len() - 1;
        }

        // Shutdown all PTY actors in the window.
        {
            let window = &self.windows[self.active_window];
            for pane in window.panes.values() {
                let _ = pane.pty_tx.try_send(PtyCommand::Shutdown);
            }
        }

        self.windows.remove(self.active_window);

        if self.windows.is_empty() {
            return true;
        }

        // Adjust active_window index to stay in bounds.
        if self.active_window >= self.windows.len() {
            self.active_window = self.windows.len() - 1;
        }

        // Resize the now-active window to current terminal size.
        let (cols, rows) = self.terminal_size;
        self.windows[self.active_window].resize_to_terminal(cols, rows);

        false
    }

    /// Remove a pane by ID from whichever window owns it.
    /// Returns true if the pane was found and removed.
    pub fn remove_pane(&mut self, pane_id: u32) -> bool {
        for window in &mut self.windows {
            if window.remove_pane(pane_id) {
                return true;
            }
        }
        false
    }

    // Pane operations (delegated to active window)

    /// Split the focused pane in the active window.
    pub fn spawn_pane(&mut self, direction: Split) {
        if self.windows.is_empty() {
            return;
        }
        if self.active_window >= self.windows.len() {
            self.active_window = self.windows.len() - 1;
        }
        let session_name = self.name.clone();
        let event_tx = self.daemon_event_tx.clone();
        let dirty = self.dirty.clone();
        let window = &mut self.windows[self.active_window];
        window.spawn_pane(direction, session_name, event_tx, dirty);

        // Resize all panes in the window to correct dimensions.
        let (cols, rows) = self.terminal_size;
        window.resize_to_terminal(cols, rows);
    }

    /// Close the focused pane in the active window.
    /// If the window becomes empty, it is closed too.
    pub fn close_pane(&mut self) {
        if self.windows.is_empty() {
            return;
        }
        if self.active_window >= self.windows.len() {
            self.active_window = self.windows.len() - 1;
        }
        let window = &mut self.windows[self.active_window];
        window.close_focused_pane();

        // Resize remaining panes.
        let (cols, rows) = self.terminal_size;
        window.resize_to_terminal(cols, rows);

        // If the window is now empty, close it.
        if window.is_empty() {
            self.close_active_window();
        }
    }

    // Resize

    /// Resize all windows to new terminal dimensions.
    pub fn resize_all(&mut self, cols: u16, rows: u16) {
        self.terminal_size = (cols, rows);
        for window in &self.windows {
            window.resize_to_terminal(cols, rows);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Grid;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn dummy_window(id: u32, pane_id: u32) -> Window {
        let mut window = Window::new(id, pane_id);
        let grid = Arc::new(RwLock::new(Grid::new(80, 24, 100)));
        let (tx, _rx) = mpsc::channel(16);
        window.add_pane(pane_id, grid, tx, "bash".to_string());
        window
    }

    #[test]
    fn session_new_has_no_windows() {
        let session = Session::new("test".to_string(), 0);
        assert!(session.is_empty());
        assert_eq!(session.name, "test");
    }

    #[test]
    fn session_add_window() {
        let mut session = Session::new("test".to_string(), 0);
        let window = dummy_window(0, 0);
        session.add_window(window);
        assert_eq!(session.windows.len(), 1);
        assert_eq!(session.active_window, 0);
    }

    #[test]
    fn session_next_window_wraps() {
        let mut session = Session::new("test".to_string(), 0);
        session.add_window(dummy_window(0, 0));
        session.add_window(dummy_window(1, 1));
        session.active_window = 1;
        session.next_window();
        assert_eq!(session.active_window, 0);
    }

    #[test]
    fn session_previous_window_wraps() {
        let mut session = Session::new("test".to_string(), 0);
        session.add_window(dummy_window(0, 0));
        session.add_window(dummy_window(1, 1));
        session.active_window = 0;
        session.previous_window();
        assert_eq!(session.active_window, 1);
    }

    #[test]
    fn session_switch_window() {
        let mut session = Session::new("test".to_string(), 0);
        session.add_window(dummy_window(0, 0));
        session.add_window(dummy_window(1, 1));
        session.switch_window(1);
        assert_eq!(session.active_window, 1);
        session.switch_window(99);
        assert_eq!(session.active_window, 1);
    }

    #[test]
    fn session_close_active_window() {
        let mut session = Session::new("test".to_string(), 0);
        session.add_window(dummy_window(0, 0));
        session.add_window(dummy_window(1, 1));
        let empty = session.close_active_window();
        assert!(!empty);
        assert_eq!(session.windows.len(), 1);
        assert_eq!(session.active_window, 0);
    }

    #[test]
    fn session_close_last_window() {
        let mut session = Session::new("test".to_string(), 0);
        session.add_window(dummy_window(0, 0));
        let empty = session.close_active_window();
        assert!(empty);
        assert!(session.is_empty());
    }

    #[test]
    fn session_close_shifts_active_index() {
        let mut session = Session::new("test".to_string(), 0);
        session.add_window(dummy_window(0, 0));
        session.add_window(dummy_window(1, 1));
        session.add_window(dummy_window(2, 2));
        session.active_window = 2;
        session.close_active_window();
        assert_eq!(session.active_window, 1);
    }

    #[test]
    fn session_remove_pane_finds_in_any_window() {
        let mut session = Session::new("test".to_string(), 0);
        let w0 = dummy_window(0, 0);
        let mut w1 = dummy_window(1, 1);
        let grid = Arc::new(RwLock::new(Grid::new(80, 24, 100)));
        let (tx, _rx) = mpsc::channel(16);
        w1.add_pane(42, grid, tx, "zsh".to_string());
        session.add_window(w0);
        session.add_window(w1);
        assert!(session.remove_pane(42));
        assert!(!session.windows[1].panes.contains_key(&42));
    }

    #[test]
    fn session_info() {
        let mut session = Session::new("my-session".to_string(), 0);
        session.add_window(dummy_window(0, 0));
        session.add_window(dummy_window(1, 1));
        let info = session.info();
        assert_eq!(info.name, "my-session");
        assert_eq!(info.window_count, 2);
        assert_eq!(info.pane_count, 2);
        assert!(!info.attached);
    }

    #[test]
    fn shell_name_from_path_extracts_basename() {
        assert_eq!(shell_name_from_path("/bin/bash"), "bash");
        assert_eq!(shell_name_from_path("/usr/bin/zsh"), "zsh");
        assert_eq!(shell_name_from_path("bash"), "bash");
    }
}
