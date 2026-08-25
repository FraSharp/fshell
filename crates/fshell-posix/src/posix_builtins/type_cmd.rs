// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! POSIX `type` builtin implementation.
//!
//! Conforms to IEEE Std 1003.1 `type` utility specification.
//! Reports how each argument would be interpreted if used as a command name.

use fshell_engine::Env;

pub fn type_posix(args: &[String], env: &Env) -> Result<(i32, String), String> {
    if args.is_empty() {
        return Ok((0, String::new()));
    }

    let mut exit_code = 0;
    let mut out = String::new();

    for name in args {
        if is_posix_keyword(name) {
            out.push_str(&format!("{} is a shell keyword\n", name));
            continue;
        }

        // Check aliases
        if let Some(alias) = env.get_alias(name) {
            out.push_str(&format!("{} is an alias for {}\n", name, alias));
            continue;
        }

        // Check functions
        let is_fn =
            crate::eval::get_posix_function(name).is_some() || env.fns.read().contains_key(name);
        if is_fn {
            out.push_str(&format!("{} is a function\n", name));
            continue;
        }

        // Check builtins
        if is_posix_builtin(name) || env.get_builtin(name).is_some() {
            out.push_str(&format!("{} is a shell builtin\n", name));
            continue;
        }

        // Check PATH executable
        if let Some(path) = resolve_in_path(name, env) {
            out.push_str(&format!("{} is {}\n", name, path));
            continue;
        }

        eprintln!("type: {}: not found", name);
        exit_code = 1;
    }

    Ok((exit_code, out))
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

fn is_posix_builtin(name: &str) -> bool {
    matches!(
        name,
        ":" | "true"
            | "false"
            | "exit"
            | "return"
            | "break"
            | "continue"
            | "shift"
            | "set"
            | "unset"
            | "export"
            | "read"
            | "printf"
            | "echo"
            | "eval"
            | "test"
            | "["
            | "cd"
            | "pwd"
            | "exec"
            | "trap"
            | "wait"
            | "kill"
            | "umask"
            | "alias"
            | "unalias"
            | "command"
            | "type"
            | "hash"
            | "getopts"
            | "dot"
            | "."
            | "source"
    )
}

fn resolve_in_path(name: &str, env: &Env) -> Option<String> {
    if name.contains('/') {
        let p = std::path::Path::new(name);
        if p.is_file() {
            return Some(
                p.canonicalize()
                    .unwrap_or_else(|_| p.to_path_buf())
                    .display()
                    .to_string(),
            );
        }
        return None;
    }

    let path_var = Some(env.vars.read())
        .and_then(|vars| {
            vars.get("PATH").map(|v| v.to_text()).or_else(|| {
                vars.get("env").and_then(|v| {
                    if let fshell_core::Val::Map(m) = v {
                        m.get(&ustr::ustr("PATH")).map(|pv| pv.to_text())
                    } else {
                        None
                    }
                })
            })
        })
        .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_string());

    for dir in path_var.split(':') {
        let candidate = std::path::Path::new(dir).join(name);
        if candidate.is_file() {
            return Some(candidate.display().to_string());
        }
    }

    None
}
