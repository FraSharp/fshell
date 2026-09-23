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
            shell_pgid,
            owns_terminal,
            finished: AtomicBool::new(false),
        }
    }

    pub fn register_spawn_attempt(&self) {
        self.spawned.fetch_add(1, Ordering::AcqRel);
        self.launch_notify.notify_waiters();
    }

    pub async fn wait_for_launch(&self) {
        loop {
            let notified = self.launch_notify.notified();
            if self.spawned.load(Ordering::Acquire) >= self.expected_spawns {
                return;
            }
            notified.await;
        }
    }

    pub fn finish(&self, env: &crate::Env) {
        if self.owns_terminal {
            if let Some(job_id) = env.foreground_job() {
                let _ = env.clear_foreground(job_id);
            }
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
