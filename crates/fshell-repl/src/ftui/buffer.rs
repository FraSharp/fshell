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
    // Track indices of auto-inserted closers for skip-over on matching character typed.
    // Indexed by the character position in `chars`.
    // Only contains entries that were genuinely auto-inserted by insert_char/insert_str,
    // NOT pairs that were manually typed or restored from undo/redo.
    auto_inserted_closers: Vec<usize>,
    // Track which opener each auto-inserted closer belongs to.
    auto_inserted_pairs: Vec<(usize, usize)>,
    // Selection range: (anchor, head) where anchor <= head
    selection: Option<(usize, usize)>,
}

impl Default for TextBuffer {
    fn default() -> Self {
        Self::new()
    }
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
            auto_inserted_closers: Vec::new(),
            auto_inserted_pairs: Vec::new(),
            selection: None,
        }
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

        let insert_op = EditOp::Insert {
            pos: 0,
            text: s.to_string(),
        };
        self.apply_op(insert_op.clone());
        self.current_transaction.push(insert_op);

        self.commit_transaction();
        // Do NOT rebuild auto-close state here — auto-inserted markers should only
        // track what was genuinely auto-inserted, not pairs that exist in the buffer.
        // The skip-over feature works by tracking at insert time, not by scanning.
        self.auto_inserted_closers.clear();
        self.auto_inserted_pairs.clear();
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
        self.auto_inserted_closers.clear();
        self.auto_inserted_pairs.clear();
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
        let wrapped_len = wrapped.len();
        let ins_op = EditOp::Insert {
            pos: lo,
            text: wrapped,
        };
        self.apply_op(ins_op.clone());
        self.current_transaction.push(ins_op);

        // Position cursor after the closing character
        self.cursor = lo + wrapped_len;
        self.selection = None;

        // Track the closer for skip-over if auto-close pair
        if matches!(
            (open, close),
            ('(', ')') | ('[', ']') | ('{', '}') | ('"', '"') | ('\'', '\'') | ('`', '`')
        ) {
            let closer_idx = lo + wrapped_len - 1;
            let opener_idx = lo;
            let delta = wrapped_len.saturating_sub(selected.len());
            // Don't shift existing auto-inserted pairs since the entire operation
            // replaced a range starting at `lo`.
            for pair in self.auto_inserted_pairs.iter_mut() {
                if pair.0 >= lo {
                    pair.0 = pair.0.saturating_add(delta);
                }
                if pair.1 >= lo {
                    pair.1 = pair.1.saturating_add(delta);
                }
            }
            for idx in self.auto_inserted_closers.iter_mut() {
                if *idx >= lo {
                    *idx = idx.saturating_add(delta);
                }
            }
            self.auto_inserted_closers.push(closer_idx);
            self.auto_inserted_pairs.push((opener_idx, closer_idx));
        }
    }

    pub fn insert_char(&mut self, c: char) {
        // Selection handling
        // If the user types a bracket/quote while text is selected, wrap the selection.
        // If they type any other character, replace the selection.
        let is_closer = matches!(c, ')' | ']' | '}' | '"' | '\'' | '`');
        let is_opener = matches!(c, '(' | '[' | '{' | '"' | '\'' | '`');

        if self.has_selection() && is_opener {
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
        } else if self.has_selection() && !is_closer {
            self.delete_selection();
        }
        // If selection is active and user types a closer ()), we just delete the
        // selection and insert the closer — for ', ", ` we let the normal flow handle it.

        // Smart skip-over logic
        // If cursor is on a genuinely auto-inserted closer and the user types the
        // matching character, just advance past it.
        if self.cursor < self.chars.len()
            && self.chars[self.cursor] == c
            && self.auto_inserted_closers.contains(&self.cursor)
            && is_closer
        {
            self.auto_inserted_closers.retain(|&idx| idx != self.cursor);
            self.auto_inserted_pairs
                .retain(|&(_, closer)| closer != self.cursor);
            self.cursor += 1;
            return;
        }

        let cursor_before = self.cursor;

        // Apply normal insertion
        let op = EditOp::Insert {
            pos: cursor_before,
            text: c.to_string(),
        };
        self.apply_op(op.clone());
        self.current_transaction.push(op);

        // Auto-close logic
        // Only auto-close if there's NO selection (we already handled that above)
        let closer = match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            '"' => Some('"'),
            '\'' => Some('\''),
            '`' => Some('`'),
            _ => None,
        };

        if let Some(cl) = closer {
            let next_char = self.chars.get(self.cursor);
            let should_autoclose = match c {
                '\'' | '"' | '`' => next_char.is_none_or(|nc| !nc.is_alphanumeric()),
                _ => true,
            };

            if should_autoclose {
                let close_op = EditOp::Insert {
                    pos: self.cursor,
                    text: cl.to_string(),
                };
                self.apply_op(close_op.clone());
                self.current_transaction.push(close_op);

                let closer_idx = cursor_before + 1;
                let opener_idx = cursor_before;

                // Shift existing auto-inserted closer indices right by 2
                for idx in self.auto_inserted_closers.iter_mut() {
                    if *idx >= cursor_before {
                        *idx += 2;
                    }
                }
                for (opener, closer) in self.auto_inserted_pairs.iter_mut() {
                    if *opener >= cursor_before {
                        *opener += 2;
                    }
                    if *closer >= cursor_before {
                        *closer += 2;
                    }
                }
                self.auto_inserted_closers.push(closer_idx);
                self.auto_inserted_pairs.push((opener_idx, closer_idx));
                // Cursor stays inside: between opener and closer
                self.cursor = cursor_before + 1;
            } else {
                // Shift indices right by 1
                for idx in self.auto_inserted_closers.iter_mut() {
                    if *idx >= cursor_before {
                        *idx += 1;
                    }
                }
                for (opener, closer) in self.auto_inserted_pairs.iter_mut() {
                    if *opener >= cursor_before {
                        *opener += 1;
                    }
                    if *closer >= cursor_before {
                        *closer += 1;
                    }
                }
            }
        } else {
            // Shift indices right by 1
            for idx in self.auto_inserted_closers.iter_mut() {
                if *idx >= cursor_before {
                    *idx += 1;
                }
            }
            for (opener, closer) in self.auto_inserted_pairs.iter_mut() {
                if *opener >= cursor_before {
                    *opener += 1;
                }
                if *closer >= cursor_before {
                    *closer += 1;
                }
            }
        }

        self.maybe_auto_commit();
    }

    pub fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        // If selection is active, replace it
        if self.has_selection() {
            self.delete_selection();
        }
        let cursor_before = self.cursor;

        // Scan pasted string for unbalanced open brackets and only auto-close
        // the net-unclosed ones. Balanced strings ("foo") get no extra closers.
        let mut unbalanced: std::collections::HashMap<char, i64> = std::collections::HashMap::new();
        for c in s.chars() {
            match c {
                '(' => *unbalanced.entry('(').or_insert(0) += 1,
                ')' => {
                    if let Some(d) = unbalanced.get_mut(&'(')
                        && *d > 0
                    {
                        *d -= 1;
                    }
                }
                '[' => *unbalanced.entry('[').or_insert(0) += 1,
                ']' => {
                    if let Some(d) = unbalanced.get_mut(&'[')
                        && *d > 0
                    {
                        *d -= 1;
                    }
                }
                '{' => *unbalanced.entry('{').or_insert(0) += 1,
                '}' => {
                    if let Some(d) = unbalanced.get_mut(&'{')
                        && *d > 0
                    {
                        *d -= 1;
                    }
                }
                '"' => {
                    let d = unbalanced.entry('"').or_insert(0);
                    if *d > 0 {
                        *d -= 1;
                    } else {
                        *d += 1;
                    }
                }
                '\'' => {
                    let d = unbalanced.entry('\'').or_insert(0);
                    if *d > 0 {
                        *d -= 1;
                    } else {
                        *d += 1;
                    }
                }
                '`' => {
                    let d = unbalanced.entry('`').or_insert(0);
                    if *d > 0 {
                        *d -= 1;
                    } else {
                        *d += 1;
                    }
                }
                _ => {}
            }
        }
        let mut closers_to_insert = String::new();
        for _ in 0..*unbalanced.get(&'(').unwrap_or(&0) {
            closers_to_insert.push(')');
        }
        for _ in 0..*unbalanced.get(&'[').unwrap_or(&0) {
            closers_to_insert.push(']');
        }
        for _ in 0..*unbalanced.get(&'{').unwrap_or(&0) {
            closers_to_insert.push('}');
        }
        for _ in 0..*unbalanced.get(&'"').unwrap_or(&0) {
            closers_to_insert.push('"');
        }
        for _ in 0..*unbalanced.get(&'\'').unwrap_or(&0) {
            closers_to_insert.push('\'');
        }
        for _ in 0..*unbalanced.get(&'`').unwrap_or(&0) {
            closers_to_insert.push('`');
        }

        let full_insert = format!("{}{}", s, closers_to_insert);
        let op = EditOp::Insert {
            pos: cursor_before,
            text: full_insert,
        };
        self.apply_op(op.clone());
        self.current_transaction.push(op);

        // Track auto-inserted closers for skip-over. Pair each closer with
        // its actual unmatched opener (LIFO) so paste "(((" -> "((()))"
        // maps closers to distinct openers, not all to the last one (R11).
        let s_chars: Vec<char> = s.chars().collect();
        let s_len = s_chars.len();
        let mut open_stacks: std::collections::HashMap<char, Vec<usize>> =
            std::collections::HashMap::new();
        let mut quote_open: std::collections::HashMap<char, Option<usize>> =
            std::collections::HashMap::new();
        for (idx, &c) in s_chars.iter().enumerate() {
            match c {
                '(' => open_stacks.entry('(').or_default().push(idx),
                ')' => {
                    if let Some(st) = open_stacks.get_mut(&'(') {
                        st.pop();
                    }
                }
                '[' => open_stacks.entry('[').or_default().push(idx),
                ']' => {
                    if let Some(st) = open_stacks.get_mut(&'[') {
                        st.pop();
                    }
                }
                '{' => open_stacks.entry('{').or_default().push(idx),
                '}' => {
                    if let Some(st) = open_stacks.get_mut(&'{') {
                        st.pop();
                    }
                }
                '"' | '\'' | '`' => {
                    let entry = quote_open.entry(c).or_insert(None);
                    if entry.is_some() {
                        *entry = None;
                    } else {
                        *entry = Some(idx);
                    }
                }
                _ => {}
            }
        }
        for (q, opt) in quote_open {
            if let Some(idx) = opt {
                open_stacks.entry(q).or_default().push(idx);
            }
        }
        let mut closer_offset = 0usize;
        for &open_char in &['(', '[', '{', '"', '\'', '`'] {
            if let Some(stack) = open_stacks.get(&open_char) {
                for &opener_rel in stack.iter().rev() {
                    let opener_idx = cursor_before + opener_rel;
                    let closer_idx = cursor_before + s_len + closer_offset;
                    self.auto_inserted_closers.push(closer_idx);
                    self.auto_inserted_pairs.push((opener_idx, closer_idx));
                    closer_offset += 1;
                }
            }
        }

        self.maybe_auto_commit();
    }

    pub fn delete_left(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let cursor_before = self.cursor;
        let del_start = self.prev_grapheme_boundary(cursor_before);
        let count = cursor_before - del_start;

        let left_char = self.chars[cursor_before - 1];
        let right_char = self.chars.get(cursor_before).copied();
        let is_matching_closer = count == 1
            && matches!(
                (left_char, right_char),
                ('(', Some(')'))
                    | ('[', Some(']'))
                    | ('{', Some('}'))
                    | ('"', Some('"'))
                    | ('\'', Some('\''))
                    | ('`', Some('`'))
            );

        // Only smart-delete the pair if the closer was genuinely auto-inserted.
        let should_delete_pair =
            is_matching_closer && self.auto_inserted_closers.contains(&cursor_before);

        let deleted_text: String = self.chars[del_start..cursor_before].iter().collect();
        let op = EditOp::Delete {
            pos: del_start,
            text: deleted_text,
        };
        self.apply_op(op.clone());
        self.current_transaction.push(op);

        if should_delete_pair {
            let right_del = EditOp::Delete {
                pos: del_start,
                text: right_char.expect("matching closer exists").to_string(),
            };
            self.apply_op(right_del.clone());
            self.current_transaction.push(right_del);

            self.auto_inserted_closers
                .retain(|&idx| idx != cursor_before && idx != cursor_before - 1);

            for idx in self.auto_inserted_closers.iter_mut() {
                if *idx > cursor_before {
                    *idx = idx.saturating_sub(2);
                }
            }

            self.auto_inserted_pairs
                .retain(|&(opener, closer)| opener != cursor_before - 1 && closer != cursor_before);
            for (opener, closer) in self.auto_inserted_pairs.iter_mut() {
                if *opener > cursor_before {
                    *opener = opener.saturating_sub(2);
                }
                if *closer > cursor_before {
                    *closer = closer.saturating_sub(2);
                }
            }
        } else {
            self.auto_inserted_closers
                .retain(|&idx| idx < del_start || idx >= cursor_before);

            for idx in self.auto_inserted_closers.iter_mut() {
                if *idx >= cursor_before {
                    *idx = idx.saturating_sub(count);
                }
            }

            self.auto_inserted_pairs.retain(|&(opener, closer)| {
                (opener < del_start || opener >= cursor_before)
                    && (closer < del_start || closer >= cursor_before)
            });
            for (opener, closer) in self.auto_inserted_pairs.iter_mut() {
                if *opener >= cursor_before {
                    *opener = opener.saturating_sub(count);
                }
                if *closer >= cursor_before {
                    *closer = closer.saturating_sub(count);
                }
            }
        }

        self.maybe_auto_commit();
    }

    pub fn delete_right(&mut self) {
        if self.cursor >= self.chars.len() {
            return;
        }
        let del_end = self.next_grapheme_boundary(self.cursor);
        let count = del_end - self.cursor;
        let deleted_pos = self.cursor;
        let deleted_text: String = self.chars[deleted_pos..del_end].iter().collect();
        let op = EditOp::Delete {
            pos: deleted_pos,
            text: deleted_text,
        };
        self.apply_op(op.clone());
        self.current_transaction.push(op);

        self.auto_inserted_closers
            .retain(|&idx| idx < deleted_pos || idx >= del_end);

        for idx in self.auto_inserted_closers.iter_mut() {
            if *idx >= del_end {
                *idx = idx.saturating_sub(count);
            }
        }

        self.auto_inserted_pairs.retain(|&(opener, closer)| {
            (opener < deleted_pos || opener >= del_end)
                && (closer < deleted_pos || closer >= del_end)
        });
        for (opener, closer) in self.auto_inserted_pairs.iter_mut() {
            if *opener >= del_end {
                *opener = opener.saturating_sub(count);
            }
            if *closer >= del_end {
                *closer = closer.saturating_sub(count);
            }
        }

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

            self.auto_inserted_closers
                .retain(|&idx| idx < target_cursor || idx >= cursor_before);
            for idx in self.auto_inserted_closers.iter_mut() {
                if *idx >= cursor_before {
                    *idx = idx.saturating_sub(delete_len);
                }
            }

            self.auto_inserted_pairs.retain(|&(opener, closer)| {
                (opener < target_cursor || opener >= cursor_before)
                    && (closer < target_cursor || closer >= cursor_before)
            });
            for (opener, closer) in self.auto_inserted_pairs.iter_mut() {
                if *opener >= cursor_before {
                    *opener = opener.saturating_sub(delete_len);
                }
                if *closer >= cursor_before {
                    *closer = closer.saturating_sub(delete_len);
                }
            }
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
            // Re-derive auto-insert tracking from the resulting buffer so
            // untouched pairs survive undo (long-term fix for R9: previously
            // cleared unconditionally, breaking skip-over for valid pairs).
            self.rebuild_auto_inserted_state();
        }
    }

    /// Rebuild `auto_inserted_closers/pairs` from the current buffer content.
    /// Pairs are those where an opener is immediately followed by its closer
    /// and both were plausibly auto-inserted. We conservatively rebuild only
    /// adjacent bracket/quote pairs that are balanced in the buffer — this
    /// preserves skip-over for untouched pairs while not resurrecting stale
    /// tracking for pairs whose opener was actually edited.
    fn rebuild_auto_inserted_state(&mut self) {
        self.auto_inserted_closers.clear();
        self.auto_inserted_pairs.clear();
        // Scan for adjacent matching pairs "()", "[]", "{}", '""', "''", "``".
        // Only adjacent pairs can be genuine auto-inserts from `insert_char`.
        let n = self.chars.len();
        let mut i = 0;
        while i + 1 < n {
            let a = self.chars[i];
            let b = self.chars[i + 1];
            let is_pair = matches!(
                (a, b),
                ('(', ')') | ('[', ']') | ('{', '}') | ('"', '"') | ('\'', '\'') | ('`', '`')
            );
            if is_pair {
                // Only track if there is at least one such pair — heuristic:
                // any adjacent empty pair whose closer would have been auto-
                // inserted is safe to re-track for skip-over purposes. This
                // does not affect correctness of pair-delete: delete_left
                // still checks that the closer is in the set, and an adjacent
                // empty pair is exactly the skip-over case.
                self.auto_inserted_closers.push(i + 1);
                self.auto_inserted_pairs.push((i, i + 1));
                i += 2;
            } else {
                i += 1;
            }
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
            self.rebuild_auto_inserted_state();
        }
    }

    fn apply_op(&mut self, op: EditOp) {
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
    fn test_autoclose_and_skipover() {
        let mut buf = TextBuffer::new();
        buf.insert_char('(');
        assert_eq!(buf.text(), "()");
        assert_eq!(buf.cursor(), 1);
        assert_eq!(buf.auto_inserted_closers, vec![1]);

        buf.insert_char('x');
        assert_eq!(buf.text(), "(x)");
        assert_eq!(buf.cursor(), 2);
        assert_eq!(buf.auto_inserted_closers, vec![2]);

        // Type ')' -> should skip over it
        buf.insert_char(')');
        assert_eq!(buf.text(), "(x)");
        assert_eq!(buf.cursor(), 3);
        assert!(buf.auto_inserted_closers.is_empty());
    }

    #[test]
    fn test_autoclose_backspace_pair() {
        let mut buf = TextBuffer::new();
        buf.insert_char('(');
        assert_eq!(buf.text(), "()");
        buf.delete_left();
        assert_eq!(buf.text(), "");
        assert_eq!(buf.cursor(), 0);
        assert!(buf.auto_inserted_closers.is_empty());
    }

    #[test]
    fn test_manually_typed_pair_not_deleted() {
        let mut buf = TextBuffer::new();
        // Manually type '(' then ')' — no auto-insert
        push_char_raw(&mut buf, '(');
        push_char_raw(&mut buf, ')');
        assert_eq!(buf.text(), "()");
        // Both characters are in the buffer, but since they were manually typed,
        // the closer should NOT be in auto_inserted_closers.
        assert!(buf.auto_inserted_closers.is_empty());

        // Backspace should only delete the opener, not the pair
        buf.set_cursor(1);
        buf.delete_left();
        assert_eq!(buf.text(), ")");
        assert_eq!(buf.cursor(), 0);
    }

    // Helper to insert a char without auto-close and without commit tracking
    fn push_char_raw(buf: &mut TextBuffer, c: char) {
        buf.chars.insert(buf.cursor, c);
        buf.cursor += 1;
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
