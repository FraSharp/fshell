// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::fuzzy::{FuzzyKind, PreparedQuery, fuzzy_score_prepared};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

struct GitPickerCachedData {
    pwd: String,
    cached_at: Instant,
    branches: Vec<(String, String, i64)>,
    commits: Vec<(String, String, i64)>,
}

static GIT_PICKER_CACHE: Mutex<Option<GitPickerCachedData>> = Mutex::new(None);
const GIT_PICKER_TTL: std::time::Duration = std::time::Duration::from_secs(300);

pub struct PickerItem {
    pub value: String,
    pub display: String,
}

pub struct Picker {
    prompt: String,
    items: Vec<PickerItem>,
}

struct PickerGuard;

impl PickerGuard {
    fn new() -> Result<Self, String> {
        terminal::enable_raw_mode().map_err(|e| e.to_string())?;
        execute!(stdout(), EnterAlternateScreen, Hide).map_err(|e| e.to_string())?;
        Ok(PickerGuard)
    }
}

impl Drop for PickerGuard {
    fn drop(&mut self) {
        let _ = execute!(stdout(), Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

fn truncate_str_to_width(s: &str, max_width: usize) -> String {
    let mut width = 0;
    let mut res = String::new();
    for c in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if width + w > max_width {
            break;
        }
        width += w;
        res.push(c);
    }
    res
}

impl Picker {
    pub fn new(prompt: &str, items: Vec<PickerItem>) -> Self {
        Self {
            prompt: prompt.to_string(),
            items,
        }
    }

    pub fn run(&mut self) -> Result<Option<String>, String> {
        let _guard = PickerGuard::new()?;
        let mut stdout = stdout();

        let mut query = String::new();
        let mut selected_idx = 0;
        let mut scroll_offset = 0;

        let result = loop {
            // Filter items based on subsequence matching and rank by score
            let filtered = fuzzy_filter(&self.items, &query);

            // Adjust selection bounds
            if filtered.is_empty() {
                selected_idx = 0;
            } else if selected_idx >= filtered.len() {
                selected_idx = filtered.len() - 1;
            }

            // Draw UI
            let (cols, rows) = terminal::size().unwrap_or((80, 24));
            let max_visible = (rows as usize).saturating_sub(4);

            // Keep selected index visible (scroll offset)
            if selected_idx < scroll_offset {
                scroll_offset = selected_idx;
            } else if selected_idx >= scroll_offset + max_visible {
                scroll_offset = selected_idx + 1 - max_visible;
            }

            // Clear screen
            execute!(stdout, MoveTo(0, 0), Clear(ClearType::All)).map_err(|e| e.to_string())?;

            // Print Header/Prompt
            execute!(
                stdout,
                Print(format!("> {} {}\r\n", self.prompt, query)),
                Print(format!(
                    "  ({}/{} matches)\r\n\r\n",
                    filtered.len(),
                    self.items.len()
                ))
            )
            .map_err(|e| e.to_string())?;

            // Render visible items
            let visible_items = filtered
                .iter()
                .skip(scroll_offset)
                .take(max_visible)
                .enumerate();

            let max_item_w = (cols as usize).saturating_sub(4);
            for (i, item) in visible_items {
                let is_selected = scroll_offset + i == selected_idx;
                let display_line = truncate_str_to_width(&item.display, max_item_w);
                if is_selected {
                    execute!(
                        stdout,
                        SetForegroundColor(Color::Black),
                        SetBackgroundColor(Color::Cyan),
                        Print(format!("~> {}\r\n", display_line)),
                        ResetColor
                    )
                    .map_err(|e| e.to_string())?;
                } else {
                    execute!(stdout, Print(format!("   {}\r\n", display_line)))
                        .map_err(|e| e.to_string())?;
                }
            }

            stdout.flush().map_err(|e| e.to_string())?;

            // Read keypresses
            if fshell_engine::is_test_mode() {
                break None;
            }
            #[cfg(unix)]
            {
                use std::os::unix::io::AsRawFd;
                let stdin_fd = std::io::stdin().as_raw_fd();
                if unsafe { libc::isatty(stdin_fd) } == 0 {
                    break None;
                }
            }
            if event::poll(std::time::Duration::from_millis(200)).map_err(|e| e.to_string())?
                && let Event::Key(KeyEvent {
                    code, modifiers, ..
                }) = event::read().map_err(|e| e.to_string())?
            {
                match code {
                    KeyCode::Esc => break None,
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                        break None;
                    }
                    KeyCode::Char('g') if modifiers.contains(KeyModifiers::CONTROL) => {
                        break None;
                    }
                    KeyCode::Enter => {
                        if !filtered.is_empty() {
                            break Some(filtered[selected_idx].value.clone());
                        } else {
                            break None;
                        }
                    }
                    KeyCode::Up => {
                        selected_idx = selected_idx.saturating_sub(1);
                    }
                    KeyCode::Down if !filtered.is_empty() && selected_idx < filtered.len() - 1 => {
                        selected_idx += 1;
                    }
                    KeyCode::Char('d') | KeyCode::Char('D')
                        if self.prompt == "sessions:" && !filtered.is_empty() =>
                    {
                        let item_value = filtered[selected_idx].value.clone();
                        if item_value != "new" {
                            let path = std::path::PathBuf::from(&item_value);
                            let filename = path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .map(|s| s.trim_end_matches(".json"))
                                .unwrap_or("unknown");

                            let confirm_msg = format!("! Delete session {}? (y/N): ", filename);
                            if let Ok(ans) = draw_prompt(&mut stdout, &confirm_msg) {
                                let trimmed = ans.trim().to_lowercase();
                                if trimmed == "y" || trimmed == "yes" {
                                    let json_path = path.clone();
                                    let log_path = path.with_extension("log");
                                    let _ = std::fs::remove_file(json_path);
                                    let _ = std::fs::remove_file(log_path);

                                    if let Some(pos) =
                                        self.items.iter().position(|it| it.value == item_value)
                                    {
                                        self.items.remove(pos);
                                    }
                                    if selected_idx >= self.items.len().saturating_sub(1) {
                                        selected_idx = self.items.len().saturating_sub(1);
                                    }
                                }
                            }
                        }
                    }
                    KeyCode::Char('r') | KeyCode::Char('R')
                        if self.prompt == "sessions:" && !filtered.is_empty() =>
                    {
                        let item_value = filtered[selected_idx].value.clone();
                        if item_value != "new" {
                            let path = std::path::PathBuf::from(&item_value);
                            let filename = path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .map(|s| s.trim_end_matches(".json"))
                                .unwrap_or("unknown");

                            let rename_msg = format!("Enter new name for session {}: ", filename);
                            if let Ok(new_name) = draw_prompt(&mut stdout, &rename_msg) {
                                let new_name = new_name.trim();
                                if !new_name.is_empty()
                                    && let Ok(content) = std::fs::read_to_string(&path)
                                    && let Ok(mut state) = serde_json::from_str::<
                                        fshell_engine::handoff::HandoffState,
                                    >(
                                        &content
                                    )
                                {
                                    state.vars.insert(
                                        "FSH_SESSION_NAME".to_string(),
                                        fshell_core::Val::String(new_name.to_string()),
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
                                        self.items.iter().position(|it| it.value == item_value)
                                    {
                                        self.items[pos].display = display;
                                    }
                                }
                            }
                        }
                    }
                    KeyCode::Char(c) => {
                        query.push(c);
                    }
                    KeyCode::Backspace => {
                        query.pop();
                    }
                    _ => {}
                }
            }
        };

        Ok(result)
    }
}

fn draw_prompt(stdout: &mut std::io::Stdout, message: &str) -> Result<String, String> {
    let (_cols, rows) = terminal::size().unwrap_or((80, 24));
    execute!(
        stdout,
        MoveTo(0, rows - 1),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::Yellow),
        Print(message),
        SetForegroundColor(Color::White),
        Show,
        ResetColor
    )
    .map_err(|e| e.to_string())?;
    let _ = stdout.flush();

    let mut input = String::new();
    loop {
        if fshell_engine::is_test_mode() {
            break;
        }
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let stdin_fd = std::io::stdin().as_raw_fd();
            if unsafe { libc::isatty(stdin_fd) } == 0 {
                break;
            }
        }
        if event::poll(std::time::Duration::from_millis(200)).map_err(|e| e.to_string())?
            && let Event::Key(KeyEvent { code, .. }) = event::read().map_err(|e| e.to_string())?
        {
            match code {
                KeyCode::Enter => break,
                KeyCode::Esc => {
                    input.clear();
                    break;
                }
                KeyCode::Char(c) => {
                    input.push(c);
                    execute!(stdout, Print(c)).map_err(|e| e.to_string())?;
                    let _ = stdout.flush();
                }
                KeyCode::Backspace if !input.is_empty() => {
                    input.pop();
                    execute!(stdout, Print("\u{0008} \u{0008}")).map_err(|e| e.to_string())?;
                    let _ = stdout.flush();
                }
                _ => {}
            }
        }
    }
    execute!(stdout, Hide).map_err(|e| e.to_string())?;
    Ok(input)
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
        if let Ok(cache) = GIT_PICKER_CACHE.lock()
            && let Some(ref data) = *cache
            && data.pwd == current_pwd
            && data.cached_at.elapsed() < GIT_PICKER_TTL
        {
            items.extend(data.branches.iter().cloned());
            items.extend(data.commits.iter().cloned());
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

            if let Ok(mut cache) = GIT_PICKER_CACHE.lock() {
                *cache = Some(GitPickerCachedData {
                    pwd: current_pwd.to_string(),
                    cached_at: Instant::now(),
                    branches: new_branches,
                    commits: new_commits,
                });
            }
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
mod tests {
    use super::*;

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
}
