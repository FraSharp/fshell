// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_engine::Env;
use nu_ansi_term::{Color, Style};
use reedline::{CommandLineSearch, Hinter, History, SearchFilter, SearchQuery};

/// Fish-like autosuggestion engine.
///
/// Searches three sources in priority order:
/// 1. **History** — finds the most recent command line that starts with the current input.
/// 2. **Path** — if the current token looks like a file path, suggests directory entries.
/// 3. **Argument prediction** — if we know the command, suggest flags/args from history.
use std::sync::{Arc, Mutex};

pub struct FshellHinter {
    style: Style,
    min_chars: usize,
    current_hint: String,
    /// Cache directory entries to avoid blocking I/O on every keystroke.
    /// Stores (dir_path, timestamp, [(name, is_dir)]).
    path_cache: Arc<Mutex<Option<DirCache>>>,
    path_updating: Arc<std::sync::atomic::AtomicBool>,
    /// Memoized result of the most recent `history_hint` DB query, keyed by
    /// the input line. The ghost hint is redrawn on *every* frame (cursor
    /// moves, completions, etc.) while the line is unchanged, so without this
    /// cache each keystroke would pay a synchronous SQLite query.
    history_cache: Arc<Mutex<Option<HistoryHintCache>>>,
    env: Option<Env>,
}

/// Cached directory listing: (path, timestamp, entries).
type DirCache = (String, std::time::Instant, Vec<(String, bool)>);

/// Memoized `history_hint` result: (input line, timestamp, hint).
type HistoryHintCache = (String, std::time::Instant, Option<String>);

impl Default for FshellHinter {
    fn default() -> Self {
        Self {
            style: Style::default().fg(Color::DarkGray),
            min_chars: 1,
            current_hint: String::new(),
            path_cache: Arc::new(Mutex::new(None)),
            path_updating: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            history_cache: Arc::new(Mutex::new(None)),
            env: None,
        }
    }
}

impl FshellHinter {
    pub fn with_env(mut self, env: Env) -> Self {
        self.env = Some(env);
        self
    }

    pub fn with_style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub fn with_min_chars(mut self, min_chars: usize) -> Self {
        self.min_chars = min_chars;
        self
    }

    const PATH_CACHE_TTL_MS: u64 = 400;
    /// Ghost hints are re-rendered on every frame; a synchronous SQLite query
    /// per frame is the main per-keystroke lag. Reuse the last result for the
    /// same line within a short window.
    const HISTORY_CACHE_TTL_MS: u64 = 150;

    /// Try to find a history-based hint.
    fn history_hint(&self, line: &str, history: &dyn History) -> Option<String> {
        if line.len() < self.min_chars {
            return None;
        }
        let now = std::time::Instant::now();
        if let Ok(cache) = self.history_cache.lock()
            && let Some((cached_line, ts, res)) = cache.as_ref()
            && cached_line == line
            && ts.elapsed().as_millis() < Self::HISTORY_CACHE_TTL_MS as u128
        {
            return res.clone();
        }
        let query = SearchQuery::last_with_search(SearchFilter::from_text_search(
            CommandLineSearch::Prefix(line.to_string()),
            None,
        ));
        let result = match history.search(query) {
            Ok(items) => {
                if let Some(item) = items.first()
                    && let cmd = &item.command_line
                    && cmd.starts_with(line)
                    && cmd.len() > line.len()
                {
                    Some(cmd[line.len()..].to_string())
                } else {
                    None
                }
            }
            Err(_) => None,
        };
        if let Ok(mut cache) = self.history_cache.lock() {
            *cache = Some((line.to_string(), now, result.clone()));
        }
        result
    }

    /// Try to find a path-based hint when the current token looks like a file path.
    fn path_hint(&mut self, line: &str) -> Option<String> {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            return None;
        }
        // Get the last token
        let last_token = trimmed
            .split(|c: char| c.is_whitespace() || c == '|' || c == '<' || c == '>')
            .next_back()
            .unwrap_or("");
        if last_token.is_empty() {
            return None;
        }
        // Only hint paths if the token contains path characters
        if !last_token.starts_with('/')
            && !last_token.starts_with('.')
            && !last_token.starts_with('~')
        {
            return None;
        }
        let expanded = if last_token.starts_with('~') {
            if let Ok(home) = std::env::var("HOME") {
                last_token.replacen('~', &home, 1)
            } else {
                last_token.to_string()
            }
        } else {
            last_token.to_string()
        };
        let path = std::path::PathBuf::from(&expanded);
        // For ~ alone, the expanded home directory IS the directory to search.
        // The file_prefix is the last component of the path. When last_token
        // is just "~", the prefix is the home directory name and we should
        // search inside the home directory, not in the parent.
        let (search_dir, file_prefix) = if last_token == "~" || last_token == "~/" {
            if let Ok(home) = std::env::var("HOME") {
                (std::path::PathBuf::from(home), String::new())
            } else {
                (path, String::new())
            }
        } else if expanded.ends_with('/') {
            (path, String::new())
        } else if let Some(parent) = path.parent() {
            let prefix = path
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("")
                .to_string();
            (parent.to_path_buf(), prefix)
        } else {
            (std::path::PathBuf::from("."), expanded.clone())
        };
        let dir_to_read = if search_dir.as_os_str().is_empty() {
            std::path::PathBuf::from(".")
        } else {
            search_dir
        };
        let dir_str = dir_to_read.to_string_lossy().to_string();
        // Compute the prefix once before the loop.
        let prefix = if !file_prefix.is_empty()
            && last_token.len() >= file_prefix.len()
            && last_token.ends_with(&file_prefix)
        {
            last_token[..last_token.len() - file_prefix.len()].to_string()
        } else if last_token.ends_with('/') {
            last_token.to_string()
        } else {
            format!("{}/", last_token)
        };
        let mut best_hint: Option<String> = None;

        let mut needs_update = false;
        let mut cached_entries = Vec::new();

        if let Ok(cache) = self.path_cache.lock() {
            if let Some((ref cached_dir, ts, ref entries)) = *cache {
                if cached_dir == &dir_str
                    && ts.elapsed().as_millis() < Self::PATH_CACHE_TTL_MS as u128
                {
                    cached_entries = entries.clone();
                } else {
                    cached_entries = entries.clone();
                    needs_update = true;
                }
            } else {
                needs_update = true;
            }
        }

        if needs_update
            && self
                .path_updating
                .compare_exchange(
                    false,
                    true,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                )
                .is_ok()
        {
            let path_cache_clone = self.path_cache.clone();
            let path_updating_clone = self.path_updating.clone();
            let dir_to_read_clone = dir_to_read.clone();
            let dir_str_clone = dir_str.clone();
            std::thread::spawn(move || {
                let mut entries = Vec::new();
                if let Ok(dir_entries) = std::fs::read_dir(&dir_to_read_clone) {
                    for entry in dir_entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        entries.push((name, is_dir));
                    }
                }
                if let Ok(mut cache) = path_cache_clone.lock() {
                    *cache = Some((dir_str_clone, std::time::Instant::now(), entries));
                }
                path_updating_clone.store(false, std::sync::atomic::Ordering::SeqCst);
            });
        }

        for (name, is_dir) in &cached_entries {
            if name.starts_with(&file_prefix) && name.len() > file_prefix.len() {
                let mut full = format!("{}{}", prefix, name);
                if *is_dir {
                    full.push('/');
                }
                // Prefer the lexicographically smallest or shortest match
                if let Some(ref current) = best_hint {
                    if full.len() < current.len()
                        || (full.len() == current.len() && full < *current)
                    {
                        best_hint = Some(full);
                    }
                } else {
                    best_hint = Some(full);
                }
            }
        }
        best_hint.map(|full| {
            // The hint is the suffix after the current input, not the full path.
            if full.len() >= last_token.len() {
                full[last_token.len()..].to_string()
            } else {
                full
            }
        })
    }

    /// Try to find a completion-based hint.
    fn completion_hint(&self, line: &str) -> Option<String> {
        let env = self.env.as_ref()?;
        let pos = line.len();
        // Get custom completions for the current line
        let suggestions = crate::autocomplete::get_custom_completions(line, pos, env)?;
        if suggestions.is_empty() {
            return None;
        }
        // Take the first suggestion
        let first_sugg = &suggestions[0];
        let span = first_sugg.span;

        if span.start <= pos && span.end <= pos && span.start <= span.end {
            let mut completed = line[..span.start].to_string();
            completed.push_str(&first_sugg.value);
            if completed.starts_with(line) && completed.len() > line.len() {
                return Some(completed[line.len()..].to_string());
            }
        }
        None
    }
}

impl Hinter for FshellHinter {
    fn handle(
        &mut self,
        line: &str,
        pos: usize,
        history: &dyn History,
        use_ansi_coloring: bool,
        _cwd: &str,
    ) -> String {
        if pos != line.len() {
            // Only show hints when cursor is at the end of the line
            self.current_hint.clear();
            return String::new();
        }
        // Try history first (highest priority)
        let hint = if let Some(h) = self.history_hint(line, history) {
            h
        } else if let Some(h) = self.completion_hint(line) {
            h
        } else {
            // Fall back to path hints
            self.path_hint(line).unwrap_or_default()
        };
        self.current_hint = hint.clone();
        if hint.is_empty() {
            return String::new();
        }
        if use_ansi_coloring {
            format!("{}{}{}", self.style.prefix(), hint, self.style.suffix())
        } else {
            hint
        }
    }

    fn complete_hint(&self) -> String {
        self.current_hint.clone()
    }

    fn next_hint_token(&self) -> String {
        // Return the first token of the hint (up to the first whitespace or slash)
        self.current_hint
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reedline::{HistoryItem, HistoryItemId, Result as HistoryResult, SearchDirection};
    use std::time::Duration;

    struct MockHistory {
        items: Vec<HistoryItem>,
    }

    fn make_item(cmd: &str) -> HistoryItem {
        HistoryItem {
            id: None,
            start_timestamp: None,
            command_line: cmd.to_string(),
            session_id: None,
            hostname: None,
            cwd: None,
            duration: Some(Duration::from_millis(0)),
            exit_status: Some(0),
            more_info: None,
        }
    }

    impl History for MockHistory {
        fn save(&mut self, h: HistoryItem) -> HistoryResult<HistoryItem> {
            let mut item = h;
            item.id = Some(HistoryItemId::new(self.items.len() as i64));
            self.items.push(item.clone());
            Ok(item)
        }
        fn load(&self, id: HistoryItemId) -> HistoryResult<HistoryItem> {
            if let Some(item) = self.items.get(id.0 as usize) {
                Ok(item.clone())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("History item {} not found", id.0),
                )
                .into())
            }
        }
        fn count(&self, query: SearchQuery) -> HistoryResult<i64> {
            let results = self.search(query)?;
            Ok(results.len() as i64)
        }
        fn search(&self, query: SearchQuery) -> HistoryResult<Vec<HistoryItem>> {
            let mut results = Vec::new();
            let items: Vec<HistoryItem> = if query.direction == SearchDirection::Backward {
                self.items.iter().rev().cloned().collect()
            } else {
                self.items.clone()
            };
            for item in items {
                let mut matches = true;
                if let Some(ref cmd_filter) = query.filter.command_line {
                    match cmd_filter {
                        CommandLineSearch::Prefix(prefix) => {
                            if !item.command_line.starts_with(prefix) {
                                matches = false;
                            }
                        }
                        CommandLineSearch::Substring(sub) => {
                            if !item.command_line.contains(sub) {
                                matches = false;
                            }
                        }
                        CommandLineSearch::Exact(exact) => {
                            if item.command_line != *exact {
                                matches = false;
                            }
                        }
                    }
                }
                if matches {
                    results.push(item);
                }
                if let Some(limit) = query.limit
                    && results.len() >= limit as usize
                {
                    break;
                }
            }
            Ok(results)
        }
        fn update(
            &mut self,
            _id: HistoryItemId,
            _updater: &dyn Fn(HistoryItem) -> HistoryItem,
        ) -> HistoryResult<()> {
            Ok(())
        }
        fn clear(&mut self) -> HistoryResult<()> {
            self.items.clear();
            Ok(())
        }
        fn delete(&mut self, _id: HistoryItemId) -> HistoryResult<()> {
            Ok(())
        }
        fn sync(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn session(&self) -> Option<reedline::HistorySessionId> {
            None
        }
    }

    #[test]
    fn test_history_hint_exact_prefix() {
        let h = FshellHinter::default();
        let history = MockHistory {
            items: vec![make_item("cargo test"), make_item("cargo build --release")],
        };
        let hint = h.history_hint("cargo b", &history);
        assert_eq!(hint, Some("uild --release".to_string()));
    }

    #[test]
    fn test_history_hint_no_match() {
        let h = FshellHinter::default();
        let history = MockHistory {
            items: vec![make_item("ls -la")],
        };
        let hint = h.history_hint("cd", &history);
        assert_eq!(hint, None);
    }

    #[test]
    fn test_history_hint_empty_input() {
        let h = FshellHinter::default();
        let history = MockHistory {
            items: vec![make_item("ls")],
        };
        let hint = h.history_hint("", &history);
        assert_eq!(hint, None);
    }

    #[test]
    fn test_path_hint_does_not_crash() {
        let mut h = FshellHinter::default();
        // Should not panic on non-existent path
        let _hint = h.path_hint("/nonexistent/path/");
    }

    #[test]
    fn test_hinter_handle() {
        let mut h = FshellHinter::default();
        let history = MockHistory {
            items: vec![make_item("cargo build --release")],
        };
        let result = h.handle("cargo b", 7, &history, false, "/home");
        assert_eq!(result, "uild --release");
    }

    #[test]
    fn test_hinter_complete_hint() {
        let mut h = FshellHinter::default();
        let history = MockHistory {
            items: vec![make_item("cargo build --release")],
        };
        h.handle("cargo b", 7, &history, false, "/home");
        assert_eq!(h.complete_hint(), "uild --release");
    }

    #[test]
    fn test_hinter_next_hint_token() {
        let mut h = FshellHinter::default();
        let history = MockHistory {
            items: vec![make_item("cargo build --release")],
        };
        h.handle("cargo ", 6, &history, false, "/home");
        assert_eq!(h.next_hint_token(), "build");
    }

    #[test]
    fn test_completion_hint() {
        let env = fshell_engine::Env::new();
        {
            let mut reg = env.completions.write();
            let mut comp = fshell_core::CommandCompletion::default();
            comp.flags.push(fshell_core::FlagCompletion {
                parent_subcmds: vec![],
                short: Some("f".to_string()),
                long: Some("force".to_string()),
                desc: Some("Force action".to_string()),
                choices: None,
            });
            reg.insert("mycmd".to_string(), comp);
        }

        let mut h = FshellHinter::default().with_env(env);
        let history = MockHistory { items: vec![] };
        let result = h.handle("mycmd -", 7, &history, false, "/home");
        assert!(!result.is_empty());
    }
}
