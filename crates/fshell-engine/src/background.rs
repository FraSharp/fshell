// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Child `fsh` processes for POSIX background jobs and subshells.
//!
//! POSIX `&` and `( … )` run in a real child process so that `$!`, `wait`,
//! `kill`, `jobs`, `fg`/`bg` have standard semantics: there is a genuine PID to
//! signal and reap, and the body is isolated from the parent's variables
//! exactly like a forked subshell.
//!
//! The child is another `fsh` process. It receives the parent environment via a
//! handoff file (the same mechanism `reload --full` uses) and runs the fragment
//! in POSIX mode.

use crate::handoff::{HandoffState, save_handoff_to};
use crate::{EngineError, Env, Job, JobStatus};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static BG_SEQ: AtomicU64 = AtomicU64::new(0);

/// Snapshot the environment into a serializable handoff state.
fn capture_handoff(env: &Env) -> HandoffState {
    // Lock order must follow docs/LOCK-ORDERING.md: caps → vars → fns → jobs →
    // reactive → tracked → options (then the lower-traffic prompt/hooks locks).
    let caps = env.caps.caps.read();
    let vars = env.vars.read();
    let fns = env.fns.read();
    let reactive = env.reactive.pipelines.read();
    let options = env.options.read();
    let hooks = env.hooks.registry.read();
    let duration = env.prompt.last_duration.read();

    HandoffState {
        vars: vars.clone(),
        fns: fns.clone(),
        caps_held: caps.held.clone(),
        caps_strict_mode: caps.strict_mode,
        reactive_pipelines: reactive.clone(),
        session_id: vars
            .get("FSH_SESSION_ID")
            .and_then(|v| match v {
                fshell_core::Val::String(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "unknown".to_string()),
        cwd: env.cwd().to_string_lossy().to_string(),
        options: options.clone(),
        hooks: hooks.clone(),
        last_exit_code: env.exit_code(),
        last_duration_secs: duration.as_secs_f64(),
    }
}

fn generic(message: String) -> EngineError {
    EngineError::Generic {
        message,
        span: None,
    }
}

/// Best-effort removal of child-shell files left behind when a shell exited
/// while its jobs were still running. Scoped to the files this module writes.
fn sweep_stale(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let Some(cutoff) =
        std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(3600))
    else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("bg-") {
            continue;
        }
        if let Ok(modified) = entry.metadata().and_then(|m| m.modified())
            && modified < cutoff
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// What a child shell runs.
pub enum ChildPayload {
    /// Source text, parsed by the child in POSIX mode.
    Source(String),
    /// A POSIX program the parent already parsed, serialized by the POSIX
    /// engine. The child evaluates exactly that structure: re-rendering
    /// parsed shell code as text cannot be done faithfully (here-document
    /// bodies, for one, must follow the line that names them).
    PosixProgram(String),
    /// Native statements the parent already parsed, serialized. Run by the
    /// native engine in the child, for the same reason.
    NativeProgram(String),
}

impl ChildPayload {
    /// Native statements as a payload.
    pub fn native(stmts: &[fshell_core::Stmt]) -> Result<Self, EngineError> {
        serde_json::to_string(stmts)
            .map(ChildPayload::NativeProgram)
            .map_err(|e| generic(format!("child shell: cannot serialize program: {e}")))
    }
}

/// The PID `$$` reports: the shell's own, which a subshell process inherits
/// from its parent rather than reporting its own.
pub fn shell_pid() -> u32 {
    *SHELL_PID.get_or_init(std::process::id)
}

/// Adopt the parent shell's PID as `$$`. Only a child shell calls this, once,
/// before evaluating anything.
pub fn inherit_shell_pid(pid: u32) {
    let _ = SHELL_PID.set(pid);
}

static SHELL_PID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

/// A child `fsh` invocation prepared to run a POSIX fragment.
pub struct ChildLaunch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Temp files to remove once the child has exited.
    pub temp_files: Vec<PathBuf>,
}

/// Write `payload` and the current environment to temp files and describe the
/// child `fsh` invocation that runs it in POSIX mode.
///
/// The caller owns `temp_files` and must remove them when the child exits.
pub fn prepare_child(payload: &ChildPayload, env: &Env) -> Result<ChildLaunch, EngineError> {
    let program = crate::exe::resolve_exe();
    let dir = crate::cache_dir()
        .ok_or_else(|| generic("cannot resolve cache dir for a child shell".to_string()))?;
    let _ = std::fs::create_dir_all(&dir);
    sweep_stale(&dir);

    let stamp = format!(
        "{}-{}",
        std::process::id(),
        BG_SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let state_path = dir.join(format!("bg-{stamp}.handoff.json"));
    let script_path = dir.join(format!("bg-{stamp}.fsh"));

    let (text, mode) = match payload {
        ChildPayload::Source(source) => (source, "--posix"),
        ChildPayload::PosixProgram(program) => (program, "--posix-program"),
        ChildPayload::NativeProgram(program) => (program, "--native-program"),
    };
    std::fs::write(&script_path, text)
        .map_err(|e| generic(format!("child shell: cannot write script: {e}")))?;
    if let Err(e) = save_handoff_to(&state_path, &capture_handoff(env)) {
        let _ = std::fs::remove_file(&script_path);
        return Err(generic(format!("child shell: cannot write handoff: {e}")));
    }

    Ok(ChildLaunch {
        program,
        args: match payload {
            ChildPayload::Source(_) => vec![
                mode.into(),
                "--handoff".into(),
                state_path.clone().into(),
                script_path.clone().into(),
            ],
            ChildPayload::PosixProgram(_) | ChildPayload::NativeProgram(_) => vec![
                "--handoff".into(),
                state_path.clone().into(),
                mode.into(),
                script_path.clone().into(),
            ],
        },
        temp_files: vec![state_path, script_path],
    })
}

/// Spawn `source` as a background job (POSIX mode) and return its PID.
///
/// Registers the job in the shared job table and records it as `$!`.
pub fn spawn_background(
    payload: &ChildPayload,
    label: &str,
    env: &Env,
) -> Result<i32, EngineError> {
    let launch = prepare_child(payload, env)?;

    let cleanup = |message: String| {
        for file in &launch.temp_files {
            let _ = std::fs::remove_file(file);
        }
        generic(message)
    };
    let mut cmd = std::process::Command::new(&launch.program);
    cmd.args(&launch.args);
    // The job inherits the shell's descriptors, except that without job
    // control POSIX gives an asynchronous list /dev/null as standard input.
    let mut table = env.descriptors.read().clone();
    table.close(0);
    crate::descriptors::ChildDescriptors::prepare(&table)
        .and_then(|prepared| prepared.attach(&mut cmd))
        .map_err(|e| cleanup(format!("background: cannot set up descriptors: {e}")))?;
    // Its own process group, so it can be signalled as a unit.
    crate::job_control::configure_child_launch(&mut cmd, 0, false);
    let spawned = {
        let _launch = crate::job_control::launch_guard();
        cmd.spawn()
    };
    drop(cmd);
    let mut child = spawned.map_err(|e| cleanup(format!("background: cannot start job: {e}")))?;
    let pid = child.id() as i32;

    let job_id = {
        let jobs = env.job_control.jobs.read();
        jobs.values().map(|j| j.id).max().unwrap_or(0) + 1
    };
    {
        let mut jobs = env.job_control.jobs.write();
        jobs.insert(
            pid,
            Job {
                id: job_id,
                pgid: pid,
                pids: vec![pid],
                last_stage_pid: None,
                last_stage_exit_code: None,
                cmd: label.to_string(),
                status: JobStatus::Running,
                disowned: false,
                started_at: Some(std::time::Instant::now()),
            },
        );
    }
    env.last_bg_pid.store(pid, Ordering::Relaxed);
    env.background_count.fetch_add(1, Ordering::Relaxed);

    // Reap on a plain thread: the child is a std process and we only need a
    // blocking wait, so this works with or without a tokio runtime.
    let reaper_env = env.clone();
    let cmd_str = label.to_string();
    let temp_files = launch.temp_files;
    std::thread::spawn(move || {
        let code = child
            .wait()
            .map(|status| {
                use std::os::unix::process::ExitStatusExt;
                status
                    .code()
                    .or_else(|| status.signal().map(|signal| 128 + signal))
                    .unwrap_or(1)
            })
            .unwrap_or(1);
        // Record the status before the job leaves the table, so a waiter
        // always finds one or the other.
        reaper_env.job_control.finished.lock().insert(pid, code);
        reaper_env.job_control.jobs.write().remove(&pid);
        reaper_env.background_count.fetch_sub(1, Ordering::Relaxed);
        // Every completion wakes waiters: one may be waiting for this job.
        reaper_env.background_notify.notify_waiters();
        for file in &temp_files {
            let _ = std::fs::remove_file(file);
        }
        if reaper_env.options.read().notify {
            let label = if code == 0 { "Done" } else { "Exit" };
            eprintln!("[{job_id}]\t{label} {code}\t{cmd_str}");
        }
    });

    Ok(pid)
}

/// Resolve a `wait` operand — a pid or a `%N` job id — to the job's pid.
pub fn wait_operand_pid(env: &Env, operand: &str) -> Option<i32> {
    match operand.strip_prefix('%') {
        Some(id) => {
            let id: usize = id.parse().ok()?;
            let jobs = env.job_control.jobs.read();
            jobs.iter()
                .find(|(_, job)| job.id == id)
                .map(|(pid, _)| *pid)
        }
        None => operand.parse().ok(),
    }
}

/// Wait for the background job `pid` and return its exit status, or `None`
/// when `pid` is not a job of this shell (POSIX: status 127).
pub async fn wait_job(env: &Env, pid: i32) -> Option<i32> {
    loop {
        // Subscribe before looking, so a job finishing in between still wakes us.
        let notified = env.background_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if let Some(code) = env.job_control.finished.lock().remove(&pid) {
            return Some(code);
        }
        if !env.job_control.jobs.read().contains_key(&pid) {
            return None;
        }
        notified.await;
    }
}

/// Wait for every background job (POSIX `wait` without operands).
pub async fn wait_all_jobs(env: &Env) {
    loop {
        let notified = env.background_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let running = env
            .job_control
            .jobs
            .read()
            .values()
            .any(|job| !job.disowned && matches!(job.status, JobStatus::Running));
        if !running && env.background_count.load(Ordering::Relaxed) == 0 {
            env.job_control.finished.lock().clear();
            return;
        }
        notified.await;
    }
}
