// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::ast::*;
use crate::expand_braces;
use crate::{ParseError, Parser};
use miette::SourceSpan;

impl Parser {
    /// Primary entry point: parse a block of statements.
    pub fn parse_statements(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut stmts = Vec::new();
        // Skip shebang line (#!/usr/bin/env fsh or similar) at the start of scripts.
        if self.pos + 1 < self.input.len() && self.input[0] == '#' && self.input[1] == '!' {
            while let Some(c) = self.peek() {
                if c == '\n' || c == '\r' {
                    self.next_char();
                    break;
                }
                self.next_char();
            }
        }
        self.skip_whitespace();
        while !self.is_eof() {
            if self.peek() == Some(';') {
                self.next_char();
                self.skip_whitespace();
                continue;
            }
            let mut stmt = self.parse_statement()?;
            self.skip_whitespace();
            while self.pos + 1 < self.input.len() {
                if self.input[self.pos] == '&' && self.input[self.pos + 1] == '&' {
                    self.pos += 2;
                    let rhs = self.parse_statement()?;
                    stmt = Stmt::And(Box::new(stmt), Box::new(rhs));
                    self.skip_whitespace();
                } else if self.input[self.pos] == '|' && self.input[self.pos + 1] == '|' {
                    self.pos += 2;
                    let rhs = self.parse_statement()?;
                    stmt = Stmt::Or(Box::new(stmt), Box::new(rhs));
                    self.skip_whitespace();
                } else {
                    break;
                }
            }
            stmts.push(stmt);
            if self.peek() == Some(';') {
                self.next_char();
                self.skip_whitespace();
            }
        }
        Ok(stmts)
    }
}

impl Parser {
    pub(crate) fn parse_update(&mut self, op: BinOp) -> Result<Stmt, ParseError> {
        if self.peek() == Some('$') {
            self.next_char();
        }
        let name = self.parse_identifier()?;
        self.skip_whitespace();
        self.next_char(); // consume the operator char (+, -, *, /)
        self.expect('=')?;
        let expr = self.parse_expr()?;
        Ok(Stmt::Update { name, op, expr })
    }

    pub fn parse_statement(&mut self) -> Result<Stmt, ParseError> {
        self.skip_whitespace();
        let start = self.pos;
        if self.peek() == Some('#') {
            self.next_char(); // Consume '#'
            let mut comment_content = String::new();
            while let Some(c) = self.peek() {
                if c == '\n' || c == '\r' {
                    break;
                }
                comment_content.push(c);
                self.next_char();
            }
            let span = self.span_from(start);
            return Ok(Stmt::Spanned {
                stmt: Box::new(Stmt::Comment(comment_content)),
                span,
            });
        }
        let stmt = self.parse_statement_body()?;
        // Check for trailing & (background operator) — must not be &&
        self.skip_horizontal_whitespace();
        if self.peek() == Some('&')
            && !(self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '&')
        {
            self.next_char(); // consume &
            let stmt = Stmt::Background(Box::new(stmt));
            let span = self.span_from(start);
            return Ok(Stmt::Spanned {
                stmt: Box::new(stmt),
                span,
            });
        }
        let span = self.span_from(start);
        Ok(Stmt::Spanned {
            stmt: Box::new(stmt),
            span,
        })
    }

    pub(crate) fn parse_statement_body(&mut self) -> Result<Stmt, ParseError> {
        self.skip_whitespace();

        // Check for variable assignment or update statement (e.g. `a = 3` or `a += 3`)
        if let Some(name) = self.peek_identifier()
            && !matches!(
                name.as_str(),
                "let"
                    | "local"
                    | "fn"
                    | "match"
                    | "try"
                    | "catch"
                    | "with"
                    | "unsafe"
                    | "break"
                    | "continue"
                    | "return"
                    | "for"
                    | "in"
                    | "exit"
                    | "on"
                    | "sh"
                    | "posix"
                    | "bash"
            )
        {
            // Peek ahead for =, +=, -=, *=, /=
            let mut peek_pos = self.pos;
            // skip the identifier
            while peek_pos < self.input.len() {
                let c = self.input[peek_pos];
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    peek_pos += 1;
                } else {
                    break;
                }
            }
            while peek_pos < self.input.len() && self.input[peek_pos].is_whitespace() {
                peek_pos += 1;
            }
            if peek_pos < self.input.len() {
                let c = self.input[peek_pos];
                if c == '=' {
                    if peek_pos + 1 < self.input.len() && self.input[peek_pos + 1] == '=' {
                        // `==` comparison — not an assignment
                    } else {
                        // Check if this looks like inline env var (VAR=value command)
                        // vs simple assignment (VAR=value)
                        let eq_pos = peek_pos;
                        let after_eq = eq_pos + 1;
                        // Skip past the value to see if there's a command after
                        let mut scan_pos = after_eq;
                        // Skip whitespace after =
                        while scan_pos < self.input.len() && self.input[scan_pos].is_whitespace() {
                            scan_pos += 1;
                        }
                        // Now scan_pos should be at the value
                        // Skip the value (quoted string, bare word, or $VAR)
                        if scan_pos < self.input.len() {
                            if self.input[scan_pos] == '"' {
                                // Skip quoted string
                                scan_pos += 1;
                                while scan_pos < self.input.len() && self.input[scan_pos] != '"' {
                                    if self.input[scan_pos] == '\\' {
                                        scan_pos += 1; // skip escaped char
                                    }
                                    scan_pos += 1;
                                }
                                if scan_pos < self.input.len() {
                                    scan_pos += 1; // skip closing quote
                                }
                            } else if self.input[scan_pos] == '$' {
                                // Skip $VAR
                                scan_pos += 1;
                                while scan_pos < self.input.len()
                                    && (self.input[scan_pos].is_alphanumeric()
                                        || self.input[scan_pos] == '_')
                                {
                                    scan_pos += 1;
                                }
                            } else {
                                // Skip bare word
                                while scan_pos < self.input.len()
                                    && !self.input[scan_pos].is_whitespace()
                                    && self.input[scan_pos] != '|'
                                    && self.input[scan_pos] != ';'
                                {
                                    scan_pos += 1;
                                }
                            }
                        }
                        // Now check if there's more content after the value
                        let mut after_value = scan_pos;
                        while after_value < self.input.len()
                            && self.input[after_value].is_whitespace()
                        {
                            after_value += 1;
                        }
                        let has_command_after = if after_value < self.input.len() {
                            let c = self.input[after_value];
                            // Check if it looks like a command (identifier, path, etc.)
                            c.is_alphabetic()
                                || c == '_'
                                || c == '/'
                                || c == '~'
                                || c == '.'
                                || c == '$'
                        } else {
                            false
                        };

                        if has_command_after {
                            // This is inline env var syntax: VAR=value command
                            // Collect all inline env vars
                            let mut inline_env: Vec<(String, Expr)> = Vec::new();
                            loop {
                                if self.peek() == Some('$') {
                                    self.next_char();
                                }
                                let name = self.parse_identifier()?;
                                self.expect('=')?;
                                let value = match self.peek() {
                                    Some('"') => self.parse_string_literal()?,
                                    Some('$') => {
                                        self.next_char();
                                        Expr::Variable(self.parse_identifier()?)
                                    }
                                    _ => {
                                        let mut val = String::new();
                                        while let Some(c) = self.peek() {
                                            if c.is_whitespace()
                                                || c == '|'
                                                || c == '\n'
                                                || c == '\r'
                                                || c == ';'
                                                || c == '}'
                                            {
                                                break;
                                            }
                                            val.push(self.next_char().ok_or_else(|| {
                                                ParseError::UnexpectedEof {
                                                    span: self.current_span(),
                                                }
                                            })?);
                                        }
                                        Expr::String(vec![StringPart::Lit(val)])
                                    }
                                };
                                inline_env.push((name, value));
                                self.skip_horizontal_whitespace();
                                // Check if next token is another VAR=value or a command
                                let saved = self.pos;
                                if let Some(next_ident) = self.peek_identifier() {
                                    let mut peek = self.pos + next_ident.len();
                                    while peek < self.input.len()
                                        && self.input[peek].is_whitespace()
                                    {
                                        peek += 1;
                                    }
                                    if peek < self.input.len()
                                        && self.input[peek] == '='
                                        && (peek + 1 >= self.input.len()
                                            || self.input[peek + 1] != '=')
                                    {
                                        // Another env var
                                        continue;
                                    }
                                }
                                self.pos = saved;
                                break;
                            }
                            // Parse the rest as a pipeline stage
                            self.skip_horizontal_whitespace();
                            let first_stage = self.parse_pipeline_stage_with_env(inline_env)?;
                            let mut stages = vec![first_stage];
                            self.skip_whitespace();
                            // Consume redirects before the next pipe
                            while let Some(write_stage) = self.parse_redirect()? {
                                stages.push(write_stage);
                                self.skip_whitespace();
                            }
                            while self.peek() == Some('|') {
                                if self.pos + 1 < self.input.len()
                                    && self.input[self.pos + 1] == '|'
                                {
                                    break;
                                }
                                self.next_char();
                                let saved_subsequent = self.is_subsequent_stage;
                                self.is_subsequent_stage = true;
                                let stage_res = self.parse_pipeline_stage();
                                self.is_subsequent_stage = saved_subsequent;
                                stages.push(stage_res?);
                                self.skip_whitespace();
                                // Consume redirects after this stage (before next pipe or end)
                                while let Some(write_stage) = self.parse_redirect()? {
                                    stages.push(write_stage);
                                    self.skip_whitespace();
                                }
                            }
                            // Check for redirect
                            if let Some(write_stage) = self.parse_redirect()? {
                                stages.push(write_stage);
                            }
                            return Ok(Stmt::Expr(Expr::Pipeline(Pipeline { stages })));
                        } else {
                            // Simple assignment
                            if self.peek() == Some('$') {
                                self.next_char();
                            }
                            let name = self.parse_identifier()?;
                            self.expect('=')?;
                            let expr = self.parse_expr_with_pipeline(false)?;
                            return Ok(Stmt::Assign { name, expr });
                        }
                    }
                } else if c == '+'
                    && peek_pos + 1 < self.input.len()
                    && self.input[peek_pos + 1] == '='
                {
                    return self.parse_update(BinOp::Add);
                } else if c == '-'
                    && peek_pos + 1 < self.input.len()
                    && self.input[peek_pos + 1] == '='
                {
                    return self.parse_update(BinOp::Sub);
                } else if c == '*'
                    && peek_pos + 1 < self.input.len()
                    && self.input[peek_pos + 1] == '='
                {
                    return self.parse_update(BinOp::Mul);
                } else if c == '/'
                    && peek_pos + 1 < self.input.len()
                    && self.input[peek_pos + 1] == '='
                {
                    return self.parse_update(BinOp::Div);
                }
            }
        }

        // Fall through to expression parsing

        if self.match_keyword("local") {
            let name = self.parse_identifier()?;
            self.skip_horizontal_whitespace();
            let expr = if self.peek() == Some('=') {
                self.next_char();
                self.skip_horizontal_whitespace();
                Some(self.parse_expr()?)
            } else {
                None
            };
            Ok(Stmt::Local { name, expr })
        } else if self.match_keyword("let") {
            let name = self.parse_identifier()?;
            self.expect('=')?;
            let expr = self.parse_expr()?;
            Ok(Stmt::Let { name, expr })
        } else if self.match_keyword("fn") {
            let name = self.parse_identifier()?;
            self.skip_whitespace();
            let mut params = Vec::new();
            let mut ret_type = None;
            if self.peek() == Some('(') {
                // fn name(params) { ... }
                self.next_char(); // consume '('
                self.skip_whitespace();
                if self.peek() != Some(')') {
                    loop {
                        let param_name = self.parse_identifier()?;
                        // Parse optional structural constraint
                        let mut constraint = TypeConstraint::Any;
                        self.skip_whitespace();
                        if self.peek() == Some(':') {
                            self.next_char();
                            constraint = self.parse_type_constraint()?;
                        }
                        params.push(Param {
                            name: param_name,
                            constraint,
                        });
                        self.skip_whitespace();
                        if self.peek() == Some(',') {
                            self.next_char();
                        } else {
                            break;
                        }
                    }
                }
                self.expect(')')?;
                self.skip_whitespace();
                if self.peek() == Some('-') {
                    self.expect_str("->")?;
                    ret_type = Some(self.parse_identifier()?);
                }
            } else if self.peek() == Some('-') {
                // fn name -> RetType { ... }
                self.expect_str("->")?;
                ret_type = Some(self.parse_identifier()?);
            } else if self
                .peek()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            {
                // fn name param1 param2 { ... } — bare params without parens
                loop {
                    let param_name = self.parse_identifier()?;
                    let mut constraint = TypeConstraint::Any;
                    self.skip_whitespace();
                    if self.peek() == Some(':') {
                        self.next_char();
                        constraint = self.parse_type_constraint()?;
                    }
                    params.push(Param {
                        name: param_name,
                        constraint,
                    });
                    self.skip_whitespace();
                    if self.peek() == Some('{') || self.peek() == Some('}') || self.peek().is_none()
                    {
                        break;
                    }
                }
            }
            // If no ( and no params, expect { directly.
            // If we parsed params, expect { here too.
            self.expect('{')?;
            let body = self.parse_block_statements()?;
            Ok(Stmt::FnDef {
                name,
                params,
                ret_type,
                body,
            })
        } else if self.match_keyword("try") {
            self.expect('{')?;
            let try_body = self.parse_block_statements()?;
            self.expect_str("catch")?;
            self.skip_whitespace();
            let catch_var = if self.peek() == Some('|') {
                // try { ... } catch |e| { ... }
                self.next_char(); // consume |
                let var = self.parse_identifier()?;
                self.skip_whitespace();
                self.expect('|')?;
                var
            } else {
                // try { ... } catch e { ... }
                self.parse_identifier()?
            };
            self.skip_whitespace();
            self.expect('{')?;
            let catch_body = self.parse_block_statements()?;
            Ok(Stmt::TryCatch {
                try_body,
                catch_var,
                catch_body,
            })
        } else if self.match_keyword("match") {
            let expr = self.parse_expr_with_pipeline(false)?;
            self.skip_whitespace();
            self.expect('{')?;
            let mut arms = Vec::new();
            self.skip_whitespace();
            while self.peek() != Some('}') && !self.is_eof() {
                let pattern = self.parse_match_pattern()?;
                self.skip_whitespace();
                self.expect_str("=>")?;
                self.skip_whitespace();
                // Body can be a single expression or a block
                let body = if self.peek() == Some('{') {
                    self.next_char();
                    self.parse_block_statements()?
                } else {
                    let expr = self.parse_expr_with_pipeline(false)?;
                    vec![Stmt::Expr(expr)]
                };
                arms.push(MatchArm { pattern, body });
                self.skip_whitespace();
                // Consume optional trailing comma or semicolon
                if self.peek() == Some(',') || self.peek() == Some(';') {
                    self.next_char();
                }
                self.skip_whitespace();
            }
            self.expect('}')?;
            Ok(Stmt::Match { expr, arms })
        } else if self.match_keyword("with") {
            self.expect_str("caps")?;
            self.expect('(')?;
            let mut caps = Vec::new();
            self.skip_whitespace();
            if self.peek() != Some(')') {
                loop {
                    caps.push(self.parse_expr()?);
                    self.skip_whitespace();
                    if self.peek() == Some(',') {
                        self.next_char();
                    } else {
                        break;
                    }
                }
            }
            self.expect(')')?;
            self.expect('{')?;
            let body = self.parse_block_statements()?;
            Ok(Stmt::WithCaps { caps, body })
        } else if self.match_keyword("unsafe") {
            self.expect('{')?;
            let body = self.parse_block_statements()?;
            Ok(Stmt::Unsafe { body })
        } else if self.peek_posix_block() {
            // Consume the keyword and opening brace, then capture body
            let _ = self.match_keyword("sh")
                || self.match_keyword("posix")
                || self.match_keyword("bash");
            self.skip_whitespace();
            if self.peek() != Some('{') {
                return Err(ParseError::SyntaxError {
                    message: "Expected '{' after POSIX block keyword".to_string(),
                    span: self.current_span(),
                });
            }
            self.next_char(); // consume '{'
            let body = self.parse_posix_block_body()?;
            Ok(Stmt::PosixBlock { body })
        } else if self.match_keyword("source") {
            self.skip_horizontal_whitespace();
            let mut bash = false;
            if self.match_keyword("--bash") {
                bash = true;
                self.skip_horizontal_whitespace();
            }
            let path = self.parse_expr_with_pipeline(false)?;
            Ok(Stmt::Source { path, bash })
        } else if self.match_keyword("while") {
            let saved_arg = self.cmd_arg_mode;
            let saved_redirect = self.redirect_mode;
            self.cmd_arg_mode = false;
            self.redirect_mode = false;
            let condition = self.parse_expr_with_pipeline(false)?;
            self.cmd_arg_mode = saved_arg;
            self.redirect_mode = saved_redirect;
            self.skip_whitespace();
            self.expect('{')?;
            let body = self.parse_block_statements()?;
            Ok(Stmt::While { condition, body })
        } else if self.match_keyword("for") {
            // `for <var> in <iterable> { <body> }`
            let var = self.parse_identifier()?;
            self.skip_whitespace();
            // consume "in" keyword
            if !self.match_keyword("in") {
                return Err(ParseError::SyntaxError {
                    message: "Expected 'in' after loop variable in 'for' statement".to_string(),
                    span: self.current_span(),
                });
            }
            let iter = self.parse_expr_with_pipeline(false)?;
            self.skip_whitespace();
            self.expect('{')?;
            let body = self.parse_block_statements()?;
            Ok(Stmt::For { var, iter, body })
        } else if self.match_keyword("break") {
            Ok(Stmt::Break)
        } else if self.match_keyword("continue") {
            Ok(Stmt::Continue)
        } else if self.match_keyword("every") {
            self.skip_horizontal_whitespace();
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
                    message: "Expected duration number after 'every'".to_string(),
                    span: self.current_span(),
                });
            }
            let duration: u64 = num_str.parse().map_err(|_| ParseError::SyntaxError {
                message: "Invalid duration number".to_string(),
                span: self.current_span(),
            })?;
            self.skip_horizontal_whitespace();
            let unit = match self.peek() {
                Some('s') => {
                    self.next_char();
                    TimeUnit::Second
                }
                Some('m') => {
                    self.next_char();
                    TimeUnit::Minute
                }
                Some('h') => {
                    self.next_char();
                    TimeUnit::Hour
                }
                _ => {
                    return Err(ParseError::SyntaxError {
                        message: "Expected duration unit ('s', 'm', or 'h') after number"
                            .to_string(),
                        span: self.current_span(),
                    });
                }
            };
            self.skip_whitespace();
            self.expect('{')?;
            let body = self.parse_block_statements()?;
            Ok(Stmt::Every {
                duration,
                unit,
                body,
            })
        } else if self.match_keyword("return") {
            self.skip_horizontal_whitespace();
            // If the next thing is a newline, semicolon, or end-of-block, return null.
            let expr = if self.peek() == Some('}')
                || self.peek() == Some(';')
                || self.peek() == Some('\n')
                || self.is_eof()
            {
                Expr::Null
            } else {
                self.parse_expr()?
            };
            Ok(Stmt::Return(expr))
        } else if self.match_keyword("exit") {
            self.skip_horizontal_whitespace();
            let expr = if self.peek() == Some('}')
                || self.peek() == Some(';')
                || self.peek() == Some('\n')
                || self.is_eof()
            {
                None
            } else {
                Some(self.parse_expr()?)
            };
            Ok(Stmt::Exit(expr))
        } else if self.match_keyword("on") {
            self.skip_horizontal_whitespace();
            let signal = self.parse_identifier()?;
            self.skip_horizontal_whitespace();
            let handler = if self.peek() == Some('{') {
                self.next_char();
                let body = self.parse_block_statements()?;
                OnHandler::Block(body)
            } else {
                let name = self.parse_identifier()?;
                OnHandler::FunctionName(name)
            };
            Ok(Stmt::On { signal, handler })
        } else if self.peek() == Some('$') {
            // Check if it's dynamic cell declaration $=
            let temp_pos = self.pos;
            if temp_pos + 1 < self.input.len() && self.input[temp_pos + 1] == '=' {
                self.pos += 2; // skip $=
                let name = self.parse_identifier()?;
                self.expect('=')?;
                self.skip_whitespace();
                if self.match_keyword("every") {
                    self.skip_horizontal_whitespace();
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
                            message: "Expected duration number after 'every'".to_string(),
                            span: self.current_span(),
                        });
                    }
                    let duration: u64 = num_str.parse().map_err(|_| ParseError::SyntaxError {
                        message: "Invalid duration number".to_string(),
                        span: self.current_span(),
                    })?;
                    self.skip_horizontal_whitespace();
                    let unit = match self.peek() {
                        Some('s') => {
                            self.next_char();
                            TimeUnit::Second
                        }
                        Some('m') => {
                            self.next_char();
                            TimeUnit::Minute
                        }
                        Some('h') => {
                            self.next_char();
                            TimeUnit::Hour
                        }
                        _ => {
                            return Err(ParseError::SyntaxError {
                                message: "Expected duration unit ('s', 'm', or 'h') after number"
                                    .to_string(),
                                span: self.current_span(),
                            });
                        }
                    };
                    self.skip_whitespace();
                    self.expect('{')?;
                    let body = self.parse_block_statements()?;
                    Ok(Stmt::ReactiveCellEvery {
                        name,
                        duration,
                        unit,
                        body,
                    })
                } else {
                    let pipeline = self.parse_pipeline()?;
                    Ok(Stmt::ReactiveCell { name, pipeline })
                }
            } else {
                // Normal expression starting with $ (e.g. variable $val)
                let expr = self.parse_expr()?;
                Ok(Stmt::Expr(expr))
            }
        } else {
            let expr = self.parse_expr()?;
            Ok(Stmt::Expr(expr))
        }
    }

    /// Peek if next tokens are a POSIX block keyword `sh`/`posix`/`bash` followed by `{`.
    /// Pure lookahead — does not consume input.
    pub(crate) fn peek_posix_block(&self) -> bool {
        let mut p = self.pos;
        while p < self.input.len() && self.input[p].is_whitespace() {
            p += 1;
        }
        let kw = if p + 2 <= self.input.len() && self.input[p..p + 2] == ['s', 'h'] {
            let end = p + 2;
            if end < self.input.len()
                && (self.input[end].is_alphanumeric() || self.input[end] == '_')
            {
                return false;
            }
            p = end;
            true
        } else if p + 5 <= self.input.len() && self.input[p..p + 5] == ['p', 'o', 's', 'i', 'x'] {
            let end = p + 5;
            if end < self.input.len()
                && (self.input[end].is_alphanumeric() || self.input[end] == '_')
            {
                return false;
            }
            p = end;
            true
        } else if p + 4 <= self.input.len() && self.input[p..p + 4] == ['b', 'a', 's', 'h'] {
            let end = p + 4;
            if end < self.input.len()
                && (self.input[end].is_alphanumeric() || self.input[end] == '_')
            {
                return false;
            }
            p = end;
            true
        } else {
            return false;
        };
        if !kw {
            return false;
        }
        while p < self.input.len() && self.input[p].is_whitespace() {
            p += 1;
        }
        p < self.input.len() && self.input[p] == '{'
    }

    /// Parse raw POSIX block body: balanced-brace capture until matching `}`.
    /// Does not parse as fsh — the content is handed to the POSIX engine.
    pub(crate) fn parse_posix_block_body(&mut self) -> Result<String, ParseError> {
        let start = self.pos;
        let mut depth = 1usize;
        let mut in_single = false;
        let mut in_double = false;
        let mut escaped = false;
        while self.pos < self.input.len() && depth > 0 {
            let c = self.input[self.pos];
            if escaped {
                escaped = false;
            } else if in_single {
                if c == '\'' {
                    in_single = false;
                }
            } else if in_double {
                if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_double = false;
                }
            } else {
                match c {
                    '\'' => in_single = true,
                    '"' => in_double = true,
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if depth > 0 {
                self.pos += 1;
            }
        }
        if depth != 0 {
            return Err(ParseError::SyntaxError {
                message: "Unclosed POSIX block — missing '}'".to_string(),
                span: self.span_from(start),
            });
        }
        let body: String = self.input[start..self.pos].iter().collect();
        self.next_char(); // consume closing '}'
        Ok(body)
    }

    pub(crate) fn parse_block_statements(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut stmts = Vec::new();
        self.skip_whitespace();
        while !self.is_eof() && self.peek() != Some('}') {
            if self.peek() == Some(';') {
                self.next_char();
                self.skip_whitespace();
                continue;
            }
            let mut stmt = self.parse_statement()?;
            self.skip_whitespace();
            while self.pos + 1 < self.input.len() {
                if self.input[self.pos] == '&' && self.input[self.pos + 1] == '&' {
                    self.pos += 2;
                    let rhs = self.parse_statement()?;
                    stmt = Stmt::And(Box::new(stmt), Box::new(rhs));
                    self.skip_whitespace();
                } else if self.input[self.pos] == '|' && self.input[self.pos + 1] == '|' {
                    self.pos += 2;
                    let rhs = self.parse_statement()?;
                    stmt = Stmt::Or(Box::new(stmt), Box::new(rhs));
                    self.skip_whitespace();
                } else {
                    break;
                }
            }
            stmts.push(stmt);
            if self.peek() == Some(';') {
                self.next_char();
                self.skip_whitespace();
            }
        }
        self.expect('}')?;
        Ok(stmts)
    }

    fn is_input_stage(stage: &PipelineStage) -> bool {
        matches!(
            stage,
            PipelineStage::Read { .. }
                | PipelineStage::Heredoc { .. }
                | PipelineStage::HereString { .. }
        )
    }

    pub fn parse_pipeline(&mut self) -> Result<Pipeline, ParseError> {
        let mut stages = Vec::new();
        let mut leading_inputs: Vec<PipelineStage> = Vec::new();
        let mut leading_writes = Vec::new();
        while let Some(redirect) = self.parse_redirect()? {
            if Self::is_input_stage(&redirect) {
                leading_inputs.push(redirect);
            } else {
                leading_writes.push(redirect);
            }
            self.skip_whitespace();
        }

        if !matches!(
            self.peek(),
            None | Some('|') | Some(';') | Some('}') | Some('\n')
        ) {
            let stage = self.parse_pipeline_stage()?;
            for r in leading_inputs.drain(..) {
                stages.push(r);
            }
            stages.push(stage);
            stages.extend(leading_writes);
            self.skip_whitespace();
            while let Some(r) = self.parse_redirect()? {
                if Self::is_input_stage(&r) {
                    let last_idx = stages.len() - 1;
                    stages.insert(last_idx, r);
                } else {
                    stages.push(r);
                }
                self.skip_whitespace();
            }
        } else {
            stages.extend(leading_inputs);
            stages.extend(leading_writes);
        }

        while self.peek() == Some('|') {
            if self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '|' {
                break;
            }
            self.next_char();
            let saved_subsequent = self.is_subsequent_stage;
            self.is_subsequent_stage = true;
            let mut seg_inputs: Vec<PipelineStage> = Vec::new();
            while let Some(redirect) = self.parse_redirect()? {
                if Self::is_input_stage(&redirect) {
                    seg_inputs.push(redirect);
                } else {
                    stages.push(redirect);
                }
                self.skip_whitespace();
            }
            let stage_res = self.parse_pipeline_stage();
            self.is_subsequent_stage = saved_subsequent;
            let stage = stage_res?;
            for r in seg_inputs {
                stages.push(r);
            }
            stages.push(stage);
            self.skip_whitespace();
            // Consume redirects after this stage (before next pipe or end)
            while let Some(r) = self.parse_redirect()? {
                if Self::is_input_stage(&r) {
                    let last_idx = stages.len() - 1;
                    stages.insert(last_idx, r);
                } else {
                    stages.push(r);
                }
                self.skip_whitespace();
            }
        }
        while let Some(write_stage) = self.parse_redirect()? {
            stages.push(write_stage);
            self.skip_whitespace();
        }
        // Fill deferred heredoc bodies in FIFO order. Bodies start on the line(s) after the command line.
        // `self.pos` is currently at the end of the command line (before its newline). Advance to body.
        // Each Heredoc's body is consumed sequentially from the remaining input.
        for stage in &mut stages {
            if let PipelineStage::Heredoc {
                delimiter,
                content,
                strip_tabs,
                quoted,
            } = stage
            {
                // Only fill placeholders (empty RawMultiLineString) — leave already-filled (should not happen for pipeline) alone.
                // Our placeholder is exactly `RawMultiLineString("")` with delimiter set.
                let is_placeholder = matches!(content, Expr::RawMultiLineString(s) if s.is_empty());
                if is_placeholder {
                    // Move to start of body: skip to next line if we are still on command line.
                    // The body starts after the command line's newline. If we are still on the command line (before \n), consume its newline first.
                    // `collect_heredoc_body` handles leading newline consumption itself, but ensure we are at \n boundary.
                    let filled = self.collect_heredoc_body(delimiter, *strip_tabs, *quoted)?;
                    *content = filled;
                }
            }
        }
        Ok(Pipeline { stages })
    }

    pub(crate) fn parse_redirect(&mut self) -> Result<Option<PipelineStage>, ParseError> {
        self.skip_whitespace();

        // Here-string: `<<< word` — feeds word as stdin (with trailing newline)
        if self.peek() == Some('<')
            && self.pos + 2 < self.input.len()
            && self.input[self.pos + 1] == '<'
            && self.input[self.pos + 2] == '<'
        {
            self.next_char();
            self.next_char();
            self.next_char();
            self.skip_whitespace();
            let saved_redirect = self.redirect_mode;
            self.redirect_mode = true;
            let saved_arg = self.cmd_arg_mode;
            self.cmd_arg_mode = false;
            let content = self.parse_expr_with_pipeline(false)?;
            self.redirect_mode = saved_redirect;
            self.cmd_arg_mode = saved_arg;
            return Ok(Some(PipelineStage::HereString { content }));
        }

        // Heredoc: `<<DELIM` / `<<-DELIM` / `<<'DELIM'` — content becomes stdin
        // Body is deferred until after all command-line redirections are parsed (so `cat <<A > file <<B` works).
        if self.peek() == Some('<')
            && self.pos + 1 < self.input.len()
            && self.input[self.pos + 1] == '<'
            && !(self.pos + 2 < self.input.len() && self.input[self.pos + 2] == '(')
        {
            // Ensure not `<<<` already handled above
            self.next_char(); // <
            self.next_char(); // <
            let (delimiter, strip_tabs, quoted) = self.parse_heredoc_header()?;
            // Placeholder content — will be filled by `parse_pipeline` after all redirections are collected.
            // Use a sentinel that `parse_pipeline` will replace.
            return Ok(Some(PipelineStage::Heredoc {
                delimiter: delimiter.clone(),
                content: Expr::RawMultiLineString(String::new()),
                strip_tabs,
                quoted,
            }));
        }

        // Check for explicit stdin file redirect: `0<`
        if self.peek() == Some('0')
            && self.pos + 1 < self.input.len()
            && self.input[self.pos + 1] == '<'
            && !(self.pos + 2 < self.input.len()
                && (self.input[self.pos + 2] == '<' || self.input[self.pos + 2] == '('))
        {
            self.next_char(); // consume '0'
            self.next_char(); // consume '<'
            self.skip_whitespace();
            let saved_redirect = self.redirect_mode;
            self.redirect_mode = true;
            let path = self.parse_expr_with_pipeline(false)?;
            self.redirect_mode = saved_redirect;
            return Ok(Some(PipelineStage::Read { path }));
        }

        // Check for standard stdin file redirect: `<` (not `<<` heredoc, `<<<` here-string, or `<(` process substitution)
        if self.peek() == Some('<')
            && !(self.pos + 1 < self.input.len()
                && (self.input[self.pos + 1] == '<' || self.input[self.pos + 1] == '('))
        {
            self.next_char(); // consume '<'
            self.skip_whitespace();
            let saved_redirect = self.redirect_mode;
            self.redirect_mode = true;
            let path = self.parse_expr_with_pipeline(false)?;
            self.redirect_mode = saved_redirect;
            return Ok(Some(PipelineStage::Read { path }));
        }

        // Check for fd-to-fd redirect: `N>&M` where N and M are fd numbers (0..=9).
        // Must check this BEFORE `N>` file redirects.
        if let Some(c) = self.peek()
            && c.is_ascii_digit()
            && self.pos + 1 < self.input.len()
            && self.input[self.pos + 1] == '>'
            && self.pos + 2 < self.input.len()
            && self.input[self.pos + 2] == '&'
            && self.pos + 3 < self.input.len()
        {
            let src_fd = (c as u8 - b'0') as i32;
            self.next_char(); // consume fd digit
            self.next_char(); // consume '>'
            self.next_char(); // consume '&'
            // Parse the destination fd (any digit 0..=9, or '-' to close)
            match self.peek() {
                Some(c) if c.is_ascii_digit() => {
                    let dst_fd = (c as u8 - b'0') as i32;
                    self.next_char();
                    return Ok(Some(PipelineStage::FdRedirect { src_fd, dst_fd }));
                }
                Some('-') => {
                    self.next_char(); // consume '-'
                    return Ok(Some(PipelineStage::FdRedirect {
                        src_fd,
                        dst_fd: -1, // -1 means close fd
                    }));
                }
                other => {
                    return Err(ParseError::SyntaxError {
                        message: format!(
                            "Expected file descriptor number (0..=9) or '-' after '{}>&', got {:?}",
                            src_fd, other
                        ),
                        span: self.current_span(),
                    });
                }
            }
        }

        let mut redirect_stdout = false;
        let mut redirect_stderr = false;
        let mut has_redirect = false;

        // Check for `&>`
        if self.peek() == Some('&')
            && self.pos + 1 < self.input.len()
            && self.input[self.pos + 1] == '>'
        {
            self.next_char(); // consume '&'
            self.next_char(); // consume '>'
            redirect_stdout = true;
            redirect_stderr = true;
            has_redirect = true;
        }
        // Check for `2>`
        else if self.peek() == Some('2')
            && self.pos + 1 < self.input.len()
            && self.input[self.pos + 1] == '>'
        {
            self.next_char(); // consume '2'
            self.next_char(); // consume '>'
            redirect_stdout = false;
            redirect_stderr = true;
            has_redirect = true;
        }
        // Check for `1>` (explicit stdout redirect to file)
        else if self.peek() == Some('1')
            && self.pos + 1 < self.input.len()
            && self.input[self.pos + 1] == '>'
        {
            self.next_char(); // consume '1'
            self.next_char(); // consume '>'
            redirect_stdout = true;
            redirect_stderr = false;
            has_redirect = true;
        }
        // Check for `>&`
        else if self.peek() == Some('>')
            && self.pos + 1 < self.input.len()
            && self.input[self.pos + 1] == '&'
        {
            self.next_char(); // consume '>'
            self.next_char(); // consume '&'
            redirect_stdout = true;
            redirect_stderr = true;
            has_redirect = true;
        }
        // Check for `>`
        else if self.peek() == Some('>') {
            self.next_char(); // consume '>'
            redirect_stdout = true;
            redirect_stderr = false;
            has_redirect = true;
        }

        if has_redirect {
            let append = if self.peek() == Some('>') {
                self.next_char();
                true
            } else {
                false
            };
            self.skip_whitespace();
            let saved_redirect = self.redirect_mode;
            self.redirect_mode = true;
            let path = self.parse_expr_with_pipeline(false)?;
            self.redirect_mode = saved_redirect;
            Ok(Some(PipelineStage::Write {
                path,
                append,
                redirect_stdout,
                redirect_stderr,
            }))
        } else {
            Ok(None)
        }
    }

    /// Parse a command name, allowing dots for extensions like .sh
    pub(crate) fn parse_command_name(&mut self) -> Result<String, ParseError> {
        if matches!(self.peek(), Some('/') | Some('~') | Some('.')) {
            let path_expr = self.parse_bare_path_or_string()?;
            match path_expr {
                Expr::String(parts) => {
                    let mut s = String::new();
                    for part in parts {
                        match part {
                            StringPart::Lit(lit) => s.push_str(&lit),
                            StringPart::Expr(_) => {
                                return Err(ParseError::SyntaxError {
                                    message: "Unexpected expression in command path".to_string(),
                                    span: self.current_span(),
                                });
                            }
                        }
                    }
                    Ok(s)
                }
                _ => Err(ParseError::SyntaxError {
                    message: "Expected path string for command name".to_string(),
                    span: self.current_span(),
                }),
            }
        } else if self.peek() == Some('[') {
            if self.pos + 1 < self.input.len() && self.input[self.pos + 1].is_whitespace() {
                self.next_char(); // consume '['
                Ok("[".to_string())
            } else {
                Err(ParseError::SyntaxError {
                    message: "Expected command name".to_string(),
                    span: self.current_span(),
                })
            }
        } else {
            let mut name = String::new();
            if self.peek() == Some('$') {
                name.push(self.next_char().ok_or_else(|| ParseError::UnexpectedEof {
                    span: self.current_span(),
                })?);
            }
            name.push_str(&self.parse_identifier()?);
            // Allow dots in command names for extensions like .sh
            while self.peek() == Some('.') {
                name.push(self.next_char().ok_or_else(|| ParseError::UnexpectedEof {
                    span: self.current_span(),
                })?);
                while let Some(c) = self.peek() {
                    if c.is_alphanumeric() || c == '_' || c == '-' {
                        name.push(self.next_char().ok_or_else(|| ParseError::UnexpectedEof {
                            span: self.current_span(),
                        })?);
                    } else {
                        break;
                    }
                }
            }
            Ok(name)
        }
    }

    pub(crate) fn parse_hash_stage(&mut self) -> Result<PipelineStage, ParseError> {
        let mut mode = HashMode::Hash256;
        let mut per_record = false;
        let mut xof_len = 32;

        self.skip_horizontal_whitespace();
        while let Some(c) = self.peek() {
            if c == '|' || c == '\n' || c == ';' || c == ')' || c == '}' || c == ']' {
                break;
            }
            let arg = self.parse_identifier()?;
            match arg.as_str() {
                "per-record" | "--per-record" => {
                    per_record = true;
                }
                "-a" | "a" => {
                    self.skip_horizontal_whitespace();
                    let algo = self.parse_identifier()?;
                    match algo.as_str() {
                        "256" => mode = HashMode::Hash256,
                        "512" => mode = HashMode::Hash512,
                        "xof" => mode = HashMode::Xof(xof_len),
                        other => {
                            return Err(ParseError::SyntaxError {
                                message: format!("unknown hash algorithm: {}", other),
                                span: self.current_span(),
                            });
                        }
                    }
                }
                "-o" | "o" => {
                    self.skip_horizontal_whitespace();
                    let len_str = self.parse_identifier()?;
                    if let Ok(len) = len_str.parse::<usize>() {
                        xof_len = len;
                        if let HashMode::Xof(_) = mode {
                            mode = HashMode::Xof(xof_len);
                        }
                    } else {
                        return Err(ParseError::SyntaxError {
                            message: format!("invalid XOF output length: {}", len_str),
                            span: self.current_span(),
                        });
                    }
                }
                other => {
                    return Err(ParseError::SyntaxError {
                        message: format!("unexpected argument for hash stage: {}", other),
                        span: self.current_span(),
                    });
                }
            }
            self.skip_horizontal_whitespace();
        }

        if let HashMode::Xof(_) = mode {
            mode = HashMode::Xof(xof_len);
        }

        Ok(PipelineStage::Hash { mode, per_record })
    }

    pub(crate) fn parse_pipeline_stage(&mut self) -> Result<PipelineStage, ParseError> {
        self.skip_whitespace();
        let start = self.pos;
        let mut inline_env: Vec<(String, Expr)> = Vec::new();
        loop {
            let saved_pos = self.pos;
            if let Some(ident) = self.peek_identifier() {
                let ident_len = ident.len();
                let mut peek_pos = self.pos + ident_len;
                while peek_pos < self.input.len() && self.input[peek_pos].is_whitespace() {
                    peek_pos += 1;
                }
                if peek_pos < self.input.len() && self.input[peek_pos] == '=' {
                    if peek_pos + 1 < self.input.len() && self.input[peek_pos + 1] == '=' {
                        self.pos = saved_pos;
                        break;
                    }
                    for _ in 0..ident_len {
                        self.next_char();
                    }
                    self.skip_horizontal_whitespace();
                    self.expect('=')?;
                    let value = match self.peek() {
                        Some('"') => self.parse_string_literal()?,
                        Some('$') => {
                            self.next_char();
                            Expr::Variable(self.parse_identifier()?)
                        }
                        _ => {
                            let mut val = String::new();
                            while let Some(c) = self.peek() {
                                if c.is_whitespace()
                                    || c == '|'
                                    || c == '\n'
                                    || c == '\r'
                                    || c == ';'
                                    || c == '}'
                                {
                                    break;
                                }
                                val.push(self.next_char().ok_or_else(|| {
                                    ParseError::UnexpectedEof {
                                        span: self.current_span(),
                                    }
                                })?);
                            }
                            if val.is_empty() {
                                return Err(ParseError::SyntaxError {
                                    message: "Expected value after '='".to_string(),
                                    span: self.current_span(),
                                });
                            }
                            Expr::String(vec![StringPart::Lit(val)])
                        }
                    };
                    inline_env.push((ident, value));
                    self.skip_horizontal_whitespace();
                    continue;
                } else {
                    self.pos = saved_pos;
                    break;
                }
            } else {
                break;
            }
        }
        if self.peek() == Some('@') {
            self.next_char();
            let format_name = self.parse_identifier()?;
            let format = match format_name.as_str() {
                "json" => SerializationFormat::Json,
                "yaml" => SerializationFormat::Yaml,
                "msgpack" => SerializationFormat::MsgPack,
                "text" => SerializationFormat::Text,
                "csv" => SerializationFormat::Csv,
                "table" => SerializationFormat::Table,
                "bar" => SerializationFormat::Bar,
                _ => {
                    return Err(ParseError::SyntaxError {
                        message: format!("Unknown serialization format: {}", format_name),
                        span: self.span_from(start),
                    });
                }
            };
            return Ok(PipelineStage::BoundaryOperator { format });
        }
        let name = self.parse_command_name()?;
        self.skip_horizontal_whitespace();
        let has_flag = {
            let mut p = self.pos;
            while p < self.input.len() && (self.input[p] == ' ' || self.input[p] == '\t') {
                p += 1;
            }
            p < self.input.len() && self.input[p] == '-'
        };
        self.parse_stage_inner(name, has_flag, inline_env)
    }

    pub(crate) fn parse_pipeline_stage_with_env(
        &mut self,
        inline_env: Vec<(String, Expr)>,
    ) -> Result<PipelineStage, ParseError> {
        self.skip_whitespace();
        let start = self.pos;
        let name = self.parse_command_name()?;
        self.skip_horizontal_whitespace();
        let has_flag = {
            let mut p = self.pos;
            while p < self.input.len() && (self.input[p] == ' ' || self.input[p] == '\t') {
                p += 1;
            }
            p < self.input.len() && self.input[p] == '-'
        };
        let mut stage = self.parse_stage_inner(name, has_flag, inline_env)?;
        if let PipelineStage::CommandCall { span, .. } = &mut stage {
            *span = SourceSpan::new(start.into(), self.pos - start);
        }
        Ok(stage)
    }

    fn parse_stage_inner(
        &mut self,
        name: String,
        has_flag: bool,
        inline_env: Vec<(String, Expr)>,
    ) -> Result<PipelineStage, ParseError> {
        match name.as_str() {
            "filter" => {
                let condition = self.parse_expr_with_pipeline(false)?;
                Ok(PipelineStage::Filter { condition })
            }
            "map" => {
                let mut projections = Vec::new();
                loop {
                    self.skip_whitespace();
                    if matches!(
                        self.peek(),
                        Some('|') | Some('\n') | Some('\r') | Some(';') | None
                    ) {
                        break;
                    }
                    projections.push(Expr::Ident(self.parse_identifier()?));
                    self.skip_horizontal_whitespace();
                    if self.peek() == Some(',') {
                        self.next_char();
                    }
                }
                Ok(PipelineStage::Map { projections })
            }
            "sort" if !has_flag && self.peek_identifier().is_some() => {
                self.skip_whitespace();
                let mut descending = false;
                let mut col = self.parse_identifier()?;
                if col.starts_with('-') {
                    descending = true;
                    col = col.trim_start_matches('-').to_string();
                }
                if let Some(dir) = self.peek_identifier()
                    && (dir == "asc" || dir == "desc")
                {
                    self.parse_identifier()?;
                    if dir == "desc" {
                        descending = true;
                    }
                }
                Ok(PipelineStage::Sort {
                    column: col,
                    descending,
                })
            }
            "grep" if !has_flag => {
                let saved_redirect = self.redirect_mode;
                let saved_arg = self.cmd_arg_mode;
                self.redirect_mode = true;
                self.cmd_arg_mode = true;
                let mut pattern = self.parse_expr_with_pipeline(false)?;
                self.redirect_mode = saved_redirect;
                self.cmd_arg_mode = saved_arg;
                if let Expr::Ident(id) = pattern {
                    pattern = Expr::String(vec![StringPart::Lit(id)]);
                }
                Ok(PipelineStage::Grep { pattern })
            }
            "mark" if !has_flag => {
                let saved_redirect = self.redirect_mode;
                let saved_arg = self.cmd_arg_mode;
                self.redirect_mode = true;
                self.cmd_arg_mode = true;
                let mut pattern = self.parse_expr_with_pipeline(false)?;
                self.redirect_mode = saved_redirect;
                self.cmd_arg_mode = saved_arg;
                if let Expr::Ident(id) = pattern {
                    pattern = Expr::String(vec![StringPart::Lit(id)]);
                }
                Ok(PipelineStage::Mark { pattern })
            }
            "count" => Ok(PipelineStage::Count),
            "hash" if self.is_subsequent_stage => self.parse_hash_stage(),
            "limit" if !has_flag => {
                let saved_redirect = self.redirect_mode;
                let saved_arg = self.cmd_arg_mode;
                self.redirect_mode = true;
                self.cmd_arg_mode = true;
                let amount = self.parse_expr_with_pipeline(false)?;
                self.redirect_mode = saved_redirect;
                self.cmd_arg_mode = saved_arg;
                Ok(PipelineStage::Limit { amount })
            }
            "traverse" if !has_flag => {
                let saved_redirect = self.redirect_mode;
                let saved_arg = self.cmd_arg_mode;
                self.redirect_mode = true;
                self.cmd_arg_mode = true;
                let edge_label = self.parse_expr_with_pipeline(false)?;
                self.redirect_mode = saved_redirect;
                self.cmd_arg_mode = saved_arg;
                Ok(PipelineStage::Traverse { edge_label })
            }
            _ => {
                let mut args = Vec::new();
                self.skip_horizontal_whitespace();
                while let Some(c) = self.peek() {
                    if c == '|'
                        || c == '\n'
                        || c == '\r'
                        || c == ';'
                        || c == '}'
                        || c == ')'
                        || c == '&'
                        || c == '#'
                    {
                        break;
                    }
                    if c == '>'
                        && !(self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '(')
                    {
                        break;
                    }
                    if c == '<'
                        && !(self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '(')
                    {
                        break;
                    }
                    if c.is_ascii_digit()
                        && self.pos + 1 < self.input.len()
                        && ((self.input[self.pos + 1] == '>'
                            && !(self.pos + 2 < self.input.len()
                                && self.input[self.pos + 2] == '('))
                            || (self.input[self.pos + 1] == '<'
                                && !(self.pos + 2 < self.input.len()
                                    && self.input[self.pos + 2] == '(')))
                    {
                        break;
                    }
                    let saved_redirect = self.redirect_mode;
                    let saved_arg = self.cmd_arg_mode;
                    self.redirect_mode = true;
                    self.cmd_arg_mode = true;
                    let mut arg = self.parse_expr_with_pipeline(false)?;
                    self.redirect_mode = saved_redirect;
                    self.cmd_arg_mode = saved_arg;
                    if (name == "export" || name == "unset")
                        && let Expr::Ident(id) = arg
                    {
                        arg = Expr::String(vec![StringPart::Lit(id)]);
                    }
                    if let Expr::String(parts) = &arg
                        && parts.len() == 1
                        && matches!(&parts[0], StringPart::Lit(s) if s.contains('{'))
                    {
                        if let StringPart::Lit(s) = &parts[0] {
                            let expanded = expand_braces(s);
                            if expanded.len() > 1 {
                                for word in expanded {
                                    args.push(Expr::String(vec![StringPart::Lit(word)]));
                                }
                            } else {
                                args.push(arg);
                            }
                        } else {
                            args.push(arg);
                        }
                    } else {
                        args.push(arg);
                    }
                    self.skip_horizontal_whitespace();
                }
                Ok(PipelineStage::CommandCall {
                    name,
                    args,
                    env: inline_env,
                    span: SourceSpan::new(0.into(), 0),
                })
            }
        }
    }
}
