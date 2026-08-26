// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::ShellError;
use fshell_core::Val;
use fshell_core::diagnostic::ErrorCode;
use fshell_core::theme::Theme;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use std::sync::Arc;

fn theme_config_dir() -> std::path::PathBuf {
    fshell_engine::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."))
}

pub fn theme_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    if args.is_empty() {
        let theme = env.active_theme();
        send_string(tx, &format!("Active theme: {}", theme.name))?;
        return Ok(());
    }

    let subcmd = match &args[0] {
        Val::String(s) => s.as_str(),
        _ => {
            return Err(ShellError::new(
                ErrorCode::InvalidArgument,
                "theme: subcommand must be a string",
            ));
        }
    };

    match subcmd {
        "show" | "current" => {
            let theme = env.active_theme();
            send_string(tx, &theme.name)
        }
        "ls" | "list" => {
            let config_dir = theme_config_dir();
            let names = Theme::available(&config_dir);
            send_string(tx, &names.join("\n"))
        }
        "set" => {
            if args.len() < 2 {
                return Err(ShellError::new(
                    ErrorCode::MissingArgument,
                    "theme set: missing theme name",
                ));
            }
            let name = match &args[1] {
                Val::String(s) => s.as_str(),
                _ => {
                    return Err(ShellError::new(
                        ErrorCode::InvalidArgument,
                        "theme set: name must be a string",
                    ));
                }
            };
            let config_dir = theme_config_dir();
            let theme = Theme::load(name, &config_dir).map_err(|e| e.to_string())?;
            env.set_theme(Arc::new(theme));
            {
                let mut opts = env.options.write();
                opts.theme = name.to_string();
            }
            crate::cmd::config::persist_settings(env)?;
            send_string(tx, &format!("Theme set to: {}", name))
        }
        "preview" => {
            if args.len() < 2 {
                return Err(ShellError::new(
                    ErrorCode::MissingArgument,
                    "theme preview: missing theme name",
                ));
            }
            let name = match &args[1] {
                Val::String(s) => s.as_str(),
                _ => {
                    return Err(ShellError::new(
                        ErrorCode::InvalidArgument,
                        "theme preview: name must be a string",
                    ));
                }
            };
            let config_dir = theme_config_dir();
            let theme = Theme::load(name, &config_dir).map_err(|e| e.to_string())?;
            env.preview_theme(Arc::new(theme));
            send_string(
                tx,
                &format!("Previewing theme: {} (reverts on next command)", name),
            )
        }
        "export" => {
            let theme = if args.len() > 1 {
                let name = match &args[1] {
                    Val::String(s) => s.as_str(),
                    _ => {
                        return Err(ShellError::new(
                            ErrorCode::InvalidArgument,
                            "theme export: name must be a string",
                        ));
                    }
                };
                let config_dir = theme_config_dir();
                Theme::load(name, &config_dir).map_err(|e| e.to_string())?
            } else {
                env.active_theme().as_ref().clone()
            };
            let toml = theme_to_toml(&theme);
            send_string(tx, &toml)
        }
        "diff" => {
            if args.len() < 3 {
                return Err("theme diff: requires two theme names".to_string().into());
            }
            let name1 = match &args[1] {
                Val::String(s) => s.as_str(),
                _ => {
                    return Err(ShellError::new(
                        ErrorCode::InvalidArgument,
                        "theme diff: names must be strings",
                    ));
                }
            };
            let name2 = match &args[2] {
                Val::String(s) => s.as_str(),
                _ => {
                    return Err(ShellError::new(
                        ErrorCode::InvalidArgument,
                        "theme diff: names must be strings",
                    ));
                }
            };
            let config_dir = theme_config_dir();
            let theme1 = Theme::load(name1, &config_dir).map_err(|e| e.to_string())?;
            let theme2 = Theme::load(name2, &config_dir).map_err(|e| e.to_string())?;
            let diff = diff_themes(&theme1, &theme2);
            send_string(tx, &diff)
        }
        _ => Err(format!(
            "theme: unknown subcommand '{}'. Use show/ls/set/preview/export/diff",
            subcmd
        )
        .into()),
    }
}

fn send_string(tx: PipeSender, s: &str) -> Result<(), ShellError> {
    let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(s.to_string()))));
    Ok(())
}

fn theme_to_toml(theme: &Theme) -> String {
    toml_edit::ser::to_string(theme).unwrap_or_default()
}

fn diff_themes(t1: &Theme, t2: &Theme) -> String {
    let mut diffs = Vec::new();

    macro_rules! diff_field {
        ($label:expr, $a:expr, $b:expr) => {
            if $a != $b {
                diffs.push(format!("  {}: {:?} → {:?}", $label, $a, $b));
            }
        };
    }

    diff_field!("syntax.keyword", t1.syntax.keyword, t2.syntax.keyword);
    diff_field!("syntax.builtin", t1.syntax.builtin, t2.syntax.builtin);
    diff_field!("syntax.variable", t1.syntax.variable, t2.syntax.variable);
    diff_field!("syntax.string", t1.syntax.string, t2.syntax.string);
    diff_field!("syntax.comment", t1.syntax.comment, t2.syntax.comment);
    diff_field!("status.ok", t1.status.ok, t2.status.ok);
    diff_field!("status.error", t1.status.error, t2.status.error);
    diff_field!("widgets.border", t1.widgets.border, t2.widgets.border);
    diff_field!("widgets.title", t1.widgets.title, t2.widgets.title);
    diff_field!(
        "chrome.background",
        t1.chrome.background,
        t2.chrome.background
    );

    if diffs.is_empty() {
        format!("No differences between '{}' and '{}'", t1.name, t2.name)
    } else {
        let header = format!("Differences between '{}' and '{}' :\n", t1.name, t2.name);
        header + &diffs.join("\n")
    }
}
