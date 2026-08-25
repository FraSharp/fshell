// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::parser::{ParseError, Parser};
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

fn is_word(input: &str, word: &str) -> bool {
    let mut i = 0;
    let bytes = input.as_bytes();
    let w = word.as_bytes();
    while i + w.len() <= bytes.len() {
        if bytes[i..i + w.len()].eq_ignore_ascii_case(w) {
            let before_ok = i == 0 || {
                let c = bytes[i - 1] as char;
                !c.is_alphanumeric() && c != '_'
            };
            let after_ok = i + w.len() == bytes.len() || {
                let c = bytes[i + w.len()] as char;
                !c.is_alphanumeric() && c != '_'
            };
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn count_word(input: &str, word: &str) -> usize {
    let mut count = 0;
    let mut i = 0;
    let bytes = input.as_bytes();
    let w = word.as_bytes();
    while i + w.len() <= bytes.len() {
        if bytes[i..i + w.len()].eq_ignore_ascii_case(w) {
            let before_ok = i == 0 || {
                let c = bytes[i - 1] as char;
                !c.is_alphanumeric() && c != '_'
            };
            let after_ok = i + w.len() == bytes.len() || {
                let c = bytes[i + w.len()] as char;
                !c.is_alphanumeric() && c != '_'
            };
            if before_ok && after_ok {
                count += 1;
                i += w.len();
                continue;
            }
        }
        i += 1;
    }
    count
}

fn looks_like_posix_shell(input: &str) -> bool {
    let trimmed = input.trim_start();
    if let Some(first_line) = trimmed.lines().next()
        && let Some(rest) = first_line.strip_prefix("#!")
        && let Some(shell) = rest.split_whitespace().next()
    {
        let name = std::path::Path::new(shell)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if matches!(name.as_str(), "sh" | "bash" | "zsh" | "dash" | "ksh") {
            return true;
        }
    }
    if is_word(input, "function") && input.contains("()") {
        return true;
    }
    if is_word(input, "then")
        || is_word(input, "fi")
        || is_word(input, "esac")
        || is_word(input, "case")
    {
        if (is_word(input, "if") && (is_word(input, "then") || is_word(input, "fi")))
            || (is_word(input, "case") && is_word(input, "esac"))
        {
            return true;
        }
    }
    let has_do = is_word(input, "do");
    let has_done = is_word(input, "done");
    if has_do && has_done {
        return true;
    }
    if has_do && (is_word(input, "for") || is_word(input, "while") || is_word(input, "until")) {
        return true;
    }
    false
}

fn posix_block_incomplete(input: &str) -> Option<bool> {
    if !looks_like_posix_shell(input) {
        return None;
    }
    let mut depth: i32 = 0;
    let words: Vec<String> = input
        .split(|c: char| {
            c.is_whitespace() || c == ';' || c == '&' || c == '|' || c == '(' || c == ')'
        })
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect();
    for w in words {
        match w.as_str() {
            "for" | "while" | "until" | "if" | "case" | "select" => depth += 1,
            "done" | "fi" | "esac" => depth -= 1,
            _ => {}
        }
    }
    let do_c = count_word(input, "do");
    let done_c = count_word(input, "done");
    if do_c != done_c {
        return Some(true);
    }
    if depth > 0 {
        return Some(true);
    }
    Some(false)
}

/// Validates whether the given input text is complete, incomplete, or invalid.
pub fn validate_input(input: &str) -> ValidationResult {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return ValidationResult::Complete;
    }

    // 1. Fast lexical scan for unclosed delimiters, quotes, and heredocs
    let lex = scan_delimiters(input, None);

    if let Some(quote_hint) = lex.unclosed_quote {
        return ValidationResult::Incomplete {
            prompt_hint: quote_hint,
        };
    }

    if lex.unclosed_heredoc {
        return ValidationResult::Incomplete {
            prompt_hint: "heredoc",
        };
    }

    if lex.brace_depth > 0 {
        return ValidationResult::Incomplete {
            prompt_hint: "brace",
        };
    }

    if lex.paren_depth > 0 {
        return ValidationResult::Incomplete {
            prompt_hint: "paren",
        };
    }

    if lex.bracket_depth > 0 {
        return ValidationResult::Incomplete {
            prompt_hint: "bracket",
        };
    }

    // 1.5 POSIX shell fast-path: `for ...; do ... done` etc. use POSIX balancing
    if let Some(incomplete) = posix_block_incomplete(input) {
        if incomplete {
            return ValidationResult::Incomplete {
                prompt_hint: "posix",
            };
        } else {
            return ValidationResult::Complete;
        }
    }

    // 1.6 `find -exec ... {} +` / `\;` - `+` is find's terminator, not a trailing operator.
    // The Pratt parser treats trailing `+` as `a + <missing>`, so `find ... {} +` would be
    // `Invalid`. Detect the common `find ... -exec ... {} +|;` pattern and short-circuit.
    {
        let trimmed = input.trim();
        if trimmed.contains(" -exec ")
            && (trimmed.ends_with(" {} +")
                || trimmed.ends_with(" {} ;")
                || trimmed.ends_with(" {} \\;")
                || trimmed.ends_with(" +")
                || trimmed.ends_with(" \\;"))
        {
            // Also handle quoted variant `} +` already handled above, but be permissive
            return ValidationResult::Complete;
        }
        // Generic: external command ending with standalone `+` after `}` is `find` style
        if is_word(trimmed, "find") && (trimmed.ends_with(" +") || trimmed.ends_with(" ;")) {
            return ValidationResult::Complete;
        }
    }

    // 2. Check for trailing line continuations, operators, or keywords
    let chars: Vec<char> = input.chars().collect();
    let mut end_pos = chars.len();
    while end_pos > 0 && chars[end_pos - 1].is_whitespace() {
        end_pos -= 1;
    }

    if end_pos > 0 {
        if end_pos >= 2 && chars[end_pos - 2] == '&' && chars[end_pos - 1] == '&' {
            return ValidationResult::Incomplete { prompt_hint: "and" };
        }

        if end_pos >= 2 && chars[end_pos - 2] == '|' && chars[end_pos - 1] == '|' {
            return ValidationResult::Incomplete { prompt_hint: "or" };
        }

        if end_pos >= 2
            && chars[end_pos - 2] == '|'
            && (chars[end_pos - 1] == '>' || chars[end_pos - 1] == '?')
        {
            return ValidationResult::Incomplete {
                prompt_hint: "pipe",
            };
        }

        if end_pos >= 2
            && (chars[end_pos - 2] == '+'
                || chars[end_pos - 2] == '-'
                || chars[end_pos - 2] == '*'
                || chars[end_pos - 2] == '/')
            && chars[end_pos - 1] == '='
        {
            return ValidationResult::Incomplete {
                prompt_hint: "operator",
            };
        }

        // Count trailing backslashes to determine if the line ends with an unescaped backslash
        let mut backslash_count = 0usize;
        let mut idx = end_pos;
        while idx > 0 && chars[idx - 1] == '\\' {
            backslash_count += 1;
            idx -= 1;
        }
        if backslash_count % 2 == 1 {
            return ValidationResult::Incomplete {
                prompt_hint: "backslash",
            };
        }

        let last_char = chars[end_pos - 1];
        if last_char == '|' {
            return ValidationResult::Incomplete {
                prompt_hint: "pipe",
            };
        }

        if last_char == ',' {
            return ValidationResult::Incomplete {
                prompt_hint: "comma",
            };
        }

        if (last_char == '+' || last_char == '*' || last_char == '=')
            && (end_pos == 1
                || (chars[end_pos - 2] != '+'
                    && chars[end_pos - 2] != '-'
                    && chars[end_pos - 2] != '*'
                    && chars[end_pos - 2] != '/'))
        {
            // Exception: `find ... -exec ... {} +` ends with `+` but is complete.
            // The `+` is a standalone find terminator after `}`, not a trailing `let x = 1 +`.
            if last_char == '+' {
                let mut idx = end_pos - 1;
                // idx points to '+', step back over whitespace
                if idx > 0 {
                    idx -= 1;
                    while idx > 0 && chars[idx].is_whitespace() {
                        idx -= 1;
                    }
                    // chars[idx] is the last non-whitespace char before '+'
                    // If it's `}` (as in `{} +`), it's find's terminator -> complete
                    if chars[idx] == '}' {
                        // Check that we actually have `{}` before it (find pattern)
                        // Walk back one more to see `{` - not strictly needed, `}` alone is enough
                    } else {
                        return ValidationResult::Incomplete {
                            prompt_hint: "operator",
                        };
                    }
                } else {
                    return ValidationResult::Incomplete {
                        prompt_hint: "operator",
                    };
                }
            } else {
                return ValidationResult::Incomplete {
                    prompt_hint: "operator",
                };
            }
        }

        // Check trailing keywords and tokens
        let non_ws_chars = &chars[..end_pos];
        let last_word: String = non_ws_chars
            .split(|c| c.is_whitespace() || *c == ';' || *c == '(' || *c == '{')
            .rfind(|s| !s.is_empty())
            .map(|slice| slice.iter().collect())
            .unwrap_or_default();

        if matches!(
            last_word.as_str(),
            "fn" | "if"
                | "else"
                | "while"
                | "match"
                | "try"
                | "catch"
                | "with"
                | "for"
                | "in"
                | "caps"
                | "sh"
                | "posix"
                | "bash"
                | "=>"
                | "->"
        ) {
            return ValidationResult::Incomplete {
                prompt_hint: "keyword",
            };
        }
    }

    // 3. Full parser verification
    let mut parser = Parser::new(input);
    match parser.parse_statements() {
        Ok(_) => ValidationResult::Complete,
        Err(err) => match err {
            ParseError::UnexpectedEof { .. } => ValidationResult::Incomplete {
                prompt_hint: "incomplete",
            },
            ParseError::ExpectedChar {
                span,
                expected,
                found: _,
            } => {
                if expected == '{'
                    || expected == '}'
                    || expected == '('
                    || expected == ')'
                    || expected == '['
                    || expected == ']'
                {
                    ValidationResult::Incomplete {
                        prompt_hint: match expected {
                            '{' | '}' => "brace",
                            '(' | ')' => "paren",
                            '[' | ']' => "bracket",
                            _ => "incomplete",
                        },
                    }
                } else {
                    ValidationResult::Invalid {
                        message: format!("Expected character '{}'", expected),
                        span,
                    }
                }
            }
            ParseError::ExpectedToken {
                span, ref expected, ..
            } => {
                if expected == "{"
                    || expected == "}"
                    || expected == "("
                    || expected == ")"
                    || expected == "["
                    || expected == "]"
                    || expected == "catch"
                    || expected == "in"
                    || expected == "=>"
                    || expected == "caps"
                {
                    ValidationResult::Incomplete {
                        prompt_hint: match expected.as_str() {
                            "{" | "}" => "brace",
                            "(" | ")" => "paren",
                            "[" | "]" => "bracket",
                            "catch" => "catch",
                            "in" => "in",
                            "=>" => "arm",
                            _ => "incomplete",
                        },
                    }
                } else {
                    ValidationResult::Invalid {
                        message: format!("Expected {}", expected),
                        span,
                    }
                }
            }
            ParseError::SyntaxError { ref message, span } => {
                let lower = message.to_lowercase();
                if lower.contains("unterminated")
                    || lower.contains("expected '{'")
                    || lower.contains("expected 'in'")
                    || lower.contains("expected block")
                    || lower.contains("expected duration")
                {
                    ValidationResult::Incomplete {
                        prompt_hint: "incomplete",
                    }
                } else {
                    ValidationResult::Invalid {
                        message: message.clone(),
                        span,
                    }
                }
            }
        },
    }
}

/// Compute open brace indentation depth for entire input.
pub fn compute_indent_depth(input: &str) -> usize {
    compute_indent_depth_at(input, None)
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
    unclosed_quote: Option<&'static str>,
    unclosed_heredoc: bool,
    brace_depth: usize,
    paren_depth: usize,
    bracket_depth: usize,
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
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;

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
                if check_line == delim || check_line.trim() == delim {
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

        // Heredocs: `<<` or `<<-`
        if c == '<' && i + 1 < len && chars[i + 1] == '<' {
            if i + 2 < len && chars[i + 2] == '<' {
                // Here-string `<<<`
                i += 3;
                continue;
            }
            if i + 2 < len && chars[i + 2] == '(' {
                // Process substitution `<(`
                i += 1;
                continue;
            }
            let mut j = i + 2;
            let mut strip_tabs = false;
            if j < len && chars[j] == '-' {
                strip_tabs = true;
                j += 1;
            }
            while j < len && (chars[j] == ' ' || chars[j] == '\t') {
                j += 1;
            }
            let mut delim = String::new();
            if j < len && chars[j] == '\'' {
                j += 1;
                while j < len && chars[j] != '\'' {
                    delim.push(chars[j]);
                    j += 1;
                }
                if j < len && chars[j] == '\'' {
                    j += 1;
                }
            } else if j < len && chars[j] == '"' {
                j += 1;
                while j < len && chars[j] != '"' {
                    delim.push(chars[j]);
                    j += 1;
                }
                if j < len && chars[j] == '"' {
                    j += 1;
                }
            } else {
                while j < len && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '-')
                {
                    delim.push(chars[j]);
                    j += 1;
                }
            }
            if !delim.is_empty() {
                pending_heredocs.push((delim, strip_tabs));
                i = j;
                continue;
            } else {
                i += 2;
                continue;
            }
        }

        match c {
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ => {}
        }

        i += 1;
    }

    let unclosed_quote = if in_triple_double_quote {
        Some("triple_dquote")
    } else if in_triple_single_quote {
        Some("triple_squote")
    } else if in_single_quote {
        Some("squote")
    } else if in_double_quote {
        Some("dquote")
    } else {
        None
    };

    let unclosed_heredoc = !heredoc_stack.is_empty() || !pending_heredocs.is_empty();

    DelimiterScan {
        unclosed_quote,
        unclosed_heredoc,
        brace_depth,
        paren_depth,
        bracket_depth,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_complete_commands() {
        assert_eq!(validate_input("echo hello"), ValidationResult::Complete);
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
        assert!(matches!(
            validate_input("ls |"),
            ValidationResult::Incomplete {
                prompt_hint: "pipe"
            }
        ));
        assert!(matches!(
            validate_input("echo a &&"),
            ValidationResult::Incomplete { prompt_hint: "and" }
        ));
        assert!(matches!(
            validate_input("echo a ||"),
            ValidationResult::Incomplete { prompt_hint: "or" }
        ));
        assert!(matches!(
            validate_input("echo a \\"),
            ValidationResult::Incomplete {
                prompt_hint: "backslash"
            }
        ));
        assert!(matches!(
            validate_input("items |>"),
            ValidationResult::Incomplete {
                prompt_hint: "pipe"
            }
        ));
        assert!(matches!(
            validate_input("let x = 1 +"),
            ValidationResult::Incomplete {
                prompt_hint: "operator"
            }
        ));
        assert!(matches!(
            validate_input("let x += "),
            ValidationResult::Incomplete {
                prompt_hint: "operator"
            }
        ));
    }

    #[test]
    fn test_compute_indent_depth() {
        assert_eq!(compute_indent_depth("fn foo() {"), 1);
        assert_eq!(compute_indent_depth("fn foo() {\n    if true {"), 2);
        assert_eq!(compute_indent_depth("fn foo() {\n    if true {}\n}"), 0);
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
        assert_eq!(validate_input(indented_heredoc), ValidationResult::Complete);

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

        let posix_block = r#"posix {
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
}
