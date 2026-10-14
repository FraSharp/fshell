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

pub(super) fn evaluate_fsh_completions_with_runtime(
    expr: &str,
    env: &Env,
    cancel: Option<&CompletionCancel>,
) -> Vec<String> {
    let runtime = RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("fsh-completion")
            .enable_all()
            .build()
            .ok()
    });
    runtime
        .as_ref()
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
