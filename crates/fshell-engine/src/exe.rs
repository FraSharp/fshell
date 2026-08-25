// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Cached, resolved path to the currently running fsh binary.
///
/// Resolution order:
/// 1. `std::env::current_exe()` — works on both macOS and Linux.
/// 2. Fallback to `std::env::args().next()` canonicalized — handles the
///    edge case where the binary was deleted after launch (Linux reports
///    "/proc/self/exe (deleted)", macOS current_exe can fail under sandbox).
/// 3. Final fallback: raw `args[0]` as PathBuf (may be relative, but better than nothing).
static CACHED_EXE: OnceLock<PathBuf> = OnceLock::new();

/// Resolve the current executable path, caching the result for the lifetime of the process.
pub fn resolve_exe() -> PathBuf {
    CACHED_EXE.get_or_init(resolve_exe_inner).clone()
}

fn resolve_exe_inner() -> PathBuf {
    if let Ok(p) = std::env::current_exe() {
        if looks_valid(&p) {
            return p;
        }
        // On Linux a deleted binary shows as "... (deleted)" — try to strip suffix
        if let Some(stripped) = strip_deleted_suffix(&p)
            && stripped.exists()
        {
            return stripped;
        }
        // If current_exe gave a path that doesn't look valid, fall through to argv0 fallback
    }

    if let Some(argv0) = std::env::args().next() {
        let raw = PathBuf::from(&argv0);
        // Try canonicalize first (resolves symlinks like multicall `ls` -> `fsh`)
        if let Ok(canon) = raw.canonicalize()
            && looks_valid(&canon)
        {
            return canon;
        }
        // Try absolute resolution via current_dir + raw
        if raw.is_relative()
            && let Ok(cwd) = std::env::current_dir()
        {
            let joined = cwd.join(&raw);
            if let Ok(canon) = joined.canonicalize() {
                return canon;
            }
            return joined;
        }
        return raw;
    }

    PathBuf::from("fsh")
}

fn looks_valid(p: &Path) -> bool {
    // Heuristic: a valid exe should exist or at least have a file name without "(deleted)"
    let s = p.to_string_lossy();
    !s.contains("(deleted)") && p.file_name().is_some()
}

fn strip_deleted_suffix(p: &Path) -> Option<PathBuf> {
    let s = p.to_string_lossy();
    s.find(" (deleted)").map(|idx| PathBuf::from(&s[..idx]))
}

/// Current process id.
pub fn current_pid() -> u32 {
    std::process::id()
}

/// Compile-time fsh version (bare semver, e.g. "0.1.0").
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Build datetime in compact form `YYYYMMDD-HHMM` (UTC), set by build.rs.
/// Falls back to `None` if the build script did not run (e.g. `cargo check` with no rebuild).
pub fn build_datetime() -> Option<&'static str> {
    option_env!("FSH_BUILD_DATETIME")
}

/// ISO-8601 build datetime (`YYYY-MM-DDTHH:MM:SSZ`), set by build.rs.
pub fn build_datetime_iso() -> Option<&'static str> {
    option_env!("FSH_BUILD_DATETIME_ISO")
}

/// Unix timestamp (seconds since epoch) of the build, set by build.rs.
pub fn build_timestamp() -> Option<&'static str> {
    option_env!("FSH_BUILD_TIMESTAMP")
}

/// Short git commit hash at build time, set by build.rs if git was available.
pub fn git_commit() -> Option<&'static str> {
    option_env!("FSH_GIT_COMMIT")
}

/// Full version string including build datetime, e.g. `"0.1.0 20260828-1738"` or
/// `"0.1.0"` if no datetime is available.
pub fn full_version() -> String {
    match build_datetime() {
        Some(dt) if !dt.is_empty() && dt != "unknown" => format!("{} {}", version(), dt),
        _ => version().to_string(),
    }
}

/// Human-friendly version line, e.g. `"fsh v0.1.0 20260828-1738"` or `"fsh v0.1.0"`.
pub fn version_string() -> String {
    match build_datetime() {
        Some(dt) if !dt.is_empty() && dt != "unknown" => format!("fsh v{} {}", version(), dt),
        _ => format!("fsh v{}", version()),
    }
}

/// Build profile (debug/release) derived from cfg.
pub fn build_profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// Structured self info. Used by `self --structured` and `self --json`.
pub fn self_info_map() -> fshell_core::FxIndexMap<ustr::Ustr, fshell_core::Val> {
    let mut m = fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    m.insert(
        ustr::ustr("exe"),
        fshell_core::Val::String(resolve_exe().to_string_lossy().to_string()),
    );
    m.insert(
        ustr::ustr("pid"),
        fshell_core::Val::Int(current_pid() as i64),
    );
    m.insert(
        ustr::ustr("version"),
        fshell_core::Val::String(version().to_string()),
    );
    m.insert(
        ustr::ustr("full_version"),
        fshell_core::Val::String(full_version()),
    );
    if let Some(dt) = build_datetime() {
        m.insert(
            ustr::ustr("build_datetime"),
            fshell_core::Val::String(dt.to_string()),
        );
    }
    if let Some(iso) = build_datetime_iso() {
        m.insert(
            ustr::ustr("build_datetime_iso"),
            fshell_core::Val::String(iso.to_string()),
        );
    }
    if let Some(ts) = build_timestamp() {
        if let Ok(n) = ts.parse::<i64>() {
            m.insert(ustr::ustr("build_timestamp"), fshell_core::Val::Int(n));
        } else {
            m.insert(
                ustr::ustr("build_timestamp"),
                fshell_core::Val::String(ts.to_string()),
            );
        }
    }
    if let Some(commit) = git_commit() {
        m.insert(
            ustr::ustr("git_commit"),
            fshell_core::Val::String(commit.to_string()),
        );
    }
    m.insert(
        ustr::ustr("profile"),
        fshell_core::Val::String(build_profile().to_string()),
    );
    // argv[0] as seen by the process, for debugging multicall
    let argv0 = std::env::args().next().unwrap_or_default();
    m.insert(ustr::ustr("argv0"), fshell_core::Val::String(argv0));
    m
}

/// Execute the current exe with the given args, replacing the current process.
/// Never returns on success. Returns an error string on failure.
pub fn exec_self(args: &[String]) -> Result<(), String> {
    let exe = resolve_exe();
    let exe_str = exe.to_string_lossy().to_string();
    let c_exe = std::ffi::CString::new(exe_str.as_bytes()).map_err(|_| "Null byte in exe path")?;

    // Build argv: [exe, ...args, null]
    let mut cstrs: Vec<std::ffi::CString> = Vec::with_capacity(args.len() + 1);
    cstrs.push(c_exe.clone());
    for a in args {
        let c = std::ffi::CString::new(a.as_bytes()).map_err(|_| "Null byte in argument")?;
        cstrs.push(c);
    }
    let ptrs: Vec<*const libc::c_char> = cstrs
        .iter()
        .map(|c| c.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();

    crate::suspend_session_logging();

    // Terminate background jobs gracefully before exec (mirrors reload --full)
    // We intentionally do not hold locks across exec.
    unsafe {
        libc::execvp(c_exe.as_ptr(), ptrs.as_ptr());
    }

    Err(format!(
        "execvp failed: {}",
        std::io::Error::last_os_error()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_exe_not_empty() {
        let p = resolve_exe();
        assert!(!p.as_os_str().is_empty());
        assert!(p.to_string_lossy().contains("fsh") || p.file_name().is_some());
    }

    #[test]
    fn test_self_info_map_has_keys() {
        let m = self_info_map();
        assert!(m.contains_key(&ustr::ustr("exe")));
        assert!(m.contains_key(&ustr::ustr("pid")));
        assert!(m.contains_key(&ustr::ustr("version")));
    }

    #[test]
    fn test_version_not_empty() {
        assert!(!version().is_empty());
        assert!(!full_version().is_empty());
    }

    #[test]
    fn test_full_version_contains_version() {
        assert!(full_version().contains(version()));
    }
}
