// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Positional binding replacement and lexical call ownership.

use fshell_core::Val;
use fshell_engine::Env;
use std::collections::HashMap;
use std::hash::BuildHasher;

fn is_binding(name: &str) -> bool {
    name == "@"
        || name == "#"
        || (name
            .as_bytes()
            .first()
            .is_some_and(|byte| matches!(byte, b'1'..=b'9'))
            && name.bytes().all(|byte| byte.is_ascii_digit()))
}

pub(crate) fn replace<S: BuildHasher>(vars: &mut HashMap<String, Val, S>, arguments: Vec<Val>) {
    vars.retain(|name, _| !is_binding(name));
    vars.insert("#".into(), Val::Int(arguments.len() as i64));
    for (index, value) in arguments.iter().enumerate() {
        vars.insert((index + 1).to_string(), value.clone());
    }
    vars.insert("@".into(), Val::List(arguments));
}

/// Restore the original values and absence, including sparse/wide numeric keys.
/// Drop covers errors, control transfers and cancellation of an awaiting call.
pub(crate) struct Scope<'a> {
    env: &'a Env,
    saved: Vec<(String, Val)>,
}

impl<'a> Scope<'a> {
    pub(crate) fn new(env: &'a Env, arguments: &[String]) -> Self {
        let mut vars = env.vars.write();
        let saved = vars
            .iter()
            .filter(|(name, _)| is_binding(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        replace(
            &mut vars,
            arguments.iter().cloned().map(Val::String).collect(),
        );
        Self { env, saved }
    }
}

impl Drop for Scope<'_> {
    fn drop(&mut self) {
        let mut vars = self.env.vars.write();
        vars.retain(|name, _| !is_binding(name));
        vars.extend(self.saved.drain(..));
    }
}

#[cfg(test)]
mod ownership {
    use super::*;
    use std::future::Future;
    use std::task::{Context, Waker};

    #[test]
    fn scope_restores_types_and_sparse_wide_bindings_without_undoing_other_effects() {
        let env = Env::for_command();
        {
            let mut vars = env.vars.write();
            vars.insert("@".into(), Val::List(vec![Val::Int(7)]));
            vars.insert("#".into(), Val::Int(1));
            vars.insert("1".into(), Val::Int(7));
            vars.insert("80".into(), Val::Bool(false));
            vars.insert("0".into(), Val::String("script".into()));
            vars.insert("outside".into(), Val::Int(1));
        }
        let before = env.vars.read().clone();
        {
            let _scope = Scope::new(&env, &["inner".into()]);
            assert_eq!(env.vars.read().get("1"), Some(&Val::String("inner".into())));
            assert!(!env.vars.read().contains_key("80"));
            env.vars.write().insert("outside".into(), Val::Int(2));
        }
        let mut expected = before;
        expected.insert("outside".into(), Val::Int(2));
        assert_eq!(*env.vars.read(), expected);
    }

    #[test]
    fn empty_argument_scope_is_zero_arguments_and_restores_initial_absence() {
        let env = Env::for_command();
        let before = env.vars.read().clone();
        {
            let _scope = Scope::new(&env, &[]);
            assert_eq!(env.vars.read().get("@"), Some(&Val::List(Vec::new())));
            assert_eq!(env.vars.read().get("#"), Some(&Val::Int(0)));
            assert!(!env.vars.read().contains_key("1"));
        }
        assert_eq!(*env.vars.read(), before);
    }

    #[test]
    fn replacement_removes_every_numbered_binding_but_not_script_or_noncanonical_names() {
        let mut vars = HashMap::new();
        for name in ["1", "65", "80", "999999999999999999999999999"] {
            vars.insert(name.into(), Val::Int(7));
        }
        vars.insert("0".into(), Val::String("script".into()));
        vars.insert("01".into(), Val::Int(9));
        replace(&mut vars, Vec::new());
        assert_eq!(
            vars,
            HashMap::from([
                ("0".into(), Val::String("script".into())),
                ("01".into(), Val::Int(9)),
                ("@".into(), Val::List(Vec::new())),
                ("#".into(), Val::Int(0))
            ])
        );
    }

    #[test]
    fn nested_scopes_restore_the_immediate_caller_before_the_outer_caller() {
        let env = Env::for_command();
        let before = env.vars.read().clone();
        {
            let _outer = Scope::new(&env, &["outer".into()]);
            let outer = env.vars.read().clone();
            {
                let _inner = Scope::new(&env, &["inner".into(), "extra".into()]);
            }
            assert_eq!(*env.vars.read(), outer);
        }
        assert_eq!(*env.vars.read(), before);
    }

    #[tokio::test]
    async fn dropping_an_awaiting_scope_restores_its_caller() {
        let env = Env::for_command();
        let before = env.vars.read().clone();
        let mut call = Box::pin(async {
            let _scope = Scope::new(&env, &["inner".into()]);
            std::future::pending::<()>().await;
        });
        assert!(
            call.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        assert_eq!(env.vars.read().get("1"), Some(&Val::String("inner".into())));
        drop(call);
        assert_eq!(*env.vars.read(), before);
    }
}
