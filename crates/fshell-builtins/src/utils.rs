// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::ShellError;
use fshell_core::{ResourceHandle, Val, set_var};
use fshell_engine::{CapAction, Env};
use std::ffi::CString;
use std::path::PathBuf;

pub fn check_read_file(env: &Env, cmd: &str, path: PathBuf) -> Result<(), ShellError> {
    env.enforce_capability(cmd, CapAction::ReadFile(path.clone()))?;
    env.track_read(path);
    Ok(())
}

pub fn change_dir_and_update_caps(
    target_path: &std::path::Path,
    env: &Env,
) -> Result<(), ShellError> {
    let old_pwd = env.cwd();
    let resolved = if target_path.is_absolute() {
        target_path.to_path_buf()
    } else {
        old_pwd.join(target_path)
    };

    let canonical = match resolved.canonicalize() {
        Ok(c) => c,
        Err(e) => {
            return Err(ShellError::io_error(
                format!("Failed to change directory to {:?}: {}", target_path, e),
                None,
            ));
        }
    };

    if !canonical.is_dir() {
        return Err(ShellError::io_error(
            format!("Not a directory: {:?}", target_path),
            None,
        ));
    }

    env.enforce_capability("cd", CapAction::ReadDir(canonical.clone()))?;
    env.set_cwd(canonical.clone());

    let mut caps = env.caps.caps.write();
    caps.held.retain(|h| match h {
        ResourceHandle::ReadDir(p)
        | ResourceHandle::WriteDir(p)
        | ResourceHandle::ReadFile(p)
        | ResourceHandle::WriteFile(p) => !(p.starts_with(&old_pwd) && p != &canonical),
        _ => true,
    });
    caps.grant(ResourceHandle::ReadDir(canonical.clone()));
    caps.grant(ResourceHandle::WriteDir(canonical.clone()));

    const MAX_PATH_CAPS: usize = 100;
    let path_count = caps
        .held
        .iter()
        .filter(|h| {
            matches!(
                h,
                ResourceHandle::ReadDir(_)
                    | ResourceHandle::WriteDir(_)
                    | ResourceHandle::ReadFile(_)
                    | ResourceHandle::WriteFile(_)
            )
        })
        .count();
    if path_count > MAX_PATH_CAPS {
        let target = canonical.clone();
        caps.held.retain(|h| match h {
            ResourceHandle::ReadDir(p)
            | ResourceHandle::WriteDir(p)
            | ResourceHandle::ReadFile(p)
            | ResourceHandle::WriteFile(p) => p.starts_with(&target) || target.starts_with(p),
            _ => true,
        });
    }
    let pwd_str = canonical.to_string_lossy().to_string();
    let old_pwd_str = old_pwd.to_string_lossy().to_string();
    drop(caps);

    {
        let mut vars = env.vars.write();
        vars.insert("PWD".to_string(), Val::String(pwd_str.clone()));
        vars.insert("OLDPWD".to_string(), Val::String(old_pwd_str.clone()));
        if let Some(Val::Map(env_map)) = vars.get("env") {
            let mut new_map = env_map.clone();
            new_map.insert(ustr::ustr("PWD"), Val::String(pwd_str.clone()));
            new_map.insert(ustr::ustr("OLDPWD"), Val::String(old_pwd_str.clone()));
            vars.insert("env".to_string(), Val::Map(new_map));
        }
    }

    set_var("PWD", &pwd_str);
    set_var("OLDPWD", &old_pwd_str);

    Ok(())
}

pub fn val_to_display_string(val: &Val) -> String {
    match val {
        Val::List(list) => {
            let items: Vec<String> = list.iter().map(val_to_display_string).collect();
            format!("[{}]", items.join(", "))
        }
        other => other.to_text(),
    }
}

pub fn interpret_ansi_escapes(s: &str) -> (String, bool) {
    let mut res = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some(&'a') => {
                    res.push('\x07');
                    chars.next();
                }
                Some(&'b') => {
                    res.push('\x08');
                    chars.next();
                }
                Some(&'c') => {
                    chars.next();
                    return (res, true);
                }
                Some(&'e') | Some(&'E') => {
                    res.push('\x1B');
                    chars.next();
                }
                Some(&'f') => {
                    res.push('\x0C');
                    chars.next();
                }
                Some(&'n') => {
                    res.push('\n');
                    chars.next();
                }
                Some(&'r') => {
                    res.push('\r');
                    chars.next();
                }
                Some(&'t') => {
                    res.push('\t');
                    chars.next();
                }
                Some(&'v') => {
                    res.push('\x0B');
                    chars.next();
                }
                Some(&'\\') => {
                    res.push('\\');
                    chars.next();
                }
                Some(&'0') => {
                    chars.next(); // consume '0'
                    let mut oct_val = 0u32;
                    let mut count = 0;
                    let mut raw_escape = "\\0".to_string();
                    while count < 3 {
                        if let Some(&next_c) = chars.peek() {
                            if ('0'..='7').contains(&next_c) {
                                oct_val = oct_val * 8 + (next_c as u32 - '0' as u32);
                                raw_escape.push(next_c);
                                chars.next();
                                count += 1;
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    if let Some(ch) = char::from_u32(oct_val) {
                        res.push(ch);
                    } else {
                        res.push_str(&raw_escape);
                    }
                }
                Some(&'x') => {
                    chars.next(); // consume 'x'
                    let mut hex_val = 0u32;
                    let mut count = 0;
                    let mut raw_escape = "\\x".to_string();
                    while count < 2 {
                        if let Some(&next_c) = chars.peek() {
                            if let Some(digit) = next_c.to_digit(16) {
                                hex_val = hex_val * 16 + digit;
                                raw_escape.push(next_c);
                                chars.next();
                                count += 1;
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    if count > 0
                        && let Some(ch) = char::from_u32(hex_val)
                    {
                        res.push(ch);
                    } else {
                        res.push_str(&raw_escape);
                    }
                }
                Some(_) => {
                    res.push('\\');
                }
                None => {
                    res.push('\\');
                }
            }
        } else {
            res.push(c);
        }
    }
    (res, false)
}

pub fn get_home_dir() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("FSH_HOME") {
        return Some(PathBuf::from(h));
    }
    if let Ok(cfg) = std::env::var("FSH_CONFIG_DIR") {
        let p = PathBuf::from(cfg);
        if p.ends_with(".config/fsh")
            && let Some(gp) = p.parent().and_then(|parent| parent.parent())
        {
            return Some(gp.to_path_buf());
        }
        if let Some(parent) = p.parent() {
            return Some(parent.to_path_buf());
        }
        return Some(p);
    }
    std::env::var("HOME").ok().map(PathBuf::from)
}

pub fn get_user_home(username: &str) -> Option<PathBuf> {
    let username_c = CString::new(username).ok()?;
    // SAFETY: getpwnam returns a pointer to a static struct that is valid until
    // the next getpwnam/getpwuid call. No concurrent calls happen here.
    unsafe {
        let pwd = libc::getpwnam(username_c.as_ptr());
        if !pwd.is_null() {
            let home_c_str = std::ffi::CStr::from_ptr((*pwd).pw_dir);
            let home_str = home_c_str.to_string_lossy().to_string();
            Some(PathBuf::from(home_str))
        } else {
            None
        }
    }
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" || path == "~/" {
        if let Some(home) = get_home_dir() {
            return home;
        }
    } else if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = get_home_dir() {
            let mut p = home;
            p.push(stripped);
            return p;
        }
    } else if let Some(rest) = path.strip_prefix('~') {
        let slash_pos = rest.find('/');
        let (username, subpath) = match slash_pos {
            Some(pos) => (&rest[..pos], &rest[pos + 1..]),
            None => (rest, ""),
        };

        if let Some(home) = get_user_home(username) {
            let mut p = home;
            if !subpath.is_empty() {
                p.push(subpath);
            }
            return p;
        }
    }
    PathBuf::from(path)
}
