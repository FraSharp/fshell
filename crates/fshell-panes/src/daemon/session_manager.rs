// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Session lifecycle management.
//!
//! The `SessionManager` owns all active sessions and provides methods
//! for creating, attaching, detaching, and destroying sessions.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, mpsc};

use crate::daemon::session::shell_name_from_path;
use crate::daemon::window::Window;
use crate::proto::SCROLLBACK_LIMIT;
use crate::proto::SessionInfo;
use crate::pty::async_pty::AsyncPty;
use crate::pty::get_default_shell;
use crate::pty::grid_manager::{GridManager, PtyCommand};
use crate::pty::pty_actor::PtyActor;

use super::session::Session;

/// Manages all active sessions in the daemon.
pub struct SessionManager {
    pub sessions: HashMap<String, Arc<RwLock<Session>>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    /// Create a new empty session manager.
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// List all active sessions.
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let mut infos = Vec::with_capacity(self.sessions.len());
        for session in self.sessions.values() {
            let s = session.read().await;
            infos.push(s.info());
        }
        infos
    }

    /// Get a session by name.
    pub async fn get_session(&self, name: &str) -> Option<Arc<RwLock<Session>>> {
        self.sessions.get(name).cloned()
    }

    /// Create a new session with a single window containing one shell pane.
    pub async fn create_session(
        &mut self,
        name: String,
        cols: u16,
        rows: u16,
        daemon_event_tx: mpsc::Sender<super::DaemonEvent>,
    ) -> Result<Arc<RwLock<Session>>, Box<dyn std::error::Error + Send + Sync>> {
        if self.sessions.contains_key(&name) {
            return Err(format!("session '{}' already exists", name).into());
        }

        let mut session = Session::new(name.clone(), 0);
        session.terminal_size = (cols, rows);
        session.set_daemon_event_tx(daemon_event_tx.clone());

        // Create the initial window.
        let pane_id = session.next_pane_id;
        session.next_pane_id += 1;

        let mut window = Window::new(0, pane_id);

        // Spawn PTY pipeline for the initial pane.
        let pane_rows = rows.saturating_sub(1);
        let init_cols = cols.saturating_sub(2).max(1);
        let init_rows = pane_rows.saturating_sub(2).max(1);

        let (pty_tx, pty_rx) = mpsc::channel(256);
        let (grid_tx, grid_rx) = mpsc::channel(256);

        // Spawn GridManager actor.
        let dirty = session.dirty.clone();
        let (mut manager, grid_ref) = GridManager::new(
            init_cols as usize,
            init_rows as usize,
            SCROLLBACK_LIMIT,
            grid_rx,
            dirty,
        );
        manager.set_pty_tx(pty_tx.clone());
        tokio::spawn(async move { manager.run().await });

        // Spawn AsyncPty + PtyActor.
        let shell = get_default_shell();
        let actor = if let Ok(pty) = AsyncPty::spawn(&shell, init_cols, init_rows) {
            let actor = PtyActor::new(pty, grid_tx, pty_rx, pane_id, None);
            Some(actor)
        } else {
            None
        };

        let shell_name = shell_name_from_path(&shell);
        window.name = shell_name.clone();
        window.add_pane(pane_id, grid_ref, pty_tx, shell_name);
        window.resize_to_terminal(cols, rows);

        // Spawn the actor after window is configured.
        if let Some(actor) = actor {
            let session_name = name.clone();
            let event_tx = daemon_event_tx;
            let actor_id = pane_id;
            tokio::spawn(async move {
                actor.run().await;
                let _ = event_tx
                    .send(crate::daemon::DaemonEvent::PaneExited {
                        session_name,
                        pane_id: actor_id,
                    })
                    .await;
            });
        }

        session.windows.push(window);
        session.active_window = 0;

        let session_handle = Arc::new(RwLock::new(session));
        self.sessions.insert(name, session_handle.clone());

        Ok(session_handle)
    }

    /// Attach a client to a session. Returns the session handle.
    pub async fn attach_session(
        &self,
        name: &str,
        client_tx: mpsc::Sender<Vec<u8>>,
    ) -> Result<Arc<RwLock<Session>>, Box<dyn std::error::Error + Send + Sync>> {
        let session = self
            .sessions
            .get(name)
            .ok_or_else(|| format!("session '{}' not found", name))?
            .clone();

        // Update the session's attached client, then drop the write guard.
        {
            let mut s = session.write().await;
            s.attached_client = Some(client_tx);
        }

        Ok(session)
    }

    /// Detach the current client from a session.
    pub async fn detach_session(
        &self,
        name: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let session = self
            .sessions
            .get(name)
            .ok_or_else(|| format!("session '{}' not found", name))?
            .clone();

        // Update the session's attached client, then drop the write guard.
        {
            let mut s = session.write().await;
            s.attached_client = None;
        }

        Ok(())
    }

    /// Kill a session and all its PTY actors.
    pub async fn kill_session(
        &mut self,
        name: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let session = self
            .sessions
            .remove(name)
            .ok_or_else(|| format!("session '{}' not found", name))?;

        let s = session.write().await;

        // Send shutdown to all PTY actors across all windows.
        for window in &s.windows {
            for pane in window.panes.values() {
                let _ = pane.pty_tx.send(PtyCommand::Shutdown).await;
            }
        }

        // Notify attached client to exit.
        if let Some(ref tx) = s.attached_client {
            let _ = tx.send(vec![]).await; // Empty = exit signal
        }

        Ok(())
    }

    /// Find which session a pane belongs to.
    pub async fn find_session_for_pane(&self, pane_id: u32) -> Option<String> {
        for (name, session) in &self.sessions {
            let s = session.read().await;
            for window in &s.windows {
                if window.panes.contains_key(&pane_id) {
                    return Some(name.clone());
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_manager_create_and_list() {
        let mut mgr = SessionManager::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);

        let _ = mgr
            .create_session("test-1".to_string(), 80, 24, tx.clone())
            .await;
        let _ = mgr.create_session("test-2".to_string(), 120, 40, tx).await;

        let list = mgr.list_sessions().await;
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|s| s.name == "test-1"));
        assert!(list.iter().any(|s| s.name == "test-2"));
    }

    #[tokio::test]
    async fn session_manager_kill_session() {
        let mut mgr = SessionManager::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);

        let _ = mgr.create_session("to-kill".to_string(), 80, 24, tx).await;
        assert!(mgr.kill_session("to-kill").await.is_ok());

        let list = mgr.list_sessions().await;
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn session_manager_detach_nonexistent() {
        let mgr = SessionManager::new();

        let result = mgr.detach_session("nope").await;
        assert!(result.is_err());
    }
}
