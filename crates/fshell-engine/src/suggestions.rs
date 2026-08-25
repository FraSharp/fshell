// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::{Env, PATH_CACHE, SUGGESTION_CACHE, SuggestionCache, resolve_config_dir};
use fshell_core::Val;

pub(crate) fn levenshtein_distance(s1: &str, s2: &str) -> usize {
    let s1_chars: Vec<char> = s1.chars().collect();
    let s2_chars: Vec<char> = s2.chars().collect();
    let m = s1_chars.len();
    let n = s2_chars.len();
    let mut dp = vec![vec![0; n + 1]; m + 1];
    for (i, row) in dp.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=m {
        for j in 1..=n {
            if s1_chars[i - 1] == s2_chars[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            } else {
                dp[i][j] = 1 + dp[i - 1][j - 1].min(dp[i - 1][j]).min(dp[i][j - 1]);
            }
        }
    }
    dp[m][n]
}

pub fn get_suggested_command(name: &str, env: &Env, env_path: Option<&str>) -> Option<String> {
    if std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1") {
        eprintln!(
            "[cnf_debug] {}:{}: get_suggested_command name={:?}",
            file!(),
            line!(),
            name
        );
    }
    // Check suggestion cache first
    if let Ok(mut cache_guard) = SUGGESTION_CACHE.lock()
        && let Some(ref mut cache) = *cache_guard
        && let Some(cached) = cache.get(name)
    {
        // Refresh stale cache entry — triggers re-check if the entry is older than 5s
        if cache.is_stale(name) {
            // Entry is stale, fall through to recompute
        } else {
            if std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1") {
                eprintln!(
                    "[cnf_debug] {}:{}: suggestion cache hit {:?}",
                    file!(),
                    line!(),
                    cached
                );
            }
            return cached;
        }
    }

    let start = std::time::Instant::now();
    let mut best_cmd = None;
    let mut best_dist = usize::MAX;
    // Priority tiers: 0=builtins, 1=fns, 2=aliases, 3=PATH
    let mut best_priority: u8 = 4;

    /// Update best candidate if `dist` is strictly better, or equal but from
    /// a higher-priority (lower number) category, or equal same-category but shorter name.
    fn update_best(
        candidate: &str,
        dist: usize,
        priority: u8,
        best_cmd: &mut Option<String>,
        best_dist: &mut usize,
        best_priority: &mut u8,
    ) {
        if dist > 2 {
            return;
        }
        if dist < *best_dist || (dist == *best_dist && priority < *best_priority) {
            *best_dist = dist;
            *best_priority = priority;
            *best_cmd = Some(candidate.to_string());
        } else if dist == *best_dist
            && priority == *best_priority
            && let Some(ref best) = *best_cmd
            && candidate.len() < best.len()
        {
            *best_cmd = Some(candidate.to_string());
        }
    }

    // 1. Check builtins (priority 0)
    let builtin_count;
    {
        let reg_guard = env.builtins.read();
        builtin_count = reg_guard.len();
        for builtin in reg_guard.keys() {
            let dist = levenshtein_distance(name, builtin);
            update_best(
                builtin,
                dist,
                0,
                &mut best_cmd,
                &mut best_dist,
                &mut best_priority,
            );
        }
    }

    // 2. Check functions (priority 1)
    let fn_count;
    {
        let fns_guard = env.fns.read();
        fn_count = fns_guard.len();
        for func in fns_guard.keys() {
            let dist = levenshtein_distance(name, func);
            update_best(
                func,
                dist,
                1,
                &mut best_cmd,
                &mut best_dist,
                &mut best_priority,
            );
        }
    }

    // 3. Check aliases (priority 2)
    let alias_count;
    {
        let reg_guard = env.aliases.read();
        alias_count = reg_guard.len();
        for alias_name in reg_guard.keys() {
            let dist = levenshtein_distance(name, alias_name);
            update_best(
                alias_name,
                dist,
                2,
                &mut best_cmd,
                &mut best_dist,
                &mut best_priority,
            );
        }
    }

    // 4. Check executables in PATH (priority 3; use the shell's env.PATH, falling back to global)
    let mut path_checked = 0;
    let current_path = env_path
        .map(|p| p.to_string())
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    if !current_path.is_empty()
        && let Ok(cache_guard) = PATH_CACHE.lock()
    {
        // Use existing cache only — never rebuild synchronously.
        // The background watcher keeps the cache fresh every 30s.
        let first_char = name.chars().next();
        if let Some(ref cache) = *cache_guard
            && cache.path == current_path
        {
            for exe_name in cache.executables.keys() {
                if let Some(fc) = first_char
                    && exe_name.chars().next().map(|c| c.to_ascii_lowercase())
                        != Some(fc.to_ascii_lowercase())
                {
                    continue;
                }
                path_checked += 1;
                let dist = levenshtein_distance(name, exe_name);
                update_best(
                    exe_name,
                    dist,
                    3,
                    &mut best_cmd,
                    &mut best_dist,
                    &mut best_priority,
                );
            }
        }
    }

    // Store result in suggestion cache with timestamp
    if let Ok(mut cache_guard) = SUGGESTION_CACHE.lock() {
        cache_guard
            .get_or_insert_with(SuggestionCache::new)
            .insert(name.to_string(), best_cmd.clone());
    }

    if std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1") {
        eprintln!(
            "[cnf_debug] {}:{}: done in {}us (builtins={}, fns={}, aliases={}, path_checked={}), result={:?}",
            file!(),
            line!(),
            start.elapsed().as_micros(),
            builtin_count,
            fn_count,
            alias_count,
            path_checked,
            best_cmd
        );
    }
    best_cmd
}

pub fn has_trust_profile(path: &str) -> bool {
    let profile_path = match resolve_config_dir() {
        Some(d) => d.join("trust_profiles.json"),
        None => return false,
    };
    if let Ok(file_content) = std::fs::read_to_string(&profile_path) {
        #[derive(serde::Deserialize)]
        struct TrustData {
            trusted_hashes: std::collections::HashMap<String, String>,
        }
        if let Ok(data) = serde_json::from_str::<TrustData>(&file_content) {
            let canonical_path = std::path::Path::new(path)
                .canonicalize()
                .unwrap_or_else(|_| std::path::PathBuf::from(path));
            let canonical_path_str = canonical_path.to_string_lossy().to_string();
            return data.trusted_hashes.contains_key(&canonical_path_str);
        }
    }
    false
}

pub fn is_script_trusted(path: &str, content: &str) -> bool {
    let profile_path = match resolve_config_dir() {
        Some(d) => d.join("trust_profiles.json"),
        None => return false,
    };
    if let Ok(file_content) = std::fs::read_to_string(&profile_path) {
        #[derive(serde::Deserialize)]
        struct TrustData {
            trusted_hashes: std::collections::HashMap<String, String>,
        }
        if let Ok(data) = serde_json::from_str::<TrustData>(&file_content) {
            let digest = fshell_hash::fhash256(content.as_bytes());
            let mut hash_hex = String::with_capacity(64);
            for b in digest {
                hash_hex.push_str(&format!("{:02x}", b));
            }

            let canonical_path = std::path::Path::new(path)
                .canonicalize()
                .unwrap_or_else(|_| std::path::PathBuf::from(path));
            let canonical_path_str = canonical_path.to_string_lossy().to_string();

            if let Some(expected_hash) = data.trusted_hashes.get(&canonical_path_str) {
                return expected_hash.eq_ignore_ascii_case(&hash_hex);
            }
        }
    }
    false
}

pub fn parse_json_value(json: serde_json::Value) -> Val {
    json.into()
}
