// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Bounded, cancellable Carapace requests with explicit cache publication.

use super::types::{CompletionCancel, CompletionCandidate, CompletionKind, TextSpan};
use fshell_core::FxIndexMap;
use fshell_core::lock::Mutex;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
enum AvailabilityState {
    Probing,
    Ready(bool),
}
struct Availability {
    path: OsString,
    state: AvailabilityState,
}
static AVAILABILITY: Mutex<Option<Availability>> = Mutex::new(None);

fn search_path() -> OsString {
    std::env::var_os("PATH").unwrap_or_default()
}

pub(crate) fn is_carapace_available_cached() -> Option<bool> {
    let path = search_path();
    let available = AVAILABILITY.lock();
    match available.as_ref() {
        Some(Availability {
            path: cached,
            state: AvailabilityState::Ready(value),
        }) if *cached == path => Some(*value),
        _ => None,
    }
}

fn finish_probe(path: OsString, value: bool) {
    let mut available = AVAILABILITY.lock();
    if let Some(entry) = available.as_mut()
        && entry.path == path
    {
        entry.state = AvailabilityState::Ready(value);
    }
    drop(available);
    super::sources::publish();
}

fn spawn_carapace_availability_probe() {
    let path = search_path();
    {
        let mut available = AVAILABILITY.lock();
        if available.as_ref().is_some_and(|entry| entry.path == path) {
            return;
        }
        *available = Some(Availability {
            path: path.clone(),
            state: AvailabilityState::Probing,
        });
    }
    let probe_path = path.clone();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    if std::thread::Builder::new()
        .name("fsh-carapace-probe".into())
        .spawn(move || {
            let value = super::provider::run_process(
                "carapace",
                &["--version".into()],
                &cwd,
                &probe_path,
                None,
            )
            .is_some();
            finish_probe(probe_path, value);
        })
        .is_err()
    {
        finish_probe(path, false);
    }
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct CacheKey {
    cwd: PathBuf,
    path: OsString,
    args: Vec<String>,
}

impl CacheKey {
    fn new(words: &[&str], last_word: &str, cwd: &Path) -> Option<Self> {
        let command = *words.first()?;
        let mut args = vec![command.to_owned(), "nushell".into()];
        args.extend(words.iter().skip(1).map(|word| (*word).to_owned()));
        if last_word.is_empty() {
            args.push(String::new());
        }
        Some(Self {
            cwd: cwd.to_path_buf(),
            path: search_path(),
            args,
        })
    }
}

struct CacheEntry {
    suggestions: Vec<CompletionCandidate>,
    loaded_at: Instant,
}
const CACHE_TTL: Duration = Duration::from_secs(60);
const MAX_CACHED_REQUESTS: usize = 64;
const MAX_IN_FLIGHT: usize = 4;
static CACHE: LazyLock<Mutex<FxIndexMap<CacheKey, CacheEntry>>> =
    LazyLock::new(|| Mutex::new(FxIndexMap::default()));
static IN_FLIGHT: LazyLock<Mutex<HashSet<CacheKey>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

pub(crate) fn complete_cached_at(
    words: &[&str],
    last_word: &str,
    pos: usize,
    cwd: &Path,
) -> Option<Vec<CompletionCandidate>> {
    if is_carapace_available_cached() != Some(true) {
        return None;
    }
    let key = CacheKey::new(words, last_word, cwd)?;
    let cache = CACHE.lock();
    let entry = cache.get(&key)?;
    if entry.loaded_at.elapsed() >= CACHE_TTL {
        return None;
    }
    let mut suggestions = entry.suggestions.clone();
    for suggestion in &mut suggestions {
        suggestion.span = TextSpan::new(pos.saturating_sub(last_word.len()), pos);
    }
    Some(suggestions)
}

fn store(
    cache: &mut FxIndexMap<CacheKey, CacheEntry>,
    key: CacheKey,
    suggestions: Vec<CompletionCandidate>,
) {
    cache.retain(|_, entry| entry.loaded_at.elapsed() < CACHE_TTL);
    cache.shift_remove(&key);
    if cache.len() >= MAX_CACHED_REQUESTS {
        cache.shift_remove_index(0);
    }
    cache.insert(
        key,
        CacheEntry {
            suggestions,
            loaded_at: Instant::now(),
        },
    );
}

/// True means the caller has a provisional answer: a later cache publication,
/// from this request or another in-flight one, triggers a fresh compute.
/// Cancelled requests free their slot without caching a negative answer.
pub(crate) fn spawn_refresh_at(
    words: Vec<String>,
    last_word: String,
    cwd: PathBuf,
    cancel: Option<CompletionCancel>,
) -> bool {
    if words.is_empty() {
        return false;
    }
    match is_carapace_available_cached() {
        Some(false) => return false,
        None => {
            spawn_carapace_availability_probe();
            return true;
        }
        Some(true) => {}
    }
    let borrowed: Vec<_> = words.iter().map(String::as_str).collect();
    let Some(key) = CacheKey::new(&borrowed, &last_word, &cwd) else {
        return false;
    };
    {
        let mut active = IN_FLIGHT.lock();
        if active.contains(&key) || active.len() >= MAX_IN_FLIGHT {
            return true;
        }
        active.insert(key.clone());
    }
    let worker_key = key.clone();
    if std::thread::Builder::new()
        .name("fsh-carapace".into())
        .spawn(move || {
            let output = super::provider::run_process(
                "carapace",
                &worker_key.args,
                &worker_key.cwd,
                &worker_key.path,
                cancel.as_ref(),
            );
            // Cancellation is not a negative provider answer. A later request for
            // the same context must be able to retry it.
            if !cancel.as_ref().is_some_and(CompletionCancel::is_cancelled) {
                let suggestions = output
                    .and_then(|bytes| parse_suggestions(&bytes, last_word.len()))
                    .unwrap_or_default();
                store(&mut CACHE.lock(), worker_key.clone(), suggestions);
            }
            IN_FLIGHT.lock().remove(&worker_key);
            super::sources::publish();
        })
        .is_err()
    {
        IN_FLIGHT.lock().remove(&key);
        store(&mut CACHE.lock(), key, Vec::new());
        super::sources::publish();
    }
    true
}

fn parse_suggestions(bytes: &[u8], span_len: usize) -> Option<Vec<CompletionCandidate>> {
    #[derive(serde::Deserialize)]
    struct Suggestion {
        value: String,
        description: Option<String>,
    }
    let suggestions: Vec<Suggestion> = serde_json::from_slice(bytes).ok()?;
    Some(
        suggestions
            .into_iter()
            .map(|suggestion| {
                let kind = if suggestion.value.starts_with('-') {
                    CompletionKind::Flag
                } else {
                    CompletionKind::ExternalCommand
                };
                CompletionCandidate::new(suggestion.value, kind, TextSpan::new(0, span_len))
                    .with_description(suggestion.description.unwrap_or_default())
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keys_include_directory_and_argument_boundaries() {
        let first = CacheKey::new(&["tool", "argument"], "argument", Path::new("/first")).unwrap();
        let second =
            CacheKey::new(&["tool", "argument"], "argument", Path::new("/second")).unwrap();
        assert!(first != second);
        assert_eq!(first.args, ["tool", "nushell", "argument"]);
        assert_eq!(
            CacheKey::new(&["tool", "argument"], "", Path::new("/first"))
                .unwrap()
                .args,
            ["tool", "nushell", "argument", ""]
        );
        assert!(CacheKey::new(&[], "", Path::new("/")).is_none());
    }

    #[test]
    fn cache_is_bounded_and_keeps_negative_answers() {
        let mut cache = FxIndexMap::default();
        for index in 0..MAX_CACHED_REQUESTS + 10 {
            let key = CacheKey::new(&["tool", &index.to_string()], "", Path::new("/")).unwrap();
            store(&mut cache, key, Vec::new());
        }
        assert_eq!(cache.len(), MAX_CACHED_REQUESTS);
        assert!(cache.values().all(|entry| entry.suggestions.is_empty()));
    }
}
