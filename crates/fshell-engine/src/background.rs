// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Background job spawning.
//!
//! POSIX `&` (and, later, `( … )` subshells) run in a real child process so
//! that `$!`, `wait`, `kill`, `jobs`, `fg`/`bg` have standard semantics: there
//! is a genuine PID to signal and reap, and the body is isolated from the
//! parent's variables exactly like a forked subshell.
//!
//! The child is another `fsh` process. It receives the parent environment via
//! a handoff file (the same mechanism `reload --full` uses) and runs the
//! fragment in POSIX mode.

use crate::handoff::{HandoffState, save_handoff_to};
use crate::{EngineError, Env, Job, JobStatus};
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

/// Best-effort removal of background job files left behind when a shell exited
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

/// Spawn `source` as a background job (POSIX mode) and return its PID.
///
/// Registers the job in the shared job table and records it as `$!`.
pub fn spawn_background(source: &str, env: &Env) -> Result<i32, EngineError> {
    let exe = crate::exe::resolve_exe();
    let dir = crate::cache_dir()
        .ok_or_else(|| generic("cannot resolve cache dir for background job".to_string()))?;
    let _ = std::fs::create_dir_all(&dir);
    sweep_stale(&dir);

    let stamp = format!(
        "{}-{}",
        std::process::id(),
        BG_SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let state_path = dir.join(format!("bg-{stamp}.handoff.json"));
    let script_path = dir.join(format!("bg-{stamp}.fsh"));

    std::fs::write(&script_path, source)
        .map_err(|e| generic(format!("background: cannot write job script: {e}")))?;
    save_handoff_to(&state_path, &capture_handoff(env)).map_err(|e| {
        let _ = std::fs::remove_file(&script_path);
        generic(format!("background: cannot write handoff: {e}"))
    })?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--posix")
        .arg("--handoff")
        .arg(&state_path)
        .arg(&script_path)
        .stdin(std::process::Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Give the job its own process group so it can be signalled as a unit.
        unsafe {
            cmd.pre_exec(|| {
                let _ = libc::setpgid(0, 0);
                Ok(())
            });
        }
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = std::fs::remove_file(&state_path);
            let _ = std::fs::remove_file(&script_path);
            return Err(generic(format!("background: cannot start job: {e}")));
        }
    };
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
                cmd: source.to_string(),
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
    let cmd_str = source.to_string();
    std::thread::spawn(move || {
        let code = child
            .wait()
            .ok()
            .and_then(|status| status.code())
            .unwrap_or(0);
        reaper_env.job_control.jobs.write().remove(&pid);
        if reaper_env.background_count.fetch_sub(1, Ordering::Relaxed) == 1 {
            reaper_env.background_notify.notify_waiters();
        }
        let _ = std::fs::remove_file(&state_path);
        let _ = std::fs::remove_file(&script_path);
        if reaper_env.options.read().notify {
            let label = if code == 0 { "Done" } else { "Exit" };
            eprintln!("[{job_id}]\t{label} {code}\t{cmd_str}");
        }
    });

    Ok(pid)
}
