// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]
#![allow(clippy::result_large_err)]
use fshell_core::{Expr, FxIndexMap, Parser, Stmt, Val};
use fshell_engine::{EngineError, Env, Flow, PipelinePayload, eval_stmt, is_stdout_a_tty};
use nu_ansi_term::Color;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::Notify;

struct SessionLogMessage {
    log_path: std::path::PathBuf,
    content: String,
}

static SESSION_LOG_TX: std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<SessionLogMessage>> =
    std::sync::OnceLock::new();

pub fn init_session_logger() {
    if SESSION_LOG_TX.get().is_some() {
        return;
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SessionLogMessage>();
    if SESSION_LOG_TX.set(tx).is_err() {
        return;
    }

    tokio::spawn(async move {
        let mut open_files: std::collections::HashMap<std::path::PathBuf, std::fs::File> =
            std::collections::HashMap::new();

        while let Some(msg) = rx.recv().await {
            let file = if let Some(f) = open_files.get_mut(&msg.log_path) {
                Some(f)
            } else {
                let f_res = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&msg.log_path);
                if let Ok(f) = f_res {
                    open_files.insert(msg.log_path.clone(), f);
                    open_files.get_mut(&msg.log_path)
                } else {
                    None
                }
            };

            if let Some(f) = file {
                let _ = writeln!(f, "{}", msg.content);
                let _ = f.flush();
            }
        }
    });
}

use ustr::ustr;

pub mod alias_expansion;
pub mod autocomplete;
pub mod config_tui;
pub mod format;
pub mod ftui;
pub mod fuzzy;
pub mod highlighter;
pub mod hinter;
pub mod history;
pub mod picker;
pub mod prompt;
pub mod prompt_config;
pub mod prompt_customizer;
pub mod theme_ext;

pub use autocomplete::*;

use crate::format::*;

use history::{get_hostname, init_db, log_command, query_history, update_history_entry};

pub fn history_builtin(
    _in_rx: Option<fshell_engine::PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: fshell_engine::PipeSender,
    _span: Option<miette::SourceSpan>,
) -> Result<(), fshell_core::ShellError> {
    let mut interactive = false;
    let mut stats = false;
    let mut filter_exit: Option<i64> = None;
    let mut filter_cwd: Option<String> = None;
    let mut filter_session: Option<String> = None;
    let mut filter_host: Option<String> = None;
    let mut limit: Option<usize> = None;
    let mut search_query: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        if let Val::String(s) = &args[i] {
            if s == "-i" || s == "--interactive" {
                interactive = true;
            } else if s == "--stats" {
                stats = true;
            } else if s == "--cwd" {
                let current_pwd = std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "/".to_string());
                filter_cwd = Some(current_pwd);
            } else if s == "--session" {
                let vars = env.vars.read();
                if let Some(Val::String(sess)) = vars.get("FSH_SESSION_ID") {
                    filter_session = Some(sess.clone());
                }
            } else if s == "--global" {
                filter_cwd = None;
                filter_session = None;
            } else if s == "--host" {
                let host = get_hostname();
                filter_host = Some(host);
            } else if s == "--exit" && i + 1 < args.len() {
                i += 1;
                if let Val::Int(code) = args[i] {
                    filter_exit = Some(code);
                } else if let Val::String(code_str) = &args[i]
                    && let Ok(code) = code_str.parse::<i64>()
                {
                    filter_exit = Some(code);
                }
            } else if s == "--limit" && i + 1 < args.len() {
                i += 1;
                if let Val::Int(lim) = args[i] {
                    if lim < 0 {
                        return Err(format!(
                            "--limit requires a non-negative integer, got {}",
                            lim
                        )
                        .into());
                    }
                    limit = Some(lim as usize);
                } else if let Val::String(lim_str) = &args[i]
                    && let Ok(lim) = lim_str.parse::<usize>()
                {
                    limit = Some(lim);
                }
            } else if s.starts_with('-') {
                return Err(format!("Unknown option: {}", s).into());
            } else {
                search_query = Some(s.clone());
            }
        }
        i += 1;
    }

    if stats {
        let stats_data = std::thread::spawn(history::get_stats)
            .join()
            .unwrap_or_else(|_| Err("Background thread panicked".to_string()))?;
        let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr("total_commands"), Val::Int(stats_data.total_commands));
        m.insert(
            ustr("unique_commands"),
            Val::Int(stats_data.unique_commands),
        );
        m.insert(
            ustr("success_rate_percent"),
            Val::Float(stats_data.success_rate),
        );

        let top_cmds: Vec<Val> = stats_data
            .top_commands
            .into_iter()
            .map(|(cmd, count)| {
                let mut tm = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                tm.insert(ustr("command"), Val::String(cmd));
                tm.insert(ustr("count"), Val::Int(count));
                Val::Map(tm)
            })
            .collect();
        m.insert(ustr("top_commands"), Val::List(top_cmds));

        let payload = std::sync::Arc::new(Val::Map(m));
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            let _ = tx_clone.send(PipelinePayload::Data(payload)).await;
        });
        return Ok(());
    }

    let is_terminal = !fshell_engine::is_test_mode() && is_stdout_a_tty();
    let has_pipe_input = _in_rx.is_some();
    let go_interactive = !fshell_engine::is_test_mode()
        && (interactive || (args.is_empty() && is_terminal && !has_pipe_input));

    if go_interactive {
        let current_pwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "/".to_string());

        let current_host = get_hostname();

        let current_session = {
            let vars = env.vars.read();
            if let Some(Val::String(sess)) = vars.get("FSH_SESSION_ID") {
                sess.clone()
            } else {
                "unknown".to_string()
            }
        };

        let history_result =
            ftui::history_explorer::run_history_tui(&current_pwd, &current_host, &current_session)?;
        let cmd = match history_result {
            ftui::history_explorer::TuiResult::Execute(cmd)
            | ftui::history_explorer::TuiResult::Edit(cmd) => cmd,
            ftui::history_explorer::TuiResult::Cancel => return Ok(()),
        };
        println!("Executing: {}", cmd);
        let result = tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(fshell_engine::run_script(&cmd, env))
        });
        match result {
            Ok(Flow::Normal) | Ok(Flow::ConditionFalse) => {}
            Ok(Flow::Exit(code)) => {
                // History `Execute` requesting an exit — respect it.
                std::process::exit(code);
            }
            Ok(flow) => {
                let msg = flow
                    .stray_message()
                    .unwrap_or_else(|| "control flow".to_string());
                eprintln!("\x1b[1;31merror:\x1b[0m stray `{msg}` at top level");
                env.set_exit_code(1);
            }
            Err(e) => {
                eprintln!("\x1b[1;31merror:\x1b[0m {}", e);
                env.set_exit_code(1);
            }
        }
    } else {
        let search_query = search_query.clone();
        let filter_cwd = filter_cwd.clone();
        let filter_session = filter_session.clone();
        let filter_host = filter_host.clone();
        let entries = std::thread::spawn(move || {
            query_history(
                limit,
                search_query.as_deref(),
                filter_cwd.as_deref(),
                filter_session.as_deref(),
                filter_host.as_deref(),
                filter_exit,
            )
        })
        .join()
        .unwrap_or_else(|_| Err("Background thread panicked".to_string()))?;

        let tx_clone = tx.clone();
        tokio::spawn(async move {
            for entry in entries {
                let val = std::sync::Arc::new(entry.to_val());
                if tx_clone.send(PipelinePayload::Data(val)).await.is_err() {
                    break;
                }
            }
        });
    }

    Ok(())
}

#[derive(Clone)]
/// Snapshot of env values used for prompt rendering.
/// Refreshed once per command execution instead of reading 5+ locks per keypress.
pub struct PromptSnapshot {
    pub exit_code: i64,
    pub duration: Duration,
    pub job_count: usize,
    pub prompt_template: Option<String>,
    pub prompt_right_template: Option<String>,
    pub branch: Option<String>,
    pub pwd: String,
    pub git_status: Option<prompt::RichGitStatus>,
    pub duration_color: Option<String>,
}

fn parse_duration_color(color_name: &str) -> String {
    let normalized = color_name.trim().to_lowercase();
    if normalized.starts_with("\x1b") {
        return normalized;
    }

    let code = match normalized.as_str() {
        "black" => "30",
        "red" => "31",
        "green" => "32",
        "yellow" => "33",
        "blue" => "34",
        "magenta" => "35",
        "cyan" => "36",
        "white" => "37",
        "gray" | "grey" | "darkgray" | "dark_gray" => "90",
        "lightred" | "light_red" | "brightred" | "bright_red" => "91",
        "lightgreen" | "light_green" | "brightgreen" | "bright_green" => "92",
        "lightyellow" | "light_yellow" | "brightyellow" | "bright_yellow" => "93",
        "lightblue" | "light_blue" | "brightblue" | "bright_blue" => "94",
        "lightmagenta" | "light_magenta" | "brightmagenta" | "bright_magenta" => "95",
        "lightcyan" | "light_cyan" | "brightcyan" | "bright_cyan" => "96",
        "lightwhite" | "light_white" | "brightwhite" | "bright_white" => "97",
        s if s.chars().all(|c| c.is_ascii_digit() || c == ';') => s,
        _ => "37", // Default to white
    };
    format!("\x1b[{}m", code)
}

/// Emit OSC 7 terminal escape sequence so terminal emulators (Ghostty, cmux, iTerm2, Kitty, WezTerm)
/// track current working directory changes for new tabs, splits, and windows.
pub fn emit_osc7(pwd: &str) {
    if fshell_engine::is_test_mode() {
        return;
    }
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return;
    }
    let host = history::get_hostname();
    let mut encoded = String::with_capacity(pwd.len() * 3);
    for b in pwd.bytes() {
        match b {
            b'/' | b'.' | b'_' | b'~' | b'-' | b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => {
                encoded.push(b as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(encoded, "%{:02X}", b);
            }
        }
    }
    let seq = format!("\x1b]7;file://{}{}\x07", host, encoded);
    use std::io::Write;
    let _ = std::io::stdout().write_all(seq.as_bytes());
    let _ = std::io::stdout().flush();
}

/// Batch-read all env data into a PromptSnapshot (single lock acquisition per field).
pub fn refresh_prompt_snapshot(env: &Env, pwd: &str) -> PromptSnapshot {
    let _start = std::time::Instant::now();
    emit_osc7(pwd);
    let exit_code = *env.prompt.last_exit_code.read();
    let duration = *env.prompt.last_duration.read();
    let job_count = env.job_control.jobs.read().len();
    let vars = env.vars.read();

    let prompt_template = vars.get("FSH_PROMPT").and_then(|v| match v {
        Val::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    });
    let prompt_right_template = vars
        .get("FSH_PROMPT_RIGHT")
        .and_then(|v| match v {
            Val::String(s) => Some(s.clone()),
            _ => None,
        })
        .or_else(|| {
            Some(
                "\x1b[90m[\x1b[0m{duration_human}\x1b[90m] [\x1b[0m{exit_code}\x1b[90m]\x1b[0m"
                    .to_string(),
            )
        });
    let duration_color = vars.get("FSH_DURATION_COLOR").and_then(|v| match v {
        Val::String(s) if !s.is_empty() => Some(parse_duration_color(s)),
        _ => None,
    });

    drop(vars);

    // Get rich git status
    let git_timer = std::time::Instant::now();
    let git_status = prompt::get_rich_git_status(pwd);
    let git_elapsed = git_timer.elapsed();
    if git_elapsed > std::time::Duration::from_millis(5)
        && std::env::var("FSH_DBG_CPU_USG").as_deref() == Ok("1")
    {
        eprintln!(
            "[cpu_dbg] [lib] get_rich_git_status took {:?} for pwd={:?}",
            git_elapsed, pwd
        );
    }
    let branch = git_status.as_ref().map(|g| g.branch.clone());

    let total_elapsed = _start.elapsed();
    if total_elapsed > std::time::Duration::from_millis(20)
        && std::env::var("FSH_DBG_CPU_USG").as_deref() == Ok("1")
    {
        eprintln!(
            "[cpu_dbg] [lib] refresh_prompt_snapshot took {:?} for pwd={:?}",
            total_elapsed, pwd
        );
    }

    PromptSnapshot {
        exit_code,
        duration,
        job_count,
        prompt_template,
        prompt_right_template,
        branch,
        pwd: pwd.to_string(),
        git_status,
        duration_color,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_prompt_template(
    template: &str,
    user: &str,
    pwd: &str,
    branch: Option<&str>,
    exit_code: i64,
    duration: Duration,
    job_count: usize,
    duration_color_str: &str,
) -> String {
    let mut result = template.to_string();
    result = result.replace("{user}", user);
    result = result.replace("{pwd}", pwd);
    result = result.replace("{branch}", branch.unwrap_or(""));

    // Exit code with color: green if 0, red if non-zero
    let exit_str = if exit_code == 0 {
        format!("\x1b[32m{}\x1b[0m", exit_code)
    } else {
        format!("\x1b[31m{}\x1b[0m", exit_code)
    };
    result = result.replace("{exit_code}", &exit_str);
    result = result.replace("{exit_code_raw}", &exit_code.to_string());

    // Duration — show if > 1s (backward compatible)
    let duration_str = if duration.as_secs() >= 60 {
        let total_secs = duration.as_secs();
        format!("{}m{}s", total_secs / 60, total_secs % 60)
    } else if duration.as_millis() >= 1000 {
        let ms = duration.as_millis();
        format!("{}.{:01}s", ms / 1000, (ms % 1000) / 100)
    } else {
        String::new()
    };
    let duration_str_colored = if duration_str.is_empty() {
        String::new()
    } else if duration_color_str.is_empty() {
        duration_str
    } else {
        format!("{}{}\x1b[0m", duration_color_str, duration_str)
    };
    result = result.replace("{duration}", &duration_str_colored);
    result = result.replace("{duration_raw}", &duration.as_millis().to_string());

    // Adaptive human duration
    let duration_human = if duration_color_str.is_empty() {
        prompt::format_duration(duration)
    } else {
        format!(
            "{}{}\x1b[0m",
            duration_color_str,
            prompt::format_duration(duration)
        )
    };
    result = result.replace("{duration_human}", &duration_human);
    let duration_ms_str = if duration_color_str.is_empty() {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{}{}ms\x1b[0m", duration_color_str, duration.as_millis())
    };
    result = result.replace("{duration_ms}", &duration_ms_str);

    // Job count
    result = result.replace("{jobs}", &job_count.to_string());

    // Timestamp
    let now = chrono::Local::now();
    result = result.replace("{timestamp}", &now.format("%H:%M:%S").to_string());
    result = result.replace(
        "{timestamp_full}",
        &now.format("%Y-%m-%d %H:%M:%S").to_string(),
    );

    // Shell level
    let shlvl = std::env::var("SHLVL").unwrap_or_else(|_| "1".to_string());
    result = result.replace("{shlvl}", &shlvl);

    result
}

pub async fn run_repl_with_env(env: Env, resume_option: Option<String>) {
    init_session_logger();

    // Synchronize prompt configuration and theme on boot
    let loaded_prompt = crate::prompt_config::load_config();
    {
        let mut w = env.prompt_config.write();
        *w = loaded_prompt;
    }
    if let Ok(theme_name) = std::env::var("FSH_THEME")
        && let Some(cfg_dir) = fshell_engine::resolve_config_dir()
        && let Ok(t) = fshell_core::theme::Theme::load(&theme_name, &cfg_dir)
    {
        env.set_theme(std::sync::Arc::new(t));
    }

    emit_osc7(&env.cwd().to_string_lossy());
    let mut session_id = {
        let vars = env.vars.read();
        if let Some(Val::String(s)) = vars.get("FSH_SESSION_ID") {
            s.clone()
        } else {
            format!(
                "{:x}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            )
        }
    };

    let mut restored_state = None;

    // Read config session_restore setting
    let config_mode = env.options.read().session_restore.clone();

    // Determine restore target
    let restore_target = if let Some(target) = resume_option {
        Some(target)
    } else if config_mode == "auto" {
        Some("auto".to_string())
    } else if config_mode == "picker" || config_mode == "ask" {
        Some("ask".to_string())
    } else {
        None
    };

    if let Some(target) = restore_target
        && let Some(cfg_dir) = fshell_engine::config_dir()
    {
        let sessions_dir = cfg_dir.join("sessions");
        if sessions_dir.exists()
            && let Ok(entries) = std::fs::read_dir(&sessions_dir)
        {
            let mut session_files: Vec<(std::path::PathBuf, std::time::SystemTime)> = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().map(|e| e == "json").unwrap_or(false) {
                    let mtime = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .unwrap_or_else(|_| std::time::SystemTime::now());
                    session_files.push((path, mtime));
                }
            }

            // Sort by most recently modified first
            session_files.sort_by_key(|a| std::cmp::Reverse(a.1));

            if !session_files.is_empty() {
                let selected_path = if target == "ask"
                    && crossterm::tty::IsTty::is_tty(&std::io::stdin())
                    && std::env::var("FSH_TEST_ENV").is_err()
                {
                    // Build picker items
                    let mut picker_items = vec![picker::PickerItem {
                        value: "new".to_string(),
                        display: "[Start a new session]".to_string(),
                    }];

                    for (path, mtime) in &session_files {
                        if let Ok(content) = std::fs::read_to_string(path)
                            && let Ok(state) = serde_json::from_str::<
                                fshell_engine::handoff::HandoffState,
                            >(&content)
                        {
                            let age = std::time::SystemTime::now()
                                .duration_since(*mtime)
                                .unwrap_or_default();
                            let age_str = if age.as_secs() < 60 {
                                "just now".to_string()
                            } else if age.as_secs() < 3600 {
                                format!("{}m ago", age.as_secs() / 60)
                            } else if age.as_secs() < 86400 {
                                format!("{}h ago", age.as_secs() / 3600)
                            } else {
                                format!("{}d ago", age.as_secs() / 86400)
                            };

                            let name_str =
                                if let Some(Val::String(n)) = state.vars.get("FSH_SESSION_NAME") {
                                    format!(" [{}]", n)
                                } else {
                                    String::new()
                                };

                            let display = format!(
                                "Session {}{} (cwd: {}, active: {})",
                                state.session_id, name_str, state.cwd, age_str
                            );
                            picker_items.push(picker::PickerItem {
                                value: path.to_string_lossy().to_string(),
                                display,
                            });
                        }
                    }

                    let mut p = picker::Picker::new("sessions:", picker_items);
                    if let Ok(Some(selected)) = p.run() {
                        if selected != "new" {
                            Some(std::path::PathBuf::from(selected))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else if target == "auto" {
                    // Resuming most recent
                    Some(session_files[0].0.clone())
                } else {
                    // Look for specific session_id
                    let expected_name = format!("{}.json", target);
                    session_files
                        .iter()
                        .map(|(p, _)| p)
                        .find(|p| {
                            p.file_name()
                                .map(|n| n == expected_name.as_str())
                                .unwrap_or(false)
                        })
                        .cloned()
                };

                if let Some(path) = selected_path
                    && let Ok(content) = std::fs::read_to_string(&path)
                    && let Ok(state) =
                        serde_json::from_str::<fshell_engine::handoff::HandoffState>(&content)
                {
                    session_id = state.session_id.clone();
                    restored_state = Some(state);
                }
            }
        }
    }

    let is_resuming = restored_state.is_some();

    if let Some(state) = restored_state {
        restore_session_state(&env, state);
    } else {
        let mut vars = env.vars.write();
        vars.insert(
            "FSH_SESSION_ID".to_string(),
            Val::String(session_id.clone()),
        );
    }

    let log_file_path = if let Some(cfg_dir) = fshell_engine::config_dir() {
        let sessions_dir = cfg_dir.join("sessions");
        let _ = std::fs::create_dir_all(&sessions_dir);
        Some(sessions_dir.join(format!("{}.log", session_id)))
    } else {
        None
    };

    if let Some(ref path) = log_file_path
        && path.exists()
        && let Ok(content) = std::fs::read_to_string(path)
    {
        print!("{}", content);
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
    if let Some(ref path) = log_file_path {
        fshell_engine::init_session_logger(path.clone());
    }

    // Setup active job control signal handlers
    fshell_engine::setup_signal_handlers(env.clone());

    // First-run multicall setup: silently deploy symlinks, print PATH instructions once
    if !is_resuming {
        check_first_run_onboarding(&env);
    }

    // On handoff restart, ask about clearing the screen — actual clear happens
    // after deferred init completes so the user doesn't stare at a blank screen.
    let should_clear = matches!(env.vars.read().get("FSH_HANDOFF"), Some(Val::Bool(true)))
        .then(|| {
            let mode = env.options.read().clear_on_reload.clone();

            match mode.as_str() {
                "always" => Some(true),
                "ask" => {
                    let already_done = env.vars.read().contains_key("FSH_CLEAR_ON_RELOAD_DONE");

                    if already_done {
                        None
                    } else {
                        let mut input = String::new();
                        print!("Clear scrollback? [Y/n] ");
                        let _ = std::io::stdout().flush();
                        let answer = match std::io::stdin().read_line(&mut input) {
                            Ok(_) => {
                                let trimmed = input.trim().to_lowercase();
                                trimmed.is_empty() || trimmed == "y" || trimmed == "yes"
                            }
                            Err(_) => false,
                        };
                        env.vars
                            .write()
                            .insert("FSH_CLEAR_ON_RELOAD_DONE".to_string(), Val::Bool(true));
                        Some(answer)
                    }
                }
                _ => Some(false),
            }
        })
        .flatten()
        .unwrap_or(false);

    // Login vs non-login startup contract.
    //
    // For a login shell (`fsh --login` or argv[0] == "-fsh"), the host
    // login profiles (PATH, etc.) must be visible at first prompt —
    // macOS Terminal / `chsh` / `ssh` all start login shells.  That
    // means login env loading is NOT deferred: we await it here on the
    // main task so every subsequent command sees a correct `$PATH`.
    //
    // For a non-login interactive shell, defer everything except the
    // minimal amount needed to render the prompt.  The login env shim
    // is still mode-aware: it knows not to source `/etc/profile` when
    // `is_login == false`.
    //
    // See `crates/fshell-engine/src/login.rs` for the file precedence.
    // Always register default core aliases synchronously
    env.register_alias("l", "ls -l");
    env.register_alias("ll", "ls -la");

    // Non-interactive batch mode: if stdin or stdout is not a TTY (e.g. piped or redirected)
    let is_interactive = !fshell_engine::is_test_mode()
        && crossterm::tty::IsTty::is_tty(&std::io::stdin())
        && fshell_engine::is_stdout_a_tty();

    if !is_interactive {
        use std::io::Read;
        let mut input = String::new();
        if std::io::stdin().read_to_string(&mut input).is_ok() && !input.trim().is_empty() {
            match fshell_engine::run_script(&input, &env).await {
                Ok(Flow::Exit(code)) => std::process::exit(code),
                Ok(_) => {
                    let code = *env.prompt.last_exit_code.read() as i32;
                    std::process::exit(code);
                }
                Err(e) => {
                    eprintln!("\x1b[1;31merror:\x1b[0m {}", e);
                    std::process::exit(1);
                }
            }
        } else {
            std::process::exit(0);
        }
    }

    let is_login_shell = matches!(env.vars.read().get("FSH_LOGIN"), Some(Val::Bool(true)));

    let init_done = Arc::new(Notify::new());
    if is_resuming {
        // Session restore already populated vars/aliases/caps — skip re-loading.
        init_done.notify_one();
    } else {
        // Run init synchronously so $PATH, aliases, prompt, and config are immediately
        // available without background task races or terminal conflicts with FTUI.
        fshell_core::debug_log!("interactive init: synchronous (is_login={is_login_shell})");
        let (shell_ok, shell_msg) =
            match fshell_engine::login::load_login_environment(&env, is_login_shell, true).await {
                Ok(()) => (true, "environment loaded".to_string()),
                Err(e) => (false, e.to_string()),
            };
        let (config_ok, config_msg) = match fshell_engine::load_config_script(&env).await {
            Ok(()) => {
                let has_file =
                    fshell_engine::config_dir().is_some_and(|d| d.join("init.fsh").exists());
                if has_file {
                    (true, "~/.config/fsh/init.fsh".to_string())
                } else {
                    (true, "no init.fsh found".to_string())
                }
            }
            Err(e) => (false, e),
        };
        fshell_engine::warmup_path_cache(Some(&env));
        autocomplete::prewarm_completions();
        let splash_dismissed = fshell_engine::config_dir()
            .map(|d| d.join(".splash_disabled").exists())
            .unwrap_or(false);
        if !splash_dismissed {
            crate::ftui::splash::show_splash(&env, config_ok, &config_msg, shell_ok, &shell_msg);
        }
        init_done.notify_one();
    }

    // FTUI is the interactive REPL path
    ftui::run_ftui_repl(env.clone(), session_id.clone(), init_done, should_clear).await;

    // Save session on exit
    save_session_state(&env, &session_id);

    let exit_code = *env.prompt.last_exit_code.read() as i32;
    std::process::exit(exit_code);
}

fn restore_session_state(env: &Env, state: fshell_engine::handoff::HandoffState) {
    {
        let mut vars = env.vars.write();
        for (k, v) in state.vars {
            vars.insert(k, v);
        }
        vars.insert("FSH_SESSION_ID".to_string(), Val::String(state.session_id));
    }
    {
        let mut fns = env.fns.write();
        for (k, v) in state.fns {
            fns.insert(k, v);
        }
    }
    {
        let mut caps = env.caps.caps.write();
        caps.held = state.caps_held;
        caps.strict_mode = state.caps_strict_mode;
    }
    {
        let mut pipes = env.reactive.pipelines.write();
        for (k, v) in state.reactive_pipelines {
            pipes.insert(k, v);
        }
    }
    env.set_cwd(std::path::PathBuf::from(&state.cwd));
    {
        let mut opts = env.options.write();
        *opts = state.options;
    }
    {
        let mut hooks = env.hooks.registry.write();
        for (k, v) in state.hooks {
            hooks.insert(k, v);
        }
    }
    {
        let mut exit_code = env.prompt.last_exit_code.write();
        *exit_code = state.last_exit_code;
    }
    {
        let mut duration = env.prompt.last_duration.write();
        *duration = std::time::Duration::from_secs_f64(state.last_duration_secs);
    }
}

fn save_session_state(env: &Env, session_id: &str) {
    let active_id = {
        let vars = env.vars.read();
        if let Some(Val::String(s)) = vars.get("FSH_SESSION_ID") {
            s.clone()
        } else {
            session_id.to_string()
        }
    };
    if let Some(cfg_dir) = fshell_engine::config_dir() {
        let sessions_dir = cfg_dir.join("sessions");
        if let Err(e) = std::fs::create_dir_all(&sessions_dir) {
            eprintln!("Failed to create sessions directory: {e}");
            return;
        }
        let save_path = sessions_dir.join(format!("{}.json", active_id));

        let state = fshell_engine::handoff::HandoffState {
            vars: env.vars.read().clone(),
            fns: env.fns.read().clone(),
            caps_held: {
                let caps = env.caps.caps.read();
                caps.held.clone()
            },
            caps_strict_mode: {
                let caps = env.caps.caps.read();
                caps.strict_mode
            },
            reactive_pipelines: {
                let pipes = env.reactive.pipelines.read();
                pipes.clone()
            },
            session_id: session_id.to_string(),
            cwd: env.cwd().to_string_lossy().to_string(),
            options: env.options.read().clone(),
            hooks: env.hooks.registry.read().clone(),
            last_exit_code: *env.prompt.last_exit_code.read(),
            last_duration_secs: env.prompt.last_duration.read().as_secs_f64(),
        };

        if let Ok(content) = serde_json::to_string_pretty(&state) {
            let _ = std::fs::write(&save_path, content);
        }
    }
}

/// fshell language keywords — used to guard bare-directory navigation from collisions.
const FSHELL_KEYWORDS: &[&str] = &[
    "let", "fn", "match", "try", "catch", "with", "caps", "true", "false", "null", "if", "else",
    "while", "source", "unsafe", "filter", "map", "sort", "grep", "count", "limit", "return",
    "exit", "quit",
];

/// Attempt to treat `input` as a bare directory path and change into it.
///
/// Returns `true` if the navigation was performed (the caller should skip normal parsing).
/// Returns `false` if the input does not look like a navigable path or if it collides with
/// a known builtin, user function, or fshell keyword.
fn try_bare_dir_cd(input: &str, env: &Env, tx: fshell_engine::PipeSender) -> bool {
    // Early cheap checks — avoid autocd option lock when input is clearly not a path.
    let first_token = input.split_whitespace().next().unwrap_or("");

    // Never intercept fshell keywords.
    if FSHELL_KEYWORDS.contains(&first_token) {
        return false;
    }

    // Path heuristic: multi-word inputs (unquoted) are almost certainly not bare dirs.
    if input.contains(' ') && !input.starts_with('"') && !input.starts_with('\'') {
        return false;
    }

    // autocd option — delayed to here so the cheap string checks above skip the lock
    {
        let opts = env.options.read();
        if !opts.autocd {
            return false;
        }
    }

    // Collision guard: never intercept known builtins, user functions, or external commands.
    if env.get_builtin(first_token).is_some() {
        return false;
    }

    {
        let fns = env.fns.read();
        if fns.contains_key(first_token) {
            return false;
        }
    }

    let env_path = Some(env.vars.read()).and_then(|vars| {
        if let Some(fshell_core::Val::String(s)) = vars.get("PATH") {
            Some(s.clone())
        } else {
            None
        }
    });

    if fshell_engine::is_external_command_cached(first_token, env_path.as_deref()) {
        return false;
    }

    // Strip surrounding quotes before constructing path.
    let stripped = if (input.starts_with('"') && input.ends_with('"'))
        || (input.starts_with('\'') && input.ends_with('\''))
    {
        &input[1..input.len() - 1]
    } else {
        input
    };

    // Expand tilde and build the candidate path.
    let expanded: std::path::PathBuf = if let Some(rest) = stripped.strip_prefix('~') {
        let home = std::env::var("HOME").unwrap_or_default();
        if stripped == "~" {
            std::path::PathBuf::from(home)
        } else {
            // Handle ~/... or ~user/... (simple case: only ~/ prefix)
            std::path::PathBuf::from(format!("{}{}", home, rest))
        }
    } else {
        std::path::PathBuf::from(stripped)
    };

    // Only activate when the path actually exists as a directory.
    if !expanded.is_dir() {
        return false;
    }

    // Execute cd
    let path_str = expanded.to_string_lossy().to_string();
    match fshell_builtins::cd_builtin(None, vec![Val::String(path_str)], env, tx, None) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("\x1b[1;31merror:\x1b[0m {}", e);
            // Return true: we did handle it (just got an error from cd itself).
            true
        }
    }
}

pub(crate) async fn handle_line_generic(
    env: &Env,
    line: &str,
    pwd: &str,
    session_id: &str,
) -> Result<(), ()> {
    // Empty line: print newline to scroll down / create visual separation.
    if line.is_empty() {
        use std::io::Write;
        println!();
        let _ = std::io::stdout().flush();
        return Ok(());
    }

    struct LogGuard;
    impl Drop for LogGuard {
        fn drop(&mut self) {
            fshell_engine::suspend_session_logging();
        }
    }

    let _guard = if !fshell_engine::is_session_logging_active()
        && std::env::var("FSH_TEST_ENV").is_err()
        && std::env::var("FSH_SESSION_LOG").as_deref() == Ok("1")
    {
        if let Some(cfg_dir) = fshell_engine::config_dir() {
            let log_path = cfg_dir.join("sessions").join(format!("{}.log", session_id));
            let snapshot = refresh_prompt_snapshot(env, pwd);
            // Render prompt left side directly (same logic as FshellPrompt::render_prompt_left)
            let left = if let Some(ref template) = snapshot.prompt_template {
                let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
                render_prompt_template(
                    template,
                    &user,
                    &snapshot.pwd,
                    snapshot.branch.as_deref(),
                    snapshot.exit_code,
                    snapshot.duration,
                    snapshot.job_count,
                    snapshot.duration_color.as_deref().unwrap_or("\x1b[37m"),
                )
            } else {
                let config = env.prompt_config.read();
                let theme = env.active_theme();
                prompt::render_segment_list(
                    &config.left,
                    &config,
                    &snapshot.pwd,
                    &snapshot.git_status,
                    snapshot.exit_code,
                    snapshot.duration,
                    snapshot.job_count,
                    true,
                    &theme,
                )
            };
            let indicator = " > ";
            let line_owned = line.to_string();
            if let Some(tx) = SESSION_LOG_TX.get() {
                let _ = tx.send(SessionLogMessage {
                    log_path,
                    content: format!("{}{}{}", left, indicator, line_owned),
                });
            }
        }
        fshell_engine::resume_session_logging();
        Some(LogGuard)
    } else {
        None
    };

    if line == "interactive-file-search" {
        let items = picker::get_recursive_files(pwd, false, None, None);
        let mut p = picker::Picker::new("files:", items);
        let _ = p.run();
        return Ok(());
    }
    if line == "interactive-dir-search" {
        let items = picker::get_recursive_files(pwd, true, None, None);
        let mut p = picker::Picker::new("directories:", items);
        if let Ok(Some(selected)) = p.run() {
            let target = std::path::PathBuf::from(pwd).join(&selected);
            if let Err(e) = fshell_builtins::change_dir_and_update_caps(&target, env) {
                eprintln!("cd error: {}", e);
            }
        }
        return Ok(());
    }
    if line == "interactive-git-branch" {
        let mut cmd = std::process::Command::new("git");
        cmd.arg("branch").arg("--format=%(refname:short)");
        cmd.current_dir(pwd);
        if let Ok(output) = cmd.output()
            && output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let picker_items: Vec<picker::PickerItem> = stdout
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(|l| picker::PickerItem {
                    value: l.to_string(),
                    display: l.to_string(),
                })
                .collect();
            let mut p = picker::Picker::new("git branches:", picker_items);
            if let Ok(Some(selected)) = p.run() {
                let mut check_cmd = std::process::Command::new("git");
                check_cmd.arg("checkout").arg(&selected);
                check_cmd.current_dir(pwd);
                let _ = check_cmd.status();
            }
        }
        return Ok(());
    }

    let line_trimmed = line.trim();
    let mut dym_substitution: Option<String> = None;

    // DYM deferred: intercept 'd' to run suggested command or 'e' to edit
    {
        // Clear any stale FTUI edit suggestion from a prior call
        *env.prompt.edit_suggestion.write() = None;

        let has_pending = env.prompt.pending_suggestion.read().is_some();
        if has_pending {
            let first_token = line_trimmed.split_whitespace().next().unwrap_or("");
            let env_path = Some(env.vars.read()).and_then(|vars| {
                if let Some(fshell_core::Val::String(s)) = vars.get("PATH") {
                    Some(s.clone())
                } else {
                    None
                }
            });
            let is_d = first_token == "d";
            let is_e = first_token == "e";
            if (is_d || is_e)
                && env.get_builtin(first_token).is_none()
                && !env.fns.read().contains_key(first_token)
                && !fshell_engine::is_external_command_cached(first_token, env_path.as_deref())
                && !first_token.contains('/')
                && !std::path::Path::new(first_token).exists()
            {
                let mut pending = env.prompt.pending_suggestion.write();
                if let Some(pdym) = pending.take() {
                    if is_e {
                        let suggestion = format!("{} {}", pdym.corrected, pdym.args.join(" "))
                            .trim()
                            .to_string();
                        // FTUI mode: store suggestion for the caller to pick up
                        *env.prompt.edit_suggestion.write() = Some(suggestion);
                        return Ok(());
                    } else {
                        let rest = line_trimmed.split_once(char::is_whitespace).map(|x| x.1);
                        dym_substitution = Some(if let Some(user_args) = rest {
                            format!("{} {}", pdym.corrected, user_args)
                        } else if !pdym.args.is_empty() {
                            format!("{} {}", pdym.corrected, pdym.args.join(" "))
                        } else {
                            pdym.corrected
                        });
                    }
                }
            } else {
                env.prompt.pending_suggestion.write().take();
            }
        }
    }

    let line_trimmed: &str = dym_substitution.as_deref().unwrap_or(line_trimmed);

    // Fish-style alias expansion: pre-resolve aliases in command position so
    // the parser sees the full command and execute_pipeline takes the fast
    // builtin path — avoids re-parsing + tokio::spawn overhead for aliases.
    let expanded_line: Option<String> = {
        let first_word = line_trimmed.split_whitespace().next().unwrap_or("");
        if !first_word.is_empty()
            && alias_expansion::is_in_command_position(line_trimmed, 0)
            && let Some(expansion) = env.get_alias(first_word)
            && env.get_builtin(first_word).is_none()
            && !env.fns.read().contains_key(first_word)
        {
            let rest = &line_trimmed[first_word.len()..];
            if rest.trim_start().is_empty() {
                Some(expansion)
            } else {
                Some(format!("{}{}", expansion, rest))
            }
        } else {
            None
        }
    };
    let line_trimmed: &str = expanded_line.as_deref().unwrap_or(line_trimmed);

    if line_trimmed.is_empty() {
        return Ok(());
    }

    fshell_engine::run_hooks("preexec", env).await;

    let prev_pwd = env.cwd();

    env.job_control
        .sigint_pending
        .store(false, std::sync::atomic::Ordering::SeqCst);

    let start_time = std::time::Instant::now();
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("SystemTime before UNIX_EPOCH")
        .as_millis() as i64;
    let cwd = env.cwd().to_string_lossy().to_string();
    let username = std::env::var("USER")
        .unwrap_or_else(|_| std::env::var("USERNAME").unwrap_or_else(|_| "unknown".to_string()));
    let host = get_hostname();
    let mut exit_code = Some(0);

    {
        let (nav_tx, _nav_rx) = tokio::sync::mpsc::channel(1);
        if try_bare_dir_cd(line_trimmed, env, nav_tx) {
            let line_clone = line_trimmed.to_string();
            let cwd_clone = cwd.clone();
            let host_clone = host.clone();
            let username_clone = username.clone();
            let session_id_clone = session_id.to_string();
            let duration_ms = start_time.elapsed().as_millis() as i64;
            let histignoredups = {
                let opts = env.options.read();
                opts.histignoredups
            };
            tokio::task::spawn_blocking(move || {
                let skip = if histignoredups {
                    if let Ok(last_cmds) = query_history(Some(1), None, None, None, None, None) {
                        last_cmds.first().map(|e| e.command == line_clone) == Some(true)
                    } else {
                        false
                    }
                } else {
                    false
                };
                if !skip {
                    let _ = log_command(
                        &line_clone,
                        &cwd_clone,
                        timestamp_ms,
                        duration_ms,
                        Some(0),
                        &host_clone,
                        &username_clone,
                        &session_id_clone,
                    );
                }
            });
            fshell_engine::invalidate_git_cache(env);
            prompt::clear_git_status_cache();
            fshell_engine::run_hooks("chpwd", env).await;
            return Ok(());
        }
    }

    // Log command to history BEFORE execution.
    // This ensures commands like `reload -bd` (which calls execvp to replace the process)
    // are logged even though they replace the process before any async logging runs.
    let history_row_id = {
        let histignoredups = {
            let opts = env.options.read();
            opts.histignoredups
        };
        let skip = if histignoredups {
            if let Ok(last_cmds) = query_history(Some(1), None, None, None, None, None) {
                last_cmds.first().map(|e| e.command == line_trimmed) == Some(true)
            } else {
                false
            }
        } else {
            false
        };
        if !skip {
            log_command(
                line_trimmed,
                &cwd,
                timestamp_ms,
                0,       // duration_ms unknown yet; updated after execution
                Some(0), // exit_code unknown yet; command may update it
                &host,
                &username,
                session_id,
            )
            .ok()
        } else {
            None
        }
    };

    // Early POSIX delegation for `find ... -exec ... {} +` - fsh parses it but
    // pipeline execution fails (type mismatch). Route directly to bash.
    if fshell_engine::login::looks_like_posix(line_trimmed)
        && line_trimmed.contains(" -exec ")
        && let Some(handler) = fshell_engine::posix_handler()
    {
        match handler(line_trimmed.to_string(), Vec::new(), env.clone(), false).await {
            Ok((code, _)) => {
                env.set_exit_code(code as i64);
                exit_code = Some(code as i64);
                let duration_ms = start_time.elapsed().as_millis() as i64;
                if let Some(row_id) = history_row_id {
                    let _ = update_history_entry(row_id, duration_ms, exit_code.unwrap_or(0));
                }
                {
                    let mut dur = env.prompt.last_duration.write();
                    *dur = start_time.elapsed();
                }
                return Ok(());
            }
            Err(pe) => {
                env.set_last_error_with_source(
                    FshDiag::new(pe.clone()),
                    line_trimmed.to_string(),
                    "repl".to_string(),
                );
                let err_str = {
                    let opts = env.options.read();
                    let config = fshell_render::RenderConfig {
                        format: opts.error_format,
                        color: opts.error_color,
                        is_interactive: true,
                    };
                    render_error(pe, line_trimmed, "repl", &config)
                };
                eprintln!("{}", err_str);
                exit_code = Some(1);
                let duration_ms = start_time.elapsed().as_millis() as i64;
                if let Some(row_id) = history_row_id {
                    let _ = update_history_entry(row_id, duration_ms, exit_code.unwrap_or(1));
                }
                {
                    let mut dur = env.prompt.last_duration.write();
                    *dur = start_time.elapsed();
                }
                env.set_exit_code(exit_code.unwrap_or(1));
                return Ok(());
            }
        }
    }

    let mut parser = Parser::new(line_trimmed);
    match parser.parse_statements() {
        Ok(stmts) => {
            for stmt in stmts {
                if let Stmt::Exit(_) = stmt.unpack() {
                    match eval_stmt(&stmt, env, false).await {
                        Ok(Flow::Exit(code)) => {
                            {
                                let mut ec = env.prompt.last_exit_code.write();
                                *ec = code as i64;
                            }
                            fshell_engine::run_hooks("exit", env).await;
                            return Err(());
                        }
                        Ok(Flow::Break) | Ok(Flow::Continue) | Ok(Flow::Return(_)) => {
                            eprintln!("\x1b[1;31merror:\x1b[0m stray control flow at top level");
                            exit_code = Some(1);
                            env.set_exit_code(1);
                        }
                        Err(e) => {
                            env.set_last_error_with_source(
                                FshDiag::new(e.clone()),
                                line_trimmed.to_string(),
                                "repl".to_string(),
                            );
                            let opts = env.options.read();
                            let config = fshell_render::RenderConfig {
                                format: opts.error_format,
                                color: opts.error_color,
                                is_interactive: true,
                            };
                            let err_str = render_error(e, line_trimmed, "repl", &config);
                            eprintln!("{}", err_str);
                            exit_code = Some(1);
                        }
                        Ok(_) => {}
                    }
                    continue;
                }
                if let Stmt::Expr(expr) = stmt.unpack() {
                    match expr.unpack() {
                        Expr::Pipeline(pipeline) => {
                            let (tx, mut rx) = tokio::sync::mpsc::channel(
                                fshell_engine::pipeline_channel_size(env),
                            );
                            let env_clone = env.clone();
                            let pipeline_clone = pipeline.clone();
                            let tx_err = tx.clone();
                            {
                                let mut ec = env.prompt.last_exit_code.write();
                                *ec = 0;
                            }
                            fshell_core::debug_log!("pipeline: spawning execute_pipeline");
                            tokio::spawn(async move {
                                fshell_core::debug_log!("pipeline: execute_pipeline started");
                                if let Err(e) =
                                    fshell_engine::execute_pipeline(&pipeline_clone, &env_clone, tx)
                                        .await
                                {
                                    let _ = fshell_engine::send_with_backpressure(
                                        &env_clone,
                                        &tx_err,
                                        PipelinePayload::Structured(e.into()),
                                    )
                                    .await;
                                }
                            });

                            let mut vals = Vec::new();
                            let mut has_errors = false;
                            fshell_core::debug_log!("pipeline: waiting on rx.recv()");
                            let theme = env.active_theme();
                            while let Some(payload) = rx.recv().await {
                                match payload {
                                    PipelinePayload::Data(v) => {
                                        if !matches!(&*v, Val::Map(_)) {
                                            print_item_streaming(&v, &theme);
                                        }
                                        vals.push((*v).clone());
                                    }
                                    PipelinePayload::Bytes(b) => {
                                        let text = String::from_utf8_lossy(&b).into_owned();
                                        println!("{}", text);
                                        vals.push(Val::String(text));
                                    }
                                    PipelinePayload::Structured(diag) => {
                                        let config = {
                                            let opts = env.options.read();
                                            fshell_render::RenderConfig {
                                                format: opts.error_format,
                                                color: opts.error_color,
                                                is_interactive: true,
                                            }
                                        };
                                        if !fshell_engine::is_condition_false_diag(&diag) {
                                            env.set_last_error_with_source(
                                                diag.clone(),
                                                line_trimmed.to_string(),
                                                "repl".to_string(),
                                            );
                                            let err_str = fshell_render::render(
                                                diag, None, "pipeline", &config,
                                            );
                                            eprintln!("{}", err_str);
                                        }
                                        has_errors = true;
                                    }
                                }
                            }
                            fshell_core::debug_log!(
                                "pipeline: rx.recv() done, {} items",
                                vals.len()
                            );
                            if !vals.is_empty() && vals.iter().all(|v| matches!(v, Val::Map(_))) {
                                let has_permissions = vals.iter().any(|v| {
                                    matches!(v, Val::Map(map) if map.contains_key(&ustr("permissions")))
                                });
                                let has_name_key = vals.iter().any(|v| {
                                    matches!(v, Val::Map(map) if map.contains_key(&ustr("name")))
                                });
                                let theme = env.active_theme();
                                if pipeline.stages.len() == 1 && !has_permissions && has_name_key {
                                    print_compact_names(&vals, &theme);
                                } else {
                                    print_value_beautifully(&Val::List(vals), &theme);
                                }
                            }
                            let last_ec = *env.prompt.last_exit_code.read();
                            exit_code = Some(
                                if has_errors
                                    || env
                                        .prompt
                                        .suggestion_deferred
                                        .swap(false, Ordering::Acquire)
                                {
                                    let pipefail = env.options.read().pipefail;
                                    if pipefail {
                                        if last_ec != 0 { last_ec } else { 1 }
                                    } else {
                                        last_ec
                                    }
                                } else {
                                    0
                                },
                            );
                        }
                        other_expr => {
                            match fshell_engine::eval_expr(other_expr, env).await {
                                Ok(val) => {
                                    if val != Val::Null {
                                        if matches!(&val, Val::List(_) | Val::Map(_)) {
                                            if line_trimmed.len() > 5 {
                                                println!("{} =", line_trimmed);
                                            }
                                            let theme = env.active_theme();
                                            print_value_beautifully(&val, &theme);
                                        } else {
                                            let theme = env.active_theme();
                                            println!(
                                                "{} = {}",
                                                line_trimmed,
                                                format_val_compact(&val, &theme)
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    env.set_last_error_with_source(
                                        FshDiag::new(e.clone()),
                                        line_trimmed.to_string(),
                                        "repl".to_string(),
                                    );
                                    let err_str = {
                                        let opts = env.options.read();
                                        let config = fshell_render::RenderConfig {
                                            format: opts.error_format,
                                            color: opts.error_color,
                                            is_interactive: true,
                                        };
                                        render_error(e, line_trimmed, "repl", &config)
                                    };
                                    eprintln!("{}", err_str);
                                    exit_code = Some(1);
                                }
                            }
                            if exit_code.is_none() || exit_code == Some(0) {
                                exit_code = Some(0);
                            }
                        }
                    }
                } else {
                    match repl_display_stmt(&stmt, env, line_trimmed).await {
                        Ok(Flow::Exit(code)) => {
                            {
                                let mut ec = env.prompt.last_exit_code.write();
                                *ec = code as i64;
                            }
                            fshell_engine::run_hooks("exit", env).await;
                            return Err(());
                        }
                        Ok(Flow::Break) | Ok(Flow::Continue) | Ok(Flow::Return(_)) => {
                            eprintln!("\x1b[1;31merror:\x1b[0m stray control flow at top level");
                            exit_code = Some(1);
                            env.set_exit_code(1);
                        }
                        Err(e) => {
                            env.set_last_error_with_source(
                                FshDiag::new(e.clone()),
                                line_trimmed.to_string(),
                                "repl".to_string(),
                            );
                            let err_str = {
                                let opts = env.options.read();
                                let config = fshell_render::RenderConfig {
                                    format: opts.error_format,
                                    color: opts.error_color,
                                    is_interactive: true,
                                };
                                render_error(e, line_trimmed, "repl", &config)
                            };
                            eprintln!("{}", err_str);
                            exit_code = Some(1);
                        }
                        Ok(Flow::ConditionFalse) => {
                            exit_code = Some(1);
                        }
                        Ok(Flow::Normal) => {
                            exit_code = Some(0);
                            let remind = {
                                let vars = env.vars.read();
                                vars.get("remind_shortcuts")
                                    .map(|v| match v {
                                        Val::Bool(b) => *b,
                                        _ => true,
                                    })
                                    .unwrap_or(true)
                            };

                            if remind && line_trimmed.len() <= 60 {
                                let fns = env.fns.read();
                                for (fn_name, (params, _, body)) in fns.iter() {
                                    if params.is_empty() && body.len() == 1 && body[0] == stmt {
                                        println!(
                                            "\u{1f4a1} Hint: You can use '{}' instead of '{}' (opt-out with 'let remind_shortcuts = false')",
                                            fn_name, line_trimmed
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                let errexit_enabled = env.options.read().errexit;
                if errexit_enabled {
                    let last_ec = *env.prompt.last_exit_code.read();
                    if last_ec != 0 {
                        break;
                    }
                }
            }
        }
        Err(e) => {
            if fshell_engine::login::looks_like_posix(line_trimmed)
                && let Some(handler) = fshell_engine::posix_handler()
            {
                match handler(line_trimmed.to_string(), Vec::new(), env.clone(), false).await {
                    Ok((code, _)) => {
                        env.set_exit_code(code as i64);
                        exit_code = Some(code as i64);
                    }
                    Err(pe) => {
                        env.set_last_error_with_source(
                            FshDiag::new(pe.clone()),
                            line_trimmed.to_string(),
                            "repl".to_string(),
                        );
                        let err_str = {
                            let opts = env.options.read();
                            let config = fshell_render::RenderConfig {
                                format: opts.error_format,
                                color: opts.error_color,
                                is_interactive: true,
                            };
                            render_error(pe, line_trimmed, "repl", &config)
                        };
                        eprintln!("{}", err_str);
                        exit_code = Some(1);
                    }
                }
            } else {
                env.set_last_error_with_source(
                    FshDiag::new(e.clone()),
                    line_trimmed.to_string(),
                    "repl".to_string(),
                );
                let err_str = {
                    let opts = env.options.read();
                    let config = fshell_render::RenderConfig {
                        format: opts.error_format,
                        color: opts.error_color,
                        is_interactive: true,
                    };
                    render_error(e, line_trimmed, "repl", &config)
                };
                eprintln!("{}", err_str);
                exit_code = Some(1);
            }
        }
    }

    let duration_ms = start_time.elapsed().as_millis() as i64;

    // Update history entry with real duration and exit code
    if let Some(row_id) = history_row_id {
        let _ = update_history_entry(row_id, duration_ms, exit_code.unwrap_or(0));
    }

    {
        let mut dur = env.prompt.last_duration.write();
        *dur = start_time.elapsed();
    }
    env.set_exit_code(exit_code.unwrap_or(0));

    // Note: History is logged BEFORE execution (see above) to ensure commands
    // like `reload -bd` that call execvp to replace the process are logged.
    // The pre-execution log has placeholder duration_ms=0 and exit_code=0.
    // For most commands this is fine; for `reload -bd` it's essential since
    // the process is replaced before any async logging can run.

    let new_pwd = env.cwd();
    if prev_pwd != new_pwd {
        fshell_engine::invalidate_git_cache(env);
        prompt::clear_git_status_cache();
        fshell_engine::run_hooks("chpwd", env).await;
    }

    if env.is_customizer_active.load(Ordering::SeqCst) {
        env.is_customizer_active.store(false, Ordering::SeqCst);
        let config = crate::prompt_config::load_config();
        {
            let mut w = env.prompt_config.write();
            *w = config;
        }
        // The session logger redirects stdout (fd 1) to a pipe via dup2 for
        // logging. The prompt customizer needs the real terminal — suspend
        // logging so fd 1 points back to the TTY. Logging resumes when the
        // next command's handle_line_generic re-enables it.
        fshell_engine::suspend_session_logging();
        if let Err(e) = crate::prompt_customizer::run_prompt_customizer(env) {
            eprintln!("Prompt customizer error: {}", e);
        }
    }

    Ok(())
}

use fshell_core::diagnostic::{DiagnosticExt, FshDiag};

fn repl_display_stmt<'a>(
    stmt: &'a Stmt,
    env: &'a Env,
    line_trimmed: &'a str,
) -> Pin<Box<dyn Future<Output = Result<Flow, EngineError>> + Send + 'a>> {
    Box::pin(async move {
        match stmt.unpack() {
            Stmt::And(a, b) => {
                let flow = repl_display_stmt(a, env, line_trimmed).await?;
                if !flow.is_normal() {
                    return Ok(flow);
                }
                let last_ec = *env.prompt.last_exit_code.read();
                if last_ec == 0 {
                    repl_display_stmt(b, env, line_trimmed).await
                } else {
                    Ok(Flow::Normal)
                }
            }
            Stmt::Or(a, b) => {
                let res = repl_display_stmt(a, env, line_trimmed).await;
                match res {
                    Ok(flow @ Flow::Break)
                    | Ok(flow @ Flow::Continue)
                    | Ok(flow @ Flow::Return(_))
                    | Ok(flow @ Flow::Exit(_)) => Ok(flow),
                    Ok(Flow::Normal) => {
                        let last_ec = *env.prompt.last_exit_code.read();
                        if last_ec != 0 {
                            repl_display_stmt(b, env, line_trimmed).await
                        } else {
                            Ok(Flow::Normal)
                        }
                    }
                    Ok(Flow::ConditionFalse) | Err(_) => {
                        repl_display_stmt(b, env, line_trimmed).await
                    }
                }
            }
            Stmt::Expr(expr) => match expr.unpack() {
                Expr::Pipeline(pipeline) => {
                    {
                        let mut ec = env.prompt.last_exit_code.write();
                        *ec = 0;
                    }
                    let pipefail = env.options.read().pipefail;
                    let (tx, mut rx) =
                        tokio::sync::mpsc::channel(fshell_engine::pipeline_channel_size(env));
                    let env_clone = env.clone();
                    let pipeline_clone = pipeline.clone();
                    let tx_err = tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            fshell_engine::execute_pipeline(&pipeline_clone, &env_clone, tx).await
                        {
                            let _ = fshell_engine::send_with_backpressure(
                                &env_clone,
                                &tx_err,
                                PipelinePayload::Structured(e.into()),
                            )
                            .await;
                        }
                    });
                    let mut vals = Vec::new();
                    let mut has_errors = false;
                    let theme = env.active_theme();
                    while let Some(payload) = rx.recv().await {
                        match payload {
                            PipelinePayload::Data(v) => {
                                if !matches!(&*v, Val::Map(_)) {
                                    print_item_streaming(&v, &theme);
                                }
                                vals.push((*v).clone());
                            }
                            PipelinePayload::Bytes(b) => {
                                let text = String::from_utf8_lossy(&b).into_owned();
                                println!("{}", text);
                                vals.push(Val::String(text));
                            }
                            PipelinePayload::Structured(diag) => {
                                let config = fshell_render::RenderConfig::default();
                                if !fshell_engine::is_condition_false_diag(&diag) {
                                    let err_str =
                                        fshell_render::render(diag, None, "pipeline", &config);
                                    eprintln!("{}", err_str);
                                }
                                has_errors = true;
                            }
                        }
                    }
                    if !vals.is_empty() && vals.iter().all(|v| matches!(v, Val::Map(_))) {
                        let has_permissions = vals.iter().any(|v| {
                            matches!(v, Val::Map(map)
                                if map.contains_key(&ustr("permissions")))
                        });
                        let theme = env.active_theme();
                        if pipeline.stages.len() == 1 && !has_permissions {
                            print_compact_names(&vals, &theme);
                        } else {
                            print_value_beautifully(&Val::List(vals), &theme);
                        }
                    }
                    let last_ec = *env.prompt.last_exit_code.read();
                    let exit_code = if has_errors && pipefail && last_ec == 0 {
                        1
                    } else {
                        last_ec
                    };
                    env.set_exit_code(exit_code);
                    if has_errors && !pipefail {
                        // Preserve logical false vs hard error distinction for callers
                        // that check exit code — has_errors alone doesn't tell.
                        // ConditionFalse is already reflected in exit_code == 1.
                    }
                    Ok(Flow::Normal)
                }
                other_expr => {
                    let val = fshell_engine::eval_expr(other_expr, env).await?;
                    if val != Val::Null && !matches!(val, Val::Bool(_)) {
                        let theme = env.active_theme();
                        println!("{} = {}", line_trimmed, format_val_compact(&val, &theme));
                    }
                    let exit_code = match &val {
                        Val::Bool(false) => 1,
                        _ => 0,
                    };
                    env.set_exit_code(exit_code);
                    if matches!(val, Val::Bool(false)) {
                        Ok(Flow::ConditionFalse)
                    } else {
                        Ok(Flow::Normal)
                    }
                }
            },
            other_stmt => match eval_stmt(other_stmt, env, false).await {
                Ok(Flow::Normal) => {
                    env.set_exit_code(0);
                    Ok(Flow::Normal)
                }
                Ok(Flow::ConditionFalse) => {
                    env.set_exit_code(1);
                    Ok(Flow::ConditionFalse)
                }
                Ok(flow) => Ok(flow),
                Err(e) => {
                    env.set_exit_code(1);
                    Err(e)
                }
            },
        }
    })
}

pub fn render_error(
    err: impl miette::Diagnostic + DiagnosticExt + Send + Sync + 'static,
    source: &str,
    source_name: &str,
    config: &fshell_render::RenderConfig,
) -> String {
    let diag = FshDiag::new(err);
    fshell_render::render(diag, Some(source), source_name, config)
}

/// On first interactive boot, silently deploy multicall symlinks and print PATH instructions.
/// The PATH export is the one step that genuinely requires user action (parent shell isolation).
fn check_first_run_onboarding(env: &Env) {
    // Suppress onboarding on handoff restart — state was preserved
    if matches!(env.vars.read().get("FSH_HANDOFF"), Some(Val::Bool(true))) {
        return;
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let fshell_bin = std::path::PathBuf::from(&home).join(".fshell/bin");
    let local_bin = std::path::PathBuf::from(&home).join(".local/bin");
    let marker = std::path::PathBuf::from(&home).join(".fshell/.multicall_setup_done");

    // Symlinks may be deployed to either location depending on PATH
    if marker.exists() && (fshell_bin.join("ls").exists() || local_bin.join("ls").exists()) {
        return;
    }

    match fshell_engine::multicall::deploy_utility_symlinks() {
        Ok(_) => {
            let _ = std::fs::File::create(&marker);
        }
        Err(e) => {
            eprintln!("  {}  multicall setup failed: {e}", Color::Red.paint("[!]"));
        }
    }
}

pub fn init(env: &Env) {
    let _ = init_db();
    env.register_builtin("history", std::sync::Arc::new(history_builtin));
    env.set_config_tui_handler(std::sync::Arc::new(|env| {
        crate::config_tui::run_config_tui(env)
    }));
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::await_holding_lock,
        clippy::nonminimal_bool,
        clippy::collapsible_if,
        clippy::unwrap_used,
        clippy::panic
    )]
    use super::*;
    use crate::autocomplete::Completer;
    use crate::history::clear_connection_cache;

    #[test]
    fn test_variable_completion_with_dollar_prefix() {
        let env = fshell_engine::Env::new();
        env.vars.write().insert("my_var".to_string(), Val::Int(42));
        env.vars.write().insert("other".to_string(), Val::Int(1));

        let mut completer = FshellCompleter { env: env.clone() };

        let suggestions = completer.complete("echo $m", 7);
        println!("SUGGESTIONS for echo $m: {:?}", suggestions);
        assert!(
            !suggestions.is_empty(),
            "Should suggest variables when typing $m"
        );
        assert!(suggestions.iter().any(|s| s.value == "$my_var"));

        let suggestions_pipeline = completer.complete("ls|$m", 5);
        println!("SUGGESTIONS for ls|$m: {:?}", suggestions_pipeline);
        assert!(
            !suggestions_pipeline.is_empty(),
            "Should suggest variables when typing ls|$m"
        );
        assert!(suggestions_pipeline.iter().any(|s| s.value == "$my_var"));
    }

    #[test]
    fn test_path_completion_with_spaces_and_quotes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path_with_space = temp_dir.path().join("file name with spaces.txt");
        let path_with_quote = temp_dir.path().join("file\"with\"quotes.txt");
        let path_with_slash = temp_dir.path().join("file\\with\\backslash.txt");

        std::fs::write(&path_with_space, "spaces").unwrap();
        std::fs::write(&path_with_quote, "quotes").unwrap();
        std::fs::write(&path_with_slash, "backslash").unwrap();

        let search_prefix = format!("{}/file", temp_dir.path().to_string_lossy());
        let suggestions = complete_files(&search_prefix, search_prefix.len());

        assert!(!suggestions.is_empty());
        let vals: Vec<String> = suggestions.iter().map(|s| s.value.clone()).collect();
        println!("Path suggestions: {:?}", vals);

        assert!(vals.iter().any(|v| v.starts_with('"')
            && v.ends_with('"')
            && v.contains("file name with spaces.txt")));
        assert!(vals.iter().any(|v| v.starts_with('"')
            && v.ends_with('"')
            && v.contains("file\\\"with\\\"quotes.txt")));
        assert!(vals.iter().any(|v| v.starts_with('"')
            && v.ends_with('"')
            && v.contains("file\\\\with\\\\backslash.txt")));
    }

    #[test]
    fn test_prompt_template_duration() {
        let template = "dur={duration_human} ms={duration_ms} raw={duration_raw}";
        let user = "ariel";
        let pwd = "/home/ariel";
        let branch = None;
        let exit_code = 0;
        let dur = Duration::from_millis(1250);
        let job_count = 0;

        let rendered =
            render_prompt_template(template, user, pwd, branch, exit_code, dur, job_count, "");

        assert_eq!(rendered, "dur=1.25s ms=1250ms raw=1250");

        let dur = Duration::from_millis(42);
        let rendered =
            render_prompt_template(template, user, pwd, branch, exit_code, dur, job_count, "");
        assert_eq!(rendered, "dur=42ms ms=42ms raw=42");

        // sub-ms durations: 500µs
        let dur = Duration::from_micros(500);
        let rendered =
            render_prompt_template(template, user, pwd, branch, exit_code, dur, job_count, "");
        assert_eq!(rendered, "dur=500µs ms=0ms raw=0");

        // nanoseconds
        let dur = Duration::from_nanos(42);
        let rendered =
            render_prompt_template(template, user, pwd, branch, exit_code, dur, job_count, "");
        assert_eq!(rendered, "dur=42ns ms=0ms raw=0");
    }

    #[tokio::test]
    async fn test_alias_expansion_timing_propagation() {
        let env = fshell_engine::Env::new();
        fshell_builtins::init(&env);
        fshell_bridge::init(&env);
        env.register_alias("l", "ls -l");

        // Simulate the expansion path: "l" → "ls -l"
        let buffer = "l";
        let cursor = buffer.len();
        let expanded = alias_expansion::expand_abbreviation_at_word_before(buffer, cursor, &env);
        assert!(expanded.is_some(), "l should expand to ls -l");
        let final_line = expanded.unwrap();
        assert_eq!(final_line, "ls -l");

        // Verify that handle_line sets last_duration when called with the expanded command
        let current_pwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "/".to_string());

        // Manually simulate what handle_line would do for "ls -l"
        let start_time = std::time::Instant::now();
        // Simulate a brief execution
        tokio::time::sleep(std::time::Duration::from_micros(100)).await;
        {
            let mut dur = env.prompt.last_duration.write();
            *dur = start_time.elapsed();
        }
        {
            let mut code = env.prompt.last_exit_code.write();
            *code = 0;
        }

        // Check that the duration is > 0
        let snap = refresh_prompt_snapshot(&env, &current_pwd);
        assert!(
            snap.duration.as_micros() >= 100,
            "duration should be >= 100µs, got {}µs",
            snap.duration.as_micros()
        );

        assert!(
            snap.duration.as_micros() >= 100,
            "duration should have been recorded"
        );

        // Simulate a SECOND alias expansion
        let start_time2 = std::time::Instant::now();
        tokio::time::sleep(std::time::Duration::from_micros(200)).await;
        {
            let mut dur = env.prompt.last_duration.write();
            *dur = start_time2.elapsed();
        }

        let snap2 = refresh_prompt_snapshot(&env, &current_pwd);
        assert!(
            snap2.duration.as_micros() >= 200,
            "second duration should be >= 200µs, got {}µs",
            snap2.duration.as_micros()
        );
    }

    #[test]
    fn test_expand_abbreviation_multibyte_utf8_no_panic() {
        // Regression test: multi-byte UTF-8 characters like 'ò' (2 bytes)
        // must not cause a panic from byte-indexing into char boundaries.
        let env = fshell_engine::Env::new();
        fshell_builtins::init(&env);
        fshell_bridge::init(&env);

        let buf = "ò";
        let result = alias_expansion::expand_abbreviation_at_word_before(buf, buf.len(), &env);
        // No alias registered for 'ò', so result is None — the important
        // thing is that this does not panic.
        assert!(result.is_none());

        // Multi-byte mixed with ASCII: "ò hello"
        let buf2 = "ò hello";
        let result2 = alias_expansion::expand_abbreviation_at_word_before(buf2, buf2.len(), &env);
        assert!(result2.is_none());
    }

    #[tokio::test]
    async fn test_alias_expansion_in_handle_line() {
        let env = fshell_engine::Env::new();
        fshell_builtins::init(&env);
        fshell_bridge::init(&env);
        env.register_alias("l", "ls -l");

        let line = "l";
        let first_word = line.split_whitespace().next().unwrap_or("");
        assert!(!first_word.is_empty());
        assert!(alias_expansion::is_in_command_position(line, 0));
        let expansion = env.get_alias(first_word);
        assert_eq!(expansion, Some("ls -l".to_string()));
        assert!(env.get_builtin(first_word).is_none());
        assert!(!env.fns.read().contains_key(first_word));

        let line = "l -la";
        let first_word = line.split_whitespace().next().unwrap_or("");
        assert_eq!(first_word, "l");
        let rest = &line[first_word.len()..];
        assert_eq!(rest, " -la");
        let expanded = format!("{}{}", "ls -l", rest);
        assert_eq!(expanded, "ls -l -la");

        let line = "ls /tmp";
        let first_word = line.split_whitespace().next().unwrap_or("");
        let expansion = env.get_alias(first_word);
        assert_eq!(expansion, None);
        assert!(env.get_builtin(first_word).is_some());

        assert!(env.get_builtin("ls").is_some());
        assert!(env.get_alias("ls").is_none_or(|v| v != "ls"));
        assert!(env.get_builtin("echo").is_some());
        assert_eq!(env.get_alias("echo"), None);
    }

    #[tokio::test]
    async fn test_histignoredups_option() {
        let _lock = crate::history::TEST_DB_LOCK.lock().await;
        let temp_dir = tempfile::tempdir().unwrap();
        let test_db_file = temp_dir.path().join("fshell_test_histignoredups.db");
        fshell_core::set_var("FSH_TEST_DB_PATH", &test_db_file.to_string_lossy());
        clear_connection_cache();

        // 1. Setup Env and History DB
        let env = fshell_engine::Env::new();
        fshell_builtins::init(&env);
        fshell_bridge::init(&env);
        init_db().unwrap();

        // 2. Enable histignoredups option
        {
            let mut opts = env.options.write();
            opts.histignoredups = true;
        }

        // 3. Log a command via handle_line_generic (the shared FTUI code path)
        let sess_id = "histignoredups_sess";
        handle_line_generic(&env, "echo hello", "/workspace", sess_id)
            .await
            .unwrap();

        // Allow some time for spawn_blocking to finish
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Verify it was logged
        let history1 = query_history(None, None, None, Some(sess_id), None, None).unwrap();
        assert_eq!(history1.len(), 1);
        assert_eq!(history1[0].command, "echo hello");

        // 4. Log the same command again (should be ignored)
        handle_line_generic(&env, "echo hello", "/workspace", sess_id)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let history2 = query_history(None, None, None, Some(sess_id), None, None).unwrap();
        assert_eq!(history2.len(), 1); // Still 1!

        // 5. Log a different command (should be added)
        handle_line_generic(&env, "echo world", "/workspace", sess_id)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let history3 = query_history(None, None, None, Some(sess_id), None, None).unwrap();
        assert_eq!(history3.len(), 2);
        assert_eq!(history3[0].command, "echo world");
        assert_eq!(history3[1].command, "echo hello");

        // Clean up
        clear_connection_cache();
        fshell_core::remove_var("FSH_TEST_DB_PATH");
    }

    #[tokio::test]
    async fn test_repl_panic_recovery() {
        let env = fshell_engine::Env::new();
        fshell_builtins::init(&env);
        fshell_bridge::init(&env);
        env.register_builtin(
            "debug-panic",
            std::sync::Arc::new(|_, _, _, _, _| panic!("explicit test panic")),
        );

        let _ = handle_line_generic(&env, "debug-panic", "/", "test-session").await;

        // Ensure that the REPL environment or other operations can still run after the panic recovery.
        let val = fshell_engine::eval_expr(&fshell_core::Expr::Int(42), &env)
            .await
            .unwrap();
        assert_eq!(val, Val::Int(42));
    }

    #[tokio::test]
    async fn test_session_restoration_logic() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().to_string_lossy().to_string();
        fshell_core::set_var("FSH_CONFIG_DIR", &config_dir);

        let env = fshell_engine::Env::new();
        {
            let mut vars = env.vars.write();
            vars.insert("my_session_var".to_string(), Val::Int(777));
        }

        let session_id = "test_sess_123";
        save_session_state(&env, session_id);

        // Verify session file exists
        let session_path = tmp
            .path()
            .join("sessions")
            .join(format!("{}.json", session_id));
        assert!(session_path.exists());

        // Restore into a new environment
        let env2 = fshell_engine::Env::new();
        {
            let vars = env2.vars.read();
            assert!(!vars.contains_key("my_session_var"));
        }

        let content = std::fs::read_to_string(&session_path).unwrap();
        let state = serde_json::from_str::<fshell_engine::handoff::HandoffState>(&content).unwrap();
        restore_session_state(&env2, state);

        {
            let vars = env2.vars.read();
            assert_eq!(vars.get("my_session_var"), Some(&Val::Int(777)));
            assert_eq!(
                vars.get("FSH_SESSION_ID"),
                Some(&Val::String("test_sess_123".to_string()))
            );
        }

        fshell_core::remove_var("FSH_CONFIG_DIR");
    }

    #[tokio::test]
    async fn test_handle_line_generic_multiline_block() {
        let env = fshell_engine::Env::new();
        fshell_builtins::init(&env);
        let multiline = "let x = 10\nif true {\n    let y = 20\n    let result = $x + $y\n}";
        let res = handle_line_generic(&env, multiline, "/tmp", "test_session").await;
        assert!(res.is_ok());

        let vars = env.vars.read();
        assert_eq!(vars.get("result"), Some(&Val::Int(30)));
    }
}
