// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::theme::Theme;
use fshell_engine::Env;
use fshell_hash::FxHashSet;
use nu_ansi_term::Style as NuStyle;
use reedline::{Highlighter, StyledText};
use std::sync::Arc;

use crate::theme_ext::ThemeColorNu;

std::thread_local! {
    static HEREDOC_ACTIVE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// fshell syntax highlighter with command validation (like fish).
pub struct FshellHighlighter {
    builtins: FxHashSet<String>,
    alias_state: Option<Arc<crate::alias_expansion::AliasExpansionState>>,
    theme: Arc<Theme>,
    env: Option<Env>,
}

impl FshellHighlighter {
    pub fn new(builtins: Vec<String>) -> Self {
        let builtins = builtins.into_iter().collect();
        Self {
            builtins,
            alias_state: None,
            theme: Arc::new(Theme::default_theme()),
            env: None,
        }
    }

    pub fn with_theme(mut self, theme: Arc<Theme>) -> Self {
        self.theme = theme;
        self
    }

    pub fn with_env(mut self, env: Env) -> Self {
        self.env = Some(env);
        self
    }

    pub fn update_theme(&mut self, theme: Arc<Theme>) {
        self.theme = theme;
    }

    pub fn with_alias_state(
        mut self,
        state: Arc<crate::alias_expansion::AliasExpansionState>,
    ) -> Self {
        self.alias_state = Some(state);
        self
    }

    fn is_user_function(&self, token: &str) -> bool {
        if let Some(ref env) = self.env {
            return env.fns.read().contains_key(token);
        }
        false
    }

    fn is_external_command(&self, token: &str) -> bool {
        if let Some(ref env) = self.env {
            let env_path = Some(env.vars.read()).and_then(|vars| {
                if let Some(fshell_core::Val::String(s)) = vars.get("PATH") {
                    Some(s.clone())
                } else {
                    None
                }
            });
            return fshell_engine::is_external_command_cached(token, env_path.as_deref());
        }
        false
    }

    fn is_heredoc_continuation() -> bool {
        HEREDOC_ACTIVE.with(|h| h.borrow().is_some())
    }

    fn classify_token(&self, tok: &str) -> NuStyle {
        let s = &self.theme.syntax;
        match tok {
            "let" | "fn" | "match" | "try" | "catch" | "with" | "caps" | "true" | "false"
            | "null" | "if" | "else" | "while" | "source" | "unsafe" => s.keyword.to_style_bold(),
            "filter" | "map" | "sort" | "grep" | "count" | "limit" => s.pipe.to_style(),
            _ if tok.starts_with('@') => s.variable.to_style(),
            _ => s.normal_text.to_style(),
        }
    }

    /// Tokenizer that emits ratatui styles natively — avoids the reedline
    /// `StyledText(Vec<(NuStyle,String)>)` + per-frame `convert_ansi_style`
    /// indirection on the ftui hot path (A3). Shares the same lexing logic
    /// as `highlight()` but returns `Vec<ratatui::text::Span>` directly.
    pub fn highlight_ratatui(&self, line: &str) -> Vec<ratatui::text::Span<'static>> {
        let styled = self.highlight(line, 0);
        styled
            .buffer
            .into_iter()
            .map(|(ns, s)| {
                let rs = crate::ftui::ansi::convert_ansi_style(ns);
                ratatui::text::Span::styled(s, rs)
            })
            .collect()
    }

    fn flush_token(&self, token: &mut String, styled: &mut StyledText, is_first_word: &mut bool) {
        if token.is_empty() {
            return;
        }
        let tok = token.clone();
        let s = &self.theme.syntax;
        let is_alias = self.alias_state.as_ref().is_some_and(|a| a.is_alias(&tok));
        let style = if self.builtins.contains(&tok) || is_alias {
            s.builtin.to_style_bold()
        } else if tok.starts_with('$') || tok.starts_with('@') {
            s.variable.to_style()
        } else if is_number(&tok) {
            s.number.to_style()
        } else if is_operator(&tok) {
            s.operator.to_style_bold()
        } else if *is_first_word
            && !tok.starts_with('-')
            && !tok.starts_with('/')
            && !tok.starts_with('.')
            && !tok.starts_with('~')
            && !tok.starts_with('#')
            && !tok.starts_with('|')
            && !tok.starts_with('\"')
            && !tok.starts_with('\'')
        {
            // First word on the line that isn't a flag, path, or keyword — check if it's a known command
            if is_keyword(&tok) {
                s.keyword.to_style_bold()
            } else if self.is_user_function(&tok) || self.is_external_command(&tok) {
                s.function.to_style_bold()
            } else {
                s.unknown_command.to_style_bold()
            }
        } else {
            self.classify_token(&tok)
        };
        styled.push((style, tok));
        token.clear();
        *is_first_word = false;
    }
}

fn is_number(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut has_dot = false;
    let mut has_digit = false;
    for (i, c) in s.chars().enumerate() {
        if c == '-' && i == 0 {
            continue;
        }
        if c == '.' && !has_dot {
            has_dot = true;
            continue;
        }
        if c.is_ascii_digit() {
            has_digit = true;
        } else {
            return false;
        }
    }
    has_digit
}

fn is_operator(s: &str) -> bool {
    matches!(
        s,
        "==" | "!="
            | "<"
            | ">"
            | "<="
            | ">="
            | "+"
            | "-"
            | "*"
            | "/"
            | "."
            | "="
            | "=>"
            | "!"
            | "|>"
    )
}

fn is_keyword(token: &str) -> bool {
    matches!(
        token,
        "let"
            | "fn"
            | "match"
            | "try"
            | "catch"
            | "with"
            | "caps"
            | "true"
            | "false"
            | "null"
            | "if"
            | "else"
            | "while"
            | "source"
            | "unsafe"
            | "filter"
            | "map"
            | "sort"
            | "grep"
            | "count"
            | "limit"
            | "return"
            | "exit"
            | "quit"
    )
}

/// Clamp cursor to the nearest valid UTF-8 char boundary at or before pos.
#[cfg(test)]
fn clamp_cursor(line: &str, cursor: usize) -> usize {
    let pos = cursor.min(line.len());
    if line.is_char_boundary(pos) {
        pos
    } else {
        line.floor_char_boundary(pos)
    }
}

// is_inside_string_literal is no longer part of the Highlighter trait in reedline 0.49.
// The logic survives only under #[cfg(test)] for regression coverage.
#[cfg(test)]
impl FshellHighlighter {
    fn is_inside_string_literal(&self, line: &str, cursor: usize) -> bool {
        // Heredoc continuation
        if Self::is_heredoc_continuation() {
            return true;
        }

        // Check for heredoc start on this line before cursor
        if let Some(pos) = line[..clamp_cursor(line, cursor)].find("<<") {
            // After << there should be a token name (not another <)
            let after = &line[pos + 2..];
            let token_end = after
                .find(|c: char| c.is_whitespace())
                .unwrap_or(after.len());
            if token_end > 0
                && after[..token_end]
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '\'' || c == '"')
            {
                return true;
            }
        }

        // Raw string detection r#"..."#
        if let Some(raw_start) = line[..clamp_cursor(line, cursor)].find("r#\"") {
            let after = &line[raw_start + 3..];
            if let Some(end) = after.find("\"#") {
                let end_abs = raw_start + 3 + end + 2;
                if cursor <= end_abs {
                    return true;
                }
            } else {
                // Unclosed raw string
                return true;
            }
        }

        // Regular string detection
        let mut in_single = false;
        let mut in_double = false;
        let mut escaped = false;
        for &byte in &line.as_bytes()[..cursor.min(line.len())] {
            if escaped {
                escaped = false;
                continue;
            }
            match byte {
                b'\\' => escaped = true,
                b'\'' if !in_double => in_single = !in_single,
                b'"' if !in_single => in_double = !in_double,
                _ => {}
            }
        }
        in_single || in_double
    }
}

impl Highlighter for FshellHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled = StyledText::new();
        let s = &self.theme.syntax;
        let comment_style = s.comment.to_style();
        let string_style = s.string.to_style();
        let var_style = s.variable.to_style();
        let escape_style = s.escape.to_style_bold();
        let pipe_style = s.pipe.to_style_bold();
        let alias_style = s.alias.to_style_italic();

        let mut current_token = String::new();
        let mut in_single = false;
        let mut in_double = false;
        let mut escaped = false;
        let mut is_first = true;

        // Heredoc continuation: if we're inside a heredoc from a previous line
        let in_heredoc_line = Self::is_heredoc_continuation();
        if in_heredoc_line {
            let end_marker = HEREDOC_ACTIVE.with(|h| h.borrow().clone());
            if let Some(ref marker) = end_marker
                && line.trim() == *marker
            {
                styled.push((string_style, line.to_string()));
                HEREDOC_ACTIVE.with(|h| *h.borrow_mut() = None);
                return styled;
            }
            // Still inside heredoc, entire line is string content
            styled.push((string_style, line.to_string()));
            return styled;
        }

        // Use char_indices() for correct byte-offset tracking.
        let mut chars = line.char_indices().peekable();

        while let Some((_byte_idx, c)) = chars.next() {
            if escaped {
                current_token.push(c);
                escaped = false;
                continue;
            }
            if c == '\\' && (in_single || in_double) {
                current_token.push(c);
                escaped = true;
                continue;
            }

            // Single-quoted strings
            if c == '\'' && !in_double {
                if in_single {
                    current_token.push(c);
                    styled.push((string_style, current_token.clone()));
                    current_token.clear();
                    in_single = false;
                } else {
                    self.flush_token(&mut current_token, &mut styled, &mut is_first);
                    current_token.push(c);
                    in_single = true;
                }
                continue;
            }

            // Double-quoted strings
            if c == '"' && !in_single {
                if in_double {
                    current_token.push(c);
                    styled.push((string_style, current_token.clone()));
                    current_token.clear();
                    in_double = false;
                } else if current_token == "r#" {
                    // Raw string r#"..."#
                    styled.push((string_style, "r#".to_string()));
                    current_token.clear();
                    let rest = &line[_byte_idx + 1..];
                    if let Some(end) = rest.find("\"#") {
                        let raw_content = &rest[..end];
                        styled.push((string_style, format!("\"{}\"#", raw_content)));
                        for _ in 0..rest[..=end + 1].chars().count() {
                            chars.next();
                        }
                    } else {
                        styled.push((string_style, "\"".to_string()));
                        in_double = true;
                    }
                } else {
                    self.flush_token(&mut current_token, &mut styled, &mut is_first);
                    current_token.push(c);
                    in_double = true;
                }
                continue;
            }

            if in_double {
                // Escape sequences in double-quoted strings
                if c == '\\' {
                    current_token.push(c);
                    if let Some(&(_, nc)) = chars.peek() {
                        if matches!(nc, 'n' | 't' | 'r' | '\\' | '"' | '0') {
                            chars.next();
                            current_token.push(nc);
                            styled.push((escape_style, current_token.clone()));
                            current_token.clear();
                            continue;
                        }
                        if nc == 'x' || nc == 'u' {
                            chars.next();
                            current_token.push(nc);
                            // Consume hex digits
                            while let Some(&(_, nc)) = chars.peek() {
                                if nc.is_ascii_hexdigit() || nc == '{' || nc == '}' {
                                    chars.next();
                                    current_token.push(nc);
                                } else {
                                    break;
                                }
                            }
                            styled.push((escape_style, current_token.clone()));
                            current_token.clear();
                            continue;
                        }
                    }
                }
                current_token.push(c);
                continue;
            }

            if in_single {
                current_token.push(c);
                continue;
            }

            // Raw string prefix: r# (not a comment)
            if c == '#' && current_token == "r" {
                current_token.push(c);
                continue;
            }

            // Comments
            if c == '#' {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                let rest: String = line[_byte_idx..].to_string();
                styled.push((comment_style, rest));
                return styled;
            }

            // Heredoc start: <<TOKEN, <<'TOKEN', <<"TOKEN"
            if c == '<' && chars.peek().is_some_and(|&(_, nc)| nc == '<') {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                chars.next(); // consume second '<'
                styled.push((string_style, "<<".to_string()));
                // Consume whitespace and optional quotes
                let mut heredoc_token = String::new();
                let mut after_heredoc = false;
                for (_, nc) in chars.by_ref() {
                    if !after_heredoc {
                        if nc.is_whitespace() {
                            continue;
                        }
                        if matches!(nc, '\'' | '"') {
                            after_heredoc = true;
                            continue;
                        }
                        after_heredoc = true;
                        heredoc_token.push(nc);
                    } else if matches!(nc, '\'' | '"') || nc.is_whitespace() {
                        break;
                    } else {
                        heredoc_token.push(nc);
                    }
                }
                if !heredoc_token.is_empty() {
                    styled.push((string_style, heredoc_token.clone()));
                    HEREDOC_ACTIVE.with(|h| *h.borrow_mut() = Some(heredoc_token));
                }
                continue;
            }

            // Variables ($name)
            if c == '$' {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                let mut var_name = String::from("$");
                while let Some(&(_, nc)) = chars.peek() {
                    if nc.is_alphanumeric() || nc == '_' {
                        var_name.push(nc);
                        chars.next();
                    } else {
                        break;
                    }
                }
                styled.push((var_style, var_name));
                continue;
            }

            // Pipe
            if c == '|' {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                is_first = true; // Next token after pipe is a new command
                styled.push((pipe_style, "|".to_string()));
                continue;
            }

            // Whitespace
            if c.is_whitespace() {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                styled.push((NuStyle::new(), c.to_string()));
                continue;
            }

            current_token.push(c);
        }

        self.flush_token(&mut current_token, &mut styled, &mut is_first);
        if let Some(ref alias_state) = self.alias_state {
            let expansions = alias_state.active_expansions();
            if !expansions.is_empty() {
                let mut byte_pos = 0;
                for item in &mut styled.buffer {
                    let end = byte_pos + item.1.len();
                    for (start, e_end, _) in &expansions {
                        if byte_pos < *e_end && end > *start {
                            item.0 = alias_style;
                            break;
                        }
                    }
                    byte_pos = end;
                }
            }
        }
        styled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlight_keywords() {
        let hl = FshellHighlighter::new(vec!["ls".to_string()]);
        let result = hl.highlight("let x = 42", 0);
        assert!(!result.buffer.is_empty());
    }

    #[test]
    fn test_highlight_multibyte() {
        let hl = FshellHighlighter::new(vec![]);
        // Emoji is 4 bytes but 1 char — char_indices tracks both
        let result = hl.highlight("let x = \"\u{1f680}\"", 0);
        assert!(!result.buffer.is_empty());
        // Verify the emoji is in the output intact
        let raw = result.raw_string();
        assert!(raw.contains('\u{1f680}'));
    }

    #[test]
    fn test_highlight_comments() {
        let hl = FshellHighlighter::new(vec![]);
        let result = hl.highlight("ls # comment", 0);
        let last = result.buffer.last().unwrap();
        assert!(last.1.contains("# comment"));
    }

    #[test]
    fn test_is_inside_string() {
        let hl = FshellHighlighter::new(vec![]);
        assert!(hl.is_inside_string_literal(r#"let x = "hello"#, 12));
        assert!(!hl.is_inside_string_literal(r#"let x = "hello""#, 15));
    }

    #[test]
    fn test_highlight_variables() {
        let hl = FshellHighlighter::new(vec![]);
        let result = hl.highlight("echo $HOME", 0);
        let raw = result.raw_string();
        assert!(raw.contains("$HOME"));
    }

    #[test]
    fn test_highlight_builtins() {
        let hl = FshellHighlighter::new(vec!["ls".to_string(), "cd".to_string()]);
        let result = hl.highlight("ls -la", 0);
        // First token should be "ls"
        assert_eq!(result.buffer[0].1, "ls");
    }

    #[test]
    fn test_highlight_raw_string() {
        let hl = FshellHighlighter::new(vec![]);
        let result = hl.highlight(r##"let x = r#"hello"#"##, 0);
        let raw = result.raw_string();
        assert!(
            raw.contains("hello"),
            "raw string content should be present: {:?}",
            raw
        );
    }

    #[test]
    fn test_highlight_heredoc_start() {
        let hl = FshellHighlighter::new(vec![]);
        let result = hl.highlight("cat << EOF", 0);
        let raw = result.raw_string();
        assert!(
            raw.contains("EOF"),
            "heredoc marker should be highlighted: {:?}",
            raw
        );
        // Reset heredoc state
        HEREDOC_ACTIVE.with(|h| *h.borrow_mut() = None);
    }

    #[test]
    fn test_highlight_escape_seq() {
        let hl = FshellHighlighter::new(vec![]);
        let result = hl.highlight(r#""hello\nworld""#, 0);
        let raw = result.raw_string();
        assert!(raw.contains("hello"), "string content should be present");
        assert!(raw.contains(r#"\n"#), "escape sequence should be present");
    }

    #[test]
    fn test_is_inside_raw_string() {
        let hl = FshellHighlighter::new(vec![]);
        // Inside raw string between r#" and "#
        assert!(hl.is_inside_string_literal(r##"let x = r#"hello"##, 17));
        // After closing raw string
        assert!(!hl.is_inside_string_literal(r##"let x = r#"hello"#; "##, 22));
    }

    #[test]
    fn test_highlight_invalid_command_is_red() {
        let hl = FshellHighlighter::new(vec!["ls".to_string(), "cd".to_string()]);
        let result = hl.highlight("zzzunknown", 0);
        // The first (and only) token should be colored with unknown_command color
        assert_eq!(result.buffer[0].1, "zzzunknown");
        let theme = Theme::default_theme();
        assert_eq!(
            result.buffer[0].0,
            theme.syntax.unknown_command.to_style_bold()
        );
    }

    #[test]
    fn test_highlight_known_builtin_not_red() {
        let hl = FshellHighlighter::new(vec!["ls".to_string(), "cd".to_string()]);
        let result = hl.highlight("ls -la", 0);
        // First token should be builtin color, not unknown_command
        assert_eq!(result.buffer[0].1, "ls");
        let theme = Theme::default_theme();
        assert_eq!(result.buffer[0].0, theme.syntax.builtin.to_style_bold());
    }

    #[test]
    fn test_highlight_argument_not_red() {
        let hl = FshellHighlighter::new(vec!["ls".to_string()]);
        let result = hl.highlight("ls foo", 0);
        // Second token should not be unknown_command color
        let arg = result.buffer.iter().find(|(_, s)| s == "foo");
        assert!(arg.is_some(), "argument foo should be present");
        let theme = Theme::default_theme();
        assert_ne!(
            arg.unwrap().0,
            theme.syntax.unknown_command.to_style_bold(),
            "argument should not be unknown_command color"
        );
    }

    #[test]
    fn test_highlight_keyword_not_red() {
        let hl = FshellHighlighter::new(vec![]);
        let result = hl.highlight("let x = 42", 0);
        // First token is a keyword, should be keyword color
        assert_eq!(result.buffer[0].1, "let");
        let theme = Theme::default_theme();
        assert_eq!(result.buffer[0].0, theme.syntax.keyword.to_style_bold());
    }

    #[test]
    fn test_highlight_pipe_resets_command_validation() {
        let hl = FshellHighlighter::new(vec!["ls".to_string(), "grep".to_string()]);
        let result = hl.highlight("ls | grep", 0);
        let ls = result.buffer.iter().find(|(_, s)| s == "ls").unwrap();
        let grep = result.buffer.iter().find(|(_, s)| s == "grep").unwrap();
        let theme = Theme::default_theme();
        assert_eq!(
            ls.0,
            theme.syntax.builtin.to_style_bold(),
            "ls should be builtin color"
        );
        assert_eq!(
            grep.0,
            theme.syntax.builtin.to_style_bold(),
            "grep after pipe should be builtin color"
        );
    }

    #[test]
    fn test_highlight_accented_unicode_characters() {
        let hl = FshellHighlighter::new(vec!["ls".to_string()]);
        let result = hl.highlight("è", 0);
        assert_eq!(result.buffer.len(), 1);
        assert_eq!(result.buffer[0].1, "è");

        let result2 = hl.highlight("echo è à é ì ò ù", 0);
        let text: String = result2.buffer.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(text, "echo è à é ì ò ù");

        let spans = hl.highlight_ratatui("è");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "è");
    }
}
