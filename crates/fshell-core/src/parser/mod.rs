// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::ast::*;
use crate::diagnostic::DiagnosticExt;
use miette::{Diagnostic, SourceSpan};
use thiserror::Error;

pub mod expr;
pub mod lexer;
pub mod stmt;

/// Parse error with detailed line/column information and miette reporting.
#[derive(Error, Debug, Diagnostic, Clone, PartialEq)]
pub enum ParseError {
    #[error("Unexpected end of input")]
    #[diagnostic(
        code = "FSH-PARSE-003",
        help(
            "This expression is incomplete. Try adding a value after the operator or closing the bracket."
        )
    )]
    UnexpectedEof {
        #[label("input ended here")]
        span: SourceSpan,
    },

    #[error("Expected character '{expected}', found '{found}'")]
    #[diagnostic(
        code = "FSH-PARSE-004",
        help("Check the syntax around this position for missing or incorrect characters.")
    )]
    ExpectedChar {
        expected: char,
        found: char,
        #[label("here")]
        span: SourceSpan,
    },

    #[error("Expected {expected}, found {found}")]
    #[diagnostic(
        code = "FSH-PARSE-005",
        help("Check the syntax around this position. Did you miss an operator or a value?")
    )]
    ExpectedToken {
        expected: String,
        found: String,
        #[label("here")]
        span: SourceSpan,
    },

    #[error("{message}")]
    #[diagnostic(
        code = "FSH-PARSE-002",
        help(
            "Review the syntax at the marked location. This might be a misplaced character or missing operator."
        )
    )]
    SyntaxError {
        message: String,
        #[label("error here")]
        span: SourceSpan,
    },
}

impl ParseError {
    pub fn span(&self) -> SourceSpan {
        match self {
            ParseError::UnexpectedEof { span }
            | ParseError::ExpectedChar { span, .. }
            | ParseError::ExpectedToken { span, .. }
            | ParseError::SyntaxError { span, .. } => *span,
        }
    }
}

pub struct Parser {
    input: Vec<char>,
    /// Byte offset of each `input[i]` (plus a final entry for end-of-input).
    /// miette `SourceSpan`s are byte offsets, while `pos` counts chars, so spans
    /// must be translated or diagnostics mis-point on non-ASCII input.
    byte_offsets: Vec<usize>,
    pos: usize,
    /// When true, `>` is not treated as an infix operator (BinOp::Gt),
    /// allowing it to be used for output redirection at the pipeline level.
    redirect_mode: bool,
    /// When true, bare symbols like `=`, `!=`, `!`, `[`, `]` are parsed as string literals.
    cmd_arg_mode: bool,
    /// When true, we are parsing a subsequent stage of a pipeline (after the first stage),
    /// which allows keywords like `hash` to be parsed as pipeline stages instead of commands.
    is_subsequent_stage: bool,
    /// When true, `&&`/`||` are boolean operators inside the expression. They are
    /// only enabled in value/condition contexts (right-hand sides, `if`/`while`
    /// conditions, ...); at statement level they keep their shell meaning of
    /// chaining on the previous command's exit status.
    bool_ops: bool,
    recursion_depth: std::cell::Cell<usize>,
}

pub(crate) const MAX_PARSER_RECURSION: usize = 1024;

pub(crate) struct RecursionGuard {
    depth_ptr: *const std::cell::Cell<usize>,
}

impl RecursionGuard {
    pub(crate) fn new(
        depth: &std::cell::Cell<usize>,
        span: miette::SourceSpan,
    ) -> Result<Self, ParseError> {
        let d = depth.get();
        if d >= MAX_PARSER_RECURSION {
            return Err(ParseError::SyntaxError {
                message: format!(
                    "Expression too deeply nested (recursion limit {MAX_PARSER_RECURSION} exceeded)"
                ),
                span,
            });
        }
        depth.set(d + 1);
        Ok(Self {
            depth_ptr: depth as *const std::cell::Cell<usize>,
        })
    }
}

impl Drop for RecursionGuard {
    fn drop(&mut self) {
        // SAFETY: depth_ptr was derived from a valid &Cell and lives as long as the Parser.
        unsafe {
            let cell = &*self.depth_ptr;
            let d = cell.get();
            cell.set(d.saturating_sub(1));
        }
    }
}

impl DiagnosticExt for ParseError {
    fn category(&self) -> &'static str {
        "syntax"
    }

    fn code_enum(&self) -> Option<crate::diagnostic::ErrorCode> {
        match self {
            ParseError::UnexpectedEof { .. } => Some(crate::diagnostic::ErrorCode::UnexpectedEof),
            ParseError::ExpectedChar { .. } => Some(crate::diagnostic::ErrorCode::ExpectedChar),
            ParseError::ExpectedToken { .. } => Some(crate::diagnostic::ErrorCode::ExpectedToken),
            ParseError::SyntaxError { .. } => Some(crate::diagnostic::ErrorCode::SyntaxError),
        }
    }
}
// Standalone helper functions
/// Interpret C-style escape sequences in ANSI-C quoted strings ($'...').
fn parse_ansi_escapes(s: &str, base_span: SourceSpan) -> Result<String, ParseError> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('\'') => out.push('\''),
                Some('\"') => out.push('\"'),
                Some(c @ ('u' | 'U')) => {
                    let width = if c == 'u' { 4 } else { 8 };
                    let mut hex = String::with_capacity(width);
                    for _ in 0..width {
                        match chars.next() {
                            Some(h) if h.is_ascii_hexdigit() => hex.push(h),
                            _ => {
                                return Err(ParseError::SyntaxError {
                                    message: format!(
                                        "invalid \\{c} escape: expected {width} hexadecimal digits"
                                    ),
                                    span: base_span,
                                });
                            }
                        }
                    }
                    let code_point =
                        u32::from_str_radix(&hex, 16).map_err(|_| ParseError::SyntaxError {
                            message: "invalid hexadecimal digits".to_string(),
                            span: base_span,
                        })?;
                    let ch = char::from_u32(code_point).ok_or_else(|| ParseError::SyntaxError {
                        message: format!("invalid unicode code point U+{code_point:04X}"),
                        span: base_span,
                    })?;
                    out.push(ch);
                }
                Some('x') => {
                    let mut hex = String::with_capacity(2);
                    for _ in 0..2 {
                        match chars.next() {
                            Some(h) if h.is_ascii_hexdigit() => hex.push(h),
                            _ => {
                                return Err(ParseError::SyntaxError {
                                    message: "Invalid hex escape: expected two hexadecimal digits after \\x".to_string(),
                                    span: base_span,
                                });
                            }
                        }
                    }
                    let byte =
                        u8::from_str_radix(&hex, 16).map_err(|_| ParseError::SyntaxError {
                            message: "Invalid hex digits".to_string(),
                            span: base_span,
                        })?;
                    out.push(byte as char);
                }
                Some(c @ '0'..='7') => {
                    let mut oct = String::with_capacity(3);
                    oct.push(c);
                    for _ in 0..2 {
                        if let Some(&o) = chars.peek() {
                            if ('0'..='7').contains(&o) {
                                if let Some(next_c) = chars.next() {
                                    oct.push(next_c);
                                }
                            } else {
                                break;
                            }
                        }
                    }
                    let byte =
                        u8::from_str_radix(&oct, 8).map_err(|_| ParseError::SyntaxError {
                            message: "Invalid octal digits".to_string(),
                            span: base_span,
                        })?;
                    out.push(byte as char);
                }
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => {
                    return Err(ParseError::UnexpectedEof { span: base_span });
                }
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

/// Drop the trailing line of a block string when it is only indentation.
///
/// Triple-quoted bodies usually put the closing delimiter on its own (possibly
/// indented) line. Without this, `dedent` would leave a stray trailing newline
/// (or an indentation-only final line) in the value.
fn trim_block_tail(content: &mut String) {
    if content.ends_with('\n') {
        content.pop();
    } else if let Some(last_newline) = content.rfind('\n') {
        if content[last_newline + 1..].trim().is_empty() {
            content.truncate(last_newline);
        }
    } else if content.trim().is_empty() {
        content.clear();
    }
}

/// Strip common leading whitespace from all non-empty lines.
fn dedent(s: &str, mode: DedentMode) -> String {
    match mode {
        DedentMode::None => s.to_string(),
        DedentMode::All => {
            let lines: Vec<&str> = s.lines().collect();
            let min_indent = lines
                .iter()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.len() - l.trim_start().len())
                .min()
                .unwrap_or(0);
            lines
                .iter()
                .map(|l| {
                    if l.len() >= min_indent {
                        &l[min_indent..]
                    } else {
                        l
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        DedentMode::LeadingTabs => s
            .lines()
            .map(|l| l.trim_start_matches('\t'))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Parse the interpolation markers of a raw string body into `StringPart`s.
///
/// This is the single interpolation grammar shared by triple-quoted strings and
/// heredocs. It understands exactly the same constructs as a double-quoted
/// string body:
///
/// - `${name}` / `${name:modifier}` — parameter expansion (the same grammar as
///   double-quoted strings),
/// - `$name`, `$1`, `$?`, `$#`, `$@`, `$*`, `$$` — variable / special parameters,
/// - `$(pipeline)` — command substitution,
/// - `$((expr))` — arithmetic expansion.
///
/// Bare `{...}` is intentionally *not* interpolation here: triple-quoted strings
/// and heredocs carry raw text (JSON, nginx configs, SQL) where braces are
/// literal. Use double-quoted strings for `{expr}` interpolation, or `<<'EOF'` /
/// `'''...'''` for a fully raw body.
fn parse_string_parts(s: &str, base_span: SourceSpan) -> Result<Vec<StringPart>, ParseError> {
    parse_string_parts_inner(s).map_err(|e| ParseError::SyntaxError {
        message: format!("Invalid interpolation: {e}"),
        span: base_span,
    })
}

fn parse_string_parts_inner(s: &str) -> Result<Vec<StringPart>, ParseError> {
    fn flush(parts: &mut Vec<StringPart>, lit: &mut String) {
        if !lit.is_empty() {
            parts.push(StringPart::Lit(std::mem::take(lit)));
        }
    }

    let mut p = Parser::new(s);
    let mut parts: Vec<StringPart> = Vec::new();
    let mut lit = String::new();

    while let Some(c) = p.peek() {
        match c {
            '$' => {
                let next = p.input.get(p.pos + 1).copied();
                match next {
                    Some('{') => {
                        p.next_char(); // '$'
                        flush(&mut parts, &mut lit);
                        let saved = p.cmd_arg_mode;
                        p.cmd_arg_mode = false;
                        let e = p.parse_braced_variable()?;
                        p.cmd_arg_mode = saved;
                        parts.push(StringPart::Expr(Box::new(e)));
                    }
                    Some('(') => {
                        p.next_char(); // '$'
                        flush(&mut parts, &mut lit);
                        let saved = p.cmd_arg_mode;
                        p.cmd_arg_mode = false;
                        // Arithmetic expansion `$(( ... ))` — try it first and fall
                        // back to command substitution if it does not close.
                        let mut handled = false;
                        if p.peek() == Some('(') && p.input.get(p.pos + 1).copied() == Some('(') {
                            let saved_pos = p.pos;
                            p.next_char(); // first '('
                            p.next_char(); // second '('
                            if let Ok(inner) = p.parse_expr() {
                                p.skip_whitespace();
                                if p.peek() == Some(')') {
                                    p.next_char();
                                    p.skip_whitespace();
                                    if p.peek() == Some(')') {
                                        p.next_char();
                                        parts.push(StringPart::Expr(Box::new(
                                            Expr::ArithmeticExpansion(Box::new(inner)),
                                        )));
                                        handled = true;
                                    }
                                }
                            }
                            if !handled {
                                p.pos = saved_pos;
                            }
                        }
                        if !handled {
                            let e = p.parse_cmd_substitution()?;
                            parts.push(StringPart::Expr(Box::new(e)));
                        }
                        p.cmd_arg_mode = saved;
                    }
                    Some(sp) if matches!(sp, '?' | '#' | '@' | '*' | '$') => {
                        p.next_char(); // '$'
                        p.next_char(); // special parameter char
                        flush(&mut parts, &mut lit);
                        parts.push(StringPart::Expr(Box::new(Expr::Variable(sp.to_string()))));
                    }
                    Some(nc) if nc.is_ascii_alphanumeric() || nc == '_' => {
                        p.next_char(); // '$'
                        let mut name = String::new();
                        while let Some(nc) = p.peek() {
                            if nc.is_ascii_alphanumeric() || nc == '_' {
                                name.push(p.next_char().expect("peeked a character"));
                            } else {
                                break;
                            }
                        }
                        flush(&mut parts, &mut lit);
                        parts.push(StringPart::Expr(Box::new(Expr::Variable(name))));
                    }
                    _ => {
                        p.next_char();
                        lit.push('$');
                    }
                }
            }
            _ => {
                lit.push(p.next_char().expect("peeked a character"));
            }
        }
    }

    flush(&mut parts, &mut lit);
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_let() {
        let mut p = Parser::new("let x = 42");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Let { name, expr } = stmts[0].unpack() {
            assert_eq!(name, "x");
            assert_eq!(expr.unpack(), &Expr::Int(42));
        } else {
            panic!("Expected Stmt::Let");
        }
    }

    #[test]
    fn test_parse_comment() {
        let mut p = Parser::new("# This is a comment statement\nlet x = 42\n# Another comment");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 3);
        if let Stmt::Comment(content) = stmts[0].unpack() {
            assert_eq!(content, " This is a comment statement");
        } else {
            panic!("Expected Stmt::Comment");
        }
        if let Stmt::Let { name, .. } = stmts[1].unpack() {
            assert_eq!(name, "x");
        } else {
            panic!("Expected Stmt::Let");
        }
        if let Stmt::Comment(content) = stmts[2].unpack() {
            assert_eq!(content, " Another comment");
        } else {
            panic!("Expected Stmt::Comment");
        }
    }

    #[test]
    fn test_parse_every() {
        let mut p = Parser::new("every 5s { ls }");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Every {
            duration,
            unit,
            body,
        } = stmts[0].unpack()
        {
            assert_eq!(*duration, 5);
            assert_eq!(*unit, TimeUnit::Second);
            assert_eq!(body.len(), 1);
        } else {
            panic!("Expected Stmt::Every");
        }
    }

    #[test]
    fn test_parse_reactive_cell_every() {
        let mut p = Parser::new("$= live = every 10s { ls }");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::ReactiveCellEvery {
            name,
            duration,
            unit,
            body,
        } = stmts[0].unpack()
        {
            assert_eq!(name, "live");
            assert_eq!(*duration, 10);
            assert_eq!(*unit, TimeUnit::Second);
            assert_eq!(body.len(), 1);
        } else {
            panic!("Expected Stmt::ReactiveCellEvery");
        }
    }

    #[test]
    fn test_parse_reactive_cell() {
        let mut p = Parser::new("$= live = ls | filter size > 100");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::ReactiveCell { name, pipeline } = stmts[0].unpack() {
            assert_eq!(name, "live");
            assert_eq!(pipeline.stages.len(), 2);
            assert!(matches!(
                pipeline.stages[0],
                PipelineStage::CommandCall { .. }
            ));
            assert!(matches!(pipeline.stages[1], PipelineStage::Filter { .. }));
        } else {
            panic!("Expected Stmt::ReactiveCell");
        }
    }

    #[test]
    fn test_parse_with_caps() {
        let mut p = Parser::new("with caps(fs.read) { ls }");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::WithCaps { caps, body } = stmts[0].unpack() {
            assert_eq!(caps.len(), 1);
            assert_eq!(body.len(), 1);
        } else {
            panic!("Expected Stmt::WithCaps");
        }
    }

    #[test]
    fn test_parse_fn_structural() {
        let mut p = Parser::new(
            "fn check(node: {ip: String, status: String, ..rest} as my_alias) { ping }",
        );
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::FnDef { name, params, .. } = stmts[0].unpack() {
            assert_eq!(name, "check");
            assert_eq!(params.len(), 1);
            assert_eq!(params[0].name, "node");
            assert!(matches!(
                params[0].constraint,
                TypeConstraint::Structural { .. }
            ));
        } else {
            panic!("Expected Stmt::FnDef");
        }
    }

    #[test]
    fn test_parse_single_command_and_expression() {
        // Standalone command call should parse as a single-stage Pipeline
        let mut p1 = Parser::new("ls");
        let stmts1 = p1.parse_statements().unwrap();
        assert_eq!(stmts1.len(), 1);
        if let Stmt::Expr(expr) = stmts1[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 1);
                assert!(matches!(
                    pipeline.stages[0],
                    PipelineStage::CommandCall { .. }
                ));
            } else {
                panic!("Expected Pipeline expression");
            }
        } else {
            panic!("Expected single command 'ls' to parse as a pipeline");
        }

        // Standalone command call with arguments should parse as a single-stage Pipeline
        let mut p2 = Parser::new("ls Users");
        let stmts2 = p2.parse_statements().unwrap();
        assert_eq!(stmts2.len(), 1);
        if let Stmt::Expr(expr) = stmts2[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 1);
                if let PipelineStage::CommandCall { name, args, .. } = &pipeline.stages[0] {
                    assert_eq!(name, "ls");
                    assert_eq!(args.len(), 1);
                } else {
                    panic!("Expected CommandCall stage");
                }
            } else {
                panic!("Expected Pipeline expression");
            }
        } else {
            panic!("Expected command call with arguments to parse as a pipeline");
        }

        // Standard math expression should NOT parse as a Pipeline
        let mut p3 = Parser::new("1 + 2");
        let stmts3 = p3.parse_statements().unwrap();
        assert_eq!(stmts3.len(), 1);
        if let Stmt::Expr(expr) = stmts3[0].unpack() {
            assert!(!matches!(expr.unpack(), Expr::Pipeline(_)));
        } else {
            panic!("Expected Stmt::Expr");
        }
    }

    #[test]
    fn test_parse_match_statement() {
        // Match with wildcard and literal patterns
        let mut p = Parser::new("match x { 42 => \"the answer\", _ => \"unknown\" }");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Match { arms, .. } = stmts[0].unpack() {
            assert_eq!(arms.len(), 2);
            assert!(matches!(
                arms[0].pattern,
                MatchPattern::Literal(LiteralPattern::Int(42))
            ));
            assert!(matches!(arms[1].pattern, MatchPattern::Wildcard));
        } else {
            panic!("Expected Stmt::Match");
        }
    }

    #[test]
    fn test_parse_match_map_pattern_debug() {
        let mut p = Parser::new("match item { {type: \"file\", name: n} => n, _ => \"other\" }");
        let result = p.parse_statement();
        eprintln!("Result: {:?}", result);
        assert!(result.is_ok(), "Failed: {:?}", result.err());
    }

    #[test]
    fn test_match_pattern_bare_identifier_is_binding() {
        let mut p = Parser::new("match item { {name: n, ..} => n, _ => \"other\" }");
        let stmts = p.parse_statements().unwrap();
        let Stmt::Match { arms, .. } = stmts[0].unpack() else {
            panic!("Expected Stmt::Match");
        };
        let MatchPattern::Map { fields, .. } = &arms[0].pattern else {
            panic!("Expected map pattern, got {:?}", arms[0].pattern);
        };
        assert_eq!(fields.len(), 1);
        assert!(
            matches!(&fields[0].1, MatchPattern::Bind(n) if n == "n"),
            "bare identifier must bind, got {:?}",
            fields[0].1
        );
    }

    #[test]
    fn test_parse_match_map_pattern() {
        let mut p = Parser::new("match item { {type: \"file\", name: n} => n, _ => \"other\" }");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Match { arms, .. } = stmts[0].unpack() {
            assert_eq!(arms.len(), 2);
            assert!(matches!(arms[0].pattern, MatchPattern::Map { .. }));
            assert!(matches!(arms[1].pattern, MatchPattern::Wildcard));
        } else {
            panic!("Expected Stmt::Match");
        }
    }

    #[test]
    fn test_parse_unsafe_block() {
        let mut p = Parser::new("unsafe { rm foo }");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Unsafe { body } = stmts[0].unpack() {
            assert_eq!(body.len(), 1);
            assert!(matches!(body[0].unpack(), Stmt::Expr(Expr::Spanned { .. })));
        } else {
            panic!("Expected Stmt::Unsafe");
        }
    }

    #[test]
    fn test_parse_unsafe_multiple_stmts() {
        let mut p = Parser::new("unsafe { rm foo; rm bar }");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Unsafe { body } = stmts[0].unpack() {
            assert_eq!(body.len(), 2);
        } else {
            panic!("Expected Stmt::Unsafe");
        }
    }

    #[test]
    fn test_parse_unsafe_wrapping_reactive_cell() {
        let mut p = Parser::new("unsafe { $= x = rm foo }");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Unsafe { body } = stmts[0].unpack() {
            assert_eq!(body.len(), 1);
            assert!(matches!(body[0].unpack(), Stmt::ReactiveCell { .. }));
        } else {
            panic!("Expected Stmt::Unsafe wrapping Stmt::ReactiveCell");
        }
    }

    #[test]
    fn test_parse_limit_stage() {
        let mut p = Parser::new("ls | limit 5");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 2);
                assert!(matches!(
                    pipeline.stages[0],
                    PipelineStage::CommandCall { .. }
                ));
                if let PipelineStage::Limit { amount } = &pipeline.stages[1] {
                    assert_eq!(amount.unpack(), &Expr::Int(5));
                } else {
                    panic!("Expected PipelineStage::Limit");
                }
            } else {
                panic!("Expected Expr::Pipeline");
            }
        } else {
            panic!("Expected Stmt::Expr");
        }
    }

    #[test]
    fn test_parse_map_literal_hybrid() {
        let mut p = Parser::new("{host: \"localhost\", \"port\": 8080}");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Map(pairs) = expr.unpack() {
                assert_eq!(pairs.len(), 2);
                assert_eq!(pairs[0].0, "host");
                assert_eq!(pairs[1].0, "port");
            } else {
                panic!("Expected Expr::Map");
            }
        } else {
            panic!("Expected Stmt::Expr");
        }
    }

    #[test]
    fn test_string_escape_hex() {
        let mut parser = Parser::new(r#""\x1Bhello""#);
        let expr = parser.parse_expr().unwrap();
        assert_eq!(
            expr.unpack(),
            &Expr::String(vec![StringPart::Lit("\x1Bhello".to_string())])
        );
    }

    #[test]
    fn test_string_escape_unicode() {
        let mut parser = Parser::new(r#""\u00E9""#);
        let expr = parser.parse_expr().unwrap();
        assert_eq!(
            expr.unpack(),
            &Expr::String(vec![StringPart::Lit("\u{00E9}".to_string())])
        );
    }

    #[test]
    fn test_map_key_escape_unicode() {
        let mut parser = Parser::new(r#"{ "\u00E9": 1 }"#);
        let expr = parser.parse_expr().unwrap();
        match expr.unpack() {
            Expr::Map(pairs) => {
                assert_eq!(pairs.len(), 1);
                assert_eq!(pairs[0].0, "\u{00E9}");
            }
            _ => panic!("Expected Expr::Map"),
        }
    }

    #[test]
    fn test_string_escape_null() {
        let mut parser = Parser::new(r#""null\0byte""#);
        let expr = parser.parse_expr().unwrap();
        assert_eq!(
            expr.unpack(),
            &Expr::String(vec![StringPart::Lit("null\x00byte".to_string())])
        );
    }

    #[test]
    fn test_string_escape_preserves_unknown() {
        // Unknown escapes like \z, \., \s are preserved literally
        // (bash double-quote convention) so regex patterns pass through.
        use crate::ast::{Expr, StringPart};

        let mut parser = Parser::new(r#""\z""#);
        let result = parser.parse_expr().unwrap();
        assert_eq!(
            result.unpack(),
            &Expr::String(vec![StringPart::Lit("\\z".to_string())])
        );

        // Test \. specifically — common grep use case
        let mut parser2 = Parser::new(r#""\.""#);
        let result2 = parser2.parse_expr().unwrap();
        assert_eq!(
            result2.unpack(),
            &Expr::String(vec![StringPart::Lit("\\.".to_string())])
        );
    }

    #[test]
    fn test_string_escape_invalid_hex() {
        let mut parser = Parser::new(r#""\xGH""#);
        let result = parser.parse_expr();
        assert!(result.is_err(), "Invalid hex escape should produce error");
    }

    #[test]
    fn test_ansi_c_quote_unicode_and_octal() {
        // \uXXXX
        let expr = Parser::new(r"$'\u00E9'").parse_expr().unwrap();
        assert_eq!(expr.unpack(), &Expr::AnsiCQuote("\u{00E9}".to_string()));

        // \UXXXXXXXX
        let expr = Parser::new(r"$'\U0001F600'").parse_expr().unwrap();
        assert_eq!(expr.unpack(), &Expr::AnsiCQuote("\u{1F600}".to_string()));

        // \0 is NUL, and octal escapes consume up to three digits (bash parity):
        // \101 == 'A', \102 == 'B'.
        let expr = Parser::new(r"$'\0'").parse_expr().unwrap();
        assert_eq!(expr.unpack(), &Expr::AnsiCQuote("\0".to_string()));
        let expr = Parser::new(r"$'\101\102'").parse_expr().unwrap();
        assert_eq!(expr.unpack(), &Expr::AnsiCQuote("AB".to_string()));

        // A surrogate code point is not a valid char.
        assert!(
            Parser::new(r"$'\uD800'").parse_expr().is_err(),
            "surrogate escape must be rejected"
        );
    }

    #[test]
    fn test_parse_if_else() {
        let mut parser = Parser::new("if true { 1 } else { 2 }");
        let expr = parser.parse_expr().unwrap();
        match expr.unpack() {
            Expr::If {
                condition,
                then_body,
                else_body,
            } => {
                assert_eq!(condition.unpack(), &Expr::Bool(true));
                assert_eq!(then_body.len(), 1);
                assert!(else_body.is_some());
                let else_body = else_body.as_ref().unwrap();
                assert_eq!(else_body.len(), 1);
            }
            _ => panic!("Expected Expr::If, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_if_else_if() {
        let mut parser = Parser::new("if false { 1 } else if true { 2 } else { 3 }");
        let expr = parser.parse_expr().unwrap();
        match expr.unpack() {
            Expr::If {
                condition,
                then_body,
                else_body,
            } => {
                assert_eq!(condition.unpack(), &Expr::Bool(false));
                assert_eq!(then_body.len(), 1);
                if let Some(else_stmts) = else_body {
                    assert_eq!(else_stmts.len(), 1);
                    if let Stmt::Expr(expr_val) = else_stmts[0].unpack() {
                        if let Expr::If { .. } = expr_val.unpack() {
                            // correct
                        } else {
                            panic!("Expected nested Expr::If");
                        }
                    } else {
                        panic!(
                            "Expected nested Expr::If in else body, got {:?}",
                            else_stmts[0]
                        );
                    }
                } else {
                    panic!("Expected else_body");
                }
            }
            _ => panic!("Expected Expr::If, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_if_no_else() {
        let mut parser = Parser::new("if x { 42 }");
        let expr = parser.parse_expr().unwrap();
        match expr.unpack() {
            Expr::If {
                condition,
                then_body,
                else_body,
            } => {
                assert_eq!(condition.unpack(), &Expr::Ident("x".to_string()));
                assert_eq!(then_body.len(), 1);
                assert!(else_body.is_none());
            }
            _ => panic!("Expected Expr::If, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_while() {
        let mut parser = Parser::new("while x < 10 { x = x + 1 }");
        let stmt = parser.parse_statement().unwrap();
        match stmt.unpack() {
            Stmt::While { condition, body } => {
                assert!(matches!(condition.unpack(), Expr::BinaryOp { .. }));
                assert!(!body.is_empty());
            }
            _ => panic!("Expected Stmt::While, got {:?}", stmt),
        }
    }

    #[test]
    fn test_parse_source_statement() {
        let mut parser = Parser::new(r#"source "config.fsh""#);
        let stmt = parser.parse_statement().unwrap();
        match stmt.unpack() {
            Stmt::Source { path, bash } => {
                assert!(!*bash);
                assert_eq!(
                    path.unpack(),
                    &Expr::String(vec![StringPart::Lit("config.fsh".to_string())])
                );
            }
            _ => panic!("Expected Stmt::Source, got {:?}", stmt),
        }

        let mut parser2 = Parser::new(r#"source --bash "config.sh""#);
        let stmt2 = parser2.parse_statement().unwrap();
        match stmt2.unpack() {
            Stmt::Source { path, bash } => {
                assert!(*bash);
                assert_eq!(
                    path.unpack(),
                    &Expr::String(vec![StringPart::Lit("config.sh".to_string())])
                );
            }
            _ => panic!("Expected Stmt::Source with bash flag, got {:?}", stmt2),
        }
    }

    #[allow(clippy::approx_constant)]
    #[test]
    fn test_parse_number_underscores() {
        let mut parser = Parser::new("1_000_000");
        let expr = parser.parse_expr().unwrap();
        assert_eq!(expr.unpack(), &Expr::Int(1000000));

        let mut parser = Parser::new("3.14_159");
        let expr = parser.parse_expr().unwrap();
        assert_eq!(expr.unpack(), &Expr::Float(3.14159));

        let mut parser = Parser::new("1_2_3_4");
        let expr = parser.parse_expr().unwrap();
        assert_eq!(expr.unpack(), &Expr::Int(1234));

        let mut parser = Parser::new("1_000_");
        assert!(parser.parse_expr().is_err());
    }

    #[test]
    fn test_var_slash_path_combined_into_single_arg() {
        // $HOME/.config/fshell/init.fsh should be parsed as a single
        // interpolated-string argument, not as two separate args.
        let mut p = Parser::new("cat $HOME/.config/fshell/init.fsh");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 1);
                if let PipelineStage::CommandCall { name, args, .. } = &pipeline.stages[0] {
                    assert_eq!(name, "cat");
                    assert_eq!(
                        args.len(),
                        1,
                        "should be 1 arg, got {}: {:?}",
                        args.len(),
                        args
                    );
                    match &args[0] {
                        Expr::String(parts) => {
                            assert_eq!(parts.len(), 2);
                            assert_eq!(
                                parts[0],
                                StringPart::Expr(Box::new(Expr::Variable("HOME".to_string())))
                            );
                            assert_eq!(
                                parts[1],
                                StringPart::Lit("/.config/fshell/init.fsh".to_string())
                            );
                        }
                        other => panic!("Expected Expr::String, got {:?}", other),
                    }
                } else {
                    panic!("Expected PipelineStage::CommandCall");
                }
            } else {
                panic!("Expected Expr::Pipeline");
            }
        } else {
            panic!("Expected Stmt::Expr");
        }
    }

    #[allow(clippy::collapsible_if)]
    #[test]
    fn test_var_not_followed_by_slash_stays_variable() {
        // $HOME without a trailing / should remain Expr::Variable
        let mut p = Parser::new("echo $HOME");
        let stmts = p.parse_statements().unwrap();
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                if let PipelineStage::CommandCall { args, .. } = &pipeline.stages[0] {
                    assert_eq!(args.len(), 1);
                    assert_eq!(args[0], Expr::Variable("HOME".to_string()));
                }
            }
        }
    }

    #[allow(clippy::collapsible_if)]
    #[test]
    fn test_var_dot_member_access_in_args() {
        // $process.spawn in command args should be parsed as a single
        // member-access expression, reflecting fshell's structured data model.
        let mut p = Parser::new("echo $process.spawn");
        let stmts = p.parse_statements().unwrap();
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                if let PipelineStage::CommandCall { args, .. } = &pipeline.stages[0] {
                    assert_eq!(args.len(), 1);
                    assert_eq!(
                        args[0],
                        Expr::MemberAccess {
                            expr: Box::new(Expr::Variable("process".to_string())),
                            member: "spawn".to_string(),
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn test_ls_dash_l_dot_dot() {
        let mut p = Parser::new("ls -l ..");
        match p.parse_statements() {
            Ok(stmts) => {
                println!("Successfully parsed: {:?}", stmts);
                assert_eq!(stmts.len(), 1);
            }
            Err(e) => {
                panic!("Failed to parse: {:?}", e);
            }
        }
    }

    #[test]
    fn test_parse_chmod_plus_x_as_command() {
        // `chmod +x file.sh` must be a command call with a literal "+x" arg,
        // not the arithmetic expression `chmod + x`.
        let mut p = Parser::new("chmod +x start-tModLoader.sh");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1, "expected one statement, got: {:?}", stmts);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 1);
                if let PipelineStage::CommandCall { name, args, .. } = &pipeline.stages[0] {
                    assert_eq!(name, "chmod");
                    assert_eq!(args.len(), 2);
                    assert_eq!(
                        args[0],
                        Expr::String(vec![StringPart::Lit("+x".to_string())])
                    );
                    assert_eq!(
                        args[1],
                        Expr::String(vec![StringPart::Lit("start-tModLoader.sh".to_string())])
                    );
                } else {
                    panic!("Expected CommandCall stage");
                }
            } else {
                panic!("Expected Expr::Pipeline");
            }
        } else {
            panic!("Expected Stmt::Expr");
        }
    }

    #[test]
    fn test_parse_chmod_symbolic_modes() {
        // Symbolic chmod modes like `a+x` and `u+x,g+w` must stay single literal args.
        for input in ["chmod a+x file.sh", "chmod u+x,g+w file.sh"] {
            let mut p = Parser::new(input);
            let stmts = p.parse_statements().unwrap();
            if let Stmt::Expr(expr) = stmts[0].unpack() {
                if let Expr::Pipeline(pipeline) = expr.unpack() {
                    if let PipelineStage::CommandCall { name, args, .. } = &pipeline.stages[0] {
                        assert_eq!(name, "chmod");
                        assert_eq!(args.len(), 2);
                        assert!(matches!(&args[0], Expr::String(parts) if parts.len() == 1));
                    }
                }
            }
        }
    }

    #[test]
    fn test_parse_operator_like_command_args_as_literals() {
        // Operators are ordinary command arguments. Expressions in command
        // arguments must use explicit syntax such as `$((1 + 2))`.
        let mut p = Parser::new("echo 1 + 2");
        let stmts = p.parse_statements().unwrap();
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                if let PipelineStage::CommandCall { name, args, .. } = &pipeline.stages[0] {
                    assert_eq!(name, "echo");
                    assert_eq!(args.len(), 3);
                    assert_eq!(
                        args[1],
                        Expr::String(vec![StringPart::Lit("+".to_string())])
                    );
                }
            }
        }

        // Bare arithmetic still parses as an expression, not a command.
        let mut p2 = Parser::new("a + 1");
        let stmts2 = p2.parse_statements().unwrap();
        if let Stmt::Expr(expr) = stmts2[0].unpack() {
            assert!(matches!(
                expr.unpack(),
                Expr::BinaryOp { op: BinOp::Add, .. }
            ));
        }
    }

    #[test]
    fn test_parse_dash_option_value_as_literal_argument() {
        // A conventional `-` option value must not be parsed as subtraction
        // from the preceding long option.
        let mut p = Parser::new("codesign --force --sign - ~/dev/app.dylib");
        let stmts = p.parse_statements().unwrap();
        let Stmt::Expr(expr) = stmts[0].unpack() else {
            panic!("Expected Stmt::Expr");
        };
        let Expr::Pipeline(pipeline) = expr.unpack() else {
            panic!("Expected Expr::Pipeline");
        };
        let PipelineStage::CommandCall { name, args, .. } = &pipeline.stages[0] else {
            panic!("Expected CommandCall stage");
        };
        assert_eq!(name, "codesign");
        assert_eq!(args.len(), 4);
        assert!(
            args.iter()
                .all(|arg| matches!(arg.unpack(), Expr::String(_)))
        );
        assert_eq!(
            args[2].unpack(),
            &Expr::String(vec![StringPart::Lit("-".into())])
        );
    }

    #[test]
    fn test_parse_absolute_path_command() {
        let mut p = Parser::new("/bin/ls");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 1);
                if let PipelineStage::CommandCall { name, args, .. } = &pipeline.stages[0] {
                    assert_eq!(name, "/bin/ls");
                    assert!(args.is_empty());
                } else {
                    panic!("Expected CommandCall stage");
                }
            } else {
                panic!("Expected Pipeline expression for /bin/ls");
            }
        } else {
            panic!("Expected Stmt::Expr");
        }
    }

    #[test]
    fn test_parse_absolute_path_command_with_args() {
        let mut p = Parser::new("/bin/echo \"hello\"");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 1);
                if let PipelineStage::CommandCall { name, args, .. } = &pipeline.stages[0] {
                    assert_eq!(name, "/bin/echo");
                    assert_eq!(args.len(), 1);
                } else {
                    panic!("Expected CommandCall stage");
                }
            } else {
                panic!("Expected Pipeline expression for /bin/echo");
            }
        } else {
            panic!("Expected Stmt::Expr");
        }
    }

    #[test]
    fn test_parse_relative_path_command() {
        let mut p = Parser::new("./bin/ls");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 1);
                if let PipelineStage::CommandCall { name, args, .. } = &pipeline.stages[0] {
                    assert_eq!(name, "./bin/ls");
                    assert!(args.is_empty());
                } else {
                    panic!("Expected CommandCall stage");
                }
            } else {
                panic!("Expected Pipeline expression for ./bin/ls");
            }
        } else {
            panic!("Expected Stmt::Expr");
        }
    }

    #[test]
    fn test_parse_parent_path_command() {
        let mut p = Parser::new("../bin/ls");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 1);
                if let PipelineStage::CommandCall { name, args, .. } = &pipeline.stages[0] {
                    assert_eq!(name, "../bin/ls");
                    assert!(args.is_empty());
                } else {
                    panic!("Expected CommandCall stage");
                }
            } else {
                panic!("Expected Pipeline expression for ../bin/ls");
            }
        } else {
            panic!("Expected Stmt::Expr");
        }
    }

    #[test]
    fn test_parse_home_path_command_with_args() {
        let mut p = Parser::new("~/bin/tool --help");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 1);
                if let PipelineStage::CommandCall { name, args, .. } = &pipeline.stages[0] {
                    assert_eq!(name, "~/bin/tool");
                    assert_eq!(args.len(), 1);
                } else {
                    panic!("Expected CommandCall stage");
                }
            } else {
                panic!("Expected Pipeline expression for ~/bin/tool");
            }
        } else {
            panic!("Expected Stmt::Expr");
        }
    }

    #[test]
    fn test_parse_plain_expression_unchanged() {
        let mut p = Parser::new("42");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            assert!(!matches!(expr.unpack(), Expr::Pipeline(_)));
        } else {
            panic!("Expected Stmt::Expr");
        }
    }

    #[test]
    fn test_parse_inline_env_var() {
        let mut p = Parser::new("FOO=bar echo test");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 1);
                if let PipelineStage::CommandCall {
                    name, args, env, ..
                } = &pipeline.stages[0]
                {
                    assert_eq!(name, "echo");
                    assert_eq!(args.len(), 1);
                    assert_eq!(env.len(), 1);
                    assert_eq!(env[0].0, "FOO");
                } else {
                    panic!("Expected CommandCall stage");
                }
            } else {
                panic!("Expected Pipeline expression");
            }
        } else {
            panic!("Expected Stmt::Expr");
        }
    }

    #[test]
    fn test_parse_inline_env_var_with_path_command() {
        let mut p = Parser::new("BAR=baz /tmp/test.sh");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 1);
                if let PipelineStage::CommandCall {
                    name, args, env, ..
                } = &pipeline.stages[0]
                {
                    assert_eq!(name, "/tmp/test.sh");
                    assert!(args.is_empty());
                    assert_eq!(env.len(), 1);
                    assert_eq!(env[0].0, "BAR");
                } else {
                    panic!("Expected CommandCall stage");
                }
            } else {
                panic!("Expected Pipeline expression");
            }
        } else {
            panic!("Expected Stmt::Expr");
        }
    }

    #[test]
    fn test_parse_multiple_inline_env_vars() {
        let mut p = Parser::new("A=1 B=2 C=3 echo test");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 1);
                if let PipelineStage::CommandCall {
                    name, args, env, ..
                } = &pipeline.stages[0]
                {
                    assert_eq!(name, "echo");
                    assert_eq!(args.len(), 1);
                    assert_eq!(env.len(), 3);
                    assert_eq!(env[0].0, "A");
                    assert_eq!(env[1].0, "B");
                    assert_eq!(env[2].0, "C");
                } else {
                    panic!("Expected CommandCall stage");
                }
            } else {
                panic!("Expected Pipeline expression");
            }
        } else {
            panic!("Expected Stmt::Expr");
        }
    }

    #[test]
    fn test_parse_regular_assignment_still_works() {
        let mut p = Parser::new("FOO = bar");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Assign { name, .. } = stmts[0].unpack() {
            assert_eq!(name, "FOO");
        } else {
            panic!("Expected Stmt::Assign");
        }
    }

    #[test]
    fn test_parse_inline_pipeline() {
        let mut p = Parser::new("let x = $| ls |");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Let { expr, .. } = stmts[0].unpack() {
            assert!(matches!(expr.unpack(), Expr::InlinePipeline(_)));
        } else {
            panic!("expected Let statement with InlinePipeline");
        }
    }

    #[test]
    fn test_parse_inline_pipeline_multi_stage() {
        let mut p = Parser::new("let x = $| ls | count |");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Let { expr, .. } = stmts[0].unpack() {
            assert!(matches!(expr.unpack(), Expr::InlinePipeline(_)));
        } else {
            panic!("expected Let with InlinePipeline");
        }
    }

    #[test]
    fn test_parse_backtick_substitution() {
        let mut p = Parser::new("let x = `ls | count`");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Let { expr, .. } = stmts[0].unpack() {
            assert!(matches!(expr.unpack(), Expr::InlinePipeline(_)));
        } else {
            panic!("expected Let with InlinePipeline from backtick");
        }
    }

    #[test]
    fn test_parse_backtick_simple_cmd() {
        let mut p = Parser::new("echo `ls`");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_parse_cmd_substitution() {
        let mut p = Parser::new(r#"echo "found $(ls) files""#);
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);

        let mut p = Parser::new("let x = $(ls | count)");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Let { expr, .. } = stmts[0].unpack() {
            assert!(matches!(expr.unpack(), Expr::InlinePipeline(_)));
        } else {
            panic!("expected Let with InlinePipeline");
        }
    }

    #[test]
    fn test_parse_on_block() {
        let mut p = Parser::new("on exit { echo cleanup }");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        assert!(
            matches!(stmts[0].unpack(), Stmt::On { signal, handler: OnHandler::Block(_) } if signal == "exit")
        );
    }

    #[test]
    fn test_parse_on_fun() {
        let mut p = Parser::new("on sigint handler_fn");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        assert!(
            matches!(stmts[0].unpack(), Stmt::On { signal, handler: OnHandler::FunctionName(_) } if signal == "sigint")
        );
    }

    #[test]
    fn test_parse_exit() {
        let mut p = Parser::new("exit");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        assert!(matches!(stmts[0].unpack(), Stmt::Exit(None)));

        let mut p = Parser::new("exit 1");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Exit(Some(expr)) = stmts[0].unpack() {
            assert!(matches!(expr.unpack(), Expr::Int(1)));
        } else {
            panic!("expected Exit(Some(Int(1))), got {:#?}", stmts[0].unpack());
        }

        let mut p = Parser::new("exit $code + 1");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        assert!(matches!(stmts[0].unpack(), Stmt::Exit(Some(_))));
    }

    #[test]
    fn test_triple_quoted_string() {
        let stmts = Parser::new(
            r#"
        let x = """
            hello
            world
            """
        "#,
        )
        .parse_statements()
        .unwrap();
        assert_eq!(stmts.len(), 1);
        assert!(matches!(stmts[0].unpack(), Stmt::Let { .. }));
    }

    #[test]
    fn test_dedent_basic() {
        let input = "    hello\n    world";
        let result = dedent(input, DedentMode::All);
        assert_eq!(result, "hello\nworld");
    }

    #[test]
    fn test_dedent_no_dedent() {
        let input = "hello\nworld";
        let result = dedent(input, DedentMode::All);
        assert_eq!(result, "hello\nworld");
    }

    #[test]
    fn test_dedent_empty_lines() {
        let input = "    hello\n\n    world";
        let result = dedent(input, DedentMode::All);
        assert_eq!(result, "hello\n\nworld");
    }

    #[test]
    fn test_dedent_leading_tabs() {
        let input = "\thello\n\tworld";
        let result = dedent(input, DedentMode::LeadingTabs);
        assert_eq!(result, "hello\nworld");
    }

    #[test]
    fn test_parse_string_parts_no_interp() {
        let parts = parse_string_parts("hello world", SourceSpan::new(0.into(), 0)).unwrap();
        assert_eq!(parts.len(), 1);
        assert!(matches!(&parts[0], StringPart::Lit(s) if s == "hello world"));
    }

    #[test]
    fn test_parse_string_parts_var_interp() {
        let parts = parse_string_parts("hello $name", SourceSpan::new(0.into(), 0)).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], StringPart::Lit(s) if s == "hello "));
        assert!(matches!(&parts[1], StringPart::Expr(_)));
    }

    #[test]
    fn test_parse_string_parts_dollar_interp() {
        // `${...}` uses the parameter-expansion grammar, exactly like a
        // double-quoted string.
        let parts = parse_string_parts("count: ${name}", SourceSpan::new(0.into(), 0)).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], StringPart::Lit(s) if s == "count: "));
        assert!(matches!(&parts[1], StringPart::Expr(_)));

        // Modifiers like `${path:t}` must expand, not be parsed as a generic
        // expression (which used to fail on the `:t`).
        let modifier = parse_string_parts("${path:t}", SourceSpan::new(0.into(), 0)).unwrap();
        assert_eq!(modifier.len(), 1);
        assert!(matches!(
            &modifier[0],
            StringPart::Expr(e)
                if matches!(
                    e.unpack(),
                    Expr::VarWithModifier {
                        modifier: ParamModifier::Tail,
                        ..
                    }
                )
        ));

        // Special parameters and command substitution also work in text bodies.
        let special = parse_string_parts("argv: $@", SourceSpan::new(0.into(), 0)).unwrap();
        assert_eq!(special.len(), 2);
        assert!(matches!(
            &special[1],
            StringPart::Expr(e) if matches!(e.unpack(), Expr::Variable(name) if name == "@")
        ));

        // Bare braces stay literal text (nginx/JSON bodies).
        let literal_brace =
            parse_string_parts("server { listen 80; }", SourceSpan::new(0.into(), 0)).unwrap();
        assert_eq!(literal_brace.len(), 1);
        assert!(matches!(&literal_brace[0], StringPart::Lit(s) if s == "server { listen 80; }"));
    }

    #[test]
    fn test_double_quoted_command_and_arithmetic_expansions() {
        let stmts = Parser::new(r#"echo "before $(echo hi) $((1 + 2)) after""#)
            .parse_statements()
            .unwrap();
        let Stmt::Expr(expr) = stmts[0].unpack() else {
            panic!("expected expression statement");
        };
        let Expr::Pipeline(pipeline) = expr.unpack() else {
            panic!("expected pipeline");
        };
        let PipelineStage::CommandCall { args, .. } = &pipeline.stages[0] else {
            panic!("expected command call");
        };
        let Expr::String(parts) = args[0].unpack() else {
            panic!("expected interpolated string");
        };
        assert!(matches!(&parts[0], StringPart::Lit(value) if value == "before "));
        assert!(matches!(
            &parts[1],
            StringPart::Expr(expr) if matches!(expr.unpack(), Expr::InlinePipeline(_))
        ));
        assert!(matches!(&parts[2], StringPart::Lit(value) if value == " "));
        assert!(matches!(
            &parts[3],
            StringPart::Expr(expr) if matches!(expr.unpack(), Expr::ArithmeticExpansion(_))
        ));
        assert!(matches!(&parts[4], StringPart::Lit(value) if value == " after"));
    }

    #[test]
    fn test_heredoc_basic() {
        let stmts = Parser::new("let x = <<EOF\nhello world\nEOF\n")
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Let { name, expr } = stmts[0].unpack() {
            assert_eq!(name, "x");
            assert!(matches!(expr.unpack(), Expr::MultiLineString { .. }));
        } else {
            panic!("Expected Stmt::Let");
        }
    }

    #[test]
    fn test_heredoc_no_interpolation() {
        let stmts = Parser::new("let x = <<'EOF'\nhello $world\nEOF\n")
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Let { expr, .. } = stmts[0].unpack() {
            assert!(matches!(expr.unpack(), Expr::RawMultiLineString(_)));
        } else {
            panic!("Expected Stmt::Let");
        }
    }

    #[test]
    fn test_heredoc_as_command_arg() {
        let stmts = Parser::new("cat <<EOF\nhello\nEOF\n")
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_heredoc_tab_strip() {
        let stmts = Parser::new("cat <<-EOF\n\thello\n\tEOF\n")
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_background_single_ampersand() {
        let stmts = Parser::new("sleep 1 &").parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        assert!(matches!(stmts[0].unpack(), Stmt::Background(_)));
    }

    #[test]
    fn test_background_let() {
        let stmts = Parser::new("let x = 42 &").parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        assert!(matches!(stmts[0].unpack(), Stmt::Background(_)));
    }

    #[test]
    fn test_background_not_confused_with_and() {
        let stmts = Parser::new("true && false").parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        // Should be Stmt::And, not background
    }

    #[test]
    fn test_background_then_another_stmt() {
        let stmts = Parser::new("sleep 1 & echo done")
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 2);
        assert!(matches!(stmts[0].unpack(), Stmt::Background(_)));
    }

    #[test]
    fn test_ansi_c_quote_newline() {
        let stmts = Parser::new(r#"let x = $'hello\nworld'"#)
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Let { expr, .. } = stmts[0].unpack() {
            if let Expr::AnsiCQuote(s) = expr.unpack() {
                assert_eq!(s, "hello\nworld");
            } else {
                panic!("Expected AnsiCQuote");
            }
        } else {
            panic!("Expected Stmt::Let");
        }
    }

    #[test]
    fn test_ansi_c_quote_tab() {
        let stmts = Parser::new(r#"$'col1\tcol2'"#).parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::AnsiCQuote(s) = expr.unpack() {
                assert_eq!(s, "col1\tcol2");
            } else {
                panic!("Expected AnsiCQuote");
            }
        }
    }

    #[test]
    fn test_ansi_c_quote_literal_backslash() {
        let stmts = Parser::new(r#"$'path\\to\\file'"#)
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::AnsiCQuote(s) = expr.unpack() {
                assert_eq!(s, r"path\to\file");
            }
        }
    }

    #[test]
    fn test_brace_expansion_simple() {
        let stmts = Parser::new("echo {a,b,c}").parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(p) = expr.unpack() {
                assert_eq!(p.stages.len(), 1);
                if let PipelineStage::CommandCall { name, args, .. } = &p.stages[0] {
                    assert_eq!(name, "echo");
                    assert_eq!(args.len(), 3);
                    assert_eq!(
                        args[0],
                        Expr::String(vec![StringPart::Lit("a".to_string())])
                    );
                    assert_eq!(
                        args[1],
                        Expr::String(vec![StringPart::Lit("b".to_string())])
                    );
                    assert_eq!(
                        args[2],
                        Expr::String(vec![StringPart::Lit("c".to_string())])
                    );
                } else {
                    panic!("Expected CommandCall");
                }
            } else {
                panic!("Expected Pipeline");
            }
        } else {
            panic!("Expected Expr");
        }
    }

    #[test]
    fn test_brace_expansion_with_prefix() {
        let stmts = Parser::new("echo file.{txt,bak}")
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(p) = expr.unpack() {
                if let PipelineStage::CommandCall { name, args, .. } = &p.stages[0] {
                    assert_eq!(name, "echo");
                    assert_eq!(args.len(), 2);
                    assert_eq!(
                        args[0],
                        Expr::String(vec![StringPart::Lit("file.txt".to_string())])
                    );
                    assert_eq!(
                        args[1],
                        Expr::String(vec![StringPart::Lit("file.bak".to_string())])
                    );
                }
            }
        }
    }

    #[test]
    fn test_brace_expansion_range() {
        let stmts = Parser::new("echo {1..3}").parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(p) = expr.unpack() {
                if let PipelineStage::CommandCall { name, args, .. } = &p.stages[0] {
                    assert_eq!(name, "echo");
                    assert_eq!(args.len(), 3);
                    assert_eq!(
                        args[0],
                        Expr::String(vec![StringPart::Lit("1".to_string())])
                    );
                    assert_eq!(
                        args[1],
                        Expr::String(vec![StringPart::Lit("2".to_string())])
                    );
                    assert_eq!(
                        args[2],
                        Expr::String(vec![StringPart::Lit("3".to_string())])
                    );
                }
            }
        }
    }

    #[test]
    fn test_brace_map_literal_still_works() {
        let stmts = Parser::new("echo {x: 1}").parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(p) = expr.unpack() {
                if let PipelineStage::CommandCall { name, args, .. } = &p.stages[0] {
                    assert_eq!(name, "echo");
                    assert_eq!(args.len(), 1);
                    assert!(matches!(args[0].unpack(), Expr::Map(_)));
                }
            }
        }
    }

    #[test]
    fn test_process_substitution() {
        let stmts = Parser::new("diff <(sort a) <(sort b)")
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(p) = expr.unpack() {
                if let PipelineStage::CommandCall { name, args, .. } = &p.stages[0] {
                    assert_eq!(name, "diff");
                    assert_eq!(args.len(), 2);
                    assert!(matches!(
                        args[0].unpack(),
                        Expr::ProcessSubst {
                            direction: ProcessSubstDirection::Input,
                            ..
                        }
                    ));
                    assert!(matches!(
                        args[1].unpack(),
                        Expr::ProcessSubst {
                            direction: ProcessSubstDirection::Input,
                            ..
                        }
                    ));
                }
            }
        }
    }

    #[test]
    fn test_param_expansion_tail() {
        let stmts = Parser::new(r#"let x = ${path:t}"#)
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Let { name, expr } = stmts[0].unpack() {
            assert_eq!(name, "x");
            assert!(matches!(
                expr.unpack(),
                Expr::VarWithModifier {
                    name: _,
                    modifier: ParamModifier::Tail
                }
            ));
        }
    }

    #[test]
    fn test_param_expansion_head() {
        let stmts = Parser::new(r#"let x = ${path:h}"#)
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Let { name, expr } = stmts[0].unpack() {
            assert_eq!(name, "x");
            match expr.unpack() {
                Expr::VarWithModifier {
                    name: _,
                    modifier: ParamModifier::Head,
                } => {}
                other => panic!("Expected VarWithModifier with Head, got {:?}", other),
            }
        }
    }

    #[test]
    fn test_param_expansion_root() {
        let stmts = Parser::new(r#"let x = ${path:r}"#)
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
        match stmts[0].unpack() {
            Stmt::Let { expr, .. } => match expr.unpack() {
                Expr::VarWithModifier {
                    modifier: ParamModifier::Root,
                    ..
                } => {}
                other => panic!("Expected VarWithModifier Root, got {:?}", other),
            },
            other => panic!("Expected Let, got {:?}", other),
        }
    }

    #[test]
    fn test_param_expansion_ext() {
        let stmts = Parser::new(r#"let x = ${path:e}"#)
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
        match stmts[0].unpack() {
            Stmt::Let { expr, .. } => match expr.unpack() {
                Expr::VarWithModifier {
                    modifier: ParamModifier::Ext,
                    ..
                } => {}
                other => panic!("Expected VarWithModifier Ext, got {:?}", other),
            },
            other => panic!("Expected Let, got {:?}", other),
        }
    }

    #[test]
    fn test_here_string() {
        let stmts = Parser::new(r#"grep "hello" <<< "hello world""#)
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_braced_variable_without_modifier() {
        let stmts = Parser::new(r#"echo ${var}"#).parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(p) = expr.unpack() {
                if let PipelineStage::CommandCall { args, .. } = &p.stages[0] {
                    assert!(matches!(args[0].unpack(), Expr::Variable(_)));
                }
            }
        }
    }

    #[test]
    fn test_shebang_skipped() {
        let stmts = Parser::new("#!/usr/bin/env fsh\nlet x = 42")
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_shebang_with_blank_line() {
        let stmts = Parser::new("#!/usr/bin/env fsh\n\nlet x = 42")
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_exit_code_parsing() {
        // Test $? variable parsing
        let stmts = Parser::new("echo $?").parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(p) = expr.unpack() {
                if let PipelineStage::CommandCall { args, .. } = &p.stages[0] {
                    assert_eq!(args[0].unpack(), &Expr::Variable("?".to_string()));
                } else {
                    panic!("Expected CommandCall");
                }
            } else {
                panic!("Expected Pipeline");
            }
        } else {
            panic!("Expected Expr");
        }

        // Test ? in expression context
        let stmts2 = Parser::new("let val = ?").parse_statements().unwrap();
        assert_eq!(stmts2.len(), 1);
        if let Stmt::Let { expr, .. } = stmts2[0].unpack() {
            assert_eq!(expr.unpack(), &Expr::Ident("?".to_string()));
        } else {
            panic!("Expected Let");
        }

        // Test $? inside double quoted string interpolation
        let stmts3 = Parser::new("echo \"exit: $?\"").parse_statements().unwrap();
        assert_eq!(stmts3.len(), 1);
        if let Stmt::Expr(expr) = stmts3[0].unpack() {
            if let Expr::Pipeline(p) = expr.unpack() {
                if let PipelineStage::CommandCall { args, .. } = &p.stages[0] {
                    if let Expr::String(parts) = args[0].unpack() {
                        assert_eq!(parts.len(), 2);
                        assert_eq!(parts[0], StringPart::Lit("exit: ".to_string()));
                        if let StringPart::Expr(inner_expr) = &parts[1] {
                            assert_eq!(inner_expr.unpack(), &Expr::Variable("?".to_string()));
                        } else {
                            panic!("Expected StringPart::Expr");
                        }
                    } else {
                        panic!("Expected String expr");
                    }
                }
            }
        }

        // Test {?} inside double quoted string interpolation
        let stmts4 = Parser::new("echo \"exit: {?}\"")
            .parse_statements()
            .unwrap();
        assert_eq!(stmts4.len(), 1);
        if let Stmt::Expr(expr) = stmts4[0].unpack() {
            if let Expr::Pipeline(p) = expr.unpack() {
                if let PipelineStage::CommandCall { args, .. } = &p.stages[0] {
                    if let Expr::String(parts) = args[0].unpack() {
                        assert_eq!(parts.len(), 2);
                        assert_eq!(parts[0], StringPart::Lit("exit: ".to_string()));
                        if let StringPart::Expr(inner_expr) = &parts[1] {
                            assert_eq!(inner_expr.unpack(), &Expr::Ident("?".to_string()));
                        } else {
                            panic!("Expected StringPart::Expr");
                        }
                    } else {
                        panic!("Expected String expr");
                    }
                }
            }
        }
    }

    #[test]
    fn parse_default_modifier() {
        let expr = Parser::new("${var:-fallback}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::Default(_),
            } => {
                assert_eq!(name, "var");
            }
            _ => panic!("Expected VarWithModifier with Default, got {:?}", expr),
        }
    }

    #[test]
    fn parse_assign_default_modifier() {
        let expr = Parser::new("${var:=value}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::AssignDefault(_),
            } => {
                assert_eq!(name, "var");
            }
            _ => panic!(
                "Expected VarWithModifier with AssignDefault, got {:?}",
                expr
            ),
        }
    }

    #[test]
    fn parse_error_if_unset_modifier() {
        let expr = Parser::new("${var:?not set}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::ErrorIfUnset(_),
            } => {
                assert_eq!(name, "var");
            }
            _ => panic!("Expected VarWithModifier with ErrorIfUnset, got {:?}", expr),
        }
    }

    #[test]
    fn parse_alternate_modifier() {
        let expr = Parser::new("${var:+has value}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::Alternate(_),
            } => {
                assert_eq!(name, "var");
            }
            _ => panic!("Expected VarWithModifier with Alternate, got {:?}", expr),
        }
    }

    #[test]
    fn parse_substring_modifier() {
        let expr = Parser::new("${var:2:5}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::Substring { offset, length },
            } => {
                assert_eq!(name, "var");
                assert_eq!(*offset, 2i64);
                assert_eq!(*length, Some(5u64));
            }
            _ => panic!("Expected VarWithModifier with Substring, got {:?}", expr),
        }
    }

    #[test]
    fn parse_substring_offset_only() {
        let expr = Parser::new("${var:2}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::Substring { offset, length },
            } => {
                assert_eq!(name, "var");
                assert_eq!(*offset, 2i64);
                assert_eq!(*length, None);
            }
            _ => panic!("Expected VarWithModifier with Substring, got {:?}", expr),
        }
    }

    #[test]
    fn parse_shortest_prefix() {
        let expr = Parser::new("${var#*/}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::ShortestPrefix(_),
            } => {
                assert_eq!(name, "var");
            }
            _ => panic!(
                "Expected VarWithModifier with ShortestPrefix, got {:?}",
                expr
            ),
        }
    }

    #[test]
    fn parse_longest_prefix() {
        let expr = Parser::new("${var##*/}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::LongestPrefix(_),
            } => {
                assert_eq!(name, "var");
            }
            _ => panic!(
                "Expected VarWithModifier with LongestPrefix, got {:?}",
                expr
            ),
        }
    }

    #[test]
    fn parse_shortest_suffix() {
        let expr = Parser::new("${var%.*}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::ShortestSuffix(_),
            } => {
                assert_eq!(name, "var");
            }
            _ => panic!(
                "Expected VarWithModifier with ShortestSuffix, got {:?}",
                expr
            ),
        }
    }

    #[test]
    fn parse_longest_suffix() {
        let expr = Parser::new("${var%%.*}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::LongestSuffix(_),
            } => {
                assert_eq!(name, "var");
            }
            _ => panic!(
                "Expected VarWithModifier with LongestSuffix, got {:?}",
                expr
            ),
        }
    }

    #[test]
    fn parse_replace_first() {
        let expr = Parser::new("${var/a/b}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::Replace { global, .. },
            } => {
                assert_eq!(name, "var");
                assert!(!global);
            }
            _ => panic!("Expected VarWithModifier with Replace, got {:?}", expr),
        }
    }

    #[test]
    fn parse_replace_all() {
        let expr = Parser::new("${var//a/b}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::Replace { global, .. },
            } => {
                assert_eq!(name, "var");
                assert!(global);
            }
            _ => panic!("Expected VarWithModifier with Replace, got {:?}", expr),
        }
    }

    #[test]
    fn parse_string_length() {
        let expr = Parser::new("${#var}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::StringLength,
            } => {
                assert_eq!(name, "var");
            }
            _ => panic!("Expected VarWithModifier with StringLength, got {:?}", expr),
        }
    }

    #[test]
    fn parse_legacy_tail_still_works() {
        let expr = Parser::new("${var:t}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::Tail,
            } => {
                assert_eq!(name, "var");
            }
            _ => panic!("Expected VarWithModifier with Tail, got {:?}", expr),
        }
    }

    #[test]
    fn parse_negative_substring() {
        // A space after `:` marks a signed substring offset (`${var: -3:5}`).
        let expr = Parser::new("${var: -3:5}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::Substring { offset, length },
            } => {
                assert_eq!(name, "var");
                assert_eq!(*offset, -3i64);
                assert_eq!(*length, Some(5u64));
            }
            _ => panic!("Expected VarWithModifier with Substring, got {:?}", expr),
        }
    }

    #[test]
    fn parse_double_colon_dash_is_default() {
        // `${var:-N}` is the POSIX default form, even for a numeric word.
        let expr = Parser::new("${var:-3}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::Default(_),
            } => assert_eq!(name, "var"),
            _ => panic!("Expected Default modifier, got {:?}", expr),
        }
    }

    #[test]
    fn parse_arithmetic_expansion() {
        let expr = Parser::new("$((1 + 2))").parse_expr().unwrap();
        match expr.unpack() {
            Expr::ArithmeticExpansion(inner) => {
                assert!(matches!(inner.unpack(), Expr::BinaryOp { .. }));
            }
            _ => panic!("Expected ArithmeticExpansion, got {:?}", expr),
        }
    }

    #[test]
    fn parse_arithmetic_with_variable() {
        let expr = Parser::new("$((x * 2))").parse_expr().unwrap();
        match expr.unpack() {
            Expr::ArithmeticExpansion(inner) => {
                assert!(matches!(inner.unpack(), Expr::BinaryOp { .. }));
            }
            _ => panic!("Expected ArithmeticExpansion, got {:?}", expr),
        }
    }

    #[test]
    fn parse_arithmetic_nested() {
        let expr = Parser::new("$(((2 + 3) * 4))").parse_expr().unwrap();
        match expr.unpack() {
            Expr::ArithmeticExpansion(inner) => {
                assert!(matches!(inner.unpack(), Expr::BinaryOp { .. }));
            }
            _ => panic!("Expected ArithmeticExpansion, got {:?}", expr),
        }
    }

    #[test]
    fn parse_arithmetic_with_spaces() {
        // Space between (( and inner expr should be handled
        let expr = Parser::new("$(( 1 + 2 ))").parse_expr().unwrap();
        match expr.unpack() {
            Expr::ArithmeticExpansion(inner) => {
                assert!(matches!(inner.unpack(), Expr::BinaryOp { .. }));
            }
            _ => panic!("Expected ArithmeticExpansion, got {:?}", expr),
        }
    }

    #[test]
    fn parse_arithmetic_nested_with_spaces() {
        // Parenthesized sub-expression with space after ((
        let expr = Parser::new("$(( ($x + 5) * 2 ))").parse_expr().unwrap();
        match expr.unpack() {
            Expr::ArithmeticExpansion(inner) => {
                assert!(matches!(inner.unpack(), Expr::BinaryOp { .. }));
            }
            _ => panic!("Expected ArithmeticExpansion, got {:?}", expr),
        }
    }

    #[test]
    fn parse_cmd_substitution_still_works() {
        let stmts = Parser::new("echo $(pwd)").parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_brace_expansion_regression() {
        // Map literal vs brace expansion
        let stmts = Parser::new("echo {x: 1}").parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(p) = expr.unpack() {
                assert!(matches!(p.stages[0], PipelineStage::CommandCall { .. }));
                if let PipelineStage::CommandCall { args, .. } = &p.stages[0] {
                    assert!(matches!(args[0].unpack(), Expr::Map(_)));
                }
            }
        }

        // Brace expansion range
        let stmts2 = Parser::new("echo {a..z}").parse_statements().unwrap();
        assert_eq!(stmts2.len(), 1);

        // Brace expansion list
        let stmts3 = Parser::new("echo {x,y}").parse_statements().unwrap();
        assert_eq!(stmts3.len(), 1);
    }

    #[test]
    fn test_npm_scope_package_arg() {
        // `npm install -g @opencode-ai/cli@next` — scope specifier with `@`
        // must parse as a single literal argument, not a parse error.
        let mut p = Parser::new("npm install -g @opencode-ai/cli@next");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1, "expected one statement, got: {:?}", stmts);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 1);
                if let PipelineStage::CommandCall { name, args, .. } = &pipeline.stages[0] {
                    assert_eq!(name, "npm");
                    assert_eq!(args.len(), 3);
                    assert_eq!(
                        args[2],
                        Expr::String(vec![StringPart::Lit("@opencode-ai/cli@next".to_string())])
                    );
                    return;
                }
            }
        }
        panic!("Expected CommandCall stage, got {:?}", stmts);
    }

    #[test]
    fn test_at_prefix_args_are_literal_strings() {
        // `echo @json` passes a literal "@json" to the command — boundary
        // operators are only recognized at the start of a pipeline stage.
        let mut p = Parser::new("echo @json");
        let stmts = p.parse_statements().unwrap();
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                if let PipelineStage::CommandCall { args, .. } = &pipeline.stages[0] {
                    assert_eq!(args.len(), 1);
                    assert_eq!(
                        args[0],
                        Expr::String(vec![StringPart::Lit("@json".to_string())])
                    );
                    return;
                }
            }
        }
        panic!("Expected CommandCall stage, got {:?}", stmts);
    }

    #[test]
    fn test_scp_user_host_arg_stays_single_token() {
        // `scp user@host:path` — the `@` mid-token must not split the arg.
        let mut p = Parser::new("scp user@github.com:notes.txt ./");
        let stmts = p.parse_statements().unwrap();
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                if let PipelineStage::CommandCall { name, args, .. } = &pipeline.stages[0] {
                    assert_eq!(name, "scp");
                    assert_eq!(args.len(), 2);
                    assert_eq!(
                        args[0],
                        Expr::String(vec![StringPart::Lit(
                            "user@github.com:notes.txt".to_string()
                        )])
                    );
                    return;
                }
            }
        }
        panic!("Expected CommandCall stage, got {:?}", stmts);
    }

    #[test]
    fn test_boundary_operator_still_parses_at_stage_start() {
        // `@` as a boundary serialization operator at the start of a pipeline
        // stage must be unaffected by the command-arg literal handling.
        let mut p = Parser::new("ls | @json");
        let stmts = p.parse_statements().unwrap();
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 2);
                assert!(matches!(
                    pipeline.stages[1],
                    PipelineStage::BoundaryOperator {
                        format: SerializationFormat::Json
                    }
                ));
                return;
            }
        }
        panic!("Expected Pipeline with boundary operator, got {:?}", stmts);
    }

    /// Parse `echo <input>` and assert the resulting CommandCall args are
    /// exactly the expected literal strings (never a parse error).
    fn assert_echo_args(input: &str, expected: &[&str]) {
        let mut p = Parser::new(&format!("echo {input}"));
        let stmts = p
            .parse_statements()
            .unwrap_or_else(|e| panic!("`echo {input}` failed to parse: {e}"));
        assert_eq!(stmts.len(), 1, "`echo {input}`: expected 1 statement");
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 1);
                if let PipelineStage::CommandCall { args, .. } = &pipeline.stages[0] {
                    let got: Vec<String> = args
                        .iter()
                        .map(|a| match a.unpack() {
                            Expr::String(parts) if parts.len() == 1 => match &parts[0] {
                                StringPart::Lit(s) => s.clone(),
                                _ => format!("{a:?}"),
                            },
                            Expr::Ident(name) => name.clone(),
                            other => format!("{other:?}"),
                        })
                        .collect();
                    let want: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
                    assert_eq!(got, want, "`echo {input}` args mismatch");
                    return;
                }
            }
        }
        panic!("`echo {input}`: expected CommandCall stage, got {stmts:?}");
    }

    #[test]
    fn test_special_chars_in_command_args_are_literal() {
        // Regression guard: any non-structural character in command-arg
        // position must parse as a literal bare word, never E004 "Expected
        // identifier". See `npm install -g @opencode-ai/cli@next`.
        assert_echo_args(",foo", &[",foo"]);
        assert_echo_args(":foo", &[":foo"]);
        assert_echo_args("a,b", &["a,b"]);
        assert_echo_args("a%b", &["a%b"]);
        assert_echo_args(r"a\b", &[r"a\b"]);
        assert_echo_args("a]b", &["a]b"]);
        assert_echo_args("file~", &["file~"]);
        assert_echo_args("foo~", &["foo~"]);
        assert_echo_args("a~b", &["a~b"]);
        assert_echo_args("a@b", &["a@b"]);
        assert_echo_args("user@host:path", &["user@host:path"]);
        assert_echo_args("@opencode-ai/cli@next", &["@opencode-ai/cli@next"]);
        assert_echo_args("file ~", &["file", "~"]);
        assert_echo_args("a{b}", &["a{b}"]);
        assert_echo_args("~", &["~"]);
    }

    #[test]
    fn test_command_words_do_not_become_operations_or_member_access() {
        assert_echo_args("2026-09-20", &["2026-09-20"]);
        assert_echo_args("1+2", &["1+2"]);
        assert_echo_args("13.32.09.png", &["13.32.09.png"]);
        assert_echo_args("foo.bar", &["foo.bar"]);
        assert_echo_args(r"foo\ bar", &["foo bar"]);
    }

    #[test]
    fn test_escaped_path_with_spaces_is_one_command_argument() {
        let input = r"ls /var/folders/nw/n7d16zv95_9fyvhl5n57v7hh0000gn/T/TemporaryItems/NSIRD_screencaptureui_K3f6Md/Screenshot\ 2026-09-20\ at\ 13.32.09.png";
        let stmts = Parser::new(input).parse_statements().unwrap();
        let Stmt::Expr(expr) = stmts[0].unpack() else {
            panic!("expected command expression");
        };
        let Expr::Pipeline(pipeline) = expr.unpack() else {
            panic!("expected pipeline");
        };
        let PipelineStage::CommandCall { args, .. } = &pipeline.stages[0] else {
            panic!("expected command call");
        };
        assert_eq!(args.len(), 1, "expected one path argument, got {args:?}");
        assert_eq!(
            args[0],
            Expr::String(vec![StringPart::Lit(
                "/var/folders/nw/n7d16zv95_9fyvhl5n57v7hh0000gn/T/TemporaryItems/NSIRD_screencaptureui_K3f6Md/Screenshot 2026-09-20 at 13.32.09.png".to_string()
            )])
        );
    }

    #[test]
    fn test_escaped_redirection_path_is_one_argument() {
        let stmts = Parser::new(r"echo data > /tmp/output\ file.txt")
            .parse_statements()
            .unwrap();
        let Stmt::Expr(expr) = stmts[0].unpack() else {
            panic!("expected command expression");
        };
        let Expr::Pipeline(pipeline) = expr.unpack() else {
            panic!("expected pipeline");
        };
        assert!(pipeline.stages.iter().any(|stage| matches!(
            stage,
            PipelineStage::Write {
                path: Expr::String(parts),
                ..
            } if parts == &vec![StringPart::Lit("/tmp/output file.txt".to_string())]
        )));
    }

    #[test]
    fn test_brace_expansion_adjacent_to_word() {
        // `file{1,2}` expands to file1 file2, like bash — `{` directly after
        // an identifier must be part of the word, not a map-literal separator.
        let mut p = Parser::new("echo file{1,2}");
        let stmts = p.parse_statements().unwrap();
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                if let PipelineStage::CommandCall { args, .. } = &pipeline.stages[0] {
                    assert_eq!(args.len(), 2, "expected 2 expanded args, got {:?}", args);
                    assert_eq!(
                        args[0],
                        Expr::String(vec![StringPart::Lit("file1".to_string())])
                    );
                    assert_eq!(
                        args[1],
                        Expr::String(vec![StringPart::Lit("file2".to_string())])
                    );
                    return;
                }
            }
        }
        panic!("Expected CommandCall stage, got {:?}", stmts);
    }

    #[test]
    fn test_regex_match_operator_still_works_in_expressions() {
        // `~` remains the regex-match operator outside command-arg mode.
        let mut p = Parser::new(r#"let m = "hello" ~ "^h""#);
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Let { expr, .. } = stmts[0].unpack() {
            assert!(matches!(
                expr.unpack(),
                Expr::BinaryOp {
                    op: BinOp::ReMatch,
                    ..
                }
            ));
        } else {
            panic!("Expected Stmt::Let");
        }
    }

    #[test]
    fn test_and_binds_tighter_than_or() {
        // `a or b and c` must parse as `a or (b and c)`, not `(a or b) and c`.
        let mut p = Parser::new("let x = a or b and c");
        let stmts = p.parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        let Stmt::Let { expr, .. } = stmts[0].unpack() else {
            panic!("Expected Stmt::Let");
        };
        match expr.unpack() {
            Expr::BinaryOp {
                op: BinOp::Or,
                lhs,
                rhs,
            } => {
                assert_eq!(lhs.unpack(), &Expr::Ident("a".to_string()));
                assert!(
                    matches!(rhs.unpack(), Expr::BinaryOp { op: BinOp::And, .. }),
                    "expected `and` to bind tighter than `or`, got {:?}",
                    rhs
                );
            }
            other => panic!("Expected `or` at the root, got {:?}", other),
        }
    }

    #[test]
    fn test_comparison_binds_tighter_than_boolean_operators() {
        // `a == 1 and b == 2` must parse as `(a == 1) and (b == 2)`.
        let mut p = Parser::new("let x = a == 1 and b == 2");
        let stmts = p.parse_statements().unwrap();
        let Stmt::Let { expr, .. } = stmts[0].unpack() else {
            panic!("Expected Stmt::Let");
        };
        match expr.unpack() {
            Expr::BinaryOp {
                op: BinOp::And,
                lhs,
                rhs,
            } => {
                assert!(matches!(lhs.unpack(), Expr::BinaryOp { op: BinOp::Eq, .. }));
                assert!(matches!(rhs.unpack(), Expr::BinaryOp { op: BinOp::Eq, .. }));
            }
            other => panic!("Expected `and` at the root, got {:?}", other),
        }
    }

    #[test]
    fn test_logical_ops_are_boolean_in_expression_context() {
        // The documented example must parse as one boolean expression.
        let stmts = Parser::new("let ok = !false && (a or b)")
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
        let Stmt::Let { expr, .. } = stmts[0].unpack() else {
            panic!("Expected Stmt::Let");
        };
        assert!(
            matches!(expr.unpack(), Expr::BinaryOp { op: BinOp::And, .. }),
            "expected `&&` to be a boolean operator, got {:?}",
            expr
        );

        let stmts = Parser::new("let ok = a || b").parse_statements().unwrap();
        let Stmt::Let { expr, .. } = stmts[0].unpack() else {
            panic!("Expected Stmt::Let");
        };
        assert!(matches!(
            expr.unpack(),
            Expr::BinaryOp { op: BinOp::Or, .. }
        ));
    }

    #[test]
    fn test_statement_level_and_chains_on_exit_status() {
        // At statement level `&&`/`||` keep shell exit-status semantics.
        let stmts = Parser::new("cargo build && cargo test")
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
        assert!(matches!(stmts[0].unpack(), Stmt::And(..)));

        let stmts = Parser::new("a || b").parse_statements().unwrap();
        assert!(matches!(stmts[0].unpack(), Stmt::Or(..)));
    }

    #[test]
    fn test_bare_identifier_member_access() {
        // `a.b` in expression position is field access, not the string "a.b".
        let stmts = Parser::new("let x = user.name").parse_statements().unwrap();
        let Stmt::Let { expr, .. } = stmts[0].unpack() else {
            panic!("Expected Stmt::Let");
        };
        match expr.unpack() {
            Expr::MemberAccess { expr, member } => {
                assert_eq!(expr.unpack(), &Expr::Ident("user".to_string()));
                assert_eq!(member, "name");
            }
            other => panic!("Expected MemberAccess, got {other:?}"),
        }
    }

    #[test]
    fn test_error_spans_use_byte_offsets() {
        // `ü` is two bytes (char index 4, byte offset 4, and the `)` after it is
        // char 8 but byte 9). miette spans are byte offsets, so the reported
        // offset must be 9 or diagnostics would mis-point on non-ASCII input.
        let err = Parser::new("let ü = )").parse_statements().unwrap_err();
        assert_eq!(
            err.span().offset(),
            9,
            "spans must be byte offsets, not char indices"
        );
    }

    #[test]
    fn test_process_substitution_with_redirect() {
        // A redirect inside the substitution body must parse.
        let result = Parser::new(r#"diff >(cat > /tmp/out) <(cat)"#).parse_statements();
        assert!(
            result.is_ok(),
            "redirect inside process substitution must parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_raw_string_argument() {
        // `grep r"\.tmp$"` — a raw string in command-argument position.
        let stage = last_stage_of(r#"ls | grep r"\.tmp$""#);
        let PipelineStage::Grep { pattern } = &stage else {
            panic!("Expected Grep stage, got {stage:?}");
        };
        assert_eq!(
            pattern.unpack(),
            &Expr::String(vec![StringPart::Lit(r"\.tmp$".to_string())])
        );
    }

    #[test]
    fn test_command_arg_dotted_path_stays_string() {
        // A dotted token in command-argument position is still a path string.
        let stage = last_stage_of("cat app.toml");
        let PipelineStage::CommandCall { args, .. } = &stage else {
            panic!("Expected CommandCall, got {stage:?}");
        };
        assert_eq!(
            args[0].unpack(),
            &Expr::String(vec![StringPart::Lit("app.toml".to_string())])
        );
    }

    #[test]
    fn test_raw_string_literal() {
        let stmts = Parser::new(r#"let re = r"\.rs$""#)
            .parse_statements()
            .unwrap();
        let Stmt::Let { expr, .. } = stmts[0].unpack() else {
            panic!("Expected Stmt::Let");
        };
        assert_eq!(
            expr.unpack(),
            &Expr::String(vec![StringPart::Lit(r"\.rs$".to_string())])
        );
    }

    #[test]
    fn test_triple_single_quoted_string_is_raw_and_dedented() {
        let stmts = Parser::new("let x = '''\n    hello\n    world\n    '''")
            .parse_statements()
            .unwrap();
        let Stmt::Let { expr, .. } = stmts[0].unpack() else {
            panic!("Expected Stmt::Let");
        };
        match expr.unpack() {
            Expr::MultiLineString { parts, .. } => {
                assert_eq!(parts, &vec![StringPart::Lit("hello\nworld".to_string())]);
            }
            other => panic!("Expected MultiLineString, got {other:?}"),
        }
    }

    #[test]
    fn test_single_quoted_args_are_raw_strings() {
        // Single quotes are raw strings and must not be retyped to Bool/Int/Null
        // the way unquoted bare words are.
        for src in [
            "echo 'true'",
            "echo 'false'",
            "echo 'null'",
            "echo '123'",
            "echo '3.14'",
        ] {
            let stmts = Parser::new(src).parse_statements().unwrap();
            let Stmt::Expr(expr) = stmts[0].unpack() else {
                panic!("{src}: expected expression statement");
            };
            let Expr::Pipeline(pipeline) = expr.unpack() else {
                panic!("{src}: expected pipeline");
            };
            let PipelineStage::CommandCall { args, .. } = &pipeline.stages[0] else {
                panic!("{src}: expected command call");
            };
            assert!(
                matches!(&args[0], Expr::String(parts)
                    if matches!(parts.as_slice(), [StringPart::Lit(_)])),
                "{src}: quoted argument must stay a string, got {:?}",
                args[0]
            );
        }
    }

    fn last_stage_of(src: &str) -> PipelineStage {
        let stmts = Parser::new(src).parse_statements().unwrap();
        let pipeline = match stmts.first().map(|s| s.unpack()) {
            Some(Stmt::Expr(e)) => match e.unpack() {
                Expr::Pipeline(p) => p.clone(),
                other => panic!("{src}: expected pipeline, got {other:?}"),
            },
            other => panic!("{src}: expected expression statement, got {other:?}"),
        };
        pipeline.stages.last().unwrap().clone()
    }

    #[test]
    fn test_pipeline_keyword_with_flag_is_external_command() {
        // Documented rule: a reserved stage keyword immediately followed by a
        // `-flag` is an external command, not the built-in stage.
        for (src, expected) in [
            ("ps | map -x", "map"),
            ("ps | filter -x", "filter"),
            ("ps | count --help", "count"),
            ("ps | limit -1", "limit"),
            ("ps | grep -E", "grep"),
        ] {
            let stage = last_stage_of(src);
            assert!(
                matches!(&stage, PipelineStage::CommandCall { name, .. } if name == expected),
                "{src}: expected external command `{expected}`, got {stage:?}"
            );
        }
    }

    #[test]
    fn test_pipeline_keyword_without_flag_is_stage() {
        assert!(matches!(last_stage_of("ps | count"), PipelineStage::Count));
        assert!(matches!(
            last_stage_of("ps | map pid command"),
            PipelineStage::Map { .. }
        ));
        assert!(matches!(
            last_stage_of("ps | limit 10"),
            PipelineStage::Limit { .. }
        ));
    }

    #[test]
    fn test_quoted_brace_interp_bare_ident_is_variable_ref() {
        // `"a{b}"` — a bare identifier inside `{...}` string interpolation is a
        // variable reference, not a command-call pipeline ("Command not found: b").
        let stmts = Parser::new("echo \"a{b}\"").parse_statements().unwrap();
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                if let PipelineStage::CommandCall { args, .. } = &pipeline.stages[0] {
                    assert_eq!(args.len(), 1);
                    match &args[0] {
                        Expr::String(parts) => {
                            assert_eq!(parts.len(), 2);
                            assert!(matches!(&parts[0], StringPart::Lit(s) if s == "a"));
                            assert!(matches!(
                                &parts[1],
                                StringPart::Expr(e)
                                    if matches!(e.unpack(), Expr::Ident(n) if n == "b")
                            ));
                        }
                        other => panic!("Expected String arg, got {:?}", other),
                    }
                    return;
                }
            }
        }
        panic!("Expected CommandCall stage, got {:?}", stmts);
    }

    #[test]
    fn test_quoted_param_modifiers_in_string() {
        // `${var:+alt}` / `${var:-alt}` inside double quotes parse through the
        // braced-variable grammar (modifiers), not a bare-expression pipeline.
        let stmts = Parser::new("echo \"${var:+alt}\"")
            .parse_statements()
            .unwrap();
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                if let PipelineStage::CommandCall { args, .. } = &pipeline.stages[0] {
                    assert_eq!(args.len(), 1);
                    match &args[0] {
                        Expr::String(parts) => {
                            assert_eq!(parts.len(), 1);
                            assert!(matches!(
                                &parts[0],
                                StringPart::Expr(e)
                                    if matches!(
                                        e.unpack(),
                                        Expr::VarWithModifier {
                                            name,
                                            modifier: ParamModifier::Alternate(_)
                                        } if name == "var"
                                    )
                            ));
                        }
                        other => panic!("Expected String arg, got {:?}", other),
                    }
                } else {
                    panic!("Expected CommandCall stage, got {:?}", pipeline);
                }
            } else {
                panic!("Expected Pipeline expr, got {:?}", expr);
            }
        } else {
            panic!("Expected Expr stmt, got {:?}", stmts);
        }

        let stmts = Parser::new("echo \"${var:-alt}\"")
            .parse_statements()
            .unwrap();
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                if let PipelineStage::CommandCall { args, .. } = &pipeline.stages[0] {
                    match &args[0] {
                        Expr::String(parts) => {
                            assert!(matches!(
                                &parts[0],
                                StringPart::Expr(e)
                                    if matches!(
                                        e.unpack(),
                                        Expr::VarWithModifier {
                                            name,
                                            modifier: ParamModifier::Default(_)
                                        } if name == "var"
                                    )
                            ));
                        }
                        other => panic!("Expected String arg, got {:?}", other),
                    }
                    return;
                }
            }
        }
        panic!("Expected CommandCall stage, got {:?}", stmts);
    }

    #[test]
    fn test_modifier_value_is_literal_not_command() {
        // `${var:+alt}` — the alternate value is a literal word, not a command.
        let expr = Parser::new("${var:+alt}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::Alternate(alt_expr),
            } => {
                assert_eq!(name, "var");
                match alt_expr.unpack() {
                    Expr::String(parts) => {
                        assert_eq!(parts.len(), 1);
                        assert!(matches!(&parts[0], StringPart::Lit(s) if s == "alt"));
                    }
                    other => panic!("Expected literal String value, got {:?}", other),
                }
            }
            _ => panic!("Expected VarWithModifier with Alternate, got {:?}", expr),
        }

        // `${var:-fallback}` — same for the default value.
        let expr = Parser::new("${var:-fallback}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::Default(default_expr),
            } => {
                assert_eq!(name, "var");
                match default_expr.unpack() {
                    Expr::String(parts) => {
                        assert_eq!(parts.len(), 1);
                        assert!(matches!(&parts[0], StringPart::Lit(s) if s == "fallback"));
                    }
                    other => panic!("Expected literal String default, got {:?}", other),
                }
            }
            _ => panic!("Expected VarWithModifier with Default, got {:?}", expr),
        }
    }

    #[test]
    fn test_modifier_value_interpolation() {
        // Modifier values still support $var interpolation: `${var:+$other}`.
        let expr = Parser::new("${var:+$other}").parse_expr().unwrap();
        match expr.unpack() {
            Expr::VarWithModifier {
                name,
                modifier: ParamModifier::Alternate(alt_expr),
            } => {
                assert_eq!(name, "var");
                match alt_expr.unpack() {
                    Expr::String(parts) => {
                        assert_eq!(parts.len(), 1);
                        assert!(matches!(
                            &parts[0],
                            StringPart::Expr(e) if matches!(e.unpack(), Expr::Variable(v) if v == "other")
                        ));
                    }
                    other => panic!("Expected interpolated String value, got {:?}", other),
                }
            }
            _ => panic!("Expected VarWithModifier with Alternate, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_input_redirection() {
        let stmts = Parser::new("cat < input.txt").parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 2);
                assert!(matches!(&pipeline.stages[0], PipelineStage::Read { .. }));
                assert!(matches!(
                    &pipeline.stages[1],
                    PipelineStage::CommandCall { .. }
                ));
                return;
            }
        }
        panic!("Expected pipeline with Read stage, got {:?}", stmts);
    }

    #[test]
    fn test_parse_input_and_output_redirection() {
        let stmts = Parser::new("cat foo < in.txt > out.txt")
            .parse_statements()
            .unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 3);
                assert!(matches!(&pipeline.stages[0], PipelineStage::Read { .. }));
                assert!(matches!(
                    &pipeline.stages[1],
                    PipelineStage::CommandCall { .. }
                ));
                assert!(matches!(&pipeline.stages[2], PipelineStage::Write { .. }));
                return;
            }
        }
        panic!(
            "Expected pipeline with Read, CommandCall, Write stages, got {:?}",
            stmts
        );
    }

    #[test]
    fn test_parse_explicit_fd0_input_redirection() {
        let stmts = Parser::new("cat foo 0< in.txt").parse_statements().unwrap();
        assert_eq!(stmts.len(), 1);
        if let Stmt::Expr(expr) = stmts[0].unpack() {
            if let Expr::Pipeline(pipeline) = expr.unpack() {
                assert_eq!(pipeline.stages.len(), 2);
                assert!(matches!(&pipeline.stages[0], PipelineStage::Read { .. }));
                assert!(matches!(
                    &pipeline.stages[1],
                    PipelineStage::CommandCall { .. }
                ));
                return;
            }
        }
        panic!("Expected pipeline with Read stage, got {:?}", stmts);
    }
}
