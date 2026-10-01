// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Modern, flicker-free interactive fuzzy pickers built on Ratatui.

use crate::fuzzy::{FuzzyKind, PreparedQuery, fuzzy_score_prepared};
use crate::tui::components::{KeyHint, ScrollState, SearchBarState, StatusFooter, modal_dialog};
use crate::tui::theme;
use fshell_core::lock::Mutex;
use fshell_core::theme::Theme;
use fshell_terminal::input::{InputEvent, Key, KeyAction, Modifiers, UnixEventStream};
use fshell_terminal::runner::{AppFlow, ShellTuiApp, run_tui};
use fshell_terminal::session::{
    TerminalDevice, TerminalMode, TerminalSession, TerminalSessionOptions,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, Paragraph};
use std::path::PathBuf;
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

struct GitPickerCachedData {
    pwd: String,
    cached_at: Instant,
    branches: Vec<(String, String, i64)>,
    commits: Vec<(String, String, i64)>,
}

static GIT_PICKER_CACHE: Mutex<Option<GitPickerCachedData>> = Mutex::new(None);
const GIT_PICKER_TTL: std::time::Duration = std::time::Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct PickerItem {
    pub value: String,
    pub display: String,
}

#[derive(Debug)]
pub enum PickerModal {
    None,
    ConfirmDelete {
        session_name: String,
        item_value: String,
    },
    Rename {
        session_name: String,
        item_value: String,
        input: SearchBarState,
    },
}

/// Interactive TUI application state for the picker.
pub struct PickerApp<'a> {
    pub prompt: &'a str,
    pub items: &'a mut Vec<PickerItem>,
    pub theme: &'a Theme,
    pub search_bar: SearchBarState,
    pub selected_idx: usize,
    pub scroll_state: ScrollState,
    pub modal: PickerModal,
}

impl<'a> PickerApp<'a> {
    pub fn new(prompt: &'a str, items: &'a mut Vec<PickerItem>, theme: &'a Theme) -> Self {
        Self {
            prompt,
            items,
            theme,
            search_bar: SearchBarState::new(),
            selected_idx: 0,
            scroll_state: ScrollState::new(),
            modal: PickerModal::None,
        }
    }
}

impl<'a> ShellTuiApp for PickerApp<'a> {
    type Message = InputEvent;
    type Output = Option<String>;

    fn handle_message(&mut self, msg: Self::Message) -> AppFlow<Self::Output> {
        let key = match msg {
            InputEvent::Key(key) => key,
            _ => return AppFlow::Ignore,
        };

        if key.action == KeyAction::Release {
            return AppFlow::Ignore;
        }

        let is_sessions = self.prompt == "sessions:";

        // Modal input routing
        match &mut self.modal {
            PickerModal::ConfirmDelete { item_value, .. } => match key.key {
                Key::Character('y') | Key::Character('Y') | Key::Enter => {
                    let path = PathBuf::from(&item_value);
                    let json_path = path.clone();
                    let log_path = path.with_extension("log");
                    let _ = std::fs::remove_file(json_path);
                    let _ = std::fs::remove_file(log_path);

                    if let Some(pos) = self.items.iter().position(|it| it.value == *item_value) {
                        self.items.remove(pos);
                    }
                    self.modal = PickerModal::None;
                    return AppFlow::Continue;
                }
                Key::Character('n') | Key::Character('N') | Key::Escape => {
                    self.modal = PickerModal::None;
                    return AppFlow::Continue;
                }
                Key::Character('c') if key.modifiers.contains(Modifiers::CONTROL) => {
                    self.modal = PickerModal::None;
                    return AppFlow::Continue;
                }
                _ => return AppFlow::Ignore,
            },
            PickerModal::Rename {
                item_value, input, ..
            } => match key.key {
                Key::Enter => {
                    let new_name = input.query.trim().to_string();
                    if !new_name.is_empty() {
                        let path = PathBuf::from(&item_value);
                        if let Ok(content) = std::fs::read_to_string(&path)
                            && let Ok(mut state) = serde_json::from_str::<
                                fshell_engine::handoff::HandoffState,
                            >(&content)
                        {
                            state.vars.insert(
                                "FSH_SESSION_NAME".to_string(),
                                fshell_core::Val::String(new_name.clone()),
                            );
                            if let Ok(serialized) = serde_json::to_string_pretty(&state) {
                                let _ = std::fs::write(&path, &serialized);
                            }

                            let mtime = std::fs::metadata(&path)
                                .and_then(|m| m.modified())
                                .unwrap_or_else(|_| std::time::SystemTime::now());
                            let age = std::time::SystemTime::now()
                                .duration_since(mtime)
                                .unwrap_or_default();
                            let age_str = if age.as_secs() < 60 {
                                "just now".to_string()
                            } else if age.as_secs() < 3600 {
                                format!("{}m ago", age.as_secs() / 60)
                            } else if age.as_secs() < 86400 {
                                format!("{}h ago", age.as_secs() / 3600)
                            } else {
                                format!("{}d ago", age.as_secs() / 86400)
                            };

                            let display = format!(
                                "Session {} [{}] (cwd: {}, active: {})",
                                state.session_id, new_name, state.cwd, age_str
                            );

                            if let Some(pos) =
                                self.items.iter().position(|it| it.value == *item_value)
                            {
                                self.items[pos].display = display;
                            }
                        }
                    }
                    self.modal = PickerModal::None;
                    return AppFlow::Continue;
                }
                Key::Escape => {
                    self.modal = PickerModal::None;
                    return AppFlow::Continue;
                }
                Key::Character('c') if key.modifiers.contains(Modifiers::CONTROL) => {
                    self.modal = PickerModal::None;
                    return AppFlow::Continue;
                }
                _ => {
                    input.handle_key(&key);
                    return AppFlow::Continue;
                }
            },
            PickerModal::None => {}
        }

        // Normal picker input
        let filtered = fuzzy_filter(self.items, &self.search_bar.query);
        let filtered_len = filtered.len();

        match key.key {
            Key::Escape => AppFlow::Break(None),
            Key::Character('c') if key.modifiers.contains(Modifiers::CONTROL) => {
                AppFlow::Break(None)
            }
            Key::Character('g') if key.modifiers.contains(Modifiers::CONTROL) => {
                AppFlow::Break(None)
            }
            Key::Enter => {
                if !filtered.is_empty() {
                    let idx = self.selected_idx.min(filtered.len() - 1);
                    AppFlow::Break(Some(filtered[idx].value.clone()))
                } else {
                    AppFlow::Break(None)
                }
            }
            Key::Up | Key::Character('p') if key.modifiers.contains(Modifiers::CONTROL) => {
                self.selected_idx = self.selected_idx.saturating_sub(1);
                AppFlow::Continue
            }
            Key::Up => {
                self.selected_idx = self.selected_idx.saturating_sub(1);
                AppFlow::Continue
            }
            Key::Down | Key::Character('n') if key.modifiers.contains(Modifiers::CONTROL) => {
                if filtered_len > 0 && self.selected_idx < filtered_len.saturating_sub(1) {
                    self.selected_idx += 1;
                }
                AppFlow::Continue
            }
            Key::Down => {
                if filtered_len > 0 && self.selected_idx < filtered_len.saturating_sub(1) {
                    self.selected_idx += 1;
                }
                AppFlow::Continue
            }
            Key::PageUp => {
                self.selected_idx = self.selected_idx.saturating_sub(10);
                AppFlow::Continue
            }
            Key::PageDown => {
                if filtered_len > 0 {
                    self.selected_idx =
                        (self.selected_idx + 10).min(filtered_len.saturating_sub(1));
                }
                AppFlow::Continue
            }
            Key::Character('d')
                if is_sessions && self.search_bar.query.is_empty() && filtered_len > 0 =>
            {
                let idx = self.selected_idx.min(filtered_len - 1);
                let item_value = filtered[idx].value.clone();
                if item_value != "new" {
                    let path = PathBuf::from(&item_value);
                    let filename = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.trim_end_matches(".json"))
                        .unwrap_or("unknown")
                        .to_string();

                    self.modal = PickerModal::ConfirmDelete {
                        session_name: filename,
                        item_value,
                    };
                }
                AppFlow::Continue
            }
            Key::Character('r')
                if is_sessions && self.search_bar.query.is_empty() && filtered_len > 0 =>
            {
                let idx = self.selected_idx.min(filtered_len - 1);
                let item_value = filtered[idx].value.clone();
                if item_value != "new" {
                    let path = PathBuf::from(&item_value);
                    let filename = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.trim_end_matches(".json"))
                        .unwrap_or("unknown")
                        .to_string();

                    self.modal = PickerModal::Rename {
                        session_name: filename,
                        item_value,
                        input: SearchBarState::new(),
                    };
                }
                AppFlow::Continue
            }
            _ => {
                if self.search_bar.handle_key(&key) {
                    self.selected_idx = 0;
                }
                AppFlow::Continue
            }
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect) {
        if area.width < 10 || area.height < 4 {
            return;
        }

        let filtered = fuzzy_filter(self.items, &self.search_bar.query);

        if filtered.is_empty() {
            self.selected_idx = 0;
        } else if self.selected_idx >= filtered.len() {
            self.selected_idx = filtered.len().saturating_sub(1);
        }

        let prompt_name = self.prompt.trim_end_matches(':');
        let is_sessions = self.prompt == "sessions:";

        // Main vertical layout: Search box (3 lines), Items list (flex), Footer (1 line)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(2),
                Constraint::Length(1),
            ])
            .split(area);

        // 1. Search Box
        let match_count_info = format!(" {}/{} ", filtered.len(), self.items.len());
        let search_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme::border_focused_style(self.theme))
            .title(format!(" Select {prompt_name} "))
            .title_style(theme::title_style(self.theme));

        let search_inner = search_block.inner(chunks[0]);
        frame.render_widget(search_block, chunks[0]);

        // Render right-aligned count tag inside search header
        let count_width = match_count_info.width() as u16;
        if chunks[0].width > count_width + 4 {
            let count_area = Rect::new(
                chunks[0].x + chunks[0].width.saturating_sub(count_width + 2),
                chunks[0].y,
                count_width,
                1,
            );
            frame.render_widget(
                Paragraph::new(Span::styled(
                    match_count_info,
                    theme::muted_style(self.theme),
                )),
                count_area,
            );
        }

        self.search_bar.render(
            search_inner,
            frame.buffer_mut(),
            self.theme,
            "> ",
            "Type to filter...",
            matches!(self.modal, PickerModal::None),
        );

        // 2. Results List
        let list_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme::border_style(self.theme));
        let list_inner = list_block.inner(chunks[1]);
        frame.render_widget(list_block, chunks[1]);

        let visible_height = list_inner.height as usize;
        self.scroll_state.update(filtered.len(), visible_height);
        self.scroll_state.ensure_visible(self.selected_idx);

        let list_items: Vec<ListItem> = filtered
            .iter()
            .skip(self.scroll_state.offset)
            .take(visible_height)
            .enumerate()
            .map(|(offset_i, item)| {
                let item_idx = self.scroll_state.offset + offset_i;
                let is_selected = item_idx == self.selected_idx;

                let mut spans = Vec::new();
                if is_selected {
                    spans.push(Span::styled("❯ ", theme::title_style(self.theme)));
                } else {
                    spans.push(Span::raw("  "));
                }

                // Render item text with match highlight
                let query_lower = self.search_bar.query.to_lowercase();
                if !query_lower.is_empty() {
                    let item_text = &item.display;
                    let mut last_idx = 0;
                    let lower = item_text.to_lowercase();
                    if let Some(found_idx) = lower.find(&query_lower) {
                        if found_idx > 0 {
                            spans.push(Span::raw(&item_text[..found_idx]));
                        }
                        let match_end = found_idx + query_lower.len();
                        spans.push(Span::styled(
                            &item_text[found_idx..match_end],
                            theme::match_highlight_style(self.theme),
                        ));
                        if match_end < item_text.len() {
                            spans.push(Span::raw(&item_text[match_end..]));
                        }
                        last_idx = item_text.len();
                    }
                    if last_idx == 0 {
                        spans.push(Span::raw(item_text));
                    }
                } else {
                    spans.push(Span::raw(&item.display));
                }

                let row_style = if is_selected {
                    theme::selected_style(self.theme)
                } else {
                    ratatui::style::Style::default()
                };

                ListItem::new(Line::from(spans)).style(row_style)
            })
            .collect();

        frame.render_widget(List::new(list_items), list_inner);
        self.scroll_state
            .render_scrollbar(list_inner, frame.buffer_mut(), self.theme);

        // 3. Status Footer
        let hints: &[KeyHint] = if is_sessions {
            &[
                KeyHint::new("Enter", "Select"),
                KeyHint::new("↑/↓", "Navigate"),
                KeyHint::new("d", "Delete"),
                KeyHint::new("r", "Rename"),
                KeyHint::new("Esc", "Cancel"),
            ]
        } else {
            &[
                KeyHint::new("Enter", "Select"),
                KeyHint::new("↑/↓", "Navigate"),
                KeyHint::new("Esc", "Cancel"),
            ]
        };

        StatusFooter::new(self.theme, hints).render(chunks[2], frame.buffer_mut());

        // 4. Modals (if active)
        match &mut self.modal {
            PickerModal::None => {}
            PickerModal::ConfirmDelete { session_name, .. } => {
                let dialog_area = modal_dialog::centered_fixed(50, 7, area);
                let inner = modal_dialog::render_modal_frame(
                    dialog_area,
                    frame.buffer_mut(),
                    self.theme,
                    "Confirm Deletion",
                );

                let msg = format!("Delete session '{session_name}'?");
                let lines = vec![
                    Line::from(Span::styled(msg, theme::status_warn_style(self.theme))),
                    Line::raw(""),
                    Line::from(vec![
                        Span::styled("[y/Enter] ", theme::key_hint_key_style(self.theme)),
                        Span::raw("Confirm   "),
                        Span::styled("[n/Esc] ", theme::key_hint_key_style(self.theme)),
                        Span::raw("Cancel"),
                    ]),
                ];
                frame.render_widget(Paragraph::new(lines), inner);
            }
            PickerModal::Rename {
                session_name,
                input,
                ..
            } => {
                let dialog_area = modal_dialog::centered_fixed(60, 7, area);
                let inner = modal_dialog::render_modal_frame(
                    dialog_area,
                    frame.buffer_mut(),
                    self.theme,
                    "Rename Session",
                );

                let msg = format!("New name for '{session_name}':");
                let rename_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Length(1),
                    ])
                    .split(inner);

                frame.render_widget(
                    Paragraph::new(Span::styled(msg, theme::title_style(self.theme))),
                    rename_chunks[0],
                );

                input.render(
                    rename_chunks[1],
                    frame.buffer_mut(),
                    self.theme,
                    "Name: ",
                    "Enter new session name",
                    true,
                );

                let hint_line = Line::from(vec![
                    Span::styled("[Enter] ", theme::key_hint_key_style(self.theme)),
                    Span::raw("Save   "),
                    Span::styled("[Esc] ", theme::key_hint_key_style(self.theme)),
                    Span::raw("Cancel"),
                ]);
                frame.render_widget(Paragraph::new(hint_line), rename_chunks[2]);
            }
        }
    }
}

pub struct Picker {
    prompt: String,
    items: Vec<PickerItem>,
    theme: Theme,
    mode: TerminalMode,
}

impl Picker {
    pub fn new(prompt: &str, items: Vec<PickerItem>) -> Self {
        Self {
            prompt: prompt.to_string(),
            items,
            theme: Theme::default_theme(),
            mode: TerminalMode::Inline { height: 10 },
        }
    }

    pub fn with_theme(prompt: &str, items: Vec<PickerItem>, theme: Theme) -> Self {
        Self {
            prompt: prompt.to_string(),
            items,
            theme,
            mode: TerminalMode::Inline { height: 10 },
        }
    }

    pub fn with_mode(mut self, mode: TerminalMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn fullscreen(mut self) -> Self {
        self.mode = TerminalMode::Fullscreen;
        self
    }

    pub async fn run_async(&mut self) -> Result<Option<String>, String> {
        if fshell_engine::is_test_mode() {
            return Ok(None);
        }

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let stdin_fd = std::io::stdin().as_raw_fd();
            if unsafe { libc::isatty(stdin_fd) } == 0 {
                return Ok(None);
            }
        }

        let device = match TerminalDevice::auto() {
            Ok(dev) => dev,
            Err(_) => return Ok(None),
        };
        let options = TerminalSessionOptions {
            mode: self.mode,
            hide_cursor: true,
            ..Default::default()
        };
        let mut session = TerminalSession::enter(device, options).map_err(|e| e.to_string())?;
        let events = UnixEventStream::new();
        let mut app = PickerApp::new(&self.prompt, &mut self.items, &self.theme);

        let result = run_tui(&mut app, &mut session, events)
            .await
            .map_err(|e| e.to_string())?;

        Ok(result.flatten())
    }

    pub fn run(&mut self) -> Result<Option<String>, String> {
        if fshell_engine::is_test_mode() {
            return Ok(None);
        }

        if let Ok(_handle) = tokio::runtime::Handle::try_current() {
            std::thread::scope(|s| {
                s.spawn(|| {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| e.to_string())?;
                    rt.block_on(self.run_async())
                })
                .join()
                .map_err(|_| "picker thread panicked".to_string())?
            })
        } else {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            rt.block_on(self.run_async())
        }
    }
}

fn fuzzy_filter<'a>(items: &'a [PickerItem], query: &str) -> Vec<&'a PickerItem> {
    if query.is_empty() {
        return items.iter().collect();
    }

    let kind = FuzzyKind::Simple;
    let prepared = PreparedQuery::new(query);
    let mut scored: Vec<(&PickerItem, isize)> = Vec::new();

    for item in items {
        if let Some(score) = fuzzy_score_prepared(&prepared, &item.display, kind) {
            scored.push((item, score));
        }
    }

    scored.sort_by_key(|a| std::cmp::Reverse(a.1));
    scored.into_iter().map(|(item, _)| item).collect()
}

pub fn get_recursive_files(
    pwd: &str,
    dirs_only: bool,
    max_depth: Option<usize>,
    max_count: Option<usize>,
) -> Vec<PickerItem> {
    let mut items = Vec::new();
    let mut stack = vec![PathBuf::from(pwd)];
    let max_depth = max_depth.unwrap_or(5);
    let max_count = max_count.unwrap_or(5000);
    let mut count = 0;

    while let Some(dir) = stack.pop() {
        if count > max_count {
            break; // limit to prevent lockup
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.to_string_lossy().to_string();

                // Skip hidden files/directories (like .git, .cache)
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }

                let relative = match path.strip_prefix(pwd) {
                    Ok(r) => r.to_string_lossy().to_string(),
                    Err(_) => name.clone(),
                };

                if path.is_dir() {
                    count += 1;
                    items.push(PickerItem {
                        value: relative.clone(),
                        display: format!("{}/", relative),
                    });
                    if path
                        .components()
                        .count()
                        .saturating_sub(PathBuf::from(pwd).components().count())
                        <= max_depth
                    {
                        stack.push(path);
                    }
                } else if !dirs_only {
                    count += 1;
                    items.push(PickerItem {
                        value: relative.clone(),
                        display: relative,
                    });
                }
            }
        }
    }

    items
}

/// Run the unified Ctrl-P picker combining history, files, directories, git branches, and git commits.
/// Returns the selected value (to be inserted into the line editor) or None if cancelled.
pub fn run_unified_picker(current_pwd: &str) -> Option<String> {
    let mut items: Vec<(String, String, i64)> = Vec::new(); // (value, display, score)

    // 1. History: last 500 commands, scored by frequency x recency
    if let Ok(entries) = crate::history::query_history(None, None, None, None, None, None) {
        let mut freq: std::collections::HashMap<&str, (usize, i64)> =
            std::collections::HashMap::new();
        let mut max_ts: i64 = 0;
        let mut min_ts: i64 = i64::MAX;
        for e in &entries {
            let entry = freq
                .entry(e.command.as_str())
                .or_insert((0, e.timestamp_ms));
            entry.0 += 1;
            if e.timestamp_ms > max_ts {
                max_ts = e.timestamp_ms;
            }
            if e.timestamp_ms < min_ts {
                min_ts = e.timestamp_ms;
            }
        }
        // Only use the last 200 unique commands (limit display to top-scored)
        let ts_range = (max_ts - min_ts).max(1) as f64;
        let mut scored: Vec<(&str, f64)> = freq
            .iter()
            .map(|(cmd, (count, ts))| {
                let recency = (ts - min_ts) as f64 / ts_range; // 0..1, higher = more recent
                let score = (*count as f64) * 100.0 + recency * 200.0;
                (*cmd, score)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(200);
        for (cmd, score) in scored {
            items.push((cmd.to_string(), format!("> {}", cmd), score as i64));
        }
    }

    // 2. Files: from current directory (up to 200, with hidden penalty)
    for file_item in get_recursive_files(current_pwd, false, None, None) {
        let score = if file_item.value.starts_with('.') {
            10
        } else {
            60
        };
        items.push((
            file_item.value.clone(),
            format!("/ {}", file_item.display),
            score,
        ));
    }

    // 3. Directories: from z frecency DB (up to 100)
    if let Some(path) = fshell_builtins::get_frecency_db_path()
        && let Ok(content) = std::fs::read_to_string(&path)
        && let Ok(db) = serde_json::from_str::<serde_json::Value>(&content)
        && let Some(paths) = db.get("paths").and_then(|v| v.as_object())
    {
        for (dir_path, entry) in paths {
            let freq = entry
                .get("frequency")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let last_visited = entry
                .get("last_visited")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let age = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let age_hours = age.saturating_sub(last_visited) / 3600;
            let age_mult = if age_hours < 1 {
                4.0
            } else if age_hours < 24 {
                2.0
            } else if age_hours < 168 {
                1.0
            } else {
                0.5
            };
            let score = (freq * age_mult * 10.0) as i64;
            // Only show directories that still exist
            if std::path::Path::new(dir_path).exists() {
                items.push((dir_path.clone(), format!("~ {}", dir_path), score));
            }
        }
    }

    // 4 & 5. Git branches & commits (cached, 5 min TTL)
    {
        let cached_hit = {
            let cache = GIT_PICKER_CACHE.lock();
            if let Some(ref data) = *cache
                && data.pwd == current_pwd
                && data.cached_at.elapsed() < GIT_PICKER_TTL
            {
                Some((data.branches.clone(), data.commits.clone()))
            } else {
                None
            }
        };

        if let Some((branches, commits)) = cached_hit {
            items.extend(branches);
            items.extend(commits);
        } else {
            let mut new_branches: Vec<(String, String, i64)> = Vec::new();
            let mut new_commits: Vec<(String, String, i64)> = Vec::new();

            // 4. Git branches (up to 50)
            if let Ok(repo) =
                fshell_git::repo::Repository::discover(std::path::Path::new(current_pwd))
            {
                for r in repo.list_refs("refs/heads/") {
                    let name = r
                        .name
                        .strip_prefix("refs/heads/")
                        .unwrap_or(&r.name)
                        .to_string();
                    if !name.is_empty() {
                        let entry = (name.clone(), format!("\u{2387} {}", name), 20);
                        new_branches.push(entry.clone());
                        items.push(entry);
                    }
                }
            }

            // 5. Git commits (last 50)
            if let Ok(repo) =
                fshell_git::repo::Repository::discover(std::path::Path::new(current_pwd))
                && let Ok(head) = repo.head()
            {
                let mut count = 0u32;
                let mut queue = std::collections::VecDeque::new();
                queue.push_back(head.oid);
                let mut visited = std::collections::HashSet::new();
                visited.insert(head.oid);

                while let Some(oid) = queue.pop_front() {
                    if count >= 50 {
                        break;
                    }
                    if let Ok(commit) = repo.read_commit(&oid) {
                        let oid_hex = hex::encode(oid);
                        let short = &oid_hex[..7];
                        let first_line = commit.message.lines().next().unwrap_or("");
                        let display = format!("{} {}", short, first_line);
                        let entry = (oid_hex, format!("\u{25C9} {}", display), 15);
                        new_commits.push(entry.clone());
                        items.push(entry);
                        count += 1;

                        for parent in &commit.parents {
                            if visited.insert(*parent) {
                                queue.push_back(*parent);
                            }
                        }
                    }
                }
            }

            let mut cache = GIT_PICKER_CACHE.lock();
            *cache = Some(GitPickerCachedData {
                pwd: current_pwd.to_string(),
                cached_at: Instant::now(),
                branches: new_branches,
                commits: new_commits,
            });
        }
    }

    // Sort by score descending and take top 500
    items.sort_by_key(|a| std::cmp::Reverse(a.2));
    items.truncate(500);

    if items.is_empty() {
        return None;
    }

    let picker_items: Vec<PickerItem> = items
        .into_iter()
        .map(|(value, display, _)| PickerItem { value, display })
        .collect();

    let mut p = Picker::new("ctrl-p:", picker_items);
    p.run().ok().flatten()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use fshell_terminal::input::KeyEvent;
    use ratatui::backend::TestBackend;

    #[test]
    fn test_fuzzy_filter() {
        let items = vec![
            PickerItem {
                value: "main.rs".to_string(),
                display: "src/main.rs".to_string(),
            },
            PickerItem {
                value: "lib.rs".to_string(),
                display: "src/lib.rs".to_string(),
            },
            PickerItem {
                value: "Cargo.toml".to_string(),
                display: "Cargo.toml".to_string(),
            },
        ];

        // Exact prefix/substring should rank higher
        let filtered = fuzzy_filter(&items, "lib");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].value, "lib.rs");

        // Subsequence match
        let filtered_sub = fuzzy_filter(&items, "srclib");
        assert_eq!(filtered_sub.len(), 1);
        assert_eq!(filtered_sub[0].value, "lib.rs");
    }

    #[test]
    fn test_picker_app_navigation_and_selection() {
        let mut items = vec![
            PickerItem {
                value: "one".to_string(),
                display: "First Option".to_string(),
            },
            PickerItem {
                value: "two".to_string(),
                display: "Second Option".to_string(),
            },
            PickerItem {
                value: "three".to_string(),
                display: "Third Option".to_string(),
            },
        ];
        let theme = Theme::default_theme();
        let mut app = PickerApp::new("test:", &mut items, &theme);

        // Move down to second item
        let flow = app.handle_message(InputEvent::Key(KeyEvent::new(
            Key::Down,
            Modifiers::empty(),
        )));
        assert_eq!(flow, AppFlow::Continue);
        assert_eq!(app.selected_idx, 1);

        // Press Enter to select
        let flow = app.handle_message(InputEvent::Key(KeyEvent::new(
            Key::Enter,
            Modifiers::empty(),
        )));
        assert_eq!(flow, AppFlow::Break(Some("two".to_string())));
    }

    #[test]
    fn test_picker_app_filtering_and_cancel() {
        let mut items = vec![
            PickerItem {
                value: "apple".to_string(),
                display: "Apple".to_string(),
            },
            PickerItem {
                value: "banana".to_string(),
                display: "Banana".to_string(),
            },
        ];
        let theme = Theme::default_theme();
        let mut app = PickerApp::new("fruits:", &mut items, &theme);

        // Type 'b' to filter
        let flow = app.handle_message(InputEvent::Key(KeyEvent::new(
            Key::Character('b'),
            Modifiers::empty(),
        )));
        assert_eq!(flow, AppFlow::Continue);
        assert_eq!(app.search_bar.query, "b");

        // Press Escape to cancel
        let flow = app.handle_message(InputEvent::Key(KeyEvent::new(
            Key::Escape,
            Modifiers::empty(),
        )));
        assert_eq!(flow, AppFlow::Break(None));
    }

    #[test]
    fn test_picker_app_rendering_headless() {
        let mut items = vec![
            PickerItem {
                value: "file1.txt".to_string(),
                display: "file1.txt".to_string(),
            },
            PickerItem {
                value: "file2.txt".to_string(),
                display: "file2.txt".to_string(),
            },
        ];
        let theme = Theme::default_theme();
        let mut app = PickerApp::new("files:", &mut items, &theme);

        // Test rendering in inline mode viewport (80x10)
        let backend = TestBackend::new(80, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.render(frame, area);
            })
            .unwrap();

        // Test rendering in fullscreen viewport (80x24)
        let backend_full = TestBackend::new(80, 24);
        let mut terminal_full = ratatui::Terminal::new(backend_full).unwrap();
        terminal_full
            .draw(|frame| {
                let area = frame.area();
                app.render(frame, area);
            })
            .unwrap();
    }

    #[test]
    fn test_picker_mode_configuration() {
        let items = vec![PickerItem {
            value: "item".to_string(),
            display: "Item".to_string(),
        }];
        let picker = Picker::new("test:", items.clone());
        assert_eq!(picker.mode, TerminalMode::Inline { height: 10 });

        let picker_fullscreen = picker.fullscreen();
        assert_eq!(picker_fullscreen.mode, TerminalMode::Fullscreen);

        let picker_custom =
            Picker::new("test:", items).with_mode(TerminalMode::Inline { height: 15 });
        assert_eq!(picker_custom.mode, TerminalMode::Inline { height: 15 });
    }
}
