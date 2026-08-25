// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::RwLock;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex};

use crate::Job;

/// Job management, foreground process tracking, and signal flags.
pub struct JobControl {
    pub jobs: Arc<RwLock<fshell_hash::FxHashMap<i32, Job>>>,
    pub fg_mutex: Arc<Mutex<Option<usize>>>,
    pub fg_cvar: Arc<Condvar>,
    pub sigint_pending: Arc<AtomicBool>,
    pub cancellation: Arc<AtomicBool>,
}

impl std::fmt::Debug for JobControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobControl")
            .field("jobs", &self.jobs)
            .field("fg_mutex", &self.fg_mutex)
            .field("fg_cvar", &self.fg_cvar)
            .finish()
    }
}

impl Clone for JobControl {
    fn clone(&self) -> Self {
        Self {
            jobs: self.jobs.clone(),
            fg_mutex: self.fg_mutex.clone(),
            fg_cvar: self.fg_cvar.clone(),
            sigint_pending: self.sigint_pending.clone(),
            cancellation: self.cancellation.clone(),
        }
    }
}
