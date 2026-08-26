// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use fshell_core::ShellError;
use fshell_core::diagnostic::ErrorCode;
use fshell_core::{ResourceHandle, Val};
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;
use std::path::PathBuf;
use std::sync::Arc;

pub fn fs_readwrite_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    _env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let path_str = if !args.is_empty() {
        match &args[0] {
            Val::String(s) => s.clone(),
            _ => {
                return Err("fs-readwrite argument must be a path string"
                    .to_string()
                    .into());
            }
        }
    } else {
        return Err(ShellError::new(
            ErrorCode::MissingArgument,
            "fs-readwrite: missing path operand",
        )
        .maybe_with_span(span));
    };
    let path = crate::utils::expand_tilde(&path_str);
    let path2 = path.clone();
    tokio::spawn(async move {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::Capability(
                ResourceHandle::ReadFile(path),
            ))))
            .await;
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::Capability(
                ResourceHandle::WriteFile(path2),
            ))))
            .await;
    });
    Ok(())
}

pub fn net_all_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    _env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    tokio::spawn(async move {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::Capability(
                ResourceHandle::NetworkAll,
            ))))
            .await;
    });
    Ok(())
}

pub fn process_spawn_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    _env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    tokio::spawn(async move {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::Capability(
                ResourceHandle::ProcessSpawn,
            ))))
            .await;
    });
    Ok(())
}

/// Canonical string form of a capability token — used both for display and as
/// the sort key so list output and revoke indices stay consistent.
fn cap_label(h: &ResourceHandle) -> String {
    match h {
        ResourceHandle::ReadDir(p) => format!("fs.read({:?})", p),
        ResourceHandle::WriteDir(p) => format!("fs.write({:?})", p),
        ResourceHandle::ReadFile(p) => format!("fs.readfile({:?})", p),
        ResourceHandle::WriteFile(p) => format!("fs.writefile({:?})", p),
        ResourceHandle::NetworkAll => "net.all".to_string(),
        ResourceHandle::NetworkSocket(host) => format!("net.connect({})", host),
        ResourceHandle::ReadEnv(var) => format!("env.read({})", var),
        ResourceHandle::WriteEnv(var) => format!("env.write({})", var),
        ResourceHandle::ProcessSpawn => "process.spawn".to_string(),
        ResourceHandle::ProcessSpawnPath(cmd) => format!("process.spawn({})", cmd),
    }
}

pub fn caps_audit_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let action = match args.first() {
        Some(Val::String(s)) => s.as_str(),
        _ => "log",
    };

    if action == "list" {
        let caps = env.caps.caps.read();
        let mut sorted_caps: Vec<String> = caps.held.iter().map(cap_label).collect();
        sorted_caps.sort();

        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(
                    "=== Active Capabilities ===".to_string(),
                ))))
                .await;
            for (idx, cap) in sorted_caps.iter().enumerate() {
                let payload =
                    PipelinePayload::Data(Arc::new(Val::String(format!("{}) {}", idx + 1, cap))));
                if tx.send(payload).await.is_err() {
                    break;
                }
            }
        });
        return Ok(());
    }

    if action == "revoke" {
        let index_arg = match args.get(1) {
            Some(Val::String(s)) => s.as_str(),
            _ => return Err("usage: caps-audit revoke <index>".to_string().into()),
        };

        let target_idx = index_arg
            .parse::<usize>()
            .map_err(|_| "caps-audit revoke: index must be a valid number".to_string())?;

        let mut caps = env.caps.caps.write();
        let mut sorted_handles: Vec<ResourceHandle> = caps.held.iter().cloned().collect();
        sorted_handles.sort_by_key(cap_label);

        if target_idx == 0 || target_idx > sorted_handles.len() {
            return Err(BuiltinError::InvalidArgument {
                cmd: "caps-audit".into(),
                arg: format!(
                    "revoke: index must be between 1 and {}",
                    sorted_handles.len()
                ),
                span,
            }
            .into());
        }

        let target_handle = &sorted_handles[target_idx - 1];
        caps.revoke(target_handle);

        let caps_path = fshell_engine::config_dir().map(|d| d.join("caps.json"));
        if let Some(path) = caps_path {
            let held = caps.held.clone();
            if let Ok(content) = serde_json::to_string(&held) {
                let _ = std::fs::write(&path, content);
            }
        }

        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(
                    "Capability revoked successfully.".to_string(),
                ))))
                .await;
        });
        return Ok(());
    }

    if action == "clean" {
        let mut caps = env.caps.caps.write();
        let pwd = env.cwd();
        caps.held.clear();
        caps.grant(ResourceHandle::ReadDir(pwd.clone()));
        caps.grant(ResourceHandle::WriteDir(pwd.clone()));
        caps.grant(ResourceHandle::ReadFile(pwd.clone()));
        caps.grant(ResourceHandle::WriteFile(pwd));
        caps.grant(ResourceHandle::ProcessSpawn);

        let caps_path = fshell_engine::config_dir().map(|d| d.join("caps.json"));
        if let Some(path) = caps_path {
            let held = caps.held.clone();
            if let Ok(content) = serde_json::to_string(&held) {
                let _ = std::fs::write(&path, content);
            }
        }

        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(
                    "Capabilities database cleaned. Pruned all persistent dynamic grants."
                        .to_string(),
                ))))
                .await;
        });
        return Ok(());
    }

    let logs = env
        .caps
        .audit_log
        .lock()
        .map_err(|_| "Lock poisoned: audit_log".to_string())?
        .clone();
    tokio::spawn(async move {
        for log in logs {
            let payload = PipelinePayload::Data(Arc::new(Val::String(log)));
            if tx.send(payload).await.is_err() {
                break;
            }
        }
    });
    Ok(())
}

pub fn strict_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let action = match args.first() {
        Some(Val::String(s)) => s.as_str(),
        _ => {
            return Err("usage: strict <command> [args...]\n       strict on | off"
                .to_string()
                .into());
        }
    };

    if action == "on" {
        let mut caps = env.caps.caps.write();
        caps.strict_mode = true;
        return Ok(());
    }
    if action == "off" {
        let mut caps = env.caps.caps.write();
        caps.strict_mode = false;
        return Ok(());
    }

    let name = match &args[0] {
        Val::String(s) => s.clone(),
        _ => {
            return Err(ShellError::new(
                ErrorCode::InvalidArgument,
                "strict: command name must be a string",
            )
            .maybe_with_span(span));
        }
    };
    let cmd_args: Vec<Val> = args[1..].to_vec();

    env.caps
        .strict_mode_temp_count
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

    let result = if let Some(handler) = env.get_builtin(&name) {
        handler(in_rx, cmd_args, env, tx, span)
    } else if let Some(fallback) = env.get_fallback_handler() {
        fallback(&name, cmd_args, in_rx, env, tx, false, span)
    } else {
        Err(ShellError::command_not_found(&name, span))
    };

    env.caps
        .strict_mode_temp_count
        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);

    result
}

#[allow(clippy::type_complexity)]
pub fn make_path_cap_builtin(
    name: &'static str,
    variant: fn(PathBuf) -> ResourceHandle,
) -> impl Fn(
    Option<PipeStream>,
    Vec<Val>,
    &Env,
    PipeSender,
    Option<SourceSpan>,
) -> Result<(), ShellError>
+ Send
+ Sync
+ 'static {
    move |_in_rx, args, _env, tx, _span| {
        let val = if !args.is_empty() {
            match &args[0] {
                Val::String(s) => s.clone(),
                _ => {
                    return Err(BuiltinError::InvalidArgument {
                        cmd: name.into(),
                        arg: "argument must be a path string".into(),
                        span: None,
                    }
                    .into());
                }
            }
        } else {
            return Err(BuiltinError::MissingArgument {
                cmd: name.into(),
                description: "path operand".into(),
                span: None,
            }
            .into());
        };
        let path = crate::utils::expand_tilde(&val);
        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::Capability(variant(
                    path,
                )))))
                .await;
        });
        Ok(())
    }
}

#[allow(clippy::type_complexity)]
pub fn make_str_cap_builtin(
    name: &'static str,
    variant: fn(String) -> ResourceHandle,
) -> impl Fn(
    Option<PipeStream>,
    Vec<Val>,
    &Env,
    PipeSender,
    Option<SourceSpan>,
) -> Result<(), ShellError>
+ Send
+ Sync
+ 'static {
    move |_in_rx, args, _env, tx, _span| {
        let val = if !args.is_empty() {
            match &args[0] {
                Val::String(s) => s.clone(),
                _ => {
                    return Err(BuiltinError::InvalidArgument {
                        cmd: name.into(),
                        arg: "argument must be a string".into(),
                        span: None,
                    }
                    .into());
                }
            }
        } else {
            return Err(BuiltinError::MissingArgument {
                cmd: name.into(),
                description: "operand".into(),
                span: None,
            }
            .into());
        };
        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::Capability(variant(
                    val,
                )))))
                .await;
        });
        Ok(())
    }
}
