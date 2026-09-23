// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Inline alias expansion for the line editor.
//!
//! When the word terminator (a space, or `;` `|` `&`) is typed after a
//! command-position word that names an alias, the word is replaced in the
//! buffer with the alias expansion, so the user sees exactly what will run.
//! One backspace immediately afterwards collapses it back to the alias name.
//!
//! The actual buffer edits live in the editor; this module owns the state that
//! makes the collapse safe (it only fires when the recorded expansion is still
//! unchanged in the buffer) and the position/word helpers.

use fshell_core::lock::Mutex;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long the expanded region stays highlighted after an expansion.
const FEEDBACK_TTL: Duration = Duration::from_millis(1500);

/// A recorded expansion, in both char indices (for the buffer) and byte offsets
/// (for the highlighter, which walks byte ranges).
struct ExpansionRecord {
    name: String,
    expansion: String,
    char_start: usize,
    char_end: usize,
    byte_start: usize,
    byte_end: usize,
}

/// A pending backspace collapse: replace `chars[start..end]` with `replacement`.
pub struct Collapse {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

struct AliasStateInner {
    last_expansion: Option<ExpansionRecord>,
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

    /// Record the expansion that was just written into the buffer.
    #[allow(clippy::too_many_arguments)]
    pub fn record_expansion(
        &self,
        name: &str,
        expansion: &str,
        char_start: usize,
        char_end: usize,
        byte_start: usize,
        byte_end: usize,
    ) {
        let mut inner = self.inner.lock();
        inner.last_expansion = Some(ExpansionRecord {
            name: name.to_string(),
            expansion: expansion.to_string(),
            char_start,
            char_end,
            byte_start,
            byte_end,
        });
        inner.feedback_expires = Some(Instant::now() + FEEDBACK_TTL);
    }

    /// Forget the recorded expansion (after a collapse, or a new command line).
    pub fn clear_expansion(&self) {
        let mut inner = self.inner.lock();
        inner.last_expansion = None;
        inner.feedback_expires = None;
    }

    /// If backspacing at `cursor` should collapse an expansion, return the
    /// replacement. Fires only when the recorded expansion text is still
    /// present in the buffer unchanged (so editing inside it disables it), and
    /// allows for a single trailing space typed as the word terminator.
    pub fn try_collapse(&self, chars: &[char], cursor: usize) -> Option<Collapse> {
        let inner = self.inner.lock();
        let rec = inner.last_expansion.as_ref()?;
        let expansion: Vec<char> = rec.expansion.chars().collect();
        let intact = chars.get(rec.char_start..rec.char_end) == Some(expansion.as_slice());

        if cursor == rec.char_end && intact {
            return Some(Collapse {
                start: rec.char_start,
                end: rec.char_end,
                replacement: rec.name.clone(),
            });
        }
        if cursor == rec.char_end + 1 && chars.get(rec.char_end).copied() == Some(' ') && intact {
            return Some(Collapse {
                start: rec.char_start,
                end: rec.char_end + 1,
                replacement: format!("{} ", rec.name),
            });
        }
        None
    }

    /// Byte ranges of the currently highlighted expansion, for the highlighter.
    pub fn active_expansions(&self) -> Vec<(usize, usize, String)> {
        let mut inner = self.inner.lock();
        if let Some(ts) = inner.feedback_expires
            && Instant::now() > ts
        {
            inner.last_expansion = None;
            inner.feedback_expires = None;
            return Vec::new();
        }
        match &inner.last_expansion {
            Some(rec) => vec![(rec.byte_start, rec.byte_end, rec.name.clone())],
            None => Vec::new(),
        }
    }

    pub fn is_alias(&self, name: &str) -> bool {
        let inner = self.inner.lock();
        inner.registered_aliases.contains_key(name)
    }

    pub fn update_registered(&self, aliases: HashMap<String, String>) {
        let mut inner = self.inner.lock();
        inner.registered_aliases = aliases;
    }

    /// True when the alias set differs from what is cached, so the editor can
    /// refresh after `alias`/`unalias` mutates it at runtime.
    pub fn aliases_changed(&self, env_aliases: &[(String, String)]) -> bool {
        let inner = self.inner.lock();
        if inner.registered_aliases.len() != env_aliases.len() {
            return true;
        }
        env_aliases
            .iter()
            .any(|(name, expansion)| inner.registered_aliases.get(name) != Some(expansion))
    }
}

/// Characters that end a command word for expansion purposes.
pub fn is_word_terminator(c: char) -> bool {
    matches!(c, ' ' | '\t' | ';' | '|' | '&')
}

/// The byte range `[start, end)` of the word ending at `end`, where a word is a
/// run of characters that are neither whitespace nor `; | & ( {`.
pub fn word_range_before(line: &str, end: usize) -> Option<(usize, usize)> {
    if end > line.len() || !line.is_char_boundary(end) {
        return None;
    }
    let mut start = end;
    while start > 0 {
        let prev = line[..start].chars().next_back()?;
        if prev.is_whitespace() || matches!(prev, ';' | '|' | '&' | '(' | '{') {
            break;
        }
        start -= prev.len_utf8();
    }
    if start == end {
        return None;
    }
    Some((start, end))
}

/// True when the word starting at byte offset `word_start` is the first token
/// of its statement (after `; | & ( {` or a newline), not inside a quote or a
/// comment. A `\`-escaped word is not in command position either (the backslash
/// is part of the word, so the alias lookup misses).
pub fn is_in_command_position(line: &str, word_start: usize) -> bool {
    if word_start > line.len() || !line.is_char_boundary(word_start) {
        return false;
    }
    let prefix = &line[..word_start];
    let mut in_single = false;
    let mut in_double = false;
    let mut in_comment = false;
    let mut segment_start = 0usize;
    let mut chars = prefix.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        if in_single {
            if c == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if c == '\\' {
                chars.next();
            } else if c == '"' {
                in_double = false;
            }
            continue;
        }
        if in_comment {
            if c == '\n' {
                in_comment = false;
                segment_start = i + c.len_utf8();
            }
            continue;
        }
        match c {
            '\'' => in_single = true,
            '"' => in_double = true,
            '\\' => {
                chars.next();
            }
            '#' => {
                let at_word_start = i == 0
                    || prefix[..i]
                        .chars()
                        .next_back()
                        .is_some_and(|p| p.is_whitespace());
                if at_word_start {
                    in_comment = true;
                }
            }
            ';' | '|' | '&' | '(' | '{' | '\n' => {
                segment_start = i + c.len_utf8();
            }
            _ => {}
        }
    }

    if in_single || in_double || in_comment {
        return false;
    }
    line[segment_start..word_start].trim().is_empty()
}

/// If the word ending at `word_end` is a command-position alias, return the
/// line with that word replaced by its expansion. Kept for callers/tests that
/// want the whole rewritten line at once.
pub fn expand_abbreviation_at_word_before(
    buffer: &str,
    word_end: usize,
    env: &fshell_engine::Env,
) -> Option<String> {
    let (start, end) = word_range_before(buffer, word_end)?;
    let word = &buffer[start..end];
    if !is_in_command_position(buffer, start) {
        return None;
    }
    let expansion = should_expand(word, env)?;
    let rest = &buffer[end..];
    Some(format!("{expansion}{rest}"))
}

/// Returns the expansion for `word` when it should be expanded inline: an
/// alias that doesn't shadow a builtin or user function and isn't a no-op.
pub fn should_expand(word: &str, env: &fshell_engine::Env) -> Option<String> {
    if env.get_builtin(word).is_some() || env.fns.read().contains_key(word) {
        return None;
    }
    let expansion = env.get_alias(word)?;
    if expansion.trim() == word {
        return None;
    }
    Some(expansion)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(aliases: &[(&str, &str)]) -> fshell_engine::Env {
        let env = fshell_engine::Env::new();
        for (name, expansion) in aliases {
            env.register_alias(name, expansion);
        }
        env
    }

    #[test]
    fn word_range_before_handles_delimiters_and_multibyte() {
        assert_eq!(word_range_before("gco", 3), Some((0, 3)));
        assert_eq!(word_range_before("echo hi", 7), Some((5, 7)));
        assert_eq!(word_range_before("cat x|gco", 9), Some((6, 9)));
        assert_eq!(word_range_before("gco|", 3), Some((0, 3)));
        assert_eq!(word_range_before("(gco", 4), Some((1, 4)));
        // No word: cursor right after a separator / whitespace.
        assert_eq!(word_range_before("cat |", 5), None);
        assert_eq!(word_range_before("echo ", 5), None);
        // Multi-byte input must not panic.
        assert_eq!(word_range_before("ò", 2), Some((0, 2)));
    }

    #[test]
    fn command_position_detection() {
        assert!(is_in_command_position("gco", 0));
        assert!(is_in_command_position("echo hi; gco", 9));
        assert!(is_in_command_position("cat x | gco", 8));
        assert!(is_in_command_position("a && gco", 5));
        assert!(!is_in_command_position("echo gco", 5)); // argument
        assert!(!is_in_command_position("echo \"a gco", 7)); // inside quotes
        assert!(!is_in_command_position("echo hi # gco", 10)); // comment
    }

    #[test]
    fn expand_abbreviation_at_word_before_rewrites_line() {
        let env = env_with(&[("gco", "git checkout")]);
        assert_eq!(
            expand_abbreviation_at_word_before("gco", 3, &env).as_deref(),
            Some("git checkout")
        );
        assert_eq!(
            expand_abbreviation_at_word_before("gco main", 3, &env).as_deref(),
            Some("git checkout main")
        );
        // Argument position: not expanded.
        assert_eq!(
            expand_abbreviation_at_word_before("echo gco", 8, &env),
            None
        );
    }

    #[test]
    fn should_expand_skips_unknown_and_self_alias() {
        let env = env_with(&[("gco", "git checkout"), ("x", "x")]);
        assert_eq!(should_expand("gco", &env).as_deref(), Some("git checkout"));
        assert!(should_expand("nope", &env).is_none());
        assert!(should_expand("x", &env).is_none());
    }

    #[test]
    fn collapse_restores_name_with_and_without_trailing_space() {
        let state = AliasExpansionState::new();
        state.record_expansion("gco", "git checkout", 0, 12, 0, 12);

        let with_space: Vec<char> = "git checkout ".chars().collect();
        let collapse = state.try_collapse(&with_space, 13).expect("collapse");
        assert_eq!((collapse.start, collapse.end), (0, 13));
        assert_eq!(collapse.replacement, "gco ");

        let no_space: Vec<char> = "git checkout".chars().collect();
        let collapse = state.try_collapse(&no_space, 12).expect("collapse");
        assert_eq!((collapse.start, collapse.end), (0, 12));
        assert_eq!(collapse.replacement, "gco");
    }

    #[test]
    fn collapse_does_nothing_after_editing_inside_the_expansion() {
        let state = AliasExpansionState::new();
        state.record_expansion("gco", "git checkout", 0, 12, 0, 12);
        // The user changed the expansion, so backspace is a plain backspace.
        let edited: Vec<char> = "git checkouX ".chars().collect();
        assert!(state.try_collapse(&edited, 13).is_none());
    }
}
