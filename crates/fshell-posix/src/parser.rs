// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use brush_parser::{Parser, ParserOptions, ast};

/// Parsed POSIX shell script.
#[derive(Debug)]
pub struct ParsedScript {
    pub program: ast::Program,
    pub source: String,
}

/// Strip a leading `#!` shebang line, which is not part of the grammar.
fn strip_shebang(source: &str) -> &str {
    if let Some(first_nl) = source.find('\n') {
        if source[..first_nl].starts_with("#!") {
            &source[first_nl + 1..]
        } else {
            source
        }
    } else if source.starts_with("#!") {
        ""
    } else {
        source
    }
}

fn posix_parser_options() -> ParserOptions {
    ParserOptions {
        enable_extended_globbing: false,
        posix_mode: true,
        sh_mode: false,
        tilde_expansion_at_word_start: true,
        tilde_expansion_after_colon: true,
        ..Default::default()
    }
}

/// How a POSIX shell source fragment parses.
///
/// `Incomplete` is the signal used to keep an interactive buffer in
/// continuation mode: the fragment is valid so far but the parser ran out of
/// input (unbalanced compound command, unterminated quote/heredoc, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosixSyntax {
    /// Parsed cleanly; the fragment is a complete script.
    Complete,
    /// Valid prefix that needs more input.
    Incomplete,
    /// A definite syntax error that more input cannot fix.
    Invalid,
}

/// A tokenizer error that more input can resolve (unterminated construct).
fn tokenizer_error_is_incomplete(error: &brush_parser::TokenizerError) -> bool {
    use brush_parser::TokenizerError as E;
    matches!(
        error,
        E::UnterminatedEscapeSequence
            | E::UnterminatedSingleQuote(_)
            | E::UnterminatedAnsiCQuote(_)
            | E::UnterminatedDoubleQuote(_)
            | E::UnterminatedBackquote(_)
            | E::UnterminatedExtendedGlob(_)
            | E::UnterminatedVariable
            | E::UnterminatedCommandSubstitution
            | E::UnterminatedExpansion
            | E::MissingHereTagForDocumentBody
            | E::MissingHereTag(_)
            | E::UnterminatedHereDocuments(_, _)
    )
}

/// Classify POSIX shell source without executing it.
///
/// This is the POSIX counterpart to `fshell_core::validate_input`: the engine's
/// unified front-end consults it to decide whether to fall back to the POSIX
/// engine and whether an interactive buffer is complete.
pub fn classify_posix(source: &str) -> PosixSyntax {
    let script = strip_shebang(source);
    let opts = posix_parser_options();
    let reader = std::io::BufReader::new(script.as_bytes());
    let mut parser = Parser::new(reader, &opts);
    match parser.parse_program() {
        Ok(_) => PosixSyntax::Complete,
        Err(brush_parser::ParseError::ParsingAtEndOfInput) => PosixSyntax::Incomplete,
        Err(brush_parser::ParseError::Tokenizing { inner, .. }) => {
            // An unterminated construct is a continuation; anything else is a
            // real error that more input cannot fix.
            if tokenizer_error_is_incomplete(&inner) {
                PosixSyntax::Incomplete
            } else {
                PosixSyntax::Invalid
            }
        }
        Err(brush_parser::ParseError::ParsingNear(_)) => PosixSyntax::Invalid,
    }
}

/// Parse POSIX shell source into a brush-parser AST.
///
/// Uses POSIX-mode tokenization and parsing (no bash extensions beyond
/// what POSIX.1-2024 requires). Shebang lines are stripped.
pub fn parse_posix_script(source: &str) -> Result<ParsedScript, fshell_engine::EngineError> {
    let script = strip_shebang(source);

    let opts = posix_parser_options();

    let reader = std::io::BufReader::new(script.as_bytes());
    let mut parser = Parser::new(reader, &opts);
    let program = parser
        .parse_program()
        .map_err(|e| fshell_engine::EngineError::Generic {
            message: format!("POSIX parse error: {:?}", e),
            span: None,
        })?;

    Ok(ParsedScript {
        program,
        source: script.to_string(),
    })
}

/// Detect a POSIX-like shebang.
pub fn is_posix_shebang(source: &str) -> bool {
    let first_line = source.lines().next().unwrap_or("");
    if let Some(rest) = first_line.strip_prefix("#!") {
        let token = rest.split_whitespace().next().unwrap_or("");
        let name = std::path::Path::new(token)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        matches!(name, "sh" | "bash" | "dash" | "ksh" | "zsh")
            || rest.contains("/env sh")
            || rest.contains("/env bash")
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_command() {
        let ps = parse_posix_script("echo hello").unwrap();
        assert!(!ps.program.complete_commands.is_empty());
    }

    #[test]
    fn test_parse_shebang_stripped() {
        let ps = parse_posix_script("#!/bin/sh\necho hi").unwrap();
        assert!(!ps.program.complete_commands.is_empty());
        assert!(!ps.source.starts_with("#!"));
    }

    #[test]
    fn test_parse_if_else() {
        let ps = parse_posix_script("if true; then echo hi; fi").unwrap();
        assert!(!ps.program.complete_commands.is_empty());
    }

    #[test]
    fn test_parse_for_loop() {
        let ps = parse_posix_script("for i in 1 2 3; do echo $i; done").unwrap();
        assert!(!ps.program.complete_commands.is_empty());
    }

    #[test]
    fn test_parse_while_loop() {
        let ps = parse_posix_script("while true; do echo hi; done").unwrap();
        assert!(!ps.program.complete_commands.is_empty());
    }

    #[test]
    fn test_parse_case() {
        let ps = parse_posix_script("case $x in a) echo a;; b) echo b;; esac").unwrap();
        assert!(!ps.program.complete_commands.is_empty());
    }

    #[test]
    fn test_parse_pipeline() {
        let ps = parse_posix_script("echo hi | cat").unwrap();
        assert!(!ps.program.complete_commands.is_empty());
    }

    #[test]
    fn test_parse_and_or() {
        let ps = parse_posix_script("true && echo ok || echo fail").unwrap();
        assert!(!ps.program.complete_commands.is_empty());
    }

    #[test]
    fn test_parse_subshell() {
        let ps = parse_posix_script("(echo hi)").unwrap();
        assert!(!ps.program.complete_commands.is_empty());
    }

    #[test]
    fn test_shebang_detection() {
        assert!(is_posix_shebang("#!/bin/sh\necho hi"));
        assert!(is_posix_shebang("#!/bin/bash\necho hi"));
        assert!(is_posix_shebang("#!/usr/bin/env bash\necho hi"));
        assert!(!is_posix_shebang("#!/usr/bin/env fsh\necho hi"));
        assert!(!is_posix_shebang("echo hi"));
    }

    #[test]
    fn test_classify_posix() {
        assert_eq!(classify_posix("echo hello"), PosixSyntax::Complete);
        assert_eq!(classify_posix(""), PosixSyntax::Complete);
        assert_eq!(
            classify_posix("for f in a b; do echo $f; done"),
            PosixSyntax::Complete
        );
        assert_eq!(classify_posix("(echo a); (echo b)"), PosixSyntax::Complete);

        // Unbalanced / unterminated constructs request continuation.
        assert_eq!(
            classify_posix("for f in a b; do echo $f"),
            PosixSyntax::Incomplete
        );
        assert_eq!(classify_posix("(echo a"), PosixSyntax::Incomplete);
        assert_eq!(
            classify_posix("echo \"unterminated"),
            PosixSyntax::Incomplete
        );
        assert_eq!(
            classify_posix("if true; then echo hi"),
            PosixSyntax::Incomplete
        );
        assert_eq!(classify_posix("echo 'open"), PosixSyntax::Incomplete);

        // Definite errors.
        assert_eq!(classify_posix(";;;"), PosixSyntax::Invalid);
        assert_eq!(classify_posix("case"), PosixSyntax::Incomplete);
    }

    #[test]
    fn test_parse_empty() {
        let ps = parse_posix_script("").unwrap();
        assert!(ps.program.complete_commands.is_empty());
    }

    #[test]
    fn test_parse_comments_only() {
        let ps = parse_posix_script("# just a comment\n# another").unwrap();
        assert!(ps.program.complete_commands.is_empty());
    }
}
