// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! PTY Management & Async I/O
//!
//! Connects the terminal grid to a real OS shell process via
//! non-blocking async PTY I/O.

pub mod async_pty;
pub mod grid_manager;
pub mod pty_actor;

/// Get the default shell process path.
/// Prefers `fsh` or `fshell` (from environment, relative binary directory, or PATH) if available,
/// otherwise falls back to the user's `SHELL` environment variable or `/bin/sh`.
pub fn get_default_shell() -> String {
    // 1. Explicit override via environment variable.
    if let Ok(sh) = std::env::var("FSH_SHELL").or_else(|_| std::env::var("FSHELL_BIN"))
        && !sh.is_empty()
        && std::path::Path::new(&sh).exists()
    {
        return sh;
    }

    // 2. Check if `fsh` or `fshell` exists alongside the running binary (e.g. target/debug/fsh).
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        for name in &["fsh", "fshell"] {
            let candidate = parent.join(name);
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    // 3. Check PATH for `fsh` or `fshell`.
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for name in &["fsh", "fshell"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return candidate.to_string_lossy().into_owned();
                }
            }
        }
    }

    // 4. Fall back to user's SHELL environment variable or /bin/sh.
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}
