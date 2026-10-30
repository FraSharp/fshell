// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::theme_ext::ThemeColorNu;
use fshell_core::theme::Theme;
use fshell_engine::Env;
use fshell_hash::FxHashSet;
use nu_ansi_term::Style as NuStyle;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyledText {
    pub buffer: Vec<(NuStyle, String)>,
}

impl StyledText {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub fn push(&mut self, pair: (NuStyle, String)) {
        self.buffer.push(pair);
    }

    pub fn raw_string(&self) -> String {
        self.buffer.iter().map(|(_, s)| s.as_str()).collect()
    }
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
            return fshell_engine::is_external_command_cached_at(
                token,
                env_path.as_deref(),
                &env.cwd(),
            );
        }
        false
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

impl FshellHighlighter {
    /// Highlight a whole buffer.
    ///
    /// The pass is pure: every call starts from the text it is given, so a
    /// here-document in one command cannot tint the next one. The concatenation
    /// of the returned spans is always exactly `input`; styling never drops or
    /// invents a character.
    pub fn highlight(&self, input: &str, _cursor: usize) -> StyledText {
        let mut styled = StyledText::new();
        let s = &self.theme.syntax;
        let comment_style = s.comment.to_style();
        let string_style = s.string.to_style();
        let var_style = s.variable.to_style();
        let escape_style = s.escape.to_style_bold();
        let pipe_style = s.pipe.to_style_bold();
        let alias_style = s.alias.to_style_italic();
        let plain_style = NuStyle::new();

        let chars: Vec<char> = input.chars().collect();
        let mut current_token = String::new();
        let mut is_first = true;
        let mut in_single = false;
        let mut in_double = false;
        let mut escaped = false;
        let mut triple: Option<(usize, char)> = None;
        let mut heredoc: Option<(String, bool)> = None;
        let mut arithmetic_depth = 0usize;
        let mut parameter_depth = 0usize;
        let mut i = 0usize;

        while i < chars.len() {
            let c = chars[i];

            // A here-document body is string data until the whole delimiter
            // line. Terminator matching mirrors the parser: the line, minus its
            // ending, must be exactly the delimiter; `<<-` ignores leading tabs.
            if let Some((delimiter, strip_tabs)) = heredoc.as_ref() {
                let line_start = i;
                let mut line_end = i;
                while line_end < chars.len() && chars[line_end] != '\n' {
                    line_end += 1;
                }
                let mut content: String = chars[line_start..line_end].iter().collect();
                if let Some(without_cr) = content.strip_suffix('\r') {
                    content.truncate(without_cr.len());
                }
                let check = if *strip_tabs {
                    content.trim_start_matches('\t')
                } else {
                    content.as_str()
                };
                let matched = check == delimiter;
                let mut end = line_end;
                if end < chars.len() {
                    end += 1;
                }
                let span: String = chars[line_start..end].iter().collect();
                if !span.is_empty() {
                    styled.push((string_style, span));
                }
                i = end;
                is_first = true;
                if matched {
                    heredoc = None;
                }
                continue;
            }

            // A triple-quoted body is string data across newlines.
            if let Some((start, quote)) = triple {
                if c == quote
                    && i + 2 < chars.len()
                    && chars[i + 1] == quote
                    && chars[i + 2] == quote
                {
                    i += 3;
                    styled.push((string_style, chars[start..i].iter().collect()));
                    triple = None;
                } else {
                    i += 1;
                }
                continue;
            }

            if !in_single
                && !in_double
                && (c == '"' || c == '\'')
                && i + 2 < chars.len()
                && chars[i + 1] == c
                && chars[i + 2] == c
            {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                triple = Some((i, c));
                i += 3;
                continue;
            }

            if escaped {
                current_token.push(c);
                escaped = false;
                i += 1;
                continue;
            }

            if in_single {
                current_token.push(c);
                if c == '\'' {
                    styled.push((string_style, std::mem::take(&mut current_token)));
                    in_single = false;
                }
                i += 1;
                continue;
            }

            if in_double {
                if c == '\\' {
                    current_token.push(c);
                    i += 1;
                    if let Some(&nc) = chars.get(i) {
                        if matches!(nc, 'n' | 't' | 'r' | '\\' | '"' | '0') {
                            current_token.push(nc);
                            i += 1;
                            styled.push((escape_style, std::mem::take(&mut current_token)));
                            continue;
                        }
                        if nc == 'x' || nc == 'u' {
                            current_token.push(nc);
                            i += 1;
                            while i < chars.len()
                                && (chars[i].is_ascii_hexdigit()
                                    || chars[i] == '{'
                                    || chars[i] == '}')
                            {
                                current_token.push(chars[i]);
                                i += 1;
                            }
                            styled.push((escape_style, std::mem::take(&mut current_token)));
                            continue;
                        }
                    }
                    continue;
                }
                current_token.push(c);
                if c == '"' {
                    styled.push((string_style, std::mem::take(&mut current_token)));
                    in_double = false;
                }
                i += 1;
                continue;
            }

            // Raw string prefix `r#"…"#`: keep the whole literal as one span.
            if c == '#' && current_token == "r" && chars.get(i + 1) == Some(&'"') {
                current_token.clear();
                let start = i - 1;
                let mut end = i + 2;
                while end + 1 < chars.len() && !(chars[end] == '"' && chars[end + 1] == '#') {
                    end += 1;
                }
                if end + 1 < chars.len() {
                    end += 2;
                } else {
                    end = chars.len();
                }
                styled.push((string_style, chars[start..end].iter().collect()));
                i = end;
                continue;
            }

            if c == '#' && current_token.is_empty() {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                let start = i;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                styled.push((comment_style, chars[start..i].iter().collect()));
                continue;
            }

            if c == '<' && chars.get(i + 1) == Some(&'<') {
                let rest: String = chars[i..].iter().collect();
                if let Some(spec) = fshell_core::scan_heredoc_delimiter(&rest) {
                    self.flush_token(&mut current_token, &mut styled, &mut is_first);
                    let consumed_chars = rest[..spec.consumed].chars().count();
                    styled.push((string_style, chars[i..i + consumed_chars].iter().collect()));
                    i += consumed_chars;
                    heredoc = Some((spec.delimiter, spec.strip_tabs));
                    continue;
                }
            }

            if c == '$' && chars.get(i + 1) == Some(&'(') && chars.get(i + 2) == Some(&'(') {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                current_token.push_str("$((");
                arithmetic_depth = 2;
                i += 3;
                continue;
            }
            if arithmetic_depth > 0 {
                current_token.push(c);
                match c {
                    '(' => arithmetic_depth += 1,
                    ')' => arithmetic_depth -= 1,
                    _ => {}
                }
                i += 1;
                if arithmetic_depth == 0 {
                    styled.push((var_style, std::mem::take(&mut current_token)));
                }
                continue;
            }

            if c == '$' && chars.get(i + 1) == Some(&'{') {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                current_token.push_str("${");
                parameter_depth = 1;
                i += 2;
                continue;
            }
            if parameter_depth > 0 {
                current_token.push(c);
                match c {
                    '{' => parameter_depth += 1,
                    '}' => parameter_depth -= 1,
                    _ => {}
                }
                i += 1;
                if parameter_depth == 0 {
                    styled.push((var_style, std::mem::take(&mut current_token)));
                }
                continue;
            }

            if c == '$' {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                let mut name = String::from("$");
                i += 1;
                if i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                        name.push(chars[i]);
                        i += 1;
                    }
                } else if i < chars.len()
                    && matches!(
                        chars[i],
                        '?' | '#' | '@' | '!' | '-' | '$' | '*' | '0'..='9'
                    )
                {
                    name.push(chars[i]);
                    i += 1;
                }
                styled.push((var_style, name));
                continue;
            }

            if c == '\'' {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                current_token.push('\'');
                in_single = true;
                i += 1;
                continue;
            }
            if c == '"' {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                current_token.push('"');
                in_double = true;
                i += 1;
                continue;
            }

            if c == '`' {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                let start = i;
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\\' {
                        i = (i + 2).min(chars.len());
                        continue;
                    }
                    if chars[i] == '`' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                styled.push((string_style, chars[start..i].iter().collect()));
                continue;
            }

            if c == '|' {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                styled.push((pipe_style, "|".to_string()));
                is_first = true;
                i += 1;
                continue;
            }

            if c == ';' || c == '&' {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                let mut text = String::from(c);
                i += 1;
                if c == '&' && i < chars.len() && chars[i] == '&' {
                    text.push('&');
                    i += 1;
                }
                styled.push((plain_style, text));
                is_first = true;
                continue;
            }

            if c == '\n' {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                styled.push((plain_style, "\n".to_string()));
                is_first = true;
                i += 1;
                continue;
            }

            if c.is_whitespace() {
                self.flush_token(&mut current_token, &mut styled, &mut is_first);
                styled.push((plain_style, c.to_string()));
                i += 1;
                continue;
            }

            if c == '\\' {
                current_token.push(c);
                i += 1;
                if i < chars.len() {
                    current_token.push(chars[i]);
                    i += 1;
                }
                continue;
            }

            current_token.push(c);
            i += 1;
        }

        if let Some((start, _)) = triple {
            styled.push((string_style, chars[start..].iter().collect()));
        } else if in_single || in_double {
            if !current_token.is_empty() {
                styled.push((string_style, current_token));
            }
        } else {
            self.flush_token(&mut current_token, &mut styled, &mut is_first);
        }

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
        let input = "cat << EOF\nhello\nEOF\n";
        let result = hl.highlight(input, 0);
        assert_eq!(result.raw_string(), input);
        assert!(
            result.buffer.iter().any(|(_, s)| s.contains("EOF")),
            "heredoc marker should be highlighted: {:?}",
            result.buffer
        );
    }

    #[test]
    fn test_highlight_is_lossless() {
        let hl = FshellHighlighter::new(vec!["ls".to_string()]);
        for input in [
            "cat <<EOF\n    indented\n      more\nEOF\n",
            "cat <<-'EOF'\n\tvalue\n\tEOF\n",
            "cat <<'EOF'\n$HOME stays literal\nEOF\n",
            "cat <<EOF\nbody with } and \"quotes\"\nEOF\n",
            "let q = \"\"\"\n  multi\n  line\n\"\"\"\necho q\n",
            "let q = '''\nraw\n'''\n",
            "echo \"a\nb\" 'c\nd'\n",
            "echo $(date) ${HOME:-/tmp} $((1 << 2))\n",
            "# comment\nls\n",
            "awk '{print $1}' file\n",
            "let x = r#\"a \"b\" c\"#\n",
            "cat <<EOF\r\nbody\r\nEOF\r\n",
            "echo `date\n +%s`\n",
        ] {
            assert_eq!(
                hl.highlight(input, 0).raw_string(),
                input,
                "highlighting lost characters for {input:?}"
            );
        }
    }

    #[test]
    fn test_highlight_heredoc_state_does_not_leak() {
        let hl = FshellHighlighter::new(vec!["ls".to_string()]);
        let theme = Theme::default_theme();

        let heredoc = hl.highlight("cat <<EOF\nbody\nEOF\n", 0);
        assert_eq!(heredoc.raw_string(), "cat <<EOF\nbody\nEOF\n");

        let next = hl.highlight("ls\n", 0);
        let ls = next
            .buffer
            .iter()
            .find(|(_, s)| s == "ls")
            .expect("ls span");
        assert_ne!(
            ls.0,
            theme.syntax.string.to_style(),
            "a previous heredoc must not tint the next buffer"
        );
    }

    #[test]
    fn test_highlight_heredoc_terminator_is_the_whole_line() {
        let hl = FshellHighlighter::new(vec!["ls".to_string()]);
        let theme = Theme::default_theme();
        let input = "cat <<EOF\nEOF   \nEOF\nls\n";
        let result = hl.highlight(input, 0);
        assert_eq!(result.raw_string(), input);
        let ls = result
            .buffer
            .iter()
            .find(|(_, s)| s == "ls")
            .expect("ls span");
        assert_ne!(
            ls.0,
            theme.syntax.string.to_style(),
            "only an exact delimiter line ends the body"
        );
    }

    #[test]
    fn test_highlight_triple_quoted_body_is_one_string_span() {
        let hl = FshellHighlighter::new(vec!["echo".to_string()]);
        let theme = Theme::default_theme();
        let input = "let q = \"\"\"\n  multi\n  line\n\"\"\"\necho q\n";
        let result = hl.highlight(input, 0);
        assert_eq!(result.raw_string(), input);
        let body = result
            .buffer
            .iter()
            .find(|(_, s)| s.contains("multi"))
            .expect("body span");
        assert_eq!(body.0, theme.syntax.string.to_style());
        let echo = result
            .buffer
            .iter()
            .find(|(_, s)| s == "echo")
            .expect("echo");
        assert_eq!(echo.0, theme.syntax.builtin.to_style_bold());
    }

    #[test]
    fn test_highlight_comment_stops_at_the_newline() {
        let hl = FshellHighlighter::new(vec!["echo".to_string()]);
        let theme = Theme::default_theme();
        let input = "# comment\necho hi\n";
        let result = hl.highlight(input, 0);
        assert_eq!(result.raw_string(), input);
        let echo = result
            .buffer
            .iter()
            .find(|(_, s)| s == "echo")
            .expect("echo");
        assert_eq!(echo.0, theme.syntax.builtin.to_style_bold());
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
