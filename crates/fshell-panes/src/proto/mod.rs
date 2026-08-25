// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Binary IPC protocol for client-daemon communication.
//!
//! Wire format: `[length: u32 BE] [type: u8] [payload: bytes]`
//!
//! - [`codec`] — tokio-util `Decoder`/`Encoder` for framed byte streams.
//! - [`message`] — typed message enums (`ClientMessage`, `ServerMessage`).

pub mod codec;
pub mod message;

use std::path::PathBuf;

pub use codec::{Frame, FshCodec, encode_client_message, encode_server_message};
pub use message::{ClientMessage, PrefixCommand, ServerMessage, SessionInfo};

/// Default socket filename.
const SOCKET_NAME: &str = "fshell-panes.sock";

/// Maximum number of pending connections on the Unix socket.
pub const BACKLOG: u32 = 128;

/// Maximum scrollback lines per pane (hard limit to prevent OOM).
pub const SCROLLBACK_LIMIT: usize = 10_000;

/// Session autosave interval in seconds.
pub const AUTOSAVE_INTERVAL_SECS: u64 = 60;

/// Graceful shutdown timeout in milliseconds.
pub const SHUTDOWN_TIMEOUT_MS: u64 = 5_000;

/// Resolve the Unix domain socket path.
///
/// Priority:
/// 1. `$FSH_PANES_SOCKET` (explicit override)
/// 2. `$XDG_RUNTIME_DIR/fshell-panes.sock` (tmpfs, guaranteed local, non-NFS)
/// 3. `/tmp/fshell-panes-$UID.sock` (fallback)
pub fn get_socket_path() -> PathBuf {
    // 1. Explicit override via environment variable.
    if let Ok(path) = std::env::var("FSH_PANES_SOCKET") {
        return PathBuf::from(path);
    }

    // 2. XDG_RUNTIME_DIR (in-memory tmpfs, fast and local).
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime_dir).join(SOCKET_NAME);
        if path
            .parent()
            .and_then(|p| p.exists().then_some(()))
            .is_some()
        {
            return path;
        }
    }

    // 3. Fallback: /tmp with UID to prevent multi-user collisions.
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/fshell-panes-{}.sock", uid))
}

/// Directory for persistent session data.
///
/// Uses `$XDG_DATA_HOME/fshell-panes/` if available, otherwise
/// `~/.local/share/fshell-panes/`.
pub fn get_data_dir() -> PathBuf {
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(data_home).join("fshell-panes");
    }

    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("fshell-panes");
    }

    // Last resort: /tmp/fshell-panes-data-$UID
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/fshell-panes-data-{}", uid))
}

/// Ensure a directory exists, creating it and parents if necessary.
pub fn ensure_dir(path: &PathBuf) -> std::io::Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
    }
    Ok(())
}

/// Clean up a stale socket file if it exists and is not in use.
///
/// Returns `Ok(true)` if the socket was cleaned up, `Ok(false)` if it
/// didn't exist, or `Err` if the socket appears to be in use by a
/// running daemon.
pub fn cleanup_stale_socket(path: &PathBuf) -> std::io::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    // Try to connect to check if the socket is actually in use.
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => {
            // Connection succeeded — daemon is running. Don't clean up.
            Ok(false)
        }
        Err(_) => {
            // Connection failed — socket is stale. Safe to remove.
            std::fs::remove_file(path)?;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_deterministic() {
        let a = get_socket_path();
        let b = get_socket_path();
        assert_eq!(a, b);
    }

    #[test]
    fn data_dir_deterministic() {
        let a = get_data_dir();
        let b = get_data_dir();
        assert_eq!(a, b);
    }

    #[test]
    fn data_dir_ends_with_fsh_panes() {
        let path = get_data_dir();
        assert_eq!(path.file_name().unwrap(), "fshell-panes");
    }
}
