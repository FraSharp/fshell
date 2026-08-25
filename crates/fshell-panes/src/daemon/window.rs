// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! A window within a session, containing its own panes and layout.
//!
//! Each window is independent — it has its own BSP tree, focus state,
//! and UI state (help, rename). PTY actors for each pane run continuously,
//! even when the window is not active.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, mpsc};

use crate::app::focus::FocusController;
use crate::grid::Grid;
use crate::layout::bsp::{BspLayout, Split};
use crate::proto::SCROLLBACK_LIMIT;
use crate::pty::async_pty::AsyncPty;
use crate::pty::get_default_shell;
use crate::pty::grid_manager::{GridManager, PtyCommand};
use crate::pty::pty_actor::PtyActor;

use super::DaemonEvent;
use super::session::shell_name_from_path;

/// State for a single terminal pane within a window.
pub struct PaneState {
    pub grid: Arc<RwLock<Grid>>,
    pub pty_tx: mpsc::Sender<PtyCommand>,
    /// User-settable label. `None` means use the default shell name.
    pub label: Option<String>,
    /// Basename of the shell running in this pane (e.g. "bash").
    pub shell_name: String,
}

/// Target for rename operation (Pane vs Window).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameTarget {
    Pane,
    Window,
}

/// Active rename state for UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameState {
    pub target: RenameTarget,
    pub buffer: String,
}

/// A window containing panes and a BSP layout.
pub struct Window {
    /// Unique window ID (session-scoped, monotonically increasing).
    pub id: u32,
    /// Display name (e.g. "bash", "vim", "logs").
    pub name: String,
    /// Panes in this window, keyed by globally unique pane ID.
    pub panes: HashMap<u32, PaneState>,
    /// BSP layout tree for this window's panes.
    pub bsp: BspLayout,
    /// Focus tracking for this window's panes.
    pub focus: FocusController,
    /// Whether the help overlay is showing.
    pub show_help: bool,
    /// Scroll offset for the help overlay.
    pub help_scroll: u16,
    /// Active rename state (Pane or Window).
    pub rename_state: Option<RenameState>,
}

impl Window {
    /// Create a new window with the given ID and initial pane ID.
    pub fn new(id: u32, initial_pane_id: u32) -> Self {
        Self {
            id,
            name: String::new(),
            panes: HashMap::new(),
            bsp: BspLayout::with_root_id(initial_pane_id),
            focus: FocusController::new(initial_pane_id),
            show_help: false,
            help_scroll: 0,
            rename_state: None,
        }
    }

    /// Returns the buffer string if rename is active.
    pub fn rename_buffer(&self) -> Option<&str> {
        self.rename_state.as_ref().map(|s| s.buffer.as_str())
    }

    /// Register a pane with its grid and PTY channel.
    pub fn add_pane(
        &mut self,
        id: u32,
        grid: Arc<RwLock<Grid>>,
        pty_tx: mpsc::Sender<PtyCommand>,
        shell_name: String,
    ) {
        self.panes.insert(
            id,
            PaneState {
                grid,
                pty_tx,
                label: None,
                shell_name,
            },
        );
        self.focus.add_pane(id);
    }

    /// Remove a pane by ID. Returns true if the pane existed.
    pub fn remove_pane(&mut self, id: u32) -> bool {
        if self.panes.remove(&id).is_some() {
            self.bsp.remove(id);
            self.focus.remove_pane(id);
            true
        } else {
            false
        }
    }

    /// Check if the window has any live panes.
    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }

    /// Resize all panes to fit the given area.
    pub fn resize_panes(&self, area: ratatui::layout::Rect) {
        let layout = self.bsp.compute_layout(area);
        for (pane_id, rect) in layout {
            let inner_width = rect.width.saturating_sub(2).max(1);
            let inner_height = rect.height.saturating_sub(2).max(1);
            if let Some(pane) = self.panes.get(&pane_id) {
                let _ = pane
                    .pty_tx
                    .try_send(PtyCommand::Resize(inner_width, inner_height));
            }
        }
    }

    /// Compute the BSP layout for the given area.
    pub fn compute_layout(&self, area: ratatui::layout::Rect) -> Vec<(u32, ratatui::layout::Rect)> {
        self.bsp.compute_layout(area)
    }

    /// Split the focused pane, spawning a new shell in the split direction.
    pub fn spawn_pane(
        &mut self,
        direction: Split,
        session_name: String,
        daemon_event_tx: Option<mpsc::Sender<DaemonEvent>>,
        dirty: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let focused = self.focus.focused_pane;
        let new_id = self.bsp.split(focused, direction, 0.5);

        // Compute layout to find the new pane's rect.
        // Use a large dummy area — resize_to_terminal corrects dimensions after.
        let dummy_area = ratatui::layout::Rect::new(0, 0, 200, 100);
        let layout = self.bsp.compute_layout(dummy_area);
        let new_rect = layout
            .iter()
            .find(|(id, _)| *id == new_id)
            .map(|(_, r)| *r)
            .unwrap_or(ratatui::layout::Rect::new(0, 0, 80, 24));
        let init_cols = new_rect.width.saturating_sub(2).max(1);
        let init_rows = new_rect.height.saturating_sub(2).max(1);

        // Spawn the full PTY pipeline.
        let (pty_tx, pty_rx) = mpsc::channel(256);
        let (grid_tx, grid_rx) = mpsc::channel(256);

        // Spawn GridManager actor.
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
        if let Ok(pty) = AsyncPty::spawn(&shell, init_cols, init_rows) {
            let actor = PtyActor::new(pty, grid_tx, pty_rx, new_id, None);
            let pane_session_name = session_name;
            let event_tx = daemon_event_tx;
            let actor_id = new_id;
            tokio::spawn(async move {
                actor.run().await;
                if let Some(tx) = event_tx {
                    let _ = tx
                        .send(DaemonEvent::PaneExited {
                            session_name: pane_session_name,
                            pane_id: actor_id,
                        })
                        .await;
                }
            });
        }

        let shell_name = shell_name_from_path(&shell);
        self.add_pane(new_id, grid_ref, pty_tx, shell_name);
        self.focus.set_focused_pane(new_id);
    }

    /// Close the focused pane, updating layout and focus.
    /// Sends shutdown to the PTY actor and removes the pane.
    /// Returns true if a pane was closed.
    pub fn close_focused_pane(&mut self) -> bool {
        let id = self.focus.focused_pane;
        if let Some(pane) = self.panes.get(&id) {
            let _ = pane.pty_tx.try_send(PtyCommand::Shutdown);
        }
        self.remove_pane(id)
    }

    /// Resize all panes to fit the given terminal dimensions.
    /// The last row is reserved for the status bar.
    pub fn resize_to_terminal(&self, cols: u16, rows: u16) {
        let pane_rows = rows.saturating_sub(1);
        let area = ratatui::layout::Rect::new(0, 0, cols, pane_rows);
        self.resize_panes(area);
    }
}
