// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_engine::Env;
use std::sync::Mutex;

pub struct AgentModeState {
    pub active: bool,
    pub prompt: String,
    pub is_loading: bool,
    pub result_command: Option<String>,
    pub error_msg: Option<String>,
    pub query_id: usize,
    active_handle: Option<tokio::task::JoinHandle<()>>,
}

impl Default for AgentModeState {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentModeState {
    pub fn new() -> Self {
        Self {
            active: false,
            prompt: String::new(),
            is_loading: false,
            result_command: None,
            error_msg: None,
            query_id: 0,
            active_handle: None,
        }
    }

    pub fn reset(&mut self) {
        if let Some(h) = self.active_handle.take() {
            h.abort();
        }
        self.active = false;
        self.prompt.clear();
        self.is_loading = false;
        self.result_command = None;
        self.error_msg = None;
        self.query_id = 0;
    }

    pub fn trigger_query(&mut self, user_prompt: &str, env: &Env) {
        self.active = true;
        self.prompt = user_prompt.to_string();
        self.is_loading = true;
        self.result_command = None;
        self.error_msg = None;

        if let Some(h) = self.active_handle.take() {
            h.abort();
        }
        self.query_id += 1;
        let qid = self.query_id;

        let prompt_clone = user_prompt.to_string();
        let env_clone = env.clone();

        let handle = tokio::spawn(async move {
            let result = query_ai_backend(&prompt_clone, &env_clone).await;
            // Only publish if this is still the latest query (avoid stale overwrite after cancel).
            let mut guard = AGENT_RESULT.lock().unwrap_or_else(|e| e.into_inner());
            if guard.as_ref().is_none_or(|(prev_qid, _)| *prev_qid < qid) {
                *guard = Some((qid, result));
            }
        });
        self.active_handle = Some(handle);
    }
}

pub static AGENT_RESULT: Mutex<Option<(usize, Result<String, String>)>> = Mutex::new(None);

#[cfg(feature = "ai")]
async fn query_ai_backend(prompt: &str, _env: &Env) -> Result<String, String> {
    // Use tokio's spawn_blocking for synchronous AiProvider::generate network calls
    let prompt_owned = prompt.to_string();
    tokio::task::spawn_blocking(move || {
        let provider = fshell_builtins::ai::resolve_provider_with_overrides(None, None);
        if !fshell_builtins::ai_provider::is_provider_configured(&*provider) {
            return Err(format!(
                "AI provider '{}' is not configured. Set ANTHROPIC_API_KEY, NVIDIA_API_KEY, or FSH_AI_API_KEY.",
                provider.name()
            ));
        }

        let system_prompt = fshell_builtins::ai::load_system_prompt();
        let context = fshell_builtins::ai::collect_context();
        let full_prompt = format!(
            "Natural language command request: {}\n\nContext:\n{}",
            prompt_owned, context
        );

        match provider.generate(&system_prompt, &full_prompt) {
            Ok(response) => {
                let cmd = fshell_builtins::ai::extract_fsh_command(&response);
                Ok(cmd.trim().to_string())
            }
            Err(e) => Err(format!("AI generation error: {}", e)),
        }
    })
    .await
    .map_err(|e| format!("Join error: {}", e))?
}

#[cfg(not(feature = "ai"))]
async fn query_ai_backend(_prompt: &str, _env: &Env) -> Result<String, String> {
    Err(
        "AI features are disabled in this build. Rebuild fshell with '--features ai' to enable."
            .to_string(),
    )
}
