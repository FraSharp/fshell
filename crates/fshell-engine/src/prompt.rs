// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::RwLock;
use fshell_core::diagnostic::FshDiag;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime};

use crate::PendingSuggestion;

/// Prompt display state, exit code, duration, DYM state, and last error diagnostic.
pub struct Prompt {
    #[allow(clippy::type_complexity)]
    pub git_branch_cache: Arc<RwLock<Option<(Instant, Vec<String>, SystemTime)>>>,
    pub last_exit_code: Arc<RwLock<i64>>,
    pub last_duration: Arc<RwLock<Duration>>,
    pub last_error: Arc<RwLock<Option<FshDiag>>>,
    pub alias_suppressed: Arc<AtomicBool>,
    pub pending_suggestion: Arc<RwLock<Option<PendingSuggestion>>>,
    pub suggestion_deferred: Arc<AtomicBool>,
    pub edit_suggestion: Arc<RwLock<Option<String>>>,
}

impl Clone for Prompt {
    fn clone(&self) -> Self {
        Self {
            git_branch_cache: self.git_branch_cache.clone(),
            last_exit_code: self.last_exit_code.clone(),
            last_duration: self.last_duration.clone(),
            last_error: self.last_error.clone(),
            alias_suppressed: self.alias_suppressed.clone(),
            pending_suggestion: self.pending_suggestion.clone(),
            suggestion_deferred: self.suggestion_deferred.clone(),
            edit_suggestion: self.edit_suggestion.clone(),
        }
    }
}

impl std::fmt::Debug for Prompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prompt")
            .field("git_branch_cache", &self.git_branch_cache)
            .field("last_exit_code", &self.last_exit_code)
            .field("last_duration", &self.last_duration)
            .field("last_error", &self.last_error)
            .field("alias_suppressed", &self.alias_suppressed)
            .field("pending_suggestion", &self.pending_suggestion)
            .field("edit_suggestion", &self.edit_suggestion)
            .finish()
    }
}
