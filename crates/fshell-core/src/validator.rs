// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::parser::{ParseError, Parser, scan_heredoc_delimiter};
use miette::SourceSpan;

/// Result of validating a shell input buffer for completion status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    /// The input is syntactically complete and ready to execute.
    Complete,
    /// The input is incomplete and requires continuation line(s).
    Incomplete { prompt_hint: &'static str },
    /// The input has a syntax error.
    Invalid { message: String, span: SourceSpan },
}

/// One parser pass over an input buffer: completeness plus routing intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputInspection {
    pub result: ValidationResult,
    /// The parser consumed a construct only the native grammar owns (`let`,
    /// `fn`, `sh { ... }`, ...). Such input must never be reinterpreted as
    /// POSIX just because a POSIX parser accepts the same words.
    pub native_intent: bool,
}

/// Inspect a buffer once for both completeness and routing intent.
pub fn inspect_input(input: &str) -> InputInspection {
    if input.trim().is_empty() {
        return InputInspection {
            result: ValidationResult::Complete,
            native_intent: false,
        };
    }
    let mut parser = Parser::new(input);
    let result = match parser.parse_statements() {
        Ok(_) => ValidationResult::Complete,
        Err(error) => classify_parse_error(input, error),
    };
    InputInspection {
        result,
        native_intent: parser.saw_native_construct(),
    }
}

/// Validates whether the given input text is complete, incomplete, or invalid.
///
/// The parser is the single authority: a successful parse is complete, and an
/// error caused by the input ending inside an open construct is incomplete. No
/// second scanner re-lexes the text to guess what the parser already knows.
pub fn validate_input(input: &str) -> ValidationResult {
    inspect_input(input).result
}

fn classify_parse_error(input: &str, error: ParseError) -> ValidationResult {
    let span = error.span();
    let at_eof = span.offset() >= input.len();
    match error {
        // The parser names the construct it was inside when the input ended.
        ParseError::Incomplete { construct, .. } => ValidationResult::Incomplete {
            prompt_hint: construct,
        },
        ParseError::UnexpectedEof { .. } => ValidationResult::Incomplete {
            prompt_hint: "incomplete",
        },
        ParseError::ExpectedChar {
            expected, found, ..
        } if at_eof || found == '\0' => ValidationResult::Incomplete {
            prompt_hint: char_hint(expected),
        },
        ParseError::ExpectedToken { .. } if at_eof => ValidationResult::Incomplete {
            prompt_hint: "incomplete",
        },
        // Any other syntax error that points at the end of the input is an
        // unterminated construct the parser did not name.
        ParseError::SyntaxError { .. } if at_eof => ValidationResult::Incomplete {
            prompt_hint: "incomplete",
        },
        other => ValidationResult::Invalid {
            message: other.to_string(),
            span: other.span(),
        },
    }
}

fn char_hint(expected: char) -> &'static str {
    match expected {
        '{' | '}' => "brace",
        '(' | ')' => "paren",
        '[' | ']' => "bracket",
        '"' => "dquote",
        '\'' => "squote",
        _ => "incomplete",
    }
}

/// Compute open brace indentation depth up to a specific character index.
pub fn compute_indent_depth_at(input: &str, up_to_char: Option<usize>) -> usize {
    let lex = scan_delimiters(input, up_to_char);
    if lex.unclosed_heredoc {
        0
    } else {
        lex.brace_depth
    }
}

#[derive(Debug)]
struct DelimiterScan {
    unclosed_heredoc: bool,
    brace_depth: usize,
}

fn scan_delimiters(input: &str, up_to_char: Option<usize>) -> DelimiterScan {
    let all_chars: Vec<char> = input.chars().collect();
    let chars: &[char] = match up_to_char {
        Some(limit) => &all_chars[..limit.min(all_chars.len())],
        None => &all_chars,
    };
    let mut i = 0;
    let len = chars.len();

    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_triple_single_quote = false;
    let mut in_triple_double_quote = false;
    let mut in_comment = false;

    let mut brace_depth = 0usize;

    // Heredoc tracking: FIFO queue of (delimiter, strip_tabs)
    let mut heredoc_stack: Vec<(String, bool)> = Vec::new();
    let mut pending_heredocs: Vec<(String, bool)> = Vec::new();
    let mut in_heredoc_body: bool = false;

    while i < len {
        let c = chars[i];

        // If we are scanning a heredoc body line
        if in_heredoc_body {
            if let Some((delim, strip_tabs)) = heredoc_stack.first().cloned() {
                let mut line_end = i;
                while line_end < len && chars[line_end] != '\n' && chars[line_end] != '\r' {
                    line_end += 1;
                }
                let raw_line: String = chars[i..line_end].iter().collect();
                let check_line = if strip_tabs {
                    raw_line.trim_start_matches('\t').to_string()
                } else {
                    raw_line.clone()
                };
                if check_line == delim {
                    // Delimiter matched! Remove from front of queue
                    heredoc_stack.remove(0);
                    i = line_end;
                    if i < len && chars[i] == '\r' {
                        i += 1;
                    }
                    if i < len && chars[i] == '\n' {
                        i += 1;
                    }
                    if heredoc_stack.is_empty() {
                        in_heredoc_body = false;
                    }
                    continue;
                } else {
                    // Body line: skip to next line
                    i = line_end;
                    if i < len && chars[i] == '\r' {
                        i += 1;
                    }
                    if i < len && chars[i] == '\n' {
                        i += 1;
                    }
                    continue;
                }
            } else {
                in_heredoc_body = false;
            }
        }

        if in_comment {
            if c == '\n' {
                in_comment = false;
            }
            i += 1;
            continue;
        }

        if in_triple_double_quote {
            if c == '"' && i + 2 < len && chars[i + 1] == '"' && chars[i + 2] == '"' {
                in_triple_double_quote = false;
                i += 3;
                continue;
            }
            if c == '\\' {
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        if in_triple_single_quote {
            if c == '\'' && i + 2 < len && chars[i + 1] == '\'' && chars[i + 2] == '\'' {
                in_triple_single_quote = false;
                i += 3;
                continue;
            }
            i += 1;
            continue;
        }

        if in_single_quote {
            if c == '\'' {
                in_single_quote = false;
            }
            i += 1;
            continue;
        }

        if in_double_quote {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == '"' {
                in_double_quote = false;
            }
            i += 1;
            continue;
        }

        // Newline outside strings: activate any pending heredocs
        if c == '\n' && !pending_heredocs.is_empty() {
            heredoc_stack.append(&mut pending_heredocs);
            in_heredoc_body = true;
            i += 1;
            continue;
        }

        // Outside quotes / comments
        if c == '#' && (i == 0 || chars[i - 1].is_whitespace()) {
            in_comment = true;
            i += 1;
            continue;
        }

        // Check for triple quotes
        if c == '"' && i + 2 < len && chars[i + 1] == '"' && chars[i + 2] == '"' {
            in_triple_double_quote = true;
            i += 3;
            continue;
        }

        if c == '\'' && i + 2 < len && chars[i + 1] == '\'' && chars[i + 2] == '\'' {
            in_triple_single_quote = true;
            i += 3;
            continue;
        }

        if c == '\'' {
            in_single_quote = true;
            i += 1;
            continue;
        }

        if c == '"' {
            in_double_quote = true;
            i += 1;
            continue;
        }

        if c == '\\' {
            i += 2;
            continue;
        }

        // Here-documents use the shared delimiter rule so indentation and
        // execution can never disagree about what a header names.
        if c == '<' && i + 1 < len && chars[i + 1] == '<' {
            let rest: String = chars[i..].iter().collect();
            if let Some(spec) = scan_heredoc_delimiter(&rest) {
                let consumed_chars = rest[..spec.consumed].chars().count();
                pending_heredocs.push((spec.delimiter, spec.strip_tabs));
                i += consumed_chars;
                continue;
            }
            i += 1;
            continue;
        }

        match c {
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            _ => {}
        }

        i += 1;
    }

    let unclosed_heredoc = !heredoc_stack.is_empty() || !pending_heredocs.is_empty();

    DelimiterScan {
        unclosed_heredoc,
        brace_depth,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_native_intent_is_independent_of_comments() {
        // A leading comment must not hide a later native declaration.
        let inspection = inspect_input("# note\nlet x = 1\nfor i in 1 2; do");
        assert!(inspection.native_intent);

        let inspection = inspect_input("echo hi\nfor i in 1 2; do");
        assert!(!inspection.native_intent);

        // `sh { ... }` is native syntax; `sh script.sh` is an ordinary command.
        assert!(inspect_input("sh { echo let }").native_intent);
        assert!(!inspect_input("sh script.sh").native_intent);
        assert!(!inspect_input("echo let").native_intent);
    }

    #[test]
    fn test_validate_complete_commands() {
        assert_eq!(validate_input("echo hello"), ValidationResult::Complete);
        assert_eq!(validate_input("echo foo*"), ValidationResult::Complete);
        assert_eq!(validate_input("echo foo,"), ValidationResult::Complete);
        assert_eq!(validate_input("echo foo="), ValidationResult::Complete);
        assert_eq!(validate_input("echo for do"), ValidationResult::Complete);
        assert_eq!(validate_input("echo if then"), ValidationResult::Complete);
        assert_eq!(
            validate_input("let x = 42; echo $x"),
            ValidationResult::Complete
        );
        assert_eq!(
            validate_input("if true { echo ok } else { echo no }"),
            ValidationResult::Complete
        );
        assert_eq!(validate_input(""), ValidationResult::Complete);
        assert_eq!(validate_input("   "), ValidationResult::Complete);
    }

    #[test]
    fn test_validate_unclosed_quotes() {
        assert!(matches!(
            validate_input("echo \"hello"),
            ValidationResult::Incomplete {
                prompt_hint: "dquote"
            }
        ));
        assert!(matches!(
            validate_input("echo 'hello"),
            ValidationResult::Incomplete {
                prompt_hint: "squote"
            }
        ));
        assert!(matches!(
            validate_input("echo r#\"hello"),
            ValidationResult::Incomplete {
                prompt_hint: "dquote"
            }
        ));
        assert!(matches!(
            validate_input("echo \"\"\"hello"),
            ValidationResult::Incomplete {
                prompt_hint: "triple_dquote"
            }
        ));
        assert!(matches!(
            validate_input("echo '''hello"),
            ValidationResult::Incomplete {
                prompt_hint: "triple_squote"
            }
        ));
        assert_eq!(
            validate_input("echo \"\"\"hello\nworld\"\"\""),
            ValidationResult::Complete
        );
    }

    #[test]
    fn test_validate_heredocs() {
        assert!(matches!(
            validate_input("cat <<EOF\nhello"),
            ValidationResult::Incomplete {
                prompt_hint: "heredoc"
            }
        ));
        assert_eq!(
            validate_input("cat <<EOF\nhello\nEOF\n"),
            ValidationResult::Complete
        );
        assert_eq!(
            validate_input("let x = <<'EOF'\nhello $world\nEOF\n"),
            ValidationResult::Complete
        );
        // Several here-documents on one line are complete only once every body
        // is terminated, and the bodies are read in header order.
        assert_eq!(
            validate_input("cat <<A; cat <<B\none\nA\ntwo\nB\n"),
            ValidationResult::Complete
        );
        assert!(matches!(
            validate_input("cat <<A; cat <<B\none\nA\n"),
            ValidationResult::Incomplete {
                prompt_hint: "heredoc"
            }
        ));
        assert!(matches!(
            validate_input("cat <<A; cat <<B\none\nA\ntwo\n"),
            ValidationResult::Incomplete {
                prompt_hint: "heredoc"
            }
        ));
    }

    #[test]
    fn test_validate_backquote_substitution() {
        assert!(matches!(
            validate_input("echo `date"),
            ValidationResult::Incomplete { .. }
        ));
        assert_eq!(validate_input("echo `date`"), ValidationResult::Complete);
        assert!(matches!(
            validate_input("echo \"a `date"),
            ValidationResult::Incomplete { .. }
        ));
    }

    #[test]
    fn heredoc_delimiters_are_words_after_quote_removal() {
        // The scanner alone decides here: none of these parse natively.
        for input in [
            "cat <<E\\OF\nbody\nEOF",
            "cat <<E\"O\"F\nbody\nEOF",
            "cat <<'E'OF\nbody\nEOF",
            "cat <<EOF;echo\nbody\nEOF",
        ] {
            assert!(
                !scan_delimiters(input, None).unclosed_heredoc,
                "terminated: {input:?}"
            );
        }
    }

    #[test]
    fn heredoc_terminator_must_match_the_whole_line() {
        assert!(scan_delimiters("cat <<EOF\nbody\n  EOF", None).unclosed_heredoc);
        assert!(!scan_delimiters("cat <<-EOF\n\tbody\n\tEOF", None).unclosed_heredoc);
        assert!(scan_delimiters("cat <<A; cat <<B\none\nA", None).unclosed_heredoc);
        assert!(!scan_delimiters("cat <<A; cat <<B\none\nA\ntwo\nB", None).unclosed_heredoc);
    }

    #[test]
    fn test_validate_unclosed_brackets() {
        assert!(matches!(
            validate_input("fn foo() {\n    echo 1"),
            ValidationResult::Incomplete {
                prompt_hint: "brace"
            }
        ));
        assert!(matches!(
            validate_input("fn greet(name) {\n    echo \"hello ${name}\""),
            ValidationResult::Incomplete {
                prompt_hint: "brace"
            }
        ));
        assert!(matches!(
            validate_input("let x = [1, 2,"),
            ValidationResult::Incomplete { .. }
        ));
        assert!(matches!(
            validate_input("let x = (1 + 2"),
            ValidationResult::Incomplete {
                prompt_hint: "paren"
            }
        ));
    }

    #[test]
    fn test_validate_compound_keywords() {
        assert!(matches!(
            validate_input("try {\n    echo 1\n}"),
            ValidationResult::Incomplete { .. }
        ));
        assert_eq!(
            validate_input("try {\n    echo 1\n} catch e {\n    echo 2\n}"),
            ValidationResult::Complete
        );
        assert!(matches!(
            validate_input("for x in"),
            ValidationResult::Incomplete { .. }
        ));
        assert!(matches!(
            validate_input("with caps"),
            ValidationResult::Incomplete { .. }
        ));
    }

    #[test]
    fn test_validate_trailing_operators() {
        // Every one of these ends inside the parser's grammar, so the parser
        // reports the end of input and the classifier asks for a continuation.
        for input in [
            "ls |",
            "echo a &&",
            "echo a ||",
            "echo a \\",
            "items |>",
            "let x = 1 +",
            "let x = 1 *",
        ] {
            let result = validate_input(input);
            assert!(
                matches!(
                    result,
                    ValidationResult::Incomplete {
                        prompt_hint: "incomplete"
                    }
                ),
                "{input:?} should be incomplete, got {result:?}"
            );
        }

        // `|?` is not a pipeline operator and `+=` is not an assignment: the
        // parser knows these characters cannot start the missing text, so they
        // are definite errors rather than continuations.
        for input in ["items |?", "let x += "] {
            assert!(
                matches!(validate_input(input), ValidationResult::Invalid { .. }),
                "{input:?} should be invalid"
            );
        }
    }

    #[test]
    fn test_validate_stray_closers_are_invalid() {
        for input in ["}", ")", "]"] {
            assert!(
                matches!(validate_input(input), ValidationResult::Invalid { .. }),
                "{input:?} should be invalid"
            );
        }
        assert!(matches!(
            validate_input("let x = |"),
            ValidationResult::Invalid { .. }
        ));
    }

    #[test]
    fn test_compute_indent_depth() {
        assert_eq!(compute_indent_depth_at("fn foo() {", None), 1);
        assert_eq!(
            compute_indent_depth_at("fn foo() {\n    if true {", None),
            2
        );
        assert_eq!(
            compute_indent_depth_at("fn foo() {\n    if true {}\n}", None),
            0
        );
        assert_eq!(
            compute_indent_depth_at(
                "fn foo() {\n    if true {\n        echo 1\n    }\n}",
                Some(15)
            ),
            1
        );
    }

    #[test]
    fn test_validate_unicode_multibyte_characters() {
        assert_eq!(validate_input("è"), ValidationResult::Complete);
        assert_eq!(validate_input("echo è à é"), ValidationResult::Complete);
        assert_eq!(validate_input("let x = \"è\""), ValidationResult::Complete);
        assert!(matches!(
            validate_input("echo \"è"),
            ValidationResult::Incomplete {
                prompt_hint: "dquote"
            }
        ));
    }

    #[test]
    fn test_validate_multiline_realworld_scripts() {
        let heredoc_with_braces = r#"cat <<EOF > /tmp/nginx-test.conf
server {
    listen 8080;
    server_name localhost;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_set_header Host $host;
    }
}
EOF"#;
        assert_eq!(
            validate_input(heredoc_with_braces),
            ValidationResult::Complete
        );

        let indented_heredoc = r#"cat <<EOF > /tmp/nginx-test.conf
    server {
        listen 8080;
        server_name localhost;

        location / {
            proxy_pass http://127.0.0.1:3000;
            proxy_set_header Host $host;
        }
    }
    EOF"#;
        // `<<-` strips leading tabs; a plain `<<EOF` terminator must be the whole
        // line, so an indented body is still waiting for its delimiter.
        assert!(matches!(
            validate_input(indented_heredoc),
            ValidationResult::Incomplete {
                prompt_hint: "heredoc"
            }
        ));

        let and_backslash = r#"echo "=== Step 1: Checking code ===" && \
git status --short && \
echo "=== Step 2: Running cargo test ===" && \
cargo test --quiet && \
echo "=== Build and verification passed! ===""#;
        assert_eq!(validate_input(and_backslash), ValidationResult::Complete);

        let match_block = r#"let env_target = "staging"
match env_target {
    "prod" => {
        echo "DEPLOY TARGET: Production"
    },
    "stage" => {
        echo "DEPLOY TARGET: Staging"
    },
    _ => {
        echo "DEPLOY TARGET: Development"
    },
}"#;
        assert_eq!(validate_input(match_block), ValidationResult::Complete);

        let posix_block = r#"sh {
    for i in 1 2 3 4 5; do
        if [ $((i % 2)) -eq 0 ]; then
            echo "Even: $i"
        else
            echo "Odd:  $i"
        fi
    done
}"#;
        assert_eq!(validate_input(posix_block), ValidationResult::Complete);
    }

    #[test]
    fn test_parser_acceptance_wins_over_generic_delimiter_scan() {
        // `[` is a valid literal argument for an external command. The
        // parser's command-argument mode knows that; a generic bracket scan
        // must not force the REPL into continuation mode first.
        assert_eq!(validate_input("echo ["), ValidationResult::Complete);
        assert_eq!(validate_input("echo [foo"), ValidationResult::Complete);
    }

    #[test]
    fn test_parser_mismatch_is_invalid_not_continuation() {
        assert!(matches!(
            validate_input("fn foo )"),
            ValidationResult::Invalid { .. }
        ));
        assert!(matches!(
            validate_input("for item nope"),
            ValidationResult::Invalid { .. }
        ));
    }
}
