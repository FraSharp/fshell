// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use std::sync::Arc;

fn help_text() -> String {
    [
        "self — reference the current running fsh binary",
        "",
        "USAGE:",
        "  self                  print exe path (default)",
        "  self --exe            print exe path",
        "  self --pid            print current pid",
        "  self --version        print fsh version with build datetime (e.g. 0.1.0 20260828-1738)",
        "  self --info           print structured map {exe, pid, version, full_version, build_datetime, profile, argv0}",
        "  self --help           show this help",
        "  self exec [args...]   exec current exe with args (replaces process)",
        "  self --structured     force structured output (map)",
        "  self --json           alias for --structured",
        "",
        "VARIABLES:",
        "  $FSH_EXE               same as `self --exe` — set at shell startup",
        "  $FSH_VERSION           bare semver (e.g. 0.1.0)",
        "  $FSH_FULL_VERSION      version + build datetime (e.g. 0.1.0 20260828-1738)",
        "  $FSH_BUILD_DATETIME    build timestamp compact (YYYYMMDD-HHMM, UTC)",
        "  $FSH_BUILD_DATETIME_ISO  ISO-8601 UTC timestamp",
        "",
        "EXAMPLES:",
        "  self                          # /opt/homebrew/bin/fsh",
        "  self --version                # 0.1.0 20260828-1738",
        "  self --pid                    # 12345",
        "  self --info | map exe         # structured query",
        "  exec (self) -c 'ls | count'   # re-invoke same binary",
        "  self exec --handoff /tmp/h.json  # handoff restart",
    ]
    .join("\n")
}

pub fn self_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    // Normalize args to strings for flag parsing
    let str_args: Vec<String> = args
        .iter()
        .map(|v| match v {
            Val::String(s) => s.clone(),
            other => other.to_text(),
        })
        .collect();

    // Subcommand `exec` takes precedence: `self exec ...` or `self --exec ...`
    // Detect bare "exec" as first positional arg.
    if str_args.first().is_some_and(|s| s == "exec") {
        let exec_args = str_args[1..].to_vec();
        // Strip leading -- if present (self exec -- -c '...' vs self exec -c ...)
        let exec_args = if exec_args.first().is_some_and(|s| s == "--") {
            exec_args[1..].to_vec()
        } else {
            exec_args
        };
        return do_exec(exec_args);
    }

    let mut flag_exe = false;
    let mut flag_pid = false;
    let mut flag_version = false;
    let mut flag_info = false;
    let mut flag_help = false;
    let mut flag_structured = false;
    let mut flag_exec = false;
    let mut exec_args: Vec<String> = Vec::new();
    let mut in_exec_collect = false;

    for a in &str_args {
        if in_exec_collect {
            exec_args.push(a.clone());
            continue;
        }
        match a.as_str() {
            "--exe" => flag_exe = true,
            "--pid" => flag_pid = true,
            "--version" | "-v" => flag_version = true,
            "--info" => flag_info = true,
            "--help" | "-h" => flag_help = true,
            "--structured" | "--json" => flag_structured = true,
            "--exec" => {
                flag_exec = true;
                in_exec_collect = true;
            }
            "--" => {
                // If --exec was seen, -- is separator already handled; otherwise treat as exec separator
                if flag_exec {
                    continue;
                }
                // Bare -- without --exec: treat remaining as exec args if exec mode
                // Otherwise ignore
            }
            s if s.starts_with('-') && !flag_exec => {
                // Unknown flag: surface error (preserves long-term strictness)
                return Err(format!("self: unknown flag '{s}'. Try 'self --help'").into());
            }
            _ => {
                if flag_exec {
                    exec_args.push(a.clone());
                } else {
                    return Err(
                        format!("self: unexpected argument '{a}'. Try 'self --help'").into(),
                    );
                }
            }
        }
    }

    if flag_exec {
        // Flags like --pid/--version are mutually exclusive with --exec
        if flag_pid || flag_version || flag_info || flag_exe {
            return Err("self: --exec cannot be combined with --pid/--version/--info/--exe".into());
        }
        // Strip leading -- leftover
        if exec_args.first().is_some_and(|s| s == "--") {
            exec_args.remove(0);
        }
        return do_exec(exec_args);
    }

    if flag_help {
        let text = help_text();
        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(text))))
                .await;
        });
        return Ok(());
    }

    if flag_info || flag_structured {
        let map = fshell_engine::exe::self_info_map();
        // If specific field flag is also present with --structured, we still emit full map
        // (user asked for structured). Otherwise single-field flags produce scalar, but
        // if structured is requested they get the map.
        let val = Val::Map(map);
        tokio::spawn(async move {
            let _ = tx.send(PipelinePayload::Data(Arc::new(val))).await;
        });
        return Ok(());
    }

    // Single-field selectors (mutually exclusive)
    let selector_count = [flag_exe, flag_pid, flag_version]
        .iter()
        .filter(|&&b| b)
        .count();
    if selector_count > 1 {
        return Err("self: --exe/--pid/--version are mutually exclusive".into());
    }

    if flag_pid {
        let pid_str = fshell_engine::exe::current_pid().to_string();
        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(pid_str))))
                .await;
        });
        return Ok(());
    }
    if flag_version {
        let ver = fshell_engine::exe::full_version();
        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(ver))))
                .await;
        });
        return Ok(());
    }
    if flag_exe || (!flag_pid && !flag_version && !flag_info) {
        // Default: exe path
        // Prefer Env-stored exe_path (canonical, cached) over recomputing.
        let exe = env.exe_path.to_string_lossy().to_string();
        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(exe))))
                .await;
        });
        return Ok(());
    }

    // Fallback (should be unreachable)
    let exe = env.exe_path.to_string_lossy().to_string();
    tokio::spawn(async move {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::String(exe))))
            .await;
    });
    Ok(())
}

fn do_exec(args: Vec<String>) -> Result<(), ShellError> {
    // This replaces the current process. On success it never returns.
    // On failure we surface the OS error as a builtin error.
    match fshell_engine::exe::exec_self(&args) {
        Ok(()) => unreachable!("exec_self should not return on success"),
        Err(e) => Err(format!("self exec: {e}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fshell_engine::PipelinePayload;
    use tokio::sync::mpsc;

    fn test_env() -> Env {
        let env = Env::for_command();
        env
    }

    #[tokio::test]
    async fn test_self_default_is_exe() {
        let env = test_env();
        let (tx, mut rx) = mpsc::channel(10);
        self_builtin(None, vec![], &env, tx).unwrap();
        let payload = rx.recv().await.unwrap();
        match payload {
            PipelinePayload::Data(v) => match v.as_ref() {
                Val::String(s) => assert!(!s.is_empty()),
                other => panic!("expected string, got {other:?}"),
            },
            _ => panic!("expected Data"),
        }
    }

    #[tokio::test]
    async fn test_self_pid() {
        let env = test_env();
        let (tx, mut rx) = mpsc::channel(10);
        self_builtin(None, vec![Val::String("--pid".into())], &env, tx).unwrap();
        let payload = rx.recv().await.unwrap();
        match payload {
            PipelinePayload::Data(v) => match v.as_ref() {
                Val::String(s) => assert!(s.parse::<u32>().is_ok()),
                other => panic!("expected string pid, got {other:?}"),
            },
            _ => panic!("expected Data"),
        }
    }

    #[tokio::test]
    async fn test_self_version() {
        let env = test_env();
        let (tx, mut rx) = mpsc::channel(10);
        self_builtin(None, vec![Val::String("--version".into())], &env, tx).unwrap();
        let payload = rx.recv().await.unwrap();
        match payload {
            PipelinePayload::Data(v) => match v.as_ref() {
                Val::String(s) => assert!(!s.is_empty()),
                other => panic!("expected string version, got {other:?}"),
            },
            _ => panic!("expected Data"),
        }
    }

    #[tokio::test]
    async fn test_self_structured() {
        let env = test_env();
        let (tx, mut rx) = mpsc::channel(10);
        self_builtin(None, vec![Val::String("--structured".into())], &env, tx).unwrap();
        let payload = rx.recv().await.unwrap();
        match payload {
            PipelinePayload::Data(v) => match v.as_ref() {
                Val::Map(m) => {
                    assert!(m.contains_key(&ustr::ustr("exe")));
                    assert!(m.contains_key(&ustr::ustr("pid")));
                    assert!(m.contains_key(&ustr::ustr("version")));
                }
                other => panic!("expected map, got {other:?}"),
            },
            _ => panic!("expected Data"),
        }
    }

    #[tokio::test]
    async fn test_self_help() {
        let env = test_env();
        let (tx, mut rx) = mpsc::channel(10);
        self_builtin(None, vec![Val::String("--help".into())], &env, tx).unwrap();
        let payload = rx.recv().await.unwrap();
        match payload {
            PipelinePayload::Data(v) => match v.as_ref() {
                Val::String(s) => assert!(s.contains("USAGE")),
                other => panic!("expected string help, got {other:?}"),
            },
            _ => panic!("expected Data"),
        }
    }

    #[tokio::test]
    async fn test_self_unknown_flag_errors() {
        let env = test_env();
        let (tx, _rx) = mpsc::channel(10);
        let err = self_builtin(None, vec![Val::String("--bogus".into())], &env, tx).unwrap_err();
        assert!(err.message.contains("unknown flag"));
    }
}
