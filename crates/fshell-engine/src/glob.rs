// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::Env;
use crate::word::ExpandedWord;
use fshell_core::Val;
use std::time::Instant;

pub fn expand_globs(args: Vec<Val>, env: &Env) -> Result<Vec<Val>, String> {
    expand_args_with_words(args, &[], env)
}

/// Expand command arguments, honouring each word's source quoting where known.
///
/// `words` runs parallel to `args`: a `None` entry means the argument has no
/// source word (a bare variable's value, say) and is expanded as before. A word
/// whose pattern characters are all quoted or escaped is data, not a pattern,
/// so it reaches argv untouched however pattern-like it looks — that is what
/// makes `'*.rs'` one literal argument instead of a file list.
pub fn expand_args_with_words(
    args: Vec<Val>,
    words: &[Option<ExpandedWord>],
    env: &Env,
) -> Result<Vec<Val>, String> {
    let (nullglob, nocaseglob) = {
        let opts = env.options.read();
        (opts.nullglob, opts.nocaseglob)
    };

    let mut expanded_args = Vec::new();
    for (index, arg) in args.into_iter().enumerate() {
        let text = match &arg {
            Val::String(s) => s.clone(),
            _ => {
                expanded_args.push(arg);
                continue;
            }
        };
        let word = words.get(index).and_then(Option::as_ref);

        if let Some(word) = word
            && !word.needs_expansion()
        {
            expanded_args.push(arg);
            continue;
        }

        // Prefer the source-derived pattern: its quoted fragments are escaped,
        // so a literal `*` cannot act as a wildcard, while an unquoted one
        // still does.
        let pattern = match word {
            Some(word) => word.glob_pattern(),
            None => text.clone(),
        };
        let pattern = expand_tilde(&pattern, env);

        let braced = expand_braces(&pattern);
        if let [single] = braced.as_slice() {
            // Ask for matches only, so a miss is distinguishable from a match
            // whose name happens to equal the pattern. On a miss the word is
            // used literally — as its own text, never as the pattern, or the
            // escaping added above would leak into argv.
            let matches =
                fshell_core::glob_utils::expand_glob_with_options(single, true, nocaseglob);
            if matches.is_empty() {
                if !nullglob {
                    expanded_args.push(Val::String(text));
                }
            } else {
                for file in matches {
                    expanded_args.push(Val::String(file));
                }
            }
        } else {
            // Brace expansion produced several words; each expands on its own.
            for braced in braced {
                for file in
                    fshell_core::glob_utils::expand_glob_with_options(&braced, nullglob, nocaseglob)
                {
                    expanded_args.push(Val::String(file));
                }
            }
        }
    }

    Ok(expanded_args)
}

fn expand_tilde(s: &str, env: &Env) -> String {
    if s == "~" {
        env.home_dir().to_string_lossy().into_owned()
    } else if let Some(rest) = s.strip_prefix("~/") {
        env.home_dir().join(rest).to_string_lossy().into_owned()
    } else {
        s.to_string()
    }
}

/// If `s` looks like a brace range (e.g. "1..3", "a..z", "01..05"),
/// return the expanded items. Otherwise return `None`.
pub(crate) fn try_expand_range_alternative(s: &str) -> Option<Vec<String>> {
    let (start_str, end_str) = s.split_once("..")?;
    let start_str = start_str.trim();
    let end_str = end_str.trim();

    // Numeric range
    if let (Ok(start_num), Ok(end_num)) = (start_str.parse::<i64>(), end_str.parse::<i64>()) {
        let step = if start_num <= end_num { 1 } else { -1 };
        let width = std::cmp::max(start_str.len(), end_str.len());
        let has_leading_zero = start_str.starts_with('0') || end_str.starts_with('0');
        let mut items = Vec::new();
        let mut curr = start_num;
        loop {
            let formatted = if has_leading_zero {
                format!("{:0width$}", curr, width = width)
            } else {
                curr.to_string()
            };
            items.push(formatted);
            if curr == end_num {
                break;
            }
            curr += step;
        }
        return Some(items);
    }

    // Character range
    if start_str.len() == 1 && end_str.len() == 1 {
        let start_char = start_str.chars().next()?;
        let end_char = end_str.chars().next()?;
        if start_char.is_ascii_alphabetic() && end_char.is_ascii_alphabetic() {
            let start_code = start_char as u8;
            let end_code = end_char as u8;
            if start_code <= end_code {
                Some(
                    (start_code..=end_code)
                        .map(|c| (c as char).to_string())
                        .collect(),
                )
            } else {
                Some(
                    (end_code..=start_code)
                        .rev()
                        .map(|c| (c as char).to_string())
                        .collect(),
                )
            }
        } else {
            None
        }
    } else {
        None
    }
}

pub(crate) fn expand_braces(s: &str) -> Vec<String> {
    let bytes = s.as_bytes();
    let mut depth = 0;
    let mut start_idx = None;
    let mut end_idx = None;

    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'{' {
            if depth == 0 {
                start_idx = Some(i);
            }
            depth += 1;
        } else if bytes[i] == b'}' {
            depth -= 1;
            if depth == 0 {
                end_idx = Some(i);
                break;
            }
        }
        i += 1;
    }

    if let (Some(start), Some(end)) = (start_idx, end_idx) {
        let prefix = &s[..start];
        let suffix = &s[end + 1..];
        let inner = &s[start + 1..end];

        let mut alternatives = Vec::new();
        {
            let mut current = String::new();
            let mut inner_depth = 0;
            let inner_bytes = inner.as_bytes();
            let mut j = 0;
            while j < inner_bytes.len() {
                if inner_bytes[j] == b'\\' {
                    if j + 1 < inner_bytes.len() {
                        current.push(inner_bytes[j + 1] as char);
                    }
                    j += 2;
                    continue;
                }
                if inner_bytes[j] == b'{' {
                    inner_depth += 1;
                } else if inner_bytes[j] == b'}' {
                    inner_depth -= 1;
                }

                if inner_bytes[j] == b',' && inner_depth == 0 {
                    alternatives.push(current.clone());
                    current.clear();
                } else {
                    current.push(inner_bytes[j] as char);
                }
                j += 1;
            }
            alternatives.push(current);
        }

        // Expand any range syntax (e.g. "1..3", "a..z") within each alternative
        let mut ranged = Vec::new();
        let mut is_range = false;
        for alt in alternatives {
            if let Some(range_items) = try_expand_range_alternative(&alt) {
                is_range = true;
                ranged.extend(range_items);
            } else {
                ranged.push(alt);
            }
        }

        // Bash semantics: a brace group with a single item that is not a range
        // is a literal (e.g. `a{b}` stays `a{b}`). Only expand when there is a
        // top-level comma or a `..` range.
        if !is_range && ranged.len() <= 1 {
            return vec![s.to_string()];
        }

        alternatives = ranged;

        let mut results = Vec::new();
        for alt in alternatives {
            let combined = format!("{}{}{}", prefix, alt, suffix);
            results.extend(expand_braces(&combined));
        }
        results
    } else {
        vec![s.to_string()]
    }
}

pub(crate) const SUGGESTION_CACHE_MAX: usize = 256;
/// Entries older than this (in seconds) will be re-computed on next lookup.
pub(crate) const SUGGESTION_CACHE_TTL_SECS: u64 = 5;

struct CacheEntry {
    value: Option<String>,
    timestamp: Instant,
}

pub(crate) struct SuggestionCache {
    entries: std::collections::HashMap<String, CacheEntry>,
    access_order: Vec<String>,
}

impl SuggestionCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            access_order: Vec::new(),
        }
    }

    pub(crate) fn get(&mut self, key: &str) -> Option<Option<String>> {
        self.entries.get(key).map(|e| e.value.clone())
    }

    /// Returns true if the entry exists and is older than TTL.
    pub(crate) fn is_stale(&mut self, key: &str) -> bool {
        self.entries
            .get(key)
            .is_some_and(|e| e.timestamp.elapsed().as_secs() >= SUGGESTION_CACHE_TTL_SECS)
    }

    pub(crate) fn insert(&mut self, key: String, value: Option<String>) {
        let now = Instant::now();
        // If the entry exists and has a different value (e.g. command was just installed),
        // update in-place without changing eviction order.
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.value = value;
            entry.timestamp = now;
            return;
        }
        if self.entries.len() >= SUGGESTION_CACHE_MAX {
            if let Some(oldest) = self.access_order.first().cloned() {
                self.entries.remove(&oldest);
            }
            self.access_order.remove(0);
        }
        self.entries.insert(
            key.clone(),
            CacheEntry {
                value,
                timestamp: now,
            },
        );
        self.access_order.push(key);
    }
}
