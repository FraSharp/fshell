// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Filesystem path drilling, fuzzy matching, and metadata lookups.

use super::types::{CompletionCancel, CompletionCandidate, CompletionKind, TextSpan};
use crate::fuzzy;
use fshell_core::{FxIndexMap, Val};
use fshell_engine::Env;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

const DIRECTORY_SNAPSHOT_TTL: Duration = Duration::from_millis(400);
const MAX_CACHED_DIRECTORIES: usize = 64;
const MAX_CACHED_DIRECTORY_ENTRIES: usize = 2000;

type DirectoryEntries = Arc<Vec<(String, bool)>>;

struct DirectorySnapshot {
    loaded_at: Instant,
    entries: DirectoryEntries,
}

// Completion runs on the REPL's blocking worker. Reuse bounded directory snapshots across
// adjacent keystrokes so a live completion session does not rescan its directory per key.
static DIRECTORY_SNAPSHOTS: LazyLock<Mutex<FxIndexMap<std::path::PathBuf, DirectorySnapshot>>> =
    LazyLock::new(|| Mutex::new(FxIndexMap::default()));

/// Expand `$VAR` and `${VAR}` patterns using environment variables.
pub fn expand_env_vars(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        if c == '$' {
            if let Some(&(_, '$')) = chars.peek() {
                chars.next();
                result.push('$');
                continue;
            }
            if let Some(&(_, '{')) = chars.peek() {
                chars.next();
                let mut name = String::new();
                for (_, c) in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    name.push(c);
                }
                match std::env::var(&name) {
                    Ok(val) => result.push_str(&val),
                    Err(_) => {
                        result.push('$');
                        result.push('{');
                        result.push_str(&name);
                        result.push('}');
                    }
                }
                continue;
            }
            let mut name = String::new();
            let mut trailing = None;
            for (_, c) in chars.by_ref() {
                if c.is_alphanumeric() || c == '_' {
                    name.push(c);
                } else {
                    trailing = Some(c);
                    break;
                }
            }
            if name.is_empty() {
                result.push('$');
                if let Some(t) = trailing {
                    result.push(t);
                }
            } else {
                match std::env::var(&name) {
                    Ok(val) => {
                        result.push_str(&val);
                        if let Some(t) = trailing {
                            result.push(t);
                        }
                    }
                    Err(_) => {
                        result.push('$');
                        result.push_str(&name);
                        if let Some(t) = trailing {
                            result.push(t);
                        }
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

pub fn complete_files(last_word: &str, pos: usize) -> Vec<CompletionCandidate> {
    complete_files_at(last_word, pos, std::path::Path::new("."), None)
}

/// Complete filesystem paths relative to the shell's logical working directory.
pub fn complete_files_at(
    last_word: &str,
    pos: usize,
    cwd: &std::path::Path,
    cancel: Option<&CompletionCancel>,
) -> Vec<CompletionCandidate> {
    let word_len = last_word.len();
    let has_opening_double_quote = last_word.starts_with('"');
    let has_opening_single_quote = last_word.starts_with('\'');

    let unquoted = if has_opening_double_quote {
        last_word
            .strip_prefix('"')
            .unwrap_or(last_word)
            .trim_end_matches('"')
    } else if has_opening_single_quote {
        last_word
            .strip_prefix('\'')
            .unwrap_or(last_word)
            .trim_end_matches('\'')
    } else {
        last_word
    };

    let unquoted_owned = unquoted.to_string();

    let expanded = if unquoted_owned.starts_with('~') {
        if let Ok(home) = std::env::var("HOME") {
            unquoted_owned.replacen('~', &home, 1)
        } else {
            unquoted_owned.clone()
        }
    } else {
        unquoted_owned.clone()
    };
    let expanded = expand_env_vars(&expanded);

    let raw_path = std::path::PathBuf::from(&expanded);
    let (search_dir, file_prefix) = if expanded.is_empty() {
        // An empty argument means "the current directory".  Treating it as
        // `cwd.join("")` and then taking that path's parent turns the cwd
        // itself into a filename prefix, which makes `ls <Tab>` complete the
        // cwd name from its parent instead of listing the cwd's entries.
        (cwd.to_path_buf(), String::new())
    } else {
        let path = if raw_path.is_absolute() {
            raw_path
        } else {
            cwd.join(raw_path)
        };
        if expanded.ends_with('/') {
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
        }
    };

    let dir_to_read = if search_dir.as_os_str().is_empty() {
        std::path::PathBuf::from(".")
    } else {
        search_dir
    };

    let perform_read = move || {
        if cancel.is_some_and(|c| c.is_cancelled()) {
            return Vec::new();
        }
        let prepared = fuzzy::PreparedQuery::new(&file_prefix);
        // The heap's maximum is the worst retained match: lower score, then
        // lexicographically later name. Bound results, never directory coverage.
        let mut scored: BinaryHeap<(Reverse<isize>, String, bool)> = BinaryHeap::new();
        if !visit_directory(&dir_to_read, cancel, |name, is_dir, kind| {
            if let Some(score) = fuzzy::fuzzy_score_prepared(&prepared, name, kind) {
                let better = scored.peek().is_none_or(|worst| {
                    Reverse(score) < worst.0
                        || (Reverse(score) == worst.0 && name < worst.1.as_str())
                });
                if scored.len() < fuzzy::MAX_RESULTS {
                    scored.push((Reverse(score), name.to_owned(), is_dir));
                } else if better && let Some(mut worst) = scored.peek_mut() {
                    *worst = (Reverse(score), name.to_owned(), is_dir);
                }
            }
        }) {
            return Vec::new();
        }

        let user_prefix = match unquoted_owned.rfind('/') {
            Some(idx) => &unquoted_owned[..=idx],
            None => "",
        };

        scored
            .into_sorted_vec()
            .into_iter()
            .map(|(_, name, is_dir)| {
                let mut path_str = format!("{}{}", user_prefix, name);
                if is_dir {
                    path_str.push('/');
                }

                let final_value = if has_opening_single_quote {
                    format!("'{}", path_str)
                } else if has_opening_double_quote {
                    let escaped = path_str.replace('\\', "\\\\").replace('"', "\\\"");
                    format!("\"{}", escaped)
                } else if path_str.contains(' ')
                    || path_str.contains('\'')
                    || path_str.contains('"')
                    || path_str.contains('\\')
                {
                    let escaped = path_str.replace('\\', "\\\\").replace('"', "\\\"");
                    format!("\"{}\"", escaped)
                } else {
                    path_str
                };

                // Skip per-entry metadata syscalls once superseded; the worker
                // discards this generation anyway.
                let desc = if is_dir {
                    Some("Directory".to_string())
                } else if cancel.is_some_and(|c| c.is_cancelled()) {
                    None
                } else {
                    file_completion_description(&dir_to_read, &name, false)
                };
                let item_kind = if is_dir {
                    CompletionKind::Directory
                } else {
                    CompletionKind::File
                };

                let mut cand = CompletionCandidate::new(
                    final_value,
                    item_kind,
                    TextSpan::new(pos.saturating_sub(word_len), pos),
                );
                if let Some(d) = desc {
                    cand = cand.with_description(d);
                }
                cand
            })
            .collect()
    };

    perform_read()
}

/// Visit every entry, caching only complete small snapshots. Large directories
/// stream through the visitor instead of allocating or caching an unbounded list.
/// False means cancellation or an incomplete read; never publish that snapshot.
fn visit_directory(
    dir: &std::path::Path,
    cancel: Option<&CompletionCancel>,
    mut visit: impl FnMut(&str, bool, fuzzy::FuzzyKind),
) -> bool {
    let cache_key = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(dir))
            .unwrap_or_else(|_| dir.to_path_buf())
    };
    let cached = DIRECTORY_SNAPSHOTS.lock().ok().and_then(|cache| {
        cache
            .get(&cache_key)
            .filter(|snapshot| snapshot.loaded_at.elapsed() < DIRECTORY_SNAPSHOT_TTL)
            .map(|snapshot| Arc::clone(&snapshot.entries))
    });
    if let Some(entries) = cached {
        let kind = fuzzy::choose_kind(entries.len());
        for (index, (name, is_dir)) in entries.iter().enumerate() {
            if index.is_multiple_of(64) && cancel.is_some_and(CompletionCancel::is_cancelled) {
                return false;
            }
            visit(name, *is_dir, kind);
        }
        return !cancel.is_some_and(CompletionCancel::is_cancelled);
    }

    let Ok(mut reader) = std::fs::read_dir(dir) else {
        return false;
    };
    let loaded_at = Instant::now();
    let mut entries = Vec::new();
    let mut complete = false;
    while entries.len() <= MAX_CACHED_DIRECTORY_ENTRIES {
        if entries.len().is_multiple_of(64) && cancel.is_some_and(CompletionCancel::is_cancelled) {
            return false;
        }
        match reader.next() {
            None => {
                complete = true;
                break;
            }
            Some(Err(_)) => return false,
            Some(Ok(entry)) => entries.push(directory_entry(entry)),
        }
    }
    let kind = fuzzy::choose_kind(entries.len());
    for (index, (name, is_dir)) in entries.iter().enumerate() {
        if index.is_multiple_of(64) && cancel.is_some_and(CompletionCancel::is_cancelled) {
            return false;
        }
        visit(name, *is_dir, kind);
    }
    if !complete {
        for (index, entry) in reader.enumerate() {
            if index.is_multiple_of(64) && cancel.is_some_and(CompletionCancel::is_cancelled) {
                return false;
            }
            let Ok(entry) = entry else {
                return false;
            };
            let (name, is_dir) = directory_entry(entry);
            visit(&name, is_dir, kind);
        }
    }
    if cancel.is_some_and(CompletionCancel::is_cancelled) {
        return false;
    }
    if complete && let Ok(mut cache) = DIRECTORY_SNAPSHOTS.lock() {
        if !cache.contains_key(&cache_key) && cache.len() >= MAX_CACHED_DIRECTORIES {
            cache.shift_remove_index(0);
        }
        cache.insert(
            cache_key,
            DirectorySnapshot {
                loaded_at,
                entries: Arc::new(entries),
            },
        );
    }
    true
}

fn directory_entry(entry: std::fs::DirEntry) -> (String, bool) {
    let name = entry.file_name().to_string_lossy().into_owned();
    let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
    (name, is_dir)
}

pub fn file_completion_description(
    dir: &std::path::Path,
    name: &str,
    is_dir: bool,
) -> Option<String> {
    let path = dir.join(name);
    let meta = std::fs::symlink_metadata(&path).ok()?;

    if meta.is_symlink() {
        let target = std::fs::read_link(&path).ok()?;
        return Some(format!("→ {}", target.display()));
    }

    if is_dir {
        let count = std::fs::read_dir(&path).map(|e| e.count()).unwrap_or(0);
        let label = if count == 1 { "item" } else { "items" };
        Some(format!("{} {}", count, label))
    } else {
        let size = meta.len();
        let size_str = if size < 1024 {
            format!("{} B", size)
        } else if size < 1024 * 1024 {
            format!("{:.1} KB", size as f64 / 1024.0)
        } else if size < 1024 * 1024 * 1024 {
            format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.1} GB", size as f64 / (1024.0 * 1024.0 * 1024.0))
        };
        Some(size_str)
    }
}

pub fn get_upstream_keys(upstream: &str, env: &Env) -> Vec<String> {
    let upstream_trimmed = upstream.trim();
    if upstream_trimmed.is_empty() {
        return Vec::new();
    }
    if upstream_trimmed.ends_with("ls") || upstream_trimmed.contains("ls ") {
        return vec![
            "name".to_string(),
            "type".to_string(),
            "size".to_string(),
            "last_modified".to_string(),
        ];
    }
    if let Some(idx) = upstream_trimmed.rfind('$') {
        let var_part = &upstream_trimmed[idx + 1..];
        let var_name: String = var_part
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if let Some(val) = env.vars.read().get(&var_name) {
            return get_val_keys(val);
        }
    }
    Vec::new()
}

pub fn get_val_keys(val: &Val) -> Vec<String> {
    match val {
        Val::Map(map) => map.keys().map(|k| k.as_str().to_string()).collect(),
        Val::List(list) => {
            if let Some(Val::Map(map)) = list.iter().find(|item| matches!(item, Val::Map(_))) {
                map.keys().map(|k| k.as_str().to_string()).collect()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::CompletionGeneration;
    use super::*;

    #[test]
    fn large_directory_matches_beyond_the_snapshot_limit() {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..3000 {
            std::fs::write(directory.path().join(format!("entry{index:04}")), b"").unwrap();
        }
        let target = std::fs::read_dir(directory.path())
            .unwrap()
            .nth(MAX_CACHED_DIRECTORY_ENTRIES + 100)
            .unwrap()
            .unwrap()
            .file_name()
            .to_string_lossy()
            .into_owned();
        let candidates = complete_files_at(&target, target.len(), directory.path(), None);
        assert!(candidates.iter().any(|candidate| candidate.value == target));
        let candidates = complete_files_at("", 0, directory.path(), None);
        assert_eq!(candidates.len(), fuzzy::MAX_RESULTS);
        assert_eq!(candidates.first().unwrap().value, "entry0000");
        assert_eq!(candidates.last().unwrap().value, "entry0049");
        assert!(
            !DIRECTORY_SNAPSHOTS
                .lock()
                .unwrap()
                .contains_key(directory.path()),
            "large directories must not become partial or unbounded cache entries"
        );
    }

    #[test]
    fn cancelled_visit_does_not_publish_a_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("entry"), b"").unwrap();
        let generation = Arc::new(CompletionGeneration::default());
        let cancel = CompletionCancel::new(0, generation.clone());
        assert!(!visit_directory(
            directory.path(),
            Some(&cancel),
            |_, _, _| generation.advance(1)
        ));
        assert!(
            !DIRECTORY_SNAPSHOTS
                .lock()
                .unwrap()
                .contains_key(directory.path())
        );
        assert_eq!(
            complete_files_at("entry", 5, directory.path(), None).len(),
            1
        );
    }
}
