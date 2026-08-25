// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::RwLock;
use fshell_hash::FxHashMap;
use std::sync::{Arc, Mutex};

use crate::{BuiltinHandler, FallbackHandler, Stmt, Val};
use fshell_core::Param;

/// Variables, functions, builtins, aliases, and fallback handler.
#[derive(Clone)]
pub struct Scope {
    pub vars: Arc<RwLock<FxHashMap<String, Val>>>,
    #[allow(clippy::type_complexity)]
    pub fns: Arc<RwLock<FxHashMap<String, (Vec<Param>, Option<String>, Vec<Stmt>)>>>,
    pub builtins: Arc<RwLock<FxHashMap<String, BuiltinHandler>>>,
    pub aliases: Arc<RwLock<indexmap::IndexMap<String, String>>>,
    pub fallback: Arc<RwLock<Option<FallbackHandler>>>,
    pub local_vars: Option<Arc<RwLock<FxHashMap<String, Val>>>>,
    pub builtins_cache: Arc<Mutex<Option<Vec<String>>>>,
}

impl std::fmt::Debug for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scope")
            .field("vars", &self.vars)
            .field("fns", &self.fns)
            .field("builtins_cache", &self.builtins_cache)
            .field("local_vars", &self.local_vars)
            .finish()
    }
}
