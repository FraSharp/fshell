// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
#![allow(clippy::result_large_err)]
use crate::cmdnotfound::{lookup_cached, spawn_background_search};
use fshell_core::Val;
use fshell_core::diagnostic::{ErrorCode, StringError};
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload, resolve_cached_command_path};
use std::io::Write;
use std::os::unix::io::FromRawFd;
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::io::AsyncWriteExt;
#[cfg(test)]
use ustr::ustr;

mod cmdnotfound;
mod structured;

/// Coerces a stream of Val into raw byte stream for legacy standard input.
pub fn coerce_val_to_bytes(val: &Val) -> Vec<u8> {
    let mut buf = Vec::new();
    write_val_to_bytes(val, &mut buf);
    buf
}

fn write_val_to_bytes(val: &Val, buf: &mut Vec<u8>) {
    match val {
        Val::String(s) => buf.extend_from_slice(s.as_bytes()),
        Val::Int(i) => {
            let _ = writeln!(buf, "{}", i);
        }
        Val::Float(f) => {
            let _ = writeln!(buf, "{}", f);
        }
        Val::List(list) => {
            for item in list {
                write_val_to_bytes(item, buf);
                if buf.last() != Some(&b'\n') {
                    buf.push(b'\n');
                }
            }
        }
        Val::Map(_) => {
            buf.extend_from_slice(&serde_json::to_vec(val).unwrap_or_default());
        }
        other => {
            buf.extend_from_slice(&serde_json::to_vec(other).unwrap_or_default());
        }
    }
}

/// Coerces a Val into raw bytes optimized for the destination external command.
pub fn coerce_val_to_bytes_for_cmd(val: &Val, cmd: &str) -> Vec<u8> {
    if let Val::Blob(bytes) = val {
        return bytes.clone();
    }
    match cmd {
        "jq" | "curl" | "http" => serde_json::to_vec(val).unwrap_or_default(),
        _ => {
            let s = val.to_text();
            let mut bytes = s.into_bytes();
            if !bytes.is_empty() && bytes.last() != Some(&b'\n') {
                bytes.push(b'\n');
            }
            bytes
        }
    }
}

/// Convert a Val to a string for use as an external command argument or env var.
/// Primitive types (String, Int, Float, Bool, Null) produce their natural string form.
/// Complex types (Map, List, Blob, etc.) use val.to_text() or a type-tagged fallback.
fn val_to_cmd_string(v: &Val) -> String {
    match v {
        Val::String(s) => s.clone(),
        Val::Int(i) => i.to_string(),
        Val::Float(f) => f.to_string(),
        Val::Bool(b) => b.to_string(),
        Val::Null => "null".to_string(),
        Val::Map(_) | Val::List(_) => v.to_text(),
        Val::DateTime(dt) => dt.to_rfc3339(),
        Val::Blob(bytes) => format!("<Blob: {} bytes>", bytes.len()),
        other => format!("<{}>", other.type_name()),
    }
}

struct InteractiveTerminalGuard {
    is_interactive: bool,
    raw_mode_was_enabled: bool,
}

impl InteractiveTerminalGuard {
    fn new(is_interactive: bool) -> Self {
        let is_interactive = is_interactive && !fshell_engine::is_test_mode();
        let mut raw_mode_was_enabled = false;
        let debug_fg = std::env::var("FSH_DEBUG_FG").is_ok();
        if is_interactive {
            raw_mode_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
            if debug_fg {
                eprintln!(
                    "[FSH_DEBUG_FG] guard: raw_mode_was_enabled={}",
                    raw_mode_was_enabled
                );
            }
            if raw_mode_was_enabled {
                if debug_fg {
                    eprintln!("[FSH_DEBUG_FG] guard: disabling raw mode");
                }
                let _ = crossterm::terminal::disable_raw_mode();
            }
            if debug_fg {
                eprintln!("[FSH_DEBUG_FG] guard: suspending session logging");
            }
            fshell_engine::suspend_session_logging();
        }
        Self {
            is_interactive,
            raw_mode_was_enabled,
        }
    }
}

impl Drop for InteractiveTerminalGuard {
    fn drop(&mut self) {
        if self.is_interactive {
            let debug_fg = std::env::var("FSH_DEBUG_FG").is_ok();
            #[cfg(unix)]
            unsafe {
                if debug_fg {
                    eprintln!(
                        "[FSH_DEBUG_FG] guard: restoring terminal to shell pgid={}",
                        libc::getpgrp()
                    );
                }
                libc::signal(libc::SIGTTOU, libc::SIG_IGN);
                let res = libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpgrp());
                libc::signal(libc::SIGTTOU, libc::SIG_DFL);
                if debug_fg {
                    eprintln!("[FSH_DEBUG_FG] guard: tcsetpgrp returned res={}", res);
                }
            }
            if self.raw_mode_was_enabled {
                if debug_fg {
                    eprintln!("[FSH_DEBUG_FG] guard: re-enabling raw mode");
                }
                let _ = crossterm::terminal::enable_raw_mode();
            }
            if debug_fg {
                eprintln!("[FSH_DEBUG_FG] guard: resuming session logging");
            }
            fshell_engine::resume_session_logging();
        }
    }
}

/// Checks if a command matches catastrophic destructive patterns (e.g. `rm -rf /` or raw disk writes).
fn check_destructive_command(name: &str, args: &[Val], env: &Env) -> Result<(), StringError> {
    let confirm_destructive = env.options.read().confirm_destructive;

    if !confirm_destructive {
        return Ok(());
    }

    let base_name = std::path::Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(name);

    let arg_strs: Vec<String> = args.iter().map(val_to_cmd_string).collect();

    let mut is_destructive = false;
    let mut warning_detail = String::new();

    // 1. Recursive RM targeting root, system dirs, or home root
    if base_name == "rm" {
        let has_recursive = arg_strs.iter().any(|a| {
            a == "-r"
                || a == "-R"
                || a == "--recursive"
                || (a.starts_with('-') && a.contains('r'))
                || (a.starts_with('-') && a.contains('R'))
        });

        if has_recursive {
            let home_dir = std::env::var("HOME").ok().unwrap_or_default();
            for arg in &arg_strs {
                if arg.starts_with('-') {
                    continue;
                }
                let clean = arg.trim_end_matches('/');
                if clean.is_empty()
                    || clean == "/"
                    || clean == "/*"
                    || clean == "/etc"
                    || clean == "/usr"
                    || clean == "/bin"
                    || clean == "/sbin"
                    || clean == "/System"
                    || clean == "/private"
                    || clean == "/var"
                    || clean == "/home"
                    || clean == "/root"
                    || clean == "/Users"
                    || clean == "~"
                    || (!home_dir.is_empty() && clean == home_dir)
                {
                    is_destructive = true;
                    warning_detail = format!("recursive deletion of root/system path '{arg}'");
                    break;
                }
            }
        }
    }

    // 2. Direct dd targeting raw block devices
    if base_name == "dd" {
        for arg in &arg_strs {
            if let Some(target) = arg.strip_prefix("of=")
                && (target.starts_with("/dev/sd")
                    || target.starts_with("/dev/hd")
                    || target.starts_with("/dev/nvme")
                    || target.starts_with("/dev/disk")
                    || target.starts_with("/dev/rdisk"))
            {
                is_destructive = true;
                warning_detail = format!("raw block device write to '{target}'");
                break;
            }
        }
    }

    // 3. Raw disk partitioning / formatting commands
    if matches!(
        base_name,
        "mkfs" | "fdisk" | "parted" | "gdisk" | "cfdisk" | "sfdisk"
    ) || base_name.starts_with("mkfs.")
    {
        is_destructive = true;
        warning_detail = format!("disk formatting or partition manipulation via '{base_name}'");
    }

    // 4. Catastrophic root permission wipe: chmod/chown -R 000 / or 777 /
    if (base_name == "chmod" || base_name == "chown")
        && arg_strs.iter().any(|a| a == "-R" || a == "-r")
    {
        for arg in &arg_strs {
            if arg == "/" || arg == "/*" || arg == "/etc" || arg == "/System" {
                is_destructive = true;
                warning_detail = format!("recursive permission change on '{arg}'");
                break;
            }
        }
    }

    if !is_destructive {
        return Ok(());
    }

    let is_interactive =
        !fshell_engine::is_test_mode() && unsafe { libc::isatty(libc::STDIN_FILENO) == 1 };
    if is_interactive {
        use std::io::Write;
        eprintln!(
            "\n\x1b[1;31m[!] DANGEROUS OPERATION DETECTED:\x1b[0m {name} {}",
            arg_strs.join(" ")
        );
        eprintln!("    Warning: {warning_detail}");
        eprint!("    Type '\x1b[1;32myes\x1b[0m' to proceed, or press Enter to cancel: ");
        let _ = std::io::stderr().flush();

        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_ok() && input.trim() == "yes" {
            Ok(())
        } else {
            Err(StringError::from(format!(
                "Cancelled dangerous operation: {name} {}",
                arg_strs.join(" ")
            )))
        }
    } else {
        Err(StringError::from(format!(
            "Dangerous operation '{name} {}' ({warning_detail}) blocked by default safety guard. Run with 'unsafe <cmd>' or unsetopt confirm_destructive to bypass.",
            arg_strs.join(" ")
        )))
    }
}

/// Spawns external shell commands executing legacy binaries securely.
pub fn run_external(
    name: &str,
    mut args: Vec<Val>,
    mut in_rx: Option<PipeStream>,
    env: &Env,
    tx: PipeSender,
    has_next: bool,
) -> Result<(), StringError> {
    let cnf_debug = std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1");
    let ext_start = std::time::Instant::now();
    if cnf_debug {
        eprintln!(
            "[cnf_debug] {}:{}: run_external name={:?} entering",
            file!(),
            line!(),
            name
        );
    }

    // 0. Pre-flight footgun protection for catastrophic commands
    check_destructive_command(name, &args, env)?;

    // 1. Verify Tier 2 process spawn capability
    env.enforce_capability(name, fshell_engine::CapAction::ProcessSpawn)?;

    fshell_core::debug_log!("run_external: name={:?} args={:?}", name, args);

    // 2. Resolve command path using PATH cache (avoids per-execution stat syscalls)
    //    This must happen before sandbox check so sandboxed scripts can be found via custom PATH
    let resolved_name = if let Some(custom_bin) = {
        let opts = Some(env.options.read());
        opts.and_then(|o| o.command_binaries.get(name).cloned())
    } {
        custom_bin
    } else if !name.contains('/') {
        let env_path = Some(env.vars.read()).and_then(|vars| {
            // PATH is stored as a top-level variable by populate_env_from_host
            vars.get("PATH").and_then(|pv| {
                if let Val::String(s) = pv {
                    if s.is_empty() { None } else { Some(s.clone()) }
                } else {
                    None
                }
            })
        });
        let path_resolve_start = std::time::Instant::now();
        let resolved = resolve_cached_command_path(name, env_path.as_deref())
            .unwrap_or_else(|| name.to_string());
        if cnf_debug {
            eprintln!(
                "[cnf_debug] {}:{}: resolve_cached_command_path took={:?}, resolved={:?}",
                file!(),
                line!(),
                path_resolve_start.elapsed(),
                resolved
            );
        }
        // If resolved to a full path but the file doesn't exist, invalidate cache and retry
        if resolved.contains('/') && !std::path::Path::new(&resolved).exists() {
            if cnf_debug {
                eprintln!(
                    "[cnf_debug] {}:{}: cached path {:?} stale, invalidating cache",
                    file!(),
                    line!(),
                    resolved
                );
            }
            // Invalidate stale cache
            fshell_engine::invalidate_path_cache();
            // Retry with fresh cache
            resolve_cached_command_path(name, env_path.as_deref())
                .unwrap_or_else(|| name.to_string())
        } else {
            resolved
        }
    } else {
        name.to_string()
    };

    fshell_core::debug_log!("run_external resolved: resolved_name={:?}", resolved_name);

    // 3. Auto-sandbox if sandbox_all is active, or for .sh scripts and bash/zsh shell worker invocations
    let sandbox_all = env.options.read().sandbox_all;
    let is_shell_worker = name == "bash"
        || name == "zsh"
        || resolved_name.ends_with("/bash")
        || resolved_name.ends_with("/zsh");
    let should_sandbox = sandbox_all || resolved_name.ends_with(".sh") || is_shell_worker;

    if should_sandbox {
        let mode = match env.options.read().sandbox_mode.as_str() {
            "deny-all" | "read-only-system" => Some(fshell_sandbox::SandboxMode::ReadOnlySystem),
            "isolated" => Some(fshell_sandbox::SandboxMode::Isolated),
            "prompt" => Some(fshell_sandbox::SandboxMode::Prompt),
            "monitor" => Some(fshell_sandbox::SandboxMode::Monitor),
            _ => None,
        };
        if let Some(mode) = mode {
            let config = fshell_sandbox::SandboxConfig::new(mode);
            return fshell_sandbox::run_sandboxed(&resolved_name, &args, in_rx, env, tx, &config)
                .map_err(StringError::from);
        }
    }

    // 1b. Verify network capability if this is a network command or an argument contains a host
    let is_net_cmd = matches!(
        name,
        "curl"
            | "wget"
            | "ping"
            | "ssh"
            | "dig"
            | "host"
            | "nslookup"
            | "nc"
            | "telnet"
            | "ftp"
            | "sftp"
            | "scp"
            | "rsync"
    );
    // Git: only require network cap when args include remote ops.
    let is_git_remote = name == "git"
        && args.iter().any(|a| {
            if let Val::String(s) = a {
                matches!(s.as_str(), "clone" | "fetch" | "pull" | "push" | "remote")
            } else {
                false
            }
        });
    let mut checked_net = false;
    for arg in &args {
        if let Val::String(s) = arg
            && let Some(host) = extract_host_from_arg(s)
        {
            env.enforce_capability(name, fshell_engine::CapAction::Network(host))?;
            checked_net = true;
        }
    }
    if (is_net_cmd || is_git_remote) && !checked_net {
        env.enforce_capability(name, fshell_engine::CapAction::Network("any".to_string()))?;
    }

    // 2a. Build command
    let mut cmd = std::process::Command::new(&resolved_name);

    // 2a. Detect interactive mode: command running in an interactive shell session with a TTY terminal.
    //     When true, we give the child terminal ownership via tcsetpgrp so that TUI programs
    //     and interactive prompts (like reading /dev/tty) function without SIGTTIN background suspension.
    // SAFETY: isatty only queries fd state, no side effects.
    let is_piped = in_rx.is_some() || has_next;
    let is_interactive = !fshell_engine::is_test_mode()
        && unsafe {
            !env.is_captured
                && !is_piped
                && libc::isatty(libc::STDIN_FILENO) == 1
                && fshell_engine::is_stdout_a_tty()
        };

    // Set implicitly derived $PWD sandbox current_dir (Tier 2 isolation)
    if let Ok(pwd) = std::env::current_dir() {
        cmd.current_dir(pwd);
    }

    // Inherit environment variables from fshell state "env" variable.
    // Skip entirely when no env modifications have been made — the child
    // inherits the correct environment from the parent via fork().
    if env.is_env_modified.load(Ordering::Acquire) {
        env.ensure_env_populated();
        if let Some(Val::Map(env_map)) = env.vars.read().get("env") {
            cmd.env_clear();
            // Batch-grant ReadEnv("*") once to avoid O(n) per-var capability checks.
            // In non-strict mode every var auto-grants anyway — a single wildcard
            // grant covers all env vars with one lock cycle instead of ~80.
            let skip_checks = {
                let caps = env.caps.caps.read();
                if caps.check_env_read("*") {
                    true
                } else {
                    drop(caps);
                    let _ = env.enforce_capability(
                        name,
                        fshell_engine::CapAction::ReadEnv("*".to_string()),
                    );
                    // Re-check: prompt may have granted the wildcard
                    env.caps.caps.read().check_env_read("*")
                }
            };

            for (k, v) in env_map {
                if !skip_checks {
                    // Strict mode: verify each variable individually
                    env.enforce_capability(
                        name,
                        fshell_engine::CapAction::ReadEnv(k.as_str().to_string()),
                    )?;
                }

                cmd.env(k.as_str(), val_to_cmd_string(v));
            }
        }
    }

    // Pre-execution flag injection for structured output (piped/captured non-interactive mode only)
    if !is_interactive && let Some(flag) = structured::maybe_inject_flag(name, &args) {
        args.insert(1, Val::String(flag.to_string()));
    }

    // Pass string formatted arguments (glob/brace expansion already done in pipeline.rs)
    let arg_strs: Vec<String> = args.iter().map(val_to_cmd_string).collect();
    for arg_str in &arg_strs {
        cmd.arg(arg_str);
    }

    // Setup stdio: interactive commands inherit the terminal for TUI support;
    // piped commands use pipes so the shell can capture their output.
    if is_interactive {
        if in_rx.is_some() {
            cmd.stdin(Stdio::piped());
        } else {
            cmd.stdin(Stdio::inherit());
        }
        if has_next {
            cmd.stdout(Stdio::piped());
        } else if let Some(fd) = fshell_engine::orig_stdout_fd() {
            let duped = unsafe { libc::dup(fd) };
            if duped >= 0 {
                cmd.stdout(unsafe { Stdio::from_raw_fd(duped) });
            } else {
                cmd.stdout(Stdio::inherit());
            }
        } else {
            cmd.stdout(Stdio::inherit());
        }
        if let Some(fd) = fshell_engine::orig_stderr_fd() {
            let duped = unsafe { libc::dup(fd) };
            if duped >= 0 {
                cmd.stderr(unsafe { Stdio::from_raw_fd(duped) });
            } else {
                cmd.stderr(Stdio::inherit());
            }
        } else {
            cmd.stderr(Stdio::inherit());
        }
    } else {
        if in_rx.is_some() {
            cmd.stdin(Stdio::piped());
        }
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
    }

    // Configure standard process group for UNIX job control
    cmd.process_group(0);

    let debug_fg = std::env::var("FSH_DEBUG_FG").is_ok();
    if debug_fg {
        let isatty_in = unsafe { libc::isatty(libc::STDIN_FILENO) };
        let isatty_out = unsafe { libc::isatty(libc::STDOUT_FILENO) };
        eprintln!(
            "[FSH_DEBUG_FG] run_external: name={}, is_interactive={}, env.is_captured={}, in_rx.is_none={}, has_next={}, isatty_in={}, isatty_out={}",
            name,
            is_interactive,
            env.is_captured,
            in_rx.is_none(),
            has_next,
            isatty_in,
            isatty_out
        );
    }

    // Reset signal mask in child before exec.
    // Tokio's signal handling (kqueue on macOS, signalfd on Linux) blocks
    // SIGINT/SIGTSTP via pthread_sigmask so the runtime can catch them. Child
    // processes inherit this blocked mask — SIGINT can never be delivered to
    // `rm -i` or any other interactive command, making Ctrl+C a no-op and
    // causing the shell to hang forever in waitpid. We unblock everything in
    // the child before exec so terminal signals work normally.
    //
    // SAFETY: runs after fork() in the single-threaded child before exec().
    // sigprocmask + signal are async-signal-safe per POSIX. No allocation.
    unsafe {
        cmd.pre_exec(move || {
            let mut set = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut set);
            libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
            // Reset SIGCHLD to SIG_DFL — Rust's stdlib sets it to SIG_IGN
            // which breaks waitpid() (returns ECHILD).
            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            // Reset SIGPIPE to SIG_DFL (Rust defaults to SIG_IGN).
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);

            if debug_fg {
                let msg = "[FSH_DEBUG_FG] child: pre_exec start\n";
                let _ = libc::write(
                    libc::STDERR_FILENO,
                    msg.as_ptr() as *const libc::c_void,
                    msg.len(),
                );
            }

            if is_interactive {
                if debug_fg {
                    let msg = "[FSH_DEBUG_FG] child: calling tcsetpgrp\n";
                    let _ = libc::write(
                        libc::STDERR_FILENO,
                        msg.as_ptr() as *const libc::c_void,
                        msg.len(),
                    );
                }
                libc::signal(libc::SIGTTOU, libc::SIG_IGN);
                let res = libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpid());
                libc::signal(libc::SIGTTOU, libc::SIG_DFL);
                if debug_fg {
                    if res == 0 {
                        let msg = "[FSH_DEBUG_FG] child: tcsetpgrp succeeded\n";
                        let _ = libc::write(
                            libc::STDERR_FILENO,
                            msg.as_ptr() as *const libc::c_void,
                            msg.len(),
                        );
                    } else {
                        let msg = "[FSH_DEBUG_FG] child: tcsetpgrp failed\n";
                        let _ = libc::write(
                            libc::STDERR_FILENO,
                            msg.as_ptr() as *const libc::c_void,
                            msg.len(),
                        );
                    }
                }
            }

            Ok(())
        });
    }

    if debug_fg {
        eprintln!("[FSH_DEBUG_FG] parent: about to spawn child");
    }

    // Create the terminal guard to handle raw mode disabling and restoration
    let _guard = InteractiveTerminalGuard::new(is_interactive);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            if cnf_debug {
                eprintln!(
                    "[cnf_debug] {}:{}: spawn failed at {:?}, elapsed={:?}, error={}",
                    file!(),
                    line!(),
                    ext_start,
                    ext_start.elapsed(),
                    e
                );
            }
            if e.raw_os_error() == Some(libc::ENOENT) {
                // Size-unit hint: `ls | filter size > 100 KB` (with space) is parsed as
                // filter condition `size > 100` followed by a pipeline stage `KB`.
                // The size literal must be written without a space: `100KB`.
                if is_size_unit(name) {
                    return Err(StringError::new(
                        ErrorCode::CommandNotFound,
                        format!("Command not found: {name}"),
                    )
                    .with_help(
                        "Hint: size literals must not have a space — use 100KB not 100 KB. Valid units: B, KB (1000), MB, GB, TB, PB, EB and KiB (1024), MiB, GiB, TiB, PiB, EiB, or shorthand K/M/G/T/P/E (1024-based). Example: ls | filter size > 10KB",
                    )
                    .with_fix(format!("remove the space: 100{name}"))
                    .with_suggestion(format!("remove the space: 100{name}")));
                }
                let cache_start = std::time::Instant::now();
                if let Some(suggestion) = lookup_cached(name) {
                    if cnf_debug {
                        eprintln!(
                            "[cnf_debug] {}:{}: lookup_cached hit suggestion={:?}, took={:?}",
                            file!(),
                            line!(),
                            suggestion,
                            cache_start.elapsed()
                        );
                    }
                    return Err(StringError::new(
                        ErrorCode::CommandNotFound,
                        format!("Command not found: {name}"),
                    )
                    .with_suggestion(suggestion)
                    .with_help("Check the command spelling or install the package."));
                }
                if cnf_debug {
                    eprintln!(
                        "[cnf_debug] {}:{}: lookup_cached miss, took={:?}, spawning background search",
                        file!(),
                        line!(),
                        cache_start.elapsed()
                    );
                }
                spawn_background_search(name);
                let total_elapsed = ext_start.elapsed();
                if cnf_debug {
                    eprintln!(
                        "[cnf_debug] {}:{}: run_external returning ENOENT after {:?}",
                        file!(),
                        line!(),
                        total_elapsed
                    );
                }
                return Err(StringError::new(
                    ErrorCode::CommandNotFound,
                    format!("Command not found: {name}"),
                )
                .with_help(
                    "Check the command name, PATH variable, or type `help` for available builtins.",
                ));
            }
            return Err(StringError::new(
                ErrorCode::CommandFailed,
                format!("Failed to spawn {name}: {e}"),
            ));
        }
    };

    let pid = child.id() as i32;
    if debug_fg {
        eprintln!("[FSH_DEBUG_FG] parent: child spawned pid={}", pid);
    }

    // In interactive mode, give the child's process group ownership of the terminal.
    // SIGTTOU is ignored during this call so the shell isn't stopped when it loses
    // foreground status — the waiter task will restore SIG_DFL when the child exits.
    if is_interactive {
        if debug_fg {
            eprintln!("[FSH_DEBUG_FG] parent: calling tcsetpgrp for pid={}", pid);
        }
        // SAFETY: signal and tcsetpgrp are async-signal-safe. SIGTTOU is ignored to
        // prevent the shell from being stopped when it loses foreground status.
        unsafe {
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            let res = libc::tcsetpgrp(libc::STDIN_FILENO, pid);
            libc::signal(libc::SIGTTOU, libc::SIG_DFL);
            if debug_fg {
                eprintln!("[FSH_DEBUG_FG] parent: tcsetpgrp returned res={}", res);
            }
        }
    }

    // Register job in active list - collect once, reuse
    let cmd_str = format!("{} {}", name, arg_strs.join(" "));

    let job_id = {
        let mut jobs = env.job_control.jobs.write();
        let next_id = jobs.values().map(|j| j.id).max().unwrap_or(0) + 1;
        jobs.insert(
            pid,
            fshell_engine::Job {
                id: next_id,
                pgid: pid,
                pids: vec![pid],
                cmd: cmd_str.clone(),
                status: fshell_engine::JobStatus::Running,
                disowned: false,
                started_at: Some(std::time::Instant::now()),
            },
        );
        if is_interactive {
            let _ = env.set_foreground_job(Some(next_id));
        }
        next_id
    };

    // Pipe upstream inputs into process stdin, capture stdout/stderr (piped mode only)
    if let Some(mut rx) = in_rx.take()
        && let Some(child_stdin) = child.stdin.take()
    {
        // SAFETY: from_raw_fd takes ownership of the fd. into_raw_fd consumes
        // the Stdio wrapper, transferring ownership. The fd is valid because
        // child.stdin was just taken from a spawned process.
        let std_stdin = unsafe {
            use std::os::unix::io::{FromRawFd, IntoRawFd};
            std::fs::File::from_raw_fd(child_stdin.into_raw_fd())
        };
        let mut async_stdin = tokio::fs::File::from_std(std_stdin);
        let cmd_name = name.to_string();
        tokio::spawn(async move {
            while let Some(payload) = rx.recv().await {
                if let PipelinePayload::Data(val_arc) = payload {
                    let bytes = coerce_val_to_bytes_for_cmd(&val_arc, &cmd_name);
                    if async_stdin.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
            }
        });
    }

    // Capture process stdout & stream line-by-line (when stdout was piped)
    if let Some(child_stdout) = child.stdout.take() {
        // SAFETY: from_raw_fd takes ownership of the fd. into_raw_fd consumes
        // the Stdio wrapper, transferring ownership. The fd is valid because
        // child.stdout was just taken from a spawned process.
        let std_stdout = unsafe {
            use std::os::unix::io::{FromRawFd, IntoRawFd};
            std::fs::File::from_raw_fd(child_stdout.into_raw_fd())
        };
        let mut async_stdout = tokio::fs::File::from_std(std_stdout);
        let tx_clone = tx.clone();
        let name_owned = name.to_string();
        let args_strs = arg_strs.clone();
        let json_auto_parse = env.options.read().json_auto_parse;
        let env_clone = env.clone();
        tokio::spawn(async move {
            use structured::{ParseResult, ParseState};
            use tokio::io::AsyncBufReadExt;
            let mut reader = tokio::io::BufReader::new(&mut async_stdout);
            let mut buf = Vec::new();
            let mut parse_state = ParseState::default();
            while let Ok(n) = reader.read_until(b'\n', &mut buf).await {
                if n == 0 {
                    break;
                }
                // Try decoding as UTF-8 line first
                let val = match std::str::from_utf8(&buf) {
                    Ok(s) => {
                        let line = s.trim_end_matches(&['\r', '\n'][..]);
                        let trimmed = line.trim();
                        // Try structured parsers first. Blank lines are preserved as
                        // empty string values (a user piping `printf "a\n\nb\n" | count`
                        // expects 3 lines, not 1) rather than being silently dropped.
                        match structured::parse_line(
                            &name_owned,
                            &args_strs,
                            trimmed,
                            &mut parse_state,
                        ) {
                            ParseResult::Data(val) => val,
                            ParseResult::Header => {
                                buf.clear();
                                continue;
                            }
                            ParseResult::Fallthrough => {
                                // Fall back to existing JSON detection
                                if json_auto_parse
                                    && ((trimmed.starts_with('{') && trimmed.ends_with('}'))
                                        || (trimmed.starts_with('[') && trimmed.ends_with(']')))
                                {
                                    if let Ok(json_val) =
                                        serde_json::from_str::<serde_json::Value>(trimmed)
                                    {
                                        parse_json_value(json_val)
                                    } else {
                                        Val::String(line.to_string())
                                    }
                                } else {
                                    Val::String(line.to_string())
                                }
                            }
                        }
                    }
                    Err(_) => {
                        // Binary data / non-UTF8 payload: preserve exact bytes as Val::Blob
                        Val::Blob(buf.clone())
                    }
                };
                buf.clear();
                if fshell_engine::send_with_backpressure(
                    &env_clone,
                    &tx_clone,
                    PipelinePayload::Data(Arc::new(val)),
                )
                .await
                .is_err()
                {
                    // Channel closed (downstream consumer exited, e.g. `head` finished).
                    break;
                }
            }
        });
    }

    // Capture stderr errors (when stderr was piped)
    if let Some(child_stderr) = child.stderr.take() {
        // SAFETY: from_raw_fd takes ownership of the fd. into_raw_fd consumes
        // the Stdio wrapper, transferring ownership. The fd is valid because
        // child.stderr was just taken from a spawned process.
        let std_stderr = unsafe {
            use std::os::unix::io::{FromRawFd, IntoRawFd};
            std::fs::File::from_raw_fd(child_stderr.into_raw_fd())
        };
        let async_stderr = tokio::fs::File::from_std(std_stderr);
        let tx_diag = tx.clone();
        let stderr_limit = env.options.read().stderr_max_bytes;
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut err_str = String::with_capacity(4096);
            let mut limited = async_stderr.take(stderr_limit as u64);
            let _ = limited.read_to_string(&mut err_str).await;
            if limited.limit() == 0 && !err_str.is_empty() {
                let total = err_str.len();
                if !err_str.ends_with('\n') {
                    err_str.push('\n');
                }
                err_str.push_str(&format!(
                    "...(stderr truncated at {} of {} bytes)\n",
                    total,
                    total + stderr_limit
                ));
            }
            if !err_str.is_empty() {
                let _ = tx_diag
                    .send(PipelinePayload::Structured(err_str.into()))
                    .await;
            }
        });
    }

    // Synchronously wait for child exit to capture exit code deterministically.
    // This replaces the previous fire-and-forget waiter + Condvar pattern which
    // raced with the statement finalizer that read last_exit_code before the
    // waiter landed. We are already inside a spawn_blocking context (called
    // from pipeline's spawn_blocking), so blocking waitpid here is correct and
    // does not stall the async executor. I/O forwarding tasks remain async
    // and continue to drain pipes concurrently.
    let _exit_code = fshell_engine::wait_for_job_sync(env, pid, job_id, &cmd_str, is_interactive);

    if cnf_debug {
        eprintln!(
            "[cnf_debug] {}:{}: run_external success after {:?}",
            file!(),
            line!(),
            ext_start.elapsed()
        );
    }
    Ok(())
}

fn parse_json_value(json: serde_json::Value) -> Val {
    match json {
        serde_json::Value::Null => Val::Null,
        serde_json::Value::Bool(b) => Val::Bool(b),
        serde_json::Value::Number(num) => {
            if let Some(i) = num.as_i64() {
                Val::Int(i)
            } else if let Some(f) = num.as_f64() {
                Val::Float(f)
            } else {
                Val::Null
            }
        }
        serde_json::Value::String(s) => Val::String(s),
        serde_json::Value::Array(arr) => Val::List(arr.into_iter().map(parse_json_value).collect()),
        serde_json::Value::Object(obj) => {
            let mut map = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            for (k, v) in obj {
                map.insert(ustr::ustr(&k), parse_json_value(v));
            }
            Val::Map(map)
        }
    }
}

pub fn init(env: &Env) {
    env.set_fallback_handler(Arc::new(|name, args, in_rx, env, tx, has_next| {
        run_external(name, args, in_rx, env, tx, has_next)
    }));
}

fn is_size_unit(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "b" | "kb"
            | "mb"
            | "gb"
            | "tb"
            | "pb"
            | "eb"
            | "kib"
            | "mib"
            | "gib"
            | "tib"
            | "pib"
            | "eib"
            | "k"
            | "m"
            | "g"
            | "t"
            | "p"
            | "e"
    )
}

fn extract_host_from_arg(arg: &str) -> Option<String> {
    if let Some(pos) = arg.find("://") {
        let rest = &arg[pos + 3..];
        let end = rest.find(['/', ':', '?']).unwrap_or(rest.len());
        let host = &rest[..end];
        return Some(host.to_string());
    }
    if let Some(pos) = arg.find('@')
        && let Some(colon_pos) = arg[pos..].find(':')
    {
        let host = &arg[pos + 1..pos + colon_pos];
        if !host.is_empty() && host.contains('.') {
            return Some(host.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::approx_constant)]
    use super::*;
    use fshell_core::FxIndexMap;
    use fshell_hash::FxBuildHasher;
    // coerce_val_to_bytes: per-variant tests
    #[test]
    fn test_coerce_null() {
        let bytes = coerce_val_to_bytes(&Val::Null);
        assert_eq!(bytes, serde_json::to_vec(&Val::Null).unwrap());
    }

    #[test]
    fn test_coerce_bool_true() {
        let bytes = coerce_val_to_bytes(&Val::Bool(true));
        assert_eq!(bytes, serde_json::to_vec(&Val::Bool(true)).unwrap());
    }

    #[test]
    fn test_coerce_bool_false() {
        let bytes = coerce_val_to_bytes(&Val::Bool(false));
        assert_eq!(bytes, serde_json::to_vec(&Val::Bool(false)).unwrap());
    }

    #[test]
    fn test_coerce_int_zero() {
        assert_eq!(coerce_val_to_bytes(&Val::Int(0)), b"0\n");
    }

    #[test]
    fn test_coerce_int_positive() {
        assert_eq!(coerce_val_to_bytes(&Val::Int(42)), b"42\n");
    }

    #[test]
    fn test_coerce_int_negative() {
        assert_eq!(coerce_val_to_bytes(&Val::Int(-7)), b"-7\n");
    }

    #[test]
    fn test_coerce_int_large() {
        assert_eq!(
            coerce_val_to_bytes(&Val::Int(i64::MAX)),
            format!("{}\n", i64::MAX).as_bytes()
        );
    }

    #[test]
    fn test_coerce_float_zero() {
        assert_eq!(coerce_val_to_bytes(&Val::Float(0.0)), b"0\n");
    }

    #[test]
    fn test_coerce_float_pi() {
        assert_eq!(coerce_val_to_bytes(&Val::Float(3.14)), b"3.14\n");
    }

    #[test]
    fn test_coerce_float_negative() {
        assert_eq!(coerce_val_to_bytes(&Val::Float(-2.5)), b"-2.5\n");
    }

    #[test]
    fn test_coerce_string_hello() {
        assert_eq!(coerce_val_to_bytes(&Val::String("hello".into())), b"hello");
    }

    #[test]
    fn test_coerce_string_empty() {
        assert_eq!(coerce_val_to_bytes(&Val::String(String::new())), b"");
    }

    #[test]
    fn test_coerce_list_empty() {
        assert_eq!(coerce_val_to_bytes(&Val::List(vec![])), b"");
    }

    #[test]
    fn test_coerce_flat_list() {
        let list = Val::List(vec![Val::Int(10), Val::String("test".to_string())]);
        let bytes = coerce_val_to_bytes(&list);
        assert_eq!(bytes, b"10\ntest\n");
    }

    #[test]
    fn test_coerce_list_mixed_types() {
        let list = Val::List(vec![
            Val::Null,
            Val::Bool(true),
            Val::Int(5),
            Val::String("hi".into()),
        ]);
        let bytes = coerce_val_to_bytes(&list);
        // Null and Bool go through the "other" branch (serde_json), Int and String have dedicated arms
        let null_json = serde_json::to_vec(&Val::Null).unwrap();
        let bool_json = serde_json::to_vec(&Val::Bool(true)).unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(&null_json);
        expected.push(b'\n');
        expected.extend_from_slice(&bool_json);
        expected.push(b'\n');
        expected.extend_from_slice(b"5\nhi\n"); // Int is "5\n", String is "hi" + appended "\n"
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_coerce_list_nested() {
        let list = Val::List(vec![Val::List(vec![Val::Int(1), Val::Int(2)])]);
        let bytes = coerce_val_to_bytes(&list);
        assert_eq!(bytes, b"1\n2\n");
    }

    #[test]
    fn test_coerce_list_nested_deep() {
        let list = Val::List(vec![
            Val::Int(1),
            Val::List(vec![
                Val::String("a".into()),
                Val::List(vec![Val::Float(3.0)]),
            ]),
        ]);
        let bytes = coerce_val_to_bytes(&list);
        assert_eq!(bytes, b"1\na\n3\n");
    }

    #[test]
    fn test_coerce_map_empty() {
        let map = Val::Map(FxIndexMap::with_hasher(FxBuildHasher::default()));
        assert_eq!(coerce_val_to_bytes(&map), serde_json::to_vec(&map).unwrap());
    }

    #[test]
    fn test_coerce_map_single_entry() {
        let mut m = FxIndexMap::with_hasher(FxBuildHasher::default());
        m.insert(ustr("x"), Val::Int(10));
        let val = Val::Map(m);
        assert_eq!(coerce_val_to_bytes(&val), serde_json::to_vec(&val).unwrap());
    }

    #[test]
    fn test_coerce_map_multi_entry() {
        let mut m = FxIndexMap::with_hasher(FxBuildHasher::default());
        m.insert(ustr("a"), Val::Int(1));
        m.insert(ustr("b"), Val::String("hello".into()));
        let val = Val::Map(m);
        assert_eq!(coerce_val_to_bytes(&val), serde_json::to_vec(&val).unwrap());
    }

    #[test]
    fn test_coerce_map_various_values() {
        let mut m = FxIndexMap::with_hasher(FxBuildHasher::default());
        m.insert(ustr("null"), Val::Null);
        m.insert(ustr("flag"), Val::Bool(true));
        m.insert(ustr("pi"), Val::Float(3.14));
        let val = Val::Map(m);
        assert_eq!(coerce_val_to_bytes(&val), serde_json::to_vec(&val).unwrap());
    }

    #[test]
    fn test_coerce_blob() {
        let blob = Val::Blob(vec![65, 66, 67]);
        let bytes = coerce_val_to_bytes(&blob);
        assert_eq!(bytes, serde_json::to_vec(&blob).unwrap());
    }

    #[test]
    fn test_coerce_blob_empty() {
        let blob = Val::Blob(vec![]);
        let bytes = coerce_val_to_bytes(&blob);
        assert_eq!(bytes, serde_json::to_vec(&blob).unwrap());
    }

    #[test]
    fn test_coerce_datetime() {
        let dt: Val =
            serde_json::from_str(r#"{"type":"DateTime","value":"2024-06-15T10:30:00Z"}"#).unwrap();
        let bytes = coerce_val_to_bytes(&dt);
        assert_eq!(bytes, serde_json::to_vec(&dt).unwrap());
    }

    #[test]
    fn test_coerce_capability() {
        let cap: Val =
            serde_json::from_str(r#"{"type":"Capability","value":{"ReadDir":"/tmp"}}"#).unwrap();
        let bytes = coerce_val_to_bytes(&cap);
        assert_eq!(bytes, serde_json::to_vec(&cap).unwrap());
    }

    #[test]
    fn test_coerce_object_graph() {
        let og: Val = serde_json::from_str(
            r#"{"type":"ObjectGraph","value":{"root":0,"graph":{"nodes":{},"edges":{}}}}"#,
        )
        .unwrap();
        let bytes = coerce_val_to_bytes(&og);
        assert_eq!(bytes, serde_json::to_vec(&og).unwrap());
    }
    // parse_json_value (private helper)
    #[test]
    fn test_parse_json_null() {
        assert_eq!(super::parse_json_value(serde_json::Value::Null), Val::Null);
    }

    #[test]
    fn test_parse_json_bool() {
        assert_eq!(
            super::parse_json_value(serde_json::Value::Bool(true)),
            Val::Bool(true)
        );
        assert_eq!(
            super::parse_json_value(serde_json::Value::Bool(false)),
            Val::Bool(false)
        );
    }

    #[test]
    fn test_parse_json_int() {
        assert_eq!(
            super::parse_json_value(serde_json::Value::Number(42.into())),
            Val::Int(42)
        );
        assert_eq!(
            super::parse_json_value(serde_json::Value::Number((-7).into())),
            Val::Int(-7)
        );
        assert_eq!(
            super::parse_json_value(serde_json::Value::Number(0.into())),
            Val::Int(0)
        );
    }

    #[test]
    fn test_parse_json_float() {
        assert_eq!(
            super::parse_json_value(serde_json::Value::Number(
                serde_json::Number::from_f64(3.14).unwrap()
            )),
            Val::Float(3.14)
        );
    }

    #[test]
    fn test_parse_json_string() {
        assert_eq!(
            super::parse_json_value(serde_json::Value::String("hello".into())),
            Val::String("hello".into())
        );
        assert_eq!(
            super::parse_json_value(serde_json::Value::String(String::new())),
            Val::String(String::new())
        );
    }

    #[test]
    fn test_parse_json_array() {
        let arr = serde_json::Value::Array(vec![
            serde_json::Value::Number(1.into()),
            serde_json::Value::String("two".into()),
            serde_json::Value::Bool(true),
        ]);
        let expected = Val::List(vec![
            Val::Int(1),
            Val::String("two".into()),
            Val::Bool(true),
        ]);
        assert_eq!(super::parse_json_value(arr), expected);
    }

    #[test]
    fn test_parse_json_array_empty() {
        assert_eq!(
            super::parse_json_value(serde_json::Value::Array(vec![])),
            Val::List(vec![])
        );
    }

    #[test]
    fn test_parse_json_object() {
        let mut obj = serde_json::Map::new();
        obj.insert("name".into(), serde_json::Value::String("Alice".into()));
        obj.insert("age".into(), serde_json::Value::Number(30.into()));
        let result = super::parse_json_value(serde_json::Value::Object(obj));

        let mut expected_map = FxIndexMap::with_hasher(FxBuildHasher::default());
        expected_map.insert(ustr("name"), Val::String("Alice".into()));
        expected_map.insert(ustr("age"), Val::Int(30));
        assert_eq!(result, Val::Map(expected_map));
    }

    #[test]
    fn test_parse_json_object_empty() {
        assert_eq!(
            super::parse_json_value(serde_json::Value::Object(serde_json::Map::new())),
            Val::Map(FxIndexMap::with_hasher(FxBuildHasher::default()))
        );
    }

    #[test]
    fn test_parse_json_nested() {
        let mut inner = serde_json::Map::new();
        inner.insert("b".into(), serde_json::Value::Number(2.into()));
        let mut outer = serde_json::Map::new();
        outer.insert("inner".into(), serde_json::Value::Object(inner));
        outer.insert(
            "items".into(),
            serde_json::Value::Array(vec![
                serde_json::Value::Number(1.into()),
                serde_json::Value::String("x".into()),
            ]),
        );
        let result = super::parse_json_value(serde_json::Value::Object(outer));

        let mut inner_map = FxIndexMap::with_hasher(FxBuildHasher::default());
        inner_map.insert(ustr("b"), Val::Int(2));
        let mut outer_map = FxIndexMap::with_hasher(FxBuildHasher::default());
        outer_map.insert(ustr("inner"), Val::Map(inner_map));
        outer_map.insert(
            ustr("items"),
            Val::List(vec![Val::Int(1), Val::String("x".into())]),
        );
        assert_eq!(result, Val::Map(outer_map));
    }
    // Edge cases
    #[test]
    fn test_coerce_special_floats() {
        assert_eq!(coerce_val_to_bytes(&Val::Float(f64::NAN)), b"NaN\n");
        assert_eq!(coerce_val_to_bytes(&Val::Float(f64::INFINITY)), b"inf\n");
        assert_eq!(
            coerce_val_to_bytes(&Val::Float(f64::NEG_INFINITY)),
            b"-inf\n"
        );
    }

    #[test]
    fn test_coerce_long_string() {
        let long = "a".repeat(10_000);
        let bytes = coerce_val_to_bytes(&Val::String(long.clone()));
        assert_eq!(bytes.len(), 10_000);
        assert_eq!(bytes, long.as_bytes());
    }

    #[test]
    fn test_coerce_string_unicode() {
        let s = "héllo wörld!";
        assert_eq!(coerce_val_to_bytes(&Val::String(s.into())), s.as_bytes());
    }

    #[test]
    fn test_coerce_list_nested_maps() {
        let mut inner = FxIndexMap::with_hasher(FxBuildHasher::default());
        inner.insert(ustr("k"), Val::Int(99));
        let inner_map = Val::Map(inner);
        let list = Val::List(vec![inner_map.clone(), Val::String("end".into())]);
        let bytes = coerce_val_to_bytes(&list);
        let mut expected = serde_json::to_vec(&inner_map).unwrap();
        expected.push(b'\n');
        expected.extend_from_slice(b"end\n");
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_coerce_map_with_list_value() {
        let mut m = FxIndexMap::with_hasher(FxBuildHasher::default());
        m.insert(ustr("nums"), Val::List(vec![Val::Int(1), Val::Int(2)]));
        let val = Val::Map(m);
        assert_eq!(coerce_val_to_bytes(&val), serde_json::to_vec(&val).unwrap());
    }

    #[test]
    fn test_parse_json_large_number() {
        let large: i64 = 9_007_199_254_740_992; // 2^53, fits in i64
        let json_val = serde_json::Value::Number(serde_json::Number::from(large));
        assert_eq!(super::parse_json_value(json_val), Val::Int(large));
    }

    #[test]
    fn test_parse_json_negative_int() {
        let json_val = serde_json::Value::Number(serde_json::Number::from(-1i64));
        assert_eq!(super::parse_json_value(json_val), Val::Int(-1));
    }

    #[test]
    fn test_coerce_list_mixed_with_map() {
        let mut m = FxIndexMap::with_hasher(FxBuildHasher::default());
        m.insert(ustr("key"), Val::Bool(false));
        let map_val = Val::Map(m);
        let list = Val::List(vec![Val::Int(7), map_val.clone()]);
        let bytes = coerce_val_to_bytes(&list);
        let mut expected = b"7\n".to_vec();
        expected.extend_from_slice(&serde_json::to_vec(&map_val).unwrap());
        expected.push(b'\n');
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_coerce_list_large() {
        let items: Vec<Val> = (0..100).map(Val::Int).collect();
        let bytes = coerce_val_to_bytes(&Val::List(items));
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 100);
        assert_eq!(lines[0], "0");
        assert_eq!(lines[99], "99");
    }

    #[test]
    fn test_coerce_val_to_bytes_for_cmd_json() {
        let val = Val::Int(42);
        let bytes_jq = coerce_val_to_bytes_for_cmd(&val, "jq");
        assert_eq!(bytes_jq, serde_json::to_vec(&val).unwrap());

        let mut map_inner = FxIndexMap::with_hasher(FxBuildHasher::default());
        map_inner.insert(ustr("key"), Val::String("val".into()));
        let map_val = Val::Map(map_inner);
        let bytes_curl = coerce_val_to_bytes_for_cmd(&map_val, "curl");
        assert_eq!(bytes_curl, serde_json::to_vec(&map_val).unwrap());
    }

    #[test]
    fn test_coerce_val_to_bytes_for_cmd_text() {
        let val = Val::Int(42);
        let bytes_grep = coerce_val_to_bytes_for_cmd(&val, "grep");
        assert_eq!(bytes_grep, b"42\n");

        let list_val = Val::List(vec![Val::Int(1), Val::Int(2)]);
        let bytes_rg = coerce_val_to_bytes_for_cmd(&list_val, "rg");
        assert_eq!(bytes_rg, b"1\n2\n");

        let bytes_fallback =
            coerce_val_to_bytes_for_cmd(&Val::String("hello".into()), "unknown_cmd");
        assert_eq!(bytes_fallback, b"hello\n");
    }
}
