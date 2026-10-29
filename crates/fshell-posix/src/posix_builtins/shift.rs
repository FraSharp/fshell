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
        crate::positional::replace(&mut vars, remaining);
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
        crate::positional::replace(&mut vars, positional);
        return Ok(None);
    }

    // Handle set -e / set +e etc. (errexit, nounset, xtrace, etc.)
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-e" => opts.errexit = true,
            "+e" => opts.errexit = false,
            "-u" => env
                .posix_nounset
                .store(true, std::sync::atomic::Ordering::Relaxed),
            "+u" => env
                .posix_nounset
                .store(false, std::sync::atomic::Ordering::Relaxed),
            "-x" => opts.xtrace = true,
            "+x" => opts.xtrace = false,
            "-n" => opts.noexec = true,
            "+n" => opts.noexec = false,
            "-f" => opts.noglob = true,
            "+f" => opts.noglob = false,
            "-C" => opts.noclobber = true,
            "+C" => opts.noclobber = false,
            "--" => {
                // Remaining are positional
                let positional: Vec<fshell_core::Val> = args[i + 1..]
                    .iter()
                    .map(|s| fshell_core::Val::String(s.clone()))
                    .collect();
                crate::positional::replace(&mut vars, positional);
                break;
            }
            "-o" | "+o" => {
                let enable = args[i] == "-o";
                match args.get(i + 1).map(String::as_str) {
                    Some(name) if set_named_option(name, enable, env, &mut opts) => {
                        i += 1;
                    }
                    // `set -o` with no name prints the options; an unrecognised
                    // name is a usage error rather than a silent success, so a
                    // script cannot believe an option is on when it is not.
                    None => {
                        let nounset = env.posix_nounset.load(std::sync::atomic::Ordering::Relaxed);
                        return Ok(Some(list_named_options(&opts, nounset, enable)));
                    }
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
fn set_named_option(
    name: &str,
    enable: bool,
    env: &Env,
    opts: &mut fshell_engine::ShellOptions,
) -> bool {
    match name {
        "errexit" => opts.errexit = enable,
        "nounset" => {
            // The POSIX engine keeps its own `-u` state; `ShellOptions::nounset`
            // is native's, and its default is not a POSIX setting.
            env.posix_nounset
                .store(enable, std::sync::atomic::Ordering::Relaxed);
        }
        "xtrace" => opts.xtrace = enable,
        "noexec" => opts.noexec = enable,
        "noglob" => opts.noglob = enable,
        "noclobber" => opts.noclobber = enable,
        "pipefail" => opts.pipefail = enable,
        _ => return false,
    }
    true
}

/// Render the implemented options for `set -o` (readable) or `set +o` (a command
/// that would restore them), the way POSIX requires.
fn list_named_options(
    opts: &fshell_engine::ShellOptions,
    nounset: bool,
    as_commands: bool,
) -> String {
    let options = [
        ("errexit", opts.errexit),
        ("nounset", nounset),
        ("xtrace", opts.xtrace),
        ("noexec", opts.noexec),
        ("noglob", opts.noglob),
        ("noclobber", opts.noclobber),
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
#[expect(
    clippy::unwrap_used,
    reason = "Test fixture preconditions are assertions"
)]
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

#[cfg(test)]
mod argument_updates {
    use super::*;
    use fshell_core::Val;

    fn wide_arguments() -> Vec<String> {
        std::iter::once("--".into())
            .chain((1..=80).map(|index| format!("arg-{index}")))
            .collect()
    }

    #[test]
    fn shift_rebuilds_numbered_bindings_without_a_fixed_index_limit() {
        let env = Env::for_command();
        set_posix(&env, &wide_arguments()).expect("wide arguments");
        shift_posix(&env, 79).expect("shift");
        let bindings: Vec<_> = env
            .vars
            .read()
            .iter()
            .filter(|(name, _)| {
                name.as_str() == "@"
                    || name.as_str() == "#"
                    || name.bytes().all(|byte| byte.is_ascii_digit())
            })
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let expected: std::collections::HashMap<_, _> = [
            (
                "@".to_string(),
                Val::List(vec![Val::String("arg-80".into())]),
            ),
            ("#".to_string(), Val::Int(1)),
            ("1".to_string(), Val::String("arg-80".into())),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            bindings
                .into_iter()
                .collect::<std::collections::HashMap<_, _>>(),
            expected
        );
    }

    #[test]
    fn set_clears_stale_arguments_with_and_without_option_prefixes() {
        for args in [vec!["--".into()], vec!["-e".into(), "--".into()]] {
            let env = Env::for_command();
            set_posix(&env, &wide_arguments()).expect("wide arguments");
            set_posix(&env, &args).expect("empty arguments");
            assert_eq!(env.vars.read().get("@"), Some(&Val::List(Vec::new())));
            assert_eq!(env.vars.read().get("#"), Some(&Val::Int(0)));
            assert!(
                !env.vars
                    .read()
                    .keys()
                    .any(|name| name.bytes().all(|byte| byte.is_ascii_digit()))
            );
        }
    }

    #[test]
    fn invalid_shift_preserves_every_binding() {
        let env = Env::for_command();
        set_posix(&env, &["--".into(), "original".into()]).expect("arguments");
        let before = env.vars.read().clone();
        assert!(shift_posix(&env, 2).is_err());
        assert_eq!(*env.vars.read(), before);
    }
}
