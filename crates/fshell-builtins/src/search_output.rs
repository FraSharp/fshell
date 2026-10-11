// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Owned pipeline adapter used by scoped blocking search workers.
use fshell_core::{ErrorCode, ShellError, Val};
use fshell_engine::{Env, PipeSender, PipelinePayload, send_with_backpressure};
use fshell_search::{Access, SearchError, traversal::Hooks};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Output {
    pub env: Env,
    pub tx: PipeSender,
    runtime: tokio::runtime::Handle,
}

impl Output {
    pub fn new(env: &Env, tx: PipeSender) -> Self {
        Self {
            env: env.clone(),
            tx,
            runtime: tokio::runtime::Handle::current(),
        }
    }
    pub fn stopped(&self) -> bool {
        self.env.pipeline_cancelled() || self.tx.is_closed()
    }
    pub fn send(&self, payload: PipelinePayload) -> bool {
        self.runtime
            .block_on(send_with_backpressure(&self.env, &self.tx, payload))
            .is_ok()
    }
    pub fn data(&self, value: Val) -> bool {
        self.send(PipelinePayload::Data(Arc::new(value)))
    }
    pub fn bytes(&self, bytes: Vec<u8>) -> bool {
        self.send(PipelinePayload::Bytes(bytes.into()))
    }
    pub fn hooks(&self, command: &'static str) -> Hooks {
        let env = self.env.clone();
        let output = self.clone();
        Hooks {
            authorize: Arc::new(move |path, access| {
                let action = match access {
                    Access::Directory => fshell_engine::CapAction::ReadDir(path.into()),
                    Access::File => fshell_engine::CapAction::ReadFile(path.into()),
                };
                env.enforce_capability(command, action)
                    .map_err(|e| e.to_string())
            }),
            cancelled: Arc::new(move || output.stopped()),
        }
    }
}

/// Invalid UTF-8 remains bytes instead of being silently replaced.
pub(crate) fn byte_value(bytes: &[u8]) -> Val {
    match std::str::from_utf8(bytes) {
        Ok(text) => Val::String(text.to_owned()),
        Err(_) => Val::Blob(bytes.to_vec()),
    }
}

pub(crate) fn utility_error(command: &str, error: impl std::fmt::Display) -> ShellError {
    ShellError::new(ErrorCode::BuiltinError, format!("{command}: {error}")).with_exit_status(2)
}

pub(crate) fn search_error(command: &str, error: SearchError) -> ShellError {
    if let SearchError::Denied(message) = error {
        // Authorization faults remain hard errors, not utility observations.
        ShellError::new(ErrorCode::CapabilityDenied, message)
    } else {
        utility_error(command, error)
    }
}
