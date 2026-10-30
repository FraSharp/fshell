// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::{ParseError, Parser};
use miette::SourceSpan;

impl Parser {
    pub fn new(input: &str) -> Self {
        let chars: Vec<char> = input.chars().collect();
        let mut byte_offsets = Vec::with_capacity(chars.len() + 1);
        let mut offset = 0usize;
        for c in &chars {
            byte_offsets.push(offset);
            offset += c.len_utf8();
        }
        byte_offsets.push(offset);
        Parser {
            input: chars,
            byte_offsets,
            pos: 0,
            redirect_mode: false,
            cmd_arg_mode: false,
            value_context: false,
            is_subsequent_stage: false,
            bool_ops: false,
            statement_terminator: '}',
            recursion_depth: std::cell::Cell::new(0),
            pending_heredocs: Vec::new(),
            heredoc_bodies: Vec::new(),
            saw_native_construct: false,
        }
    }

    /// Whether parsing consumed a construct that only the native grammar owns.
    ///
    /// The engine router uses this to keep a failed native parse from being
    /// reinterpreted as POSIX: `let x = 1` and a later POSIX loop is a native
    /// script with a POSIX-only construct in it, not a POSIX script whose
    /// `let` happens to be a command.
    pub fn saw_native_construct(&self) -> bool {
        self.saw_native_construct
    }

    /// Whether the rest of the current line contains the POSIX `do` keyword
    /// before any native block opener.
    ///
    /// A POSIX `for` list (`for i in 1 2; do …`) is not a native iterable, so
    /// the iterable parser fails before the `{` check. This lookahead lets the
    /// loop parser name the POSIX shape instead of reporting a malformed list.
    pub(crate) fn line_has_posix_do(&self, from: usize) -> bool {
        let mut p = from;
        while p < self.input.len() && !matches!(self.input[p], '\n' | '\r') {
            if self.input[p] == '{' {
                return false;
            }
            p += 1;
        }
        let line: String = self.input[from..p].iter().collect();
        line.split(|c: char| c.is_whitespace() || c == ';')
            .any(|word| word == "do")
    }

    /// Whether the cursor sits on a POSIX control-flow continuation
    /// (`; then`, `; do`, or the bare keyword) rather than a native block.
    ///
    /// The native `if`/`for`/`while` parsers expect `{` here. Recognizing the
    /// POSIX shape lets them say how to wrap the block instead of reporting a
    /// bare missing brace.
    pub(crate) fn at_posix_continuation(&self, keyword: &str) -> bool {
        let mut p = self.pos;
        while p < self.input.len() && self.input[p].is_whitespace() {
            p += 1;
        }
        if p < self.input.len() && self.input[p] == ';' {
            p += 1;
            while p < self.input.len() && self.input[p].is_whitespace() {
                p += 1;
            }
        }
        let end = p + keyword.chars().count();
        if end > self.input.len() || !self.input[p..end].iter().copied().eq(keyword.chars()) {
            return false;
        }
        end == self.input.len() || !(self.input[end].is_alphanumeric() || self.input[end] == '_')
    }

    pub(crate) fn peek(&self) -> Option<char> {
        if self.pos < self.input.len() {
            Some(self.input[self.pos])
        } else {
            None
        }
    }

    pub(crate) fn next_char(&mut self) -> Option<char> {
        if self.pos < self.input.len() {
            let c = self.input[self.pos];
            self.pos += 1;
            Some(c)
        } else {
            None
        }
    }

    pub(crate) fn is_eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    /// Parse an escape sequence inside a double-quoted string literal.
    ///
    /// Following the POSIX/Bash double-quote convention, backslashes retain
    /// special escape meaning only before `"`, `\`, `$`, `` ` ``, `{`, `}`,
    /// and newline (line continuation). All other sequences (`\n`, `\t`, `\r`,
    /// `\0`, `\x`, `\u`, `\.`, `\d`, etc.) preserve the backslash verbatim so
    /// scripts, regexes, and arguments passed to external tools (Python, grep,
    /// sed, awk) are not corrupted.
    pub(crate) fn parse_escape_seq(&mut self) -> Result<String, ParseError> {
        match self.next_char() {
            Some('\\') => Ok("\\".to_string()),
            Some('"') => Ok("\"".to_string()),
            Some('$') => Ok("$".to_string()),
            Some('`') => Ok("`".to_string()),
            Some('{') => Ok("{".to_string()),
            Some('}') => Ok("}".to_string()),
            Some('\r') => {
                if self.peek() == Some('\n') {
                    self.next_char();
                }
                Ok(String::new())
            }
            Some('\n') => Ok(String::new()),
            Some(other) => Ok(format!("\\{}", other)),
            None => Err(ParseError::UnexpectedEof {
                span: self.current_span(),
            }),
        }
    }
}

impl Parser {
    /// Byte offset for a char index (clamped to end-of-input).
    pub(crate) fn byte_offset(&self, char_pos: usize) -> usize {
        self.byte_offsets
            .get(char_pos)
            .copied()
            .unwrap_or_else(|| self.byte_offsets.last().copied().unwrap_or(0))
    }

    pub(crate) fn current_span(&self) -> SourceSpan {
        let start = self.byte_offset(self.pos);
        let end = self.byte_offset(self.pos + 1);
        SourceSpan::new(start.into(), end.saturating_sub(start).max(1))
    }

    pub(crate) fn span_from(&self, start: usize) -> SourceSpan {
        let from = self.byte_offset(start);
        let to = self.byte_offset(self.pos);
        SourceSpan::new(from.into(), to.saturating_sub(from))
    }

    pub(crate) fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            // A here-document body starts after the newline that ends the
            // command line, so while a header is pending the newline is a
            // boundary, not whitespace.
            if !self.pending_heredocs.is_empty() && (c == '\n' || c == '\r') {
                break;
            }
            if c.is_whitespace() {
                self.next_char();
            } else if c == '\\'
                && self.pos + 1 < self.input.len()
                && (self.input[self.pos + 1] == '\n' || self.input[self.pos + 1] == '\r')
            {
                self.next_char(); // skip '\'
                if self.peek() == Some('\r') {
                    self.next_char();
                }
                if self.peek() == Some('\n') {
                    self.next_char();
                }
            } else {
                break;
            }
        }
    }

    pub(crate) fn skip_horizontal_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c == ' ' || c == '\t' {
                self.next_char();
            } else if c == '\\'
                && self.pos + 1 < self.input.len()
                && (self.input[self.pos + 1] == '\n' || self.input[self.pos + 1] == '\r')
            {
                self.next_char(); // skip '\'
                if self.peek() == Some('\r') {
                    self.next_char();
                }
                if self.peek() == Some('\n') {
                    self.next_char();
                }
            } else {
                break;
            }
        }
    }

    pub(crate) fn expect(&mut self, expected: char) -> Result<(), ParseError> {
        self.skip_whitespace();
        let start = self.pos;
        match self.next_char() {
            Some(c) if c == expected => Ok(()),
            Some(c) => Err(ParseError::ExpectedChar {
                expected,
                found: c,
                span: self.span_from(start),
            }),
            None => Err(ParseError::ExpectedChar {
                expected,
                found: '\0',
                span: self.current_span(),
            }),
        }
    }

    pub(crate) fn expect_str(&mut self, s: &str) -> Result<(), ParseError> {
        self.skip_whitespace();
        let start = self.pos;
        for expected_c in s.chars() {
            match self.next_char() {
                Some(c) if c == expected_c => {}
                Some(c) => {
                    return Err(ParseError::ExpectedToken {
                        expected: s.to_string(),
                        found: c.to_string(),
                        span: self.span_from(start),
                    });
                }
                None => {
                    return Err(ParseError::UnexpectedEof {
                        span: self.current_span(),
                    });
                }
            }
        }
        Ok(())
    }

    pub(crate) fn match_keyword(&mut self, kw: &str) -> bool {
        self.skip_whitespace();
        let mut temp_pos = self.pos;
        for expected_c in kw.chars() {
            if temp_pos >= self.input.len() || self.input[temp_pos] != expected_c {
                return false;
            }
            temp_pos += 1;
        }
        // Verify keyword boundary (not part of a larger identifier)
        if temp_pos < self.input.len()
            && (self.input[temp_pos].is_alphanumeric() || self.input[temp_pos] == '_')
        {
            return false;
        }
        self.pos = temp_pos;
        true
    }

    pub(crate) fn peek_identifier(&self) -> Option<String> {
        let mut pos = self.pos;
        // Skip whitespace in local pos
        while pos < self.input.len() && self.input[pos].is_whitespace() {
            pos += 1;
        }
        if pos >= self.input.len() {
            return None;
        }
        let first = self.input[pos];
        // Identifiers must not start with a digit
        if first.is_ascii_digit() {
            return None;
        }
        // If it starts with '-' followed by a digit, it is a number
        if first == '-' && pos + 1 < self.input.len() && self.input[pos + 1].is_ascii_digit() {
            return None;
        }

        let mut name = String::new();
        while pos < self.input.len() {
            let c = self.input[pos];
            if c.is_alphanumeric() || c == '_' || c == '-' {
                name.push(c);
                pos += 1;
            } else {
                break;
            }
        }
        if name.is_empty() { None } else { Some(name) }
    }

    pub(crate) fn parse_identifier(&mut self) -> Result<String, ParseError> {
        self.skip_whitespace();
        if self.is_eof() {
            return Err(ParseError::UnexpectedEof {
                span: self.current_span(),
            });
        }
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                name.push(self.next_char().ok_or_else(|| ParseError::UnexpectedEof {
                    span: self.current_span(),
                })?);
            } else {
                break;
            }
        }
        if name.is_empty() {
            let found = self
                .peek()
                .map(|c| format!("'{}'", c))
                .unwrap_or_else(|| "end of input".to_string());
            return Err(ParseError::ExpectedToken {
                expected: "identifier".to_string(),
                found,
                span: self.current_span(),
            });
        }
        Ok(name)
    }

    /// Check if the current position (which is `{`) starts a brace expansion pattern.
    /// Scans ahead without consuming input.
    pub(crate) fn is_brace_expansion(&self) -> bool {
        let mut pos = self.pos + 1; // skip {
        let mut depth = 1u32;
        let mut has_comma_or_range = false;
        while pos < self.input.len() {
            match self.input[pos] {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                ',' => has_comma_or_range = true,
                ':' if depth == 1 => {
                    // A colon before any comma/range means map literal, not brace expansion.
                    // But we need to check if we're at depth 1 (not inside nested braces)
                    return false;
                }
                _ => {}
            }
            // Check for ".." range pattern
            if !has_comma_or_range
                && pos + 1 < self.input.len()
                && self.input[pos] == '.'
                && self.input[pos + 1] == '.'
            {
                has_comma_or_range = true;
                pos += 1; // skip second .
            }
            pos += 1;
        }
        has_comma_or_range
    }
}
