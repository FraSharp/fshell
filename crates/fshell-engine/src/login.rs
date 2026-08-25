// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(clippy::result_large_err)]
use crate::{EngineError, Env};
use fshell_core::Val;
use std::path::{Path, PathBuf};
// Login shell detection
/// Detect whether this invocation is a login shell.
///
/// Unix convention: login shells are invoked with argv[0][0] == '-'.
/// We also honour `fsh --login` / `fsh -l`.
pub fn detect_login(argv0: &str, cli_login: bool) -> bool {
    cli_login || argv0.starts_with('-')
}

/// Returns true when the session is interactive (connected to a TTY).
pub fn is_interactive() -> bool {
    if crate::is_test_mode() {
        return false;
    }
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && crate::is_stdout_a_tty()
}
// SHLVL management
/// Increment `$SHLVL` for the new shell instance.
///
/// Bash, zsh and fish all bump `SHLVL` on every new **interactive**
/// invocation.  Login shells included — every entry through `login(1)`
/// or `Terminal.app` is interactive, so the increment is unconditional
/// for interactive sessions and skipped for `fsh -c` / script mode.
///
/// The host `SHLVL` env var and the fshell `SHLVL` variable are kept
/// in sync so that both `echo $SHLVL` (fshell var) and child processes
/// (`env | grep SHLVL`) observe the same value.
pub fn bump_shlvl(env: &Env) {
    let host_current = std::env::var("SHLVL")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let next = host_current + 1;
    let next_str = next.to_string();
    unsafe {
        std::env::set_var("SHLVL", &next_str);
    }
    {
        let mut vars = env.vars.write();
        vars.insert("SHLVL".to_string(), Val::String(next_str));
    }
}

/// Ensure `$SHLVL` exists as a fshell variable so `echo $SHLVL` does not
/// error under `nounset`.  For non-interactive shells (`fsh -c`,
/// `fsh script.fsh`) we do not bump the host `SHLVL` — we just expose
/// the inherited value.  Interactive shells must use `bump_shlvl` instead.
fn ensure_shlvl(env: &Env) {
    if env.vars.read().contains_key("SHLVL") {
        return;
    }
    let host_val = std::env::var("SHLVL").unwrap_or_else(|_| "0".to_string());
    // Normalise non-numeric host values rather than leaking garbage.
    let normalised = host_val
        .parse::<i64>()
        .map(|n| n.to_string())
        .unwrap_or(host_val);
    {
        let mut vars = env.vars.write();
        vars.entry("SHLVL".to_string())
            .or_insert(Val::String(normalised));
    }
}
// Profile / RC file resolution — long-term Unix-correct precedence
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// System-wide profile files sourced by login shells.
///
/// `/etc/profile` is the POSIX standard; `/etc/zprofile` is the zsh
/// equivalent. Both are best-effort (`2>/dev/null` in the shim) so the
/// absence of either file is not an error.
fn system_login_profiles() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/etc/profile"),
        PathBuf::from("/etc/zprofile"),
    ]
}

/// User login profiles in POSIX / bash precedence order.
///
/// bash(1) tries `~/.bash_profile` → `~/.bash_login` → `~/.profile` and
/// stops at the first that exists.  zsh tries `~/.zprofile`.  We model
/// that precedence: prefer `.bash_profile` over `.bash_login` over
/// `.profile`, but also include `.zprofile` independently because many
/// macOS / zsh users keep their login PATH there even after switching
/// `$SHELL` to fsh.
pub fn user_login_profiles(home: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let bash_profile = home.join(".bash_profile");
    let bash_login = home.join(".bash_login");
    let profile = home.join(".profile");
    let zprofile = home.join(".zprofile");

    // bash precedence: first existing among these three is the "primary"
    // login file.  We still source the others if they exist because the
    // user's previous shell may have split configuration across them, but
    // the primary comes first.
    if bash_profile.is_file() {
        candidates.push(bash_profile);
        if zprofile.is_file() {
            candidates.push(zprofile);
        }
        if profile.is_file() {
            candidates.push(profile);
        }
    } else if bash_login.is_file() {
        candidates.push(bash_login);
        if zprofile.is_file() {
            candidates.push(zprofile);
        }
        if profile.is_file() {
            candidates.push(profile);
        }
    } else if profile.is_file() {
        candidates.push(profile);
        if zprofile.is_file() {
            candidates.push(zprofile);
        }
    } else if zprofile.is_file() {
        candidates.push(zprofile);
    }
    candidates
}

/// Interactive (non-login) RC files.
///
/// These are sourced for every interactive shell, login or not — a login
/// shell on macOS (Terminal.app) is also interactive, and the user's
/// aliases/functions live in `.zshrc`/`.bashrc`.  Non-interactive shells
/// (`fsh -c`, `fsh script.fsh`) do not source these.
fn interactive_rc_files(home: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let zshrc = home.join(".zshrc");
    let bashrc = home.join(".bashrc");
    // Preserve historical fshell behaviour: source both if they exist,
    // regardless of `$SHELL`, so a user who migrated from zsh to fsh
    // doesn't lose aliases.
    if zshrc.is_file() {
        v.push(zshrc);
    }
    if bashrc.is_file() {
        v.push(bashrc);
    }
    v
}

/// Resolve the ordered list of shell fragments to source for this
/// invocation.
///
/// * `is_login` — true for `fsh --login` or `argv[0] == "-fsh"`
/// * `is_interactive` — true when stdin/stdout are TTYs
///
/// Returns an ordered, deduplicated list. System profiles come first,
/// then user login profiles, then interactive RC files (when
/// `is_interactive`).  Every entry is guaranteed to exist and be a
/// regular file at call time.
pub fn resolve_source_files(is_login: bool, is_interactive: bool) -> Vec<PathBuf> {
    let home = match home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };
    let mut files = Vec::new();

    if is_login {
        for p in system_login_profiles() {
            if p.is_file() {
                files.push(p);
            }
        }
        files.extend(user_login_profiles(&home));
    }

    if is_interactive {
        for p in interactive_rc_files(&home) {
            if p.is_file() && !files.contains(&p) {
                files.push(p);
            }
        }
    }

    files
}
// POSIX detection & login cache management
/// Heuristic detection for POSIX/bash shell scripts.
pub fn looks_like_posix(content: &str) -> bool {
    let trimmed = content.trim_start();
    if let Some(first_line) = trimmed.lines().next()
        && let Some(rest) = first_line.strip_prefix("#!")
        && let Some(shell) = rest.split_whitespace().next()
    {
        let name = Path::new(shell)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if matches!(name.as_str(), "sh" | "bash" | "zsh" | "dash" | "ksh") {
            return true;
        }
    }
    trimmed.contains("function ")
        || trimmed.contains("() {")
        || trimmed.contains("()\n")
        || trimmed.contains(" then ")
        || trimmed.contains("\nelif ")
        || trimmed.contains("\nfi\n")
        || trimmed.contains(";;")
        || trimmed.contains("\ncase ")
        || trimmed.contains("\ndo\n")
        || trimmed.contains("\ndone\n")
        || trimmed.contains("export ")
        || {
            fn is_word(s: &str, w: &str) -> bool {
                let mut i = 0;
                let b = s.as_bytes();
                let wb = w.as_bytes();
                while i + wb.len() <= b.len() {
                    if b[i..i + wb.len()].eq_ignore_ascii_case(wb) {
                        let before_ok = i == 0 || {
                            let c = b[i - 1] as char;
                            !c.is_alphanumeric() && c != '_'
                        };
                        let after_ok = i + wb.len() == b.len() || {
                            let c = b[i + wb.len()] as char;
                            !c.is_alphanumeric() && c != '_'
                        };
                        if before_ok && after_ok {
                            return true;
                        }
                    }
                    i += 1;
                }
                false
            }
            let has_do = is_word(trimmed, "do");
            let has_done = is_word(trimmed, "done");
            (has_do && has_done)
                || (has_do
                    && (is_word(trimmed, "for")
                        || is_word(trimmed, "while")
                        || is_word(trimmed, "until")))
                || (is_word(trimmed, "find")
                    && trimmed.contains(" -exec ")
                    && (trimmed.trim_end().ends_with(" +")
                        || trimmed.contains(" {} +")
                        || trimmed.contains(" {} ;")))
        }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct LoginEnvCache {
    rc_mtimes: std::collections::BTreeMap<String, u64>,
    env_vars: std::collections::BTreeMap<String, String>,
    aliases: std::collections::BTreeMap<String, String>,
}

fn cache_path() -> Option<PathBuf> {
    crate::cache_dir().map(|d| d.join("login_env.json"))
}

fn rc_mtimes_for_mode(
    is_login: bool,
    is_interactive: bool,
) -> std::collections::BTreeMap<String, u64> {
    let files: Vec<&str> = if is_login && is_interactive {
        vec![
            ".zprofile",
            ".profile",
            ".bash_profile",
            ".zshrc",
            ".bashrc",
        ]
    } else if is_login {
        vec![".zprofile", ".profile", ".bash_profile"]
    } else if is_interactive {
        vec![".zshrc", ".bashrc"]
    } else {
        return std::collections::BTreeMap::new();
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let mut mtimes = std::collections::BTreeMap::new();
    for rc_file in files {
        let path = Path::new(&home).join(rc_file);
        if let Ok(meta) = std::fs::metadata(&path)
            && let Ok(mtime) = meta.modified()
            && let Ok(ts) = mtime.duration_since(std::time::SystemTime::UNIX_EPOCH)
        {
            mtimes.insert(rc_file.to_string(), ts.as_secs());
        }
    }
    if is_login {
        for p in ["/etc/profile", "/etc/zprofile"] {
            if let Ok(meta) = std::fs::metadata(p)
                && let Ok(mtime) = meta.modified()
                && let Ok(ts) = mtime.duration_since(std::time::SystemTime::UNIX_EPOCH)
            {
                mtimes.insert(p.to_string(), ts.as_secs());
            }
        }
    }
    mtimes
}

fn is_ignored_login_env_var(key: &str) -> bool {
    matches!(
        key,
        "_" | "SHLVL"
            | "PWD"
            | "OLDPWD"
            | "CARGO_MANIFEST_DIR"
            | "CARGO_MANIFEST_PATH"
            | "TMPDIR"
            | "FSH_HANDOFF"
            | "FSH_SESSION_ID"
    ) || key.starts_with("CARGO_PKG_")
}

fn try_load_from_cache(env: &Env, is_login: bool, is_interactive: bool) -> bool {
    let cache_path = match cache_path() {
        Some(p) => p,
        None => return false,
    };
    let data = match std::fs::read_to_string(&cache_path) {
        Ok(d) => d,
        Err(_) => return false,
    };
    let cache: LoginEnvCache = match serde_json::from_str(&data) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let current_mtimes = rc_mtimes_for_mode(is_login, is_interactive);
    if current_mtimes != cache.rc_mtimes {
        return false;
    }

    {
        let mut vars = env.vars.write();
        for (key, val) in &cache.env_vars {
            if is_ignored_login_env_var(key) {
                continue;
            }
            if key == "PATH" {
                unsafe {
                    std::env::set_var("PATH", val);
                }
            }
            vars.insert(key.clone(), Val::String(val.clone()));
        }
    }

    for (name, val) in &cache.aliases {
        let mut parser = fshell_core::Parser::new(val);
        if parser.parse_statements().is_ok() && env.get_builtin(name).is_none() {
            env.register_alias(name, val);
        }
    }

    true
}

fn save_to_cache(env: &Env, is_login: bool, is_interactive: bool) {
    let cache_path = match cache_path() {
        Some(p) => p,
        None => return,
    };

    let current_mtimes = rc_mtimes_for_mode(is_login, is_interactive);
    let mut env_vars = std::collections::BTreeMap::new();
    {
        let vars = env.vars.read();
        for (k, v) in vars.iter() {
            if !is_ignored_login_env_var(k)
                && let Val::String(s) = v
            {
                env_vars.insert(k.clone(), s.clone());
            }
        }
    }

    let mut aliases = std::collections::BTreeMap::new();
    for (k, v) in env.get_all_aliases() {
        aliases.insert(k, v);
    }

    let cache = LoginEnvCache {
        rc_mtimes: current_mtimes,
        env_vars,
        aliases,
    };

    if let Ok(data) = serde_json::to_string(&cache) {
        let tmp = cache_path.with_extension("json.tmp");
        if std::fs::write(&tmp, &data).is_ok() {
            let _ = std::fs::rename(&tmp, &cache_path);
        }
    }
}
// High-level entry point
/// Load the host shell environment appropriate for this invocation.
///
/// * Bumps `SHLVL` for interactive sessions.
/// * Sources resolved profile files directly in-process via POSIX handler.
/// * On failure, never aborts shell startup — login profile errors are
///   warnings, matching bash/zsh behaviour (`$?` is not set, shell
///   continues).
pub async fn load_login_environment(
    env: &Env,
    is_login: bool,
    is_interactive: bool,
) -> Result<(), EngineError> {
    if is_interactive {
        bump_shlvl(env);
    } else {
        ensure_shlvl(env);
    }

    // Record whether this is a login shell as a variable for scripts.
    // Mirrors zsh's `$login_shell` / fish's `status --is-login`.
    {
        let mut vars = env.vars.write();
        vars.insert("FSH_LOGIN".to_string(), Val::Bool(is_login));
    }

    if try_load_from_cache(env, is_login, is_interactive) {
        return Ok(());
    }

    let files = resolve_source_files(is_login, is_interactive);
    for file in &files {
        if let Ok(content) = std::fs::read_to_string(file) {
            if let Some(handler) = crate::posix_handler() {
                let _ = handler(content, Vec::new(), env.clone(), false).await;
            } else {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if let Some(after) = trimmed.strip_prefix("export ")
                        && let Some((k, v)) = after.split_once('=')
                    {
                        let k = k.trim();
                        let mut val = v.trim();
                        if ((val.starts_with('"') && val.ends_with('"'))
                            || (val.starts_with('\'') && val.ends_with('\'')))
                            && val.len() >= 2
                        {
                            val = &val[1..val.len() - 1];
                        }
                        {
                            let mut vars = env.vars.write();
                            vars.insert(k.to_string(), Val::String(val.to_string()));
                        }
                    } else if let Some(after) = trimmed.strip_prefix("alias ")
                        && let Some((k, v)) = after.split_once('=')
                    {
                        let k = k.trim();
                        let mut val = v.trim();
                        if ((val.starts_with('"') && val.ends_with('"'))
                            || (val.starts_with('\'') && val.ends_with('\'')))
                            && val.len() >= 2
                        {
                            val = &val[1..val.len() - 1];
                        }
                        env.register_alias(k, val);
                    }
                }
            }
        }
    }

    save_to_cache(env, is_login, is_interactive);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_login() {
        assert!(detect_login("-fsh", false));
        assert!(detect_login("-fshell", false));
        assert!(detect_login("fsh", true));
        assert!(!detect_login("fsh", false));
    }

    #[test]
    fn test_user_login_precedence() {
        let dir = std::env::temp_dir().join("fsh_login_test_prec");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Simulate .bash_profile present — it should be first
        std::fs::write(dir.join(".bash_profile"), "bp").unwrap();
        std::fs::write(dir.join(".profile"), "prof").unwrap();
        std::fs::write(dir.join(".zprofile"), "zp").unwrap();
        let files = user_login_profiles(&dir);
        assert_eq!(files[0], dir.join(".bash_profile"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_non_login_interactive() {
        let dir = std::env::temp_dir().join("fsh_login_test_nonlogin");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".zshrc"), "rc").unwrap();
        // resolve_source_files uses $HOME, so we can't easily test with a temp
        // dir without mutating HOME. Just sanity-check the interactive helper.
        let rc = interactive_rc_files(&dir);
        assert_eq!(rc, vec![dir.join(".zshrc")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
