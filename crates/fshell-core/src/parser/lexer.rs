// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::{ParseError, Parser};
use miette::SourceSpan;

impl Parser {
    pub fn new(input: &str) -> Self {
        Parser {
            input: input.chars().collect(),
            pos: 0,
            redirect_mode: false,
            cmd_arg_mode: false,
            is_subsequent_stage: false,
            recursion_depth: std::cell::Cell::new(0),
        }
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

    /// Parse an escape sequence inside a string literal.
    /// Returns the expanded string (one or more chars).
    /// For unrecognized escapes (e.g. `\.`), preserves the literal `\`+char
    /// matching bash double-quote convention — essential for passing regex
    /// patterns like `\.rs$` through to external commands unchanged.
    pub(crate) fn parse_escape_seq(&mut self) -> Result<String, ParseError> {
        match self.next_char() {
            Some('n') => Ok("\n".to_string()),
            Some('t') => Ok("\t".to_string()),
            Some('r') => Ok("\r".to_string()),
            Some('\\') => Ok("\\".to_string()),
            Some('"') => Ok("\"".to_string()),
            Some('{') => Ok("{".to_string()),
            Some('}') => Ok("}".to_string()),
            Some('0') => Ok("\0".to_string()),
            Some('x') => {
                let mut hex = String::with_capacity(2);
                for _ in 0..2 {
                    match self.next_char() {
                        Some(c) if c.is_ascii_hexdigit() => hex.push(c),
                        _ => {
                            return Err(ParseError::SyntaxError {
                                message:
                                    "Invalid hex escape: expected two hexadecimal digits after \\x"
                                        .to_string(),
                                span: self.current_span(),
                            });
                        }
                    }
                }
                let byte = u8::from_str_radix(&hex, 16).map_err(|_| ParseError::SyntaxError {
                    message: "Invalid hex digits".to_string(),
                    span: self.current_span(),
                })?;
                Ok((byte as char).to_string())
            }
            Some('u') => {
                let mut hex = String::with_capacity(4);
                for _ in 0..4 {
                    match self.next_char() {
                        Some(c) if c.is_ascii_hexdigit() => hex.push(c),
                        _ => {
                            return Err(ParseError::SyntaxError {
                                message:
                                    "Invalid unicode escape: expected four hexadecimal digits after \\u"
                                        .to_string(),
                                span: self.current_span(),
                            });
                        }
                    }
                }
                let code = u32::from_str_radix(&hex, 16).map_err(|_| ParseError::SyntaxError {
                    message: "Invalid unicode escape digits".to_string(),
                    span: self.current_span(),
                })?;
                let c = char::from_u32(code).ok_or_else(|| ParseError::SyntaxError {
                    message: format!("Invalid unicode code point: U+{:X}", code),
                    span: self.current_span(),
                })?;
                Ok(c.to_string())
            }
            // Unknown escapes like `\.`, `\s`, `\[`: preserve `\`+char literally.
            // This is the standard bash double-quote behavior and critical for
            // regex patterns passed to grep, rg, sed, etc.
            Some(other) => Ok(format!("\\{}", other)),
            None => Err(ParseError::UnexpectedEof {
                span: self.current_span(),
            }),
        }
    }
}

impl Parser {
    pub(crate) fn current_span(&self) -> SourceSpan {
        SourceSpan::new(self.pos.into(), 1)
    }

    pub(crate) fn span_from(&self, start: usize) -> SourceSpan {
        SourceSpan::new(start.into(), self.pos - start)
    }

    pub(crate) fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
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
