// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Session-owned capability prompts.
//!
//! Capability checks are synchronous because they can run in any engine
//! command. FTUI therefore services their channel from one long-lived task,
//! rather than creating a receiver per prompt-loop iteration. The task owns
//! the temporary cooked-mode transition and restores raw mode on every exit,
//! including cancellation.

use std::io::{self, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use fshell_engine::{CapPromptRequest, CapPromptResponse, Env};
use tokio::task::JoinHandle;

use super::raw;

pub struct CapabilityPromptTask {
    handle: Option<JoinHandle<()>>,
    session_active: Arc<AtomicBool>,
}

impl CapabilityPromptTask {
    /// Start the single prompt worker for this shell session.
    ///
    /// The receiver is taken exactly once from the environment. If another
    /// owner already claimed it (or this environment has no prompt channel),
    /// the worker exits immediately and the engine's normal timeout behavior
    /// remains the safe fallback.
    pub fn spawn(env: &Env) -> Self {
        let receiver = env.caps.cap_prompt_rx.lock().take();
        let env = env.clone();
        let session_active = Arc::new(AtomicBool::new(true));
        let worker_session_active = session_active.clone();
        let handle = tokio::spawn(async move {
            let Some(mut receiver) = receiver else {
                return;
            };
            while let Some(request) = receiver.recv().await {
                let response = handle_request(&env, &request, worker_session_active.clone()).await;
                // The engine may have timed out while input was being read;
                // dropping the response is then the correct outcome.
                let _ = request.response_tx.send(response);
            }
        });
        Self {
            handle: Some(handle),
            session_active,
        }
    }

    /// Stop the worker before the terminal session is destroyed.
    pub async fn shutdown(mut self) {
        self.session_active.store(false, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

async fn handle_request(
    env: &Env,
    request: &CapPromptRequest,
    session_active: Arc<AtomicBool>,
) -> CapPromptResponse {
    let Ok(_cooked_mode) = CookedModeGuard::enter(session_active) else {
        return CapPromptResponse::Deny;
    };

    eprint!(
        "\r\n[fshell] Allow '{}' to {:?}? [y/N/a] ",
        request.cmd_name, request.action
    );
    let _ = io::stderr().flush();

    let line = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        let _ = io::stdin().read_line(&mut line);
        line
    })
    .await
    .unwrap_or_default();

    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => CapPromptResponse::GrantOnce,
        "a" | "always" => {
            env.caps
                .caps
                .write()
                .grant(request.action.to_resource_handle());
            CapPromptResponse::GrantAlways
        }
        _ => CapPromptResponse::Deny,
    }
}

/// Re-enters raw mode if the prompt task is cancelled while waiting for
/// terminal input. This makes terminal restoration independent of the async
/// task's cancellation timing.
impl CookedModeGuard {
    fn enter(session_active: Arc<AtomicBool>) -> io::Result<Self> {
        raw::enter_cooked_mode()?;
        Ok(Self { session_active })
    }
}

struct CookedModeGuard {
    session_active: Arc<AtomicBool>,
}

impl Drop for CookedModeGuard {
    fn drop(&mut self) {
        if self.session_active.load(Ordering::Acquire) {
            let _ = raw::enter_raw_mode();
        }
    }
}
