// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Authoritative fuzzy ranking and scoring engine for completions.

use super::types::{CompletionCandidate, CompletionKind};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Matcher, Utf32String};
use std::collections::HashSet;

/// A completion candidate with authoritative score and match indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedCompletion {
    pub candidate: CompletionCandidate,
    pub score: i64,
    pub match_indices: Vec<usize>,
}

/// Authoritative ranking engine combining Nucleo fuzzy matching with
/// exact, prefix, word-boundary, and contextual bonuses.
pub fn rank_candidates(
    candidates: Vec<CompletionCandidate>,
    query: &str,
    matcher: &mut Matcher,
    recent_commands: Option<&HashSet<String>>,
) -> Vec<RankedCompletion> {
    if candidates.is_empty() {
        return Vec::new();
    }

    if query.is_empty() {
        // Empty query: preserve base source relevance and alphabetical order
        let mut ranked: Vec<RankedCompletion> = candidates
            .into_iter()
            .map(|c| {
                let base_score = match c.kind {
                    CompletionKind::Builtin | CompletionKind::UserFunction => 2000,
                    CompletionKind::ExternalCommand => 1000,
                    CompletionKind::Directory => 500,
                    CompletionKind::File => 400,
                    CompletionKind::Flag => 300,
                    _ => 100,
                };
                let boost = if let Some(recent) = recent_commands {
                    if recent.contains(&c.value) { 500 } else { 0 }
                } else {
                    0
                };
                RankedCompletion {
                    candidate: c,
                    score: base_score + boost,
                    match_indices: Vec::new(),
                }
            })
            .collect();

        ranked.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.candidate.value.cmp(&b.candidate.value))
        });

        let mut seen = HashSet::new();
        ranked.retain(|r| seen.insert(r.candidate.value.clone()));
        ranked.truncate(100);
        return ranked;
    }

    let pattern = Pattern::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );

    let query_lower = query.to_lowercase();
    let mut ranked = Vec::new();

    for mut candidate in candidates {
        let haystack = Utf32String::from(candidate.value.as_str());
        let mut indices = Vec::new();
        let nucleo_score = pattern.indices(haystack.slice(..), matcher, &mut indices);

        let Some(n_score) = nucleo_score else {
            continue;
        };

        indices.sort_unstable();
        let match_indices: Vec<usize> = indices.into_iter().map(|idx| idx as usize).collect();

        let mut score = n_score as i64;
        let val_lower = candidate.value.to_lowercase();
        let val_clean = val_lower
            .trim_matches(['"', '\''])
            .strip_prefix('$')
            .unwrap_or(&val_lower);

        // 1. Exact match bonus (highest priority)
        if val_lower == query_lower || val_clean == query_lower {
            score += 100_000;
        } else if val_lower.starts_with(&query_lower) || val_clean.starts_with(&query_lower) {
            // 2. Exact prefix match bonus (with length penalty for tighter matches)
            let len_diff = val_clean.len().saturating_sub(query_lower.len());
            score += 50_000 + (1000 - (len_diff as i64).min(1000));
        } else if val_lower.contains(&query_lower) || val_clean.contains(&query_lower) {
            // 3. Substring match bonus
            score += 20_000;
        }

        // 4. Word boundary match bonus (matches immediately after '-', '_', '/', '.')
        if let Some(&first_idx) = match_indices.first()
            && first_idx > 0
        {
            let prev_char = candidate.value.chars().nth(first_idx.saturating_sub(1));
            if matches!(prev_char, Some('-' | '_' | '/' | '.')) {
                score += 10_000;
            }
        }

        // 5. Semantic / context bonus
        match candidate.kind {
            CompletionKind::Builtin | CompletionKind::UserFunction => score += 2_000,
            CompletionKind::ExternalCommand => score += 1_000,
            CompletionKind::Keyword => score += 500,
            _ => {}
        }

        if let Some(recent) = recent_commands
            && recent.contains(&candidate.value)
        {
            score += 500;
        }

        candidate.match_indices = Some(match_indices.clone());

        ranked.push(RankedCompletion {
            candidate,
            score,
            match_indices,
        });
    }

    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.candidate.value.len().cmp(&b.candidate.value.len()))
            .then_with(|| a.candidate.value.cmp(&b.candidate.value))
    });

    let mut seen = HashSet::new();
    ranked.retain(|r| seen.insert(r.candidate.value.clone()));
    ranked.truncate(100);

    ranked
}
