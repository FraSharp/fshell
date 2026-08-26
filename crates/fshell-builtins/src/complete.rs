// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_core::diagnostic::ErrorCode;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;
use std::sync::Arc;

pub fn complete_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let mut command_name = None;
    let mut list_all = false;
    let mut erase = false;
    let mut context_subcmds = Vec::new();
    let mut short_flag = None;
    let mut long_flag = None;
    let mut desc = None;
    let mut arguments = None;
    let mut _wrap_mode = false;

    let mut i = 0;
    while i < args.len() {
        let arg_str = match &args[i] {
            Val::String(s) => s.as_str(),
            _ => {
                return Err(BuiltinError::InvalidArgument {
                    cmd: "complete".into(),
                    arg: "arguments must be strings".into(),
                    span,
                }
                .into());
            }
        };
        match arg_str {
            "-l" | "--list" => {
                list_all = true;
                i += 1;
            }
            "-e" | "--erase" => {
                erase = true;
                i += 1;
            }
            "-w" | "--wordlist" => {
                _wrap_mode = true;
                i += 1;
            }
            "-c" | "--context" => {
                if i + 1 >= args.len() {
                    return Err(ShellError::new(
                        ErrorCode::MissingArgument,
                        "Missing value for context option",
                    )
                    .maybe_with_span(span));
                }
                let ctx_val = match &args[i + 1] {
                    Val::String(s) => s,
                    _ => {
                        return Err(ShellError::new(
                            ErrorCode::InvalidArgument,
                            "Context must be a string",
                        )
                        .maybe_with_span(span));
                    }
                };
                context_subcmds = ctx_val.split_whitespace().map(|s| s.to_string()).collect();
                i += 2;
            }
            "-s" | "--short" => {
                if i + 1 >= args.len() {
                    return Err(ShellError::new(
                        ErrorCode::MissingArgument,
                        "Missing value for short option",
                    )
                    .maybe_with_span(span));
                }
                let s_val = match &args[i + 1] {
                    Val::String(s) => s,
                    _ => {
                        return Err(ShellError::new(
                            ErrorCode::InvalidArgument,
                            "Short option must be a string",
                        )
                        .maybe_with_span(span));
                    }
                };
                short_flag = Some(s_val.clone());
                i += 2;
            }
            "--long" => {
                if i + 1 >= args.len() {
                    return Err(ShellError::new(
                        ErrorCode::MissingArgument,
                        "Missing value for --long option",
                    )
                    .maybe_with_span(span));
                }
                let l_val = match &args[i + 1] {
                    Val::String(s) => s,
                    _ => {
                        return Err(ShellError::new(
                            ErrorCode::InvalidArgument,
                            "Long option must be a string",
                        )
                        .maybe_with_span(span));
                    }
                };
                long_flag = Some(l_val.clone());
                i += 2;
            }
            "-d" | "--desc" | "--description" => {
                if i + 1 >= args.len() {
                    return Err(ShellError::new(
                        ErrorCode::MissingArgument,
                        "Missing value for description option",
                    )
                    .maybe_with_span(span));
                }
                let d_val = match &args[i + 1] {
                    Val::String(s) => s,
                    _ => {
                        return Err(ShellError::new(
                            ErrorCode::InvalidArgument,
                            "Description must be a string",
                        )
                        .maybe_with_span(span));
                    }
                };
                desc = Some(d_val.clone());
                i += 2;
            }
            "-a" | "--arguments" | "-W" => {
                if i + 1 >= args.len() {
                    return Err(ShellError::new(
                        ErrorCode::MissingArgument,
                        "Missing value for arguments option",
                    )
                    .maybe_with_span(span));
                }
                let a_val = match &args[i + 1] {
                    Val::String(s) => s,
                    _ => {
                        return Err(ShellError::new(
                            ErrorCode::InvalidArgument,
                            "Arguments must be a string",
                        )
                        .maybe_with_span(span));
                    }
                };
                arguments = Some(a_val.clone());
                i += 2;
            }
            "-F" => {
                if i + 1 >= args.len() {
                    return Err(ShellError::new(
                        ErrorCode::MissingArgument,
                        "Missing value for -F option",
                    )
                    .maybe_with_span(span));
                }
                let f_val = match &args[i + 1] {
                    Val::String(s) => s,
                    _ => {
                        return Err(ShellError::new(
                            ErrorCode::InvalidArgument,
                            "-F argument must be a string",
                        )
                        .maybe_with_span(span));
                    }
                };
                arguments = Some(format!("fn:{}", f_val));
                i += 2;
            }
            "-C" => {
                if i + 1 >= args.len() {
                    return Err(ShellError::new(
                        ErrorCode::MissingArgument,
                        "Missing value for -C option",
                    )
                    .maybe_with_span(span));
                }
                let c_val = match &args[i + 1] {
                    Val::String(s) => s,
                    _ => {
                        return Err(ShellError::new(
                            ErrorCode::InvalidArgument,
                            "-C argument must be a string",
                        )
                        .maybe_with_span(span));
                    }
                };
                arguments = Some(c_val.clone());
                i += 2;
            }
            "-f" => {
                arguments = Some("files".to_string());
                i += 1;
            }
            "--dirs" => {
                arguments = Some("dirs".to_string());
                i += 1;
            }
            other => {
                if other.starts_with('-') {
                    // Note: `-l` cannot be used here to declare a long option to
                    // complete — it is consumed above as `--list`. Long-option
                    // completion specs are therefore not expressible yet.
                    return Err(BuiltinError::InvalidArgument {
                        cmd: "complete".into(),
                        arg: format!("unknown option: {other}"),
                        span,
                    }
                    .into());
                } else {
                    if command_name.is_none() {
                        command_name = Some(other.to_string());
                    } else {
                        return Err(BuiltinError::UnexpectedArgument {
                            cmd: "complete".into(),
                            arg: other.to_string(),
                            span,
                        }
                        .into());
                    }
                    i += 1;
                }
            }
        }
    }

    if list_all || args.is_empty() {
        let reg = env.completions.read();
        let mut out = String::new();
        for (cmd, comp) in reg.iter() {
            out.push_str(&format!("completions for {}:\n", cmd));
            for sub in &comp.subcommands {
                out.push_str(&format!(
                    "  subcommand: {} (context: {:?}) - {:?}\n",
                    sub.name, sub.parent_subcmds, sub.desc
                ));
            }
            for flag in &comp.flags {
                out.push_str(&format!(
                    "  flag: {:?}/{:?} (context: {:?}) - {:?} (choices: {:?})\n",
                    flag.short, flag.long, flag.parent_subcmds, flag.desc, flag.choices
                ));
            }
            for prov in &comp.dynamic_providers {
                out.push_str(&format!(
                    "  dynamic provider: {} (context: {:?})\n",
                    prov.command, prov.parent_subcmds
                ));
            }
        }
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            let _ = tx_clone
                .send(PipelinePayload::Data(Arc::new(Val::String(out))))
                .await;
        });
        return Ok(());
    }

    if erase {
        if let Some(cmd) = command_name {
            {
                let mut reg = env.completions.write();
                reg.remove(&cmd);
            }
            fshell_engine::save_completions(env).map_err(ShellError::from)?;
            return Ok(());
        } else {
            return Err(ShellError::new(
                ErrorCode::MissingArgument,
                "Missing command name to erase",
            )
            .maybe_with_span(span));
        }
    }

    let cmd = command_name.ok_or_else(|| "Missing command name".to_string())?;
    let mut reg = env.completions.write();
    let comp = reg.entry(cmd.clone()).or_default();

    if let Some(args_val) = arguments {
        if short_flag.is_none() && long_flag.is_none() {
            comp.dynamic_providers.push(fshell_core::DynamicProvider {
                parent_subcmds: context_subcmds,
                command: args_val,
                cache_ms: None,
            });
        } else {
            comp.flags.push(fshell_core::FlagCompletion {
                parent_subcmds: context_subcmds,
                short: short_flag,
                long: long_flag,
                desc,
                choices: Some(args_val),
            });
        }
    } else if short_flag.is_some() || long_flag.is_some() {
        comp.flags.push(fshell_core::FlagCompletion {
            parent_subcmds: context_subcmds,
            short: short_flag,
            long: long_flag,
            desc,
            choices: None,
        });
    } else if !context_subcmds.is_empty() {
        let mut parents = context_subcmds;
        if let Some(sub_name) = parents.pop() {
            comp.subcommands.push(fshell_core::SubcmdCompletion {
                parent_subcmds: parents,
                name: sub_name,
                desc,
            });
        }
    }

    drop(reg);
    fshell_engine::save_completions(env).map_err(ShellError::from)?;

    Ok(())
}

/// `compgen` builtin: generate completion candidates based on options and prefix.
pub fn compgen_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let arg_strs: Vec<String> = args
        .iter()
        .map(|v| match v {
            Val::String(s) => s.clone(),
            other => other.to_text(),
        })
        .collect();

    let mut actions = Vec::new();
    let mut wordlist = None;
    let mut prefix = "";

    let mut i = 0;
    while i < arg_strs.len() {
        let arg = &arg_strs[i];
        match arg.as_str() {
            "-f" => actions.push("file"),
            "-d" => actions.push("dir"),
            "-c" => actions.push("command"),
            "-v" => actions.push("variable"),
            "-a" => actions.push("alias"),
            "-b" => actions.push("builtin"),
            "-k" => actions.push("keyword"),
            "-W" => {
                i += 1;
                if i < arg_strs.len() {
                    wordlist = Some(&arg_strs[i]);
                }
            }
            other if !other.starts_with('-') => {
                prefix = other;
            }
            _ => {}
        }
        i += 1;
    }

    let mut candidates = Vec::new();

    // 1. Wordlist candidates
    if let Some(wl) = wordlist {
        for word in wl.split_whitespace() {
            if word.starts_with(prefix) {
                candidates.push(word.to_string());
            }
        }
    }

    // 2. Variables
    if actions.contains(&"variable") {
        for k in env.vars.read().keys() {
            if k.starts_with(prefix) {
                candidates.push(k.clone());
            }
        }
    }

    // 3. Aliases
    if actions.contains(&"alias") {
        for k in env.aliases.read().keys() {
            if k.starts_with(prefix) {
                candidates.push(k.clone());
            }
        }
    }

    // 4. Builtins & Commands
    if actions.contains(&"builtin") || actions.contains(&"command") {
        for k in env.builtins.read().keys() {
            if k.starts_with(prefix) {
                candidates.push(k.clone());
            }
        }
    }

    // 5. Files / Dirs
    if actions.contains(&"file") || actions.contains(&"dir") {
        let only_dirs = actions.contains(&"dir") && !actions.contains(&"file");
        let search_dir = if prefix.contains('/') {
            let path = std::path::Path::new(prefix);
            if let Some(parent) = path.parent() {
                if parent.as_os_str().is_empty() {
                    env.cwd()
                } else if parent.is_absolute() {
                    parent.to_path_buf()
                } else {
                    env.cwd().join(parent)
                }
            } else {
                env.cwd()
            }
        } else {
            env.cwd()
        };

        if let Ok(entries) = std::fs::read_dir(&search_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let full_str = if prefix.contains('/') {
                    let parent_str = prefix
                        .rsplit_once('/')
                        .map(|(p, _)| format!("{}/", p))
                        .unwrap_or_default();
                    format!("{}{}", parent_str, name)
                } else {
                    name
                };
                if full_str.starts_with(prefix) {
                    if only_dirs {
                        if entry.path().is_dir() {
                            candidates.push(format!("{}/", full_str));
                        }
                    } else {
                        candidates.push(full_str);
                    }
                }
            }
        }
    }

    // 6. Output candidates
    candidates.sort();
    candidates.dedup();

    let tx_clone = tx.clone();
    tokio::spawn(async move {
        for c in candidates {
            let _ = tx_clone
                .send(PipelinePayload::Data(Arc::new(Val::String(c))))
                .await;
        }
    });

    Ok(())
}
