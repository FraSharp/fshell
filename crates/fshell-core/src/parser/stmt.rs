// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::ast::*;
use crate::expand_braces;
use crate::{ParseError, Parser};
use miette::SourceSpan;

use super::PendingHeredoc;

/// A POSIX reserved word where a command name belongs is POSIX syntax, never
/// a command: `case x in …`, `until …; do`, `… | while read l; do`. Native's
/// own `if`/`for`/`while` statements are parsed before a pipeline stage is, so
/// meeting one here means the input is not native. Rejecting it makes the
/// input a parse error, which is what hands it to the POSIX frontend — for
/// execution and for the editor's continuation prompt alike — instead of
/// running a nonexistent `case` command.
fn reject_posix_reserved_word(name: &str, span: SourceSpan) -> Result<(), ParseError> {
    if matches!(
        name,
        "case"
            | "esac"
            | "if"
            | "then"
            | "elif"
            | "else"
            | "fi"
            | "for"
            | "while"
            | "until"
            | "do"
            | "done"
            | "in"
            | "function"
    ) {
        return Err(ParseError::SyntaxError {
            message: format!("`{name}` is a POSIX reserved word, not a native command"),
            span,
        });
    }
    Ok(())
}

/// Whether a `#` at this position begins a POSIX comment.
///
/// POSIX starts a comment only when `#` begins a word, so `foo#bar` stays a
/// word while `echo # note`, `;# note` and a `#` at the start of a line do not.
fn posix_hash_starts_comment(previous: Option<char>) -> bool {
    match previous {
        None => true,
        Some(c) => {
            c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '<' | '>' | '{' | '}')
        }
    }
}

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
            // A pending here-document makes the newline the end of the command
            // line: read the bodies there, then parse the next line.
            if self.at_command_line_end() {
                self.finish_command_line()?;
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
        // A header at end of input has no newline to end its line; reading the
        // body reports the unterminated here-document.
        self.finish_command_line()?;
        self.assign_heredoc_bodies(&mut stmts);
        Ok(stmts)
    }
}

impl Parser {
    /// Whether the cursor sits at the newline that ends a command line whose
    /// here-document bodies have not been read yet.
    fn at_command_line_end(&self) -> bool {
        !self.pending_heredocs.is_empty() && self.peek().is_none_or(|c| c == '\n' || c == '\r')
    }

    pub(crate) fn consume_line_ending(&mut self) {
        if self.peek() == Some('\r') {
            self.next_char();
        }
        if self.peek() == Some('\n') {
            self.next_char();
        }
    }

    /// Read the bodies of the here-document headers queued on the command line
    /// that just ended. Every body on a line follows its terminating newline, in
    /// header order, so a line may carry several different here-documents.
    fn finish_command_line(&mut self) -> Result<(), ParseError> {
        if self.pending_heredocs.is_empty() {
            return Ok(());
        }
        self.consume_line_ending();
        for header in std::mem::take(&mut self.pending_heredocs) {
            let body =
                self.collect_heredoc_body(&header.delimiter, header.strip_tabs, header.quoted)?;
            self.heredoc_bodies.push(body);
        }
        Ok(())
    }

    /// Attach the bodies read during the parse to their stages. Bodies are
    /// queued in the order their headers appeared, and the walk visits the
    /// stages in source order, so the two sequences pair element by element.
    fn assign_heredoc_bodies(&mut self, stmts: &mut [Stmt]) {
        if self.heredoc_bodies.is_empty() {
            return;
        }
        let mut bodies = std::mem::take(&mut self.heredoc_bodies).into_iter();
        for stmt in stmts {
            assign_stmt_heredocs(stmt, &mut bodies);
        }
        debug_assert!(
            bodies.next().is_none(),
            "here-document body left without a stage"
        );
    }
}

/// Walk a statement in source order, giving each here-document stage the next
/// collected body.
fn assign_stmt_heredocs(stmt: &mut Stmt, bodies: &mut impl Iterator<Item = Expr>) {
    match stmt {
        Stmt::Spanned { stmt, .. } => assign_stmt_heredocs(stmt, bodies),
        Stmt::And(lhs, rhs) | Stmt::Or(lhs, rhs) => {
            assign_stmt_heredocs(lhs, bodies);
            assign_stmt_heredocs(rhs, bodies);
        }
        Stmt::Background(inner) => assign_stmt_heredocs(inner, bodies),
        Stmt::Expr(expr) => assign_expr_heredocs(expr, bodies),
        Stmt::ReactiveCell { pipeline, .. } => assign_pipeline_heredocs(pipeline, bodies),
        Stmt::Local { expr, .. } => {
            if let Some(expr) = expr {
                assign_expr_heredocs(expr, bodies);
            }
        }
        Stmt::Let { expr, .. }
        | Stmt::Assign { expr, .. }
        | Stmt::Update { expr, .. }
        | Stmt::Source { path: expr, .. }
        | Stmt::Return(expr) => assign_expr_heredocs(expr, bodies),
        Stmt::Exit(expr) => {
            if let Some(expr) = expr {
                assign_expr_heredocs(expr, bodies);
            }
        }
        Stmt::FnDef { body, .. }
        | Stmt::ReactiveCellEvery { body, .. }
        | Stmt::Unsafe { body }
        | Stmt::Every { body, .. } => {
            for stmt in body {
                assign_stmt_heredocs(stmt, bodies);
            }
        }
        Stmt::TryCatch {
            try_body,
            catch_body,
            ..
        } => {
            for stmt in try_body.iter_mut().chain(catch_body.iter_mut()) {
                assign_stmt_heredocs(stmt, bodies);
            }
        }
        Stmt::WithCaps { caps, body } => {
            for expr in caps {
                assign_expr_heredocs(expr, bodies);
            }
            for stmt in body {
                assign_stmt_heredocs(stmt, bodies);
            }
        }
        Stmt::While { condition, body } => {
            assign_expr_heredocs(condition, bodies);
            for stmt in body {
                assign_stmt_heredocs(stmt, bodies);
            }
        }
        Stmt::For { iter, body, .. } => {
            assign_expr_heredocs(iter, bodies);
            for stmt in body {
                assign_stmt_heredocs(stmt, bodies);
            }
        }
        Stmt::Match { expr, arms } => {
            assign_expr_heredocs(expr, bodies);
            for arm in arms {
                for stmt in &mut arm.body {
                    assign_stmt_heredocs(stmt, bodies);
                }
            }
        }
        Stmt::On { handler, .. } => {
            if let OnHandler::Block(body) = handler {
                for stmt in body {
                    assign_stmt_heredocs(stmt, bodies);
                }
            }
        }
        Stmt::Comment(_) | Stmt::PosixBlock { .. } | Stmt::Break | Stmt::Continue => {}
    }
}

fn assign_expr_heredocs(expr: &mut Expr, bodies: &mut impl Iterator<Item = Expr>) {
    match expr {
        Expr::Pipeline(pipeline) | Expr::InlinePipeline(pipeline) => {
            assign_pipeline_heredocs(pipeline, bodies);
        }
        Expr::ProcessSubst { pipeline, .. } => assign_pipeline_heredocs(pipeline, bodies),
        Expr::Substitution(stmts) => {
            for stmt in stmts {
                assign_stmt_heredocs(stmt, bodies);
            }
        }
        Expr::Spanned { expr, .. } | Expr::Not(expr) | Expr::ArithmeticExpansion(expr) => {
            assign_expr_heredocs(expr, bodies);
        }
        Expr::MemberAccess { expr, .. } => assign_expr_heredocs(expr, bodies),
        Expr::BinaryOp { lhs, rhs, .. } => {
            assign_expr_heredocs(lhs, bodies);
            assign_expr_heredocs(rhs, bodies);
        }
        Expr::List(items) => {
            for item in items {
                assign_expr_heredocs(item, bodies);
            }
        }
        Expr::Map(entries) => {
            for (_, value) in entries {
                assign_expr_heredocs(value, bodies);
            }
        }
        Expr::String(parts) | Expr::MultiLineString { parts, .. } => {
            for part in parts {
                if let StringPart::Expr { expr, .. } = part {
                    assign_expr_heredocs(expr, bodies);
                }
            }
        }
        Expr::VarWithModifier { modifier, .. } => assign_modifier_heredocs(modifier, bodies),
        Expr::If {
            condition,
            then_body,
            else_body,
        } => {
            assign_expr_heredocs(condition, bodies);
            for stmt in then_body {
                assign_stmt_heredocs(stmt, bodies);
            }
            if let Some(else_body) = else_body {
                for stmt in else_body {
                    assign_stmt_heredocs(stmt, bodies);
                }
            }
        }
        Expr::Null
        | Expr::Bool(_)
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::Ident(_)
        | Expr::Variable(_)
        | Expr::AnsiCQuote(_)
        | Expr::RawMultiLineString(_) => {}
    }
}

fn assign_modifier_heredocs(modifier: &mut ParamModifier, bodies: &mut impl Iterator<Item = Expr>) {
    match modifier {
        ParamModifier::Default(expr)
        | ParamModifier::AssignDefault(expr)
        | ParamModifier::ErrorIfUnset(expr)
        | ParamModifier::Alternate(expr)
        | ParamModifier::ShortestPrefix(expr)
        | ParamModifier::LongestPrefix(expr)
        | ParamModifier::ShortestSuffix(expr)
        | ParamModifier::LongestSuffix(expr) => assign_expr_heredocs(expr, bodies),
        ParamModifier::Replace {
            pattern,
            replacement,
            ..
        } => {
            assign_expr_heredocs(pattern, bodies);
            assign_expr_heredocs(replacement, bodies);
        }
        ParamModifier::Tail
        | ParamModifier::Head
        | ParamModifier::Root
        | ParamModifier::Ext
        | ParamModifier::Substring { .. }
        | ParamModifier::StringLength
        | ParamModifier::Upper
        | ParamModifier::Lower => {}
    }
}

fn assign_pipeline_heredocs(pipeline: &mut Pipeline, bodies: &mut impl Iterator<Item = Expr>) {
    for stage in &mut pipeline.stages {
        match stage {
            PipelineStage::Heredoc { content, .. } => {
                if let Some(body) = bodies.next() {
                    *content = body;
                }
            }
            PipelineStage::CommandCall { args, env, .. } => {
                for (_, value) in env {
                    assign_expr_heredocs(value, bodies);
                }
                for arg in args {
                    assign_expr_heredocs(arg, bodies);
                }
            }
            PipelineStage::Filter { condition } => assign_expr_heredocs(condition, bodies),
            PipelineStage::Map { projections } => {
                for projection in projections {
                    assign_expr_heredocs(projection, bodies);
                }
            }
            PipelineStage::Mark { pattern } => assign_expr_heredocs(pattern, bodies),
            PipelineStage::Limit { amount } => assign_expr_heredocs(amount, bodies),
            PipelineStage::Traverse { edge_label } => assign_expr_heredocs(edge_label, bodies),
            PipelineStage::Write { path, .. } | PipelineStage::Read { path } => {
                assign_expr_heredocs(path, bodies);
            }
            PipelineStage::HereString { content } => assign_expr_heredocs(content, bodies),
            PipelineStage::Sort { .. }
            | PipelineStage::Count
            | PipelineStage::BoundaryOperator { .. }
            | PipelineStage::Hash { .. }
            | PipelineStage::FdRedirect { .. } => {}
        }
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
        let expr = self.parse_expr_bool()?;
        Ok(Stmt::Update { name, op, expr })
    }

    pub fn parse_statement(&mut self) -> Result<Stmt, ParseError> {
        let span = self.current_span();
        let _guard = crate::parser::RecursionGuard::new(&self.recursion_depth, span)?;
        stacker::maybe_grow(32 * 1024, 1024 * 1024, || self.parse_statement_inner())
    }

    fn parse_statement_inner(&mut self) -> Result<Stmt, ParseError> {
        self.skip_whitespace();
        self.note_native_construct();
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

    /// Record a native-only keyword at the start of the statement being parsed.
    ///
    /// The router must not hand input that began in fsh to the POSIX engine
    /// merely because POSIX can read the same words as command names. `sh`
    /// counts only when it opens a native block (`sh { ... }`), never for the
    /// ordinary `sh script.sh` command.
    fn note_native_construct(&mut self) {
        let mut p = self.pos;
        while p < self.input.len() && self.input[p].is_whitespace() {
            p += 1;
        }
        let start = p;
        while p < self.input.len() && (self.input[p].is_alphanumeric() || self.input[p] == '_') {
            p += 1;
        }
        let word: String = self.input[start..p].iter().collect();
        let native = match word.as_str() {
            "let" | "local" | "fn" | "match" | "try" | "catch" | "with" | "caps" | "unsafe" => true,
            "sh" => {
                let mut q = p;
                while q < self.input.len() && self.input[q].is_whitespace() {
                    q += 1;
                }
                q < self.input.len() && self.input[q] == '{'
            }
            _ => false,
        };
        if native {
            self.saw_native_construct = true;
        }
    }

    pub(crate) fn parse_statement_body(&mut self) -> Result<Stmt, ParseError> {
        let span = self.current_span();
        let _guard = crate::parser::RecursionGuard::new(&self.recursion_depth, span)?;
        stacker::maybe_grow(32 * 1024, 1024 * 1024, || self.parse_statement_body_inner())
    }

    fn parse_statement_body_inner(&mut self) -> Result<Stmt, ParseError> {
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
                        // `NAME=value cmd` (a prefix assignment) or a plain
                        // assignment? Decide by parsing the value as the word
                        // it is and looking at what follows on the same line.
                        // A prefix has `=` right after the name; native's
                        // spaced `x = value` is always an assignment.
                        let adjacent = !self.input[peek_pos - 1].is_whitespace();
                        let has_command_after = adjacent && {
                            let saved = self.pos;
                            let found = self.prefix_assignment_precedes_command();
                            self.pos = saved;
                            found
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
                                let value = self.parse_assignment_value()?;
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
                            let mut boundaries = Vec::new();
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
                                boundaries.push(stages.len());
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
                            return Ok(Stmt::Expr(Expr::Pipeline(Pipeline { stages, boundaries })));
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
            let ty = self.parse_type_annotation()?;
            self.skip_horizontal_whitespace();
            let expr = if self.peek() == Some('=') {
                self.next_char();
                self.skip_horizontal_whitespace();
                Some(self.parse_expr_bool()?)
            } else {
                None
            };
            Ok(Stmt::Local { name, ty, expr })
        } else if self.match_keyword("let") {
            let name = self.parse_identifier()?;
            let ty = self.parse_type_annotation()?;
            self.expect('=')?;
            let expr = self.parse_expr_bool()?;
            Ok(Stmt::Let { name, ty, expr })
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
            let expr = self.parse_expr_with_pipeline_bool(false)?;
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
        } else if let Some(keyword) = self.removed_posix_block_keyword() {
            Err(ParseError::SyntaxError {
                message: format!(
                    "`{keyword} {{ ... }}` is no longer a block keyword; use `sh {{ ... }}`"
                ),
                span: self.current_span(),
            })
        } else if self.peek_posix_block() {
            // Consume the keyword and opening brace, then capture body
            self.match_keyword("sh");
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
            let condition = self.parse_expr_with_pipeline_bool(false)?;
            self.cmd_arg_mode = saved_arg;
            self.redirect_mode = saved_redirect;
            self.skip_whitespace();
            if self.at_posix_continuation("do") {
                return Err(ParseError::SyntaxError {
                    message: "POSIX `while ...; do ... done` is not native syntax; use `while cond { ... }` or wrap the loop in `sh { ... }`".to_string(),
                    span: self.current_span(),
                });
            }
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
            let iter_start = self.pos;
            let iter = match self.parse_expr_with_pipeline_bool(false) {
                Ok(iter) => iter,
                Err(err) => {
                    if self.line_has_posix_do(iter_start) {
                        return Err(ParseError::SyntaxError {
                            message: "POSIX `for ...; do ... done` is not native syntax; use `for x in values { ... }` or wrap the loop in `sh { ... }`".to_string(),
                            span: err.span(),
                        });
                    }
                    return Err(err);
                }
            };
            self.skip_whitespace();
            if self.at_posix_continuation("do") || self.line_has_posix_do(iter_start) {
                return Err(ParseError::SyntaxError {
                    message: "POSIX `for ...; do ... done` is not native syntax; use `for x in values { ... }` or wrap the loop in `sh { ... }`".to_string(),
                    span: self.current_span(),
                });
            }
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
                self.parse_expr_bool()?
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
                Some(self.parse_expr_bool()?)
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

    /// Peek if the next tokens are the POSIX block keyword `sh` followed by `{`.
    /// Pure lookahead — does not consume input.
    pub(crate) fn peek_posix_block(&self) -> bool {
        let mut p = self.pos;
        while p < self.input.len() && self.input[p].is_whitespace() {
            p += 1;
        }
        if p + 2 > self.input.len() || self.input[p] != 's' || self.input[p + 1] != 'h' {
            return false;
        }
        let end = p + 2;
        if end < self.input.len() && (self.input[end].is_alphanumeric() || self.input[end] == '_') {
            return false;
        }
        let mut q = end;
        while q < self.input.len() && self.input[q].is_whitespace() {
            q += 1;
        }
        q < self.input.len() && self.input[q] == '{'
    }

    /// If the removed block keywords (`posix`, `bash`) are followed by `{`,
    /// returns which one, so the parser can point users at `sh { ... }`.
    pub(crate) fn removed_posix_block_keyword(&self) -> Option<&'static str> {
        let mut p = self.pos;
        while p < self.input.len() && self.input[p].is_whitespace() {
            p += 1;
        }
        for &kw in &["posix", "bash"] {
            let end = p + kw.len();
            if end > self.input.len()
                || !self.input[p..end].iter().copied().eq(kw.chars())
                || (end < self.input.len()
                    && (self.input[end].is_alphanumeric() || self.input[end] == '_'))
            {
                continue;
            }
            let mut q = end;
            while q < self.input.len() && self.input[q].is_whitespace() {
                q += 1;
            }
            if q < self.input.len() && self.input[q] == '{' {
                return Some(kw);
            }
        }
        None
    }

    /// Parse raw POSIX block body: balanced-brace capture until matching `}`.
    /// Does not parse as fsh — the content is handed to the POSIX engine.
    ///
    /// The capture understands enough POSIX lexical structure to find the right
    /// closing brace: comments, quotes, backticks, `${...}`, `$((...))`, and
    /// here-documents. Here-document bodies that live inside the block are
    /// consumed as body text, so a `}` inside one cannot close the block. A
    /// header whose body follows the closing brace is supported when the brace
    /// ends its line; the bodies are appended to the POSIX source, exactly as
    /// a POSIX shell reads them after the command line.
    pub(crate) fn parse_posix_block_body(&mut self) -> Result<String, ParseError> {
        let start = self.pos;
        let mut depth = 1usize;
        let mut in_single = false;
        let mut in_double = false;
        let mut in_backtick = false;
        let mut in_comment = false;
        let mut escaped = false;
        let mut arithmetic_depth = 0usize;
        let mut parameter_depth = 0usize;
        let mut pending: Vec<PendingHeredoc> = Vec::new();
        let mut previous: Option<char> = None;

        while self.pos < self.input.len() && depth > 0 {
            let c = self.input[self.pos];

            if escaped {
                // An escaped character is word content: `\ #` does not start a
                // comment even though a space was escaped.
                escaped = false;
                previous = Some('\\');
                self.pos += 1;
                continue;
            }
            if in_single {
                if c == '\'' {
                    in_single = false;
                }
                previous = Some(c);
                self.pos += 1;
                continue;
            }
            if in_double {
                if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_double = false;
                }
                previous = Some(c);
                self.pos += 1;
                continue;
            }
            if in_backtick {
                if c == '\\' {
                    escaped = true;
                } else if c == '`' {
                    in_backtick = false;
                    previous = Some('0');
                } else {
                    previous = Some(c);
                }
                self.pos += 1;
                continue;
            }
            if arithmetic_depth > 0 {
                match c {
                    '(' => arithmetic_depth += 1,
                    ')' => {
                        arithmetic_depth -= 1;
                        if arithmetic_depth == 0 {
                            previous = Some('0');
                        }
                    }
                    _ => {}
                }
                self.pos += 1;
                continue;
            }
            if parameter_depth > 0 {
                match c {
                    '{' => parameter_depth += 1,
                    '}' => {
                        parameter_depth -= 1;
                        if parameter_depth == 0 {
                            previous = Some('0');
                        }
                    }
                    _ => {}
                }
                self.pos += 1;
                continue;
            }
            if in_comment {
                if c == '\n' || c == '\r' {
                    in_comment = false;
                } else {
                    self.pos += 1;
                    continue;
                }
            }

            match c {
                '\'' => {
                    in_single = true;
                    previous = Some(c);
                    self.pos += 1;
                }
                '"' => {
                    in_double = true;
                    previous = Some(c);
                    self.pos += 1;
                }
                '`' => {
                    in_backtick = true;
                    previous = Some(c);
                    self.pos += 1;
                }
                '\\' => {
                    escaped = true;
                    self.pos += 1;
                }
                '$' if self.pos + 2 < self.input.len()
                    && self.input[self.pos + 1] == '('
                    && self.input[self.pos + 2] == '(' =>
                {
                    arithmetic_depth = 2;
                    self.pos += 3;
                }
                '$' if self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '{' => {
                    parameter_depth = 1;
                    self.pos += 2;
                }
                '#' if posix_hash_starts_comment(previous) => {
                    in_comment = true;
                    self.pos += 1;
                }
                '<' if self.pos + 1 < self.input.len()
                    && self.input[self.pos + 1] == '<'
                    && !(self.pos + 2 < self.input.len()
                        && matches!(self.input[self.pos + 2], '<' | '(')) =>
                {
                    self.pos += 2;
                    let (delimiter, strip_tabs, quoted) = self.parse_heredoc_header()?;
                    pending.push(PendingHeredoc {
                        delimiter,
                        strip_tabs,
                        quoted,
                    });
                    // The header parser skipped the trailing blanks, so the
                    // next `#` is at the start of a word.
                    previous = Some(' ');
                }
                '{' => {
                    depth += 1;
                    previous = Some(c);
                    self.pos += 1;
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    previous = Some(c);
                    self.pos += 1;
                }
                '\n' | '\r' => {
                    if c == '\r'
                        && self.pos + 1 < self.input.len()
                        && self.input[self.pos + 1] == '\n'
                    {
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                    }
                    previous = Some('\n');
                    // The line inside the block ends here, so any pending
                    // bodies are part of the block: consume them as body text.
                    if !pending.is_empty() {
                        for header in std::mem::take(&mut pending) {
                            self.read_heredoc_body_content(&header.delimiter, header.strip_tabs)?;
                        }
                    }
                }
                _ => {
                    previous = Some(c);
                    self.pos += 1;
                }
            }
        }

        if depth != 0 {
            return Err(ParseError::Incomplete {
                construct: "posix",
                message: "Unclosed POSIX block — missing '}'".to_string(),
                span: self.current_span(),
            });
        }

        let mut body: String = self.input[start..self.pos].iter().collect();
        self.next_char(); // consume closing '}'

        if pending.is_empty() {
            return Ok(body);
        }

        // The headers are on the line that closes the block, so their bodies
        // follow the closing brace. Only the end of that line (optionally with
        // a comment) may follow: anything else belongs to the surrounding fsh
        // syntax, where the body's position would be ambiguous.
        self.skip_horizontal_whitespace();
        if self.peek() == Some('#') {
            while let Some(c) = self.peek() {
                if c == '\n' || c == '\r' {
                    break;
                }
                self.next_char();
            }
        }
        if !matches!(self.peek(), None | Some('\n') | Some('\r')) {
            return Err(ParseError::SyntaxError {
                message:
                    "a here-document inside an inline `sh { ... }` block must end the block's line"
                        .to_string(),
                span: self.current_span(),
            });
        }
        self.consume_line_ending();
        if !body.ends_with('\n') && !body.ends_with('\r') {
            body.push('\n');
        }
        for header in std::mem::take(&mut pending) {
            let body_start = self.pos;
            self.read_heredoc_body_content(&header.delimiter, header.strip_tabs)?;
            body.extend(self.input[body_start..self.pos].iter());
        }
        Ok(body)
    }

    pub(crate) fn parse_block_statements(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.parse_statements_until('}')
    }

    /// Parse a statement list terminated by `terminator`, which is consumed: `}`
    /// for a block, `)` for the statement list inside `$(...)`.
    pub(crate) fn parse_statements_until(
        &mut self,
        terminator: char,
    ) -> Result<Vec<Stmt>, ParseError> {
        let span = self.current_span();
        let _guard = crate::parser::RecursionGuard::new(&self.recursion_depth, span)?;
        let saved = self.statement_terminator;
        self.statement_terminator = terminator;
        let res = stacker::maybe_grow(32 * 1024, 1024 * 1024, || {
            self.parse_block_statements_inner()
        });
        self.statement_terminator = saved;
        res
    }

    fn parse_block_statements_inner(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let terminator = self.statement_terminator;
        let mut stmts = Vec::new();
        self.skip_whitespace();
        while !self.is_eof() && self.peek() != Some(terminator) {
            if self.peek() == Some(';') {
                self.next_char();
                self.skip_whitespace();
                continue;
            }
            if self.at_command_line_end() {
                self.finish_command_line()?;
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
        self.expect(terminator)?;
        Ok(stmts)
    }

    pub fn parse_pipeline(&mut self) -> Result<Pipeline, ParseError> {
        let mut stages = Vec::new();
        let mut boundaries = Vec::new();

        // Keep redirections in source order. The execution planner associates
        // them with the command in this segment; moving them while parsing
        // loses the ordering required by fd duplication (for example,
        // `2>&1 > file`).
        while let Some(redirect) = self.parse_redirect()? {
            stages.push(redirect);
            self.skip_whitespace();
        }

        if !matches!(self.peek(), None | Some('|') | Some(';') | Some('\n'))
            && self.peek() != Some(self.statement_terminator)
        {
            stages.push(self.parse_pipeline_stage()?);
            self.skip_whitespace();
            while let Some(r) = self.parse_redirect()? {
                stages.push(r);
                self.skip_whitespace();
            }
        }

        while self.peek() == Some('|') {
            if self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '|' {
                break;
            }
            self.next_char();
            boundaries.push(stages.len());
            let saved_subsequent = self.is_subsequent_stage;
            self.is_subsequent_stage = true;
            while let Some(redirect) = self.parse_redirect()? {
                stages.push(redirect);
                self.skip_whitespace();
            }
            let stage_res = self.parse_pipeline_stage();
            self.is_subsequent_stage = saved_subsequent;
            let stage = stage_res?;
            stages.push(stage);
            self.skip_whitespace();
            // Consume redirects after this stage (before next pipe or end)
            while let Some(r) = self.parse_redirect()? {
                stages.push(r);
                self.skip_whitespace();
            }
        }
        while let Some(write_stage) = self.parse_redirect()? {
            stages.push(write_stage);
            self.skip_whitespace();
        }
        Ok(Pipeline { stages, boundaries })
    }

    /// Parse a file redirect target using command-word boundaries. A bare dotted
    /// token such as `output.txt` is a path here, while quoted and variable
    /// expressions remain available for computed targets.
    fn parse_redirect_path(&mut self) -> Result<Expr, ParseError> {
        let saved_redirect = self.redirect_mode;
        let saved_arg = self.cmd_arg_mode;
        self.redirect_mode = true;
        self.cmd_arg_mode = true;
        let path = self.parse_expr_with_pipeline(false);
        self.redirect_mode = saved_redirect;
        self.cmd_arg_mode = saved_arg;
        path
    }

    /// Probe from a `NAME=value` at the cursor: whether a command word follows
    /// the value on the same line. Moves the cursor; the caller restores it.
    fn prefix_assignment_precedes_command(&mut self) -> bool {
        if self.peek() == Some('$') {
            self.next_char();
        }
        if self.parse_identifier().is_err() || self.expect('=').is_err() {
            return false;
        }
        if self.parse_assignment_value().is_err() {
            return false;
        }
        self.skip_horizontal_whitespace();
        self.peek().is_some_and(|c| {
            c.is_alphabetic() || matches!(c, '_' | '/' | '~' | '.' | '$' | '\'' | '"' | '\\')
        })
    }

    /// The value of a `NAME=value` prefix: one shell word, quoted, expanded
    /// or bare (`X='a b'`, `X=$HOME/bin`, `X=$(cmd)`), or empty (`X= cmd`).
    fn parse_assignment_value(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            None | Some(' ' | '\t' | '\n' | '\r' | ';' | '|' | '&' | '}' | ')') => {
                Ok(Expr::String(Vec::new()))
            }
            Some(_) => {
                let saved = self.cmd_arg_mode;
                self.cmd_arg_mode = true;
                let value = self.parse_command_arg();
                self.cmd_arg_mode = saved;
                value
            }
        }
    }

    /// Describe a redirection at the cursor that only the POSIX engine can
    /// perform, if there is one.
    fn posix_only_redirection(&self) -> Option<String> {
        let rest = &self.input[self.pos..];
        let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
        let after = &rest[digits..];
        let is_redirect = matches!(after.first(), Some('<' | '>'));
        if !is_redirect {
            return None;
        }
        // The descriptor being redirected.
        if digits > 0 {
            let fd: String = rest[..digits].iter().collect();
            if fd.parse::<u32>().map_or(true, |fd| fd > 2) {
                return Some(format!("redirection of descriptor {fd} is POSIX syntax"));
            }
        }
        match after {
            ['<', '>', ..] => Some("`<>` is POSIX syntax".to_string()),
            ['<', '&', ..] => Some("`<&` is POSIX syntax".to_string()),
            ['>', '&', target @ ..] => {
                let n = target.iter().take_while(|c| c.is_ascii_digit()).count();
                let fd: String = target[..n].iter().collect();
                (n > 0 && fd.parse::<u32>().map_or(true, |fd| fd > 2))
                    .then(|| format!("duplication onto descriptor {fd} is POSIX syntax"))
            }
            _ => None,
        }
    }

    pub(crate) fn parse_redirect(&mut self) -> Result<Option<PipelineStage>, ParseError> {
        self.skip_whitespace();

        // Native models descriptors 0–2 only. A redirection naming any other
        // (`3> f`, `>&5`, `exec 4<&0`) or the read-write and input-duplication
        // forms is POSIX syntax: reject it so the input goes to the POSIX
        // engine, which has a full descriptor table, instead of being misread.
        if let Some(message) = self.posix_only_redirection() {
            return Err(ParseError::SyntaxError {
                message,
                span: self.current_span(),
            });
        }

        // `>&N` / `1>&N`: duplicate stdout onto a standard descriptor.
        if self.peek() == Some('>')
            && self.input.get(self.pos + 1) == Some(&'&')
            && let Some(&dst) = self.input.get(self.pos + 2)
            && matches!(dst, '0'..='2' | '-')
            && self
                .input
                .get(self.pos + 3)
                .is_none_or(|c| c.is_whitespace() || matches!(c, ';' | '|' | '&' | ')' | '}'))
        {
            self.next_char();
            self.next_char();
            self.next_char();
            let dst_fd = if dst == '-' {
                -1
            } else {
                (dst as u8 - b'0') as i32
            };
            return Ok(Some(PipelineStage::FdRedirect { src_fd: 1, dst_fd }));
        }

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

        // Heredoc: `<<DELIM` / `<<-DELIM` / `<<'DELIM'` — content becomes stdin.
        // The body is not read here: like POSIX, every body on a command line
        // follows the newline that ends the line, in header order, so the header
        // is queued and the body is read when the line ends.
        if self.peek() == Some('<')
            && self.pos + 1 < self.input.len()
            && self.input[self.pos + 1] == '<'
            && !(self.pos + 2 < self.input.len() && self.input[self.pos + 2] == '(')
        {
            // Ensure not `<<<` already handled above
            self.next_char(); // <
            self.next_char(); // <
            let (delimiter, strip_tabs, quoted) = self.parse_heredoc_header()?;
            self.pending_heredocs.push(PendingHeredoc {
                delimiter: delimiter.clone(),
                strip_tabs,
                quoted,
            });
            return Ok(Some(PipelineStage::Heredoc {
                delimiter,
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
            let path = self.parse_redirect_path()?;
            return Ok(Some(PipelineStage::Read { path }));
        }

        // Check for standard stdin file redirect: `<` (not `<<` heredoc, `<<<` here-string, or `<(` process substitution)
        if self.peek() == Some('<')
            && !(self.pos + 1 < self.input.len()
                && (self.input[self.pos + 1] == '<' || self.input[self.pos + 1] == '('))
        {
            self.next_char(); // consume '<'
            self.skip_whitespace();
            let path = self.parse_redirect_path()?;
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
            let path = self.parse_redirect_path()?;
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

    /// Parse a bare command name, including punctuation commonly used in executable
    /// names (for example `clang++`, `g++-14`, and `tool.sh`).
    pub(crate) fn parse_command_name(&mut self) -> Result<String, ParseError> {
        if matches!(self.peek(), Some('/') | Some('~') | Some('.')) {
            let path_expr = self.parse_bare_path_or_string()?;
            match path_expr {
                Expr::String(parts) => {
                    let mut s = String::new();
                    for part in parts {
                        match part {
                            StringPart::Lit { text, .. } => s.push_str(&text),
                            StringPart::Expr { .. } => {
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
            // Keep punctuation that is valid within a shell word attached to the
            // executable name. `+` matters for compiler commands such as `clang++`
            // and `c++`; dot support preserves names like `tool.sh`.
            while let Some(c) = self.peek() {
                if c.is_alphanumeric() || matches!(c, '_' | '-' | '+' | '.') {
                    name.push(self.next_char().ok_or_else(|| ParseError::UnexpectedEof {
                        span: self.current_span(),
                    })?);
                } else {
                    break;
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
                        if len > MAX_HASH_XOF_OUTPUT_BYTES {
                            return Err(ParseError::SyntaxError {
                                message: format!(
                                    "XOF output length exceeds the {} byte shell limit",
                                    MAX_HASH_XOF_OUTPUT_BYTES
                                ),
                                span: self.current_span(),
                            });
                        }
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
                    let value = self.parse_assignment_value()?;
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
                "inspect" => SerializationFormat::Inspect,
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
        reject_posix_reserved_word(&name, self.span_from(start))?;
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
            *span = self.span_from(start);
        }
        Ok(stage)
    }

    pub(crate) fn parse_pipeline_stage_with_env(
        &mut self,
        inline_env: Vec<(String, Expr)>,
    ) -> Result<PipelineStage, ParseError> {
        self.skip_whitespace();
        let start = self.pos;
        let name = self.parse_command_name()?;
        reject_posix_reserved_word(&name, self.span_from(start))?;
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
            *span = self.span_from(start);
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
            // A reserved pipeline keyword immediately followed by a `-flag` is an
            // external command call, not the built-in stage (e.g. `sort -n`,
            // `map -x`). `hash` is exempt: its own flags (`-a`, `-o`,
            // `--per-record`) are part of the stage.
            "filter" if !has_flag => {
                let condition = self.parse_expr_with_pipeline_bool(false)?;
                Ok(PipelineStage::Filter { condition })
            }
            "map" if !has_flag => {
                let mut projections = Vec::new();
                loop {
                    self.skip_whitespace();
                    if matches!(
                        self.peek(),
                        Some('|') | Some('\n') | Some('\r') | Some(';') | None
                    ) {
                        break;
                    }
                    projections.push(self.parse_projection()?);
                    self.skip_horizontal_whitespace();
                    if self.peek() == Some(',') {
                        self.next_char();
                    }
                }
                Ok(PipelineStage::Map { projections })
            }
            // The structured stage sorts by a column named by a whole word
            // (`sort size`); `sort file.txt` is the sort utility on a file.
            "sort"
                if !has_flag
                    && self.peek_identifier().is_some_and(|column| {
                        self.input
                            .get(self.pos + column.len())
                            .is_none_or(|c| c.is_whitespace() || matches!(c, '|' | ';' | ')' | '}'))
                    }) =>
            {
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
            "mark" if !has_flag => {
                let pattern = self.parse_command_arg()?;
                Ok(PipelineStage::Mark { pattern })
            }
            "count" if !has_flag => Ok(PipelineStage::Count),
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
                        || (c == '#' && (self.pos == 0 || self.input[self.pos - 1].is_whitespace()))
                    {
                        break;
                    }
                    // Finder comparisons are arguments with their own grammar,
                    // not redirections. Only an unquoted predicate keyword opens
                    // this production; all other `<`/`>` retain shell semantics.
                    let finder_predicate = name == "ff"
                        && args.last().is_some_and(|arg| {
                            matches!(arg, Expr::String(parts) if matches!(parts.as_slice(),
                            [StringPart::Lit { text, quote: crate::ast::QuoteKind::Unquoted }]
                            if matches!(text.as_str(), "name" | "size" | "modified" | "type")))
                        });
                    if finder_predicate && matches!(c, '<' | '>' | '=' | '!') {
                        let op_start = self.pos;
                        let mut operator = String::new();
                        operator.push(self.next_char().unwrap_or(c));
                        if self.peek() == Some('=') {
                            operator.push(self.next_char().unwrap_or('='));
                        }
                        if !matches!(
                            operator.as_str(),
                            "<" | "<=" | ">" | ">=" | "=" | "!=" | "=="
                        ) {
                            return Err(ParseError::SyntaxError {
                                message: "invalid finder comparison".into(),
                                span: self.span_from(op_start),
                            });
                        }
                        args.push(Expr::String(vec![StringPart::unquoted(operator)]));
                        self.skip_horizontal_whitespace();
                        args.push(self.parse_command_arg()?);
                        self.skip_horizontal_whitespace();
                        continue;
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
                    let mut arg = self.parse_command_arg()?;
                    if (name == "export" || name == "unset")
                        && let Expr::Ident(id) = arg
                    {
                        arg = Expr::String(vec![StringPart::unquoted(id)]);
                    }
                    // Only an unquoted `{a,b}` is brace syntax. Quoted text that
                    // merely looks like braces — `'{"a":[1,2]}'` — is data, and
                    // splitting it on the comma inside `[…]` would hand the
                    // command fragments of a JSON argument.
                    if let Expr::String(parts) = &arg
                        && parts.len() == 1
                        && matches!(
                            &parts[0],
                            StringPart::Lit { text, quote }
                                if *quote == crate::ast::QuoteKind::Unquoted
                                    && text.contains('{')
                        )
                    {
                        if let StringPart::Lit { text: s, .. } = &parts[0] {
                            let expanded = expand_braces(s);
                            if expanded.len() > 1 {
                                for word in expanded {
                                    args.push(Expr::String(vec![StringPart::unquoted(word)]));
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

#[cfg(test)]
mod command_locations {
    use super::*;

    #[test]
    fn ordinary_command_stages_use_exact_unicode_byte_ranges() {
        let actual = Parser::new("echo 'é'|other")
            .parse_pipeline()
            .expect("command pipeline");
        assert_eq!(
            actual,
            Pipeline {
                stages: vec![
                    PipelineStage::CommandCall {
                        name: "echo".into(),
                        args: vec![Expr::String(vec![StringPart::Lit {
                            text: "é".into(),
                            quote: QuoteKind::Single,
                        }])],
                        env: Vec::new(),
                        span: (0, 9).into(),
                    },
                    PipelineStage::CommandCall {
                        name: "other".into(),
                        args: Vec::new(),
                        env: Vec::new(),
                        span: (10, 5).into()
                    },
                ],
                boundaries: vec![1],
            }
        );
    }

    #[test]
    fn inline_environment_prefix_is_included_in_the_pipeline_command_range() {
        let source = "VALUE=x echo 'é'";
        let actual = Parser::new(source)
            .parse_pipeline()
            .expect("prefixed command");
        assert_eq!(
            actual,
            Pipeline {
                stages: vec![PipelineStage::CommandCall {
                    name: "echo".into(),
                    args: vec![Expr::String(vec![StringPart::Lit {
                        text: "é".into(),
                        quote: QuoteKind::Single
                    }])],
                    env: vec![(
                        "VALUE".into(),
                        Expr::String(vec![StringPart::unquoted("x")])
                    )],
                    span: (0, source.len()).into(),
                }],
                boundaries: Vec::new()
            }
        );
    }

    #[test]
    fn already_parsed_environment_preserves_the_command_body_range() {
        let source = "echo 'é'";
        let env = vec![(
            "VALUE".into(),
            Expr::String(vec![StringPart::unquoted("x")]),
        )];
        let actual = Parser::new(source)
            .parse_pipeline_stage_with_env(env.clone())
            .expect("command stage");
        assert_eq!(
            actual,
            PipelineStage::CommandCall {
                name: "echo".into(),
                args: vec![Expr::String(vec![StringPart::Lit {
                    text: "é".into(),
                    quote: QuoteKind::Single
                }])],
                env,
                span: (0, source.len()).into(),
            }
        );
    }
}
