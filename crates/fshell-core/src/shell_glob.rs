// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Shell pattern matching for one path component.
//!
//! The pattern grammar is the shell's, not a regular expression's: `*` matches
//! any run of characters, `?` any one character, `[...]` a bracket expression
//! with `!`/`^` negation, ranges and POSIX character classes, and `\` quotes
//! the character after it. An unclosed `[` is an ordinary character, as it is
//! in every shell.
//!
//! Both engines expand pathnames, and they must agree on what a pattern means,
//! so this is the single matcher they share. Traversal — which directories are
//! read and how matches are rendered — stays with each engine.

/// A POSIX bracket-expression character class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosixClass {
    Alnum,
    Alpha,
    Blank,
    Cntrl,
    Digit,
    Graph,
    Lower,
    Print,
    Punct,
    Space,
    Upper,
    Xdigit,
}

impl PosixClass {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "alnum" => Self::Alnum,
            "alpha" => Self::Alpha,
            "blank" => Self::Blank,
            "cntrl" => Self::Cntrl,
            "digit" => Self::Digit,
            "graph" => Self::Graph,
            "lower" => Self::Lower,
            "print" => Self::Print,
            "punct" => Self::Punct,
            "space" => Self::Space,
            "upper" => Self::Upper,
            "xdigit" => Self::Xdigit,
            _ => return None,
        })
    }

    fn matches(self, c: char) -> bool {
        match self {
            Self::Alnum => c.is_alphanumeric(),
            Self::Alpha => c.is_alphabetic(),
            Self::Blank => matches!(c, ' ' | '\t'),
            Self::Cntrl => c.is_control(),
            Self::Digit => c.is_ascii_digit(),
            Self::Graph => !c.is_control() && !c.is_whitespace(),
            Self::Lower => c.is_lowercase(),
            Self::Print => !c.is_control(),
            Self::Punct => c.is_ascii_punctuation(),
            Self::Space => c.is_whitespace(),
            Self::Upper => c.is_uppercase(),
            Self::Xdigit => c.is_ascii_hexdigit(),
        }
    }
}

#[derive(Debug)]
enum ClassItem {
    Char(char),
    Range(char, char),
    Posix(PosixClass),
}

#[derive(Debug)]
struct Class {
    negated: bool,
    items: Vec<ClassItem>,
}

#[derive(Debug)]
enum Token {
    Literal(char),
    AnyChar,
    AnyRun,
    Class(Class),
}

fn chars_equal(a: char, b: char, case_insensitive: bool) -> bool {
    if case_insensitive {
        a.to_lowercase().eq(b.to_lowercase())
    } else {
        a == b
    }
}

fn parse_class(pattern: &[char], start: usize) -> Option<(Class, usize)> {
    let mut i = start + 1;
    let negated = matches!(pattern.get(i), Some('!') | Some('^'));
    if negated {
        i += 1;
    }
    let mut items = Vec::new();
    let mut first = true;
    loop {
        let c = *pattern.get(i)?;
        if c == ']' && !first {
            return Some((Class { negated, items }, i + 1));
        }
        first = false;
        // `[:alpha:]` and friends.
        if c == '[' && pattern.get(i + 1) == Some(&':') {
            if let Some(offset) = pattern[i + 2..]
                .windows(2)
                .position(|window| window == [':', ']'])
            {
                let name: String = pattern[i + 2..i + 2 + offset].iter().collect();
                let class = PosixClass::parse(&name)?;
                items.push(ClassItem::Posix(class));
                i += 2 + offset + 2;
                continue;
            }
        }
        if c == '\\' {
            items.push(ClassItem::Char(*pattern.get(i + 1)?));
            i += 2;
            continue;
        }
        // A range needs both endpoints inside the brackets: `[a-]` is a
        // literal `a` and `-`.
        if pattern.get(i + 1) == Some(&'-') && pattern.get(i + 2).is_some_and(|end| *end != ']') {
            items.push(ClassItem::Range(c, pattern[i + 2]));
            i += 3;
            continue;
        }
        items.push(ClassItem::Char(c));
        i += 1;
    }
}

/// The end (exclusive) of the bracket expression starting at `start`, or
/// `start + 1` when it has no closing `]`.
fn class_end(pattern: &[char], start: usize) -> usize {
    let mut i = start + 1;
    if matches!(pattern.get(i), Some('!') | Some('^')) {
        i += 1;
    }
    if pattern.get(i) == Some(&']') {
        i += 1;
    }
    while i < pattern.len() {
        if pattern[i] == '\\' {
            i += 2;
            continue;
        }
        if pattern[i] == ']' {
            return i + 1;
        }
        i += 1;
    }
    start + 1
}

fn parse_pattern(pattern: &str) -> Vec<Token> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                tokens.push(Token::Literal(chars[i + 1]));
                i += 2;
            }
            '*' => {
                // `**` is still one run: a component has no separator to cross.
                if !matches!(tokens.last(), Some(Token::AnyRun)) {
                    tokens.push(Token::AnyRun);
                }
                i += 1;
            }
            '?' => {
                tokens.push(Token::AnyChar);
                i += 1;
            }
            '[' => match parse_class(&chars, i) {
                Some((class, next)) => {
                    tokens.push(Token::Class(class));
                    i = next;
                }
                None => {
                    // An unclosed or invalid bracket expression is literal text,
                    // all of it: `[[:bogus:]]` names exactly those characters.
                    let end = class_end(&chars, i);
                    tokens.extend(chars[i..end].iter().map(|c| Token::Literal(*c)));
                    i = end;
                }
            },
            c => {
                tokens.push(Token::Literal(c));
                i += 1;
            }
        }
    }
    tokens
}

fn class_matches(class: &Class, c: char, case_insensitive: bool) -> bool {
    let matched = class.items.iter().any(|item| match item {
        ClassItem::Char(item) => chars_equal(*item, c, case_insensitive),
        ClassItem::Range(start, end) => {
            let (c, start, end) = if case_insensitive {
                let c = c.to_lowercase().next().unwrap_or(c);
                let start = start.to_lowercase().next().unwrap_or(*start);
                let end = end.to_lowercase().next().unwrap_or(*end);
                (c, start, end)
            } else {
                (c, *start, *end)
            };
            start <= c && c <= end
        }
        ClassItem::Posix(posix) => {
            if case_insensitive {
                match posix {
                    // The case classes name both halves of the alphabet.
                    PosixClass::Upper | PosixClass::Lower => c.is_alphabetic(),
                    other => other.matches(c),
                }
            } else {
                posix.matches(c)
            }
        }
    });
    matched != class.negated
}

/// Whether `name` matches the shell pattern `pattern`.
///
/// `pattern` is one path component: `/` is an ordinary character here, and a
/// separator is matched by the traversal, not by `*`.
pub fn component_matches(pattern: &str, name: &str, case_insensitive: bool) -> bool {
    let tokens = parse_pattern(pattern);
    let chars: Vec<char> = name.chars().collect();
    // dp[i][j]: the pattern's suffix from token i matches the name's suffix
    // from character j.
    let mut dp = vec![vec![false; chars.len() + 1]; tokens.len() + 1];
    dp[tokens.len()][chars.len()] = true;
    for i in (0..tokens.len()).rev() {
        for j in (0..=chars.len()).rev() {
            dp[i][j] = match &tokens[i] {
                Token::Literal(expected) => {
                    j < chars.len()
                        && chars_equal(*expected, chars[j], case_insensitive)
                        && dp[i + 1][j + 1]
                }
                Token::AnyChar => j < chars.len() && dp[i + 1][j + 1],
                Token::AnyRun => dp[i + 1][j] || (j < chars.len() && dp[i][j + 1]),
                Token::Class(class) => {
                    j < chars.len()
                        && class_matches(class, chars[j], case_insensitive)
                        && dp[i + 1][j + 1]
                }
            };
        }
    }
    dp[0][0]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, name: &str) -> bool {
        component_matches(pattern, name, false)
    }

    #[test]
    fn wildcards_and_literals() {
        assert!(matches("*", "anything"));
        assert!(matches("*.rs", "main.rs"));
        assert!(!matches("*.rs", "main.rb"));
        assert!(matches("?ain.rs", "main.rs"));
        assert!(!matches("?ain.rs", "mmain.rs"));
        assert!(matches("**", "a"));
        assert!(matches("a*b*c", "axxbyyc"));
        assert!(!matches("a*b*c", "axxbyy"));
        assert!(matches("", ""));
        assert!(!matches("", "x"));
    }

    #[test]
    fn brackets() {
        assert!(matches("[abc].txt", "b.txt"));
        assert!(!matches("[abc].txt", "d.txt"));
        assert!(matches("[!abc].txt", "d.txt"));
        assert!(!matches("[!abc].txt", "a.txt"));
        assert!(matches("[^abc].txt", "d.txt"));
        assert!(matches("[a-c].txt", "b.txt"));
        assert!(!matches("[a-c].txt", "d.txt"));
        assert!(matches("[]a].txt", "].txt"));
        assert!(matches("[a-].txt", "-.txt"));
        // An unclosed bracket is a literal.
        assert!(matches("[abc", "[abc"));
        assert!(!matches("[abc", "a"));
    }

    #[test]
    fn posix_classes() {
        assert!(matches("[[:alpha:]].txt", "a.txt"));
        assert!(!matches("[[:alpha:]].txt", "1.txt"));
        assert!(matches("[[:digit:]].txt", "1.txt"));
        assert!(matches("[[:alnum:]].txt", "z.txt"));
        assert!(matches("[[:upper:]].txt", "Z.txt"));
        assert!(!matches("[[:upper:]].txt", "z.txt"));
        assert!(matches("[[:xdigit:]].txt", "f.txt"));
        assert!(matches("[![:digit:]].txt", "a.txt"));
        assert!(!matches("[![:digit:]].txt", "1.txt"));
        // An unknown class name leaves the bracket expression literal.
        assert!(matches("[[:bogus:]]", "[[:bogus:]]"));
    }

    #[test]
    fn escapes() {
        assert!(matches(r"\*.txt", "*.txt"));
        assert!(!matches(r"\*.txt", "a.txt"));
        assert!(matches(r"a\[b", "a[b"));
        assert!(matches(r"a\\b", r"a\b"));
    }

    #[test]
    fn case_insensitive() {
        assert!(component_matches("*.RS", "main.rs", true));
        assert!(component_matches("[a-z].rs", "A.RS", true));
        assert!(!component_matches("*.RS", "main.rs", false));
    }
}
