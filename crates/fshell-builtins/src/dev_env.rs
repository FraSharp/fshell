// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use fshell_core::Val;
use fshell_core::diagnostic::StringError;
use fshell_engine::{CapAction, Env, PipeSender, PipeStream};

pub fn eval_direnv_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    // Check process spawn capability
    env.enforce_capability("eval_direnv", CapAction::ProcessSpawn)?;

    let current_dir = std::env::current_dir().map_err(|e| format!("direnv: {}", e))?;

    // Check if .envrc or .env exists first to avoid spawning process unnecessarily
    let envrc_exists = current_dir.join(".envrc").exists();
    let dotenv_exists = current_dir.join(".env").exists();
    if !envrc_exists && !dotenv_exists {
        drop(tx);
        return Ok(());
    }

    // Run `direnv export json`
    let output = match std::process::Command::new("direnv")
        .args(["export", "json"])
        .current_dir(&current_dir)
        .output()
    {
        Ok(out) => out,
        Err(_) => {
            // direnv is probably not installed, fallback to direct .env loading if it exists
            if dotenv_exists {
                return load_env_file_internal(env, tx);
            }
            drop(tx);
            return Ok(());
        }
    };

    if !output.status.success() {
        // direnv failed, fallback to direct .env if it exists
        if dotenv_exists {
            return load_env_file_internal(env, tx);
        }
        let err_msg = String::from_utf8_lossy(&output.stderr).to_string();
        drop(tx);
        return Err(BuiltinError::CommandFailed {
            cmd: "direnv".into(),
            status: output.status.code().unwrap_or(1),
            stderr: err_msg.clone(),
        }
        .into());
    }

    let json_str = String::from_utf8_lossy(&output.stdout).to_string();
    if json_str.trim().is_empty() || json_str.trim() == "{}" {
        drop(tx);
        return Ok(());
    }

    // Parse the JSON env map
    let parsed: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| format!("Failed to parse direnv JSON output: {}", e))?;

    if let serde_json::Value::Object(map) = parsed {
        let mut vars = env.vars.write();
        for (key, val) in map {
            match val {
                serde_json::Value::Null => {
                    vars.remove(&key);
                }
                serde_json::Value::String(s) => {
                    vars.insert(key, Val::String(s));
                }
                other => {
                    vars.insert(key, Val::String(other.to_string()));
                }
            }
        }
    }

    drop(tx);
    Ok(())
}

pub fn load_env_file_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    load_env_file_internal(env, tx)
}

fn load_env_file_internal(env: &Env, tx: PipeSender) -> Result<(), StringError> {
    let current_dir = std::env::current_dir().map_err(|e| format!("load_env: {}", e))?;
    let dotenv_path = current_dir.join(".env");

    if !dotenv_path.exists() {
        drop(tx);
        return Ok(());
    }

    // Check capability to read the file
    env.enforce_capability("load_env", CapAction::ReadFile(dotenv_path.clone()))?;

    let content = std::fs::read_to_string(&dotenv_path)
        .map_err(|e| format!("Failed to read .env file: {}", e))?;

    let mut vars = env.vars.write();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // strip optional leading "export "
        let mut process_line = trimmed;
        if let Some(after) = trimmed.strip_prefix("export ") {
            process_line = after.trim();
        }

        if let Some((key, val_raw)) = process_line.split_once('=') {
            let key = key.trim();
            let mut val = val_raw.trim();

            // strip optional quotes
            if ((val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\'')))
                && val.len() >= 2
            {
                val = &val[1..val.len() - 1];
            }

            vars.insert(key.to_string(), Val::String(val.to_string()));
        }
    }

    drop(tx);
    Ok(())
}
