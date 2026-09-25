// Copyright (C) 2025 Francesco Duca <f.duca00@gmail.com>

//! A word's text with its source quoting still attached.
//!
//! Quoting is a property of how a word was *written*, not of the value it
//! produces, so it must not live on `Val`: a value layer carrying parser
//! provenance would leak syntax into every consumer of `Val`. Expansion that
//! has to respect quoting — globbing and brace expansion here, field splitting
//! in future — runs on this intermediate form instead:
//!
//! ```text
//! StringPart[]  ->  interpolate  ->  ExpandedWord  ->  argv / Val
//! ```
//!
//! Per-fragment granularity is the point. `foo'*'*.rs` is one word whose middle
//! `*` is data and whose trailing `*.rs` is a pattern, so a per-word "was this
//! quoted" boolean cannot describe it.

use fshell_core::QuoteKind;

/// Characters that are pattern syntax in unquoted source.
fn is_glob_meta(c: char) -> bool {
    matches!(c, '*' | '?' | '[' | ']' | '{' | '}')
}

/// One contiguous run of a word written with a single quoting form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedFragment {
    pub text: String,
    pub quote: QuoteKind,
}

impl ExpandedFragment {
    pub fn new(text: impl Into<String>, quote: QuoteKind) -> Self {
        Self {
            text: text.into(),
            quote,
        }
    }

    /// Whether this fragment's characters are data rather than pattern syntax.
    pub fn is_literal(&self) -> bool {
        self.quote.is_literal()
    }

    /// Render this fragment for a glob pattern.
    ///
    /// Literal fragments are escaped so the matcher reads their characters as
    /// data. `Escaped` fragments hold escape syntax the parser already
    /// normalised (`\*` stays escaped, while a de-escaped `\[` gains one), so
    /// their existing backslashes are left in place rather than doubled.
    fn pattern(&self) -> String {
        if !self.is_literal() {
            return self.text.clone();
        }
        let mut out = String::with_capacity(self.text.len());
        let mut chars = self.text.chars().peekable();
        while let Some(c) = chars.next() {
            if self.quote == QuoteKind::Escaped && c == '\\' {
                // Already an escape: copy it and whatever it escapes verbatim.
                out.push(c);
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
                continue;
            }
            if is_glob_meta(c) || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
        out
    }
}

/// A word after interpolation, before it becomes argv.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExpandedWord {
    pub fragments: Vec<ExpandedFragment>,
}

impl ExpandedWord {
    pub fn new(fragments: Vec<ExpandedFragment>) -> Self {
        Self { fragments }
    }

    /// A word of unquoted source text.
    pub fn push(&mut self, text: impl Into<String>, quote: QuoteKind) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        self.fragments.push(ExpandedFragment::new(text, quote));
    }

    /// The word's value: its fragments concatenated.
    ///
    /// This is what the word means as data. Quoting never changes it — only
    /// which characters are pattern syntax, which is what the other methods
    /// describe.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for f in &self.fragments {
            out.push_str(&f.text);
        }
        out
    }

    /// Whether this word must go through pattern processing.
    ///
    /// True when an unquoted fragment carries pattern syntax, or when the word
    /// holds escaped source at all — the matcher is what turns the parser's
    /// retained `\*` into a literal `*`, and a de-escaped `\[` back into data.
    /// A word that is purely quoted has neither, and must reach argv untouched:
    /// that is what makes `'*.rs'` an argument rather than a file list. Tilde
    /// expansion is only active in the leading unquoted fragment, which is
    /// where a shell looks for it.
    pub fn needs_expansion(&self) -> bool {
        for (index, f) in self.fragments.iter().enumerate() {
            match f.quote {
                QuoteKind::Unquoted => {
                    if f.text.contains(is_glob_meta) || f.text.contains("..") {
                        return true;
                    }
                    if index == 0 && f.text.starts_with('~') {
                        return true;
                    }
                }
                QuoteKind::Escaped => return true,
                QuoteKind::Single | QuoteKind::Double => {}
            }
        }
        false
    }

    /// This word as a pattern, with literal fragments escaped.
    pub fn glob_pattern(&self) -> String {
        let mut out = String::new();
        for f in &self.fragments {
            out.push_str(&f.pattern());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fshell_core::QuoteKind::{Double, Escaped, Single, Unquoted};

    fn word(fragments: &[(&str, QuoteKind)]) -> ExpandedWord {
        ExpandedWord::new(
            fragments
                .iter()
                .map(|(t, q)| ExpandedFragment::new(*t, *q))
                .collect(),
        )
    }

    #[test]
    fn text_concatenates_regardless_of_quoting() {
        let w = word(&[("foo", Unquoted), ("*", Single), ("*.rs", Unquoted)]);
        assert_eq!(w.text(), "foo**.rs");
    }

    #[test]
    fn fully_quoted_word_has_no_active_expansion() {
        assert!(!word(&[("*", Single), (".rs", Unquoted)]).needs_expansion());
        assert!(!word(&[("*.rs", Double)]).needs_expansion());
        assert!(!word(&[("a;b|c>d", Single)]).needs_expansion());
        assert!(!word(&[("[a,b]", Single)]).needs_expansion());
    }

    #[test]
    fn escaped_words_still_need_the_matcher() {
        // The parser keeps the backslash; the matcher is what resolves it.
        assert!(word(&[("\\*", Escaped), (".rs", Unquoted)]).needs_expansion());
        assert!(word(&[("a", Unquoted), (" ", Escaped), ("b", Unquoted)]).needs_expansion());
    }

    #[test]
    fn unquoted_metacharacters_are_active() {
        assert!(word(&[("*.rs", Unquoted)]).needs_expansion());
        assert!(word(&[("foo", Unquoted), ("*", Single), ("*.rs", Unquoted)]).needs_expansion());
        assert!(word(&[("{a,b}", Unquoted)]).needs_expansion());
        assert!(word(&[("~", Unquoted), ("/src", Unquoted)]).needs_expansion());
    }

    #[test]
    fn quoted_tilde_does_not_expand_but_unquoted_does() {
        assert!(!word(&[("~", Single), ("/src", Unquoted)]).needs_expansion());
        assert!(word(&[("~/src", Unquoted)]).needs_expansion());
    }

    #[test]
    fn a_tilde_that_is_not_leading_is_not_expansion() {
        assert!(!word(&[("a~b", Unquoted)]).needs_expansion());
    }

    #[test]
    fn pattern_escapes_literal_fragments_only() {
        let w = word(&[("foo", Unquoted), ("*", Single), ("*.rs", Unquoted)]);
        assert_eq!(w.glob_pattern(), "foo\\**.rs");
    }

    #[test]
    fn pattern_keeps_escaped_fragments_as_written() {
        // The parser retained the backslash: the matcher must see one escape.
        assert_eq!(
            word(&[("\\*", Escaped), (".rs", Unquoted)]).glob_pattern(),
            "\\*.rs"
        );
        // A de-escaped metacharacter needs one added, or it would open a class.
        assert_eq!(
            word(&[("[", Escaped), ("a", Unquoted)]).glob_pattern(),
            "\\[a"
        );
    }

    #[test]
    fn pattern_escapes_backslashes_inside_quoted_text() {
        assert_eq!(
            word(&[("a\\b", Single), ("*.rs", Unquoted)]).glob_pattern(),
            "a\\\\b*.rs"
        );
    }
}
