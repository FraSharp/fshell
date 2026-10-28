// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::{CapAction, Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;
use std::sync::Arc;

/// Render one exported variable. The `env` map's key set is the export
/// attribute; a key's current value is the shell variable's when it has one,
/// and the map's own value otherwise. `Val::Null` marks a name exported while
/// it has no value, so it is absent from the environment a child would see.
fn show_env_vars(env: &Env, tx: PipeSender) -> Result<(), ShellError> {
    env.enforce_capability("env", CapAction::ReadEnv("*".to_string()))?;
    env.ensure_env_populated();
    let entries: Vec<(String, String)> = {
        let vars = env.vars.read();
        if let Some(Val::Map(env_map)) = vars.get("env") {
            env_map
                .iter()
                .filter_map(|(k, inherited)| {
                    let value = vars.get(k.as_str()).unwrap_or(inherited);
                    if matches!(value, Val::Null) {
                        return None;
                    }
                    Some((k.to_string(), val_to_text(value)))
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

fn val_to_text(value: &Val) -> String {
    match value {
        Val::String(s) => s.clone(),
        Val::Int(i) => i.to_string(),
        Val::Float(f) => f.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

pub fn export_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return show_env_vars(env, tx);
    }

    let mut universal = false;
    let words: Vec<Val> = args
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
    if words.is_empty() {
        return show_env_vars(env, tx);
    }
    env.ensure_env_populated();

    let mut i = 0;
    while i < words.len() {
        let Val::String(word) = &words[i] else {
            return Err(BuiltinError::InvalidArgument {
                cmd: "export".to_string(),
                arg: format!("{:?}", words[i]),
                span: None,
            }
            .into());
        };
        if let Some((key, text)) = word.split_once('=') {
            // `NAME=` with a following word is the spaced form `NAME= value`;
            // otherwise an empty value is the literal `NAME=`.
            let value = if text.is_empty() && i + 1 < words.len() {
                i += 2;
                words[i - 1].clone()
            } else {
                i += 1;
                Val::String(text.to_string())
            };
            set_exported(env, key, value, universal)?;
        } else if i + 2 < words.len() && matches!(&words[i + 1], Val::String(eq) if eq == "=") {
            // `NAME = value`
            set_exported(env, word, words[i + 2].clone(), universal)?;
            i += 3;
        } else if i + 1 < words.len()
            && let Val::String(next) = &words[i + 1]
            && let Some(text) = next.strip_prefix('=')
        {
            // `NAME ="value"`
            set_exported(env, word, Val::String(text.to_string()), universal)?;
            i += 2;
        } else {
            // A bare name is an export mark with no value, as in POSIX:
            // `export FOO` leaves FOO unset, and a later assignment exports it.
            if word.is_empty() {
                return Err(BuiltinError::InvalidArgument {
                    cmd: "export".to_string(),
                    arg: "empty key".to_string(),
                    span: None,
                }
                .into());
            }
            env.enforce_capability("export", CapAction::WriteEnv(word.clone()))?;
            let had_value = env.export_existing_var(word);
            if universal
                && had_value
                && let Some(value) = env.vars.read().get(word.as_str()).cloned()
            {
                env.save_exported_env_var(word, value)
                    .map_err(|e| format!("export: failed to persist '{}': {}", word, e))?;
            }
            i += 1;
        }
    }
    Ok(())
}

fn set_exported(env: &Env, key: &str, value: Val, universal: bool) -> Result<(), ShellError> {
    if key.is_empty() {
        return Err(BuiltinError::InvalidArgument {
            cmd: "export".to_string(),
            arg: "empty key".to_string(),
            span: None,
        }
        .into());
    }
    env.enforce_capability("export", CapAction::WriteEnv(key.to_string()))?;
    env.set_exported_var(key, value.clone());
    if universal {
        env.save_exported_env_var(key, value)
            .map_err(|e| format!("export: failed to persist '{}': {}", key, e))?;
    }
    Ok(())
}

pub fn env_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    show_env_vars(env, tx)
}

pub fn set_universal_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return Err(BuiltinError::MissingArgument {
            cmd: "set-universal".into(),
            description: "at least one key=value or key value pair".into(),
            span,
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
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return Err(BuiltinError::MissingArgument {
            cmd: "unset-universal".into(),
            description: "at least one variable name".into(),
            span,
        }
        .into());
    }
    for arg in args {
        let Val::String(name) = arg else {
            return Err(BuiltinError::InvalidArgument {
                cmd: "unset-universal".into(),
                arg: format!("{:?}", arg),
                span,
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
