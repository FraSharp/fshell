// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Isolated, deadline-bound evaluation of expression-valued completion providers.

use super::types::CompletionCancel;
use fshell_core::{Stmt, Val};
use fshell_engine::Env;
use std::sync::OnceLock;
use std::time::Duration;

const PROVIDER_TIMEOUT: Duration = Duration::from_secs(2);
const CANCEL_GRACE: Duration = Duration::from_millis(200);
static RUNTIME: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();

struct Invocation(Env);

impl Invocation {
    fn new(env: &Env) -> Self {
        let mut env = env.begin_invocation();
        // Providers must not interrupt, replace, or publish status into the
        // foreground command. Their children belong to this invocation alone.
        env.job_control = fshell_engine::job_control::JobControl::default();
        env.pipeline_job = None;
        env.is_captured = true;
        Self(env)
    }

    fn cancel(&self) {
        // Some synchronous builtins still consume only the interrupt flag.
        // Both flags are private to this provider, never the foreground shell.
        self.0.request_pipeline_interrupt();
        self.0.request_shell_cancellation();
        let jobs = self.0.job_control.jobs.read();
        for job in jobs.values() {
            if job.pgid > 0 && job.pgid != unsafe { libc::getpgrp() } {
                // The ordinary engine waiters still own reaping. Kill the
                // provider group, including descendants holding pipe writers.
                unsafe {
                    libc::kill(-job.pgid, libc::SIGKILL);
                }
            }
        }
    }
}

impl Drop for Invocation {
    fn drop(&mut self) {
        self.cancel();
    }
}

async fn evaluate(expr: &str, env: &Env) -> Vec<String> {
    let mut parser = fshell_core::Parser::new(expr);
    let Ok(stmts) = parser.parse_statements() else {
        return Vec::new();
    };
    let mut results = Vec::new();
    for stmt in stmts {
        if env.pipeline_cancelled() {
            break;
        }
        if let Stmt::Expr(expr) = stmt.unpack() {
            if let Ok(result) = fshell_engine::eval_expr(expr, env).await {
                results = match result {
                    Val::List(list) => list.into_iter().map(|value| value.to_text()).collect(),
                    Val::String(value) => value.split_whitespace().map(str::to_string).collect(),
                    other => vec![other.to_text()],
                };
            }
        } else {
            let _ = fshell_engine::eval_stmt(&stmt, env, false).await;
        }
    }
    results
}

async fn evaluate_bounded(
    expr: &str,
    env: &Env,
    cancel: Option<&CompletionCancel>,
    timeout: Duration,
) -> Vec<String> {
    if cancel.is_some_and(CompletionCancel::is_cancelled) {
        return Vec::new();
    }
    let invocation = Invocation::new(env);
    let result = evaluate(expr, &invocation.0);
    tokio::pin!(result);
    let cancelled = async {
        match cancel {
            Some(cancel) => cancel.cancelled().await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        result = &mut result => return result,
        _ = cancelled => {},
        _ = tokio::time::sleep(timeout) => {},
    }

    // Keep driving evaluation long enough for cooperative builtins and child
    // waiters to finish. Repeat cleanup to cover a spawn concurrent with the
    // cancellation notification, without holding up the next request forever.
    let _ = tokio::time::timeout(CANCEL_GRACE, async {
        loop {
            invocation.cancel();
            tokio::select! {
                _ = &mut result => break,
                _ = tokio::time::sleep(Duration::from_millis(5)) => {},
            }
        }
    })
    .await;
    Vec::new()
}

fn runtime() -> Option<&'static tokio::runtime::Runtime> {
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("fsh-completion")
                .enable_all()
                .build()
                .ok()
        })
        .as_ref()
}

pub(super) fn evaluate_fsh_completions_with_runtime(
    expr: &str,
    env: &Env,
    cancel: Option<&CompletionCancel>,
) -> Vec<String> {
    runtime()
        .map(|runtime| runtime.block_on(evaluate_bounded(expr, env, cancel, PROVIDER_TIMEOUT)))
        .unwrap_or_default()
}

pub(super) fn evaluate_fsh_completions_sync(
    expr: &str,
    env: &Env,
    cancel: Option<&CompletionCancel>,
) -> Vec<String> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| {
                handle.block_on(evaluate_bounded(expr, env, cancel, PROVIDER_TIMEOUT))
            })
        }
        // A synchronous API called from a current-thread executor cannot
        // enter another runtime on that executor's thread.
        Ok(_) => std::thread::scope(|scope| {
            scope
                .spawn(|| evaluate_fsh_completions_with_runtime(expr, env, cancel))
                .join()
                .unwrap_or_default()
        }),
        Err(_) => evaluate_fsh_completions_with_runtime(expr, env, cancel),
    }
}

/// Blocking-worker API for external completion generators. Bound process
/// lifetime and output, and terminate the launch group on error or
/// cancellation. Group cleanup only runs before the direct child is reaped,
/// while its PID still reserves the process-group ID.
pub(super) fn run_process(
    program: &str,
    args: &[String],
    cwd: &std::path::Path,
    search_path: &std::ffi::OsStr,
    cancel: Option<&CompletionCancel>,
) -> Option<Vec<u8>> {
    use std::sync::atomic::{AtomicBool, Ordering};
    if cancel.is_some_and(CompletionCancel::is_cancelled) {
        return None;
    }
    runtime()?.block_on(async {
        use tokio::io::AsyncReadExt;
        let mut child = tokio::process::Command::new(program)
            .args(args)
            .current_dir(cwd)
            .env("PATH", search_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        let group = i32::try_from(child.id()?).ok()?;
        let stdout = child.stdout.take()?;
        // Reaping releases the PID, which also identifies the launch group. A
        // cancellation race can select this branch after the work future has
        // already waited, so record the reap before deciding to signal.
        let reaped = std::sync::Arc::new(AtomicBool::new(false));
        let observed = reaped.clone();
        let work = tokio::time::timeout(PROVIDER_TIMEOUT, async {
            const MAX_BYTES: usize = 1024 * 1024;
            let mut bytes = Vec::new();
            if stdout
                .take((MAX_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .await
                .is_err()
            {
                return (None, false);
            }
            if bytes.len() > MAX_BYTES {
                return (None, false);
            }
            let status = child.wait().await;
            observed.store(true, Ordering::Release);
            match status {
                Ok(status) => (status.success().then_some(bytes), true),
                Err(_) => (None, true),
            }
        });
        let cancelled = async {
            match cancel {
                Some(cancel) => cancel.cancelled().await,
                None => std::future::pending().await,
            }
        };
        let (output, work_reaped) = tokio::select! {
            result = work => result.unwrap_or((None, false)),
            _ = cancelled => (None, false),
        };
        let leader_reaped = work_reaped || reaped.load(Ordering::Acquire);
        if output.is_none() && !leader_reaped {
            // SAFETY: the direct child has not been reaped, so its PID and the
            // process-group ID created by process_group(0) are still reserved.
            // A negative PID signals only that launch group.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
            let _ = child.start_kill();
            let _ = tokio::time::timeout(CANCEL_GRACE, child.wait()).await;
        }
        output
    })
}

#[cfg(test)]
mod tests {
    use super::super::types::CompletionGeneration;
    use super::*;
    use std::sync::Arc;

    #[test]
    fn cancellation_releases_provider_without_interrupting_parent() {
        let env = Env::new();
        env.set_exit_code(37);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        env.register_async_builtin("waitchoices", move |_, _, env, _, _| {
            let started = started_tx.clone();
            async move {
                let _ = started.send(());
                env.wait_pipeline_cancelled().await;
                Ok(())
            }
        });
        let generation = Arc::new(CompletionGeneration::default());
        let cancel = CompletionCancel::new(0, generation.clone());
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker_env = env.clone();
        let worker = std::thread::spawn(move || {
            let result = evaluate_fsh_completions_sync("waitchoices", &worker_env, Some(&cancel));
            done_tx.send(result).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        generation.advance(1);
        assert!(
            done_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .is_empty()
        );
        worker.join().unwrap();
        assert!(!env.pipeline_cancelled());
        assert_eq!(env.exit_code(), 37);
    }

    #[test]
    fn cancellation_interrupts_synchronous_builtin_work() {
        let env = Env::new();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        env.register_builtin(
            "waitchoices",
            Arc::new(move |input, _, env, output, span| {
                started_tx.send(()).unwrap();
                let result = fshell_builtins::cmd::misc::sleep_builtin(
                    input,
                    vec![Val::String("30".into())],
                    env,
                    output,
                    span,
                );
                finished_tx.send(()).unwrap();
                result
            }),
        );
        let generation = Arc::new(CompletionGeneration::default());
        let cancel = CompletionCancel::new(0, generation.clone());
        let worker = std::thread::spawn(move || {
            evaluate_fsh_completions_sync("waitchoices", &env, Some(&cancel))
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        generation.advance(1);
        finished_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("native work survived cancellation");
        assert!(worker.join().unwrap().is_empty());
    }

    #[test]
    fn cancellation_terminates_and_reaps_provider_children() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("pid");
        let env = Env::new();
        fshell_bridge::init(&env);
        let generation = Arc::new(CompletionGeneration::default());
        let cancel = CompletionCancel::new(0, generation.clone());
        let expression = format!(
            "/bin/sh -c 'echo $$ > {}; exec /bin/sleep 30'",
            marker.display()
        );
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            done_tx
                .send(evaluate_fsh_completions_sync(
                    &expression,
                    &env,
                    Some(&cancel),
                ))
                .unwrap();
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let pid = loop {
            if let Ok(text) = std::fs::read_to_string(&marker)
                && let Ok(pid) = text.trim().parse::<i32>()
            {
                break pid;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "provider child did not start"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        generation.advance(1);
        assert!(
            done_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .is_empty()
        );
        worker.join().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while unsafe { libc::kill(pid, 0) } == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "provider child survived cancellation"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[tokio::test]
    async fn deadline_cancels_cooperative_provider() {
        let env = Env::new();
        env.register_async_builtin("waitchoices", |_, _, env, _, _| async move {
            env.wait_pipeline_cancelled().await;
            Ok(())
        });
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            evaluate_bounded("waitchoices", &env, None, Duration::from_millis(10)),
        )
        .await
        .unwrap();
        assert!(result.is_empty());
        assert!(!env.pipeline_cancelled());
    }
}
