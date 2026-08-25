// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use super::{dedent, parse_ansi_escapes, parse_string_parts};
use crate::ast::*;
use crate::{ParseError, Parser};

impl Parser {
    pub(crate) fn parse_match_pattern(&mut self) -> Result<MatchPattern, ParseError> {
        self.skip_whitespace();
        match self.peek() {
            Some('_') => {
                self.next_char();
                Ok(MatchPattern::Wildcard)
            }
            Some('{') => {
                self.next_char();
                let mut fields = Vec::new();
                let mut rest = false;
                self.skip_whitespace();
                while self.peek() != Some('}') && !self.is_eof() {
                    if self.peek() == Some('.') {
                        self.expect_str("..")?;
                        rest = true;
                        self.skip_whitespace();
                        break;
                    }
                    let field_name = self.parse_identifier()?;
                    self.skip_whitespace();
                    self.expect(':')?;
                    let pattern = self.parse_match_pattern()?;
                    fields.push((field_name, pattern));
                    self.skip_whitespace();
                    if self.peek() == Some(',') {
                        self.next_char();
                    }
                    self.skip_whitespace();
                }
                self.expect('}')?;
                Ok(MatchPattern::Map { fields, rest })
            }
            Some('"') => {
                // String literal pattern
                let expr = self.parse_string_literal()?;
                if let Expr::String(parts) = expr {
                    let has_interp = parts.iter().any(|p| matches!(p, StringPart::Expr(_)));
                    if has_interp {
                        return Err(ParseError::SyntaxError {
                            message: "String interpolation is not supported in match patterns"
                                .to_string(),
                            span: self.current_span(),
                        });
                    }
                    let s = parts
                        .iter()
                        .filter_map(|p| match p {
                            StringPart::Lit(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    Ok(MatchPattern::Literal(LiteralPattern::String(s)))
                } else {
                    Err(ParseError::SyntaxError {
                        message: "Expected string literal pattern".to_string(),
                        span: self.current_span(),
                    })
                }
            }
            Some(c) if c.is_ascii_digit() || c == '-' => {
                // Numeric literal pattern
                let expr = self.parse_number_literal()?;
                match expr {
                    Expr::Int(i) => Ok(MatchPattern::Literal(LiteralPattern::Int(i))),
                    Expr::Float(f) => Ok(MatchPattern::Literal(LiteralPattern::Float(f))),
                    _ => Err(ParseError::SyntaxError {
                        message: "Expected numeric literal pattern".to_string(),
                        span: self.current_span(),
                    }),
                }
            }
            _ => {
                // Could be null, true, false, or a variable name (treated as string)
                let ident = self.parse_identifier()?;
                match ident.as_str() {
                    "null" => Ok(MatchPattern::Literal(LiteralPattern::Null)),
                    "true" => Ok(MatchPattern::Literal(LiteralPattern::Bool(true))),
                    "false" => Ok(MatchPattern::Literal(LiteralPattern::Bool(false))),
                    _ => Ok(MatchPattern::Literal(LiteralPattern::String(ident))),
                }
            }
        }
    }

    pub(crate) fn parse_type_constraint(&mut self) -> Result<TypeConstraint, ParseError> {
        self.skip_whitespace();
        if self.peek() == Some('{') {
            self.next_char();
            let mut fields = Vec::new();
            let mut rest = false;
            self.skip_whitespace();
            while self.peek() != Some('}') {
                if self.peek() == Some('.') {
                    self.expect_str("..")?;
                    rest = true;
                    self.skip_whitespace();
                    if let Some(c) = self.peek()
                        && (c.is_ascii_alphabetic() || c == '_')
                    {
                        let _ = self.parse_identifier()?;
                    }
                    self.skip_whitespace();
                    break;
                }
                let name = self.parse_identifier()?;
                self.expect(':')?;
                let sub_constraint = self.parse_type_constraint()?;
                fields.push((name, sub_constraint));
                self.skip_whitespace();
                if self.peek() == Some(',') {
                    self.next_char();
                    self.skip_whitespace();
                } else {
                    break;
                }
            }
            self.expect('}')?;
            self.skip_whitespace();
            let mut alias = None;
            if self.match_keyword("as") {
                alias = Some(self.parse_identifier()?);
            }
            Ok(TypeConstraint::Structural {
                fields,
                rest,
                alias,
            })
        } else {
            let name = self.parse_identifier()?;
            Ok(TypeConstraint::Primitive(name))
        }
    }

    /// Parses a single expression.
    pub fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.pos;
        let expr = self.parse_expr_with_pipeline(true)?;
        let span = self.span_from(start);
        Ok(Expr::Spanned {
            expr: Box::new(expr),
            span,
        })
    }

    pub(crate) fn binop_precedence(&mut self) -> Option<(BinOp, u8)> {
        let c = self.peek()?;
        match c {
            '+' => {
                self.next_char();
                Some((BinOp::Add, 3))
            }
            '-' => {
                if self.pos + 1 < self.input.len()
                    && (self.input[self.pos + 1] == '-'
                        || self.input[self.pos + 1].is_ascii_alphabetic()
                        || (self.cmd_arg_mode && self.input[self.pos + 1].is_ascii_digit()))
                {
                    // `--flag` or `-f` is a command-line flag, not the subtraction operator.
                    // Without this check, `cargo bench -p arg` is parsed as `bench - p`.
                    // In cmd_arg_mode, `-5` is a negative number argument (e.g. `git log --oneline -5`),
                    // not subtraction from the previous argument.
                    return None;
                }
                self.next_char();
                Some((BinOp::Sub, 3))
            }
            '*' => {
                self.next_char();
                Some((BinOp::Mul, 4))
            }
            '/' => {
                if self.cmd_arg_mode {
                    return None;
                }
                self.next_char();
                Some((BinOp::Div, 4))
            }
            '=' if self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '=' => {
                self.pos += 2;
                Some((BinOp::Eq, 2))
            }
            '!' if self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '=' => {
                self.pos += 2;
                Some((BinOp::Neq, 2))
            }
            '<' => {
                if (self.redirect_mode || self.cmd_arg_mode)
                    && !(self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '=')
                {
                    return None;
                }
                self.next_char();
                let op = if self.peek() == Some('=') {
                    self.next_char();
                    BinOp::Lte
                } else {
                    BinOp::Lt
                };
                Some((op, 2))
            }
            '>' => {
                if (self.redirect_mode || self.cmd_arg_mode)
                    && !(self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '=')
                {
                    // In redirect mode or command arg mode, `>` is not an infix operator
                    // (unless it's `>=`). Return None so the caller
                    // treats `>` as a pipeline-level redirect or arg boundary.
                    return None;
                }
                self.next_char();
                let op = if self.peek() == Some('=') {
                    self.next_char();
                    BinOp::Gte
                } else {
                    BinOp::Gt
                };
                Some((op, 2))
            }
            '~' => {
                // In command-arg mode `~` is part of a literal bare word
                // (e.g. `file~`, `echo a ~ b`), not the regex-match operator.
                if self.cmd_arg_mode {
                    return None;
                }
                self.next_char();
                Some((BinOp::ReMatch, 1))
            }
            _ => None,
        }
    }

    pub(crate) fn is_bracket_command(&self) -> bool {
        if self.peek() != Some('[') {
            return false;
        }
        let mut idx = self.pos + 1;
        while idx < self.input.len() && self.input[idx].is_whitespace() {
            idx += 1;
        }
        if idx >= self.input.len() {
            return false;
        }
        let mut first = String::new();
        if self.input[idx] == '"' || self.input[idx] == '\'' {
            let quote = self.input[idx];
            idx += 1;
            while idx < self.input.len() && self.input[idx] != quote {
                if self.input[idx] == '\\' {
                    idx += 1;
                }
                if idx < self.input.len() {
                    first.push(self.input[idx]);
                    idx += 1;
                }
            }
            if idx < self.input.len() {
                idx += 1;
            }
        } else {
            while idx < self.input.len()
                && !self.input[idx].is_whitespace()
                && self.input[idx] != ']'
                && self.input[idx] != ','
            {
                first.push(self.input[idx]);
                idx += 1;
            }
        }

        if first == "!" {
            return true;
        }
        if let Some(rest) = first.strip_prefix('-') {
            if rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                return true;
            }
        }

        while idx < self.input.len() && self.input[idx].is_whitespace() {
            idx += 1;
        }
        if idx >= self.input.len() {
            return false;
        }

        let mut second = String::new();
        if self.input[idx] == '"' || self.input[idx] == '\'' {
            let quote = self.input[idx];
            idx += 1;
            while idx < self.input.len() && self.input[idx] != quote {
                if self.input[idx] == '\\' {
                    idx += 1;
                }
                if idx < self.input.len() {
                    second.push(self.input[idx]);
                    idx += 1;
                }
            }
        } else {
            while idx < self.input.len()
                && !self.input[idx].is_whitespace()
                && self.input[idx] != ']'
                && self.input[idx] != ','
            {
                second.push(self.input[idx]);
                idx += 1;
            }
        }

        matches!(
            second.as_str(),
            "=" | "!=" | "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge"
        )
    }

    pub(crate) fn parse_expr_with_pipeline(
        &mut self,
        allow_pipeline: bool,
    ) -> Result<Expr, ParseError> {
        let start = self.pos;

        self.skip_whitespace();
        if self.is_bracket_command() {
            let pipeline = self.parse_pipeline()?;
            return Ok(Expr::Pipeline(pipeline));
        }

        if allow_pipeline {
            self.skip_whitespace();
            // Check if it starts with a bare identifier (representing a command call pipeline stage)
            if let Some(ident) = self.peek_identifier()
                && ident != "null"
                && ident != "true"
                && ident != "false"
                && ident != "and"
                && ident != "or"
                && ident != "if"
                && ident != "else"
                && ident != "while"
                && ident != "source"
                && ident != "match"
                && ident != "try"
                && ident != "catch"
                && ident != "with"
                && ident != "caps"
                && ident != "unsafe"
                && ident != "let"
                && ident != "fn"
                && ident != "exit"
                && ident != "on"
            {
                // Find the next character after the identifier
                let mut next_pos = self.pos;
                while next_pos < self.input.len() && self.input[next_pos].is_whitespace() {
                    next_pos += 1;
                }
                next_pos += ident.len();
                let ident_end = next_pos;
                while next_pos < self.input.len() && self.input[next_pos].is_whitespace() {
                    next_pos += 1;
                }
                let next_char = if next_pos < self.input.len() {
                    Some(self.input[next_pos])
                } else {
                    None
                };

                let is_member_access = if next_char == Some('.') {
                    let char_after_dot = if next_pos + 1 < self.input.len() {
                        Some(self.input[next_pos + 1])
                    } else {
                        None
                    };
                    // Member access requires no whitespace between ident and dot
                    // (e.g. `hx.config` not `hx .config`, which is a command with a path arg)
                    next_pos == ident_end
                        && matches!(char_after_dot, Some(c) if c.is_ascii_alphabetic() || c == '_')
                } else {
                    false
                };

                let is_operator = match next_char {
                    Some('+') => {
                        // `chmod +x file`: `+x` is a flag-style argument, not addition.
                        // Mirror the `-` handling: `+` is an operator only when not
                        // immediately followed by an identifier-ish char (e.g. `a + b`,
                        // `x + 1`), so `chmod +x ...` parses as a command call.
                        if next_pos + 1 < self.input.len() {
                            let nc = self.input[next_pos + 1];
                            !(nc.is_ascii_alphabetic() || nc.is_ascii_digit() || nc == '+')
                        } else {
                            false
                        }
                    }
                    Some('=') | Some('!') => true,
                    Some('*') => {
                        // * is multiplication only if followed by whitespace (binary op with spaces).
                        // Immediately followed by non-whitespace means it's a glob pattern (e.g., *.md, **/*.rs).
                        next_pos + 1 < self.input.len() && self.input[next_pos + 1].is_whitespace()
                    }
                    Some('<') => {
                        // `<=` is comparison operator. `<` (and `<<`, `<<<`, `<(`) is redirection / command argument.
                        next_pos + 1 < self.input.len() && self.input[next_pos + 1] == '='
                    }
                    Some('-') => {
                        if next_pos + 1 < self.input.len() {
                            let nc = self.input[next_pos + 1];
                            !(nc.is_ascii_alphabetic() || nc.is_ascii_digit() || nc == '-')
                        } else {
                            false
                        }
                    }
                    Some('/') => {
                        // /<alpha> or /<digit> or // is a path, not division
                        if next_pos + 1 < self.input.len() {
                            let nc = self.input[next_pos + 1];
                            !(nc.is_ascii_alphabetic() || nc.is_ascii_digit() || nc == '/')
                        } else {
                            false
                        }
                    }
                    _ => {
                        let mut temp_pos = next_pos;
                        let mut next_ident = String::new();
                        while temp_pos < self.input.len() {
                            let c = self.input[temp_pos];
                            if c.is_alphanumeric() || c == '_' {
                                next_ident.push(c);
                                temp_pos += 1;
                            } else {
                                break;
                            }
                        }
                        next_ident == "and" || next_ident == "or"
                    }
                };

                if !is_member_access && !is_operator {
                    let pipeline = self.parse_pipeline()?;
                    return Ok(Expr::Pipeline(pipeline));
                }
            }

            // Check for path-like command names (/bin/ls, ./foo, ~/bin/tool)
            if matches!(self.peek(), Some('/') | Some('~')) {
                let pipeline = self.parse_pipeline()?;
                return Ok(Expr::Pipeline(pipeline));
            }
            if self.peek() == Some('.') && self.pos + 1 < self.input.len() {
                let next = self.input[self.pos + 1];
                if next == '/' || next == '.' {
                    let pipeline = self.parse_pipeline()?;
                    return Ok(Expr::Pipeline(pipeline));
                }
            }

            // Check for input redirection at statement/pipeline start (< in.txt or 0< in.txt)
            if self.peek() == Some('<')
                && !(self.pos + 1 < self.input.len()
                    && (self.input[self.pos + 1] == '<' || self.input[self.pos + 1] == '('))
            {
                let pipeline = self.parse_pipeline()?;
                return Ok(Expr::Pipeline(pipeline));
            }
            if self.peek() == Some('0')
                && self.pos + 1 < self.input.len()
                && self.input[self.pos + 1] == '<'
                && !(self.pos + 2 < self.input.len()
                    && (self.input[self.pos + 2] == '<' || self.input[self.pos + 2] == '('))
            {
                let pipeline = self.parse_pipeline()?;
                return Ok(Expr::Pipeline(pipeline));
            }
        }

        let saved_redirect = self.redirect_mode;
        self.redirect_mode = saved_redirect || allow_pipeline;
        let lhs = self.parse_expr_with_precedence(0)?;
        self.redirect_mode = saved_redirect;

        // Check if it is a pipeline (after the main expression)
        let before_pipeline_whitespace = self.pos;
        self.skip_whitespace();
        let is_redirect = self.peek() == Some('>')
            || (self.peek() == Some('2')
                && self.pos + 1 < self.input.len()
                && self.input[self.pos + 1] == '>')
            || (self.peek() == Some('&')
                && self.pos + 1 < self.input.len()
                && self.input[self.pos + 1] == '>');
        let is_pipe = self.peek() == Some('|')
            && !(self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '|');
        if (is_pipe || is_redirect) && allow_pipeline {
            self.pos = start;
            let pipeline = self.parse_pipeline()?;
            return Ok(Expr::Pipeline(pipeline));
        } else {
            self.pos = before_pipeline_whitespace;
        }

        // Check for member access (e.g. `$val.member`)
        let mut lhs = lhs;
        while self.peek() == Some('.') {
            self.next_char();
            let member = self.parse_identifier()?;
            lhs = Expr::MemberAccess {
                expr: Box::new(lhs),
                member,
            };
        }

        Ok(lhs)
    }

    /// Pratt parser entry for binary expressions with precedence.
    pub(crate) fn parse_expr_with_precedence(&mut self, min_prec: u8) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_primary_expr()?;

        loop {
            let before_whitespace = self.pos;
            self.skip_whitespace();
            // Check for keyword operators `and` / `or`
            if let Some(ident) = self.peek_identifier()
                && (ident == "and" || ident == "or")
            {
                let op = if ident == "and" {
                    BinOp::And
                } else {
                    BinOp::Or
                };
                let prec = 1;
                if prec < min_prec {
                    self.pos = before_whitespace;
                    break;
                }
                // Consume the identifier
                for _ in 0..ident.len() {
                    self.next_char();
                }
                let rhs = self.parse_expr_with_precedence(prec + 1)?;
                lhs = Expr::BinaryOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                };
                continue;
            }

            let next = self.binop_precedence();
            match next {
                Some((op, prec)) => {
                    if prec < min_prec {
                        self.pos = before_whitespace;
                        break;
                    }
                    let rhs = self.parse_expr_with_precedence(prec + 1)?;
                    lhs = Expr::BinaryOp {
                        op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    };
                }
                None => {
                    self.pos = before_whitespace;
                    break;
                }
            }
        }

        Ok(lhs)
    }

    pub(crate) fn parse_bare_path_or_string(&mut self) -> Result<Expr, ParseError> {
        let mut path = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric()
                || c == '_'
                || c == '-'
                || c == '/'
                || c == '.'
                || c == '~'
                || c == '\\'
                || c == '*'
                || c == '?'
                || c == '['
                || c == ']'
                || c == '!'
                || c == '{'
                || c == '}'
                || c == ','
                || c == ':'
                || c == '%'
                || c == '+'
                || (c == '=' && self.pos + 1 < self.input.len() && self.input[self.pos + 1] != '=')
                || c == '@'
            {
                path.push(self.next_char().ok_or_else(|| ParseError::UnexpectedEof {
                    span: self.current_span(),
                })?);
            } else {
                break;
            }
        }
        if path.is_empty() {
            return Err(ParseError::SyntaxError {
                message: "Expected path or bare string".to_string(),
                span: self.current_span(),
            });
        }
        Ok(Expr::String(vec![StringPart::Lit(path)]))
    }

    pub(crate) fn parse_primary_expr(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_atom()?;
        while self.peek() == Some('.') {
            if self.pos + 1 < self.input.len() {
                let next = self.input[self.pos + 1];
                if next == '/' || next == '.' {
                    break;
                }
            }
            self.next_char();
            let member = self.parse_identifier()?;
            lhs = Expr::MemberAccess {
                expr: Box::new(lhs),
                member,
            };
        }
        Ok(lhs)
    }

    pub(crate) fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        self.skip_whitespace();
        if self.cmd_arg_mode
            && let Some(c) = self.peek()
        {
            if c == '=' {
                if self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '=' {
                    // Let == be parsed as comparison operator
                } else if self.pos + 1 < self.input.len()
                    && !self.input[self.pos + 1].is_whitespace()
                {
                    // `=word` (e.g. `=foo`) is a single literal argument.
                    return self.parse_bare_path_or_string();
                } else {
                    self.next_char();
                    return Ok(Expr::String(vec![StringPart::Lit("=".to_string())]));
                }
            }
            if c == '!' {
                if self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '=' {
                    self.next_char();
                    self.next_char();
                    return Ok(Expr::String(vec![StringPart::Lit("!=".to_string())]));
                }
                if self.pos + 1 < self.input.len() && self.input[self.pos + 1].is_whitespace() {
                    self.next_char();
                    return Ok(Expr::String(vec![StringPart::Lit("!".to_string())]));
                }
            }
            if c == ']' {
                self.next_char();
                return Ok(Expr::String(vec![StringPart::Lit("]".to_string())]));
            }
            // A leading `+flag` (e.g. `chmod +x file`) is a literal argument, not unary plus.
            if c == '+'
                && self.pos + 1 < self.input.len()
                && (self.input[self.pos + 1].is_ascii_alphabetic()
                    || self.input[self.pos + 1].is_ascii_digit())
            {
                return self.parse_bare_path_or_string();
            }
            // In command-arg mode, treat pure glob characters as literal string starters
            if c == '*' || c == '?' || c == '%' {
                return self.parse_bare_path_or_string();
            }
            // A leading `@` in command-arg mode is a literal bare string — npm
            // scope specifiers (`@opencode-ai/cli@next`), scp-style user@host:path,
            // or a lone `@` — not a boundary serialization operator (those are only
            // recognized at the start of a pipeline stage, see stmt.rs).
            if c == '@' {
                return self.parse_bare_path_or_string();
            }
        }
        // Check for `if` expression before other parsing
        if self.match_keyword("if") {
            return self.parse_if_expr();
        }
        // Check for here-string <<< before heredoc <<
        if self.peek() == Some('<')
            && self.pos + 2 < self.input.len()
            && self.input[self.pos + 1] == '<'
            && self.input[self.pos + 2] == '<'
        {
            return self.parse_here_string();
        }
        // Check for heredoc << before anything else in primary position
        if self.peek() == Some('<')
            && self.pos + 1 < self.input.len()
            && self.input[self.pos + 1] == '<'
        {
            return self.parse_heredoc_expr();
        }
        // Check for process substitution <( and >(
        if (self.peek() == Some('<') || self.peek() == Some('>'))
            && self.pos + 1 < self.input.len()
            && self.input[self.pos + 1] == '('
        {
            return self.parse_process_substitution();
        }

        match self.peek() {
            // Unary boolean negation: `!expr`
            Some('!') => {
                self.next_char(); // consume '!'
                let inner = self.parse_primary_expr()?;
                Ok(Expr::Not(Box::new(inner)))
            }
            Some('n') if self.match_keyword("null") => Ok(Expr::Null),
            Some('t') if self.match_keyword("true") => Ok(Expr::Bool(true)),
            Some('f') if self.match_keyword("false") => Ok(Expr::Bool(false)),
            Some('?') => {
                self.next_char();
                Ok(Expr::Ident("?".to_string()))
            }
            Some('$') => {
                self.next_char();
                // Check for $| inline pipeline
                if self.peek() == Some('|') {
                    return self.parse_inline_pipeline();
                }
                // Check for $( command substitution
                if self.peek() == Some('(') {
                    // Check for $((...)) arithmetic expansion
                    let saved_pos = self.pos;
                    self.next_char(); // consume first (
                    if self.peek() == Some('(') {
                        self.next_char(); // consume second (
                        match self.parse_expr() {
                            Ok(inner) => {
                                // Skip any whitespace between the inner expression
                                // and the closing `))` — parse_expr_with_precedence restores pos to
                                // before trailing whitespace on `None` from binop check.
                                self.skip_whitespace();
                                if self.peek() == Some(')') {
                                    self.next_char(); // consume first )
                                    self.skip_whitespace();
                                    if self.peek() == Some(')') {
                                        self.next_char(); // consume second )
                                        return Ok(Expr::ArithmeticExpansion(Box::new(inner)));
                                    }
                                }
                                // Backtrack — not arithmetic
                                self.pos = saved_pos;
                            }
                            Err(_) => {
                                // Backtrack — not arithmetic
                                self.pos = saved_pos;
                            }
                        }
                    } else {
                        // Single ( — not arithmetic, restore and parse as cmd substitution
                        self.pos = saved_pos;
                    }
                    return self.parse_cmd_substitution();
                }
                // Check for $' ANSI-C quoting
                if self.peek() == Some('\'') {
                    return self.parse_ansi_c_quote_expr();
                }
                // Check for ${...} braced variable (with optional modifier)
                if self.peek() == Some('{') {
                    return self.parse_braced_variable();
                }
                // Check for $? last exit code
                if self.peek() == Some('?') {
                    self.next_char();
                    return Ok(Expr::Variable("?".to_string()));
                }
                // Check for positional / special variables: $#, $@, $*, $$, $0, $1..$9
                if let Some(c) = self.peek() {
                    if c == '#' || c == '@' || c == '*' || c == '$' {
                        self.next_char();
                        return Ok(Expr::Variable(c.to_string()));
                    }
                    if c.is_ascii_digit() {
                        let mut name = String::new();
                        while let Some(d) = self.peek() {
                            if d.is_ascii_digit() {
                                if let Some(ch) = self.next_char() {
                                    name.push(ch);
                                }
                            } else {
                                break;
                            }
                        }
                        return Ok(Expr::Variable(name));
                    }
                }
                // Regular $var
                let name = self.parse_identifier()?;
                // If $VAR is immediately followed by `/`, combine both into an
                // interpolated string so that `$HOME/.config/fshell/init.fsh`
                // becomes a single command argument instead of two.
                if self.peek() == Some('/') {
                    let rest = self.parse_bare_path_or_string()?;
                    if let Expr::String(parts) = rest {
                        let mut combined = Vec::new();
                        combined.push(StringPart::Expr(Box::new(Expr::Variable(name))));
                        combined.extend(parts);
                        return Ok(Expr::String(combined));
                    }
                }
                Ok(Expr::Variable(name))
            }
            Some('"') => {
                // Check for triple-quote """
                if self.pos + 2 < self.input.len()
                    && self.input[self.pos + 1] == '"'
                    && self.input[self.pos + 2] == '"'
                {
                    return self.parse_triple_quoted_string();
                }
                self.parse_string_literal()
            }
            Some('\'') => {
                self.next_char();
                let mut s = String::new();
                while let Some(c) = self.peek() {
                    if c == '\'' {
                        self.next_char();
                        break;
                    }
                    s.push(self.next_char().ok_or_else(|| ParseError::UnexpectedEof {
                        span: self.current_span(),
                    })?);
                }
                Ok(Expr::String(vec![StringPart::Lit(s)]))
            }
            Some('`') => {
                self.next_char();
                let mut content = String::new();
                loop {
                    match self.peek() {
                        None => {
                            return Err(ParseError::UnexpectedEof {
                                span: self.current_span(),
                            });
                        }
                        Some('`') => {
                            self.next_char();
                            break;
                        }
                        Some('\\') => {
                            self.next_char();
                            match self.next_char() {
                                Some('`') => content.push('`'),
                                Some(c) => {
                                    content.push('\\');
                                    content.push(c);
                                }
                                None => {
                                    return Err(ParseError::UnexpectedEof {
                                        span: self.current_span(),
                                    });
                                }
                            }
                        }
                        Some(c) => {
                            content.push(c);
                            self.next_char();
                        }
                    }
                }
                // Parse the content between backticks as a pipeline
                let mut inner = Parser::new(&content);
                let pipeline = inner
                    .parse_pipeline()
                    .map_err(|e| ParseError::SyntaxError {
                        message: format!("invalid pipeline in backtick substitution: {e}"),
                        span: self.current_span(),
                    })?;
                Ok(Expr::InlinePipeline(pipeline))
            }
            Some('(') => {
                self.next_char();
                let saved_arg = self.cmd_arg_mode;
                self.cmd_arg_mode = false;
                let expr = self.parse_expr()?;
                self.cmd_arg_mode = saved_arg;
                self.expect(')')?;
                Ok(expr)
            }
            Some('[') => {
                self.next_char();
                let mut elements = Vec::new();
                self.skip_whitespace();
                if self.peek() != Some(']') {
                    let saved_arg = self.cmd_arg_mode;
                    self.cmd_arg_mode = false;
                    loop {
                        elements.push(self.parse_expr()?);
                        self.skip_whitespace();
                        if self.peek() == Some(',') {
                            self.next_char();
                        } else {
                            break;
                        }
                    }
                    self.cmd_arg_mode = saved_arg;
                }
                self.expect(']')?;
                Ok(Expr::List(elements))
            }
            Some('{') => {
                // Brace expansion {a,b,c} or {1..5} takes priority over map literal.
                if self.is_brace_expansion() {
                    return self.parse_bare_path_or_string();
                }
                self.next_char();
                let mut pairs = Vec::new();
                self.skip_whitespace();
                if self.peek() != Some('}') {
                    loop {
                        self.skip_whitespace();
                        let key = if self.peek() == Some('"') {
                            self.next_char(); // consume '"'
                            let mut s_val = String::new();
                            while let Some(c) = self.peek() {
                                if c == '"' {
                                    self.next_char(); // consume '"'
                                    break;
                                } else if c == '\\' {
                                    self.next_char();
                                    s_val.push_str(&self.parse_escape_seq()?);
                                } else {
                                    s_val.push(self.next_char().ok_or_else(|| {
                                        ParseError::UnexpectedEof {
                                            span: self.current_span(),
                                        }
                                    })?);
                                }
                            }
                            s_val
                        } else {
                            self.parse_identifier()?
                        };
                        self.expect(':')?;
                        let saved_arg = self.cmd_arg_mode;
                        self.cmd_arg_mode = false;
                        let val = self.parse_expr()?;
                        self.cmd_arg_mode = saved_arg;
                        pairs.push((key, val));
                        self.skip_whitespace();
                        if self.peek() == Some(',') {
                            self.next_char();
                        } else {
                            break;
                        }
                    }
                }
                self.expect('}')?;
                Ok(Expr::Map(pairs))
            }
            Some('/') | Some('~') | Some('.') => self.parse_bare_path_or_string(),
            Some(c) if c.is_ascii_digit() || c == '-' => {
                if c == '-'
                    && (self.pos + 1 >= self.input.len()
                        || !self.input[self.pos + 1].is_ascii_digit())
                {
                    self.parse_bare_path_or_string()
                } else {
                    self.parse_number_literal()
                }
            }
            _ => {
                // Check if it is an identifier followed by path/glob/KV chars
                let ident = self.peek_identifier();
                let is_path_or_kv = if let Some(ident) = &ident {
                    let next_pos = self.pos + ident.len();
                    next_pos < self.input.len()
                        && (self.input[next_pos] == '/'
                            || self.input[next_pos] == '.'
                            || self.input[next_pos] == '*'
                            || self.input[next_pos] == '?'
                            || self.input[next_pos] == '['
                            || self.input[next_pos] == ':'
                            || (self.cmd_arg_mode
                                && self.input[next_pos] == '+'
                                && next_pos + 1 < self.input.len()
                                && !self.input[next_pos + 1].is_whitespace())
                            || (self.cmd_arg_mode
                                && self.input[next_pos] == '@'
                                && next_pos + 1 < self.input.len()
                                && !self.input[next_pos + 1].is_whitespace())
                            || (self.cmd_arg_mode
                                && matches!(
                                    self.input[next_pos],
                                    '~' | '\\' | ']' | '{' | ',' | '%'
                                )
                                && (next_pos + 1 >= self.input.len()
                                    || !self.input[next_pos + 1].is_whitespace()))
                            || (self.input[next_pos] == '='
                                && (next_pos + 1 >= self.input.len()
                                    || self.input[next_pos + 1] != '=')))
                } else {
                    false
                };
                if is_path_or_kv {
                    self.parse_bare_path_or_string()
                } else if self.cmd_arg_mode && ident.is_none() {
                    // In command-arg mode any unrecognized leading character is a
                    // literal bare word (e.g. `,foo`, `:foo`, `\foo`), never an error.
                    self.parse_bare_path_or_string()
                } else if ident.is_some() {
                    let name = self.parse_identifier()?;
                    Ok(Expr::Ident(name))
                } else if self.is_eof() {
                    Err(ParseError::UnexpectedEof {
                        span: self.current_span(),
                    })
                } else {
                    let c = self.peek().unwrap_or('\0');
                    Err(ParseError::ExpectedToken {
                        expected: "expression".to_string(),
                        found: format!("'{c}'"),
                        span: self.current_span(),
                    })
                }
            }
        }
    }

    fn parse_next_stage(&mut self, is_first: bool) -> Result<PipelineStage, ParseError> {
        let saved = self.is_subsequent_stage;
        if !is_first {
            self.is_subsequent_stage = true;
        }
        let res = self.parse_pipeline_stage();
        self.is_subsequent_stage = saved;
        res
    }

    fn try_parse_dollar_var(&mut self) -> Result<Option<Expr>, ParseError> {
        match self.peek() {
            Some('{') => {
                let saved_arg = self.cmd_arg_mode;
                self.cmd_arg_mode = false;
                let e = self.parse_braced_variable()?;
                self.cmd_arg_mode = saved_arg;
                Ok(Some(e))
            }
            Some('?') => {
                self.next_char();
                Ok(Some(Expr::Variable("?".to_string())))
            }
            Some(c) if c.is_alphanumeric() || c == '_' => {
                let mut name = String::new();
                while let Some(c) = self.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        name.push(c);
                        self.next_char();
                    } else {
                        break;
                    }
                }
                Ok(Some(Expr::Variable(name)))
            }
            _ => Ok(None),
        }
    }

    /// Parse `$| pipeline |` — inline pipeline capture expression.
    /// Called after `$` has been consumed, with `|` as the current char.
    pub(crate) fn parse_inline_pipeline(&mut self) -> Result<Expr, ParseError> {
        self.next_char(); // consume the | after $
        self.skip_whitespace();

        let mut stages = Vec::new();

        loop {
            if self.is_eof() || self.peek() == Some('|') {
                return Err(ParseError::SyntaxError {
                    message: "empty pipeline in inline capture".to_string(),
                    span: self.current_span(),
                });
            }
            stages.push(self.parse_next_stage(stages.is_empty())?);
            self.skip_whitespace();

            // Must see | now
            if self.peek() != Some('|') {
                return Err(ParseError::SyntaxError {
                    message: "expected | to close inline pipeline".to_string(),
                    span: self.current_span(),
                });
            }

            // Peek past | to determine if it's closing or a stage separator
            self.next_char(); // consume |
            self.skip_horizontal_whitespace();

            let is_closing = self.is_eof()
                || self
                    .peek()
                    .is_none_or(|c| matches!(c, '\n' | ';' | ')' | '}' | '#'));

            if is_closing {
                break;
            }
            // Otherwise | is a stage separator — continue to next stage
        }

        Ok(Expr::InlinePipeline(Pipeline { stages }))
    }

    /// Parse `$(pipeline)` — command substitution, desugars to inline pipeline.
    /// Called after `$` has been consumed, with `(` as the current char.
    pub(crate) fn parse_cmd_substitution(&mut self) -> Result<Expr, ParseError> {
        self.next_char(); // consume (
        self.skip_whitespace();

        let mut stages = Vec::new();

        loop {
            if self.peek() == Some(')') {
                // Empty $( )
                self.next_char(); // consume )
                break;
            }
            stages.push(self.parse_next_stage(stages.is_empty())?);
            self.skip_whitespace();

            if self.peek() == Some(')') {
                self.next_char();
                break;
            }
            if self.peek() == Some('|') {
                self.next_char(); // consume | as stage separator
                self.skip_whitespace();
                if self.peek() == Some(')') {
                    self.next_char(); // consume ) — trailing | before )
                    break;
                }
                continue;
            }
            return Err(ParseError::SyntaxError {
                message: "expected ) to close command substitution".to_string(),
                span: self.current_span(),
            });
        }

        Ok(Expr::InlinePipeline(Pipeline { stages }))
    }

    /// Parse `<(pipeline)` or `>(pipeline)` process substitution.
    pub(crate) fn parse_process_substitution(&mut self) -> Result<Expr, ParseError> {
        let direction = if self.peek() == Some('<') {
            ProcessSubstDirection::Input
        } else {
            ProcessSubstDirection::Output
        };
        self.next_char(); // consume < or >
        self.next_char(); // consume (
        self.skip_whitespace();

        let mut stages = Vec::new();

        loop {
            if self.peek() == Some(')') {
                self.next_char(); // consume )
                break;
            }
            stages.push(self.parse_next_stage(stages.is_empty())?);
            self.skip_whitespace();

            if self.peek() == Some(')') {
                self.next_char();
                break;
            }
            if self.peek() == Some('|') {
                self.next_char(); // consume | as stage separator
                self.skip_whitespace();
                if self.peek() == Some(')') {
                    self.next_char(); // consume )
                    break;
                }
                continue;
            }
            return Err(ParseError::SyntaxError {
                message: "expected ) to close process substitution".to_string(),
                span: self.current_span(),
            });
        }

        Ok(Expr::ProcessSubst {
            direction,
            pipeline: Pipeline { stages },
        })
    }

    /// Parse `${var}`, `${var:modifier}`, `${var#pattern}`, `${var/pattern/replacement}`, etc.
    pub(crate) fn parse_braced_variable(&mut self) -> Result<Expr, ParseError> {
        self.next_char(); // consume {

        // Check for ${#var} — string length
        if self.peek() == Some('#') {
            self.next_char(); // consume #
            let name = self.parse_identifier()?;
            self.expect('}')?;
            return Ok(Expr::VarWithModifier {
                name,
                modifier: ParamModifier::StringLength,
            });
        }

        let name = self.parse_identifier()?;
        let mut modifier = None;

        if self.peek() == Some(':') {
            self.next_char(); // consume :
            match self.peek() {
                // :t, :h, :r, :e — legacy path modifiers (single char)
                Some('t') => {
                    self.next_char();
                    modifier = Some(ParamModifier::Tail);
                }
                Some('h') => {
                    self.next_char();
                    modifier = Some(ParamModifier::Head);
                }
                Some('r') => {
                    self.next_char();
                    modifier = Some(ParamModifier::Root);
                }
                Some('e') => {
                    self.next_char();
                    modifier = Some(ParamModifier::Ext);
                }
                Some('u') => {
                    self.next_char();
                    modifier = Some(ParamModifier::Upper);
                }
                Some('l') => {
                    self.next_char();
                    modifier = Some(ParamModifier::Lower);
                }
                // -, =, ?, + — default/assign/error/alternate (check BEFORE digit case)
                Some('-') => {
                    self.next_char(); // consume -
                    // Check if this is a negative offset substring: ${var:-N:M}
                    if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                        let offset = -(self.parse_unsigned_int()? as i64);
                        if self.peek() == Some(':') {
                            // ${var:-N:M} — negative offset substring
                            self.next_char(); // consume :
                            let length = Some(self.parse_unsigned_int()?);
                            modifier = Some(ParamModifier::Substring { offset, length });
                        } else if self.peek() == Some('}') {
                            // ${var:-N} — negative offset substring, no length
                            modifier = Some(ParamModifier::Substring {
                                offset,
                                length: None,
                            });
                        } else {
                            // Not a valid substring — treat as default with the number as text
                            let rest = format!("-{}", offset.abs());
                            modifier = Some(ParamModifier::Default(Box::new(Expr::String(vec![
                                StringPart::Lit(rest),
                            ]))));
                        }
                    } else {
                        let default_expr = self.parse_modifier_value()?;
                        modifier = Some(ParamModifier::Default(Box::new(default_expr)));
                    }
                }
                Some('=') => {
                    self.next_char(); // consume =
                    let default_expr = self.parse_modifier_value()?;
                    modifier = Some(ParamModifier::AssignDefault(Box::new(default_expr)));
                }
                Some('?') => {
                    self.next_char(); // consume ?
                    let msg_expr = self.parse_modifier_value()?;
                    modifier = Some(ParamModifier::ErrorIfUnset(Box::new(msg_expr)));
                }
                Some('+') => {
                    self.next_char(); // consume +
                    let alt_expr = self.parse_modifier_value()?;
                    modifier = Some(ParamModifier::Alternate(Box::new(alt_expr)));
                }
                // Digit — substring (positive offset)
                Some(c) if c.is_ascii_digit() => {
                    let offset = self.parse_signed_int()?;
                    let length = if self.peek() == Some(':') {
                        self.next_char(); // consume :
                        Some(self.parse_unsigned_int()?)
                    } else {
                        None
                    };
                    modifier = Some(ParamModifier::Substring { offset, length });
                }
                Some(c) => {
                    return Err(ParseError::SyntaxError {
                        message: format!(
                            "unknown parameter expansion modifier ':{}' (expected u, l, t, h, r, e, digits, -, =, ?, or +)",
                            c
                        ),
                        span: self.current_span(),
                    });
                }
                None => {
                    return Err(ParseError::UnexpectedEof {
                        span: self.current_span(),
                    });
                }
            }
        } else if self.peek() == Some('#') {
            // # or ## — prefix removal
            self.next_char(); // consume #
            let global = self.peek() == Some('#');
            if global {
                self.next_char();
            } // consume second #
            let pattern = self.parse_braced_pattern()?;
            modifier = Some(if global {
                ParamModifier::LongestPrefix(Box::new(pattern))
            } else {
                ParamModifier::ShortestPrefix(Box::new(pattern))
            });
        } else if self.peek() == Some('%') {
            // % or %% — suffix removal
            self.next_char(); // consume %
            let global = self.peek() == Some('%');
            if global {
                self.next_char();
            } // consume second %
            let pattern = self.parse_braced_pattern()?;
            modifier = Some(if global {
                ParamModifier::LongestSuffix(Box::new(pattern))
            } else {
                ParamModifier::ShortestSuffix(Box::new(pattern))
            });
        } else if self.peek() == Some('/') {
            // /pat/repl or //pat/repl — replace
            self.next_char(); // consume first /
            let global = self.peek() == Some('/');
            if global {
                self.next_char();
            } // consume second /
            let pattern = self.parse_replace_pattern()?;
            self.expect('/')?;
            let replacement = self.parse_replace_pattern()?;
            modifier = Some(ParamModifier::Replace {
                pattern: Box::new(pattern),
                replacement: Box::new(replacement),
                global,
            });
        }

        self.expect('}')?;

        match modifier {
            Some(m) => Ok(Expr::VarWithModifier { name, modifier: m }),
            None => Ok(Expr::Variable(name)),
        }
    }

    /// Parse a signed integer (optional minus followed by digits).
    fn parse_signed_int(&mut self) -> Result<i64, ParseError> {
        let negative = self.peek() == Some('-');
        if negative {
            self.next_char();
        }
        let mut num_str = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                num_str.push(c);
                self.next_char();
            } else {
                break;
            }
        }
        if num_str.is_empty() {
            return Err(ParseError::SyntaxError {
                message: "expected digits after '-'".to_string(),
                span: self.current_span(),
            });
        }
        let val: i64 = num_str.parse().map_err(|_| ParseError::SyntaxError {
            message: format!("number too large: {}", num_str),
            span: self.current_span(),
        })?;
        Ok(if negative { -val } else { val })
    }

    /// Parse an unsigned integer (digits only).
    fn parse_unsigned_int(&mut self) -> Result<u64, ParseError> {
        let mut num_str = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                num_str.push(c);
                self.next_char();
            } else {
                break;
            }
        }
        if num_str.is_empty() {
            return Err(ParseError::SyntaxError {
                message: "expected digits".to_string(),
                span: self.current_span(),
            });
        }
        num_str.parse().map_err(|_| ParseError::SyntaxError {
            message: format!("number too large: {}", num_str),
            span: self.current_span(),
        })
    }

    /// Parse a replace pattern: reads until unescaped `/` or `}`.
    fn parse_replace_pattern(&mut self) -> Result<Expr, ParseError> {
        let mut s = String::new();
        loop {
            match self.peek() {
                Some('\\') => {
                    self.next_char(); // consume backslash
                    if let Some(c) = self.next_char() {
                        s.push(c);
                    }
                }
                Some('/') | Some('}') | None => break,
                Some(c) => {
                    s.push(c);
                    self.next_char();
                }
            }
        }
        Ok(Expr::String(vec![StringPart::Lit(s)]))
    }

    /// Parse the value of a `:-`, `:=`, `:?`, or `:+` modifier: a shell word of
    /// literal text with `$`/`${...}` interpolation, ending at the closing `}`.
    /// A bare word is literal text (bash semantics), never a command call.
    fn parse_modifier_value(&mut self) -> Result<Expr, ParseError> {
        let mut parts = Vec::new();
        let mut current_lit = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(ParseError::UnexpectedEof {
                        span: self.current_span(),
                    });
                }
                Some('}') => break,
                Some('\\') => {
                    self.next_char(); // consume backslash
                    current_lit.push_str(&self.parse_escape_seq()?);
                }
                Some('"') => {
                    // Quoted segment: delegate to the string literal parser and
                    // merge its parts (quote removal, escapes, interpolation).
                    if !current_lit.is_empty() {
                        parts.push(StringPart::Lit(std::mem::take(&mut current_lit)));
                    }
                    if let Expr::String(inner) = self.parse_string_literal()? {
                        parts.extend(inner);
                    }
                }
                Some('$') => {
                    self.next_char();
                    if !current_lit.is_empty() {
                        parts.push(StringPart::Lit(std::mem::take(&mut current_lit)));
                    }
                    if let Some(expr) = self.try_parse_dollar_var()? {
                        parts.push(StringPart::Expr(Box::new(expr)));
                    } else {
                        current_lit.push('$');
                        continue;
                    }
                }
                Some(c) => {
                    current_lit.push(c);
                    self.next_char();
                }
            }
        }
        if !current_lit.is_empty() {
            parts.push(StringPart::Lit(current_lit));
        }
        if parts.is_empty() {
            Ok(Expr::String(vec![StringPart::Lit(String::new())]))
        } else {
            Ok(Expr::String(parts))
        }
    }

    /// Parse a simple pattern inside `${...}` — reads until `}`, returns as a literal string.
    /// Used for prefix/suffix patterns like `*/` in `${var#*/}`.
    fn parse_braced_pattern(&mut self) -> Result<Expr, ParseError> {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c == '}' {
                break;
            }
            s.push(c);
            self.next_char();
        }
        Ok(Expr::String(vec![StringPart::Lit(s)]))
    }

    /// Parse here-string: `<<< "string"` — syntactic sugar for `echo "string" |`.
    pub(crate) fn parse_here_string(&mut self) -> Result<Expr, ParseError> {
        self.next_char(); // consume first <
        self.next_char(); // consume second <
        self.next_char(); // consume third <
        self.skip_whitespace();
        let value = self.parse_primary_expr()?;
        // Build an echo command piped into the rest of the pipeline:
        // "echo value" as a single CommandCall stage
        let echo_stage = PipelineStage::CommandCall {
            name: "echo".to_string(),
            args: vec![value],
            env: Vec::new(),
        };
        Ok(Expr::Pipeline(Pipeline {
            stages: vec![echo_stage],
        }))
    }

    pub(crate) fn parse_if_expr(&mut self) -> Result<Expr, ParseError> {
        self.skip_whitespace();
        // Parse condition
        let saved_arg = self.cmd_arg_mode;
        let saved_redirect = self.redirect_mode;
        self.cmd_arg_mode = false;
        self.redirect_mode = false;
        let condition = self.parse_expr_with_pipeline(false)?;
        self.cmd_arg_mode = saved_arg;
        self.redirect_mode = saved_redirect;
        self.skip_whitespace();
        // Parse then body (required block)
        self.expect('{')?;
        let then_body = self.parse_block_statements()?;
        // Parse optional else
        self.skip_whitespace();
        let else_body = if self.match_keyword("else") {
            self.skip_whitespace();
            // Check for `else if`
            if self.match_keyword("if") {
                let inner_if = self.parse_if_expr()?;
                Some(vec![Stmt::Expr(inner_if)])
            } else if self.peek() == Some('{') {
                self.expect('{')?;
                let body = self.parse_block_statements()?;
                Some(body)
            } else {
                return Err(ParseError::SyntaxError {
                    message: "expected block or if after else".to_string(),
                    span: self.current_span(),
                });
            }
        } else {
            None
        };
        Ok(Expr::If {
            condition: Box::new(condition),
            then_body,
            else_body,
        })
    }

    pub(crate) fn parse_string_literal(&mut self) -> Result<Expr, ParseError> {
        self.expect('"')?;
        let mut parts = Vec::new();
        let mut current_lit = String::new();

        loop {
            match self.peek() {
                None => {
                    return Err(ParseError::SyntaxError {
                        message: "Unterminated string literal".to_string(),
                        span: self.current_span(),
                    });
                }
                Some('"') => {
                    self.next_char();
                    break;
                }
                Some('\\') => {
                    self.next_char(); // skip '\\'
                    current_lit.push_str(&self.parse_escape_seq()?);
                }
                Some('$') => {
                    self.next_char();
                    if !current_lit.is_empty() {
                        parts.push(StringPart::Lit(std::mem::take(&mut current_lit)));
                    }
                    if let Some(expr) = self.try_parse_dollar_var()? {
                        parts.push(StringPart::Expr(Box::new(expr)));
                    } else {
                        current_lit.push('$');
                        continue;
                    }
                }
                Some('{') => {
                    // Check for {{ escape (literal brace)
                    if self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '{' {
                        self.next_char(); // skip first {
                        self.next_char(); // skip second {
                        current_lit.push('{');
                    } else {
                        self.next_char();
                        if !current_lit.is_empty() {
                            parts.push(StringPart::Lit(current_lit));
                            current_lit = String::new();
                        }
                        let saved_arg = self.cmd_arg_mode;
                        self.cmd_arg_mode = false;
                        // allow_pipeline=false so a bare identifier inside `{...}`
                        // is a variable reference (e.g. `"a{b}"`), not a command call.
                        let expr = self.parse_expr_with_pipeline(false)?;
                        self.cmd_arg_mode = saved_arg;
                        self.expect('}')?;
                        parts.push(StringPart::Expr(Box::new(expr)));
                    }
                }
                Some(_) => {
                    current_lit.push(self.next_char().ok_or_else(|| {
                        ParseError::UnexpectedEof {
                            span: self.current_span(),
                        }
                    })?);
                }
            }
        }
        if !current_lit.is_empty() {
            parts.push(StringPart::Lit(current_lit));
        }
        Ok(Expr::String(parts))
    }

    /// Parse a triple-quoted string """...""" with dedent and interpolation.
    pub(crate) fn parse_triple_quoted_string(&mut self) -> Result<Expr, ParseError> {
        // Consume the three opening quotes
        self.next_char(); // "
        self.next_char(); // "
        self.next_char(); // "

        // If the next char is a newline, consume it (opening newline is not content)
        if self.peek() == Some('\n') {
            self.next_char();
        }

        // Collect content until closing """
        let mut content = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(ParseError::SyntaxError {
                        message: "Unterminated triple-quoted string".to_string(),
                        span: self.current_span(),
                    });
                }
                Some('"')
                    if self.pos + 2 < self.input.len()
                        && self.input[self.pos + 1] == '"'
                        && self.input[self.pos + 2] == '"' =>
                {
                    self.next_char(); // "
                    self.next_char(); // "
                    self.next_char(); // "
                    // If the closing """ is followed by a newline, strip trailing newline from content
                    if content.ends_with('\n') {
                        content.pop();
                    }
                    break;
                }
                Some(c) => {
                    content.push(c);
                    self.next_char();
                }
            }
        }

        let dedented = dedent(&content, DedentMode::All);
        let parts = parse_string_parts(&dedented, self.current_span())?;
        Ok(Expr::MultiLineString {
            parts,
            dedent: DedentMode::All,
        })
    }

    /// Parse an ANSI-C quoted string: $'...' with C-style escape sequences.
    pub(crate) fn parse_ansi_c_quote_expr(&mut self) -> Result<Expr, ParseError> {
        // $' already consumed, we're at the opening '
        self.next_char(); // consume '
        let mut raw = String::new();
        loop {
            match self.peek() {
                Some('\'') => {
                    self.next_char(); // consume closing '
                    break;
                }
                Some('\\') => {
                    self.next_char();
                    if let Some(c) = self.next_char() {
                        raw.push('\\');
                        raw.push(c);
                    } else {
                        raw.push('\\');
                    }
                }
                Some(c) => {
                    raw.push(c);
                    self.next_char();
                }
                None => {
                    return Err(ParseError::SyntaxError {
                        message: "Unterminated ANSI-C quoted string".to_string(),
                        span: self.current_span(),
                    });
                }
            }
        }
        let interpreted = parse_ansi_escapes(&raw, self.current_span())?;
        Ok(Expr::AnsiCQuote(interpreted))
    }

    pub(crate) fn parse_heredoc_header(&mut self) -> Result<(String, bool, bool), ParseError> {
        // Assumes `<<` already consumed; parses optional `-` and delimiter, skips trailing horizontal whitespace, but does NOT consume body.
        let strip_tabs = if self.peek() == Some('-') {
            self.next_char();
            true
        } else {
            false
        };
        self.skip_horizontal_whitespace();
        let delimiter: String;
        let quoted_delimiter: bool;
        match self.peek() {
            Some('\'') => {
                quoted_delimiter = true;
                self.next_char();
                let mut d = String::new();
                while let Some(c) = self.peek() {
                    if c == '\'' {
                        self.next_char();
                        break;
                    }
                    d.push(self.next_char().ok_or_else(|| ParseError::UnexpectedEof {
                        span: self.current_span(),
                    })?);
                }
                delimiter = d;
            }
            Some('"') => {
                quoted_delimiter = true;
                self.next_char();
                let mut d = String::new();
                while let Some(c) = self.peek() {
                    if c == '"' {
                        self.next_char();
                        break;
                    }
                    d.push(self.next_char().ok_or_else(|| ParseError::UnexpectedEof {
                        span: self.current_span(),
                    })?);
                }
                delimiter = d;
            }
            _ => {
                quoted_delimiter = false;
                delimiter = self.parse_identifier()?;
            }
        }
        // Skip only horizontal whitespace after delimiter; the outer `parse_pipeline` will handle following `> file` redirects before body.
        while self.peek() == Some(' ') || self.peek() == Some('\t') {
            self.next_char();
        }
        Ok((delimiter, strip_tabs, quoted_delimiter))
    }

    pub(crate) fn collect_heredoc_body(
        &mut self,
        delimiter: &str,
        strip_tabs: bool,
        quoted: bool,
    ) -> Result<Expr, ParseError> {
        // Body starts at beginning of next line. Consume the single newline after delimiter line if present.
        if self.peek() == Some('\n') || self.peek() == Some('\r') {
            self.next_char();
            if self.peek() == Some('\n') {
                self.next_char();
            }
        } else if !self.is_eof() {
            // If we are still on same line (e.g. `<<EOF > file` case, the `> file` was already parsed and we are at `\n`), ensure we move to next line.
            while self.peek() == Some(' ') || self.peek() == Some('\t') {
                self.next_char();
            }
            if self.peek() == Some('\n') || self.peek() == Some('\r') {
                self.next_char();
                if self.peek() == Some('\n') {
                    self.next_char();
                }
            }
        }
        let mut content = String::new();
        let mut found_delimiter = false;
        loop {
            if self.is_eof() {
                break;
            }
            let saved = self.pos;
            let line = self.read_line_content();
            let trimmed_line = if strip_tabs {
                line.trim_start_matches('\t')
            } else {
                &line
            };
            if trimmed_line.trim_end_matches('\n') == delimiter
                || trimmed_line.trim_end_matches('\r') == delimiter
                || trimmed_line.trim_end_matches('\n').trim_end_matches('\r') == delimiter
                || trimmed_line.trim() == delimiter
            {
                self.pos = saved;
                while let Some(c) = self.peek() {
                    if c == '\n' {
                        self.next_char();
                        break;
                    }
                    if c == '\r' {
                        self.next_char();
                        if self.peek() == Some('\n') {
                            self.next_char();
                        }
                        break;
                    }
                    self.next_char();
                }
                found_delimiter = true;
                break;
            }
            self.pos = saved;
            while let Some(c) = self.peek() {
                if c == '\n' || c == '\r' {
                    if c == '\r' {
                        self.next_char();
                    }
                    break;
                }
                content.push(c);
                self.next_char();
            }
            content.push('\n');
            if self.peek() == Some('\n') {
                self.next_char();
            }
        }
        if !found_delimiter {
            return Err(ParseError::UnexpectedEof {
                span: self.current_span(),
            });
        }
        let dedent_mode = if quoted {
            DedentMode::None
        } else if strip_tabs {
            DedentMode::LeadingTabs
        } else {
            DedentMode::All
        };
        if quoted {
            let dedented = dedent(&content, dedent_mode);
            Ok(Expr::RawMultiLineString(dedented))
        } else {
            let dedented = dedent(&content, dedent_mode);
            let parts = parse_string_parts(&dedented, self.current_span())?;
            Ok(Expr::MultiLineString {
                parts,
                dedent: dedent_mode,
            })
        }
    }

    pub(crate) fn parse_heredoc_inner(&mut self) -> Result<(Expr, bool, bool), ParseError> {
        let (delimiter, strip_tabs, quoted) = self.parse_heredoc_header()?;
        // For `let x = <<EOF` immediate body: consume newline and read body now.
        let expr = self.collect_heredoc_body(&delimiter, strip_tabs, quoted)?;
        Ok((expr, strip_tabs, quoted))
    }

    /// Parse a heredoc: <<DELIM ... DELIM or <<'DELIM' ... DELIM or <<-DELIM ... DELIM
    pub(crate) fn parse_heredoc_expr(&mut self) -> Result<Expr, ParseError> {
        self.next_char(); // <
        self.next_char(); // <
        let (expr, _, _) = self.parse_heredoc_inner()?;
        Ok(expr)
    }

    /// Read the rest of the current line (for heredoc delimiter matching).
    pub(crate) fn read_line_content(&self) -> String {
        let start = self.pos;
        let mut end = self.pos;
        while end < self.input.len() {
            let c = self.input[end];
            if c == '\n' || c == '\r' {
                break;
            }
            end += 1;
        }
        self.input[start..end].iter().collect()
    }

    pub(crate) fn parse_number_literal(&mut self) -> Result<Expr, ParseError> {
        let start = self.pos;
        let mut val_str = String::new();
        if self.peek() == Some('-') {
            val_str.push(self.next_char().ok_or_else(|| ParseError::UnexpectedEof {
                span: self.current_span(),
            })?);
        }

        let mut last_was_underscore = false;
        let mut has_dot = false;
        let mut has_exp = false;

        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                last_was_underscore = false;
                val_str.push(self.next_char().ok_or_else(|| ParseError::UnexpectedEof {
                    span: self.current_span(),
                })?);
            } else if c == '_' {
                self.next_char(); // skip underscores
                last_was_underscore = true;
            } else if c == '.' && !has_dot && !has_exp {
                has_dot = true;
                val_str.push(self.next_char().ok_or_else(|| ParseError::UnexpectedEof {
                    span: self.current_span(),
                })?);
            } else if (c == 'e' || c == 'E') && !has_exp {
                has_exp = true;
                val_str.push(self.next_char().ok_or_else(|| ParseError::UnexpectedEof {
                    span: self.current_span(),
                })?);
                if let Some(next_c) = self.peek()
                    && (next_c == '+' || next_c == '-')
                {
                    val_str.push(self.next_char().ok_or_else(|| ParseError::UnexpectedEof {
                        span: self.current_span(),
                    })?);
                }
            } else {
                break;
            }
        }

        if last_was_underscore {
            return Err(ParseError::SyntaxError {
                message: "Trailing underscore in number literal".to_string(),
                span: self.current_span(),
            });
        }

        // Duration suffixes for `sleep` etc.: ms, s, m, h — must be checked before size suffixes
        // because `m` is ambiguous (mebibytes vs minutes) and `ms` would otherwise be
        // mis-parsed as size `M` + leftover `s`.
        // Valid units (case-insensitive): ms, s, m, h
        let mut raw_suffix = String::new();
        let mut look = self.pos;
        while look < self.input.len() && self.input[look].is_ascii_alphabetic() {
            raw_suffix.push(self.input[look]);
            look += 1;
        }

        if !raw_suffix.is_empty() {
            let lower = raw_suffix.to_ascii_lowercase();
            // Duration literals: 5ms, 10s, 2m, 1h — return as string so `sleep`'s
            // parse_duration can handle them uniformly (avoids size/duration conflict).
            if matches!(lower.as_str(), "ms" | "s" | "m" | "h") {
                for _ in 0..raw_suffix.len() {
                    self.next_char();
                }
                let lit = format!("{}{}", val_str, raw_suffix);
                return Ok(Expr::String(vec![StringPart::Lit(lit)]));
            }
        }

        if raw_suffix.is_empty() {
            if has_dot || has_exp {
                let f = val_str
                    .parse::<f64>()
                    .map_err(|e| ParseError::SyntaxError {
                        message: format!("Invalid float: {}", e),
                        span: self.current_span(),
                    })?;
                return Ok(Expr::Float(f));
            } else {
                let i = val_str
                    .parse::<i64>()
                    .map_err(|e| ParseError::SyntaxError {
                        message: format!("Invalid integer: {}", e),
                        span: self.current_span(),
                    })?;
                return Ok(Expr::Int(i));
            }
        }

        let lower = raw_suffix.to_ascii_lowercase();
        // Try longest valid prefix of raw_suffix.
        let mut matched: Option<(usize, i64)> = None;
        for len in (1..=raw_suffix.len()).rev() {
            let cand = &lower[..len];
            let factor = match cand {
                "b" => Some(1_i64),
                "kb" => Some(1_000),
                "mb" => Some(1_000_000),
                "gb" => Some(1_000_000_000),
                "tb" => Some(1_000_000_000_000),
                "pb" => Some(1_000_000_000_000_000),
                "eb" => Some(1_000_000_000_000_000_000),
                "kib" => Some(1_024),
                "mib" => Some(1_048_576),
                "gib" => Some(1_073_741_824),
                "tib" => Some(1_099_511_627_776),
                "pib" => Some(1_125_899_906_842_624),
                "eib" => Some(1_152_921_504_606_846_976),
                "k" => Some(1_024),
                "m" => Some(1_048_576),
                "g" => Some(1_073_741_824),
                "t" => Some(1_099_511_627_776),
                "p" => Some(1_125_899_906_842_624),
                "e" => Some(1_152_921_504_606_846_976),
                _ => None,
            };
            if let Some(f) = factor {
                matched = Some((len, f));
                break;
            }
        }

        if let Some((consumed, factor)) = matched {
            // Consume only the matched prefix, leave remainder for next token.
            for _ in 0..consumed {
                self.next_char();
            }
            if has_dot || has_exp {
                let f = val_str
                    .parse::<f64>()
                    .map_err(|e| ParseError::SyntaxError {
                        message: format!("Invalid float: {}", e),
                        span: self.current_span(),
                    })?;
                let scaled = f * factor as f64;
                if scaled.is_infinite() || scaled.is_nan() {
                    return Err(ParseError::SyntaxError {
                        message: format!(
                            "Size literal overflow: {}{} exceeds representable range",
                            val_str,
                            &raw_suffix[..consumed]
                        ),
                        span: self.span_from(start),
                    });
                }
                // If scaled value is integral and fits in i64, return Int for type stability.
                if scaled.fract() == 0.0 && scaled >= i64::MIN as f64 && scaled <= i64::MAX as f64 {
                    return Ok(Expr::Int(scaled as i64));
                }
                return Ok(Expr::Float(scaled));
            } else {
                let i = val_str
                    .parse::<i64>()
                    .map_err(|e| ParseError::SyntaxError {
                        message: format!("Invalid integer: {}", e),
                        span: self.current_span(),
                    })?;
                let scaled = i
                    .checked_mul(factor)
                    .ok_or_else(|| ParseError::SyntaxError {
                        message: format!(
                            "Size literal overflow: {}{} ({} * {}) exceeds i64",
                            val_str,
                            &raw_suffix[..consumed],
                            i,
                            factor
                        ),
                        span: self.span_from(start),
                    })?;
                return Ok(Expr::Int(scaled));
            }
        }

        // No valid prefix — consume entire contiguous alphabetic run to report a clear error.
        for _ in 0..raw_suffix.len() {
            self.next_char();
        }
        Err(ParseError::SyntaxError {
            message: format!(
                "Unknown size suffix '{}' after number. Valid units: B, KB (1000), MB, GB, TB, PB, EB and KiB (1024), MiB, GiB, TiB, PiB, EiB, or shorthand K/M/G/T/P/E (1024-based). Use no space, e.g., 100KB or 1.5MiB, or plain bytes like 1_000_000",
                raw_suffix
            ),
            span: self.span_from(start),
        })
    }
}
