// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Linter that emits structured diagnostics through the miette/fshell-render
//! pipeline instead of ad-hoc line/column reports.
//!
//! Each lint rule is a variant of [`LintDiagnostic`] with a proper miette
//! annotation, source span, severity, help text, and error code.

use crate::diagnostic::{DiagnosticExt, ErrorCode, FshDiag};
use crate::{Expr, ParseError, Parser, Pipeline, Stmt};
use miette::{Diagnostic, Severity, SourceSpan};
use thiserror::Error;

/// Severity level for a lint rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintLevel {
    Warning,
    Error,
    Info,
}

impl From<LintLevel> for Severity {
    fn from(l: LintLevel) -> Self {
        match l {
            LintLevel::Warning => Severity::Warning,
            LintLevel::Error => Severity::Error,
            LintLevel::Info => Severity::Advice,
        }
    }
}

/// A structured lint diagnostic that renders through the fshell-render pipeline.
#[derive(Error, Diagnostic, Debug, Clone)]
#[error("{message}")]
pub struct LintDiagnostic {
    pub message: String,
    pub help: Option<String>,
    pub level: LintLevel,
    #[label]
    pub span: Option<SourceSpan>,
    pub code: ErrorCode,
}

impl DiagnosticExt for LintDiagnostic {
    fn category(&self) -> &'static str {
        self.code.category()
    }

    fn code_enum(&self) -> Option<ErrorCode> {
        Some(self.code)
    }

    fn suggestions(&self) -> Vec<String> {
        self.help.iter().cloned().collect()
    }
}

impl From<LintDiagnostic> for FshDiag {
    fn from(diag: LintDiagnostic) -> Self {
        FshDiag::new(diag)
    }
}

/// Run the linter on a source string. Returns structured diagnostics.
pub fn lint(source: &str) -> Vec<LintDiagnostic> {
    let mut diagnostics = Vec::new();

    check_empty_pipeline(source, &mut diagnostics);

    match Parser::new(source).parse_statements() {
        Err(parse_err) => {
            let span = parse_error_span(&parse_err);
            diagnostics.push(LintDiagnostic {
                level: LintLevel::Error,
                message: parse_err.to_string(),
                span: Some(span),
                help: Some("Check the syntax around this position.".into()),
                code: ErrorCode::LintParseError,
            });
        }
        Ok(stmts) => {
            for stmt in &stmts {
                check_stmt(stmt, source, &mut diagnostics);
            }
        }
    }

    diagnostics
}

/// Detect a trailing `|` with nothing after it (empty pipeline stage).
fn check_empty_pipeline(source: &str, diagnostics: &mut Vec<LintDiagnostic>) {
    let bytes = source.as_bytes();
    if bytes.is_empty() {
        return;
    }
    let mut i = bytes.len();
    loop {
        if i == 0 {
            return;
        }
        i -= 1;
        if bytes[i] == b'|' {
            // Skip `||` (logical or)
            if i > 0 && bytes[i - 1] == b'|' {
                return;
            }
            if i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                return;
            }
            let after = i + 1;
            if after >= bytes.len() || bytes[after..].iter().all(|&b| b.is_ascii_whitespace()) {
                diagnostics.push(LintDiagnostic {
                    level: LintLevel::Warning,
                    message: "Empty pipeline stage: nothing follows '|'".to_string(),
                    span: Some(SourceSpan::new(i.into(), 1)),
                    help: Some("Remove the trailing `|` or add a command after it.".into()),
                    code: ErrorCode::LintEmptyStage,
                });
            }
            return;
        } else if !bytes[i].is_ascii_whitespace() {
            return;
        }
    }
}

/// Recursively check a statement tree for lint issues.
fn check_stmt(stmt: &Stmt, source: &str, diagnostics: &mut Vec<LintDiagnostic>) {
    let inner = stmt.unpack();
    match inner {
        Stmt::Expr(expr) => check_expr(expr, source, diagnostics),
        Stmt::Let { expr, .. } | Stmt::Assign { expr, .. } | Stmt::Update { expr, .. } => {
            check_expr(expr, source, diagnostics)
        }
        Stmt::Return(expr) => check_expr(expr, source, diagnostics),
        Stmt::Exit(Some(expr)) => check_expr(expr, source, diagnostics),
        Stmt::While { condition, body }
        | Stmt::For {
            iter: condition,
            body,
            ..
        } => {
            check_expr(condition, source, diagnostics);
            for s in body {
                check_stmt(s, source, diagnostics);
            }
        }
        Stmt::FnDef { body, .. } => {
            for s in body {
                check_stmt(s, source, diagnostics);
            }
        }
        Stmt::Match { expr, arms } => {
            check_expr(expr, source, diagnostics);
            for arm in arms {
                for s in &arm.body {
                    check_stmt(s, source, diagnostics);
                }
            }
        }
        Stmt::TryCatch {
            try_body,
            catch_body,
            ..
        } => {
            for s in try_body {
                check_stmt(s, source, diagnostics);
            }
            for s in catch_body {
                check_stmt(s, source, diagnostics);
            }
        }
        Stmt::WithCaps { body, .. } => {
            for s in body {
                check_stmt(s, source, diagnostics);
            }
        }
        Stmt::ReactiveCell { pipeline, .. } => check_pipeline(pipeline, source, diagnostics),
        Stmt::ReactiveCellEvery { body, .. } => {
            for s in body {
                check_stmt(s, source, diagnostics);
            }
        }
        Stmt::Unsafe { body } => {
            for s in body {
                check_stmt(s, source, diagnostics);
            }
        }
        Stmt::Source { path, .. } => check_expr(path, source, diagnostics),
        Stmt::On {
            handler: crate::OnHandler::Block(body),
            ..
        } => {
            for s in body {
                check_stmt(s, source, diagnostics);
            }
        }
        Stmt::Background(stmt) => check_stmt(stmt, source, diagnostics),
        Stmt::And(a, b) | Stmt::Or(a, b) => {
            check_stmt(a, source, diagnostics);
            check_stmt(b, source, diagnostics);
        }
        Stmt::Every { body, .. } => {
            for s in body {
                check_stmt(s, source, diagnostics);
            }
        }
        _ => {}
    }
}

/// Recursively check expressions for lint issues.
fn check_expr(expr: &Expr, source: &str, diagnostics: &mut Vec<LintDiagnostic>) {
    let inner = expr.unpack();
    match inner {
        Expr::Pipeline(pipeline) | Expr::InlinePipeline(pipeline) => {
            check_pipeline(pipeline, source, diagnostics);
        }
        Expr::If {
            condition,
            then_body,
            else_body,
        } => {
            check_expr(condition, source, diagnostics);
            for s in then_body {
                check_stmt(s, source, diagnostics);
            }
            if let Some(body) = else_body {
                for s in body {
                    check_stmt(s, source, diagnostics);
                }
            }
        }
        Expr::BinaryOp { lhs, rhs, .. } => {
            check_expr(lhs, source, diagnostics);
            check_expr(rhs, source, diagnostics);
        }
        Expr::Not(inner) => check_expr(inner, source, diagnostics),
        Expr::MemberAccess { expr, .. } => check_expr(expr, source, diagnostics),
        Expr::List(items) => {
            for item in items {
                check_expr(item, source, diagnostics);
            }
        }
        Expr::Map(entries) => {
            for (_, value) in entries {
                check_expr(value, source, diagnostics);
            }
        }
        _ => {}
    }
}

fn check_pipeline(pipeline: &Pipeline, _source: &str, diagnostics: &mut Vec<LintDiagnostic>) {
    if pipeline.stages.is_empty() {
        diagnostics.push(LintDiagnostic {
            level: LintLevel::Warning,
            message: "Empty pipeline has no effect".to_string(),
            span: None,
            help: Some("Add pipeline stages or remove the empty pipeline.".into()),
            code: ErrorCode::LintEmptyPipeline,
        });
    }
}

/// Extract a span from a ParseError.
fn parse_error_span(err: &ParseError) -> SourceSpan {
    match err {
        ParseError::UnexpectedEof { span }
        | ParseError::ExpectedChar { span, .. }
        | ParseError::ExpectedToken { span, .. }
        | ParseError::SyntaxError { span, .. } => *span,
    }
}

/// Convert a byte offset in source text to a 1-based (line, column) pair.
pub fn offset_to_line_col(source: &str, byte_offset: usize) -> (usize, usize) {
    if byte_offset >= source.len() {
        let lines: Vec<&str> = source.split('\n').collect();
        return (lines.len(), 0);
    }
    let mut line = 1;
    let mut col = 1;
    for (i, c) in source.char_indices() {
        if i == byte_offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lint_valid_script() {
        let result = lint("ls -la | count");
        assert!(
            result.is_empty(),
            "Expected no diagnostics, got: {result:?}"
        );
    }

    #[test]
    fn test_lint_parse_error() {
        let result = lint("let x =");
        assert!(!result.is_empty());
        assert_eq!(result[0].level, LintLevel::Error);
        assert!(result[0].span.is_some());
    }

    #[test]
    fn test_lint_empty_pipeline() {
        let result = lint("echo hello | ");
        assert!(!result.is_empty());
        let has_empty = result.iter().any(|d| d.message.contains("Empty pipeline"));
        assert!(
            has_empty,
            "Expected empty pipeline warning, got: {result:?}"
        );
        let empty = result
            .iter()
            .find(|d| d.message.contains("Empty pipeline"))
            .unwrap();
        assert!(empty.span.is_some(), "Empty pipeline should have a span");
    }

    #[test]
    fn test_lint_diagnostic_is_diagnostic() {
        let diag = LintDiagnostic {
            level: LintLevel::Warning,
            message: "test".into(),
            span: Some(SourceSpan::new(0.into(), 4)),
            help: Some("do better".into()),
            code: ErrorCode::LintEmptyStage,
        };
        let wrapped: FshDiag = diag.into();
        let rendered = format!("{}", wrapped.report);
        assert!(
            rendered.contains("test"),
            "message should appear in rendered output"
        );
    }

    #[test]
    fn test_offset_to_line_col() {
        let source = "line1\nline2\nline3";
        assert_eq!(offset_to_line_col(source, 0), (1, 1));
        assert_eq!(offset_to_line_col(source, 6), (2, 1));
        assert_eq!(offset_to_line_col(source, 12), (3, 1));
    }
}
