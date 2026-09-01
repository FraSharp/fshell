// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Background async bridge for Carapace external command completion generator.

use super::types::{CompletionCandidate, CompletionKind, TextSpan};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static CARAPACE_CHECKED: AtomicBool = AtomicBool::new(false);
static CARAPACE_AVAILABLE: AtomicBool = AtomicBool::new(false);

pub fn is_carapace_available_cached() -> Option<bool> {
    if CARAPACE_CHECKED.load(Ordering::Acquire) {
        Some(CARAPACE_AVAILABLE.load(Ordering::Acquire))
    } else {
        None
    }
}

pub fn spawn_carapace_availability_probe() {
    if CARAPACE_CHECKED.load(Ordering::Acquire) {
        return;
    }
    std::thread::spawn(|| {
        let available = std::process::Command::new("carapace")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        CARAPACE_AVAILABLE.store(available, Ordering::Release);
        CARAPACE_CHECKED.store(true, Ordering::Release);
    });
}

struct CarapaceCacheEntry {
    suggestions: Vec<CompletionCandidate>,
    loaded_at: Instant,
}

static CARAPACE_CACHE: Mutex<Option<HashMap<Vec<String>, CarapaceCacheEntry>>> = Mutex::new(None);
const CARAPACE_CACHE_TTL: Duration = Duration::from_secs(60);
static CARAPACE_IN_FLIGHT: OnceLock<Mutex<HashSet<Vec<String>>>> = OnceLock::new();

fn carapace_in_flight() -> &'static Mutex<HashSet<Vec<String>>> {
    CARAPACE_IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn complete_with_carapace_cached(
    words: &[&str],
    last_word: &str,
    pos: usize,
) -> Option<Vec<CompletionCandidate>> {
    if words.is_empty() {
        return None;
    }
    let available = is_carapace_available_cached()?;
    if !available {
        return None;
    }
    let cmd = words[0];
    let mut key = vec![cmd.to_string(), "nushell".to_string()];
    for w in words.iter().skip(1) {
        key.push(w.to_string());
    }
    if last_word.is_empty() {
        key.push("".to_string());
    }
    let cache = CARAPACE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = cache.as_ref()?;
    let entry = map.get(&key)?;
    if entry.loaded_at.elapsed() >= CARAPACE_CACHE_TTL {
        return None;
    }
    let mut suggestions = entry.suggestions.clone();
    for s in &mut suggestions {
        s.span = TextSpan::new(pos.saturating_sub(last_word.len()), pos);
    }
    Some(suggestions)
}

pub fn spawn_carapace_refresh(words: Vec<String>, last_word: String) {
    let available = is_carapace_available_cached();
    if available == Some(false) {
        return;
    }
    if available.is_none() {
        spawn_carapace_availability_probe();
        return;
    }
    let mut key = vec![words[0].clone(), "nushell".to_string()];
    for w in words.iter().skip(1) {
        key.push(w.clone());
    }
    if last_word.is_empty() {
        key.push("".to_string());
    }
    let key_for_inflight = key.clone();
    if let Ok(mut inflight) = carapace_in_flight().lock()
        && !inflight.insert(key_for_inflight.clone())
    {
        return;
    }
    std::thread::spawn(move || {
        let carapace_args = key_for_inflight.clone();
        let output = std::process::Command::new("carapace")
            .args(&carapace_args)
            .output();
        let results = (|| {
            let output = output.ok()?;
            if !output.status.success() {
                return None;
            }
            #[derive(serde::Deserialize)]
            struct CarapaceSuggestion {
                value: String,
                description: Option<String>,
            }
            let suggestions: Vec<CarapaceSuggestion> =
                serde_json::from_slice(&output.stdout).ok()?;
            let span_len = last_word.len();
            let mut results = Vec::new();
            for s in suggestions {
                let kind = if s.value.starts_with('-') {
                    CompletionKind::Flag
                } else {
                    CompletionKind::ExternalCommand
                };
                results.push(
                    CompletionCandidate::new(s.value, kind, TextSpan::new(0, span_len))
                        .with_description(s.description.unwrap_or_default()),
                );
            }
            Some(results)
        })();
        if let Some(results) = results
            && let Ok(mut cache) = CARAPACE_CACHE.lock()
        {
            if cache.is_none() {
                *cache = Some(HashMap::new());
            }
            if let Some(map) = cache.as_mut() {
                map.insert(
                    carapace_args.clone(),
                    CarapaceCacheEntry {
                        suggestions: results,
                        loaded_at: Instant::now(),
                    },
                );
            }
        }
        if let Ok(mut inflight) = carapace_in_flight().lock() {
            inflight.remove(&carapace_args);
        }
    });
}
