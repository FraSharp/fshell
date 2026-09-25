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
//!
//! A pipeline's own bookkeeping lives in [`PipelineOutcomes`] instead of in a
//! single shared slot: one ledger per pipeline execution, one slot per stage.
//! What one slot could not say — `false | true` under `pipefail`, or the failure
//! of a command whose diagnostic was redirected away — becomes ordinary
//! per-stage data.

use fshell_core::RwLock;
use fshell_core::diagnostic::FshDiag;
use std::sync::Arc;

/// The part of a pipeline an `Env` records into.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Slot {
    /// A command stage, by position in the pipeline.
    Stage(usize),
    /// The pipeline's own output boundary — the `Env` that drives it, which
    /// records only when it cannot route its output.
    Boundary,
}

/// One stage's result, recorded by the stage itself and read by the pipeline's
/// finalizer.
#[derive(Clone, Debug, Default)]
pub(crate) struct StageOutcome {
    status: Option<i64>,
    failure: Option<crate::PipelineFailure>,
}

impl StageOutcome {
    /// The status the stage finished with, or `None` when it never recorded one
    /// (a stage that passed its input through and said nothing).
    pub(crate) fn status(&self) -> Option<i64> {
        self.status
    }

    /// How the stage failed, if it did.
    pub(crate) fn failure(&self) -> Option<&crate::PipelineFailure> {
        self.failure.as_ref()
    }
}

/// The outcomes of one pipeline execution: one slot per command stage, plus one
/// for the pipeline's own output boundary.
///
/// A stage owns exactly one slot, so no stage can overwrite another's status and
/// no scheduling order can change the result. The stage records it itself, which
/// is what makes a failure independent of diagnostic transport: redirection
/// decides where a diagnostic's *text* goes, and nothing it does can change
/// whether the command failed.
#[derive(Debug, Default)]
pub(crate) struct PipelineOutcomes {
    /// How many command stages the pipeline has. Only the planner knows, so the
    /// ledger is told rather than guessing from which stages happen to report.
    stages: RwLock<usize>,
    slots: RwLock<Vec<StageOutcome>>,
    boundary: RwLock<StageOutcome>,
}

impl PipelineOutcomes {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Fix the pipeline's stage count and give it that many slots.
    pub(crate) fn set_stages(&self, stages: usize) {
        *self.stages.write() = stages;
        self.slots.write().resize(stages, StageOutcome::default());
    }

    /// Record a status for `slot`.
    pub(crate) fn record_status(&self, slot: Slot, status: i64) {
        self.with_slot(slot, |outcome| outcome.status = Some(status));
    }

    /// Record a failure for `slot`: its class decides what the statement does
    /// next, and its status is what the stage finished with.
    pub(crate) fn record_failure(&self, slot: Slot, failure: crate::PipelineFailure) {
        let status = failure.status();
        self.with_slot(slot, |outcome| {
            outcome.status = Some(status);
            outcome.failure = Some(failure);
        });
    }

    /// The status recorded for `slot`, if anything recorded one.
    pub(crate) fn recorded_status(&self, slot: Slot) -> Option<i64> {
        self.read_slot(slot).and_then(|outcome| outcome.status)
    }

    /// The command stages, in pipeline order.
    pub(crate) fn command_stages(&self) -> Vec<StageOutcome> {
        let stages = *self.stages.read();
        let slots = self.slots.read();
        slots[..stages.min(slots.len())].to_vec()
    }

    /// What the pipeline's output boundary recorded, if anything.
    pub(crate) fn boundary_outcome(&self) -> StageOutcome {
        self.boundary.read().clone()
    }

    fn with_slot(&self, slot: Slot, write: impl FnOnce(&mut StageOutcome)) {
        match slot {
            Slot::Stage(index) => {
                if let Some(outcome) = self.slots.write().get_mut(index) {
                    write(outcome);
                }
            }
            Slot::Boundary => write(&mut self.boundary.write()),
        }
    }

    fn read_slot(&self, slot: Slot) -> Option<StageOutcome> {
        match slot {
            Slot::Stage(index) => self.slots.read().get(index).cloned(),
            Slot::Boundary => Some(self.boundary.read().clone()),
        }
    }
}

/// Mutable status belonging to one logical shell invocation.
///
/// Two statuses, deliberately kept apart rather than sharing one slot:
///
/// * `exit_code` is the status of the last *completed* statement — what `$?`
///   expands to. It is written only when a statement finishes, so a value read
///   while a command's words are being expanded always belongs to a finished
///   command.
/// * the pipeline in flight accumulates into its own [`PipelineOutcomes`], which
///   the statement's finalizer reduces to a status. `$?` must never read that:
///   while `echo "$?"` expands, it holds whatever the executor happens to be
///   doing, which is precisely the bug that reading it caused.
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
