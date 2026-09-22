// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Per-invocation state for shell execution.
//!
//! `Env` contains both persistent shell state and the state of the command
//! currently being evaluated.  Pipeline stages are cloned from `Env`, so
//! command status must not live in the persistent prompt state: a stage that
//! finishes late must not rewrite the prompt for a later command.  The
//! invocation owner creates one `ExecutionState`, and all clones belonging to
//! that invocation share it.

use fshell_core::RwLock;
use fshell_core::diagnostic::FshDiag;

/// Mutable status belonging to one logical shell invocation.
#[derive(Debug)]
pub struct ExecutionState {
    exit_code: RwLock<i64>,
    last_error: RwLock<Option<FshDiag>>,
}

impl ExecutionState {
    pub fn new(exit_code: i64) -> Self {
        Self {
            exit_code: RwLock::new(exit_code),
            last_error: RwLock::new(None),
        }
    }

    pub fn exit_code(&self) -> i64 {
        *self.exit_code.read()
    }

    pub fn set_exit_code(&self, code: i64) {
        *self.exit_code.write() = code;
    }

    pub fn last_error(&self) -> Option<FshDiag> {
        self.last_error.read().clone()
    }

    pub fn set_last_error(&self, diag: FshDiag) {
        *self.last_error.write() = Some(diag);
    }

    pub fn clear_last_error(&self) {
        *self.last_error.write() = None;
    }
}
