// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
#![cfg(unix)]

pub mod profile;
#[cfg(target_os = "linux")]
pub mod sandbox_linux;
#[cfg(target_os = "macos")]
pub mod sandbox_macos;

pub use profile::{SandboxMode, SandboxProfile};

use fshell_core::Val;
use fshell_engine::{Env, PipeSender, PipeStream};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWriteExt};
use tokio::process::Command;

/// Configuration for the subprocess sandbox.
#[derive(Clone, Debug)]
pub struct SandboxConfig {
    pub mode: SandboxMode,
    pub profile: SandboxProfile,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        let mode = SandboxMode::ReadOnlySystem;
        Self {
            mode: mode.clone(),
            profile: SandboxProfile::new(mode),
        }
    }
}

impl SandboxConfig {
    pub fn new(mode: SandboxMode) -> Self {
        Self {
            mode: mode.clone(),
            profile: SandboxProfile::new(mode),
        }
    }

    pub fn with_profile(profile: SandboxProfile) -> Self {
        Self {
            mode: profile.mode.clone(),
            profile,
        }
    }
}

fn pipe_input(
    mut rx: PipeStream,
    mut stdin: tokio::process::ChildStdin,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(payload) = rx.recv().await {
            if let fshell_engine::PipelinePayload::Data(data) = payload {
                let bytes: &[u8] = match &*data {
                    Val::Blob(b) => b.as_slice(),
                    Val::String(s) => s.as_bytes(),
                    _ => continue,
                };
                let _ = stdin.write_all(bytes).await;
            }
        }
    })
}

fn pipe_output<R>(mut reader: R, tx: PipeSender) -> tokio::task::JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = buf[..n].to_vec();
                    let val = match String::from_utf8(chunk) {
                        Ok(s) => Val::String(s),
                        Err(e) => Val::Blob(e.into_bytes()),
                    };
                    if tx
                        .send(fshell_engine::PipelinePayload::Data(Arc::new(val)))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

/// Spawn a subprocess protected by OS kernel sandboxing (Landlock on Linux, SBPL on macOS).
pub fn run_sandboxed(
    name: &str,
    args: &[Val],
    in_rx: Option<PipeStream>,
    env: &Env,
    tx: PipeSender,
    config: &SandboxConfig,
) -> Result<(), String> {
    fshell_core::debug_log!("run_sandboxed: name={}, mode={:?}", name, config.mode);

    if config.mode == SandboxMode::DenyAll {
        eprintln!("[fshell sandbox] warning: 'DenyAll' is deprecated, use 'ReadOnlySystem'");
    }
    // Prompt mode: interactively confirm before applying sandbox
    let effective_mode = if config.mode == SandboxMode::Prompt {
        let is_tty =
            !fshell_engine::is_test_mode() && unsafe { libc::isatty(libc::STDIN_FILENO) == 1 };
        if is_tty {
            eprint!(
                "[fshell sandbox:prompt] Run '{}' sandboxed (ReadOnlySystem, cwd={}, allow={:?})? [Y/n] ",
                name,
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .display(),
                config.profile.allow_write_paths
            );
            let _ = std::io::Write::flush(&mut std::io::stderr());
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            match line.trim().to_lowercase().as_str() {
                "n" | "no" => {
                    return Err("sandbox prompt denied by user".into());
                }
                _ => SandboxMode::ReadOnlySystem,
            }
        } else {
            SandboxMode::ReadOnlySystem
        }
    } else {
        config.mode.clone()
    };

    let mut cmd = Command::new(name);
    cmd.args(args.iter().map(|v| match v {
        Val::String(s) => s.clone(),
        Val::Int(i) => i.to_string(),
        Val::Float(f) => f.to_string(),
        Val::Bool(b) => b.to_string(),
        Val::Null => "null".to_string(),
        Val::Map(_) | Val::List(_) => v.to_text(),
        Val::DateTime(dt) => dt.to_rfc3339(),
        Val::Blob(bytes) => format!("<Blob: {} bytes>", bytes.len()),
        other => format!("<{}>", other.type_name()),
    }));

    if in_rx.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // Populate environment
    env.ensure_env_populated();
    if let Some(Val::Map(env_map)) = Some(env.vars.read()).and_then(|v| v.get("env").cloned()) {
        for (k, v) in &env_map {
            if let Val::String(s) = v {
                cmd.env(k.as_str(), s);
            }
        }
    }

    // Set working directory
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cmd.current_dir(&cwd);

    // Apply OS kernel sandbox via pre_exec (unless Off)
    if effective_mode != SandboxMode::Off {
        let effective_profile = if config.mode == SandboxMode::Prompt {
            let mut p = config.profile.clone();
            p.mode = effective_mode.clone();
            p
        } else {
            config.profile.clone()
        };
        let profile = effective_profile;
        let cwd_clone = cwd.clone();
        unsafe {
            cmd.pre_exec(move || {
                #[cfg(target_os = "linux")]
                sandbox_linux::apply_landlock_sandbox(&profile, &cwd_clone)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e))?;
                #[cfg(target_os = "macos")]
                sandbox_macos::apply_sbpl_sandbox(&profile, &cwd_clone)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e))?;
                Ok(())
            });
        }
    }

    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;

    // Pipe stdin from in_rx to child stdin if provided
    if let Some(rx) = in_rx
        && let Some(stdin) = child.stdin.take()
    {
        pipe_input(rx, stdin);
    }

    // Pipe stdout to tx
    let stdout = child.stdout.take().ok_or("stdout not available")?;
    let stdout_handle = pipe_output(stdout, tx.clone());

    // Pipe stderr to tx
    let stderr = child.stderr.take().ok_or("stderr not available")?;
    let stderr_handle = pipe_output(stderr, tx.clone());

    // Wait for child in background task
    tokio::spawn(async move {
        let status = child.wait().await;
        let code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
        let _ = tokio::join!(stdout_handle, stderr_handle);
        let _ = tx
            .send(fshell_engine::PipelinePayload::Data(Arc::new(Val::String(
                format!("\0exit:{code}"),
            ))))
            .await;
    });

    Ok(())
}
