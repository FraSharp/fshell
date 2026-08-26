// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use crate::utils::expand_tilde;
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_core::diagnostic::ErrorCode;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct FrecencyEntry {
    pub frequency: f64,
    pub last_visited: u64,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct FrecencyDb {
    pub paths: std::collections::HashMap<String, FrecencyEntry>,
}

struct FrecencyCache {
    db: FrecencyDb,
    db_path: PathBuf,
    _loaded_at: Instant,
}

static FRECENCY_CACHE: Mutex<Option<FrecencyCache>> = Mutex::new(None);

fn get_frecency_db(db_path: &PathBuf) -> Result<FrecencyDb, String> {
    let mut cache = FRECENCY_CACHE
        .lock()
        .map_err(|e| format!("Lock poisoned: FRECENCY_CACHE: {}", e))?;
    if let Some(ref cached) = *cache
        && cached.db_path == *db_path
    {
        return Ok(cached.db.clone());
    }
    // Cache miss: load from disk
    let db = if db_path.exists() {
        let content = std::fs::read_to_string(db_path)
            .map_err(|e| format!("Failed to read frecency DB: {}", e))?;
        serde_json::from_str::<FrecencyDb>(&content).unwrap_or_default()
    } else {
        FrecencyDb::default()
    };
    *cache = Some(FrecencyCache {
        db: db.clone(),
        db_path: db_path.clone(),
        _loaded_at: Instant::now(),
    });
    Ok(db)
}

fn with_frecency_db<T>(db_path: &PathBuf, f: impl FnOnce(&FrecencyDb) -> T) -> Result<T, String> {
    let mut cache = FRECENCY_CACHE
        .lock()
        .map_err(|e| format!("Lock poisoned: FRECENCY_CACHE: {}", e))?;
    if let Some(ref cached) = *cache
        && cached.db_path == *db_path
    {
        return Ok(f(&cached.db));
    }
    let db = if db_path.exists() {
        let content = std::fs::read_to_string(db_path)
            .map_err(|e| format!("Failed to read frecency DB: {}", e))?;
        serde_json::from_str::<FrecencyDb>(&content).unwrap_or_default()
    } else {
        FrecencyDb::default()
    };
    let result = f(&db);
    *cache = Some(FrecencyCache {
        db,
        db_path: db_path.clone(),
        _loaded_at: Instant::now(),
    });
    Ok(result)
}

pub fn get_frecency_db_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("FSH_Z_DB_PATH")
        && !path.is_empty()
    {
        return Some(PathBuf::from(path));
    }
    fshell_engine::config_dir().map(|d| d.join("frecency.db"))
}

pub fn log_frecency_visit(path: &std::path::Path) -> Result<(), ShellError> {
    let path_str = path.to_string_lossy().to_string();
    let db_path = match get_frecency_db_path() {
        Some(p) => p,
        None => return Ok(()),
    };

    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Use in-memory cache to avoid disk read on every visit
    let mut db = get_frecency_db(&db_path).unwrap_or_default();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let entry = db.paths.entry(path_str).or_insert(FrecencyEntry {
        frequency: 0.0,
        last_visited: now,
    });
    entry.frequency += 1.0;
    entry.last_visited = now;

    if db.paths.len() > 500 {
        let mut entries: Vec<(String, FrecencyEntry)> = db.paths.into_iter().collect();
        entries.sort_by_key(|(_, e)| std::cmp::Reverse(e.last_visited));
        entries.truncate(400);
        db.paths = entries.into_iter().collect();
    }

    let write_path = db_path.clone();
    let write_db = db.clone();
    // Persist off the hot path: `cd`/`z` shouldn't block on disk I/O. The
    // in-memory cache above is updated synchronously so ranking stays correct;
    // the JSON write happens on a background thread. `cd` is user-paced, so
    // a per-visit writer thread (writing a monotonic snapshot) is cheap.
    std::thread::spawn(move || {
        if let Ok(content) = serde_json::to_string_pretty(&write_db) {
            let _ = std::fs::write(&write_path, content);
        }
    });

    // Update cache with latest state
    if let Ok(mut cache) = FRECENCY_CACHE.lock() {
        match cache.as_mut() {
            Some(cached) => {
                cached.db = db;
                cached.db_path = db_path;
            }
            None => {
                *cache = Some(FrecencyCache {
                    db,
                    db_path,
                    _loaded_at: Instant::now(),
                });
            }
        }
    }

    Ok(())
}

/// Helper to find the highest-ranked frecent directory matching a set of query fragments.
/// If `subdirectory_only` is true, results are restricted to subdirectories of the current directory.
pub fn resolve_z_match(
    fragments: &[String],
    subdirectory_only: bool,
    cwd: Option<&std::path::Path>,
) -> Result<PathBuf, String> {
    let db_path = match get_frecency_db_path() {
        Some(p) => p,
        None => return Err("HOME not set".to_string()),
    };

    let db = get_frecency_db(&db_path)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let current_dir = cwd.map(|p| p.to_path_buf());

    let mut candidates: Vec<(String, f64)> = Vec::new();
    for (path, entry) in &db.paths {
        let path_lower = path.to_lowercase();

        // Match all fragments
        if !fragments.iter().all(|frag| path_lower.contains(frag)) {
            continue;
        }

        // Subdirectory check if requested
        if subdirectory_only {
            if let Some(ref cur) = current_dir {
                let path_buf = PathBuf::from(path);
                if !path_buf.starts_with(cur) || path_buf == *cur {
                    continue;
                }
            } else {
                continue;
            }
        }

        let age = now.saturating_sub(entry.last_visited);
        let factor = age_factor(age);
        candidates.push((path.clone(), entry.frequency * factor));
    }

    if candidates.is_empty() {
        return Err(BuiltinError::NotFound {
            cmd: "cd".into(),
            what: format!("directory matching {:?}", fragments),
            span: None,
        }
        .to_string());
    }

    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    Ok(PathBuf::from(candidates[0].0.clone()))
}

/// Recency multiplier for a directory entry: visits within the last hour count
/// 4×, last day 2×, last week 1×, older 0.5×.
fn age_factor(age_secs: u64) -> f64 {
    if age_secs < 3600 {
        4.0
    } else if age_secs < 86400 {
        2.0
    } else if age_secs < 604800 {
        1.0
    } else {
        0.5
    }
}

pub fn z_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    if args.is_empty() {
        let db_path = match get_frecency_db_path() {
            Some(p) => p,
            None => return Err("HOME not set".to_string().into()),
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let scored: Vec<(String, f64)> = with_frecency_db(&db_path, |db| {
            let mut scored: Vec<(String, f64)> = db
                .paths
                .iter()
                .map(|(path, entry)| {
                    let age = now.saturating_sub(entry.last_visited);
                    let factor = age_factor(age);
                    (path.clone(), entry.frequency * factor)
                })
                .collect();
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scored
        })
        .unwrap_or_default();

        tokio::spawn(async move {
            for (path, score) in scored.iter().take(20) {
                let mut m =
                    fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                m.insert(ustr::ustr("path"), Val::String(path.clone()));
                m.insert(ustr::ustr("score"), Val::Float(*score));
                let payload = PipelinePayload::Data(Arc::new(Val::Map(m)));
                if tx.send(payload).await.is_err() {
                    break;
                }
            }
        });
        return Ok(());
    }

    // Check if it's "-" first
    let is_dash = args.len() == 1 && matches!(&args[0], Val::String(s) if s == "-");
    if is_dash {
        let oldpwd = match std::env::var("OLDPWD") {
            Ok(o) => PathBuf::from(o),
            Err(_) => return Err("z: OLDPWD not set".to_string().into()),
        };
        let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(
            oldpwd.display().to_string(),
        ))));
        crate::utils::change_dir_and_update_caps(&oldpwd, env)?;
        let _ = log_frecency_visit(&oldpwd);
        drop(tx);
        return Ok(());
    }

    // If there is a single argument that represents an existing directory (exact match)
    if args.len() == 1
        && let Val::String(ref s) = args[0]
    {
        let expanded = expand_tilde(s);
        if expanded.is_dir()
            && let Ok(target_path) = std::fs::canonicalize(&expanded)
        {
            crate::utils::change_dir_and_update_caps(&target_path, env)?;
            let _ = log_frecency_visit(&target_path);
            drop(tx);
            return Ok(());
        }
    }

    let mut fragments = Vec::new();
    let mut subdirectory_only = false;
    for arg in args {
        match arg {
            Val::String(s) => {
                if s == "/" {
                    subdirectory_only = true;
                } else {
                    fragments.push(s.to_lowercase());
                }
            }
            _ => {
                return Err(ShellError::new(
                    ErrorCode::InvalidArgument,
                    "z arguments must be strings",
                )
                .maybe_with_span(span));
            }
        }
    }

    let target = resolve_z_match(&fragments, subdirectory_only, Some(&env.cwd()))?;

    // Optional echo matched path if _ZO_ECHO == 1 or Z_ECHO == 1
    let echo_matched = std::env::var("_ZO_ECHO").unwrap_or_default() == "1"
        || std::env::var("Z_ECHO").unwrap_or_default() == "1";
    if echo_matched {
        let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(
            target.display().to_string(),
        ))));
    }

    crate::utils::change_dir_and_update_caps(&target, env)?;
    let _ = log_frecency_visit(&target);

    drop(tx);
    Ok(())
}

pub fn zi_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let mut fragments = Vec::new();
    let mut subdirectory_only = false;
    for arg in args {
        match arg {
            Val::String(s) => {
                if s == "/" {
                    subdirectory_only = true;
                } else {
                    fragments.push(s.to_lowercase());
                }
            }
            _ => {
                return Err(ShellError::new(
                    ErrorCode::InvalidArgument,
                    "zi arguments must be strings",
                )
                .maybe_with_span(span));
            }
        }
    }

    let db_path = match get_frecency_db_path() {
        Some(p) => p,
        None => return Err("HOME not set".to_string().into()),
    };

    if !db_path.exists() {
        return Err("No frecency history yet. cd into directories first!"
            .to_string()
            .into());
    }

    let content = std::fs::read_to_string(&db_path)
        .map_err(|e| format!("Failed to read frecency DB: {}", e))?;
    let db = serde_json::from_str::<FrecencyDb>(&content).unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let current_dir = Some(env.cwd());

    let mut scored: Vec<(String, f64)> = Vec::new();
    for (path, entry) in &db.paths {
        let path_lower = path.to_lowercase();

        // Match all fragments
        if !fragments.iter().all(|frag| path_lower.contains(frag)) {
            continue;
        }

        // Subdirectory check if requested
        if subdirectory_only {
            if let Some(ref cur) = current_dir {
                let path_buf = PathBuf::from(path);
                if !path_buf.starts_with(cur) || path_buf == *cur {
                    continue;
                }
            } else {
                continue;
            }
        }

        let age = now.saturating_sub(entry.last_visited);
        let factor = age_factor(age);
        scored.push((path.clone(), entry.frequency * factor));
    }

    if scored.is_empty() {
        return Err(BuiltinError::NotFound {
            cmd: "cd".into(),
            what: format!("directory matching {:?}", fragments),
            span,
        }
        .into());
    }

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let candidates: Vec<String> = scored.into_iter().map(|(p, _)| p).collect();

    // Try to run fzf
    let fzf_opts = std::env::var("_ZO_FZF_OPTS").unwrap_or_default();
    let mut args_list = vec!["--height", "40%", "--reverse"];
    if !fzf_opts.is_empty() {
        for opt in fzf_opts.split_whitespace() {
            args_list.push(opt);
        }
    }

    let mut use_fallback = false;
    let selected_path = match std::process::Command::new("fzf")
        .args(&args_list)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
    {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                for path in &candidates {
                    let _ = writeln!(stdin, "{}", path);
                }
            }

            match child.wait_with_output() {
                Ok(output) => {
                    if output.status.success() {
                        let selected = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if selected.is_empty() {
                            return Err(ShellError::new(
                                ErrorCode::Cancelled,
                                "Interactive selection cancelled",
                            )
                            .maybe_with_span(span));
                        }
                        selected
                    } else {
                        return Err(ShellError::new(
                            ErrorCode::Cancelled,
                            "Interactive selection cancelled",
                        )
                        .maybe_with_span(span));
                    }
                }
                Err(_) => {
                    use_fallback = true;
                    String::new()
                }
            }
        }
        Err(_) => {
            use_fallback = true;
            String::new()
        }
    };

    let target_path = if use_fallback {
        println!("fzf not found. Falling back to built-in selection:");
        let displayed_count = std::cmp::min(candidates.len(), 10);
        for (i, candidate) in candidates.iter().enumerate().take(displayed_count) {
            println!("{}) {}", i + 1, candidate);
        }
        print!("Select directory (1-{}, or q to cancel): ", displayed_count);
        use std::io::Write;
        let _ = std::io::stdout().flush();

        let raw_mode_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if raw_mode_was_enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }

        let mut input = String::new();
        let read_res = std::io::stdin().read_line(&mut input);

        if raw_mode_was_enabled {
            let _ = crossterm::terminal::enable_raw_mode();
        }

        if read_res.is_err() {
            return Err(
                ShellError::new(ErrorCode::IoError, "Failed to read input").maybe_with_span(span)
            );
        }

        let input_trimmed = input.trim();
        if input_trimmed.eq_ignore_ascii_case("q") {
            return Err(
                ShellError::new(ErrorCode::Cancelled, "Interactive selection cancelled")
                    .maybe_with_span(span),
            );
        }

        match input_trimmed.parse::<usize>() {
            Ok(idx) if idx > 0 && idx <= displayed_count => candidates[idx - 1].clone(),
            _ => {
                return Err(
                    ShellError::new(ErrorCode::InvalidArgument, "Invalid selection")
                        .maybe_with_span(span),
                );
            }
        }
    } else {
        selected_path
    };

    let target = PathBuf::from(target_path);
    crate::utils::change_dir_and_update_caps(&target, env)?;
    let _ = log_frecency_visit(&target);

    drop(tx);
    Ok(())
}
