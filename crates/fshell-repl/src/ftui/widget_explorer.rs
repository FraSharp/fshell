// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_engine::Env;
use fshell_engine::keybindings::{KeyMapMode, all_widgets};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetExplorerItem {
    pub name: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    pub bound_chord: String,
}

#[derive(Default)]
pub struct WidgetExplorerManager {
    pub active: bool,
    pub query: String,
    pub selected_idx: usize,
    pub scroll_offset: usize,
    pub items: Vec<WidgetExplorerItem>,
}

impl WidgetExplorerManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, env: &Env) {
        self.active = true;
        self.query.clear();
        self.selected_idx = 0;
        self.scroll_offset = 0;
        self.update_filter(env);
    }

    pub fn close(&mut self) {
        self.active = false;
        self.query.clear();
        self.selected_idx = 0;
        self.scroll_offset = 0;
    }

    pub fn update_filter(&mut self, env: &Env) {
        let active_mode = {
            let reg = env.keybindings.read();
            reg.active_mode
        };

        let reg = Some(env.keybindings.read());
        let all = all_widgets();
        let q = self.query.trim().to_lowercase();

        let mut filtered = Vec::new();
        for w in all {
            let matches_query = if q.is_empty() {
                true
            } else {
                w.name.to_lowercase().contains(&q)
                    || w.category.to_lowercase().contains(&q)
                    || w.description.to_lowercase().contains(&q)
            };

            if matches_query {
                let bound_str = if let Some(ref r) = reg {
                    let chords = r.find_chords_for_widget(active_mode, w.name);
                    if !chords.is_empty() {
                        chords.join(", ")
                    } else {
                        match active_mode {
                            KeyMapMode::Emacs => w.default_chord_emacs.unwrap_or("-").to_string(),
                            KeyMapMode::ViNormal | KeyMapMode::ViInsert | KeyMapMode::ViVisual => {
                                w.default_chord_vi.unwrap_or("-").to_string()
                            }
                        }
                    }
                } else {
                    w.default_chord_emacs.unwrap_or("-").to_string()
                };

                filtered.push(WidgetExplorerItem {
                    name: w.name,
                    category: w.category,
                    description: w.description,
                    bound_chord: bound_str,
                });
            }
        }

        self.items = filtered;
        if self.items.is_empty() {
            self.selected_idx = 0;
            self.scroll_offset = 0;
        } else if self.selected_idx >= self.items.len() {
            self.selected_idx = self.items.len() - 1;
        }
    }

    pub fn select_next(&mut self) {
        if !self.items.is_empty() && self.selected_idx + 1 < self.items.len() {
            self.selected_idx += 1;
        }
    }

    pub fn select_prev(&mut self) {
        if self.selected_idx > 0 {
            self.selected_idx -= 1;
        }
    }

    pub fn page_down(&mut self, page_size: usize) {
        if !self.items.is_empty() {
            self.selected_idx = (self.selected_idx + page_size).min(self.items.len() - 1);
        }
    }

    pub fn page_up(&mut self, page_size: usize) {
        self.selected_idx = self.selected_idx.saturating_sub(page_size);
    }

    pub fn get_selected_widget(&self) -> Option<&'static str> {
        self.items.get(self.selected_idx).map(|item| item.name)
    }
}
