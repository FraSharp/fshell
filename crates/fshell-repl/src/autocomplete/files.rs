// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Filesystem path drilling, fuzzy matching, and metadata lookups.

use super::types::{CompletionCandidate, CompletionKind, TextSpan};
use crate::fuzzy;
use fshell_core::Val;
use fshell_engine::Env;

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

    let path = std::path::PathBuf::from(&expanded);
    let (search_dir, file_prefix) = if expanded.ends_with('/') {
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

    let perform_read = move || {
        let mut entries: Vec<(String, bool)> = Vec::new();
        if let Ok(dir_entries) = std::fs::read_dir(&dir_to_read) {
            for entry in dir_entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                entries.push((name, is_dir));
                if entries.len() >= 2000 {
                    break;
                }
            }
        }

        if entries.is_empty() {
            return Vec::new();
        }

        let kind = fuzzy::choose_kind(entries.len());
        let prepared = fuzzy::PreparedQuery::new(&file_prefix);
        let mut scored: Vec<(isize, String, bool)> = entries
            .into_iter()
            .filter_map(|(name, is_dir)| {
                fuzzy::fuzzy_score_prepared(&prepared, &name, kind).map(|s| (s, name, is_dir))
            })
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        scored.truncate(fuzzy::MAX_RESULTS);

        let user_prefix = match unquoted_owned.rfind('/') {
            Some(idx) => &unquoted_owned[..=idx],
            None => "",
        };

        scored
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

                let desc = file_completion_description(&dir_to_read, &name, is_dir);
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
