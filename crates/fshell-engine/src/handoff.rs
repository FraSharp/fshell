// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::{Param, ResourceHandle, Stmt, Val};
use fshell_hash::FxHashMap;
use std::collections::HashSet;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

/// Serializable snapshot of shell environment for process handoff.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct HandoffState {
    pub vars: FxHashMap<String, Val>,
    #[allow(clippy::type_complexity)]
    pub fns: FxHashMap<String, (Vec<Param>, Option<String>, Vec<Stmt>)>,
    pub caps_held: HashSet<ResourceHandle>,
    pub caps_strict_mode: bool,
    pub reactive_pipelines: FxHashMap<String, String>,
    pub session_id: String,
    pub cwd: String,
    pub options: crate::ShellOptions,
    pub hooks: FxHashMap<String, Vec<String>>,
    pub last_exit_code: i64,
    pub last_duration_secs: f64,
}

/// Atomically write handoff state to a temp file. Returns the final path.
///
/// Durability: fsync the temp file before rename and fsync the parent dir
/// after rename so a crash/power loss cannot leave a torn handoff.json.
/// Privacy: file is 0600, dir is 0700 — handoff contains env vars / secrets.
pub fn save_handoff(state: &HandoffState) -> Result<PathBuf, String> {
    let cache_dir = crate::cache_dir().ok_or_else(|| "HOME not set".to_string())?;
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("Failed to create cache dir: {e}"))?;
    #[cfg(unix)]
    {
        let _ = std::fs::set_permissions(&cache_dir, std::fs::Permissions::from_mode(0o700));
    }

    let final_path = cache_dir.join("handoff.json");
    let tmp_path = cache_dir.join("handoff.json.tmp");

    let content = serde_json::to_string_pretty(state)
        .map_err(|e| format!("Failed to serialize handoff state: {e}"))?;

    // Write with restricted permissions and fsync before rename.
    {
        #[cfg(unix)]
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp_path)
            .map_err(|e| format!("Failed to open handoff temp file: {e}"))?;
        #[cfg(not(unix))]
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)
            .map_err(|e| format!("Failed to open handoff temp file: {e}"))?;
        file.write_all(content.as_bytes())
            .map_err(|e| format!("Failed to write handoff temp file: {e}"))?;
        file.flush()
            .map_err(|e| format!("Failed to flush handoff temp file: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("Failed to fsync handoff temp file: {e}"))?;
        #[cfg(unix)]
        {
            let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
        }
    }

    std::fs::rename(&tmp_path, &final_path)
        .map_err(|e| format!("Failed to atomically move handoff file: {e}"))?;

    // fsync parent dir so the rename is durable.
    if let Ok(dir) = std::fs::File::open(&cache_dir) {
        let _ = dir.sync_all();
    }
    #[cfg(unix)]
    {
        let _ = std::fs::set_permissions(&final_path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(final_path)
}

/// Read and restore handoff state from file.
/// Returns the state on success. On error, the handoff file is cleaned up
/// and the caller should start with a fresh environment.
pub fn load_handoff(path: &std::path::Path) -> Result<HandoffState, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read handoff file: {e}"))?;

    // catch_unwind guards against serde panicking on structural mismatch
    let result = std::panic::catch_unwind(|| {
        serde_json::from_str::<HandoffState>(&content)
            .map_err(|e| format!("Failed to deserialize handoff state: {e}"))
    });

    match result {
        Ok(Ok(state)) => {
            let _ = std::fs::remove_file(path);
            Ok(state)
        }
        Ok(Err(e)) => {
            let backup_path = path.with_extension("json.corrupted");
            let _ = std::fs::rename(path, backup_path);
            Err(e)
        }
        Err(panic) => {
            let backup_path = path.with_extension("json.corrupted");
            let _ = std::fs::rename(path, backup_path);
            let msg = panic
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic during deserialization");
            Err(format!("Handoff deserialization panicked: {msg}"))
        }
    }
}
