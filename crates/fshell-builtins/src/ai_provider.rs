// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::diagnostic::StringError;

/// Safety verdict for AI-generated commands.
#[derive(Debug, Clone, PartialEq)]
pub enum SafetyVerdict {
    /// Command is safe to execute
    Safe,
    /// Command requires user confirmation
    RequireConfirm(String),
    /// Command is blocked entirely — show disclaimer
    Blocked(String),
}

/// AI provider trait — synchronous, Send + Sync.
pub trait AiProvider: Send + Sync {
    /// Generate a response given system prompt and user prompt.
    fn generate(&self, system: &str, prompt: &str) -> Result<String, StringError>;
    /// Provider display name.
    fn name(&self) -> &str;
}

fn make_agent(timeout_secs: u64) -> ureq::Agent {
    let config = ureq::config::Config::builder()
        .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
        .build();
    ureq::Agent::new_with_config(config)
}

fn se(s: String) -> StringError {
    StringError::from(s)
}

// NVIDIA NIM Provider (default)

/// NVIDIA NIM via build.nvidia.com API.
/// Uses the OpenAI-compatible chat completions endpoint.
pub struct AiProviderNvidia {
    api_key: String,
    model: String,
    endpoint: String,
}

impl AiProviderNvidia {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            endpoint: "https://integrate.api.nvidia.com/v1".to_string(),
        }
    }

    /// Create from environment variables, with fallback defaults.
    pub fn from_env() -> Self {
        let api_key = std::env::var("NVIDIA_API_KEY")
            .or_else(|_| std::env::var("FSH_AI_API_KEY"))
            .unwrap_or_default();
        let model = std::env::var("FSH_AI_MODEL")
            .unwrap_or_else(|_| "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning".to_string());
        Self::new(api_key, model)
    }
}

impl AiProvider for AiProviderNvidia {
    fn generate(&self, system: &str, prompt: &str) -> Result<String, StringError> {
        let url = format!("{}/chat/completions", self.endpoint);
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": prompt}
            ],
            "temperature": 0.6,
            "top_p": 0.95,
            "max_tokens": 65536,
            "chat_template_kwargs": {"enable_thinking": true},
            "reasoning_budget": 16384,
        });
        let body_str =
            serde_json::to_string(&body).map_err(|e| se(format!("serialization failed: {e}")))?;

        let agent = make_agent(15);
        let mut resp = agent
            .post(&url)
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .send(&body_str)
            .map_err(|e| se(format!("NVIDIA NIM request failed: {e}")))?;

        let body_text = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| se(format!("NVIDIA NIM response read failed: {e}")))?;

        let json: serde_json::Value = serde_json::from_str(&body_text)
            .map_err(|e| se(format!("NVIDIA NIM response parse failed: {e}")))?;

        // Try content, then fall back to reasoning_content (reasoning models)
        let content = json["choices"][0]["message"]["content"]
            .as_str()
            .and_then(|s| {
                let t = s.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            })
            .or_else(|| {
                json["choices"][0]["message"]["reasoning_content"]
                    .as_str()
                    .map(|s| s.trim().to_string())
            });

        content.ok_or_else(|| {
            if let Some(err) = json["error"]["message"].as_str() {
                se(format!("NVIDIA NIM API error: {err}"))
            } else if let Some(err) = json["error"].as_str() {
                se(format!("NVIDIA NIM API error: {err}"))
            } else {
                let preview = &body_text[..body_text.len().min(200)];
                se(format!(
                    "NVIDIA NIM API error (unexpected response): {preview}"
                ))
            }
        })
    }

    fn name(&self) -> &str {
        "nvidia-nim"
    }
}

// Anthropic Provider (alternate)

pub struct AiProviderAnthropic {
    api_key: String,
    model: String,
}

impl AiProviderAnthropic {
    pub fn new(api_key: String, model: String) -> Self {
        Self { api_key, model }
    }

    pub fn from_env() -> Self {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .or_else(|_| std::env::var("FSH_ANTHROPIC_API_KEY"))
            .unwrap_or_default();
        let model =
            std::env::var("FSH_AI_MODEL").unwrap_or_else(|_| "claude-3-5-haiku-latest".to_string());
        Self::new(api_key, model)
    }
}

impl AiProvider for AiProviderAnthropic {
    fn generate(&self, system: &str, prompt: &str) -> Result<String, StringError> {
        let body = serde_json::json!({
            "model": self.model,
            "system": system,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.1,
            "max_tokens": 1024,
        });
        let body_str =
            serde_json::to_string(&body).map_err(|e| se(format!("serialization failed: {e}")))?;

        let agent = make_agent(15);
        let mut resp = agent
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .send(&body_str)
            .map_err(|e| se(format!("Anthropic request failed: {e}")))?;

        let body_text = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| se(format!("Anthropic response read failed: {e}")))?;

        let json: serde_json::Value = serde_json::from_str(&body_text)
            .map_err(|e| se(format!("Anthropic response parse failed: {e}")))?;

        json["content"][0]["text"]
            .as_str()
            .map(|s| s.trim().to_string())
            .ok_or_else(|| {
                let err = json["error"]["message"].as_str().unwrap_or("unknown");
                se(format!("Anthropic API error: {err}"))
            })
    }

    fn name(&self) -> &str {
        "anthropic"
    }
}

// Ollama Provider (alternate, local)

pub struct AiProviderOllama {
    endpoint: String,
    model: String,
}

impl AiProviderOllama {
    pub fn new(endpoint: String, model: String) -> Self {
        Self { endpoint, model }
    }

    pub fn from_env() -> Self {
        let endpoint = std::env::var("FSH_OLLAMA_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:11434".to_string());
        let model = std::env::var("FSH_AI_MODEL").unwrap_or_else(|_| "llama3.2".to_string());
        Self::new(endpoint, model)
    }
}

impl AiProvider for AiProviderOllama {
    fn generate(&self, system: &str, prompt: &str) -> Result<String, StringError> {
        let url = format!("{}/api/chat", self.endpoint);
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": prompt}
            ],
            "stream": false,
        });
        let body_str =
            serde_json::to_string(&body).map_err(|e| se(format!("serialization failed: {e}")))?;

        let agent = make_agent(30);
        let mut resp = agent
            .post(&url)
            .header("Content-Type", "application/json")
            .send(&body_str)
            .map_err(|e| se(format!("Ollama request failed: {e}")))?;

        let body_text = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| se(format!("Ollama response read failed: {e}")))?;

        let json: serde_json::Value = serde_json::from_str(&body_text)
            .map_err(|e| se(format!("Ollama response parse failed: {e}")))?;

        json["message"]["content"]
            .as_str()
            .map(|s| s.trim().to_string())
            .ok_or_else(|| {
                let err = json["error"].as_str().unwrap_or("unknown");
                se(format!("Ollama error: {err}"))
            })
    }

    fn name(&self) -> &str {
        "ollama"
    }
}

/// Resolve the AI provider based on the `FSH_AI_PROVIDER` env var.
/// Defaults to NVIDIA NIM.
pub fn resolve_provider() -> Box<dyn AiProvider> {
    match std::env::var("FSH_AI_PROVIDER").as_deref() {
        Ok("anthropic") => Box::new(AiProviderAnthropic::from_env()),
        Ok("ollama") => Box::new(AiProviderOllama::from_env()),
        _ => Box::new(AiProviderNvidia::from_env()),
    }
}

/// Check if a provider is configured (has an API key, or is Ollama which is local).
pub fn is_provider_configured(provider: &dyn AiProvider) -> bool {
    if provider.name() == "ollama" {
        return true; // local, always available
    }
    let key = std::env::var("NVIDIA_API_KEY")
        .or_else(|_| std::env::var("FSH_AI_API_KEY"))
        .unwrap_or_default();
    let anthropic_key = std::env::var("ANTHROPIC_API_KEY")
        .or_else(|_| std::env::var("FSH_ANTHROPIC_API_KEY"))
        .unwrap_or_default();
    match provider.name() {
        "nvidia-nim" => !key.is_empty(),
        "anthropic" => !anthropic_key.is_empty(),
        _ => false,
    }
}

/// List available providers with descriptions.
pub fn list_providers() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "nvidia-nim",
            "NVIDIA NIM (default) — free via build.nvidia.com. Set NVIDIA_API_KEY.",
        ),
        (
            "anthropic",
            "Anthropic Claude — requires ANTHROPIC_API_KEY.",
        ),
        ("ollama", "Ollama — local, no API key needed."),
    ]
}
