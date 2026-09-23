// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;
use std::collections::HashSet;
use std::sync::Arc;

pub fn jobs_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let jobs = env.job_control.jobs.read().clone();
    tokio::spawn(async move {
        let mut listed = HashSet::new();
        for (_, job) in jobs {
            if job.disowned || !listed.insert(job.id) {
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

fn resolve_job(args: &[Val], env: &Env, cmd: &str) -> Result<(usize, i32, String), ShellError> {
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
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
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

    let is_pipeline_job = {
        let mut jobs = env.job_control.jobs.write();
        let pids = jobs
            .values()
            .find(|job| job.id == job_id)
            .map(|job| job.pids.len())
            .unwrap_or(1);
        for job in jobs.values_mut().filter(|job| job.pgid == pgid) {
            job.status = fshell_engine::JobStatus::Running;
        }
        pids > 1
    };

    env.set_foreground_job(Some(job_id))
        .map_err(|e| e.to_string())?;

    if is_pipeline_job {
        fshell_engine::spawn_job_group_waiter(env.clone(), pgid, job_id, true);
    } else {
        fshell_engine::spawn_job_waiter(env.clone(), cmd, pgid, job_id, restore_terminal);
    }

    env.wait_foreground(job_id)?;

    Ok(())
}

pub fn bg_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let (job_id, pgid, cmd) = resolve_job(&args, env, "bg")?;

    println!("[{}] + Resuming background {}", job_id, cmd);
    unsafe {
        libc::kill(-pgid, libc::SIGCONT);
    }

    let is_pipeline_job = {
        let mut jobs = env.job_control.jobs.write();
        let pids = jobs
            .values()
            .find(|job| job.id == job_id)
            .map(|job| job.pids.len())
            .unwrap_or(1);
        for job in jobs.values_mut().filter(|job| job.pgid == pgid) {
            job.status = fshell_engine::JobStatus::Running;
        }
        pids > 1
    };

    if is_pipeline_job {
        fshell_engine::spawn_job_group_waiter(env.clone(), pgid, job_id, false);
    } else {
        fshell_engine::spawn_job_waiter(env.clone(), cmd, pgid, job_id, false);
    }

    Ok(())
}

/// Maps a POSIX signal specification (a name such as `TERM`/`SIGKILL` or a
/// number such as `9`) to a signal number. Leading `SIG` is optional and the
/// name is case-insensitive.
fn parse_signal_spec(spec: &str) -> Option<libc::c_int> {
    if let Ok(number) = spec.parse::<libc::c_int>() {
        return Some(number.abs());
    }
    let upper = spec.to_ascii_uppercase();
    let name = upper.strip_prefix("SIG").unwrap_or(&upper);
    let signal = match name {
        "HUP" => libc::SIGHUP,
        "INT" => libc::SIGINT,
        "QUIT" => libc::SIGQUIT,
        "ILL" => libc::SIGILL,
        "TRAP" => libc::SIGTRAP,
        "ABRT" | "IOT" => libc::SIGABRT,
        "BUS" => libc::SIGBUS,
        "FPE" => libc::SIGFPE,
        "KILL" => libc::SIGKILL,
        "USR1" => libc::SIGUSR1,
        "SEGV" => libc::SIGSEGV,
        "USR2" => libc::SIGUSR2,
        "PIPE" => libc::SIGPIPE,
        "ALRM" => libc::SIGALRM,
        "TERM" => libc::SIGTERM,
        "CHLD" | "CLD" => libc::SIGCHLD,
        "CONT" => libc::SIGCONT,
        "STOP" => libc::SIGSTOP,
        "TSTP" => libc::SIGTSTP,
        "TTIN" => libc::SIGTTIN,
        "TTOU" => libc::SIGTTOU,
        "URG" => libc::SIGURG,
        "XCPU" => libc::SIGXCPU,
        "XFSZ" => libc::SIGXFSZ,
        "VTALRM" => libc::SIGVTALRM,
        "PROF" => libc::SIGPROF,
        "WINCH" => libc::SIGWINCH,
        "IO" | "POLL" => libc::SIGIO,
        "SYS" => libc::SIGSYS,
        _ => return None,
    };
    Some(signal)
}

/// Names printed by `kill -l`, in the conventional order.
const SIGNAL_NAMES: &[&str] = &[
    "HUP", "INT", "QUIT", "ILL", "TRAP", "ABRT", "BUS", "FPE", "KILL", "USR1", "SEGV", "USR2",
    "PIPE", "ALRM", "TERM", "CHLD", "CONT", "STOP", "TSTP", "TTIN", "TTOU", "URG", "XCPU", "XFSZ",
    "VTALRM", "PROF", "WINCH", "IO", "SYS",
];

fn invalid_signal(spec: &str, span: Option<SourceSpan>) -> ShellError {
    BuiltinError::InvalidArgument {
        cmd: "kill".into(),
        arg: format!("invalid signal '{spec}'"),
        span,
    }
    .into()
}

pub fn kill_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    // POSIX: `kill [-s signal | -n number | -signal] pid...`. A signal option
    // is only recognised before the first operand, so `kill -9 -1234` treats
    // `-1234` as a (process-group) target, not another signal.
    let mut signal = libc::SIGTERM;
    let mut list = false;
    let mut targets: Vec<Val> = Vec::new();
    let mut index = 0;
    let mut saw_operand = false;

    while index < args.len() {
        if !saw_operand {
            match &args[index] {
                Val::String(flag) if flag == "-l" => {
                    list = true;
                    index += 1;
                    continue;
                }
                Val::String(flag) if flag == "-s" || flag == "-n" => {
                    let spec = args.get(index + 1).ok_or_else(|| {
                        ShellError::from(format!("kill: {flag} requires a signal argument"))
                    })?;
                    let spec = spec.to_text();
                    signal = parse_signal_spec(&spec)
                        .ok_or_else(|| invalid_signal(&spec, span.clone()))?;
                    index += 2;
                    continue;
                }
                Val::String(flag) if flag.starts_with('-') && flag.len() > 1 => {
                    let spec = &flag[1..];
                    signal = parse_signal_spec(spec)
                        .ok_or_else(|| invalid_signal(spec, span.clone()))?;
                    index += 1;
                    continue;
                }
                Val::Int(number) if *number < 0 => {
                    signal = (-*number).clamp(0, libc::c_int::MAX as i64) as libc::c_int;
                    index += 1;
                    continue;
                }
                _ => {}
            }
        }
        targets.push(args[index].clone());
        saw_operand = true;
        index += 1;
    }

    if list {
        let line = SIGNAL_NAMES.join(" ");
        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(line))))
                .await;
        });
        return Ok(());
    }

    if targets.is_empty() {
        return Err("kill: expected at least one PID or job ID"
            .to_string()
            .into());
    }

    fn send_and_cleanup(
        env: &Env,
        pgid: i32,
        job_id: usize,
        signal: libc::c_int,
    ) -> Result<(), ShellError> {
        unsafe {
            libc::kill(-pgid, libc::SIGCONT);
            libc::kill(-pgid, signal);
        }
        // Clean up the job entry — the waiter already exited after Ctrl+Z
        {
            let mut jobs = env.job_control.jobs.write();
            jobs.retain(|_, j| j.id != job_id);
        }
        // Reap every process in the pipeline group to prevent zombies.
        std::thread::spawn(move || {
            loop {
                let mut status = 0;
                let result = unsafe { libc::waitpid(-pgid, &mut status, 0) };
                if result <= 0 {
                    if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                        continue;
                    }
                    break;
                }
            }
        });
        Ok(())
    }

    for target in &targets {
        match target {
            // A bare (or negative, for a process group) number is a PID.
            Val::Int(pid) => unsafe {
                libc::kill(*pid as libc::pid_t, signal);
            },
            Val::String(s) => {
                if let Some(job_ref) = s.strip_prefix('%') {
                    let job_id: usize =
                        job_ref.parse().map_err(|_| BuiltinError::InvalidArgument {
                            cmd: "kill".into(),
                            arg: format!("invalid job ID: {s}"),
                            span: span.clone(),
                        })?;
                    let pgid = env
                        .job_control
                        .jobs
                        .read()
                        .values()
                        .find(|j| j.id == job_id && !j.disowned && j.pgid > 0)
                        .map(|j| j.pgid)
                        .ok_or_else(|| format!("kill: job {job_id} not found"))?;
                    send_and_cleanup(env, pgid, job_id, signal)?;
                } else if let Ok(pid) = s.parse::<libc::pid_t>() {
                    unsafe {
                        libc::kill(pid, signal);
                    }
                } else {
                    return Err(BuiltinError::InvalidArgument {
                        cmd: "kill".into(),
                        arg: format!("invalid PID or job ID: {s}"),
                        span,
                    }
                    .into());
                }
            }
            _ => {
                return Err("kill: argument must be a PID or a %job ID"
                    .to_string()
                    .into());
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
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
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
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let (job_id, pgid, cmd) = resolve_job(&args, env, "disown")?;
    let mut jobs = env.job_control.jobs.write();
    if let Some(job) = jobs.get_mut(&pgid) {
        job.disowned = true;
    }
    println!("[{}]  disowned  {}", job_id, cmd);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_signal_spec;

    #[test]
    fn signal_spec_accepts_names_and_numbers() {
        assert_eq!(parse_signal_spec("9"), Some(9));
        assert_eq!(parse_signal_spec("KILL"), Some(libc::SIGKILL));
        assert_eq!(parse_signal_spec("SIGKILL"), Some(libc::SIGKILL));
        assert_eq!(parse_signal_spec("sigterm"), Some(libc::SIGTERM));
        assert_eq!(parse_signal_spec("TERM"), Some(libc::SIGTERM));
        assert_eq!(parse_signal_spec("15"), Some(15));
        assert_eq!(parse_signal_spec("BOGUS"), None);
        assert_eq!(parse_signal_spec(""), None);
    }
}
