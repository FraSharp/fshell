// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Focus controller for tracking active pane.

/// Manages which pane is focused and prefix key state.
pub struct FocusController {
    pub focused_pane: u32,
    pub all_panes: Vec<u32>,
    pub prefix_active: bool,
}

impl FocusController {
    pub fn new(initial_pane: u32) -> Self {
        Self {
            focused_pane: initial_pane,
            all_panes: vec![initial_pane],
            prefix_active: false,
        }
    }

    /// Create an empty focus controller with no panes.
    pub fn empty() -> Self {
        Self {
            focused_pane: 0,
            all_panes: Vec::new(),
            prefix_active: false,
        }
    }

    pub fn set_focused_pane(&mut self, id: u32) {
        self.focused_pane = id;
    }

    pub fn add_pane(&mut self, id: u32) {
        self.all_panes.push(id);
    }

    pub fn remove_pane(&mut self, id: u32) {
        self.all_panes.retain(|&p| p != id);
        if self.focused_pane == id {
            self.focused_pane = self.all_panes.first().copied().unwrap_or(0);
        }
    }

    pub fn focus_up(&mut self) {
        let pos = self.all_panes.iter().position(|&p| p == self.focused_pane);
        if let Some(new_pos) = pos.and_then(|p| p.checked_sub(1)) {
            self.focused_pane = self.all_panes[new_pos];
        }
    }

    pub fn focus_down(&mut self) {
        let pos = self.all_panes.iter().position(|&p| p == self.focused_pane);
        if let Some(new_pos) = pos.map(|p| p + 1).filter(|&p| p < self.all_panes.len()) {
            self.focused_pane = self.all_panes[new_pos];
        }
    }
}
