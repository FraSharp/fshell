// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use fshell_core::Val;
use fshell_core::diagnostic::StringError;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use std::sync::Arc;

pub fn jobs_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    let jobs = env.job_control.jobs.read().clone();
    tokio::spawn(async move {
        for (_, job) in jobs {
            if job.disowned {
                continue;
            }
            let status_str = match job.status {
                fshell_engine::JobStatus::Running => "Running",
                fshell_engine::JobStatus::Suspended => "Suspended",
            };
            let line = format!("[{:>3}] {:<10} {}", job.id, status_str, job.cmd);
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(line))))
                .await;
        }
    });
    Ok(())
}

fn resolve_job(args: &[Val], env: &Env, cmd: &str) -> Result<(usize, i32, String), StringError> {
    let job_id = if !args.is_empty() {
        match &args[0] {
            Val::Int(i) => *i as usize,
            Val::String(s) => {
                let s_trimmed = s.trim_start_matches('%');
                s_trimmed
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid job format: {s}"))?
            }
            _ => {
                return Err(BuiltinError::InvalidArgument {
                    cmd: cmd.into(),
                    arg: "argument must be a job ID".into(),
                    span: None,
                }
                .into());
            }
        }
    } else {
        let jobs = env.job_control.jobs.read();
        jobs.values()
            .filter(|j| !j.disowned && j.pgid > 0)
            .map(|j| j.id)
            .max()
            .ok_or_else(|| format!("{cmd}: no current job"))?
    };

    let jobs = env.job_control.jobs.read();
    let job = jobs
        .iter()
        .find(|(_, j)| j.id == job_id && !j.disowned && j.pgid > 0)
        .ok_or_else(|| format!("{cmd}: job {job_id} not found"))?;
    Ok((job_id, job.1.pgid, job.1.cmd.clone()))
}

pub fn fg_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _tx: PipeSender,
) -> Result<(), StringError> {
    let (job_id, pgid, cmd) = resolve_job(&args, env, "fg")?;

    println!("Resuming foreground: {}", cmd);

    let restore_terminal =
        !fshell_engine::is_test_mode() && unsafe { libc::isatty(libc::STDIN_FILENO) == 1 };

    struct FgTerminalGuard {
        restore_terminal: bool,
        raw_mode_was_enabled: bool,
        shell_pgid: i32,
    }

    impl FgTerminalGuard {
        fn new(restore_terminal: bool) -> Self {
            let mut raw_mode_was_enabled = false;
            if restore_terminal {
                raw_mode_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
                if raw_mode_was_enabled {
                    let _ = crossterm::terminal::disable_raw_mode();
                }
            }
            #[cfg(unix)]
            let shell_pgid = unsafe { libc::getpgrp() };
            #[cfg(not(unix))]
            let shell_pgid = 0;

            Self {
                restore_terminal,
                raw_mode_was_enabled,
                shell_pgid,
            }
        }
    }

    impl Drop for FgTerminalGuard {
        fn drop(&mut self) {
            if self.restore_terminal {
                #[cfg(unix)]
                unsafe {
                    // SAFETY: Restoring the shell to the foreground process group requires
                    // disabling SIGTTOU to prevent the shell process itself from being stopped
                    // when we call tcsetpgrp. This is a standard job-control pattern.
                    // While signal handlers are global per-process and calling libc::signal in
                    // a multi-threaded executor has potential race conditions (e.g., if other threads
                    // expect SIGTTOU default behavior at the same moment), this shell isolates
                    // terminal stdout/stderr interaction of all background pipelines.
                    // The main task execution thread is the sole controller of the terminal's
                    // active foreground process group.
                    libc::signal(libc::SIGTTOU, libc::SIG_IGN);
                    libc::tcsetpgrp(libc::STDIN_FILENO, self.shell_pgid);
                    libc::signal(libc::SIGTTOU, libc::SIG_DFL);
                }
                if self.raw_mode_was_enabled {
                    let _ = crossterm::terminal::enable_raw_mode();
                }
            }
        }
    }

    let _guard = FgTerminalGuard::new(restore_terminal);

    if restore_terminal {
        unsafe {
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            libc::tcsetpgrp(libc::STDIN_FILENO, pgid);
            libc::signal(libc::SIGTTOU, libc::SIG_DFL);
        }
    }

    unsafe {
        libc::kill(-pgid, libc::SIGCONT);
    }

    {
        let mut jobs = env.job_control.jobs.write();
        if let Some(j) = jobs.get_mut(&pgid) {
            j.status = fshell_engine::JobStatus::Running;
        }
    }

    env.set_foreground_job(Some(job_id))
        .map_err(|e| e.to_string())?;

    fshell_engine::spawn_job_waiter(env.clone(), cmd, pgid, job_id, restore_terminal);

    env.wait_foreground(job_id)?;

    Ok(())
}

pub fn bg_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _tx: PipeSender,
) -> Result<(), StringError> {
    let (job_id, pgid, cmd) = resolve_job(&args, env, "bg")?;

    println!("[{}] + Resuming background {}", job_id, cmd);
    unsafe {
        libc::kill(-pgid, libc::SIGCONT);
    }

    {
        let mut jobs = env.job_control.jobs.write();
        if let Some(j) = jobs.get_mut(&pgid) {
            j.status = fshell_engine::JobStatus::Running;
        }
    }

    fshell_engine::spawn_job_waiter(env.clone(), cmd, pgid, job_id, false);

    Ok(())
}

pub fn kill_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _tx: PipeSender,
) -> Result<(), StringError> {
    if args.is_empty() {
        return Err("kill: expected at least one PID or job ID"
            .to_string()
            .into());
    }

    let signal = libc::SIGTERM;

    fn send_and_cleanup(
        env: &Env,
        pgid: i32,
        job_id: usize,
        signal: libc::c_int,
    ) -> Result<(), StringError> {
        unsafe {
            libc::kill(-pgid, libc::SIGCONT);
            libc::kill(-pgid, signal);
        }
        // Clean up the job entry — the waiter already exited after Ctrl+Z
        {
            let mut jobs = env.job_control.jobs.write();
            jobs.retain(|_, j| j.id != job_id);
        }
        // Reap the process in the background to prevent zombies
        std::thread::spawn(move || {
            let mut status = 0;
            unsafe {
                libc::waitpid(pgid, &mut status, 0);
            }
        });
        Ok(())
    }

    for arg in &args {
        match arg {
            Val::Int(job_id) => {
                let jobs = env.job_control.jobs.read();
                let job = jobs
                    .values()
                    .find(|j| j.id == *job_id as usize && !j.disowned && j.pgid > 0)
                    .ok_or_else(|| format!("kill: job {} not found", job_id))?;
                let pgid = job.pgid;
                let jid = job.id;
                drop(jobs);
                send_and_cleanup(env, pgid, jid, signal)?;
            }
            Val::String(s) => {
                let s = s.trim_start_matches('%');
                if let Ok(job_id) = s.parse::<usize>() {
                    let jobs = env.job_control.jobs.read();
                    let job = jobs
                        .values()
                        .find(|j| j.id == job_id && !j.disowned && j.pgid > 0)
                        .ok_or_else(|| format!("kill: job {} not found", job_id))?;
                    let pgid = job.pgid;
                    let jid = job.id;
                    drop(jobs);
                    send_and_cleanup(env, pgid, jid, signal)?;
                } else if let Ok(pid) = s.parse::<libc::pid_t>() {
                    unsafe {
                        libc::kill(pid, signal);
                    }
                } else {
                    return Err(BuiltinError::InvalidArgument {
                        cmd: "kill".into(),
                        arg: format!("invalid PID or job ID: {}", s),
                        span: None,
                    }
                    .into());
                }
            }
            _ => {
                return Err(
                    "kill: argument must be an Int (job ID) or String (PID or %job_id)"
                        .to_string()
                        .into(),
                );
            }
        }
    }
    Ok(())
}

pub fn wait_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    let env = env.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        loop {
            let notified = env.background_notify.notified();
            if env
                .background_count
                .load(std::sync::atomic::Ordering::Relaxed)
                == 0
            {
                break;
            }
            notified.await;
        }
        let _ = tx.send(PipelinePayload::Data(Arc::new(Val::Int(0)))).await;
    });
    Ok(())
}

pub fn disown_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _tx: PipeSender,
) -> Result<(), StringError> {
    let (job_id, pgid, cmd) = resolve_job(&args, env, "disown")?;
    let mut jobs = env.job_control.jobs.write();
    if let Some(job) = jobs.get_mut(&pgid) {
        job.disowned = true;
    }
    println!("[{}]  disowned  {}", job_id, cmd);
    Ok(())
}
