// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use brush_parser::{Parser, ParserOptions, ast};

/// Parsed POSIX shell script.
#[derive(Debug)]
pub struct ParsedScript {
    pub program: ast::Program,
    pub source: String,
}

/// Parse POSIX shell source into a brush-parser AST.
///
/// Uses POSIX-mode tokenization and parsing (no bash extensions beyond
/// what POSIX.1-2024 requires). Shebang lines are stripped.
pub fn parse_posix_script(source: &str) -> Result<ParsedScript, fshell_engine::EngineError> {
    // Strip leading shebang — not part of the grammar.
    let script = if let Some(first_nl) = source.find('\n') {
        if source[..first_nl].starts_with("#!") {
            &source[first_nl + 1..]
        } else {
            source
        }
    } else if source.starts_with("#!") {
        ""
    } else {
        source
    };

    let opts = ParserOptions {
        enable_extended_globbing: false,
        posix_mode: true,
        sh_mode: false,
        tilde_expansion_at_word_start: true,
        tilde_expansion_after_colon: true,
        ..Default::default()
    };

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
