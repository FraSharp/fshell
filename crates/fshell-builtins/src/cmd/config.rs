// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
#[cfg(feature = "config-tui")]
use fshell_config_tui;
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_core::diagnostic::ErrorCode;
use fshell_engine::{CapAction, Env, PipeSender, PipeStream, PipelinePayload, ShellOptions};
use nu_ansi_term::Color;
use std::sync::Arc;

fn setopt_impl(args: &[Val], env: &Env, value: bool) -> Result<(), ShellError> {
    for arg in args {
        let name = match arg {
            Val::String(s) => s.as_str(),
            _ => {
                return Err(BuiltinError::InvalidArgument {
                    cmd: "setopt".into(),
                    arg: format!("{:?}", arg),
                    span: None,
                }
                .into());
            }
        };
        apply_option(env, name, &Val::Bool(value))?;
    }
    Ok(())
}

pub fn setopt_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    if args.is_empty() {
        let opts = env.options.read();
        let mut out = String::new();
        out.push_str(&format!(
            "autocd      {}\n",
            if opts.autocd { "on" } else { "off" }
        ));
        out.push_str(&format!(
            "pipefail    {}\n",
            if opts.pipefail { "on" } else { "off" }
        ));
        out.push_str(&format!(
            "notify      {}\n",
            if opts.notify { "on" } else { "off" }
        ));
        out.push_str(&format!(
            "json_auto_parse {}\n",
            if opts.json_auto_parse { "on" } else { "off" }
        ));
        out.push_str(&format!(
            "did_you_mean {}\n",
            if opts.did_you_mean { "on" } else { "off" }
        ));
        out.push_str(&format!(
            "confirm_destructive {}\n",
            if opts.confirm_destructive {
                "on"
            } else {
                "off"
            }
        ));
        out.push_str(&format!(
            "sandbox_all {}\n",
            if opts.sandbox_all { "on" } else { "off" }
        ));
        out.push_str(&format!("sandbox_mode {}", opts.sandbox_mode));
        out.push_str(&format!(
            "\npipeline_channel_size {}",
            opts.pipeline_channel_size
        ));
        out.push_str(&format!("\nclear_on_reload {}", opts.clear_on_reload));
        let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(out))));
        return Ok(());
    }

    setopt_impl(&args, env, true)
}

pub fn unsetopt_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _tx: PipeSender,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return Err(ShellError::new(
            ErrorCode::MissingArgument,
            "unsetopt: missing option names",
        ));
    }
    setopt_impl(&args, env, false)
}

pub fn unset_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _tx: PipeSender,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return Err(ShellError::new(
            ErrorCode::InvalidArgument,
            "unset: expected variable or function name",
        ));
    }

    env.ensure_env_populated();

    let mut unset_fns_only = false;
    let mut names = Vec::new();

    for arg in args {
        match arg {
            Val::String(ref s) if s == "-f" => {
                unset_fns_only = true;
            }
            Val::String(ref s) if s == "-v" => {
                unset_fns_only = false;
            }
            Val::String(ref name) => {
                names.push(name.clone());
            }
            _ => {
                return Err(BuiltinError::InvalidArgument {
                    cmd: "unset".into(),
                    arg: format!("{:?}", arg),
                    span: None,
                }
                .into());
            }
        }
    }

    for name in names {
        if unset_fns_only {
            let mut fns = env.fns.write();
            fns.remove(&name);
        } else {
            env.enforce_capability("unset", CapAction::WriteEnv(name.clone()))?;
            env.unset_var(&name);
            // Also remove any function with same name (bash parity: unset without -f removes both)
            let mut fns = env.fns.write();
            fns.remove(&name);
        }
    }
    Ok(())
}

pub fn set_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return set_list(env, tx);
    }

    if let Val::String(first) = &args[0]
        && (first.starts_with('-') || first.starts_with('+'))
    {
        return set_flags_or_positional(&args, env, tx);
    }

    match args.len() {
        1 => {
            let key = match &args[0] {
                Val::String(s) => s.as_str(),
                _ => {
                    return Err(ShellError::new(
                        ErrorCode::InvalidArgument,
                        "set: key must be a string",
                    ));
                }
            };
            set_show(key, env, tx)
        }
        _ => {
            let key = match &args[0] {
                Val::String(s) => s.as_str(),
                _ => {
                    return Err(ShellError::new(
                        ErrorCode::InvalidArgument,
                        "set: key must be a string",
                    ));
                }
            };
            set_apply(key, &args[1..], env)
        }
    }
}

fn set_flags_or_positional(args: &[Val], env: &Env, tx: PipeSender) -> Result<(), ShellError> {
    let mut i = 0;
    let mut positional = Vec::new();
    let mut has_positional_specifier = false;

    while i < args.len() {
        let s = match &args[i] {
            Val::String(s) => s.as_str(),
            other => {
                positional.push(other.clone());
                i += 1;
                continue;
            }
        };

        if s == "--" {
            has_positional_specifier = true;
            for arg in &args[i + 1..] {
                positional.push(arg.clone());
            }
            break;
        } else if s == "-o" || s == "+o" {
            let is_set = s.starts_with('-');
            if i + 1 < args.len()
                && let Val::String(opt_name) = &args[i + 1]
            {
                let mut opts = env.options.write();
                opts.set_bool(opt_name, is_set)
                    .map_err(|e| format!("set: {e}"))?;
                i += 2;
                continue;
            }
            let opts = env.options.read();
            let mut out = String::new();
            opts.for_each_bool(|name, val| {
                out.push_str(&format!("{name:20} {}\n", if *val { "on" } else { "off" }));
            });
            let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(out))));
            return Ok(());
        } else if s.starts_with('-') || s.starts_with('+') {
            let is_set = s.starts_with('-');
            let chars: Vec<char> = s[1..].chars().collect();
            let mut opts = env.options.write();

            for c in chars {
                match c {
                    'e' => opts.errexit = is_set,
                    'x' => opts.xtrace = is_set,
                    'u' => opts.nounset = is_set,
                    'v' => opts.verbose = is_set,
                    'n' => opts.noexec = is_set,
                    'C' => opts.noclobber = is_set,
                    'f' => opts.nullglob = is_set,
                    _ => {
                        return Err(ShellError::invalid_argument(
                            "set",
                            &format!("-{c}"),
                            None,
                        )
                        .with_help("Valid options: -e (errexit) -x (xtrace) -u (nounset) -v (verbose) -n (noexec) -C (noclobber) -f (nullglob)"));
                    }
                }
            }
            i += 1;
        } else {
            positional.push(args[i].clone());
            i += 1;
        }
    }

    if has_positional_specifier || !positional.is_empty() {
        let mut vars = env.vars.write();
        let mut k = 1;
        while vars.remove(&k.to_string()).is_some() {
            k += 1;
        }
        for (idx, val) in positional.iter().enumerate() {
            vars.insert((idx + 1).to_string(), val.clone());
        }
        vars.insert("#".to_string(), Val::Int(positional.len() as i64));
        vars.insert("@".to_string(), Val::List(positional.clone()));
        vars.insert("*".to_string(), Val::List(positional));
    }

    Ok(())
}

fn set_list(env: &Env, tx: PipeSender) -> Result<(), ShellError> {
    let vars = env.vars.read();
    let opts = env.options.read();

    let mut out = String::new();
    opts.for_each_bool(|name, val| {
        out.push_str(&format!(
            "{name:20} {}\n",
            if *val { "true" } else { "false" }
        ));
    });
    out.push_str(&format!("sandbox_mode        {}\n", opts.sandbox_mode));
    out.push_str(&format!(
        "pipeline_channel_size {}\n",
        opts.pipeline_channel_size
    ));
    out.push_str(&format!("clear_on_reload     {}\n", opts.clear_on_reload));
    out.push_str(&format!("session_restore     {}\n", opts.session_restore));
    out.push_str(&format!("theme               {}\n", opts.theme));
    let disabled_str = opts.disabled_builtins.join(", ");
    out.push_str(&format!("disabled_builtins   [{}]\n", disabled_str));
    let binaries_str = opts
        .command_binaries
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("command_binaries    {{{}}}\n", binaries_str));

    let p = |k: &str, d: &str| -> String {
        match vars.get(k) {
            Some(Val::String(s)) => format!("{s:?}"),
            _ => d.to_string(),
        }
    };
    out.push_str(&format!(
        "prompt              {}\n",
        p("FSH_PROMPT", "(default)")
    ));
    out.push_str(&format!(
        "prompt_right        {}\n",
        p("FSH_PROMPT_RIGHT", "(none)")
    ));
    out.push_str(&format!(
        "keybinding          {}\n",
        p("FSH_KEYBINDING_MODE", "emacs")
    ));
    drop(vars);
    drop(opts);

    let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(out))));
    Ok(())
}

pub fn get_option_value(env: &Env, key: &str) -> Result<Val, ShellError> {
    let bare_key = key.strip_prefix("options.").unwrap_or(key);
    let opts = env.options.read();

    if let Some(b) = opts.get_bool(bare_key) {
        return Ok(Val::Bool(b));
    }

    match bare_key {
        "sandbox_mode" => Ok(Val::String(opts.sandbox_mode.clone())),
        "clear_on_reload" => Ok(Val::String(opts.clear_on_reload.clone())),
        "session_restore" => Ok(Val::String(opts.session_restore.clone())),
        "theme" => Ok(Val::String(opts.theme.clone())),
        "disabled_builtins" => Ok(Val::List(
            opts.disabled_builtins
                .iter()
                .map(|s| Val::String(s.clone()))
                .collect(),
        )),
        "command_binaries" => {
            let mut m = fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            for (k, v) in &opts.command_binaries {
                m.insert(ustr::ustr(k), Val::String(v.clone()));
            }
            Ok(Val::Map(m))
        }
        "pipeline_channel_size" => Ok(Val::Int(opts.pipeline_channel_size as i64)),
        "prompt" | "prompt_right" | "keybinding" => {
            drop(opts);
            let vars = env.vars.read();
            let var_name = match bare_key {
                "prompt" => "FSH_PROMPT",
                "prompt_right" => "FSH_PROMPT_RIGHT",
                "keybinding" => "FSH_KEYBINDING_MODE",
                _ => unreachable!(),
            };
            Ok(vars.get(var_name).cloned().unwrap_or(match bare_key {
                "keybinding" => Val::String("emacs".into()),
                _ => Val::String("(default)".into()),
            }))
        }
        _ => Err(BuiltinError::InvalidArgument {
            cmd: "config".into(),
            arg: format!("unknown key '{key}'"),
            span: None,
        }
        .into()),
    }
}

fn set_show(key: &str, env: &Env, tx: PipeSender) -> Result<(), ShellError> {
    let val = get_option_value(env, key)?;
    let _ = tx.try_send(PipelinePayload::Data(Arc::new(val)));
    Ok(())
}

fn set_apply(key: &str, vals: &[Val], env: &Env) -> Result<(), ShellError> {
    if vals.is_empty() {
        return Err(ShellError::new(
            ErrorCode::MissingArgument,
            "set: missing value",
        ));
    }
    apply_option(env, key, &vals[0])
}

pub fn apply_option(env: &Env, key: &str, val: &Val) -> Result<(), ShellError> {
    let bare_key = key.strip_prefix("options.").unwrap_or(key);
    if ShellOptions::bool_keys().contains(&bare_key) {
        let b = parse_bool(val)?;
        let mut opts = env.options.write();
        opts.set_bool(bare_key, b)
            .map_err(|e| format!("set: {e}"))?;
    } else {
        match bare_key {
            "sandbox_mode" => {
                let s = match val {
                    Val::String(s) => s.clone(),
                    _ => return Err("set: sandbox_mode requires a string value".into()),
                };
                if !matches!(s.as_str(), "prompt" | "deny-all" | "monitor" | "off") {
                    return Err(
                        "set: sandbox_mode must be 'prompt', 'deny-all', 'monitor', or 'off'"
                            .into(),
                    );
                }
                let mut opts = env.options.write();
                opts.sandbox_mode = s;
            }
            "clear_on_reload" => {
                let s = match val {
                    Val::String(s) => s.clone(),
                    _ => return Err("set: clear_on_reload requires a string value".into()),
                };
                if !matches!(s.as_str(), "ask" | "always" | "never") {
                    return Err(ShellError::new(
                        ErrorCode::InvalidArgument,
                        "set: clear_on_reload must be 'ask', 'always', or 'never'",
                    ));
                }
                let mut opts = env.options.write();
                opts.clear_on_reload = s;
            }
            "session_restore" => {
                let s = match val {
                    Val::String(s) => s.clone(),
                    _ => return Err("set: session_restore requires a string value".into()),
                };
                if !matches!(s.as_str(), "none" | "auto" | "picker" | "ask") {
                    return Err(
                        "set: session_restore must be 'none', 'auto', 'picker', or 'ask'".into(),
                    );
                }
                let mut opts = env.options.write();
                opts.session_restore = s;
            }
            "theme" => {
                let s = match val {
                    Val::String(s) => s.clone(),
                    _ => return Err("set: theme requires a string value".into()),
                };
                let config_dir =
                    fshell_engine::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
                let theme =
                    fshell_core::theme::Theme::load(&s, &config_dir).map_err(|e| e.to_string())?;

                env.set_theme(Arc::new(theme));

                let mut opts = env.options.write();
                opts.theme = s;
            }
            "disabled_builtins" => {
                let mut list = Vec::new();
                match val {
                    Val::List(l) => {
                        for item in l {
                            if let Val::String(s) = item {
                                list.push(s.clone());
                            } else {
                                return Err(
                                    "set: disabled_builtins list items must be strings".into()
                                );
                            }
                        }
                    }
                    Val::String(s) => {
                        for part in s.split(',') {
                            let trimmed = part.trim();
                            if !trimmed.is_empty() {
                                list.push(trimmed.to_string());
                            }
                        }
                    }
                    _ => {
                        return Err(
                            "set: expected a list or comma-separated string for disabled_builtins"
                                .into(),
                        );
                    }
                }
                let mut opts = env.options.write();
                opts.disabled_builtins = list;
                env.invalidate_builtins_cache();
            }
            "command_binaries" => {
                let mut map = std::collections::HashMap::new();
                match val {
                    Val::Map(m) => {
                        for (k, v) in m {
                            if let Val::String(val_str) = v {
                                map.insert(k.to_string(), val_str.clone());
                            } else {
                                return Err(
                                    "set: command_binaries map values must be strings".into()
                                );
                            }
                        }
                    }
                    _ => {
                        return Err(ShellError::new(
                            ErrorCode::InvalidArgument,
                            "set: expected a map for command_binaries",
                        ));
                    }
                }
                let mut opts = env.options.write();
                opts.command_binaries = map;
            }
            "pipeline_channel_size" => {
                let v = parse_int(val)?;
                if v <= 0 {
                    return Err(ShellError::new(
                        ErrorCode::InvalidArgument,
                        "set: pipeline_channel_size must be positive",
                    ));
                }
                let mut opts = env.options.write();
                opts.pipeline_channel_size = v as usize;
            }
            "prompt" | "prompt_right" | "keybinding" => {
                let s = match val {
                    Val::String(s) => s.clone(),
                    _ => {
                        return Err(BuiltinError::InvalidArgument {
                            cmd: "set".into(),
                            arg: format!("{bare_key} requires a string value"),
                            span: None,
                        }
                        .into());
                    }
                };
                if bare_key == "keybinding" && !matches!(s.as_str(), "emacs" | "vi") {
                    return Err(ShellError::new(
                        ErrorCode::InvalidArgument,
                        "set: keybinding must be 'emacs' or 'vi'",
                    ));
                }
                let var_name = match bare_key {
                    "prompt" => "FSH_PROMPT",
                    "prompt_right" => "FSH_PROMPT_RIGHT",
                    "keybinding" => "FSH_KEYBINDING_MODE",
                    _ => unreachable!(),
                };
                let mut vars = env.vars.write();
                vars.insert(var_name.into(), Val::String(s));
            }
            _ => {
                return Err(BuiltinError::InvalidArgument {
                    cmd: "set".into(),
                    arg: format!("unknown key '{key}'"),
                    span: None,
                }
                .into());
            }
        }
    }

    env.sync_options_map()
        .map_err(|e| format!("Lock poisoned during option sync: {e}"))?;

    if !env
        .is_loading_init_script
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        persist_settings(env)?;
    }

    Ok(())
}

fn parse_bool(v: &Val) -> Result<bool, ShellError> {
    match v {
        Val::Bool(b) => Ok(*b),
        Val::String(s) => match s.as_str() {
            "true" | "on" | "yes" | "1" => Ok(true),
            "false" | "off" | "no" | "0" => Ok(false),
            _ => Err(ShellError::invalid_argument("set", s, None)
                .with_help("Expected true/false/on/off/yes/no/1/0")),
        },
        _ => Err(ShellError::new(
            ErrorCode::InvalidArgument,
            "set: expected a boolean or string",
        )),
    }
}

fn parse_int(v: &Val) -> Result<i64, ShellError> {
    match v {
        Val::Int(i) => Ok(*i),
        Val::String(s) => s
            .parse::<i64>()
            .map_err(|_| {
                ShellError::invalid_argument("set", s, None).with_help("Expected an integer value")
            }),
        _ => Err(ShellError::new(
            ErrorCode::InvalidArgument,
            "set: expected an integer or string",
        )),
    }
}

pub(crate) fn persist_settings(env: &Env) -> Result<(), ShellError> {
    let opts = env.options.read();
    let vars = env.vars.read();

    let prompt = match vars.get("FSH_PROMPT") {
        Some(Val::String(s)) => s.clone(),
        _ => String::new(),
    };
    let prompt_right = match vars.get("FSH_PROMPT_RIGHT") {
        Some(Val::String(s)) => s.clone(),
        _ => String::new(),
    };
    let keybinding = match vars.get("FSH_KEYBINDING_MODE") {
        Some(Val::String(s)) => s.clone(),
        _ => String::new(),
    };
    drop(vars);

    let snapshot = fshell_engine::config::SettingsSnapshot {
        autocd: opts.autocd,
        pipefail: opts.pipefail,
        notify: opts.notify,
        json_auto_parse: opts.json_auto_parse,
        did_you_mean: opts.did_you_mean,
        sandbox_mode: opts.sandbox_mode.clone(),
        pipeline_channel_size: opts.pipeline_channel_size,
        prompt,
        prompt_right,
        keybinding,
        errexit: opts.errexit,
        nounset: opts.nounset,
        nullglob: opts.nullglob,
        nocaseglob: opts.nocaseglob,
        noclobber: opts.noclobber,
        noexec: opts.noexec,
        xtrace: opts.xtrace,
        verbose: opts.verbose,
        ignoreeof: opts.ignoreeof,
        autopushd: opts.autopushd,
        histignoredups: opts.histignoredups,
        cdable_vars: opts.cdable_vars,
        quiet_aliases: opts.quiet_aliases,
        clear_on_reload: opts.clear_on_reload.clone(),
        session_restore: opts.session_restore.clone(),
        theme: opts.theme.clone(),
        disabled_builtins: opts.disabled_builtins.clone(),
        command_binaries: opts.command_binaries.clone(),
        confirm_destructive: opts.confirm_destructive,
        sandbox_all: opts.sandbox_all,
    };
    drop(opts);

    let lines = fshell_engine::config::collect_settings_lines(&snapshot);
    fshell_engine::config::update_managed_settings(&lines)
        .map_err(|e| ShellError::io_error(format!("Failed to persist settings: {e}"), None))
}

fn config_list(env: &Env, tx: PipeSender) -> Result<(), ShellError> {
    let vars = env.vars.read();
    let opts = env.options.read();

    let mut out = String::new();
    out.push_str("[options]\n");
    opts.for_each_bool(|name, val| {
        out.push_str(&format!("{name} = {val}\n"));
    });
    out.push_str(&format!("sandbox_mode = {}\n", opts.sandbox_mode));
    out.push_str(&format!(
        "pipeline_channel_size = {}\n",
        opts.pipeline_channel_size
    ));
    out.push_str(&format!("clear_on_reload = {}\n", opts.clear_on_reload));
    out.push_str(&format!("session_restore = {}\n", opts.session_restore));
    out.push_str(&format!("theme = {}\n", opts.theme));
    let disabled_str = opts.disabled_builtins.join(", ");
    out.push_str(&format!("disabled_builtins = [{}]\n", disabled_str));
    let binaries_str = opts
        .command_binaries
        .iter()
        .map(|(k, v)| format!("{k} = {v}"))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("command_binaries = {{{}}}\n", binaries_str));
    out.push('\n');

    let prompt_str = match vars.get("FSH_PROMPT") {
        Some(Val::String(s)) => s.clone(),
        _ => "(default)".to_string(),
    };
    out.push_str(&format!("prompt = {prompt_str}\n"));

    let prompt_right_str = match vars.get("FSH_PROMPT_RIGHT") {
        Some(Val::String(s)) => s.clone(),
        _ => "(default)".to_string(),
    };
    out.push_str(&format!("prompt_right = {prompt_right_str}\n"));

    let keybinding_str = match vars.get("FSH_KEYBINDING_MODE") {
        Some(Val::String(s)) => s.clone(),
        _ => "emacs".to_string(),
    };
    out.push_str(&format!("keybinding = {keybinding_str}\n"));

    let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(out))));
    Ok(())
}

fn config_get(env: &Env, key: &str, tx: PipeSender) -> Result<(), ShellError> {
    let val = get_option_value(env, key)?;
    let _ = tx.try_send(PipelinePayload::Data(Arc::new(val)));
    Ok(())
}

pub fn config_set(env: &Env, key: &str, val: &Val) -> Result<(), ShellError> {
    apply_option(env, key, val)
}

fn config_reload_sync(env: &Env) -> Result<(), ShellError> {
    fshell_core::debug_log!("config_reload_sync: resetting to defaults");
    {
        let mut opts = env.options.write();
        *opts = ShellOptions::default();
    }
    {
        let mut vars = env.vars.write();
        vars.remove("FSH_PROMPT");
        vars.remove("FSH_PROMPT_RIGHT");
        vars.remove("FSH_KEYBINDING_MODE");
    }
    env.sync_options_map()
        .map_err(|e| format!("Failed to sync options: {e}"))?;

    fshell_core::debug_log!("config_reload_sync: re-sourcing init.fsh");
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(fshell_engine::load_config_script(env))
    })
    .map_err(|e| format!("Failed to reload init.fsh: {e}"))?;

    fshell_core::debug_log!("config_reload_sync: done");

    Ok(())
}

pub fn config_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return config_list(env, tx);
    }
    let cmd = match &args[0] {
        Val::String(s) => s.as_str(),
        _ => {
            return Err("config: subcommand must be a string (list/get/set/reload)"
                .to_string()
                .into());
        }
    };
    match cmd {
        "tui" => {
            #[cfg(feature = "config-tui")]
            {
                fshell_config_tui::run_config_tui(env).map_err(|e| e.to_string())?;
                Ok(())
            }
            #[cfg(not(feature = "config-tui"))]
            {
                Err(
                    "config tui: not available (compile with config-tui feature)"
                        .to_string()
                        .into(),
                )
            }
        }
        "list" => config_list(env, tx),
        "get" => {
            if args.len() < 2 {
                return Err(ShellError::new(
                    ErrorCode::MissingArgument,
                    "config get: missing key",
                ));
            }
            let key = match &args[1] {
                Val::String(s) => s.as_str(),
                _ => {
                    return Err(ShellError::new(
                        ErrorCode::InvalidArgument,
                        "config get: key must be a string",
                    ));
                }
            };
            config_get(env, key, tx)
        }
        "set" => {
            if args.len() < 3 {
                return Err(ShellError::new(
                    ErrorCode::MissingArgument,
                    "config set: missing key or value",
                ));
            }
            let key = match &args[1] {
                Val::String(s) => s.as_str(),
                _ => {
                    return Err(ShellError::new(
                        ErrorCode::InvalidArgument,
                        "config set: key must be a string",
                    ));
                }
            };
            let val = &args[2];
            config_set(env, key, val)
        }
        "reload" => config_reload_sync(env),
        _ => {
            Err(ShellError::invalid_argument("config", cmd, None)
                .with_help("Expected one of: list/get/set/reload"))
        }
    }
}

pub fn alias_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    let str_args: Vec<String> = args
        .iter()
        .filter_map(|a| {
            if let Val::String(s) = a {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect();

    if str_args.is_empty() {
        let all = env.get_all_aliases();
        if all.is_empty() {
            println!("No aliases defined.");
        } else {
            for (name, expansion) in &all {
                let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(format!(
                    "alias {} = {}",
                    name,
                    fshell_engine::config::fshell_quote(expansion)
                )))));
            }
        }
        drop(tx);
        return Ok(());
    }

    if str_args[0] == "--delete" || str_args[0] == "-d" {
        if str_args.len() < 2 {
            return Err("alias --delete requires a name".to_string().into());
        }
        let name = &str_args[1];
        let removed = env.remove_alias(name);
        if removed.is_none() {
            eprintln!("\x1b[1;33mNotice:\x1b[0m alias '{}' was not defined.", name);
        } else {
            if let Err(e) = fshell_engine::config::remove_alias(name) {
                eprintln!("\x1b[1;31mWarning:\x1b[0m could not update init.fsh: {}", e);
            } else {
                println!("alias '{}' removed.", name);
            }
        }
        drop(tx);
        return Ok(());
    }

    // Single argument without `=`: query existing alias (e.g. `alias ll`)
    if str_args.len() == 1 && !str_args[0].contains('=') {
        let name = &str_args[0];
        if let Some(exp) = env.get_alias(name) {
            let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(format!(
                "alias {} = {}",
                name,
                fshell_engine::config::fshell_quote(&exp)
            )))));
            drop(tx);
            return Ok(());
        } else {
            return Err(ShellError::new(
                ErrorCode::NotFound,
                format!("alias: {}: not found", name),
            )
            .with_help(format!("Define with `alias {name}=\"<expansion>\"`")));
        }
    }

    let definitions = parse_alias_args(&str_args)?;
    for (name, expansion) in definitions {
        register_alias_entry(&name, &expansion, env)?;
    }
    drop(tx);
    Ok(())
}

fn strip_quotes(s: &str) -> &str {
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn parse_alias_args(str_args: &[String]) -> Result<Vec<(String, String)>, ShellError> {
    let mut results = Vec::new();

    // Check if tokens use lone `=`: e.g. `alias name = expansion ...`
    if str_args.len() >= 3 && str_args[1] == "=" {
        let name = str_args[0].trim();
        let raw_val = str_args[2..].join(" ");
        let val = strip_quotes(raw_val.trim());
        results.push((name.to_string(), val.to_string()));
        return Ok(results);
    }

    let has_eq = str_args.iter().any(|arg| arg.contains('='));
    if has_eq {
        let mut current_name = None;
        let mut current_val_parts: Vec<String> = Vec::new();

        for arg in str_args {
            if let Some(eq_pos) = arg.find('=') {
                if let Some(name) = current_name.take() {
                    let full_val = current_val_parts.join(" ");
                    results.push((name, strip_quotes(full_val.trim()).to_string()));
                    current_val_parts.clear();
                }
                let name = arg[..eq_pos].trim();
                let rest = arg[eq_pos + 1..].trim();
                if name.is_empty() {
                    continue;
                }
                current_name = Some(name.to_string());
                if !rest.is_empty() {
                    current_val_parts.push(rest.to_string());
                }
            } else if current_name.is_some() {
                current_val_parts.push(arg.clone());
            }
        }

        if let Some(name) = current_name {
            let full_val = current_val_parts.join(" ");
            results.push((name, strip_quotes(full_val.trim()).to_string()));
        }

        return Ok(results);
    }

    // Space-separated syntax: `alias name expansion ...`
    if str_args.len() >= 2 {
        let name = str_args[0].trim().to_string();
        let expansion = str_args[1..]
            .iter()
            .filter(|s| s.as_str() != "--force")
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        results.push((name, strip_quotes(expansion.trim()).to_string()));
        return Ok(results);
    }

    Err(
        "alias requires at least 2 arguments or a name=value definition"
            .to_string()
            .into(),
    )
}

fn register_alias_entry(name: &str, expansion: &str, env: &Env) -> Result<(), ShellError> {
    if name.is_empty() {
        return Err("alias: name cannot be empty".to_string().into());
    }
    if name.contains(|c: char| c.is_whitespace()) {
        return Err("alias: name cannot contain whitespace".to_string().into());
    }

    let is_loading_init = env
        .is_loading_init_script
        .load(std::sync::atomic::Ordering::SeqCst);
    if !is_loading_init {
        let is_quiet = env.options.read().quiet_aliases;

        if !is_quiet && env.get_builtin(name).is_some() {
            eprintln!(
                "  {}  alias '{}' shadows builtin — will be ignored",
                Color::Yellow.paint("[!]"),
                name,
            );
        }

        if !is_quiet && let Some(old_expansion) = env.get_alias(name) {
            eprintln!(
                "  {}  alias '{}' redefined: {}",
                Color::Cyan.paint("[i]"),
                name,
                fshell_engine::config::fshell_quote(&old_expansion),
            );
        }
    }

    env.register_alias(name, expansion);

    if !is_loading_init && let Err(e) = fshell_engine::config::persist_alias(name, expansion) {
        eprintln!(
            "\x1b[1;31mWarning:\x1b[0m could not write to init.fsh: {}",
            e
        );
    }

    Ok(())
}

pub fn hook_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    let str_args: Vec<String> = args
        .iter()
        .filter_map(|a| {
            if let Val::String(s) = a {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect();

    if str_args.is_empty() {
        let mut found = false;
        for event in &["precmd", "preexec", "chpwd"] {
            for h in &fshell_engine::get_hooks(event, env) {
                let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(format!(
                    "hook {} {}",
                    event, h
                )))));
                found = true;
            }
        }
        if !found {
            println!("No hooks defined.");
        }
        drop(tx);
        return Ok(());
    }

    if str_args[0] == "--delete" || str_args[0] == "-d" {
        if str_args.len() < 3 {
            return Err("hook --delete requires EVENT and FN_NAME"
                .to_string()
                .into());
        }
        fshell_engine::remove_hook(&str_args[1], &str_args[2], env)?;
        let _ = fshell_engine::config::remove_hook(&str_args[1], &str_args[2]);
        println!("hook {} {} removed.", str_args[1], str_args[2]);
        drop(tx);
        return Ok(());
    }

    if str_args.len() < 2 {
        return Err(
            "hook requires at least 2 arguments: hook EVENT FN_NAME\n\
             Usage:\n  hook EVENT FN_NAME    — register hook\n  hook                  — list hooks\n  hook --delete EVENT FN_NAME — remove hook"
                .to_string().into(),
        );
    }

    fshell_engine::register_hook(&str_args[0], &str_args[1], env)?;
    if !env
        .is_loading_init_script
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        let _ = fshell_engine::config::persist_hook(&str_args[0], &str_args[1]);
    }
    if !env
        .is_loading_init_script
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        println!("hook {} {} registered.", str_args[0], str_args[1]);
    }
    drop(tx);
    Ok(())
}

fn format_param(p: &fshell_core::Param) -> String {
    match &p.constraint {
        fshell_core::TypeConstraint::Any => p.name.clone(),
        fshell_core::TypeConstraint::Primitive(t) => format!("{}: {}", p.name, t),
        _ => p.name.clone(),
    }
}

pub fn format_function(
    name: &str,
    params: &[fshell_core::Param],
    ret_type: &Option<String>,
    body: &[fshell_core::Stmt],
) -> String {
    let mut res = String::new();
    res.push_str("fn ");
    res.push_str(name);
    res.push('(');
    let param_strs: Vec<String> = params.iter().map(format_param).collect();
    res.push_str(&param_strs.join(", "));
    res.push(')');
    if let Some(ret) = ret_type {
        res.push_str(" -> ");
        res.push_str(ret);
    }
    res.push_str(" {\n");
    for stmt in body {
        res.push_str(&format_stmt(stmt, 4));
    }
    res.push_str("}\n");
    res
}

pub fn format_stmt(stmt: &fshell_core::Stmt, indent: usize) -> String {
    let spaces = " ".repeat(indent);
    match stmt.unpack() {
        fshell_core::Stmt::Local { name, expr } => {
            if let Some(expr) = expr {
                format!("{spaces}local {name} = {};\n", format_expr(expr))
            } else {
                format!("{spaces}local {name};\n")
            }
        }
        fshell_core::Stmt::Let { name, expr } => {
            format!("{spaces}let {name} = {};\n", format_expr(expr))
        }
        fshell_core::Stmt::Assign { name, expr } => {
            format!("{spaces}{name} = {};\n", format_expr(expr))
        }
        fshell_core::Stmt::Update { name, op, expr } => {
            let op_str = match op {
                fshell_core::BinOp::Add => "+",
                fshell_core::BinOp::Sub => "-",
                fshell_core::BinOp::Mul => "*",
                fshell_core::BinOp::Div => "/",
                _ => "=",
            };
            format!("{spaces}{name} {op_str}= {};\n", format_expr(expr))
        }
        fshell_core::Stmt::FnDef {
            name,
            params,
            ret_type,
            body,
        } => {
            let mut s = format!("{spaces}fn {name}(");
            let p_strs: Vec<String> = params.iter().map(format_param).collect();
            s.push_str(&p_strs.join(", "));
            s.push(')');
            if let Some(r) = ret_type {
                s.push_str(" -> ");
                s.push_str(r);
            }
            s.push_str(" {\n");
            for sub in body {
                s.push_str(&format_stmt(sub, indent + 4));
            }
            s.push_str(&format!("{spaces}}}\n"));
            s
        }
        fshell_core::Stmt::While { condition, body } => {
            let mut s = format!("{spaces}while {} {{\n", format_expr(condition));
            for sub in body {
                s.push_str(&format_stmt(sub, indent + 4));
            }
            s.push_str(&format!("{spaces}}}\n"));
            s
        }
        fshell_core::Stmt::For { var, iter, body } => {
            let mut s = format!("{spaces}for {var} in {} {{\n", format_expr(iter));
            for sub in body {
                s.push_str(&format_stmt(sub, indent + 4));
            }
            s.push_str(&format!("{spaces}}}\n"));
            s
        }
        fshell_core::Stmt::Break => format!("{spaces}break;\n"),
        fshell_core::Stmt::Continue => format!("{spaces}continue;\n"),
        fshell_core::Stmt::Return(e) => format!("{spaces}return {};\n", format_expr(e)),
        fshell_core::Stmt::Exit(e_opt) => {
            if let Some(e) = e_opt {
                format!("{spaces}exit {};\n", format_expr(e))
            } else {
                format!("{spaces}exit;\n")
            }
        }
        fshell_core::Stmt::Expr(e) => {
            format!("{spaces}{};\n", format_expr(e))
        }
        fshell_core::Stmt::TryCatch {
            try_body,
            catch_var,
            catch_body,
        } => {
            let mut s = format!("{spaces}try {{\n");
            for sub in try_body {
                s.push_str(&format_stmt(sub, indent + 4));
            }
            s.push_str(&format!("{spaces}}} catch {catch_var} {{\n"));
            for sub in catch_body {
                s.push_str(&format_stmt(sub, indent + 4));
            }
            s.push_str(&format!("{spaces}}}\n"));
            s
        }
        fshell_core::Stmt::WithCaps { caps, body } => {
            let mut s = format!("{spaces}with ");
            let c_strs: Vec<String> = caps.iter().map(format_expr).collect();
            s.push_str(&c_strs.join(", "));
            s.push_str(" {\n");
            for sub in body {
                s.push_str(&format_stmt(sub, indent + 4));
            }
            s.push_str(&format!("{spaces}}}\n"));
            s
        }
        fshell_core::Stmt::Unsafe { body } => {
            let mut s = format!("{spaces}unsafe {{\n");
            for sub in body {
                s.push_str(&format_stmt(sub, indent + 4));
            }
            s.push_str(&format!("{spaces}}}\n"));
            s
        }
        fshell_core::Stmt::Comment(c) => {
            format!("{spaces}# {c}\n")
        }
        fshell_core::Stmt::Match { expr, arms } => {
            let mut s = format!("{spaces}match {} {{\n", format_expr(expr));
            for arm in arms {
                s.push_str(&format!(
                    "{spaces}    {} => {{\n",
                    format_match_pattern(&arm.pattern)
                ));
                for sub in &arm.body {
                    s.push_str(&format_stmt(sub, indent + 8));
                }
                s.push_str(&format!("{spaces}    }}\n"));
            }
            s.push_str(&format!("{spaces}}}\n"));
            s
        }
        fshell_core::Stmt::ReactiveCell { name, pipeline } => {
            format!("{spaces}cell {name} = {};\n", format_pipeline(pipeline))
        }
        fshell_core::Stmt::ReactiveCellEvery {
            name,
            duration,
            unit,
            body,
        } => {
            let mut s = format!("{spaces}cell {name} every {duration} {:?} {{\n", unit);
            for sub in body {
                s.push_str(&format_stmt(sub, indent + 4));
            }
            s.push_str(&format!("{spaces}}}\n"));
            s
        }
        fshell_core::Stmt::Source { path, bash } => {
            let prefix = if *bash { "source --bash " } else { "source " };
            format!("{spaces}{prefix}{};\n", format_expr(path))
        }
        fshell_core::Stmt::On { signal, handler } => {
            let handler_str = match handler {
                fshell_core::OnHandler::FunctionName(name) => name.clone(),
                fshell_core::OnHandler::Block(body) => {
                    let mut s = "{\n".to_string();
                    for sub in body {
                        s.push_str(&format_stmt(sub, indent + 4));
                    }
                    s.push_str(&format!("{spaces}}}"));
                    s
                }
            };
            format!("{spaces}on {signal} {handler_str}\n")
        }
        fshell_core::Stmt::Background(body) => {
            format!("{}async {}", spaces, format_stmt(body, 0))
        }
        fshell_core::Stmt::Every {
            duration,
            unit,
            body,
        } => {
            let mut s = format!("{spaces}every {duration} {:?} {{\n", unit);
            for sub in body {
                s.push_str(&format_stmt(sub, indent + 4));
            }
            s.push_str(&format!("{spaces}}}\n"));
            s
        }
        fshell_core::Stmt::And(left, right) => {
            format!(
                "{}{} && {}",
                spaces,
                format_stmt(left, 0).trim_end(),
                format_stmt(right, 0)
            )
        }
        fshell_core::Stmt::Or(left, right) => {
            format!(
                "{}{} || {}",
                spaces,
                format_stmt(left, 0).trim_end(),
                format_stmt(right, 0)
            )
        }
        fshell_core::Stmt::Spanned { stmt: inner, .. } => format_stmt(inner, indent),
        fshell_core::Stmt::PosixBlock { body } => {
            format!("{spaces}sh {{\n{body}\n{spaces}}}\n")
        }
    }
}

fn format_match_pattern(p: &fshell_core::MatchPattern) -> String {
    match p {
        fshell_core::MatchPattern::Wildcard => "_".to_string(),
        fshell_core::MatchPattern::Literal(lit) => match lit {
            fshell_core::LiteralPattern::Null => "null".to_string(),
            fshell_core::LiteralPattern::Bool(b) => b.to_string(),
            fshell_core::LiteralPattern::Int(i) => i.to_string(),
            fshell_core::LiteralPattern::Float(f) => f.to_string(),
            fshell_core::LiteralPattern::String(s) => format!("{s:?}"),
        },
        fshell_core::MatchPattern::Map { fields, rest } => {
            let mut s = "{".to_string();
            let mut parts = Vec::new();
            for (k, v) in fields {
                parts.push(format!("{k}: {}", format_match_pattern(v)));
            }
            if *rest {
                parts.push("..".to_string());
            }
            s.push_str(&parts.join(", "));
            s.push('}');
            s
        }
    }
}

pub fn format_expr(expr: &fshell_core::Expr) -> String {
    match expr.unpack() {
        fshell_core::Expr::Null => "null".to_string(),
        fshell_core::Expr::Bool(b) => b.to_string(),
        fshell_core::Expr::Int(i) => i.to_string(),
        fshell_core::Expr::Float(f) => f.to_string(),
        fshell_core::Expr::String(parts) => {
            let mut s = String::new();
            s.push('"');
            for part in parts {
                match part {
                    fshell_core::StringPart::Lit(l) => {
                        s.push_str(&l.replace('\\', "\\\\").replace('"', "\\\""));
                    }
                    fshell_core::StringPart::Expr(e) => {
                        s.push('{');
                        s.push_str(&format_expr(e));
                        s.push('}');
                    }
                }
            }
            s.push('"');
            s
        }
        fshell_core::Expr::Ident(name) => name.clone(),
        fshell_core::Expr::Variable(name) => format!("${name}"),
        fshell_core::Expr::List(items) => {
            let item_strs: Vec<String> = items.iter().map(format_expr).collect();
            format!("[{}]", item_strs.join(", "))
        }
        fshell_core::Expr::Map(pairs) => {
            let pair_strs: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", k, format_expr(v)))
                .collect();
            format!("{{{}}}", pair_strs.join(", "))
        }
        fshell_core::Expr::BinaryOp { op, lhs, rhs } => {
            let op_str = match op {
                fshell_core::BinOp::Add => "+",
                fshell_core::BinOp::Sub => "-",
                fshell_core::BinOp::Mul => "*",
                fshell_core::BinOp::Div => "/",
                fshell_core::BinOp::Eq => "==",
                fshell_core::BinOp::Neq => "!=",
                fshell_core::BinOp::Lt => "<",
                fshell_core::BinOp::Lte => "<=",
                fshell_core::BinOp::Gt => ">",
                fshell_core::BinOp::Gte => ">=",
                fshell_core::BinOp::ReMatch => "~",
                fshell_core::BinOp::And => "&&",
                fshell_core::BinOp::Or => "||",
            };
            format!("{} {op_str} {}", format_expr(lhs), format_expr(rhs))
        }
        fshell_core::Expr::Not(e) => format!("!{}", format_expr(e)),
        fshell_core::Expr::MemberAccess { expr, member } => {
            format!("{}.{}", format_expr(expr), member)
        }
        fshell_core::Expr::Pipeline(p) => format_pipeline(p),
        fshell_core::Expr::InlinePipeline(p) => format!("$| {}", format_pipeline(p)),
        fshell_core::Expr::VarWithModifier { name, modifier } => {
            let mod_str = match modifier {
                fshell_core::ParamModifier::Tail => ":t".to_string(),
                fshell_core::ParamModifier::Head => ":h".to_string(),
                fshell_core::ParamModifier::Root => ":r".to_string(),
                fshell_core::ParamModifier::Ext => ":e".to_string(),
                fshell_core::ParamModifier::Default(e) => format!(":-{}", format_expr(e)),
                fshell_core::ParamModifier::AssignDefault(e) => format!(":={}", format_expr(e)),
                fshell_core::ParamModifier::ErrorIfUnset(e) => format!(":?{}", format_expr(e)),
                fshell_core::ParamModifier::Alternate(e) => format!(":+{}", format_expr(e)),
                fshell_core::ParamModifier::Substring { offset, length } => {
                    if let Some(l) = length {
                        format!(":{}:{}", offset, l)
                    } else {
                        format!(":{}", offset)
                    }
                }
                fshell_core::ParamModifier::ShortestPrefix(e) => format!("#{}", format_expr(e)),
                fshell_core::ParamModifier::LongestPrefix(e) => format!("##{}", format_expr(e)),
                fshell_core::ParamModifier::ShortestSuffix(e) => format!("%{}", format_expr(e)),
                fshell_core::ParamModifier::LongestSuffix(e) => format!("%%{}", format_expr(e)),
                fshell_core::ParamModifier::Replace {
                    pattern,
                    replacement,
                    global,
                } => {
                    let slash = if *global { "//" } else { "/" };
                    format!(
                        "{}{}/{}",
                        slash,
                        format_expr(pattern),
                        format_expr(replacement)
                    )
                }
                fshell_core::ParamModifier::StringLength => format!("${{#{}}}", name),
                fshell_core::ParamModifier::Upper => ":u".to_string(),
                fshell_core::ParamModifier::Lower => ":l".to_string(),
            };
            format!("${{{name}{mod_str}}}")
        }
        fshell_core::Expr::ArithmeticExpansion(e) => format!("$(({}))", format_expr(e)),
        fshell_core::Expr::AnsiCQuote(s) => format!("$'{}'", s.replace('\'', "\\'")),
        fshell_core::Expr::RawMultiLineString(s) => format!("'''{}'''", s),
        fshell_core::Expr::MultiLineString { parts, dedent: _ } => {
            let mut s = String::new();
            s.push_str("\"\"\"");
            for part in parts {
                match part {
                    fshell_core::StringPart::Lit(l) => s.push_str(l),
                    fshell_core::StringPart::Expr(e) => {
                        s.push('{');
                        s.push_str(&format_expr(e));
                        s.push('}');
                    }
                }
            }
            s.push_str("\"\"\"");
            s
        }
        fshell_core::Expr::If {
            condition,
            then_body,
            else_body,
        } => {
            let mut s = format!("if {} {{\n", format_expr(condition));
            for sub in then_body {
                s.push_str(&format_stmt(sub, 4));
            }
            s.push('}');
            if let Some(else_stmts) = else_body {
                s.push_str(" else {\n");
                for sub in else_stmts {
                    s.push_str(&format_stmt(sub, 4));
                }
                s.push_str("}\n");
            } else {
                s.push('\n');
            }
            s
        }
        fshell_core::Expr::ProcessSubst {
            direction,
            pipeline,
        } => {
            let sign = match direction {
                fshell_core::ProcessSubstDirection::Input => "<",
                fshell_core::ProcessSubstDirection::Output => ">",
            };
            format!("{}({})", sign, format_pipeline(pipeline))
        }
        fshell_core::Expr::Spanned { expr: inner, .. } => format_expr(inner),
    }
}

pub fn format_pipeline(p: &fshell_core::Pipeline) -> String {
    let mut stages = Vec::new();
    for stage in &p.stages {
        let s = match stage {
            fshell_core::PipelineStage::CommandCall { name, args, env } => {
                let mut cmd = String::new();
                for (k, v) in env {
                    cmd.push_str(&format!("{k}={} ", format_expr(v)));
                }
                cmd.push_str(name);
                for arg in args {
                    cmd.push_str(&format!(" {}", format_expr(arg)));
                }
                cmd
            }
            fshell_core::PipelineStage::Filter { condition } => {
                format!("filter {}", format_expr(condition))
            }
            fshell_core::PipelineStage::Map { projections } => {
                let proj_strs: Vec<String> = projections.iter().map(format_expr).collect();
                format!("map {}", proj_strs.join(", "))
            }
            fshell_core::PipelineStage::Sort { column, descending } => {
                if *descending {
                    format!("sort -d {column}")
                } else {
                    format!("sort {column}")
                }
            }
            fshell_core::PipelineStage::Grep { pattern } => {
                format!("grep {}", format_expr(pattern))
            }
            fshell_core::PipelineStage::Count => "count".to_string(),
            fshell_core::PipelineStage::Limit { amount } => {
                format!("limit {}", format_expr(amount))
            }
            fshell_core::PipelineStage::BoundaryOperator { format } => {
                let f = match format {
                    fshell_core::SerializationFormat::Json => "@json",
                    fshell_core::SerializationFormat::Yaml => "@yaml",
                    fshell_core::SerializationFormat::MsgPack => "@msgpack",
                    fshell_core::SerializationFormat::Text => "@text",
                    fshell_core::SerializationFormat::Csv => "@csv",
                    fshell_core::SerializationFormat::Table => "@table",
                    fshell_core::SerializationFormat::Bar => "@bar",
                };
                f.to_string()
            }
            fshell_core::PipelineStage::Write { path, append, .. } => {
                let sign = if *append { ">>" } else { ">" };
                format!("{sign} {}", format_expr(path))
            }
            fshell_core::PipelineStage::Read { path } => {
                format!("< {}", format_expr(path))
            }
            fshell_core::PipelineStage::FdRedirect { src_fd, dst_fd } => {
                format!("{src_fd}>&{dst_fd}")
            }
            _ => "[unsupported stage]".to_string(),
        };
        stages.push(s);
    }
    stages.join(" | ")
}

pub fn funced_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return Err(ShellError::new(
            ErrorCode::InvalidArgument,
            "funced: expected function name",
        ));
    }
    let Val::String(name) = &args[0] else {
        return Err(ShellError::new(
            ErrorCode::InvalidArgument,
            "funced: function name must be a string",
        ));
    };

    // 1. Retrieve function from memory
    let (params, ret_type, body) = {
        let fns = env.fns.read();
        let Some(f) = fns.get(name) else {
            return Err(ShellError::function_not_found(name, None));
        };
        f.clone()
    };

    // 2. Format the function to source code
    let formatted = format_function(name, &params, &ret_type, &body);

    // 3. Create temp file
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join(format!("fsh_funced_{}_{}.fsh", name, std::process::id()));
    std::fs::write(&tmp_path, &formatted)
        .map_err(|e| format!("funced: failed to write temp file: {}", e))?;

    // 4. Determine editor to open
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "nano".to_string());

    // 5. Spawn editor
    let mut parts = editor.split_whitespace();
    let cmd = parts
        .next()
        .ok_or("funced: invalid EDITOR environment variable")?;
    let mut command = std::process::Command::new(cmd);
    for part in parts {
        command.arg(part);
    }
    command.arg(&tmp_path);

    // Ensure editor inherits stdin/stdout/stderr for interactive TUI support
    command.stdin(std::process::Stdio::inherit());
    command.stdout(std::process::Stdio::inherit());
    command.stderr(std::process::Stdio::inherit());

    // Configure standard process group for UNIX job control
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    struct EditorTerminalGuard {
        raw_mode_was_enabled: bool,
        shell_pgid: i32,
    }

    impl EditorTerminalGuard {
        fn new() -> Self {
            fshell_engine::suspend_session_logging();
            let raw_mode_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
            if raw_mode_was_enabled {
                let _ = crossterm::terminal::disable_raw_mode();
            }
            #[cfg(unix)]
            let shell_pgid = unsafe { libc::getpgrp() };
            #[cfg(not(unix))]
            let shell_pgid = 0;

            Self {
                raw_mode_was_enabled,
                shell_pgid,
            }
        }
    }

    impl Drop for EditorTerminalGuard {
        fn drop(&mut self) {
            #[cfg(unix)]
            unsafe {
                libc::signal(libc::SIGTTOU, libc::SIG_IGN);
                libc::tcsetpgrp(libc::STDIN_FILENO, self.shell_pgid);
                libc::signal(libc::SIGTTOU, libc::SIG_DFL);
            }
            if self.raw_mode_was_enabled {
                let _ = crossterm::terminal::enable_raw_mode();
            }
            fshell_engine::resume_session_logging();
        }
    }

    let _guard = EditorTerminalGuard::new();

    let mut child = command
        .spawn()
        .map_err(|e| format!("funced: failed to spawn editor '{}': {}", editor, e))?;

    // Give child's process group ownership of the terminal
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGTTOU, libc::SIG_IGN);
        libc::tcsetpgrp(libc::STDIN_FILENO, child.id() as i32);
        libc::signal(libc::SIGTTOU, libc::SIG_DFL);
    }

    let status = child
        .wait()
        .map_err(|e| format!("funced: editor process error: {}", e))?;

    drop(_guard);

    if !status.success() {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!(
            "funced: editor exited with non-zero status: {:?}",
            status.code()
        )
        .into());
    }

    // 6. Read edited content
    let edited = std::fs::read_to_string(&tmp_path)
        .map_err(|e| format!("funced: failed to read edited temp file: {}", e))?;
    let _ = std::fs::remove_file(&tmp_path);

    // 7. Parse the edited content
    let mut parser = fshell_core::Parser::new(&edited);
    let stmts = parser
        .parse_statements()
        .map_err(|e| format!("funced: failed to parse edited function: {:?}", e))?;

    if stmts.is_empty() {
        return Err("funced: edited function is empty".to_string().into());
    }

    let first = stmts[0].unpack();
    let fshell_core::Stmt::FnDef {
        name: parsed_name,
        params: parsed_params,
        ret_type: parsed_ret,
        body: parsed_body,
    } = first
    else {
        return Err("funced: edited content must define exactly one function"
            .to_string()
            .into());
    };

    if parsed_name != name {
        return Err(ShellError::new(
            ErrorCode::InvalidArgument,
            format!("funced: cannot rename function from '{name}' to '{parsed_name}'"),
        )
        .with_help(format!("Keep the function name as '{name}' when editing")));
    }

    // 8. Update in memory
    {
        let mut fns = env.fns.write();
        fns.insert(
            name.clone(),
            (
                parsed_params.clone(),
                parsed_ret.clone(),
                parsed_body.clone(),
            ),
        );
    }

    println!("function '{}' updated in memory.", name);
    drop(tx);
    Ok(())
}

pub fn funcsave_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return Err(ShellError::new(
            ErrorCode::InvalidArgument,
            "funcsave: expected function name",
        ));
    }
    let Val::String(name) = &args[0] else {
        return Err("funcsave: function name must be a string"
            .to_string()
            .into());
    };

    // 1. Retrieve function from memory
    let (params, ret_type, body) = {
        let fns = env.fns.read();
        let Some(f) = fns.get(name) else {
            return Err(ShellError::function_not_found(name, None));
        };
        f.clone()
    };

    // 2. Format the function to source code
    let formatted = format_function(name, &params, &ret_type, &body);

    // 3. Append/insert it into init.fsh
    fshell_engine::config::persist_function(&formatted).map_err(|e| format!("funcsave: {e}"))?;

    println!("function '{}' saved to init.fsh.", name);
    drop(tx);
    Ok(())
}
