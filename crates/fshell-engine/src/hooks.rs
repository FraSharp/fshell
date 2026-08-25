// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::RwLock;
use std::sync::Arc;

/// Hook system: maps hook event names to lists of function names.
pub struct Hooks {
    pub registry: Arc<RwLock<fshell_hash::FxHashMap<String, Vec<String>>>>,
}

impl Clone for Hooks {
    fn clone(&self) -> Self {
        Self {
            registry: self.registry.clone(),
        }
    }
}

impl std::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hooks")
            .field("registry", &self.registry)
            .finish()
    }
}
