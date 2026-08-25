// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::history::{HistoryEntry, delete_history_entry, query_history};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum HistoryFilterMode {
    Global,
    Host,
    Cwd,
    Session,
}

impl HistoryFilterMode {
    pub fn next(self) -> Self {
        match self {
            HistoryFilterMode::Global => HistoryFilterMode::Host,
            HistoryFilterMode::Host => HistoryFilterMode::Cwd,
            HistoryFilterMode::Cwd => HistoryFilterMode::Session,
            HistoryFilterMode::Session => HistoryFilterMode::Global,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            HistoryFilterMode::Global => "GLOBAL",
            HistoryFilterMode::Host => "HOST",
            HistoryFilterMode::Cwd => "DIRECTORY",
            HistoryFilterMode::Session => "SESSION",
        }
    }
}

pub struct HistoryManager {
    pub active: bool,
    pub explorer_active: bool,
    pub query: String,
    pub filter_mode: HistoryFilterMode,
    pub selected_idx: usize,
    pub scroll_offset: usize,
    pub results: Vec<HistoryEntry>,
    // Alt+R stack: Cache of commands aborted via Ctrl-C
    pub aborted_commands: Vec<String>,
    pub aborted_active: bool,
}

impl Default for HistoryManager {
    fn default() -> Self {
        Self {
            active: false,
            explorer_active: false,
            query: String::new(),
            filter_mode: HistoryFilterMode::Global,
            selected_idx: 0,
            scroll_offset: 0,
            results: Vec::new(),
            aborted_commands: Vec::new(),
            aborted_active: false,
        }
    }
}

impl HistoryManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_aborted(&mut self, cmd: &str) {
        let trimmed = cmd.trim();
        if !trimmed.is_empty() {
            // Remove duplication if it was already on top of stack
            self.aborted_commands.retain(|c| c != trimmed);
            self.aborted_commands.push(trimmed.to_string());
        }
    }

    pub fn update_results(&mut self, current_cwd: &str, current_host: &str, current_session: &str) {
        if self.aborted_active {
            return;
        }

        let search_query = if self.query.trim().is_empty() {
            None
        } else {
            Some(self.query.trim())
        };

        // Query database history (capped at 100 entries for faster TUI listing)
        self.results = query_history(
            Some(100),
            search_query,
            if self.filter_mode == HistoryFilterMode::Cwd {
                Some(current_cwd)
            } else {
                None
            },
            if self.filter_mode == HistoryFilterMode::Session {
                Some(current_session)
            } else {
                None
            },
            if self.filter_mode == HistoryFilterMode::Host {
                Some(current_host)
            } else {
                None
            },
            None,
        )
        .unwrap_or_default();

        if self.results.is_empty() {
            self.selected_idx = 0;
            self.scroll_offset = 0;
        } else if self.selected_idx >= self.results.len() {
            self.selected_idx = self.results.len() - 1;
        }
    }

    /// Returns the count of items in the currently active view.
    /// For aborted commands, this is the count of aborted commands.
    /// For normal history, this is the count of results.
    pub fn active_count(&self) -> usize {
        if self.aborted_active {
            self.aborted_commands.len()
        } else {
            self.results.len()
        }
    }

    pub fn select_next(&mut self) {
        let len = self.active_count();
        if len == 0 {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % len;
    }

    pub fn select_prev(&mut self) {
        let len = self.active_count();
        if len == 0 {
            return;
        }
        if self.selected_idx == 0 {
            self.selected_idx = len - 1;
        } else {
            self.selected_idx -= 1;
        }
    }

    pub fn reset(&mut self) {
        self.active = false;
        self.explorer_active = false;
        self.aborted_active = false;
        self.query.clear();
        self.selected_idx = 0;
        self.scroll_offset = 0;
        self.results.clear();
    }

    pub fn get_selected(&self) -> Option<String> {
        if self.aborted_active {
            if self.selected_idx < self.aborted_commands.len() {
                // Return in reverse order (newest first)
                let rev_idx = self.aborted_commands.len() - 1 - self.selected_idx;
                return Some(self.aborted_commands[rev_idx].clone());
            }
        } else if self.selected_idx < self.results.len() {
            return Some(self.results[self.selected_idx].command.clone());
        }
        None
    }

    /// Adjust `scroll_offset` to ensure the currently selected item is visible.
    /// `max_h` is the maximum number of visible rows.
    /// `selected_idx` is always a visual index: 0 = first displayed item (newest
    /// for aborted commands, first DB result for normal history).
    /// `scroll_offset` is the index into the displayed list (reversed for aborted).
    /// This ensures the invariant: scroll_offset <= selected_idx < scroll_offset + max_h.
    pub fn adjust_scroll(&mut self, max_h: usize) {
        if max_h == 0 {
            return;
        }
        let total = self.active_count();
        if total == 0 {
            self.scroll_offset = 0;
            return;
        }
        if self.selected_idx < self.scroll_offset {
            self.scroll_offset = self.selected_idx;
        } else if self.selected_idx >= self.scroll_offset + max_h {
            let new_offset = self.selected_idx + 1 - max_h;
            self.scroll_offset = new_offset.min(total.saturating_sub(max_h));
        }
    }

    pub fn delete_selected(&mut self) {
        if self.aborted_active || self.selected_idx >= self.results.len() {
            return;
        }
        let id = self.results[self.selected_idx].id;
        let _ = delete_history_entry(id);
        self.results.remove(self.selected_idx);
        if self.selected_idx > 0 && self.selected_idx >= self.results.len() {
            self.selected_idx = self.results.len().saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_explorer_state() {
        let mut mgr = HistoryManager::new();
        assert!(!mgr.explorer_active);

        mgr.explorer_active = true;
        assert!(mgr.explorer_active);

        mgr.reset();
        assert!(!mgr.explorer_active);
    }
}
