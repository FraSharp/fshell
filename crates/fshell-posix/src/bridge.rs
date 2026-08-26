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
        for (k, v) in locals.read().iter() {
            snap_vars.insert(k.clone(), v.clone());
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
    child.scope.fns = std::sync::Arc::new(fshell_core::RwLock::new(snap_fns.into_iter().collect()));
    child.scope.cwd = std::sync::Arc::new(fshell_core::RwLock::new(parent.cwd()));
    child.posix_traps =
        std::sync::Arc::new(fshell_core::RwLock::new(parent.posix_traps.read().clone()));
    child.options = std::sync::Arc::new(fshell_core::RwLock::new(parent.options.read().clone()));
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
