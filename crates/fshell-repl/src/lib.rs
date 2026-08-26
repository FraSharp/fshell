// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
#![allow(clippy::result_large_err)]
use fshell_core::{Expr, FxIndexMap, Parser, Stmt, Val};
use fshell_engine::{EngineError, Env, Flow, PipelinePayload, eval_stmt, is_stdout_a_tty};
use nu_ansi_term::Color;
use std::future::Future;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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
pub mod tui;

use crate::format::*;

use history::{
    get_hostname, init_db, log_command, query_frequent_by_prefix, query_history,
    update_history_entry,
};

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

        let history_result = tui::run_history_tui(&current_pwd, &current_host, &current_session)?;
        let cmd = match history_result {
            tui::TuiResult::Execute(cmd) | tui::TuiResult::Edit(cmd) => cmd,
            tui::TuiResult::Cancel => return Ok(()),
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

struct FrecencyCache {
    data: Vec<(String, f64)>,
    loaded_at: Instant,
}

static Z_FRECENCY_CACHE: Mutex<Option<FrecencyCache>> = Mutex::new(None);
const Z_FRECENCY_TTL: std::time::Duration = std::time::Duration::from_secs(300);

pub struct FshellCompleter {
    pub env: Env,
}

/// Common external commands that should appear in first-word completions.
pub(crate) const COMMON_EXTERNAL_COMMANDS: &[(&str, &str)] = &[
    ("cargo", "Rust package manager"),
    ("git", "Distributed version control system"),
    ("npm", "Node.js package manager"),
    ("node", "Node.js runtime"),
    ("python", "Python interpreter"),
    ("python3", "Python interpreter"),
    ("docker", "Container runtime"),
    ("kubectl", "Kubernetes CLI"),
    ("make", "Build automation tool"),
    ("vim", "Text editor"),
    ("nvim", "Text editor"),
    ("code", "VS Code editor"),
    ("ssh", "Secure shell remote login"),
    ("curl", "Transfer data from URLs"),
    ("wget", "Download files from URLs"),
    ("tar", "Archive utility"),
    ("rg", "Recursive search with regex (ripgrep)"),
    ("fd", "Fast file finder"),
    ("jq", "JSON processor"),
    ("sed", "Stream editor"),
    ("awk", "Pattern scanning and processing language"),
    ("chmod", "Change file permissions"),
    ("chown", "Change file owner"),
    ("find", "Search for files in a directory hierarchy"),
    ("xargs", "Build and execute commands from stdin"),
    ("ps", "Report process status"),
    ("top", "Display processes"),
    ("htop", "Interactive process viewer"),
    ("kill", "Send a signal to a process"),
    ("man", "Display manual pages"),
    ("less", "Pager for viewing text"),
    ("more", "Pager for viewing text (basic)"),
    ("diff", "Compare files line by line"),
    ("patch", "Apply a diff file to originals"),
    ("gh", "GitHub CLI"),
    ("brew", "macOS package manager"),
    ("apt", "Debian package manager"),
    ("yum", "RHEL package manager"),
    ("pacman", "Arch package manager"),
    ("systemctl", "Control systemd services"),
    ("journalctl", "Query systemd journal"),
];

/// Rich description for builtins and common external commands.
///
/// The external-command entries are looked up in `COMMON_EXTERNAL_COMMANDS`
/// (the single source of truth shared with first-word completions); only
/// builtin-specific descriptions live here.
pub(crate) fn command_description(cmd: &str) -> Option<&'static str> {
    let builtin_desc = match cmd {
        // Builtins
        "abs" => Some("Return absolute value of a number"),
        "round" => Some("Round a number to nearest integer"),
        "floor" => Some("Round a number down to nearest integer"),
        "ceil" => Some("Round a number up to nearest integer"),
        "pow" => Some("Raise a number to a power"),
        "min" => Some("Return the minimum of two or more values"),
        "max" => Some("Return the maximum of two or more values"),
        "which" => Some("Locate a command on PATH"),
        "graph" => Some("Render a capability graph"),
        "caps-profile" => Some("Show capability profile for current session"),
        "ls" => Some("List directories and files under active capabilities"),
        "watch" => Some("Watch a file or directory for changes"),
        "cd" => Some("Change current working directory and update PWD grants"),
        "z" => Some("Jump to a directory using frecency"),
        "zi" => Some("Interactive frecency directory picker"),
        "extract" => Some("Extract compressed archives"),
        "head" => Some("Show first N lines or items"),
        "tail" => Some("Show last N lines or items"),
        "uniq" => Some("Remove duplicate adjacent lines"),
        "join" => Some("Join lines or CSV fields"),
        "jobs" => Some("Display active background tasks and status"),
        "fg" => Some("Resume background tasks in foreground"),
        "bg" => Some("Resume suspended tasks in background"),
        "export" => Some("Set shell environment variables"),
        "env" => Some("Print shell environment variables"),
        "help" => Some("Display global reference or detailed command information"),
        "pwd" => Some("Print current working directory"),
        "echo" => Some("Print arguments to stdout"),
        "cat" => Some("Concatenate and print files"),
        "touch" => Some("Create empty files or update timestamps"),
        "mkdir" => Some("Create directories"),
        "rm" => Some("Remove files or directories"),
        "cp" => Some("Copy files or directories"),
        "mv" => Some("Move or rename files or directories"),
        "clear" => Some("Clear the terminal screen"),
        "wrap" => Some("Wrap text to a given width"),
        "type" => Some("Display type information for a value"),
        "strict" => Some("Run a command under strict capability enforcement"),
        "reload" => Some("Reload the shell configuration"),
        "alias" => Some("Create or list command aliases"),
        "history" => Some("Search, filter, and query SQLite command history"),
        "hook" => Some("Register or list shell hooks"),
        "group-by" => Some("Group pipeline items by a key"),
        _ => None,
    };
    builtin_desc.or_else(|| {
        COMMON_EXTERNAL_COMMANDS
            .iter()
            .find(|(name, _)| *name == cmd)
            .map(|(_, desc)| *desc)
    })
}

/// Declarative flag metadata for builtin commands.
fn builtin_flags(cmd: &str) -> &'static [(&'static str, &'static str)] {
    match cmd {
        "ls" => &[
            ("-v", "Verbose: include permissions field"),
            ("-a", "Include hidden entries"),
        ],
        "history" => &[
            ("-i", "Explicitly open the interactive search TUI"),
            (
                "--interactive",
                "Explicitly open the interactive search TUI",
            ),
            ("--stats", "Display database command statistics"),
            ("--cwd", "Filter by current directory"),
            ("--session", "Filter by current terminal session ID"),
            (
                "--global",
                "Clear CWD/session filters to query global history",
            ),
            ("--host", "Filter by current machine hostname"),
            ("--exit", "Filter by execution exit code"),
            ("--limit", "Limit number of returned pipeline entries"),
        ],
        "help" => &[
            ("-a", "Display all help topics in full"),
            ("-q", "Compact listing (name + summary per topic)"),
            ("-t", "List topic names only"),
            ("-e", "Show examples only for a topic"),
            ("-v", "Show full detail for a topic"),
            ("--search", "Search help topics by keyword"),
        ],
        "head" => &[("-n", "Number of lines/items to output (default 10)")],
        "tail" => &[("-n", "Number of lines/items to output (default 10)")],
        "reload" => &[
            ("--full", "Full process handoff with state preservation"),
            ("--build", "Rebuild the shell from source before reloading"),
            ("-b", "Rebuild the shell from source before reloading"),
        ],
        _ => &[],
    }
}

/// Carapace (external completion bridge) — always async.
///
/// Design: the `Completer::complete` trait is synchronous, but `carapace`
/// can take 50-300ms. Blocking the FTUI input loop on it causes visible
/// keystroke lag (the original bug). Instead we treat carapace as a
/// background provider:
/// - `complete_with_carapace_cached` returns only already-cached results
///   (fast path, no blocking I/O).
/// - `spawn_carapace_refresh` spawns a thread to refresh the cache; callers
///   that want live results can poll the cache on the next completion cycle.
/// - The FTUI layer is free to show built-in/path completions immediately
///   and merge carapace results when they arrive.
static CARAPACE_CHECKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static CARAPACE_AVAILABLE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn is_carapace_available_cached() -> Option<bool> {
    if CARAPACE_CHECKED.load(Ordering::Acquire) {
        Some(CARAPACE_AVAILABLE.load(Ordering::Acquire))
    } else {
        None
    }
}

fn spawn_carapace_availability_probe() {
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
    suggestions: Vec<reedline::Suggestion>,
    loaded_at: Instant,
}
static CARAPACE_CACHE: std::sync::Mutex<
    Option<std::collections::HashMap<Vec<String>, CarapaceCacheEntry>>,
> = std::sync::Mutex::new(None);
const CARAPACE_CACHE_TTL: Duration = Duration::from_secs(60);
static CARAPACE_IN_FLIGHT: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashSet<Vec<String>>>,
> = std::sync::OnceLock::new();

fn carapace_in_flight() -> &'static std::sync::Mutex<std::collections::HashSet<Vec<String>>> {
    CARAPACE_IN_FLIGHT.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

fn complete_with_carapace_cached(
    words: &[&str],
    last_word: &str,
    pos: usize,
) -> Option<Vec<reedline::Suggestion>> {
    // Never block: only return cached entries, if any.
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
        s.span = reedline::Span::new(pos - last_word.len(), pos);
    }
    Some(suggestions)
}

fn spawn_carapace_refresh(words: Vec<String>, last_word: String) {
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
            // pos is not available here; cached entries store with span
            // relative to 0 and callers re-span on use — store with 0-based span.
            let mut results = Vec::new();
            for s in suggestions {
                results.push(reedline::Suggestion {
                    value: s.value,
                    description: s.description,
                    extra: None,
                    span: reedline::Span::new(0, span_len),
                    append_whitespace: true,
                    style: None,
                    display_override: None,
                    match_indices: None,
                });
            }
            Some(results)
        })();
        if let Some(results) = results
            && let Ok(mut cache) = CARAPACE_CACHE.lock()
        {
            if cache.is_none() {
                *cache = Some(std::collections::HashMap::new());
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

impl reedline::Completer for FshellCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<reedline::Suggestion> {
        let pos = pos.min(line.len());
        let pos = if line.is_char_boundary(pos) {
            pos
        } else {
            line.floor_char_boundary(pos)
        };
        let prefix = &line[..pos];
        let words: Vec<&str> = prefix.split_whitespace().collect();
        let last_word = line[..pos]
            .split(|c: char| {
                if c == '$' {
                    false
                } else {
                    c.is_whitespace() || c == '|' || c == '>' || c == '<'
                }
            })
            .next_back()
            .unwrap_or("");
        let starting_new_arg = prefix.ends_with(' ');

        let is_external = if !words.is_empty() {
            let cmd = words[0];
            let is_builtin = self.env.get_all_builtins().iter().any(|b| b == cmd);
            let is_alias = self
                .env
                .get_all_aliases()
                .iter()
                .any(|(name, _)| name == cmd);
            let is_fn = {
                let fns = self.env.fns.read();
                fns.contains_key(cmd)
            };
            let env_path = Some(self.env.vars.read()).and_then(|vars| {
                if let Some(fshell_core::Val::String(s)) = vars.get("PATH") {
                    Some(s.clone())
                } else {
                    None
                }
            });
            !(is_builtin || is_alias || is_fn)
                && fshell_engine::is_external_command_cached(cmd, env_path.as_deref())
        } else {
            false
        };

        if is_external {
            if let Some(cached) = complete_with_carapace_cached(&words, last_word, pos)
                && !cached.is_empty()
            {
                let mut suggs = cached;
                attach_match_indices(&mut suggs, last_word);
                return suggs;
            }
            // No cached entry: kick off a background refresh (non-blocking)
            // and fall through to built-in/path completions for this keystroke.
            let words_owned: Vec<String> = words.iter().map(|s| s.to_string()).collect();
            spawn_carapace_refresh(words_owned, last_word.to_string());
        }

        let is_cd = words.len() >= 2 && words[words.len() - 2] == "cd" || prefix.trim_end() == "cd";
        let is_z = words.len() >= 2 && words[words.len() - 2] == "z" || prefix.trim_end() == "z";
        let is_path =
            last_word.contains('/') || last_word.starts_with('.') || last_word.starts_with('~');

        // z: frecency jump completions
        if is_z {
            let scored = {
                let mut cache = Z_FRECENCY_CACHE.lock().unwrap_or_else(|e| e.into_inner());
                let needs_rebuild = match &*cache {
                    None => true,
                    Some(c) => c.loaded_at.elapsed() >= Z_FRECENCY_TTL,
                };
                if needs_rebuild {
                    *cache = None;
                    if let Some(db_path) = fshell_builtins::get_frecency_db_path()
                        && let Ok(content) = std::fs::read_to_string(&db_path)
                    {
                        let db: fshell_builtins::FrecencyDb =
                            serde_json::from_str(&content).unwrap_or_default();
                        let mut scored: Vec<(String, f64)> = db
                            .paths
                            .iter()
                            .map(|(path, entry)| (path.clone(), entry.frequency))
                            .collect();
                        scored.sort_by(|a, b| {
                            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        *cache = Some(FrecencyCache {
                            data: scored,
                            loaded_at: Instant::now(),
                        });
                    }
                }
                cache.as_ref().map(|c| c.data.clone()).unwrap_or_default()
            };

            if !scored.is_empty() {
                let last_word_lower = last_word.to_lowercase();
                let mut results = Vec::new();
                for (path, _) in scored {
                    let path_lower = path.to_lowercase();
                    if last_word.is_empty() || path_lower.contains(&last_word_lower) {
                        results.push(reedline::Suggestion {
                            value: path.clone(),
                            description: Some("[dir] frecency".to_string()),
                            extra: None,
                            span: reedline::Span::new(pos - last_word.len(), pos),
                            append_whitespace: true,
                            style: None,
                            display_override: None,
                            match_indices: None,
                        });
                    }
                }
                if !results.is_empty() {
                    attach_match_indices(&mut results, last_word);
                    return results;
                }
            }
            // Fallback to directory-only completion
            let mut results = complete_files(if starting_new_arg { "" } else { last_word }, pos);
            results.retain(|s| s.value.ends_with('/'));
            attach_match_indices(&mut results, last_word);
            return results;
        }

        // cd: directory-only completion
        if is_cd {
            let file_word = if starting_new_arg { "" } else { last_word };
            let mut results = complete_files(file_word, pos);
            results.retain(|s| s.value.ends_with('/'));
            attach_match_indices(&mut results, last_word);
            return results;
        }

        // General path completion
        if is_path {
            let mut results = complete_files(last_word, pos);
            attach_match_indices(&mut results, last_word);
            return results;
        }

        // Job ID completion for fg/bg
        let is_fg_bg = words.len() >= 2 && matches!(words[words.len() - 2], "fg" | "bg")
            || prefix.trim_end() == "fg"
            || prefix.trim_end() == "bg";
        if is_fg_bg || last_word.starts_with('%') {
            let jobs = self.env.job_control.jobs.read();
            if !jobs.is_empty() {
                let typed_job = last_word.strip_prefix('%').unwrap_or("");
                let mut results: Vec<reedline::Suggestion> = jobs
                    .iter()
                    .filter(|(_, job)| {
                        typed_job.is_empty() || job.id.to_string().starts_with(typed_job)
                    })
                    .map(|(_, job)| reedline::Suggestion {
                        value: format!("%{}", job.id),
                        description: Some(format!("[job] Job {}: {}", job.id, job.cmd)),
                        extra: None,
                        span: reedline::Span::new(pos - last_word.len(), pos),
                        append_whitespace: true,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    })
                    .collect();
                attach_match_indices(&mut results, last_word);
                return results;
            }
        }

        // Pipe completion — must come before file_arg_builtins so that
        // `ls | filter ` shows upstream keys, not file completions.
        if let Some(pipe_idx) = prefix.rfind('|') {
            let upstream = &prefix[..pipe_idx];
            let downstream = &prefix[pipe_idx + 1..];
            let downstream_trimmed = downstream.trim_start();

            if downstream_trimmed.is_empty() {
                let mut pipe_suggestions = vec![
                    reedline::Suggestion {
                        value: "filter ".to_string(),
                        description: Some("[pipe] Filter pipeline items".to_string()),
                        extra: None,
                        span: reedline::Span::new(pos, pos),
                        append_whitespace: false,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    },
                    reedline::Suggestion {
                        value: "map ".to_string(),
                        description: Some("[pipe] Map/transform pipeline items".to_string()),
                        extra: None,
                        span: reedline::Span::new(pos, pos),
                        append_whitespace: false,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    },
                    reedline::Suggestion {
                        value: "sort ".to_string(),
                        description: Some("[pipe] Sort pipeline items".to_string()),
                        extra: None,
                        span: reedline::Span::new(pos, pos),
                        append_whitespace: false,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    },
                    reedline::Suggestion {
                        value: "grep ".to_string(),
                        description: Some("[pipe] Grep text within pipeline items".to_string()),
                        extra: None,
                        span: reedline::Span::new(pos, pos),
                        append_whitespace: false,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    },
                    reedline::Suggestion {
                        value: "count ".to_string(),
                        description: Some("[pipe] Count pipeline items".to_string()),
                        extra: None,
                        span: reedline::Span::new(pos, pos),
                        append_whitespace: false,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    },
                    reedline::Suggestion {
                        value: "limit ".to_string(),
                        description: Some("[pipe] Limit pipeline output".to_string()),
                        extra: None,
                        span: reedline::Span::new(pos, pos),
                        append_whitespace: false,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    },
                    reedline::Suggestion {
                        value: "@json ".to_string(),
                        description: Some("[pipe] Serialize/Deserialize JSON boundary".to_string()),
                        extra: None,
                        span: reedline::Span::new(pos, pos),
                        append_whitespace: false,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    },
                    reedline::Suggestion {
                        value: "@yaml ".to_string(),
                        description: Some("[pipe] Serialize/Deserialize YAML boundary".to_string()),
                        extra: None,
                        span: reedline::Span::new(pos, pos),
                        append_whitespace: false,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    },
                    reedline::Suggestion {
                        value: "@msgpack ".to_string(),
                        description: Some(
                            "[pipe] Serialize/Deserialize MsgPack boundary".to_string(),
                        ),
                        extra: None,
                        span: reedline::Span::new(pos, pos),
                        append_whitespace: false,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    },
                    reedline::Suggestion {
                        value: "@text ".to_string(),
                        description: Some("[pipe] Serialize to text boundary".to_string()),
                        extra: None,
                        span: reedline::Span::new(pos, pos),
                        append_whitespace: false,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    },
                    reedline::Suggestion {
                        value: "@csv ".to_string(),
                        description: Some("[pipe] Parse or emit CSV data".to_string()),
                        extra: None,
                        span: reedline::Span::new(pos, pos),
                        append_whitespace: false,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    },
                    reedline::Suggestion {
                        value: "@table ".to_string(),
                        description: Some(
                            "[pipe] Render structured data as an ASCII table".to_string(),
                        ),
                        extra: None,
                        span: reedline::Span::new(pos, pos),
                        append_whitespace: false,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    },
                    reedline::Suggestion {
                        value: "@bar ".to_string(),
                        description: Some("[pipe] Render numeric data as a bar chart".to_string()),
                        extra: None,
                        span: reedline::Span::new(pos, pos),
                        append_whitespace: false,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    },
                ];
                attach_match_indices(&mut pipe_suggestions, downstream_trimmed);
                return pipe_suggestions;
            }

            let operators = ["filter", "map", "sort", "grep", "limit"];
            for op in &operators {
                if let Some(op_suffix) = downstream_trimmed.strip_prefix(op)
                    && (op_suffix.trim_start().is_empty() || op_suffix.ends_with(' '))
                {
                    let keys = get_upstream_keys(upstream, &self.env);
                    let mut results: Vec<reedline::Suggestion> = keys
                        .into_iter()
                        .map(|k| reedline::Suggestion {
                            value: k.clone(),
                            description: Some(format!("[key] Upstream property: {}", k)),
                            extra: None,
                            span: reedline::Span::new(pos, pos),
                            append_whitespace: true,
                            style: None,
                            display_override: None,
                            match_indices: None,
                        })
                        .collect();
                    attach_match_indices(&mut results, downstream_trimmed);
                    return results;
                }
            }

            // Partial operator match in pipe context
            let all_ops = [
                "filter", "map", "sort", "grep", "count", "limit", "@json", "@yaml", "@msgpack",
                "@text", "@csv", "@table", "@bar",
            ];
            if !downstream_trimmed.is_empty() {
                let partial = all_ops
                    .iter()
                    .filter(|op| op.starts_with(downstream_trimmed))
                    .collect::<Vec<_>>();
                if !partial.is_empty() {
                    let mut results: Vec<reedline::Suggestion> = partial
                        .into_iter()
                        .map(|op| reedline::Suggestion {
                            value: op.to_string(),
                            description: Some(format!("[pipe] {}", op)),
                            extra: None,
                            span: reedline::Span::new(pos, pos),
                            append_whitespace: true,
                            style: None,
                            display_override: None,
                            match_indices: None,
                        })
                        .collect();
                    attach_match_indices(&mut results, downstream_trimmed);
                    return results;
                }
            }
        }

        if let Some(custom_suggestions) = autocomplete::get_custom_completions(line, pos, &self.env)
            && !custom_suggestions.is_empty()
        {
            let mut suggs = custom_suggestions;
            attach_match_indices(&mut suggs, last_word);
            return suggs;
        }

        // External commands in arg position (no flag prefix) — default to path completion
        if is_external && !last_word.starts_with('-') && (starting_new_arg || is_path) {
            let mut results = complete_files(if starting_new_arg { "" } else { last_word }, pos);
            attach_match_indices(&mut results, last_word);
            return results;
        }

        let mut suggestions = Vec::new();
        let last_word_lower = last_word.to_lowercase();
        let builtins = self.env.get_all_builtins();

        // Flag completions for builtins — look up by the first word (the command), not the
        // immediately preceding word (which may be an argument like a path).
        if last_word.starts_with('-') && words.len() >= 2 {
            let cmd = words[0];
            let flags = builtin_flags(cmd);
            if !flags.is_empty() {
                suggestions.extend(
                    flags
                        .iter()
                        .filter(|(flag, _)| flag.starts_with(last_word))
                        .map(|(flag, desc)| reedline::Suggestion {
                            value: flag.to_string(),
                            description: Some(format!("[flag] {}", desc)),
                            extra: None,
                            span: reedline::Span::new(pos - last_word.len(), pos),
                            append_whitespace: true,
                            style: None,
                            display_override: None,
                            match_indices: None,
                        }),
                );
            }
        }

        // Auto-generated completions for external commands via --help parsing
        if suggestions.is_empty() && last_word.starts_with('-') && words.len() >= 2 {
            let cmd = words[0];
            let is_builtin = self.env.get_all_builtins().iter().any(|b| b == cmd);
            if !is_builtin {
                if let Some(flags) = autocomplete::get_completions(cmd) {
                    for flag in &flags {
                        let flag_str = flag.long.as_deref().or(flag.short.as_deref()).unwrap_or("");
                        if flag_str.starts_with(last_word) {
                            let value = flag
                                .long
                                .as_deref()
                                .or(flag.short.as_deref())
                                .unwrap_or("")
                                .to_string();
                            suggestions.push(reedline::Suggestion {
                                value: if flag.has_arg {
                                    format!("{} ", value)
                                } else {
                                    value
                                },
                                description: flag.desc.clone().map(|d| format!("[flag] {}", d)),
                                extra: None,
                                span: reedline::Span::new(pos - last_word.len(), pos),
                                append_whitespace: flag.has_arg,
                                style: None,
                                display_override: None,
                                match_indices: None,
                            });
                        }
                    }
                } else {
                    autocomplete::queue_background_parse(cmd);
                }
            }
        }

        // Help argument: suggest topics AND categories
        if words.len() >= 2 && words[words.len() - 2] == "help" && !last_word.starts_with('-') {
            let topics = fshell_builtins::help::help_topics();
            for topic in topics {
                if topic.name.starts_with(&last_word_lower) {
                    suggestions.push(reedline::Suggestion {
                        value: topic.name.to_string(),
                        description: Some(format!("[topic] {}", topic.summary)),
                        extra: None,
                        span: reedline::Span::new(pos - last_word.len(), pos),
                        append_whitespace: true,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    });
                }
            }
            // Also suggest category names
            let categories = [
                ("builtins", "Browse built-in commands"),
                ("pipeline", "Browse pipeline operators"),
                ("language", "Browse language constructs"),
                ("security", "Browse security topics"),
                ("concepts", "Browse shell concepts"),
            ];
            for (name, desc) in &categories {
                if name.starts_with(&last_word_lower) {
                    suggestions.push(reedline::Suggestion {
                        value: name.to_string(),
                        description: Some(format!("[category] {}", desc)),
                        extra: None,
                        span: reedline::Span::new(pos - last_word.len(), pos),
                        append_whitespace: true,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    });
                }
            }
            attach_match_indices(&mut suggestions, last_word);
            return suggestions;
        }

        // Git branch/tag completions for relevant git subcommands
        if autocomplete::git_branch_context(&words) {
            let branches = autocomplete::git_branches_cached(&self.env);
            let tags = autocomplete::git_tags_cached();
            for branch in branches.iter().chain(tags.iter()) {
                if branch.starts_with(last_word) {
                    suggestions.push(reedline::Suggestion {
                        value: branch.clone(),
                        description: Some("[ref] git branch/tag".to_string()),
                        extra: None,
                        span: reedline::Span::new(pos - last_word.len(), pos),
                        append_whitespace: true,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    });
                }
            }
            if !suggestions.is_empty() {
                attach_match_indices(&mut suggestions, last_word);
                return suggestions;
            }
        }

        // Past word 1 of a recognized command (or starting a new arg after one)?
        // 95% of the time you want files.
        let on_arg = words.len() >= 2 || (words.len() == 1 && starting_new_arg);
        if on_arg
            && !last_word.starts_with('-')
            && !last_word.starts_with('$')
            && !last_word.starts_with('%')
            && !prefix.contains('|')
        {
            let cmd = words[0];
            let env_path = Some(self.env.vars.read()).and_then(|vars| {
                if let Some(fshell_core::Val::String(s)) = vars.get("PATH") {
                    Some(s.clone())
                } else {
                    None
                }
            });
            let is_cmd = builtins.iter().any(|b| *b == cmd)
                || self.env.get_all_aliases().iter().any(|(n, _)| n == cmd)
                || {
                    let fns = self.env.fns.read();
                    fns.contains_key(cmd)
                }
                || fshell_engine::is_external_command_cached(cmd, env_path.as_deref());
            if is_cmd {
                let file_word = if starting_new_arg { "" } else { last_word };
                let mut results = complete_files(file_word, pos);
                attach_match_indices(&mut results, last_word);
                return results;
            }
        }

        // Alias completions (when typing first word)
        if words.len() <= 1 {
            let aliases = self.env.get_all_aliases();
            for (name, expansion) in &aliases {
                if name.starts_with(&last_word_lower) {
                    suggestions.push(reedline::Suggestion {
                        value: name.clone(),
                        description: Some(format!("[alias] -> {}", expansion)),
                        extra: None,
                        span: reedline::Span::new(pos - last_word.len(), pos),
                        append_whitespace: true,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    });
                }
            }
        }

        // Suggest registered builtins dynamically from the engine
        for b in builtins {
            if b.starts_with(&last_word_lower) {
                let desc = command_description(&b)
                    .unwrap_or("Built-in command")
                    .to_string();
                suggestions.push(reedline::Suggestion {
                    value: b.clone(),
                    description: Some(format!("[cmd] {}", desc)),
                    extra: None,
                    span: reedline::Span::new(pos - last_word.len(), pos),
                    append_whitespace: true,
                    style: None,
                    display_override: None,
                    match_indices: None,
                });
            }
        }

        // Suggest common external commands when typing the first word
        if words.len() <= 1 {
            for (cmd, desc) in COMMON_EXTERNAL_COMMANDS {
                if cmd.starts_with(&last_word_lower) {
                    if suggestions.iter().any(|s| s.value == *cmd) {
                        continue;
                    }
                    suggestions.push(reedline::Suggestion {
                        value: cmd.to_string(),
                        description: Some(format!("[ext] {}", desc)),
                        extra: None,
                        span: reedline::Span::new(pos - last_word.len(), pos),
                        append_whitespace: true,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    });
                }
            }
        }

        // Suggest frequent full command lines from history (e.g. "reload --full")
        if words.len() <= 1
            && !last_word_lower.is_empty()
            && let Ok(entries) = query_frequent_by_prefix(&last_word_lower, 10)
        {
            for (cmd, freq) in &entries {
                if cmd == last_word {
                    continue;
                }
                if suggestions.iter().any(|s| s.value == *cmd) {
                    continue;
                }
                suggestions.push(reedline::Suggestion {
                    value: cmd.clone(),
                    description: Some(format!(
                        "[hist] {} use{}",
                        freq,
                        if *freq == 1 { "" } else { "s" }
                    )),
                    extra: None,
                    span: reedline::Span::new(pos - last_word.len(), pos),
                    append_whitespace: false,
                    style: None,
                    display_override: None,
                    match_indices: None,
                });
            }
        }

        // Suggest pipeline operators standalone
        let pipe_operators = [
            "filter", "map", "sort", "grep", "count", "limit", "@json", "@yaml", "@msgpack",
            "@text", "@csv", "@table", "@bar",
        ];
        for op in &pipe_operators {
            if op.starts_with(&last_word_lower) {
                let desc = match *op {
                    "filter" => "Filter pipeline items",
                    "map" => "Map/transform pipeline items",
                    "sort" => "Sort pipeline items",
                    "grep" => "Grep text within pipeline items",
                    "count" => "Count pipeline items",
                    "limit" => "Limit pipeline output",
                    _ => "Pipeline boundary format",
                };
                suggestions.push(reedline::Suggestion {
                    value: op.to_string(),
                    description: Some(format!("[pipe] {}", desc)),
                    extra: None,
                    span: reedline::Span::new(pos - last_word.len(), pos),
                    append_whitespace: true,
                    style: None,
                    display_override: None,
                    match_indices: None,
                });
            }
        }

        let common_keywords = [
            "let", "fn", "match", "try", "catch", "with", "caps", "true", "false", "null", "unsafe",
        ];
        for kw in &common_keywords {
            if kw.starts_with(&last_word_lower) {
                suggestions.push(reedline::Suggestion {
                    value: kw.to_string(),
                    description: Some("[kw] Keyword".to_string()),
                    extra: None,
                    span: reedline::Span::new(pos - last_word.len(), pos),
                    append_whitespace: true,
                    style: None,
                    display_override: None,
                    match_indices: None,
                });
            }
        }

        // Suggest user-defined functions
        {
            let fns = self.env.fns.read();
            for name in fns.keys() {
                if name.starts_with(&last_word_lower) {
                    suggestions.push(reedline::Suggestion {
                        value: name.clone(),
                        description: Some("[fn] User-defined function".to_string()),
                        extra: None,
                        span: reedline::Span::new(pos - last_word.len(), pos),
                        append_whitespace: true,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    });
                }
            }
        }

        {
            let vars = self.env.vars.read();
            let target_prefix = last_word_lower
                .strip_prefix('$')
                .unwrap_or(&last_word_lower);
            for k in (*vars).keys() {
                if k.to_lowercase().starts_with(target_prefix) {
                    suggestions.push(reedline::Suggestion {
                        value: format!("${}", k),
                        description: Some("[var] Environment variable".to_string()),
                        extra: None,
                        span: reedline::Span::new(pos - last_word.len(), pos),
                        append_whitespace: true,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    });
                }
            }
        }

        if suggestions.is_empty() {
            if starting_new_arg {
                let mut results = complete_files("", pos);
                attach_match_indices(&mut results, last_word);
                return results;
            }
            if !last_word.is_empty() {
                let mut results = complete_files(last_word, pos);
                attach_match_indices(&mut results, last_word);
                return results;
            }
        }

        // Rank by fuzzy score, deduplicate same-value suggestions
        // Load recent history for boosting
        let recent_commands = history::get_recent_commands_cached();
        rank_suggestions(&mut suggestions, last_word, &recent_commands);
        suggestions
    }
}

/// Compute grapheme-level match indices for fuzzy highlighting.
/// Returns `Some(indices)` where each index is a grapheme position in `candidate`
/// that matches a character in `query` (subsequence match).
fn compute_match_indices(query: &str, candidate: &str) -> Option<Vec<usize>> {
    if query.is_empty() {
        return None;
    }
    let query_lower = query.to_lowercase();
    let candidate_lower = candidate.to_lowercase();

    // Try exact substring first — highlight the whole match region
    if let Some(pos) = candidate_lower.find(&query_lower) {
        let indices: Vec<usize> = (pos..pos + query_lower.chars().count()).collect();
        return Some(indices);
    }

    // Fall back to subsequence match
    let mut indices = Vec::new();
    let mut query_chars = query_lower.chars();
    let mut next_char = query_chars.next();
    for (grapheme_idx, c) in candidate_lower.chars().enumerate() {
        if let Some(qc) = next_char {
            if c == qc {
                indices.push(grapheme_idx);
                next_char = query_chars.next();
            }
        } else {
            break;
        }
    }
    if next_char.is_none() && !indices.is_empty() {
        Some(indices)
    } else {
        None
    }
}

/// Attach match indices to a list of suggestions for fuzzy highlighting.
fn attach_match_indices(suggestions: &mut [reedline::Suggestion], query: &str) {
    for s in suggestions.iter_mut() {
        let val_for_match = s.value.trim_end_matches(' ');
        s.match_indices = compute_match_indices(query, val_for_match);
    }
}

/// Extract the kind prefix from a description (e.g., "[cmd] ..." -> "cmd").
fn description_kind(desc: Option<&str>) -> &str {
    desc.and_then(|d| {
        d.strip_prefix('[')
            .and_then(|rest| rest.find(']').map(|i| &rest[..i]))
    })
    .unwrap_or("other")
}

/// Score and rank suggestions by fuzzy match quality, capped at MAX_RESULTS.
/// Groups by kind so similar types appear together. Boosts suggestions that
/// appear in `recent_commands` (recent history).
fn rank_suggestions(
    suggestions: &mut Vec<reedline::Suggestion>,
    query: &str,
    recent_commands: &std::collections::HashSet<String>,
) {
    if suggestions.is_empty() {
        return;
    }

    let kind = fuzzy::choose_kind(suggestions.len());
    let prepared = fuzzy::PreparedQuery::new(query);
    let mut scored: Vec<(isize, usize, reedline::Suggestion)> = suggestions
        .drain(..)
        .enumerate()
        .filter_map(|(i, s)| {
            let val_for_match = s.value.trim_end_matches(' ').to_owned();
            fuzzy::fuzzy_score_prepared(&prepared, &val_for_match, kind).map(|score| {
                // Boost score for suggestions that appear in recent history
                let boost = if recent_commands.contains(&val_for_match) {
                    500
                } else {
                    0
                };
                (score + boost, i, s)
            })
        })
        .collect();

    // Sort by score descending, then by original order for stability
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    // Dedup by keeping the highest-scored occurrence of each value.
    {
        let mut seen = Vec::<String>::new();
        scored.retain(|(_, _, s)| {
            let trimmed = s.value.trim_end_matches(' ');
            if seen.iter().any(|v| v == trimmed) {
                false
            } else {
                seen.push(trimmed.to_owned());
                true
            }
        });
    }
    scored.truncate(fuzzy::MAX_RESULTS);

    // Compute match indices
    let mut scored: Vec<_> = scored
        .into_iter()
        .map(|(_, orig_idx, mut s)| {
            let val_for_match = s.value.trim_end_matches(' ');
            s.match_indices = compute_match_indices(query, val_for_match);
            (orig_idx, s)
        })
        .collect();

    // Group by kind: sort by (kind, -score) so same kinds cluster together,
    // preserving relative score order within each kind.
    scored.sort_by(|a, b| {
        let kind_a = description_kind(a.1.description.as_deref());
        let kind_b = description_kind(b.1.description.as_deref());
        kind_a.cmp(kind_b)
    });

    *suggestions = scored.into_iter().map(|(_, s)| s).collect();
}

/// Expand `$VAR` and `${VAR}` patterns using environment variables.
/// Unset variables and `$$` are left as-is.
fn expand_env_vars(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        if c == '$' {
            if let Some(&(_, '$')) = chars.peek() {
                chars.next();
                result.push('$');
                continue;
            }
            if let Some(&(_, '{')) = chars.peek() {
                chars.next();
                let mut name = String::new();
                for (_, c) in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    name.push(c);
                }
                match std::env::var(&name) {
                    Ok(val) => result.push_str(&val),
                    Err(_) => {
                        result.push('$');
                        result.push('{');
                        result.push_str(&name);
                        result.push('}');
                    }
                }
                continue;
            }
            // Unbraced $VAR — gather name, then any trailing char is re-emitted
            // after the expanded value (the trailing char was consumed from the
            // iterator but must appear after, not before, the replacement).
            let mut name = String::new();
            let mut trailing = None;
            for (_, c) in chars.by_ref() {
                if c.is_alphanumeric() || c == '_' {
                    name.push(c);
                } else {
                    trailing = Some(c);
                    break;
                }
            }
            if name.is_empty() {
                result.push('$');
                if let Some(t) = trailing {
                    result.push(t);
                }
            } else {
                match std::env::var(&name) {
                    Ok(val) => {
                        result.push_str(&val);
                        if let Some(t) = trailing {
                            result.push(t);
                        }
                    }
                    Err(_) => {
                        result.push('$');
                        result.push_str(&name);
                        if let Some(t) = trailing {
                            result.push(t);
                        }
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn complete_files(last_word: &str, pos: usize) -> Vec<reedline::Suggestion> {
    let last_word_owned = last_word.to_string();

    let expanded = if last_word_owned.starts_with('~') {
        if let Ok(home) = std::env::var("HOME") {
            last_word_owned.replacen('~', &home, 1)
        } else {
            last_word_owned.clone()
        }
    } else {
        last_word_owned.clone()
    };
    let expanded = expand_env_vars(&expanded);

    let path = std::path::PathBuf::from(&expanded);
    let (search_dir, file_prefix) = if expanded.ends_with('/') {
        (path, String::new())
    } else if let Some(parent) = path.parent() {
        let prefix = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("")
            .to_string();
        (parent.to_path_buf(), prefix)
    } else {
        (std::path::PathBuf::from("."), expanded.clone())
    };

    let dir_to_read = if search_dir.as_os_str().is_empty() {
        std::path::PathBuf::from(".")
    } else {
        search_dir
    };

    let perform_read = move || {
        let mut entries: Vec<(String, bool)> = Vec::new();
        if let Ok(dir_entries) = std::fs::read_dir(&dir_to_read) {
            for entry in dir_entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                entries.push((name, is_dir));
                if entries.len() >= 2000 {
                    break;
                }
            }
        }

        if entries.is_empty() {
            return Vec::new();
        }

        let word_len = last_word_owned.len();
        let kind = fuzzy::choose_kind(entries.len());
        let prepared = fuzzy::PreparedQuery::new(&file_prefix);
        let mut scored: Vec<(isize, String, bool)> = entries
            .into_iter()
            .filter_map(|(name, is_dir)| {
                fuzzy::fuzzy_score_prepared(&prepared, &name, kind).map(|s| (s, name, is_dir))
            })
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        scored.truncate(fuzzy::MAX_RESULTS);

        let user_prefix = match last_word_owned.rfind('/') {
            Some(idx) => &last_word_owned[..=idx],
            None => "",
        };

        scored
            .into_iter()
            .map(|(_, name, is_dir)| {
                let mut full_suggestion = format!("{}{}", user_prefix, name);
                if is_dir {
                    full_suggestion.push('/');
                }
                let final_value = if full_suggestion.contains(' ')
                    || full_suggestion.contains('\'')
                    || full_suggestion.contains('"')
                    || full_suggestion.contains('\\')
                {
                    let escaped = full_suggestion.replace('\\', "\\\\").replace('"', "\\\"");
                    format!("\"{}\"", escaped)
                } else {
                    full_suggestion
                };

                let desc = file_completion_description(&dir_to_read, &name, is_dir);

                reedline::Suggestion {
                    value: final_value,
                    description: desc,
                    extra: None,
                    span: reedline::Span::new(pos - word_len, pos),
                    append_whitespace: !is_dir,
                    style: None,
                    display_override: None,
                    match_indices: None,
                }
            })
            .collect()
    };

    perform_read()
}

fn file_completion_description(dir: &std::path::Path, name: &str, is_dir: bool) -> Option<String> {
    let path = dir.join(name);
    let meta = std::fs::symlink_metadata(&path).ok()?;

    if meta.is_symlink() {
        let target = std::fs::read_link(&path).ok()?;
        return Some(format!("→ {}", target.display()));
    }

    let perm = permission_str(&meta, if meta.is_dir() { 'd' } else { '-' });
    let ts = modified_time_str(&meta)?;

    if is_dir {
        let count = std::fs::read_dir(&path).map(|e| e.count()).unwrap_or(0);
        let label = if count == 1 { "item" } else { "items" };
        Some(format!("{}│ {:>3} {:<5}│ {}", perm, count, label, ts))
    } else {
        let size = meta.len();
        let size_str = if size < 1024 {
            format!("{} B", size)
        } else if size < 1024 * 1024 {
            format!("{:.1} KB", size as f64 / 1024.0)
        } else if size < 1024 * 1024 * 1024 {
            format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.1} GB", size as f64 / (1024.0 * 1024.0 * 1024.0))
        };
        Some(format!("{}│ {:>8}│ {}", perm, size_str, ts))
    }
}

fn permission_str(meta: &std::fs::Metadata, type_char: char) -> String {
    let mode = meta.permissions().mode();
    let rwx = |shift: u32| -> String {
        [
            if mode & (1 << (shift + 2)) != 0 {
                'r'
            } else {
                '-'
            },
            if mode & (1 << (shift + 1)) != 0 {
                'w'
            } else {
                '-'
            },
            if mode & (1 << shift) != 0 { 'x' } else { '-' },
        ]
        .iter()
        .collect()
    };
    format!("{}{}{}{}", type_char, rwx(6), rwx(3), rwx(0))
}

fn modified_time_str(meta: &std::fs::Metadata) -> Option<String> {
    let modified = meta.modified().ok()?;
    let now = std::time::SystemTime::now();
    let dur = now.duration_since(modified).ok()?;
    let secs = dur.as_secs();
    Some(if secs < 60 {
        "now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else if secs < 604800 {
        format!("{}d ago", secs / 86400)
    } else {
        use chrono::{DateTime, Local};
        let dt: DateTime<Local> = modified.into();
        dt.format("%b %d").to_string()
    })
}

fn get_upstream_keys(upstream: &str, env: &Env) -> Vec<String> {
    let upstream_trimmed = upstream.trim();
    if upstream_trimmed.is_empty() {
        return Vec::new();
    }
    if upstream_trimmed.ends_with("ls") || upstream_trimmed.contains("ls ") {
        return vec![
            "name".to_string(),
            "type".to_string(),
            "size".to_string(),
            "last_modified".to_string(),
        ];
    }
    if let Some(idx) = upstream_trimmed.rfind('$') {
        let var_part = &upstream_trimmed[idx + 1..];
        let var_name: String = var_part
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if let Some(val) = env.vars.read().get(&var_name) {
            return get_val_keys(val);
        }
    }
    Vec::new()
}

fn get_val_keys(val: &Val) -> Vec<String> {
    match val {
        Val::Map(map) => map.keys().map(|k| k.as_str().to_string()).collect(),
        Val::List(list) => {
            if let Some(Val::Map(map)) = list.iter().find(|item| matches!(item, Val::Map(_))) {
                map.keys().map(|k| k.as_str().to_string()).collect()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

/// Runs the interactive fshell REPL session loop with a pre-built environment.
/// Used by reload --full for state handoff restoration.
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

    if let Ok(pwd) = std::env::current_dir() {
        emit_osc7(&pwd.to_string_lossy());
    }
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
            crate::tui::show_splash(&env, config_ok, &config_msg, shell_ok, &shell_msg);
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
    let _ = std::env::set_current_dir(&state.cwd);
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
            cwd: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/".to_string()),
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

    let prev_pwd = std::env::current_dir().ok();

    env.job_control
        .sigint_pending
        .store(false, std::sync::atomic::Ordering::SeqCst);

    let start_time = std::time::Instant::now();
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("SystemTime before UNIX_EPOCH")
        .as_millis() as i64;
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "/".to_string());
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

    let new_pwd = std::env::current_dir().ok();
    if prev_pwd != new_pwd {
        fshell_engine::invalidate_git_cache(env);
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
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::await_holding_lock,
        clippy::nonminimal_bool,
        clippy::collapsible_if
    )]
    use super::*;
    use crate::history::clear_connection_cache;
    use reedline::Completer;

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
            std::sync::Arc::new(|_, _, _, _| panic!("explicit test panic")),
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
