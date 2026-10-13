// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Prompt surface commit and text utilities for the inline REPL.
//!
//! Extracted from the REPL loop (`ftui/mod.rs`): transcript commit, prompt
//! boundary normalization, span slicing/styling helpers and completion
//! application. The loop keeps behavior; this module owns the pieces.

use super::buffer::{self, TextBuffer};
use super::completions::CompletionsManager;
use fshell_terminal::FshellBackend;
use ratatui::Terminal;
use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Copy)]
pub(crate) enum PromptBoundary {
    PreserveCursor,
    NextLine,
}

pub(crate) fn normalize_prompt_boundary(
    out: &mut impl std::io::Write,
    boundary: PromptBoundary,
) -> std::io::Result<()> {
    // Keep the cursor hidden through normalization and the next repaint. Reset
    // style and scrolling margins, restore the saved cursor, and force autowrap
    // on so the inline renderer can rely on terminal-native soft wrapping.
    out.write_all(b"\x1b[?25l\x1b[0m\x1b[s\x1b[r\x1b[u\x1b[?7h")?;
    if matches!(boundary, PromptBoundary::NextLine) {
        out.write_all(b"\r\n")?;
    }
    out.flush()
}

pub(crate) fn command_transcript_lines(prompt: &str, command: &str) -> Vec<String> {
    if command.contains('\n') {
        std::iter::once(prompt.to_string())
            .chain(command.split('\n').map(str::to_string))
            .collect()
    } else {
        vec![format!("{prompt}{command}")]
    }
}

pub(crate) fn commit_surface_lines(
    terminal: &mut Option<Terminal<FshellBackend<std::io::Stdout>>>,
    lines: &[String],
) -> std::io::Result<()> {
    let terminal = terminal.as_mut().ok_or_else(|| {
        std::io::Error::other("cannot commit a prompt without an active inline surface")
    })?;
    // Keep Ratatui's visibility bookkeeping in sync with the backend's
    // transcript commit. Otherwise Terminal::drop shows the old cursor after
    // the backend has hidden it, briefly flashing at the previous position.
    terminal.hide_cursor()?;
    terminal.backend_mut().commit_relative_lines(lines)
}

pub(crate) fn slice_spans_by_column(
    spans: &[Span<'static>],
    start_col: usize,
    len_cols: usize,
) -> Vec<Span<'static>> {
    if len_cols == 0 {
        return Vec::new();
    }
    let mut result = Vec::new();
    let end_col = start_col + len_cols;
    let mut current_col = 0;

    for span in spans {
        let span_width = UnicodeWidthStr::width(span.content.as_ref());
        let span_end_col = current_col + span_width;

        if span_end_col <= start_col || current_col >= end_col {
            current_col = span_end_col;
            continue;
        }

        let local_start = start_col.saturating_sub(current_col);
        let local_end = end_col.saturating_sub(current_col).min(span_width);

        if local_start < local_end {
            let sub = extract_columns_by_width(&span.content, local_start, local_end);
            if !sub.is_empty() {
                result.push(Span::styled(sub, span.style));
            }
        }

        current_col = span_end_col;
    }
    result
}

pub(crate) fn extract_columns_by_width(s: &str, start_col: usize, end_col: usize) -> String {
    let mut out = String::new();
    let mut col = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if w == 0 {
            continue;
        }
        let next_col = col + w;
        if next_col <= start_col {
            col = next_col;
            continue;
        }
        if col >= end_col {
            break;
        }
        if col < start_col && next_col > start_col {
            // Wide char straddles the left edge — replace with single-width
            // space so the column alignment stays correct (R10). Truncating
            // the char to half would corrupt UTF-8 width.
            if start_col < end_col {
                out.push(' ');
            }
            col = next_col;
            if col >= end_col {
                break;
            }
            continue;
        }
        if next_col > end_col && w > 1 {
            // Wide char would overflow right edge — omit rather than clip.
            break;
        }
        out.push(c);
        col = next_col;
        if col >= end_col {
            break;
        }
    }
    out
}

pub(crate) fn apply_style_override(
    spans: Vec<Span<'static>>,
    start_char_idx: usize,
    end_char_idx: usize,
    override_style: Style,
) -> Vec<Span<'static>> {
    let mut result = Vec::new();
    let mut current_char_idx = 0;

    for span in spans {
        let content = span.content.as_ref();
        let chars: Vec<char> = content.chars().collect();
        let span_len = chars.len();

        if current_char_idx + span_len <= start_char_idx || current_char_idx >= end_char_idx {
            // Span is entirely outside the override range
            result.push(span);
        } else {
            // Span intersects with the override range
            // 1. Before override
            let before_len = start_char_idx.saturating_sub(current_char_idx);
            if before_len > 0 {
                let part1: String = chars[0..before_len].iter().collect();
                result.push(Span::styled(part1, span.style));
            }

            // 2. Overlapping override
            let overlap_start = start_char_idx.saturating_sub(current_char_idx);
            let overlap_end = (end_char_idx.saturating_sub(current_char_idx)).min(span_len);
            if overlap_end > overlap_start {
                let part2: String = chars[overlap_start..overlap_end].iter().collect();
                let combined_style = span.style.patch(override_style);
                result.push(Span::styled(part2, combined_style));
            }

            // 3. After override
            let after_start = (end_char_idx.saturating_sub(current_char_idx)).min(span_len);
            if after_start < span_len {
                let part3: String = chars[after_start..].iter().collect();
                result.push(Span::styled(part3, span.style));
            }
        }
        current_char_idx += span_len;
    }
    result
}

pub(crate) fn get_command_for_cursor(text: &str, cursor_char_idx: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor_char_idx.min(chars.len());

    // Find the start of the current pipeline stage or statement by scanning backward
    let mut boundary = cursor;
    while boundary > 0 {
        let prev = chars[boundary - 1];
        if prev == '|' || prev == ';' || prev == '&' || prev == '(' || prev == '{' {
            break;
        }
        boundary -= 1;
    }

    // Find the first word after the boundary
    let mut pos = boundary;
    // Skip leading whitespace
    while pos < chars.len() && chars[pos].is_whitespace() {
        pos += 1;
    }

    // Read the word
    let mut word = String::new();
    while pos < chars.len() {
        let c = chars[pos];
        if c.is_whitespace()
            || c == '|'
            || c == ';'
            || c == '&'
            || c == '('
            || c == ')'
            || c == '{'
            || c == '}'
        {
            break;
        }
        word.push(c);
        pos += 1;
    }

    word
}

// Completion helpers

/// Accept a completion: replace the byte span given by `span` with `value`.
/// Uses the Suggestion span (byte offsets into the current line) rather than
/// scanning backward for whitespace — this is the correct behavior (Bug 1.1).
pub(crate) fn accept_completion_at_span(
    text_buf: &mut buffer::TextBuffer,
    line: &str,
    span: crate::autocomplete::TextSpan,
    value: &str,
    append_whitespace: bool,
) {
    let line_chars: Vec<char> = line.chars().collect();
    let total_chars = line_chars.len();

    // Convert byte span to char indices
    let start_char = byte_offset_to_char_index(line, span.start).min(total_chars);
    let end_char = byte_offset_to_char_index(line, span.end).min(total_chars);

    // Delete the entire span range [start_char..end_char]
    if start_char <= end_char && start_char <= total_chars {
        text_buf.set_cursor(start_char);
        let len_to_delete = end_char.saturating_sub(start_char);
        for _ in 0..len_to_delete {
            text_buf.delete_right();
        }
        text_buf.insert_str(value);
        append_completion_tail(text_buf, value, append_whitespace);
    }
}

/// Convert a byte offset to a character index.
pub(crate) fn byte_offset_to_char_index(s: &str, byte_offset: usize) -> usize {
    let byte_offset = byte_offset.min(s.len());
    s[..byte_offset].chars().count()
}

/// Dispatch function: use span-aware completion when the suggestion has a valid span,
/// otherwise fall back to the legacy word-boundary approach.
pub fn apply_completion(
    text_buf: &mut buffer::TextBuffer,
    line: &str,
    suggestion: &crate::autocomplete::CompletionCandidate,
) {
    let span = suggestion.span;
    let append_ws = suggestion.kind.append_whitespace();
    // A meaningful span has start < end and covers a range that matches the buffer
    if span.start < span.end && span.end <= line.len() {
        accept_completion_at_span(text_buf, line, span, &suggestion.value, append_ws);
    } else {
        // Fallback: delete back to word boundary (legacy behavior for suggestions
        // from completers that don't return proper spans)
        let chars: Vec<char> = line.chars().collect();
        let cursor = text_buf.cursor();
        let last_word_start = chars[..cursor]
            .iter()
            .rposition(|&c| c.is_whitespace() || c == '|' || c == '>' || c == '<')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let len_to_delete = cursor - last_word_start;
        for _ in 0..len_to_delete {
            text_buf.delete_left();
        }
        accept_completion_legacy(text_buf, &suggestion.value, append_ws);
    }
}

/// Legacy accept method — deletes back to the last word boundary.
/// Used only when a Suggestion has no meaningful span (e.g., default span of (0,0)).
pub(crate) fn accept_completion_legacy(
    text_buf: &mut buffer::TextBuffer,
    value: &str,
    append_whitespace: bool,
) {
    text_buf.insert_str(value);
    append_completion_tail(text_buf, value, append_whitespace);
}

pub(crate) fn accept_first_tab_completion(
    comp_mgr: &mut CompletionsManager,
    text_buf: &mut buffer::TextBuffer,
) {
    if !comp_mgr.visible || comp_mgr.suggestions.is_empty() {
        return;
    }

    if comp_mgr.suggestions.len() == 1 {
        if let Some(suggestion) = comp_mgr.suggestions.first() {
            let line = text_buf.text();
            apply_completion(text_buf, &line, suggestion);
        }
        comp_mgr.refresh_after_completion(&text_buf.text(), text_buf.cursor());
    } else if !comp_mgr.prefix_accepted {
        if let Some(prefix) = comp_mgr.longest_common_prefix() {
            let cursor = text_buf.cursor();
            let chars = text_buf.chars();
            let last_word_start = chars[..cursor]
                .iter()
                .rposition(|&c| c.is_whitespace() || c == '|' || c == '>' || c == '<')
                .map(|index| index + 1)
                .unwrap_or(0);
            let current_word: String = chars[last_word_start..cursor].iter().collect();
            if prefix.len() > current_word.len() {
                for c in prefix[current_word.len()..].chars() {
                    text_buf.insert_char(c);
                }
                append_slash_if_dir(text_buf, &prefix);
                comp_mgr.update_from_buffer(text_buf, false);
            }
        }
        comp_mgr.prefix_accepted = true;
    } else {
        comp_mgr.select_next();
    }
}

pub(crate) fn append_slash_if_dir(text_buf: &mut buffer::TextBuffer, value: &str) {
    if text_buf.cursor() == text_buf.len() && !value.ends_with('/') && is_dir_expanded(value) {
        text_buf.insert_char('/');
    }
}

/// Complete the accepted suggestion at the cursor: directories get a trailing
/// '/', command/flag completions that request it get a trailing space, so
/// `git<Tab>` leaves `git ` and the user can keep typing.
pub(crate) fn append_completion_tail(
    text_buf: &mut buffer::TextBuffer,
    value: &str,
    append_whitespace: bool,
) {
    if text_buf.cursor() != text_buf.len() {
        return;
    }
    if !value.ends_with('/') && is_dir_expanded(value) {
        text_buf.insert_char('/');
    } else if append_whitespace && !value.ends_with(' ') {
        text_buf.insert_char(' ');
    }
}

/// Expand a command-position alias in the buffer when its word terminator is
/// typed. Returns true when the buffer changed. See [`crate::alias_expansion`].
pub(crate) fn try_inline_alias_expansion(
    text_buf: &mut TextBuffer,
    env: &fshell_engine::Env,
    alias_state: &crate::alias_expansion::AliasExpansionState,
) -> bool {
    use crate::alias_expansion::{is_in_command_position, should_expand, word_range_before};

    if !env.options.read().expand_aliases {
        return false;
    }
    let chars = text_buf.chars().to_vec();
    let cursor = text_buf.cursor();
    if cursor > chars.len() {
        return false;
    }
    let line: String = chars.iter().collect();
    let cursor_byte = chars[..cursor].iter().map(|c| c.len_utf8()).sum();
    let Some((word_start, word_end)) = word_range_before(&line, cursor_byte) else {
        return false;
    };
    if !is_in_command_position(&line, word_start) {
        return false;
    }
    let word = &line[word_start..word_end];
    let Some(expansion) = should_expand(word, env) else {
        return false;
    };

    let char_start = line[..word_start].chars().count();
    let char_end = char_start + word.chars().count();
    let expansion_chars = expansion.chars().count();

    // Replace the word with the expansion through the buffer's edit path so the
    // change lands in its undo history.
    text_buf.set_selection(char_start, char_end);
    text_buf.delete_selection();
    text_buf.insert_str(&expansion);
    text_buf.set_cursor(char_start + expansion_chars);

    alias_state.record_expansion(
        word,
        &expansion,
        char_start,
        char_start + expansion_chars,
        word_start,
        word_start + expansion.len(),
    );
    true
}

pub(crate) fn is_dir_expanded(path: &str) -> bool {
    let expanded = if let Some(rest) = path.strip_prefix('~') {
        if let Some(home) = std::env::var("HOME")
            .ok()
            .or_else(|| dirs::home_dir().map(|p| p.to_string_lossy().to_string()))
        {
            format!("{home}{rest}")
        } else {
            path.to_string()
        }
    } else {
        path.to_string()
    };
    std::path::Path::new(&expanded).is_dir()
}

// Feature 8: multi-line gutter helpers

/// Split highlighted spans into per-line Vec<Vec<Span>> at '\n' boundaries.
/// Each Span's content may contain embedded '\n' characters. The `start_col`
/// and `len_cols` are passed through to `slice_spans_by_column` for horizontal
/// scroll clipping. Correctly handles leading and consecutive newlines (R3).
pub(crate) fn split_spans_by_newline(
    spans: &[Span<'static>],
    active_line: usize,
    start_col: usize,
    len_cols: usize,
) -> Vec<Vec<Span<'static>>> {
    // Flatten all span contents into a single char stream with per-char style,
    // then split at '\n'. This avoids the multi-span byte_offset bug where
    // consecutive spans each starting with '\n' were miscounted.
    let mut styled_chars: Vec<(char, Style)> = Vec::new();
    for span in spans {
        for ch in span.content.chars() {
            styled_chars.push((ch, span.style));
        }
    }
    if styled_chars.is_empty() {
        return vec![slice_spans_by_column(&[], start_col, len_cols)];
    }
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut cur_text = String::new();
    let mut cur_style: Option<Style> = None;
    let flush =
        |cur_text: &mut String, cur_style: &mut Option<Style>, current: &mut Vec<Span<'static>>| {
            if !cur_text.is_empty()
                && let Some(st) = cur_style.take()
            {
                current.push(Span::styled(std::mem::take(cur_text), st));
            }
        };
    for (ch, style) in styled_chars {
        if ch == '\n' {
            flush(&mut cur_text, &mut cur_style, &mut current);
            let line_idx = lines.len();
            let col = if line_idx == active_line {
                start_col
            } else {
                0
            };
            lines.push(slice_spans_by_column(&current, col, len_cols));
            current = Vec::new();
        } else {
            if cur_style != Some(style) {
                flush(&mut cur_text, &mut cur_style, &mut current);
                cur_style = Some(style);
            }
            cur_text.push(ch);
        }
    }
    flush(&mut cur_text, &mut cur_style, &mut current);
    let line_idx = lines.len();
    let col = if line_idx == active_line {
        start_col
    } else {
        0
    };
    lines.push(slice_spans_by_column(&current, col, len_cols));
    lines
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_spans_by_newline_basic() {
        let spans = vec![Span::raw("fn foo() {\n    echo 1\n}")];
        let lines = split_spans_by_newline(&spans, 0, 0, 80);
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "fn foo() {"
        );
        assert_eq!(
            lines[1]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "    echo 1"
        );
        assert_eq!(
            lines[2]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "}"
        );
    }

    #[test]
    fn test_split_spans_by_newline_active_line_scrolling() {
        let spans = vec![Span::raw(
            "first line is long and scrolling\nsecond line\nthird line",
        )];
        // Line 0 is active and scrolled by 6 columns
        let lines = split_spans_by_newline(&spans, 0, 6, 80);
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "line is long and scrolling"
        );
        // Non-active lines are NOT scrolled
        assert_eq!(
            lines[1]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "second line"
        );
        assert_eq!(
            lines[2]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "third line"
        );
    }

    #[test]
    fn test_split_spans_by_newline_consecutive_and_trailing_newlines() {
        let spans = vec![Span::raw("a\n\nb\n")];
        let lines = split_spans_by_newline(&spans, 0, 0, 80);
        assert_eq!(lines.len(), 4);
        assert_eq!(
            lines[0]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "a"
        );
        assert_eq!(
            lines[1]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            ""
        );
        assert_eq!(
            lines[2]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "b"
        );
        assert_eq!(
            lines[3]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            ""
        );
    }

    #[test]
    fn inline_alias_expansion_edits_and_collapses() {
        let env = fshell_engine::Env::new();
        env.register_alias("gco", "git checkout");
        let state = crate::alias_expansion::AliasExpansionState::new();

        let mut buf = TextBuffer::new();
        buf.insert_str("gco");

        // Typing the word terminator expands the word before it lands.
        assert!(try_inline_alias_expansion(&mut buf, &env, &state));
        assert_eq!(buf.text(), "git checkout");
        assert_eq!(buf.cursor(), "git checkout".chars().count());

        // The terminator the user typed still gets inserted.
        buf.insert_char(' ');
        assert_eq!(buf.text(), "git checkout ");

        // One backspace collapses back to the alias name, keeping the space.
        let collapse = state
            .try_collapse(buf.chars(), buf.cursor())
            .expect("collapse");
        buf.set_selection(collapse.start, collapse.end);
        buf.delete_selection();
        buf.insert_str(&collapse.replacement);
        assert_eq!(buf.text(), "gco ");
    }

    #[test]
    fn inline_alias_expansion_respects_disabled_option() {
        let env = fshell_engine::Env::new();
        env.register_alias("gco", "git checkout");
        env.options.write().expand_aliases = false;
        let state = crate::alias_expansion::AliasExpansionState::new();

        let mut buf = TextBuffer::new();
        buf.insert_str("gco");
        assert!(!try_inline_alias_expansion(&mut buf, &env, &state));
        assert_eq!(buf.text(), "gco");
    }

    #[test]
    fn inline_alias_expansion_skips_argument_position() {
        let env = fshell_engine::Env::new();
        env.register_alias("gco", "git checkout");
        let state = crate::alias_expansion::AliasExpansionState::new();

        let mut buf = TextBuffer::new();
        buf.insert_str("echo gco");
        assert!(!try_inline_alias_expansion(&mut buf, &env, &state));
        assert_eq!(buf.text(), "echo gco");
    }
}
