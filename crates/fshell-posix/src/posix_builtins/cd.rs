// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! POSIX `cd` and `pwd`.
//!
//! Both are logical by default: the shell tracks the path the user travelled
//! (`$PWD`), with `..` removing the previous component, rather than the
//! symlink-resolved physical path. `cd /tmp; pwd` prints `/tmp` even where
//! `/tmp` is a link. `-P` asks for the physical path instead.

use fshell_core::Val;
use fshell_engine::Env;
use std::path::{Component, Path, PathBuf};

/// Change directory. Returns the line to print (`cd -`, or a directory found
/// through `CDPATH`), or a status and diagnostic.
pub fn cd_posix(args: &[String], env: &Env) -> Result<Option<String>, (i32, String)> {
    let mut physical = false;
    let mut operands = args.iter().peekable();
    while let Some(flag) = operands.peek() {
        match flag.as_str() {
            "-L" => physical = false,
            "-P" => physical = true,
            "--" => {
                operands.next();
                break;
            }
            _ => break,
        }
        operands.next();
    }
    let operands: Vec<&String> = operands.collect();
    if operands.len() > 1 {
        return Err((2, "cd: too many arguments".to_string()));
    }

    let var = |name: &str| env.vars.read().get(name).map(Val::to_text);
    let mut print = false;
    let directory = match operands.first().map(|s| s.as_str()) {
        None => match var("HOME") {
            Some(home) if !home.is_empty() => home,
            _ => return Err((1, "cd: HOME not set".to_string())),
        },
        Some("-") => {
            print = true;
            match var("OLDPWD") {
                Some(old) if !old.is_empty() => old,
                _ => return Err((1, "cd: OLDPWD not set".to_string())),
            }
        }
        Some(dir) => dir.to_string(),
    };

    let current = logical_cwd(env);
    let mut target = None;
    let cdpath = var("CDPATH").filter(|path| !path.is_empty());
    if let Some(cdpath) = cdpath
        && !directory.starts_with('/')
        && !starts_with_dot_component(&directory)
    {
        for entry in cdpath.split(':') {
            let base = if entry.is_empty() {
                current.clone()
            } else {
                current.join(entry)
            };
            let candidate = lexical(&base.join(&directory));
            if candidate.is_dir() {
                print |= !entry.is_empty();
                target = Some(candidate);
                break;
            }
        }
    }
    let target = target.unwrap_or_else(|| lexical(&current.join(&directory)));

    match std::fs::metadata(&target) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err((1, format!("cd: {directory}: Not a directory"))),
        Err(error) => {
            return Err((
                1,
                format!("cd: {directory}: {}", crate::redirect::os_message(&error)),
            ));
        }
    }
    let target = if physical {
        target.canonicalize().map_err(|e| {
            (
                1,
                format!("cd: {directory}: {}", crate::redirect::os_message(&e)),
            )
        })?
    } else {
        target
    };

    env.set_cwd(target.clone());
    let new_pwd = target.to_string_lossy().into_owned();
    env.set_shell_var(
        "OLDPWD",
        Val::String(current.to_string_lossy().into_owned()),
    );
    env.set_shell_var("PWD", Val::String(new_pwd.clone()));
    Ok(print.then(|| format!("{new_pwd}\n")))
}

/// Print the working directory, logically unless `-P`.
pub fn pwd_posix(args: &[String], env: &Env) -> Result<String, (i32, String)> {
    let mut physical = false;
    for arg in args {
        match arg.as_str() {
            "-L" => physical = false,
            "-P" => physical = true,
            other => return Err((2, format!("pwd: {other}: invalid option"))),
        }
    }
    let path = if physical {
        env.cwd()
            .canonicalize()
            .map_err(|e| (1, format!("pwd: {}", crate::redirect::os_message(&e))))?
    } else {
        logical_cwd(env)
    };
    Ok(format!("{}\n", path.display()))
}

/// `$PWD` when it is an absolute, normalized path naming the current
/// directory; otherwise the physical path, as POSIX specifies for `pwd -L`.
fn logical_cwd(env: &Env) -> PathBuf {
    let cwd = env.cwd();
    let pwd = env.vars.read().get("PWD").map(Val::to_text);
    if let Some(pwd) = pwd {
        let candidate = PathBuf::from(&pwd);
        if candidate.is_absolute()
            && lexical(&candidate) == candidate
            && same_file(&candidate, &cwd)
        {
            return candidate;
        }
    }
    cwd
}

fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

fn starts_with_dot_component(dir: &str) -> bool {
    dir == "." || dir == ".." || dir.starts_with("./") || dir.starts_with("../")
}

/// Remove `.` components and resolve `..` against the preceding component,
/// without consulting the filesystem.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => out.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
            Component::Prefix(_) => {}
        }
    }
    if out.as_os_str().is_empty() {
        out.push("/");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_paths_resolve_dot_dot_against_the_previous_component() {
        assert_eq!(lexical(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(lexical(Path::new("/../..")), PathBuf::from("/"));
        assert_eq!(lexical(Path::new("/tmp/")), PathBuf::from("/tmp"));
    }
}
