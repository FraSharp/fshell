// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use nucleo_matcher::{Config, Matcher, Utf32String};
use std::cell::RefCell;

thread_local! {
    /// Cached Matcher per thread. Each tokio blocking thread gets its own
    /// instance lazily on first use — the common single-threaded REPL path
    /// allocates only once.
    static SMART_MATCHER: RefCell<Matcher> = RefCell::new(Matcher::new(Config::DEFAULT));
}

#[derive(Clone, Copy)]
pub enum FuzzyKind {
    Simple,
    Smart,
}

pub const MAX_RESULTS: usize = 50;
const SIMPLE_CANDIDATE_LIMIT: usize = 200;

pub struct PreparedQuery {
    query: String,
    query_lower: String,
    query_utf32: Utf32String,
}

impl PreparedQuery {
    pub fn new(query: &str) -> Self {
        Self {
            query: query.to_string(),
            query_lower: query.to_lowercase(),
            query_utf32: Utf32String::from(query),
        }
    }
}

/// Score a candidate against a prepared query. Returns `None` if below threshold (no match).
pub fn fuzzy_score_prepared(
    prepared: &PreparedQuery,
    candidate: &str,
    kind: FuzzyKind,
) -> Option<isize> {
    if prepared.query.is_empty() {
        return Some(0);
    }
    match kind {
        FuzzyKind::Simple => {
            let text_lower = candidate.to_lowercase();
            if let Some(pos) = text_lower.find(&prepared.query_lower) {
                let score = 1000 - (pos as isize) - (candidate.len() as isize);
                return Some(score);
            }
            if is_subsequence(&prepared.query_lower, &text_lower) {
                let score = 500 - (candidate.len() as isize);
                return Some(score);
            }
            None
        }
        FuzzyKind::Smart => SMART_MATCHER.with(|m| {
            let mut matcher = m.borrow_mut();
            let haystack = Utf32String::from(candidate);
            matcher
                .fuzzy_match(haystack.slice(..), prepared.query_utf32.slice(..))
                .map(|s| s as isize)
        }),
    }
}

/// Score a candidate against a query. Returns `None` if below threshold (no match).
pub fn fuzzy_score(query: &str, candidate: &str, kind: FuzzyKind) -> Option<isize> {
    let prepared = PreparedQuery::new(query);
    fuzzy_score_prepared(&prepared, candidate, kind)
}

fn is_subsequence(query: &str, text: &str) -> bool {
    let mut query_chars = query.chars();
    let mut next_char = query_chars.next();
    for c in text.chars() {
        if let Some(qc) = next_char {
            if c == qc {
                next_char = query_chars.next();
            }
        } else {
            return true;
        }
    }
    next_char.is_none()
}

pub fn choose_kind(candidate_count: usize) -> FuzzyKind {
    if candidate_count <= SIMPLE_CANDIDATE_LIMIT {
        FuzzyKind::Simple
    } else {
        FuzzyKind::Smart
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_fuzzy_score(query: &str, candidate: &str) -> Option<isize> {
        fuzzy_score(query, candidate, FuzzyKind::Simple)
    }

    fn smart_fuzzy_score(query: &str, candidate: &str) -> Option<isize> {
        fuzzy_score(query, candidate, FuzzyKind::Smart)
    }

    #[test]
    fn test_simple_exact_substring_scores_higher() {
        let a = simple_fuzzy_score("lib", "src/lib.rs").unwrap();
        let b = simple_fuzzy_score("lib", "src/foobar.rs");
        assert!(a > 0);
        assert!(b.is_none());
    }

    #[test]
    fn test_simple_subsequence_match() {
        let score = simple_fuzzy_score("srclib", "src/lib.rs");
        assert!(score.is_some());
        assert!(score.unwrap() > 0);
    }

    #[test]
    fn test_simple_no_match() {
        assert!(simple_fuzzy_score("xyz", "hello").is_none());
    }

    #[test]
    fn test_smart_fuzzy_match() {
        let score = smart_fuzzy_score("cargo", "cargo/");
        assert!(score.is_some(), "should match 'cargo' in 'cargo/'");
        assert!(score.unwrap() > 0, "score should be positive");
    }

    #[test]
    fn test_smart_fuzzy_subsequence() {
        let score = smart_fuzzy_score("abc", "axbyc");
        assert!(
            score.is_some(),
            "should match 'abc' as subsequence of 'axbyc'"
        );
    }

    #[test]
    fn test_smart_fuzzy_no_match() {
        let score = smart_fuzzy_score("zzzzzz", "hello");
        assert!(score.is_none(), "should not match 'zzzzzz' in 'hello'");
    }

    #[test]
    fn test_empty_query_returns_match() {
        assert!(fuzzy_score("", "anything", FuzzyKind::Simple).is_some());
        assert!(fuzzy_score("", "anything", FuzzyKind::Smart).is_some());
    }

    #[test]
    fn test_choose_kind_small_is_simple() {
        assert!(matches!(choose_kind(10), FuzzyKind::Simple));
    }

    #[test]
    fn test_choose_kind_large_is_smart() {
        assert!(matches!(choose_kind(500), FuzzyKind::Smart));
    }

    #[test]
    fn test_com_query_matches_commandcode() {
        // Regression: .com should match .commandcode (substring at pos 0)
        let score = simple_fuzzy_score(".com", ".commandcode");
        assert!(
            score.is_some(),
            ".com should match .commandcode as substring, got None"
        );
        if let Some(s) = score {
            assert!(s > 0, "score should be positive, got {}", s);
        }

        let score_smart = smart_fuzzy_score(".com", ".commandcode");
        assert!(
            score_smart.is_some(),
            ".com should match .commandcode with smart matcher, got None"
        );

        let score_com = smart_fuzzy_score("com", ".commandcode");
        assert!(
            score_com.is_some(),
            "com should match .commandcode with smart matcher, got None"
        );

        let score_co = smart_fuzzy_score("co", ".commandcode");
        assert!(
            score_co.is_some(),
            "co should match .commandcode with smart matcher, got None"
        );
    }

    #[test]
    fn test_subsequence_fn() {
        assert!(is_subsequence("abc", "axbxc"));
        assert!(is_subsequence("main", "src/main.rs"));
        assert!(!is_subsequence("abc", "ab"));
        assert!(!is_subsequence("xyz", "xby"));
    }
}
