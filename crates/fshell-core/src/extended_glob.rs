// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

fn alts_escaped(alts: &[String]) -> Vec<String> {
    alts.iter().map(|a| regex::escape(a)).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExtendedGlob {
    /// *(foo|bar) — zero or more repetitions
    Group(Vec<String>),
    /// !(foo|bar) or ^foo — negated
    Negated(Vec<String>),
    /// ?(foo|bar) — zero or one
    ZeroOrOne(Vec<String>),
    /// +(foo|bar) — one or more
    OneOrMore(Vec<String>),
}

impl ExtendedGlob {
    /// Parse an extended glob pattern from a string like "*(foo|bar)"
    pub fn parse(pattern: &str) -> Option<Self> {
        // Handle ^foo shorthand (no parens needed)
        if pattern.starts_with('^') && pattern.len() > 1 {
            return Some(Self::Negated(vec![pattern[1..].to_string()]));
        }
        if pattern.len() < 3 {
            return None;
        }
        let first = pattern.chars().next()?;
        let prefix_len = first.len_utf8();
        let rest = &pattern[prefix_len..];
        if !rest.ends_with(')') {
            return None;
        }
        let inner = &rest[1..rest.len() - 1]; // strip leading char and trailing )

        match first {
            '*' => Some(Self::Group(Self::parse_alternatives(inner))),
            '!' => Some(Self::Negated(Self::parse_alternatives(inner))),
            '?' => Some(Self::ZeroOrOne(Self::parse_alternatives(inner))),
            '+' => Some(Self::OneOrMore(Self::parse_alternatives(inner))),
            _ => None,
        }
    }

    fn parse_alternatives(s: &str) -> Vec<String> {
        if s.is_empty() {
            return vec![];
        }
        // split() naturally yields an empty trailing element for "a|b|"
        s.split('|').map(|p| p.to_string()).collect()
    }

    /// Compile this extended glob to a regex pattern (escaped, safe).
    /// Caller is responsible for `Regex::new` compilation; this returns only
    /// the pattern string. Alternates are escaped so `a.b` is literal.
    pub fn to_regex(&self) -> String {
        // Keep patterns bounded — huge alternates are a ReDoS / memory hazard.
        const MAX_ALTS: usize = 128;
        const MAX_TOTAL_LEN: usize = 2000;
        let escaped: Vec<String> = alts_escaped(self.alternatives())
            .into_iter()
            .take(MAX_ALTS)
            .collect();
        let total_len: usize = escaped.iter().map(|s| s.len()).sum();
        // Truncate silently would change semantics — instead degrade to a
        // pattern that can never be compiled, caller will fallback.
        if total_len > MAX_TOTAL_LEN {
            return "[invalid-extended-glob-too-large".to_string(); // unclosed class — Regex::new will error
        }
        let alts_str = escaped.join("|");
        match self {
            Self::Group(_) => format!("^({})*$", alts_str),
            Self::Negated(_) => format!("^(?!({})$).*$", alts_str),
            Self::ZeroOrOne(_) => format!("^({})?$", alts_str),
            Self::OneOrMore(_) => format!("^({})+$", alts_str),
        }
    }

    fn alternatives(&self) -> &[String] {
        match self {
            Self::Group(v) | Self::Negated(v) | Self::ZeroOrOne(v) | Self::OneOrMore(v) => v,
        }
    }

    /// Check if a string matches this extended glob.
    /// Uses DP partitioning for Group/OneOrMore to avoid ReDoS-prone nested
    /// quantifiers; escapes are literal. Negated is exact string exclusion.
    pub fn matches(&self, s: &str) -> bool {
        match self {
            Self::Negated(alts) => !alts.iter().any(|a| a == s),
            Self::ZeroOrOne(alts) => s.is_empty() || alts.iter().any(|a| a == s),
            Self::Group(alts) => Self::matches_partition(s, alts, true),
            Self::OneOrMore(alts) => {
                if s.is_empty() {
                    // Only matches if empty alternative exists
                    return alts.iter().any(|a| a.is_empty());
                }
                Self::matches_partition(s, alts, false)
            }
        }
    }

    fn matches_partition(s: &str, alts: &[String], allow_empty: bool) -> bool {
        if s.is_empty() {
            return allow_empty;
        }
        // Ignore empty alternatives for partitioning — they match zero chars
        // and would cause infinite loops; zero-length consumption is already
        // represented by allow_empty / repetitions.
        let needles: Vec<&str> = alts
            .iter()
            .map(|a| a.as_str())
            .filter(|a| !a.is_empty())
            .collect();
        if needles.is_empty() {
            return allow_empty && s.is_empty();
        }
        // Guard against pathological input (ReDoS via many alts).
        if s.len() > 8192 || needles.len() > 128 {
            return false;
        }
        let n = s.len();
        let mut dp = vec![false; n + 1];
        dp[0] = true;
        for i in 0..=n {
            if !dp[i] {
                continue;
            }
            if i == n {
                continue;
            }
            for needle in &needles {
                if s[i..].starts_with(needle) {
                    dp[i + needle.len()] = true;
                }
            }
        }
        dp[n]
    }
}

/// Check if a glob pattern contains extended glob syntax
pub fn has_extended_glob(pattern: &str) -> bool {
    if pattern.starts_with("*>(")
        || pattern.starts_with("!(")
        || pattern.starts_with("?(")
        || pattern.starts_with("+(")
        || pattern.starts_with('^')
    {
        return true;
    }
    // *( could be either a qualifier or extended glob — check for | inside parens
    if pattern.starts_with("*(") && pattern.ends_with(')') {
        let inner = &pattern[2..pattern.len() - 1];
        return inner.contains('|');
    }
    false
}

/// Check if a pattern after *( is a qualifier or an extended glob
pub fn is_glob_qualifier(pattern: &str) -> bool {
    // Qualifiers use specific chars: . / @ * = p % L m o s
    // Extended globs use |
    if pattern.contains('|') {
        return false;
    }
    // Check for qualifier characters
    pattern.chars().any(|c| {
        matches!(
            c,
            '.' | '/' | '@' | '*' | '=' | 'p' | '%' | 'L' | 'm' | 'o' | 's'
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_glob_group_matches() {
        let glob = ExtendedGlob::parse("*(foo|bar)").unwrap();
        assert!(glob.matches(""));
        assert!(glob.matches("foo"));
        assert!(glob.matches("bar"));
        assert!(glob.matches("foofoo"));
        assert!(glob.matches("foobar"));
        assert!(!glob.matches("baz"));
    }

    #[test]
    fn extended_glob_negated_matches() {
        let glob = ExtendedGlob::parse("!(foo|bar)").unwrap();
        assert!(!glob.matches("foo"));
        assert!(!glob.matches("bar"));
        assert!(glob.matches("baz"));
        assert!(glob.matches(""));
    }

    #[test]
    fn extended_glob_zero_or_one_matches() {
        let glob = ExtendedGlob::parse("?(foo|bar)").unwrap();
        assert!(glob.matches(""));
        assert!(glob.matches("foo"));
        assert!(glob.matches("bar"));
        assert!(!glob.matches("foofoo"));
    }

    #[test]
    fn extended_glob_one_or_more_matches() {
        let glob = ExtendedGlob::parse("+(foo|bar)").unwrap();
        assert!(!glob.matches(""));
        assert!(glob.matches("foo"));
        assert!(glob.matches("bar"));
        assert!(glob.matches("foofoo"));
        assert!(glob.matches("foobar"));
    }

    #[test]
    fn extended_glob_shorthand_negated() {
        let glob = ExtendedGlob::parse("^foo").unwrap();
        assert!(!glob.matches("foo"));
        assert!(glob.matches("bar"));
        assert!(glob.matches(""));
    }

    #[test]
    fn extended_glob_trailing_pipe() {
        let glob = ExtendedGlob::parse("*(foo|bar|)").unwrap();
        assert!(glob.matches(""));
        assert!(glob.matches("foo"));
        assert!(glob.matches("bar"));
        assert!(glob.matches("foofoo"));
    }

    #[test]
    fn has_extended_glob_detection() {
        assert!(has_extended_glob("*(foo|bar)"));
        assert!(has_extended_glob("!(foo|bar)"));
        assert!(has_extended_glob("?(foo|bar)"));
        assert!(has_extended_glob("+(foo|bar)"));
        assert!(has_extended_glob("^foo"));
        assert!(!has_extended_glob("*(.)"));
        assert!(!has_extended_glob("*.rs"));
    }
}
