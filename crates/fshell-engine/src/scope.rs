// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::lock::{Mutex, RwLock};
use fshell_hash::FxHashMap;
use std::sync::Arc;

use crate::{
    AsyncBuiltinHandler, AsyncFallbackHandler, BuiltinHandler, FallbackHandler, Stmt, Val,
};
use fshell_core::Param;

pub type ConfigTuiHandler = Arc<dyn Fn(&crate::Env) -> Result<(), String> + Send + Sync>;

/// A lexical frame of local variables, linked to its enclosing frame.
///
/// Lookups walk outward through the chain, so a loop body or a pipeline stage
/// still sees the function parameters of the scope it runs in. Writes update the
/// innermost frame that already holds the name, which keeps a parameter mutated
/// inside a block visible after the block.
#[derive(Clone, Debug)]
pub struct LocalScope {
    frame: Arc<RwLock<FxHashMap<String, Val>>>,
    parent: Option<Arc<LocalScope>>,
}

impl LocalScope {
    /// A root frame with no enclosing scope.
    pub fn new(frame: Arc<RwLock<FxHashMap<String, Val>>>) -> Self {
        Self {
            frame,
            parent: None,
        }
    }

    /// A frame nested inside `parent`.
    pub fn child(frame: Arc<RwLock<FxHashMap<String, Val>>>, parent: Arc<LocalScope>) -> Self {
        Self {
            frame,
            parent: Some(parent),
        }
    }

    /// The innermost frame, where declarations land.
    pub fn frame(&self) -> &Arc<RwLock<FxHashMap<String, Val>>> {
        &self.frame
    }

    /// Look up a binding, walking outward through enclosing frames.
    pub fn get(&self, name: &str) -> Option<Val> {
        if let Some(v) = self.frame.read().get(name) {
            return Some(v.clone());
        }
        self.parent.as_ref().and_then(|p| p.get(name))
    }

    /// Whether any frame holds `name`.
    pub fn contains(&self, name: &str) -> bool {
        let here = self.frame.read().contains_key(name);
        here || self.parent.as_ref().is_some_and(|p| p.contains(name))
    }

    /// Update the innermost frame that already holds `name`. Returns `false` if
    /// no frame holds it.
    pub fn update(&self, name: &str, val: Val) -> bool {
        let here = self.frame.read().contains_key(name);
        if here {
            self.frame.write().insert(name.to_string(), val);
            true
        } else if let Some(p) = &self.parent {
            p.update(name, val)
        } else {
            false
        }
    }

    /// Flatten the whole chain outermost-first, so inner bindings win when
    /// inserted into a map.
    pub fn flatten(&self) -> Vec<(String, Val)> {
        let mut out = Vec::new();
        self.collect_into(&mut out);
        out
    }

    fn collect_into(&self, out: &mut Vec<(String, Val)>) {
        if let Some(p) = &self.parent {
            p.collect_into(out);
        }
        for (k, v) in self.frame.read().iter() {
            out.push((k.clone(), v.clone()));
        }
    }

    /// Declare `name` in the innermost frame (shadows any outer binding).
    pub fn declare(&self, name: &str, val: Val) {
        self.frame.write().insert(name.to_string(), val);
    }

    /// Remove `name` from the innermost frame that holds it.
    pub fn remove(&self, name: &str) -> bool {
        let here = self.frame.read().contains_key(name);
        if here {
            self.frame.write().remove(name);
            true
        } else if let Some(p) = &self.parent {
            p.remove(name)
        } else {
            false
        }
    }
}

/// Variables, functions, builtins, aliases, fallback handler, and interactive config TUI handler.
#[derive(Clone)]
pub struct Scope {
    pub vars: Arc<RwLock<FxHashMap<String, Val>>>,
    #[allow(clippy::type_complexity)]
    pub fns: Arc<RwLock<FxHashMap<String, (Vec<Param>, Option<String>, Vec<Stmt>)>>>,
    pub builtins: Arc<RwLock<FxHashMap<String, BuiltinHandler>>>,
    pub async_builtins: Arc<RwLock<FxHashMap<String, AsyncBuiltinHandler>>>,
    pub aliases: Arc<RwLock<indexmap::IndexMap<String, String>>>,
    pub fallback: Arc<RwLock<Option<FallbackHandler>>>,
    pub async_fallback: Arc<RwLock<Option<AsyncFallbackHandler>>>,
    pub config_tui_handler: Arc<RwLock<Option<ConfigTuiHandler>>>,
    pub local_vars: Option<Arc<LocalScope>>,
    pub builtins_cache: Arc<Mutex<Option<Vec<String>>>>,
    pub cwd: Arc<RwLock<std::path::PathBuf>>,
}

impl std::fmt::Debug for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scope")
            .field("vars", &self.vars)
            .field("fns", &self.fns)
            .field("builtins_cache", &self.builtins_cache)
            .field("local_vars", &self.local_vars)
            .field("cwd", &self.cwd)
            .finish()
    }
}
