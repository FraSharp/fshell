// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::lock::{Condvar, Mutex, RwLock};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::Job;

/// Shared process-group and terminal ownership for the external processes in
/// one foreground pipeline. The pipeline executor owns the context; bridge
/// stages use `launch` to join the same group while the executor restores the
/// terminal after every stage has completed.
pub struct PipelineJobContext {
    pub launch: Mutex<Option<i32>>,
    pub job_id: AtomicUsize,
    pub pids: Mutex<Vec<i32>>,
    expected_spawns: usize,
    spawned: AtomicUsize,
    launch_notify: tokio::sync::Notify,
    /// Set once the executor has finished dispatching this pipeline's stages.
    /// Releases every waiter even if the expected spawn count was never reached
    /// (a stage returned before spawning, the dispatch loop broke on
    /// cancellation, or the executor unwound early). This makes the barrier
    /// impossible to deadlock: it always has a guaranteed release.
    dispatch_complete: AtomicBool,
    shell_pgid: i32,
    owns_terminal: bool,
    finished: AtomicBool,
}

impl PipelineJobContext {
    pub fn new(shell_pgid: i32, owns_terminal: bool, expected_spawns: usize) -> Self {
        Self {
            launch: Mutex::new(None),
            job_id: AtomicUsize::new(0),
            pids: Mutex::new(Vec::new()),
            expected_spawns,
            spawned: AtomicUsize::new(0),
            launch_notify: tokio::sync::Notify::new(),
            dispatch_complete: AtomicBool::new(false),
            shell_pgid,
            owns_terminal,
            finished: AtomicBool::new(false),
        }
    }

    pub fn register_spawn_attempt(&self) {
        self.spawned.fetch_add(1, Ordering::AcqRel);
        self.launch_notify.notify_waiters();
    }

    /// Releases every stage blocked in [`Self::wait_for_launch`]. Idempotent.
    /// Called once the executor stops scheduling stages for this pipeline.
    pub fn close(&self) {
        self.dispatch_complete.store(true, Ordering::Release);
        self.launch_notify.notify_waiters();
    }

    fn launch_satisfied(&self) -> bool {
        self.dispatch_complete.load(Ordering::Acquire)
            || self.spawned.load(Ordering::Acquire) >= self.expected_spawns
    }

    pub async fn wait_for_launch(&self) {
        loop {
            if self.launch_satisfied() {
                return;
            }
            let notified = self.launch_notify.notified();
            tokio::pin!(notified);
            // Register with the notifier before re-checking, so a `close` or a
            // spawn that lands between the check above and the await is not
            // lost (which would hang the stage forever).
            notified.as_mut().enable();
            if self.launch_satisfied() {
                return;
            }
            notified.await;
        }
    }

    pub fn finish(&self, env: &crate::Env) {
        if self.owns_terminal
            && let Some(job_id) = env.foreground_job()
        {
            let _ = env.clear_foreground(job_id);
        }
        self.restore_terminal();
    }

    fn restore_terminal(&self) {
        if self.owns_terminal && !self.finished.swap(true, Ordering::AcqRel) {
            // SAFETY: ignore SIGTTOU while returning the controlling terminal
            // to the shell process group after the pipeline has joined.
            unsafe {
                libc::signal(libc::SIGTTOU, libc::SIG_IGN);
                libc::tcsetpgrp(libc::STDIN_FILENO, self.shell_pgid);
                libc::signal(libc::SIGTTOU, libc::SIG_DFL);
            }
        }
    }
}

impl Drop for PipelineJobContext {
    fn drop(&mut self) {
        self.restore_terminal();
    }
}

/// Closes a [`PipelineJobContext`] when dropped. The pipeline executor holds
/// one for the duration of stage dispatch, so any early return (a dispatch
/// error, an aborted loop) still releases stages blocked in
/// [`PipelineJobContext::wait_for_launch`].
pub struct PipelineJobBarrierGuard(Option<Arc<PipelineJobContext>>);

impl PipelineJobBarrierGuard {
    pub fn new(job: Option<Arc<PipelineJobContext>>) -> Self {
        Self(job)
    }
}

impl Drop for PipelineJobBarrierGuard {
    fn drop(&mut self) {
        if let Some(job) = &self.0 {
            job.close();
        }
    }
}

/// Job management, foreground process tracking, and signal flags.
pub struct JobControl {
    pub jobs: Arc<RwLock<fshell_hash::FxHashMap<i32, Job>>>,
    pub fg_mutex: Arc<Mutex<Option<usize>>>,
    pub fg_cvar: Arc<Condvar>,
    pub sigint_pending: Arc<AtomicBool>,
    pub cancellation: Arc<AtomicBool>,
    /// Set by `exit` executed inside a pipeline task (e.g. a user function,
    /// which runs in a spawned task and cannot return `Flow::Exit` directly).
    /// The statement driver observes it after the pipeline completes and turns
    /// it into `Flow::Exit`.
    pub exit_request: Arc<Mutex<Option<i32>>>,
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
            exit_request: self.exit_request.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn wait_for_launch_released_by_close() {
        // A stage that never spawns (denied, sandboxed, cancelled) means the
        // count is never reached; `close` must still release the waiters.
        let job = Arc::new(PipelineJobContext::new(0, false, 2));
        job.register_spawn_attempt();
        let waiter = tokio::spawn({
            let job = Arc::clone(&job);
            async move { job.wait_for_launch().await }
        });
        job.close();
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("wait_for_launch must be released by close")
            .expect("waiter task panicked");
    }

    #[tokio::test]
    async fn wait_for_launch_released_when_count_reached() {
        let job = Arc::new(PipelineJobContext::new(0, false, 2));
        let waiter = tokio::spawn({
            let job = Arc::clone(&job);
            async move { job.wait_for_launch().await }
        });
        job.register_spawn_attempt();
        job.register_spawn_attempt();
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("wait_for_launch must return once every stage has spawned")
            .expect("waiter task panicked");
    }

    #[tokio::test]
    async fn wait_for_launch_no_expected_spawns_does_not_block() {
        let job = PipelineJobContext::new(0, false, 0);
        tokio::time::timeout(Duration::from_secs(5), job.wait_for_launch())
            .await
            .expect("an empty barrier must not block");
    }
}
