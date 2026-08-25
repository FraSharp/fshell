// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct AliasStateInner {
    last_expansion: Option<(String, String)>,
    recently_expanded: Vec<(usize, usize, String)>,
    feedback_expires: Option<Instant>,
    registered_aliases: HashMap<String, String>,
}

pub struct AliasExpansionState {
    inner: Mutex<AliasStateInner>,
}

impl Default for AliasExpansionState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(AliasStateInner {
                last_expansion: None,
                recently_expanded: Vec::new(),
                feedback_expires: None,
                registered_aliases: HashMap::new(),
            }),
        }
    }
}

impl AliasExpansionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_expansion(
        &self,
        alias_name: &str,
        expansion: &str,
        start_pos: usize,
        end_pos: usize,
    ) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.last_expansion = Some((alias_name.to_string(), expansion.to_string()));
        inner
            .recently_expanded
            .push((start_pos, end_pos, alias_name.to_string()));
        inner.feedback_expires = Some(Instant::now() + Duration::from_millis(300));
    }

    pub fn clear_undo(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.last_expansion = None;
    }

    pub fn check_undo(&self, current_line: &str, cursor_pos: usize) -> Option<String> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (alias_name, expansion) = inner.last_expansion.as_ref()?;

        let expansion_end = cursor_pos;
        let expansion_start = expansion_end.saturating_sub(expansion.len());
        if current_line.get(expansion_start..expansion_end)? == expansion.as_str() {
            return Some(alias_name.clone());
        }
        None
    }

    pub fn active_expansions(&self) -> Vec<(usize, usize, String)> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ts) = inner.feedback_expires
            && Instant::now() > ts
        {
            inner.recently_expanded.clear();
            return Vec::new();
        }
        inner.recently_expanded.clone()
    }

    pub fn is_alias(&self, name: &str) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.registered_aliases.contains_key(name)
    }

    pub fn clear_feedback(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.recently_expanded.clear();
        inner.feedback_expires = None;
    }

    pub fn update_registered(&self, aliases: HashMap<String, String>) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.registered_aliases = aliases;
    }

    pub fn aliases_changed(&self, env_aliases: &[(String, String)]) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.registered_aliases.len() != env_aliases.len() {
            return true;
        }
        for (name, expansion) in env_aliases {
            if inner.registered_aliases.get(name) != Some(expansion) {
                return true;
            }
        }
        false
    }
}

pub fn build_abbreviations(env: &fshell_engine::Env) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let aliases = env.get_all_aliases();
    let builtins = env.get_all_builtins();
    for (name, expansion) in aliases {
        if name.contains(' ') || name.contains('\t') {
            continue;
        }
        if expansion.trim() == name {
            continue;
        }
        if builtins.iter().any(|b| b == &name) {
            continue;
        }
        map.insert(name, expansion);
    }
    map
}

pub fn is_in_command_position(line: &str, cursor: usize) -> bool {
    if cursor > line.len() || !line.is_char_boundary(cursor) {
        return false;
    }
    let before = &line[..cursor];
    let last_delim = before.rfind(['|', ';', '&', '(']);
    let after_delim = match last_delim {
        Some(pos) => &before[pos + 1..],
        None => before,
    };
    let trimmed = after_delim.trim_start();
    !trimmed.contains(' ') && !trimmed.contains('\t')
}

/// Given a buffer and the byte position just after a word, check if that
/// word is an abbreviation (alias in command position, not shadowed by
/// builtin/fn). If so, return the expansion with remaining buffer content.
pub fn expand_abbreviation_at_word_before(
    buffer: &str,
    word_end: usize,
    env: &fshell_engine::Env,
) -> Option<String> {
    if word_end == 0 || !buffer.is_char_boundary(word_end) {
        return None;
    }
    let prefix = &buffer[..word_end];
    let word_start = prefix
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    if word_start >= word_end {
        return None;
    }
    let word = &buffer[word_start..word_end];
    if !is_in_command_position(buffer, word_start) {
        return None;
    }
    if let Some(expansion) = env.get_alias(word)
        && env.get_builtin(word).is_none()
        && !env.fns.read().contains_key(word)
    {
        let rest = &buffer[word_end..];
        if rest.is_empty() {
            Some(expansion)
        } else {
            Some(format!("{}{}", expansion, rest))
        }
    } else {
        None
    }
}

pub fn expand_with_arguments(expansion: &str, user_args: &[String]) -> String {
    if user_args.is_empty() {
        return expansion.to_string();
    }
    let mut result = expansion.to_string();
    if !result.ends_with(' ') {
        result.push(' ');
    }
    for (i, arg) in user_args.iter().enumerate() {
        if i > 0 {
            result.push(' ');
        }
        if arg.contains(' ') || arg.contains('\t') || arg.contains('"') || arg.contains('\\') {
            let escaped = arg.replace('\\', "\\\\").replace('"', "\\\"");
            result.push_str(&format!("\"{}\"", escaped));
        } else {
            result.push_str(arg);
        }
    }
    result
}
