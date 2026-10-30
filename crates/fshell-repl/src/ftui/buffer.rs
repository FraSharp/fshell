// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use unicode_width::UnicodeWidthChar;

const MAX_UNDO_DEPTH: usize = 200;
/// After this many edits without a commit, auto-commit for better undo granularity.
const AUTO_COMMIT_THRESHOLD: usize = 20;
/// After this idle duration (ms), any pending edits are auto-committed.
const AUTO_COMMIT_IDLE_MS: u64 = 800;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditOp {
    Insert { pos: usize, text: String },
    Delete { pos: usize, text: String },
}

pub struct TextBuffer {
    chars: Vec<char>,
    cursor: usize,
    undo_stack: Vec<Vec<EditOp>>,
    redo_stack: Vec<Vec<EditOp>>,
    current_transaction: Vec<EditOp>,
    /// Count of individual edits since last commit — used for auto-commit.
    edits_since_commit: usize,
    /// Timestamp of the last edit — used for idle-based auto-commit.
    last_edit_time: std::time::Instant,
    // Selection range: (anchor, head) where anchor <= head
    selection: Option<(usize, usize)>,
    /// Monotonic edit counter. Text-derived work (validation) is cached by
    /// revision so a repaint that changed nothing does not reparse the buffer.
    revision: u64,
}

impl Default for TextBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Normalize clipboard and editor line endings to `\n`.
///
/// Shell input is line-oriented: a carriage return from a CRLF clipboard is a
/// line ending, not data. `$'\r'` and `printf '\r'` remain the ways to embed a
/// literal carriage return in a command.
fn normalize_newlines(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

impl TextBuffer {
    pub fn new() -> Self {
        Self {
            chars: Vec::new(),
            cursor: 0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            current_transaction: Vec::new(),
            edits_since_commit: 0,
            last_edit_time: std::time::Instant::now(),
            selection: None,
            revision: 0,
        }
    }

    /// Monotonic counter bumped by every buffer edit.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn chars(&self) -> &[char] {
        &self.chars
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set_cursor(&mut self, pos: usize) {
        self.cursor = pos.min(self.chars.len());
    }

    /// Auto-commit if enough edits have accumulated or enough idle time has passed.
    /// Call this after every edit operation.
    fn maybe_auto_commit(&mut self) {
        self.edits_since_commit += 1;
        self.last_edit_time = std::time::Instant::now();
        if self.edits_since_commit >= AUTO_COMMIT_THRESHOLD {
            self.commit_transaction();
        }
    }

    /// Try to auto-commit based on idle time since the last edit.
    /// Should be called periodically (e.g., before poll, on idle).
    pub fn try_idle_commit(&mut self) {
        if !self.current_transaction.is_empty()
            && self.last_edit_time.elapsed()
                >= std::time::Duration::from_millis(AUTO_COMMIT_IDLE_MS)
        {
            self.commit_transaction();
        }
    }

    pub fn clear(&mut self) {
        if !self.chars.is_empty() {
            let text = self.text();
            self.commit_transaction();
            self.apply_op(EditOp::Delete { pos: 0, text });
            self.commit_transaction();
        }
    }

    pub fn replace_content(&mut self, s: &str) {
        let s = normalize_newlines(s);
        let old_text = self.text();
        if old_text == s {
            return;
        }
        self.commit_transaction();

        let delete_op = EditOp::Delete {
            pos: 0,
            text: old_text,
        };
        self.apply_op(delete_op.clone());
        self.current_transaction.push(delete_op);

        let insert_op = EditOp::Insert { pos: 0, text: s };
        self.apply_op(insert_op.clone());
        self.current_transaction.push(insert_op);

        self.commit_transaction();
    }

    pub fn prev_grapheme_boundary(&self, char_idx: usize) -> usize {
        if char_idx == 0 {
            return 0;
        }
        let s = self.text();
        let byte_offset = s
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(s.len());
        let mut prev_byte = 0;
        for (b, _) in unicode_segmentation::UnicodeSegmentation::grapheme_indices(s.as_str(), true)
        {
            if b >= byte_offset {
                break;
            }
            prev_byte = b;
        }
        s[..prev_byte].chars().count()
    }

    pub fn next_grapheme_boundary(&self, char_idx: usize) -> usize {
        if char_idx >= self.chars.len() {
            return self.chars.len();
        }
        let s = self.text();
        let byte_offset = s
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(s.len());
        for (b, g) in unicode_segmentation::UnicodeSegmentation::grapheme_indices(s.as_str(), true)
        {
            if b > byte_offset {
                return s[..b].chars().count();
            }
            let end_b = b + g.len();
            if end_b > byte_offset {
                return s[..end_b].chars().count();
            }
        }
        self.chars.len()
    }

    pub fn char_index_to_column(&self, char_idx: usize) -> usize {
        self.char_index_to_column_cached(char_idx, None)
    }

    /// Column calc with optional precomputed line_start to avoid re-scanning.
    /// Long-term the input_loop can keep `current_line_start` and pass it in,
    /// avoiding the O(n) rposition on every cursor move.
    pub fn char_index_to_column_cached(&self, char_idx: usize, line_start: Option<usize>) -> usize {
        let char_idx = char_idx.min(self.chars.len());
        let line_start = line_start.unwrap_or_else(|| {
            self.chars[..char_idx]
                .iter()
                .rposition(|&c| c == '\n')
                .map(|pos| pos + 1)
                .unwrap_or(0)
        });
        let mut col = 0;
        for &c in &self.chars[line_start..char_idx] {
            if c == '\t' {
                col = (col / Self::TAB_STOP + 1) * Self::TAB_STOP;
            } else {
                col += c.width().unwrap_or(0);
            }
        }
        col
    }

    /// Set the tab stop width. Default is 8. This is a module-level constant
    /// that can be changed at any point — it affects all calculations globally.
    /// We keep it as a const for now; if configurability is needed, promote to
    /// a field on TextBuffer.
    const TAB_STOP: usize = 8;

    pub fn column_to_char_index(&self, col: usize) -> usize {
        let mut current_col = 0;
        for (i, &c) in self.chars.iter().enumerate() {
            if c == '\n' {
                return i;
            }
            let w = if c == '\t' {
                let next_tab = (current_col / Self::TAB_STOP + 1) * Self::TAB_STOP;
                next_tab - current_col
            } else {
                c.width().unwrap_or(0)
            };
            if current_col + w > col {
                // Click fell within this character's visual span. Characters
                // cannot be split across cells, so the cursor always lands
                // before the character (tabs land on the tab itself).
                if c == '\t' {
                    return i;
                }
                return i;
            }
            current_col += w;
        }
        self.chars.len()
    }

    pub fn text_column_width(&self) -> usize {
        let mut col = 0;
        for &c in &self.chars {
            if c == '\n' {
                col = 0;
            } else if c == '\t' {
                col = (col / Self::TAB_STOP + 1) * Self::TAB_STOP;
            } else {
                col += c.width().unwrap_or(0);
            }
        }
        col
    }

    pub fn clear_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.current_transaction.clear();
        self.edits_since_commit = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.prev_grapheme_boundary(self.cursor);
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.chars.len() {
            self.cursor = self.next_grapheme_boundary(self.cursor);
        }
    }

    /// Move cursor to the start of the current line (not absolute buffer start).
    pub fn move_to_line_start(&mut self) {
        let line_start = self.chars[..self.cursor]
            .iter()
            .rposition(|&c| c == '\n')
            .map(|pos| pos + 1)
            .unwrap_or(0);
        self.cursor = line_start;
    }

    /// Move cursor to the end of the current line (not absolute buffer end).
    pub fn move_to_line_end(&mut self) {
        let line_end = self.chars[self.cursor..]
            .iter()
            .position(|&c| c == '\n')
            .map(|pos| self.cursor + pos)
            .unwrap_or(self.chars.len());
        self.cursor = line_end;
    }

    pub fn move_to_start(&mut self) {
        self.cursor = 0;
    }

    pub fn move_to_end(&mut self) {
        self.cursor = self.chars.len();
    }

    pub fn move_word_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut pos = self.cursor - 1;
        // Skip initial spaces
        while pos > 0 && self.chars[pos].is_whitespace() {
            pos -= 1;
        }
        // Find beginning of the word
        if pos > 0 {
            let is_alphanumeric = self.chars[pos].is_alphanumeric();
            while pos > 0
                && self.chars[pos - 1].is_alphanumeric() == is_alphanumeric
                && !self.chars[pos - 1].is_whitespace()
            {
                pos -= 1;
            }
        }
        self.cursor = pos;
    }

    pub fn move_word_right(&mut self) {
        let len = self.chars.len();
        if self.cursor >= len {
            return;
        }
        let mut pos = self.cursor;
        let is_alphanumeric = self.chars[pos].is_alphanumeric();
        // Skip current word characters
        while pos < len
            && self.chars[pos].is_alphanumeric() == is_alphanumeric
            && !self.chars[pos].is_whitespace()
        {
            pos += 1;
        }
        // Skip trailing spaces
        while pos < len && self.chars[pos].is_whitespace() {
            pos += 1;
        }
        self.cursor = pos;
    }
    fn char_index_at_column_in_range(&self, start: usize, end: usize, target_col: usize) -> usize {
        let mut cur_col = 0;
        for i in start..end {
            let c = self.chars[i];
            let w = if c == '\t' {
                let next_tab = (cur_col / Self::TAB_STOP + 1) * Self::TAB_STOP;
                next_tab - cur_col
            } else {
                c.width().unwrap_or(0)
            };
            if cur_col + w > target_col {
                return i;
            }
            cur_col += w;
        }
        end
    }

    pub fn move_up(&mut self) -> bool {
        if self.chars.is_empty() {
            return false;
        }
        let mut lines = Vec::new();
        let mut current_start = 0;
        for i in 0..self.chars.len() {
            if self.chars[i] == '\n' {
                lines.push((current_start, i));
                current_start = i + 1;
            }
        }
        lines.push((current_start, self.chars.len()));

        let mut cursor_line = 0;
        for (idx, &(start, end)) in lines.iter().enumerate() {
            if self.cursor >= start && self.cursor <= end {
                cursor_line = idx;
                break;
            }
        }

        if cursor_line == 0 {
            return false;
        }

        let target_col = self.char_index_to_column(self.cursor);
        let (prev_start, prev_end) = lines[cursor_line - 1];
        self.cursor = self.char_index_at_column_in_range(prev_start, prev_end, target_col);
        true
    }

    pub fn move_down(&mut self) -> bool {
        if self.chars.is_empty() {
            return false;
        }
        let mut lines = Vec::new();
        let mut current_start = 0;
        for i in 0..self.chars.len() {
            if self.chars[i] == '\n' {
                lines.push((current_start, i));
                current_start = i + 1;
            }
        }
        lines.push((current_start, self.chars.len()));

        let mut cursor_line = 0;
        for (idx, &(start, end)) in lines.iter().enumerate() {
            if self.cursor >= start && self.cursor <= end {
                cursor_line = idx;
                break;
            }
        }

        if cursor_line + 1 >= lines.len() {
            return false;
        }

        let target_col = self.char_index_to_column(self.cursor);
        let (next_start, next_end) = lines[cursor_line + 1];
        self.cursor = self.char_index_at_column_in_range(next_start, next_end, target_col);
        true
    }

    // Selection

    pub fn has_selection(&self) -> bool {
        self.selection.is_some_and(|(lo, hi)| lo < hi)
    }

    /// Returns the selection range as (lo, hi) with lo <= hi.
    pub fn selection_range(&self) -> Option<(usize, usize)> {
        self.selection
    }

    /// Returns the selected text, or empty string if no selection.
    pub fn selected_text(&self) -> String {
        self.selection
            .filter(|&(lo, hi)| lo < hi)
            .map(|(lo, hi)| self.chars[lo..hi].iter().collect())
            .unwrap_or_default()
    }

    /// Begin selection at the current cursor position.
    pub fn start_selection(&mut self) {
        self.selection = Some((self.cursor, self.cursor));
    }

    /// Extend selection to the current cursor position.
    pub fn extend_selection(&mut self) {
        if let Some((anchor, _)) = self.selection {
            self.selection = Some((anchor.min(self.cursor), anchor.max(self.cursor)));
        }
    }

    /// Clear selection without modifying text.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Set selection between anchor and cursor (for mouse drag — Bug 2.3).
    pub fn set_selection(&mut self, anchor: usize, cursor: usize) {
        if anchor != cursor {
            self.selection = Some((anchor.min(cursor), anchor.max(cursor)));
        } else {
            self.selection = None;
        }
    }

    /// Delete the selected text and return the cursor to the start of the range.
    /// The deletion is recorded as a single EditOp in the current transaction.
    /// Sets cursor to `lo`. Clears selection.
    pub fn delete_selection(&mut self) {
        let Some((lo, hi)) = self.selection.filter(|&(lo, hi)| lo < hi) else {
            return;
        };
        let deleted: String = self.chars[lo..hi].iter().collect();
        let op = EditOp::Delete {
            pos: lo,
            text: deleted,
        };
        self.apply_op(op.clone());
        self.current_transaction.push(op);
        self.cursor = lo;
        self.selection = None;
    }

    /// Wrap the selection with opening and closing characters.
    /// E.g., if text is "hello", selected, and wrap_with('(') is called,
    /// the result is "(hello)".
    fn wrap_selection(&mut self, open: char, close: char) {
        let Some((lo, hi)) = self.selection.filter(|&(lo, hi)| lo < hi) else {
            return;
        };

        // Delete the selected text, then re-insert it wrapped.
        let selected: String = self.chars[lo..hi].iter().collect();

        // Delete selection
        let del_op = EditOp::Delete {
            pos: lo,
            text: selected.clone(),
        };
        self.apply_op(del_op.clone());
        self.current_transaction.push(del_op);

        // Insert wrap: open + selected + close
        let wrapped = format!("{}{}{}", open, selected, close);
        // Buffer positions are character indices, never UTF-8 byte offsets.
        // Using `String::len()` here makes wrapping a non-ASCII selection place
        // the cursor and auto-close markers past the end of the buffer.
        let wrapped_len = wrapped.chars().count();
        let ins_op = EditOp::Insert {
            pos: lo,
            text: wrapped,
        };
        self.apply_op(ins_op.clone());
        self.current_transaction.push(ins_op);

        // Position cursor after the closing character
        self.cursor = lo + wrapped_len;
        self.selection = None;
    }

    /// Insert exactly one character at the cursor.
    ///
    /// Typing never manufactures text the user did not type. In particular it
    /// does not auto-insert closing brackets or quotes: in a shell line those
    /// characters are data as often as syntax (`awk '{print $1}'`, `echo {`,
    /// a triple-quoted `"""` opener), and a synthetic closer changes both what
    /// the user sees and what the validator considers complete. A multi-line
    /// construct continues when Enter finds the buffer incomplete.
    pub fn insert_char(&mut self, c: char) {
        let is_opener = matches!(c, '(' | '[' | '{' | '"' | '\'' | '`');
        if self.has_selection() {
            if is_opener {
                let close = match c {
                    '(' => ')',
                    '[' => ']',
                    '{' => '}',
                    '"' => '"',
                    '\'' => '\'',
                    '`' => '`',
                    _ => unreachable!(),
                };
                self.wrap_selection(c, close);
                self.maybe_auto_commit();
                return;
            }
            self.delete_selection();
        }

        let op = EditOp::Insert {
            pos: self.cursor,
            text: c.to_string(),
        };
        self.apply_op(op.clone());
        self.current_transaction.push(op);

        self.maybe_auto_commit();
    }

    /// Insert `s` at the cursor verbatim.
    ///
    /// This path is used for pasted text and programmatic insertion (clipboard,
    /// completion, history recall) — content that is already complete. A
    /// bracket/quote counter must not reinterpret it: shell input is full of
    /// `[`, `(`, `'` and `"` inside strings and patterns, and the old
    /// "close the net-unclosed delimiters" heuristic appended a stray `]` to
    /// `sed 's/\x1b\[[0-9;]*m//g'` and silently broke it.
    pub fn insert_str(&mut self, s: &str) {
        let s = normalize_newlines(s);
        if s.is_empty() {
            return;
        }
        // If selection is active, replace it
        if self.has_selection() {
            self.delete_selection();
        }
        let op = EditOp::Insert {
            pos: self.cursor,
            text: s,
        };
        self.apply_op(op.clone());
        self.current_transaction.push(op);

        self.maybe_auto_commit();
    }

    pub fn delete_left(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let cursor_before = self.cursor;
        let del_start = self.prev_grapheme_boundary(cursor_before);
        let deleted_text: String = self.chars[del_start..cursor_before].iter().collect();
        let op = EditOp::Delete {
            pos: del_start,
            text: deleted_text,
        };
        self.apply_op(op.clone());
        self.current_transaction.push(op);

        self.maybe_auto_commit();
    }

    pub fn delete_right(&mut self) {
        if self.cursor >= self.chars.len() {
            return;
        }
        let del_end = self.next_grapheme_boundary(self.cursor);
        let deleted_text: String = self.chars[self.cursor..del_end].iter().collect();
        let op = EditOp::Delete {
            pos: self.cursor,
            text: deleted_text,
        };
        self.apply_op(op.clone());
        self.current_transaction.push(op);

        self.maybe_auto_commit();
    }

    pub fn delete_word_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let cursor_before = self.cursor;
        self.move_word_left();
        let target_cursor = self.cursor;
        self.cursor = cursor_before;

        let delete_len = cursor_before - target_cursor;
        if delete_len > 0 {
            let deleted_str: String = self.chars[target_cursor..cursor_before].iter().collect();
            let op = EditOp::Delete {
                pos: target_cursor,
                text: deleted_str,
            };
            self.apply_op(op.clone());
            self.current_transaction.push(op);
        }

        self.maybe_auto_commit();
    }

    pub fn commit_transaction(&mut self) {
        if !self.current_transaction.is_empty() {
            let tx = std::mem::take(&mut self.current_transaction);
            self.undo_stack.push(tx);
            if self.undo_stack.len() > MAX_UNDO_DEPTH {
                self.undo_stack.remove(0);
            }
            self.redo_stack.clear();
            self.edits_since_commit = 0;
        }
    }

    pub fn undo(&mut self) {
        self.commit_transaction();
        if let Some(tx) = self.undo_stack.pop() {
            let mut redo_tx = Vec::new();
            for op in tx.into_iter().rev() {
                let inverse = match op {
                    EditOp::Insert { pos, text } => EditOp::Delete { pos, text },
                    EditOp::Delete { pos, text } => EditOp::Insert { pos, text },
                };
                self.apply_op(inverse.clone());
                redo_tx.push(inverse);
            }
            self.redo_stack.push(redo_tx);
        }
    }

    pub fn redo(&mut self) {
        if let Some(tx) = self.redo_stack.pop() {
            let mut undo_tx = Vec::new();
            for op in tx.into_iter().rev() {
                let inverse = match op {
                    EditOp::Insert { pos, text } => EditOp::Delete { pos, text },
                    EditOp::Delete { pos, text } => EditOp::Insert { pos, text },
                };
                self.apply_op(inverse.clone());
                undo_tx.push(inverse);
            }
            self.undo_stack.push(undo_tx);
        }
    }

    fn apply_op(&mut self, op: EditOp) {
        self.revision = self.revision.wrapping_add(1);
        match op {
            EditOp::Insert { pos, text } => {
                let text_chars: Vec<char> = text.chars().collect();
                let insert_len = text_chars.len();
                if pos <= self.chars.len() {
                    self.chars.splice(pos..pos, text_chars);
                    self.cursor = pos + insert_len;
                }
            }
            EditOp::Delete { pos, text } => {
                let delete_len = text.chars().count();
                if pos + delete_len <= self.chars.len() {
                    self.chars.drain(pos..pos + delete_len);
                    self.cursor = pos;
                }
            }
        }
    }
}

impl std::str::FromStr for TextBuffer {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut buf = Self::new();
        buf.insert_str(s);
        buf.clear_history();
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paste_inserts_verbatim_without_synthetic_closers() {
        let mut buf = TextBuffer::new();
        // Two `[` and one `]` inside a quoted sed program used to make the
        // old bracket counter append a stray `]` to the pasted command.
        buf.insert_str("sed 's/\\x1b\\[[0-9;]*m//g' file");
        assert_eq!(buf.text(), "sed 's/\\x1b\\[[0-9;]*m//g' file");

        let mut buf = TextBuffer::new();
        buf.insert_str("for f in a b; do echo $f; done\n");
        assert_eq!(buf.text(), "for f in a b; do echo $f; done\n");
    }

    #[test]
    fn test_revision_tracks_every_edit() {
        let mut buf = TextBuffer::new();
        let initial = buf.revision();
        buf.insert_char('a');
        let after_char = buf.revision();
        assert!(after_char > initial);
        buf.insert_str("bc");
        let after_str = buf.revision();
        assert!(after_str > after_char);
        buf.delete_left();
        let after_delete = buf.revision();
        assert!(after_delete > after_str);
        buf.clear();
        assert!(buf.revision() > after_delete);
    }

    #[test]
    fn test_insert_str_normalizes_line_endings() {
        let mut buf = TextBuffer::new();
        buf.insert_str("echo one\r\necho two\r");
        assert_eq!(buf.text(), "echo one\necho two\n");
    }

    #[test]
    fn test_replace_content_normalizes_line_endings() {
        let mut buf = TextBuffer::new();
        buf.replace_content("echo one\r\necho two\r\n");
        assert_eq!(buf.text(), "echo one\necho two\n");
    }

    #[test]
    fn test_normalize_newlines_leaves_lf_alone() {
        assert_eq!(normalize_newlines("a\nb\n"), "a\nb\n");
        assert_eq!(normalize_newlines("a\r\nb\r\n"), "a\nb\n");
        assert_eq!(normalize_newlines("a\rb"), "a\nb");
    }

    #[test]
    fn test_typing_inserts_exactly_one_character() {
        let mut buf = TextBuffer::new();
        for c in ['(', '[', '{', '"', '\'', '`'] {
            buf.insert_char(c);
        }
        assert_eq!(buf.text(), "([{\"'`");
        assert_eq!(buf.cursor(), 6);
    }

    #[test]
    fn test_typing_three_quotes_keeps_three_quotes() {
        let mut buf = TextBuffer::new();
        for c in ['"', '"', '"'] {
            buf.insert_char(c);
        }
        assert_eq!(buf.text(), "\"\"\"");
        assert_eq!(buf.cursor(), 3);
    }

    #[test]
    fn test_backspace_deletes_exactly_one_character() {
        let mut buf = TextBuffer::new();
        buf.insert_char('(');
        buf.insert_char(')');
        assert_eq!(buf.text(), "()");

        buf.delete_left();
        assert_eq!(buf.text(), "(");
        buf.delete_left();
        assert_eq!(buf.text(), "");
    }

    #[test]
    fn test_typing_a_closer_replaces_the_selection() {
        let mut buf = TextBuffer::new();
        buf.insert_str("hello");
        buf.set_cursor(0);
        buf.start_selection();
        buf.set_cursor(5);
        buf.extend_selection();

        buf.insert_char(')');
        assert_eq!(buf.text(), ")");
        assert_eq!(buf.cursor(), 1);
        assert!(!buf.has_selection());
    }

    #[test]
    fn test_wrap_selection_with_brackets() {
        let mut buf = TextBuffer::new();
        buf.insert_str("hello world");
        // Select "world" starting at index 6
        buf.set_cursor(6);
        buf.start_selection();
        buf.set_cursor(11);
        buf.extend_selection();
        assert_eq!(buf.selected_text(), "world", "selection should be 'world'");

        // Type '(' -> should wrap
        buf.insert_char('(');
        assert_eq!(buf.text(), "hello (world)", "text should have (world)");
        // Cursor should be after the closing ')' at the end (len 13)
        assert_eq!(buf.cursor(), 13, "cursor should be after ')'");
        assert!(!buf.has_selection());
    }

    #[test]
    fn test_wrap_unicode_selection_uses_character_indices() {
        let mut buf = TextBuffer::new();
        buf.insert_str("café 👋");
        buf.set_cursor(3);
        buf.start_selection();
        buf.set_cursor(4);
        buf.extend_selection();

        buf.insert_char('(');

        assert_eq!(buf.text(), "caf(é) 👋");
        assert_eq!(buf.cursor(), 6);
        assert!(buf.cursor() <= buf.len());
    }

    #[test]
    fn test_undo_with_auto_commit() {
        let mut buf = TextBuffer::new();
        buf.insert_char('h');
        buf.insert_char('e');
        buf.insert_char('l');
        buf.insert_char('l');
        buf.insert_char('o');

        // After AUTO_COMMIT_THRESHOLD (20) edits, but we only did 5, so no auto-commit yet
        assert_eq!(buf.edits_since_commit, 5);

        // commit_transaction moves to undo stack
        buf.commit_transaction();

        buf.insert_str(" world");
        buf.commit_transaction();

        assert_eq!(buf.text(), "hello world");
        buf.undo();
        assert_eq!(buf.text(), "hello");
    }

    #[test]
    fn test_delete_word_left() {
        let mut buf = TextBuffer::new();
        buf.replace_content("hello world  ");
        buf.delete_word_left();
        assert_eq!(buf.text(), "hello ");

        let mut buf2 = TextBuffer::new();
        buf2.replace_content("git checkout main");
        buf2.delete_word_left();
        assert_eq!(buf2.text(), "git checkout ");

        let mut buf3 = TextBuffer::new();
        buf3.replace_content("a");
        buf3.delete_word_left();
        assert_eq!(buf3.text(), "");
    }

    #[test]
    fn test_replace_content_undo_redo() {
        let mut buf = TextBuffer::new();
        buf.insert_str("hello");
        buf.commit_transaction();

        buf.replace_content("world");
        assert_eq!(buf.text(), "world");

        buf.undo();
        assert_eq!(buf.text(), "hello");

        buf.redo();
        assert_eq!(buf.text(), "world");
    }

    #[test]
    fn test_move_up_down() {
        let mut buf = TextBuffer::new();
        buf.insert_str("hello\nworld");
        assert_eq!(buf.text(), "hello\nworld");

        buf.set_cursor(11);
        assert!(buf.move_up());
        assert_eq!(buf.cursor(), 5);

        assert!(!buf.move_up());
        assert_eq!(buf.cursor(), 5);

        buf.set_cursor(8);
        assert!(buf.move_up());
        assert_eq!(buf.cursor(), 2);

        assert!(buf.move_down());
        assert_eq!(buf.cursor(), 8);
    }

    #[test]
    fn test_multiline_char_index_to_column_resets_at_newline() {
        let mut buf = TextBuffer::new();
        buf.insert_str("first line\nsecond line");
        assert_eq!(buf.char_index_to_column(11), 0);
        assert_eq!(buf.char_index_to_column(12), 1);
    }

    #[test]
    fn test_tabstop_column_calculation() {
        let mut buf = TextBuffer::new();
        buf.insert_str("a\tb");
        assert_eq!(buf.char_index_to_column(1), 1);
        // Tab at col 1 -> expands to col 8
        assert_eq!(buf.char_index_to_column(2), 8);
        // 'b' at col 9
        assert_eq!(buf.char_index_to_column(3), 9);
    }

    #[test]
    fn test_grapheme_cluster_movement() {
        let mut buf = TextBuffer::new();
        buf.insert_str("hi 👨‍👩‍👧‍👦 bye");
        buf.move_to_end();
        buf.move_left();
        buf.move_left();
        buf.move_left();
        buf.move_left();
        let before_emoji = buf.cursor();
        buf.move_left();
        let after_emoji = buf.cursor();
        assert!(before_emoji - after_emoji > 1);
    }

    #[test]
    fn test_move_to_line_start_end() {
        let mut buf = TextBuffer::new();
        buf.insert_str("hello\nworld\nfoo");
        buf.set_cursor(8); // 'o' in "world"
        buf.move_to_line_start();
        assert_eq!(buf.cursor(), 6); // 'w' in "world"
        buf.move_to_line_end();
        assert_eq!(buf.cursor(), 11); // end of "world"
    }
}
