// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_core::diagnostic::ErrorCode;
use fshell_engine::{Env, PipeSender, PipeStream};
use fshell_sandbox::{SandboxConfig, SandboxMode, run_sandboxed};

pub fn sandbox_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return Err("usage: sandbox [options] <command> [args...]

Options:
  --read-only-system, --deny-all  Block write mutations to system directories (default)
  --isolated, --no-net            Block write mutations AND isolate network access
  --allow-path <PATH>             Explicitly grant write access to specified path
  --deny-path <PATH>              Explicitly deny write access to specified path
  --allow-all, --off              Run command without OS sandbox confinement"
            .into());
    }

    let mut profile = fshell_sandbox::SandboxProfile::default();
    let mut arg_idx = 0;

    while arg_idx < args.len() {
        match &args[arg_idx] {
            Val::String(s) if s == "--allow-all" || s == "--off" => {
                profile.mode = SandboxMode::Off;
                arg_idx += 1;
            }
            Val::String(s) if s == "--deny-all" || s == "--read-only-system" => {
                profile.mode = SandboxMode::ReadOnlySystem;
                arg_idx += 1;
            }
            Val::String(s) if s == "--isolated" || s == "--no-net" => {
                profile.mode = SandboxMode::Isolated;
                profile.allow_network = false;
                arg_idx += 1;
            }
            Val::String(s) if s == "--monitor" || s == "--prompt" => {
                profile.mode = SandboxMode::ReadOnlySystem;
                arg_idx += 1;
            }
            Val::String(s) if s == "--allow-path" => {
                if arg_idx + 1 < args.len() {
                    let path_val = &args[arg_idx + 1];
                    let path_str = path_val.to_text();
                    profile
                        .allow_write_paths
                        .push(crate::utils::expand_tilde(&path_str));
                    arg_idx += 2;
                } else {
                    return Err("sandbox: --allow-path requires a path argument".into());
                }
            }
            Val::String(s) if s == "--deny-path" => {
                if arg_idx + 1 < args.len() {
                    let path_val = &args[arg_idx + 1];
                    let path_str = path_val.to_text();
                    profile
                        .deny_write_paths
                        .push(crate::utils::expand_tilde(&path_str));
                    arg_idx += 2;
                } else {
                    return Err("sandbox: --deny-path requires a path argument".into());
                }
            }
            _ => break,
        }
    }

    if arg_idx >= args.len() {
        return Err(ShellError::new(
            ErrorCode::MissingArgument,
            "sandbox: missing command",
        ));
    }

    let name = match &args[arg_idx] {
        Val::String(s) => s.clone(),
        other => {
            return Err(BuiltinError::InvalidArgument {
                cmd: "sandbox".into(),
                arg: format!("expected command name, got {other:?}"),
                span: None,
            }
            .into());
        }
    };
    let cmd_args: Vec<Val> = args[arg_idx + 1..].to_vec();

    let config = SandboxConfig::with_profile(profile);
    run_sandboxed(&name, &cmd_args, in_rx, env, tx, &config).map_err(ShellError::from)
}

pub fn unsafe_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return Err("usage: unsafe <command> [args...]".into());
    }

    let name = match &args[0] {
        Val::String(s) => s.clone(),
        other => {
            return Err(BuiltinError::InvalidArgument {
                cmd: "unsafe".into(),
                arg: format!("expected command name, got {other:?}"),
                span: None,
            }
            .into());
        }
    };
    let cmd_args: Vec<Val> = args[1..].to_vec();

    let config = SandboxConfig {
        mode: SandboxMode::Off,
        ..Default::default()
    };
    run_sandboxed(&name, &cmd_args, in_rx, env, tx, &config).map_err(ShellError::from)
}
