// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::Val;
use fshell_engine::Env;
use std::collections::HashMap;

pub type FnDefTuple = (
    Vec<fshell_core::Param>,
    Option<String>,
    Vec<fshell_core::Stmt>,
);

/// Create a child Env for subshell `( ... )` execution.
///
/// The child shares capability and job state but gets isolated variable and
/// function state. `Env::clone` would share the same `Arc<RwLock<...>>` for
/// `vars`/`fns`, so we must allocate fresh Arcs.
pub fn fork_env_for_subshell(parent: &Env) -> Env {
    let mut snap_vars: HashMap<String, Val> = parent
        .vars
        .read()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if let Some(ref locals) = parent.local_vars {
        for (k, v) in locals.flatten() {
            snap_vars.insert(k, v);
        }
    }
    let snap_fns: HashMap<String, FnDefTuple> = parent
        .fns
        .read()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    // Clone parent but then replace vars/fns/cwd/traps/options with isolated copies
    let mut child = parent.clone();
    child.scope.vars =
        std::sync::Arc::new(fshell_core::RwLock::new(snap_vars.into_iter().collect()));
    // Lexical bindings were flattened into the snapshot. Keeping the parent's
    // frame chain would let child assignments update that chain instead.
    child.scope.local_vars = None;
    child.scope.fns = std::sync::Arc::new(fshell_core::RwLock::new(snap_fns.into_iter().collect()));
    child.scope.cwd = std::sync::Arc::new(fshell_core::RwLock::new(parent.cwd()));
    child.posix_traps =
        std::sync::Arc::new(fshell_core::RwLock::new(parent.posix_traps.read().clone()));
    child.posix_fns =
        std::sync::Arc::new(fshell_core::RwLock::new(parent.posix_fns.read().clone()));
    child.options = std::sync::Arc::new(fshell_core::RwLock::new(parent.options.read().clone()));
    child.descriptors =
        std::sync::Arc::new(fshell_core::RwLock::new(parent.descriptors.read().clone()));
    child
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fork_isolation() {
        let parent = Env::for_command();
        parent
            .vars
            .write()
            .insert("X".to_string(), Val::String("1".to_string()));
        let child = fork_env_for_subshell(&parent);
        child
            .vars
            .write()
            .insert("X".to_string(), Val::String("2".to_string()));
        assert_eq!(
            parent.vars.read().get("X"),
            Some(&Val::String("1".to_string()))
        );
        assert_eq!(
            child.vars.read().get("X"),
            Some(&Val::String("2".to_string()))
        );
    }

    #[test]
    fn test_fork_cwd_isolation() {
        let parent = Env::for_command();
        let initial = parent.cwd();
        let child = fork_env_for_subshell(&parent);
        child.set_cwd(std::path::PathBuf::from("/tmp"));
        assert_eq!(parent.cwd(), initial);
        assert_eq!(child.cwd(), std::path::PathBuf::from("/tmp"));
    }
}

#[cfg(test)]
mod subshell_bindings {
    use super::*;
    use fshell_core::RwLock;
    use std::sync::Arc;

    #[test]
    fn snapshot_uses_innermost_lexical_values_and_retains_outer_bindings() {
        let parent = Env::for_command();
        parent
            .vars
            .write()
            .insert("value".into(), Val::String("global".into()));
        let outer = parent.push_scope(Arc::new(RwLock::new(Default::default())));
        let frame = outer.local_vars.as_ref().expect("outer frame");
        frame.declare("value", Val::String("outer".into()));
        frame.declare("outer_only", Val::Int(7));
        let inner = outer.push_scope(Arc::new(RwLock::new(Default::default())));
        inner
            .local_vars
            .as_ref()
            .expect("inner frame")
            .declare("value", Val::String("inner".into()));
        let child = fork_env_for_subshell(&inner);
        assert!(
            child.local_vars.is_none(),
            "child must not retain shared lexical frames"
        );
        assert_eq!(
            child.vars.read().get("value"),
            Some(&Val::String("inner".into()))
        );
        assert_eq!(child.vars.read().get("outer_only"), Some(&Val::Int(7)));
    }

    #[test]
    fn child_assignment_does_not_update_the_parent_lexical_frame_or_global_binding() {
        let parent = Env::for_command();
        parent
            .vars
            .write()
            .insert("value".into(), Val::String("global".into()));
        let scoped = parent.push_scope(Arc::new(RwLock::new(Default::default())));
        scoped
            .local_vars
            .as_ref()
            .expect("parent frame")
            .declare("value", Val::String("local".into()));
        let child = fork_env_for_subshell(&scoped);
        child.set_shell_var("value", Val::String("child".into()));
        assert_eq!(
            child.vars.read().get("value"),
            Some(&Val::String("child".into()))
        );
        assert_eq!(
            scoped
                .local_vars
                .as_ref()
                .expect("parent frame")
                .get("value"),
            Some(Val::String("local".into()))
        );
        assert_eq!(
            parent.vars.read().get("value"),
            Some(&Val::String("global".into()))
        );
    }
}
