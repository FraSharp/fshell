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
pub fn set_posix(env: &Env, args: &[String]) -> Result<(), String> {
    let mut vars = env.vars.write();
    let mut opts = env.options.write();

    if args.is_empty() {
        return Ok(());
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
        return Ok(());
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
            "-f" => opts.nullglob = false, // disable glob = -f is noglob
            "+f" => opts.nullglob = true,
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
            other if other.starts_with('-') || other.starts_with('+') => {
                // Unknown flag — ignore for compatibility
            }
            _ => break,
        }
        i += 1;
    }
    Ok(())
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
