// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Builtins that share a name with a standard utility.
//!
//! These builtins give structured results for the invocations they implement,
//! and every other invocation belongs to the utility on PATH: shadowing must
//! never take an interface away (`sort -n`, `cat -v`, `ps aux`, `tail +2`,
//! `env FOO=1 cmd`). Each predicate below is the builtin's own argument
//! grammar, so the two cannot drift apart.

use fshell_core::Val;
use fshell_engine::Env;

fn is_option(arg: &Val) -> bool {
    matches!(arg, Val::String(s) if s.starts_with('-') && s != "-")
}

/// Operands only: no option at all.
fn operands_only(args: &[Val]) -> bool {
    !args.iter().any(is_option)
}

fn cat(args: &[Val]) -> bool {
    operands_only(args)
}

fn sort(args: &[Val]) -> bool {
    crate::cmd::sort::parse_sort_args(args, None).is_ok()
}

/// `-n N`, `-nN` and `-N` line counts; the parser takes any other word as a
/// file name, so other options (`-c`) are checked for here.
fn head(args: &[Val]) -> bool {
    let mut words = args.iter();
    while let Some(arg) = words.next() {
        let Val::String(word) = arg else { continue };
        if word == "-n" {
            words.next();
        } else if is_option(arg) {
            let count = word.strip_prefix("-n").unwrap_or(&word[1..]);
            if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
        }
    }
    crate::cmd::fs::parse_head_tail_args(args).is_ok()
}

/// `tail +N` (from line N) is the utility's; the builtin counts from the end.
fn tail(args: &[Val]) -> bool {
    head(args)
        && !args
            .iter()
            .any(|a| matches!(a, Val::String(s) if s.starts_with('+')))
}

fn uniq(args: &[Val]) -> bool {
    crate::cmd::fs::parse_uniq_args(args, None).is_ok()
}

fn mkdir(args: &[Val]) -> bool {
    args.iter()
        .all(|a| !is_option(a) || matches!(a, Val::String(s) if s == "-p" || s == "--parents"))
}

fn diff(args: &[Val]) -> bool {
    args.len() == 2 && operands_only(args)
}

/// The builtin prints the environment; running a command in a modified one
/// is the utility's.
fn env(args: &[Val]) -> bool {
    args.is_empty()
}

/// One duration (`sleep 2`, `sleep 50ms`).
fn sleep(args: &[Val]) -> bool {
    args.len() == 1 && operands_only(args)
}

/// No arguments, `-a`/`-A`/`-u` (all users) and `-pPID`; BSD-style `aux`,
/// `-ef`, `-o` formats and the rest are the utility's.
fn ps(args: &[Val]) -> bool {
    args.iter().all(|a| match a {
        Val::String(s) => {
            matches!(s.as_str(), "-a" | "-A" | "-u")
                || s.strip_prefix("-p")
                    .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
        }
        _ => false,
    })
}

/// A builtin's argument grammar: whether it implements an invocation.
type Accepts = fn(&[Val]) -> bool;

const SHADOWING: &[(&str, Accepts)] = &[
    ("cat", cat),
    ("diff", diff),
    ("env", env),
    ("head", head),
    ("mkdir", mkdir),
    ("ps", ps),
    ("sleep", sleep),
    ("sort", sort),
    ("tail", tail),
    ("touch", operands_only),
    ("uniq", uniq),
    ("which", operands_only),
];

/// Declare, for each registered shadowing builtin, which invocations it owns.
pub(crate) fn register(env: &Env) {
    for (name, accepts) in SHADOWING {
        if let Some(handler) = env.get_builtin(name) {
            let mut contract = env.builtin_contract(name);
            contract.accepts = Some(*accepts);
            env.register_builtin_with_contract(name, handler, contract);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(args: &[&str]) -> Vec<Val> {
        args.iter().map(|a| Val::String((*a).to_string())).collect()
    }

    #[test]
    fn builtins_keep_what_they_implement_and_hand_over_the_rest() {
        assert!(sort(&words(&["-r"])));
        assert!(!sort(&words(&["-n"])));
        assert!(!sort(&words(&["file.txt"])));
        assert!(cat(&words(&["a", "-"])));
        assert!(!cat(&words(&["-n", "a"])));
        assert!(head(&words(&["-n", "3"])));
        assert!(!head(&words(&["-c", "3"])));
        assert!(!tail(&words(&["+2", "f"])));
        assert!(ps(&words(&["-p123"])));
        assert!(!ps(&words(&["aux"])));
        assert!(env(&[]));
        assert!(!env(&words(&["FOO=1", "cmd"])));
    }
}
