// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_engine::Env;

/// POSIX `shift [n]` — rotate positional parameters left by n (default 1).
/// We store positional params in `env.vars["@"]` as Val::List and `env.vars["#"]` count.
pub fn shift_posix(env: &Env, n: usize) -> Result<(), String> {
    let mut vars = env.vars.write();
    let list = vars
        .get("@")
        .cloned()
        .unwrap_or(fshell_core::Val::List(Vec::new()));
    if let fshell_core::Val::List(items) = list {
        if n > items.len() {
            return Err(format!(
                "shift: shift count {} exceeds positional parameter count {}",
                n,
                items.len()
            ));
        }
        let remaining = items.into_iter().skip(n).collect::<Vec<_>>();
        let count = remaining.len();
        vars.insert("@".to_string(), fshell_core::Val::List(remaining.clone()));
        vars.insert("#".to_string(), fshell_core::Val::Int(count as i64));
        // Also update $1..$n
        for i in 1..=count {
            vars.insert(i.to_string(), remaining[i - 1].clone());
        }
        // Clear stale high indices
        for i in (count + 1)..=64 {
            if vars.contains_key(&i.to_string()) {
                vars.remove(&i.to_string());
            } else {
                break;
            }
        }
    }
    Ok(())
}

/// POSIX `set -- args` / `set -e` / `set +e` — set positional params and shell flags.
pub fn set_posix(env: &Env, args: &[String]) -> Result<Option<String>, String> {
    let mut vars = env.vars.write();
    let mut opts = env.options.write();

    if args.is_empty() {
        return Ok(None);
    }

    // If first arg is "--", the rest are positional params
    if args[0] == "--" {
        let positional: Vec<fshell_core::Val> = args[1..]
            .iter()
            .map(|s| fshell_core::Val::String(s.clone()))
            .collect();
        let count = positional.len();
        vars.insert("@".to_string(), fshell_core::Val::List(positional.clone()));
        vars.insert("#".to_string(), fshell_core::Val::Int(count as i64));
        for (i, v) in positional.into_iter().enumerate() {
            vars.insert((i + 1).to_string(), v);
        }
        return Ok(None);
    }

    // Handle set -e / set +e etc. (errexit, nounset, xtrace, etc.)
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-e" => opts.errexit = true,
            "+e" => opts.errexit = false,
            "-u" => opts.nounset = true,
            "+u" => opts.nounset = false,
            "-x" => opts.xtrace = true,
            "+x" => opts.xtrace = false,
            "-n" => opts.noexec = true,
            "+n" => opts.noexec = false,
            "-f" => opts.noglob = true,
            "+f" => opts.noglob = false,
            "--" => {
                // Remaining are positional
                let positional: Vec<fshell_core::Val> = args[i + 1..]
                    .iter()
                    .map(|s| fshell_core::Val::String(s.clone()))
                    .collect();
                let count = positional.len();
                vars.insert("@".to_string(), fshell_core::Val::List(positional.clone()));
                vars.insert("#".to_string(), fshell_core::Val::Int(count as i64));
                for (idx, v) in positional.into_iter().enumerate() {
                    vars.insert((idx + 1).to_string(), v);
                }
                break;
            }
            "-o" | "+o" => {
                let enable = args[i] == "-o";
                match args.get(i + 1).map(String::as_str) {
                    Some(name) if set_named_option(name, enable, &mut opts) => {
                        i += 1;
                    }
                    // `set -o` with no name prints the options; an unrecognised
                    // name is a usage error rather than a silent success, so a
                    // script cannot believe an option is on when it is not.
                    None => return Ok(Some(list_named_options(&opts, enable))),
                    Some(name) => return Err(format!("set: unknown option name '{name}'")),
                }
            }
            other if other.starts_with('-') || other.starts_with('+') => {
                // Unknown flag — ignore for compatibility
            }
            _ => break,
        }
        i += 1;
    }
    Ok(None)
}

/// Set one named shell option (`set -o name` / `set +o name`).
///
/// Returns false for a name this shell does not implement, which the caller
/// reports: POSIX names only a few, and quietly ignoring the rest is how a script
/// ends up running with weaker settings than it asked for.
fn set_named_option(name: &str, enable: bool, opts: &mut fshell_engine::ShellOptions) -> bool {
    match name {
        "errexit" => opts.errexit = enable,
        "nounset" => opts.nounset = enable,
        "xtrace" => opts.xtrace = enable,
        "noexec" => opts.noexec = enable,
        "noglob" => opts.noglob = enable,
        "pipefail" => opts.pipefail = enable,
        _ => return false,
    }
    true
}

/// Render the implemented options for `set -o` (readable) or `set +o` (a command
/// that would restore them), the way POSIX requires.
fn list_named_options(opts: &fshell_engine::ShellOptions, as_commands: bool) -> String {
    let options = [
        ("errexit", opts.errexit),
        ("nounset", opts.nounset),
        ("xtrace", opts.xtrace),
        ("noexec", opts.noexec),
        ("noglob", opts.noglob),
        ("pipefail", opts.pipefail),
    ];
    let mut out = String::new();
    for (name, enabled) in options {
        if as_commands {
            let sign = if enabled { "-o" } else { "+o" };
            out.push_str(&format!("set {sign} {name}\n"));
        } else {
            let state = if enabled { "on" } else { "off" };
            out.push_str(&format!("{name:<15}{state}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use fshell_core::Val;

    #[test]
    fn test_shift_basic() {
        let env = Env::for_command();
        set_posix(
            &env,
            &[
                "--".to_string(),
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
            ],
        )
        .unwrap();
        shift_posix(&env, 1).unwrap();
        assert_eq!(
            env.vars.read().get("1"),
            Some(&Val::String("b".to_string()))
        );
        assert_eq!(env.vars.read().get("#"), Some(&Val::Int(2)));
    }

    #[test]
    fn test_shift_by_two() {
        let env = Env::for_command();
        set_posix(
            &env,
            &[
                "--".to_string(),
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
            ],
        )
        .unwrap();
        shift_posix(&env, 2).unwrap();
        assert_eq!(
            env.vars.read().get("1"),
            Some(&Val::String("c".to_string()))
        );
    }

    #[test]
    fn test_set_flags() {
        let env = Env::for_command();
        set_posix(&env, &["-e".to_string()]).unwrap();
        assert!(env.options.read().errexit);
        set_posix(&env, &["+e".to_string()]).unwrap();
        assert!(!env.options.read().errexit);
    }
}
