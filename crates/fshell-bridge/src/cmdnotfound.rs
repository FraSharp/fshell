// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
pub struct CmdNotFoundDb {
    pub entries: HashMap<String, CmdNotFoundEntry>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct CmdNotFoundEntry {
    pub expires_at: u64,
    pub suggestion: Option<String>,
}

fn cache_path() -> Option<PathBuf> {
    let dir = fshell_engine::cache_dir()?;
    Some(dir.join("cmdnotfound.json"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_cache() -> CmdNotFoundDb {
    let path = match cache_path() {
        Some(p) => p,
        None => return CmdNotFoundDb::default(),
    };
    if !path.exists() {
        return CmdNotFoundDb::default();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_cache_atomically(db: &CmdNotFoundDb) {
    let path = match cache_path() {
        Some(p) => p,
        None => return,
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Write to .tmp then atomically rename to prevent partial reads
    let tmp_path = path.with_extension("json.tmp");
    if let Ok(content) = serde_json::to_string_pretty(db)
        && std::fs::write(&tmp_path, &content).is_ok()
    {
        let _ = std::fs::rename(&tmp_path, &path);
    }
}

/// Check the cache for a suggestion. Returns Some(suggestion) if a
/// fresh entry exists, None if the cache needs refreshing.
pub fn lookup_cached(name: &str) -> Option<String> {
    if std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1") {
        eprintln!(
            "[cnf_debug] {}:{}: lookup_cached name={:?}",
            file!(),
            line!(),
            name
        );
    }
    let db = read_cache();
    let entry = db.entries.get(name)?;
    if entry.expires_at > now_secs() {
        if std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1") {
            eprintln!(
                "[cnf_debug] {}:{}: lookup_cached hit suggestion={:?}",
                file!(),
                line!(),
                entry.suggestion
            );
        }
        entry.suggestion.clone()
    } else {
        if std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1") {
            eprintln!(
                "[cnf_debug] {}:{}: lookup_cached expired entry",
                file!(),
                line!()
            );
        }
        None
    }
}

/// Run a command with a timeout, returning its output if it succeeded within the deadline.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<String> {
    let mut child = cmd.spawn().ok()?;
    let start = Instant::now();

    let status = loop {
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait(); // reap to prevent zombie
            return None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => {
                let _ = child.wait();
                return None;
            }
        }
    };

    if !status.success() {
        let _ = child.wait();
        return None;
    }

    child
        .wait_with_output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}

/// Spawn a background thread to search for the command and cache results.
pub fn spawn_background_search(name: &str) {
    if std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1") {
        eprintln!(
            "[cnf_debug] {}:{}: spawn_background_search name={:?}",
            file!(),
            line!(),
            name
        );
    }
    let name_owned = name.to_string();
    std::thread::spawn(move || {
        let search_start = std::time::Instant::now();
        let suggestion = platform_search(&name_owned);
        let elapsed = search_start.elapsed();
        let display_suggestion = suggestion.clone();
        let expires_at = if suggestion.is_some() {
            now_secs() + 1800 // 30 min for positive
        } else {
            now_secs() + 300 // 5 min for negative
        };
        let mut db = read_cache();
        db.entries.insert(
            name_owned,
            CmdNotFoundEntry {
                expires_at,
                suggestion,
            },
        );
        write_cache_atomically(&db);
        if std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1") {
            eprintln!(
                "[cnf_debug] {}:{}: background search done in {:?}, result={:?}",
                file!(),
                line!(),
                elapsed,
                display_suggestion
            );
        }
    });
}

#[cfg(target_os = "macos")]
fn platform_search(name: &str) -> Option<String> {
    let output = run_with_timeout(
        Command::new("brew")
            .args(["search", name])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(30),
    )?;
    for line in output.lines() {
        let pkg = line.trim().split('/').next_back().unwrap_or(line.trim());
        if pkg.contains(name) && !pkg.contains("No formula") {
            return Some(format!("brew install {}", pkg));
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn platform_search(name: &str) -> Option<String> {
    // Try apt-cache first (10 second timeout)
    if let Some(output) = run_with_timeout(
        Command::new("apt-cache")
            .args(["search", name])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(10),
    ) {
        for line in output.lines() {
            let pkg = line.split_once(' ').map(|(p, _)| p).unwrap_or(line);
            if pkg.contains(name) {
                return Some(format!("apt install {}", pkg));
            }
        }
    }
    // Fallback to dnf (10 second timeout)
    if let Some(output) = run_with_timeout(
        Command::new("dnf")
            .args(["search", name])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(10),
    ) {
        for line in output.lines() {
            if line.trim().starts_with(name) || line.contains(&format!(" {}", name)) {
                return Some(format!("dnf install {}", name));
            }
        }
    }
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_search(_name: &str) -> Option<String> {
    None
}
