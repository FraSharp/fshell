// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::{Env, ensure_config_dir, resolve_config_dir};
use fshell_core::CommandCompletion;
use fshell_hash::FxHashMap;
use std::fs;

pub fn load_completions(env: &Env) -> Result<(), String> {
    let Some(cfg_dir) = resolve_config_dir() else {
        return Ok(());
    };
    let path = cfg_dir.join("completions.toml");
    if !path.exists() {
        return Ok(());
    }
    let content =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read completions file: {e}"))?;

    let parsed: FxHashMap<String, CommandCompletion> = toml_edit::de::from_str(&content)
        .map_err(|e| format!("Failed to parse completions.toml: {e}"))?;

    let mut reg = env.completions.write();
    for (k, v) in parsed {
        reg.insert(k, v);
    }
    Ok(())
}

pub fn save_completions(env: &Env) -> Result<(), String> {
    let Some(cfg_dir) = ensure_config_dir() else {
        return Err("Could not determine config directory".to_string());
    };
    let path = cfg_dir.join("completions.toml");

    let snapshot = {
        let reg = env.completions.read();
        reg.clone()
    };

    let content = toml_edit::ser::to_string(&snapshot)
        .map_err(|e| format!("Failed to serialize completions: {e}"))?;

    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, &content)
        .map_err(|e| format!("Failed to write {}: {e}", tmp_path.display()))?;
    fs::rename(&tmp_path, &path).map_err(|e| {
        format!(
            "Failed to rename {} -> {}: {e}",
            tmp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}
