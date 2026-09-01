// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Fish-like autosuggestion engine.
//!
//! Searches three sources in priority order:
//! 1. **History** — finds the most recent command line that starts with the current input.
//! 2. **Path** — if the current token looks like a file path, suggests directory entries.
//! 3. **Argument prediction** — if we know the command, suggest flags/args from history.

use fshell_engine::Env;
use nu_ansi_term::{Color, Style};
use std::sync::{Arc, Mutex};

pub struct FshellHinter {
    style: Style,
    min_chars: usize,
    current_hint: String,
    path_cache: Arc<Mutex<Option<DirCache>>>,
    path_updating: Arc<std::sync::atomic::AtomicBool>,
    history_cache: Arc<Mutex<Option<HistoryHintCache>>>,
    env: Option<Env>,
}

type DirCache = (String, std::time::Instant, Vec<(String, bool)>);
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
    const HISTORY_CACHE_TTL_MS: u64 = 150;

    /// Try to find a history-based hint.
    pub fn history_hint(&self, line: &str) -> Option<String> {
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

        let result = crate::history::query_history(Some(10), Some(line), None, None, None, None)
            .ok()
            .and_then(|entries| {
                for entry in entries {
                    if entry.command.starts_with(line) && entry.command.len() > line.len() {
                        return Some(entry.command[line.len()..].to_string());
                    }
                }
                None
            });

        if let Ok(mut cache) = self.history_cache.lock() {
            *cache = Some((line.to_string(), now, result.clone()));
        }
        result
    }

    /// Try to find a path-based hint when the current token looks like a file path.
    pub fn path_hint(&mut self, line: &str) -> Option<String> {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            return None;
        }
        let last_token = trimmed
            .split(|c: char| c.is_whitespace() || c == '|' || c == '<' || c == '>')
            .next_back()
            .unwrap_or("");

        let looks_like_path = last_token.starts_with('/')
            || last_token.starts_with("./")
            || last_token.starts_with("../")
            || last_token.starts_with("~/")
            || (last_token.contains('/') && !last_token.starts_with('-'));

        if !looks_like_path {
            return None;
        }

        let unquoted = last_token.trim_matches('\'').trim_matches('"');
        let expanded = if let Some(stripped) = unquoted.strip_prefix("~/") {
            if let Ok(home) = std::env::var("HOME") {
                format!("{}/{}", home, stripped)
            } else {
                unquoted.to_string()
            }
        } else {
            unquoted.to_string()
        };

        let path = std::path::Path::new(&expanded);
        let (dir_to_read, file_prefix, prefix) = if expanded.ends_with('/') {
            (path.to_path_buf(), String::new(), unquoted.to_string())
        } else {
            let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
            let prefix_dir = match unquoted.rfind('/') {
                Some(idx) => &unquoted[..=idx],
                None => "",
            };
            let file_prefix = path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            (parent.to_path_buf(), file_prefix, prefix_dir.to_string())
        };

        let dir_str = dir_to_read.to_string_lossy().to_string();
        let mut cached_entries = Vec::new();
        let mut needs_refresh = false;

        if let Ok(cache) = self.path_cache.lock() {
            if let Some((cached_dir, timestamp, entries)) = cache.as_ref() {
                if cached_dir == &dir_str
                    && timestamp.elapsed().as_millis() < Self::PATH_CACHE_TTL_MS as u128
                {
                    cached_entries = entries.clone();
                } else {
                    needs_refresh = true;
                }
            } else {
                needs_refresh = true;
            }
        }

        let mut best_hint: Option<String> = None;

        if needs_refresh
            && !self
                .path_updating
                .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            let path_cache_clone = Arc::clone(&self.path_cache);
            let path_updating_clone = Arc::clone(&self.path_updating);
            let dir_to_read_clone = dir_to_read.clone();
            let dir_str_clone = dir_str.clone();

            std::thread::spawn(move || {
                let mut entries = Vec::new();
                if let Ok(read_dir) = std::fs::read_dir(&dir_to_read_clone) {
                    for entry in read_dir.flatten() {
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
            if full.len() >= last_token.len() {
                full[last_token.len()..].to_string()
            } else {
                full
            }
        })
    }

    /// Try to find a completion-based hint.
    pub fn completion_hint(&self, line: &str) -> Option<String> {
        let env = self.env.as_ref()?;
        let pos = line.len();
        let suggestions = crate::autocomplete::get_custom_completions(line, pos, env)?;
        if suggestions.is_empty() {
            return None;
        }
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

    pub fn handle(&mut self, line: &str, pos: usize, use_ansi_coloring: bool) -> String {
        if pos != line.len() {
            self.current_hint.clear();
            return String::new();
        }
        let hint = if let Some(h) = self.history_hint(line) {
            h
        } else if let Some(h) = self.completion_hint(line) {
            h
        } else {
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

    pub fn complete_hint(&self) -> String {
        self.current_hint.clone()
    }

    pub fn next_hint_token(&self) -> String {
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

    #[test]
    fn test_path_hint_does_not_crash() {
        let mut h = FshellHinter::default();
        let _hint = h.path_hint("/nonexistent/path/");
    }

    #[test]
    fn test_hinter_complete_hint() {
        let mut h = FshellHinter::default();
        h.current_hint = "uild --release".to_string();
        assert_eq!(h.complete_hint(), "uild --release");
    }

    #[test]
    fn test_hinter_next_hint_token() {
        let mut h = FshellHinter::default();
        h.current_hint = "build --release".to_string();
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
        let result = h.handle("mycmd -", 7, false);
        assert!(!result.is_empty());
    }
}
