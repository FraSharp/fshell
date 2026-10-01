// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! POSIX `type` builtin implementation.
//!
//! Conforms to IEEE Std 1003.1 `type` utility specification.
//! Reports how each argument would be interpreted if used as a command name.

use fshell_engine::Env;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandResolution {
    Keyword,
    Alias(String),
    SpecialBuiltin,
    Function,
    Builtin,
    Path(String),
    NotFound,
}

pub fn type_posix(args: &[String], env: &Env) -> Result<(i32, String), String> {
    if args.is_empty() {
        return Ok((0, String::new()));
    }

    let mut exit_code = 0;
    let mut out = String::new();

    for name in args {
        match resolve_command(name, env, None) {
            CommandResolution::Keyword => out.push_str(&format!("{name} is a shell keyword\n")),
            CommandResolution::Alias(alias) => {
                out.push_str(&format!("{name} is an alias for {alias}\n"));
            }
            CommandResolution::SpecialBuiltin => {
                out.push_str(&format!("{name} is a shell builtin\n"));
            }
            CommandResolution::Function => out.push_str(&format!("{name} is a function\n")),
            CommandResolution::Builtin => {
                out.push_str(&format!("{name} is a shell builtin\n"));
            }
            CommandResolution::Path(path) => out.push_str(&format!("{name} is {path}\n")),
            CommandResolution::NotFound => {
                eprintln!("type: {name}: not found");
                exit_code = 1;
            }
        }
    }

    Ok((exit_code, out))
}

pub fn resolve_command(name: &str, env: &Env, path_override: Option<&str>) -> CommandResolution {
    if is_posix_keyword(name) {
        return CommandResolution::Keyword;
    }
    if let Some(alias) = env.get_alias(name) {
        return CommandResolution::Alias(alias);
    }
    if is_special_builtin(name) {
        return CommandResolution::SpecialBuiltin;
    }
    if crate::eval::get_posix_function(env, name).is_some() {
        return CommandResolution::Function;
    }
    if is_posix_builtin(name)
        || (matches!(name, "jobs" | "kill" | "fg" | "bg" | "disown")
            && env.get_builtin(name).is_some())
    {
        return CommandResolution::Builtin;
    }
    resolve_in_path(name, env, path_override)
        .map(CommandResolution::Path)
        .unwrap_or(CommandResolution::NotFound)
}

fn is_posix_keyword(name: &str) -> bool {
    matches!(
        name,
        "if" | "then"
            | "else"
            | "elif"
            | "fi"
            | "case"
            | "esac"
            | "for"
            | "while"
            | "until"
            | "do"
            | "done"
            | "in"
            | "{"
            | "}"
            | "!"
            | "time"
            | "function"
    )
}

pub fn is_special_builtin(name: &str) -> bool {
    matches!(
        name,
        ":" | "break"
            | "continue"
            | "."
            | "eval"
            | "exec"
            | "exit"
            | "export"
            | "readonly"
            | "return"
            | "set"
            | "shift"
            | "times"
            | "trap"
            | "unset"
    )
}

fn is_posix_builtin(name: &str) -> bool {
    matches!(
        name,
        "true"
            | "false"
            | "local"
            | "read"
            | "printf"
            | "echo"
            | "eval"
            | "test"
            | "["
            | "cd"
            | "pwd"
            | "wait"
            | "umask"
            | "alias"
            | "unalias"
            | "command"
            | "type"
            | "hash"
            | "getopts"
            | "ulimit"
            | "dot"
            | "source"
    )
}

pub fn resolve_in_path(name: &str, env: &Env, path_override: Option<&str>) -> Option<String> {
    if name.contains('/') {
        return executable_absolute_path(env.resolve_path(name));
    }

    let path_var = path_override.map(str::to_string).or_else(|| {
        let vars = env.vars.read();
        vars.get("PATH").map(|value| value.to_text()).or_else(|| {
            vars.get("env").and_then(|value| {
                if let fshell_core::Val::Map(map) = value {
                    map.get(&ustr::ustr("PATH")).map(|path| path.to_text())
                } else {
                    None
                }
            })
        })
    });
    let path_var =
        path_var.unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_string());

    for dir in path_var.split(':') {
        let candidate = env.resolve_path(std::path::Path::new(dir).join(name));
        if let Some(path) = executable_absolute_path(candidate) {
            return Some(path);
        }
    }

    None
}

fn executable_absolute_path(path: std::path::PathBuf) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    if unsafe { libc::access(c_path.as_ptr(), libc::X_OK) } != 0 {
        return None;
    }
    Some(path.display().to_string())
}
