// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::{CapAction, Env, PipeSender, PipeStream, PipelinePayload};
use std::sync::Arc;

fn show_env_vars(env: &Env, tx: PipeSender) -> Result<(), ShellError> {
    env.enforce_capability("env", CapAction::ReadEnv("*".to_string()))?;
    env.ensure_env_populated();
    let entries: Vec<(String, String)> = {
        let vars = env.vars.read();
        if let Some(Val::Map(env_map)) = vars.get("env") {
            env_map
                .iter()
                .map(|(k, v)| {
                    let val_str = match v {
                        Val::String(s) => s.clone(),
                        Val::Int(i) => i.to_string(),
                        Val::Float(f) => f.to_string(),
                        other => serde_json::to_string(other).unwrap_or_default(),
                    };
                    (k.to_string(), val_str)
                })
                .collect()
        } else {
            std::env::vars().collect()
        }
    };

    tokio::spawn(async move {
        for (key, value) in entries {
            let mut map =
                fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            map.insert(ustr::ustr("key"), Val::String(key));
            map.insert(ustr::ustr("value"), Val::String(value));
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::Map(map))))
                .await;
        }
    });

    Ok(())
}

pub fn export_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return show_env_vars(env, tx);
    }

    let mut universal = false;
    let kv_args: Vec<Val> = args
        .into_iter()
        .filter(|arg| {
            if let Val::String(s) = arg
                && (s == "--universal" || s == "-U")
            {
                universal = true;
                return false;
            }
            true
        })
        .collect();

    let pairs = parse_key_value_pairs(&kv_args, "export")?;
    env.ensure_env_populated();

    for (key, val) in pairs {
        env.enforce_capability("export", CapAction::WriteEnv(key.clone()))?;
        env.set_exported_var(&key, val.clone());

        if universal {
            env.save_exported_env_var(&key, val)
                .map_err(|e| format!("export: failed to persist '{}': {}", key, e))?;
        }
    }
    Ok(())
}

pub fn env_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    show_env_vars(env, tx)
}

pub fn set_universal_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _tx: PipeSender,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return Err(BuiltinError::MissingArgument {
            cmd: "set-universal".into(),
            description: "at least one key=value or key value pair".into(),
            span: None,
        }
        .into());
    }

    let pairs = parse_key_value_pairs(&args, "set-universal")?;

    for (key, val) in pairs {
        env.save_universal_var(&key, val)
            .map_err(|e| format!("set-universal: failed to save '{}': {}", key, e))?;
    }

    Ok(())
}

pub fn unset_universal_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _tx: PipeSender,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return Err(BuiltinError::MissingArgument {
            cmd: "unset-universal".into(),
            description: "at least one variable name".into(),
            span: None,
        }
        .into());
    }
    for arg in args {
        let Val::String(name) = arg else {
            return Err(BuiltinError::InvalidArgument {
                cmd: "unset-universal".into(),
                arg: format!("{:?}", arg),
                span: None,
            }
            .into());
        };
        env.remove_universal_var(&name)
            .map_err(|e| format!("unset-universal: failed to remove '{}': {}", name, e))?;
    }
    Ok(())
}

fn parse_key_value_pairs(args: &[Val], cmd: &str) -> Result<Vec<(String, Val)>, BuiltinError> {
    let mut pairs = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match &args[i] {
            Val::String(s) => {
                if let Some(eq_pos) = s.find('=') {
                    let key = s[..eq_pos].to_string();
                    let val_str = &s[eq_pos + 1..];
                    if val_str.is_empty() && i + 1 < args.len() {
                        // Syntax: export KEY= "value"
                        let val = args[i + 1].clone();
                        pairs.push((key, val));
                        i += 2;
                    } else {
                        // Syntax: export KEY=value
                        let val = Val::String(val_str.to_string());
                        pairs.push((key, val));
                        i += 1;
                    }
                } else if i + 2 < args.len() && matches!(&args[i + 1], Val::String(eq) if eq == "=")
                {
                    // Syntax: export KEY = "value" (or KEY = value)
                    let key = s.clone();
                    let val = args[i + 2].clone();
                    pairs.push((key, val));
                    i += 3;
                } else if i + 1 < args.len() {
                    if let Val::String(next_s) = &args[i + 1]
                        && let Some(val_part) = next_s.strip_prefix('=')
                    {
                        // Syntax: export KEY ="value"
                        let key = s.clone();
                        let val = Val::String(val_part.to_string());
                        pairs.push((key, val));
                        i += 2;
                    } else {
                        // Syntax: export KEY "value"
                        let key = s.clone();
                        let val = args[i + 1].clone();
                        pairs.push((key, val));
                        i += 2;
                    }
                } else {
                    return Err(BuiltinError::MissingArgument {
                        cmd: cmd.to_string(),
                        description: format!("value for key '{s}'"),
                        span: None,
                    });
                }
            }
            other => {
                return Err(BuiltinError::InvalidArgument {
                    cmd: cmd.to_string(),
                    arg: format!("{:?}", other),
                    span: None,
                });
            }
        }
    }

    for (key, _) in &pairs {
        if key.is_empty() {
            return Err(BuiltinError::InvalidArgument {
                cmd: cmd.to_string(),
                arg: "empty key".to_string(),
                span: None,
            });
        }
        let mut chars = key.chars();
        let Some(first) = chars.next() else {
            return Err(BuiltinError::InvalidArgument {
                cmd: cmd.to_string(),
                arg: "invalid key".into(),
                span: None,
            });
        };
        if !first.is_ascii_alphabetic() && first != '_' {
            return Err(BuiltinError::InvalidArgument {
                cmd: cmd.to_string(),
                arg: format!("key '{key}' must start with a letter or underscore"),
                span: None,
            });
        }
        if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(BuiltinError::InvalidArgument {
                cmd: cmd.to_string(),
                arg: format!("key '{key}' must be alphanumeric + underscores only"),
                span: None,
            });
        }
    }

    Ok(pairs)
}
