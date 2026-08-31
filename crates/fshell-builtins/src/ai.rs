// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::ai_provider::*;
use fshell_core::ShellError;
use fshell_core::{Expr, Parser, PipelineStage, Stmt, Val};
use fshell_engine::{ChatConfig, Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

pub const SYSTEM_PROMPT_EMBEDDED: &str = include_str!("ai_system_prompt.md");
const EXPLAIN_PROMPT_EMBEDDED: &str = include_str!("ai_explain_prompt.md");

pub fn load_system_prompt() -> String {
    let config_dir: Option<PathBuf> = std::env::var("FSH_CONFIG_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config/fsh"))
        });
    if let Some(dir) = config_dir {
        let override_path = dir.join("ai_prompt.md");
        if override_path.exists()
            && let Ok(content) = std::fs::read_to_string(&override_path)
        {
            return content;
        }
    }
    SYSTEM_PROMPT_EMBEDDED.to_string()
}

pub fn load_explain_prompt() -> String {
    EXPLAIN_PROMPT_EMBEDDED.to_string()
}

pub fn ai_main(
    _input: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let raw: Vec<String> = args
        .iter()
        .filter_map(|v| match v {
            Val::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    if raw.first().map(|s| s.as_str()) == Some("--help")
        || raw.first().map(|s| s.as_str()) == Some("-h")
    {
        let help = "\
ai \u{2014} Natural language to fsh command generation

USAGE:
  ai [FLAGS] [PROMPT...]

FLAGS:
  --run, -r        Skip confirmation, execute immediately
  --explain, -e    Explain an fsh command in natural language
  --chat, -c       Enter conversational chat mode
  --provider, -p   Override AI provider
  --model, -m      Override model name
  --list-providers Show available providers
  --help, -h       Show this help

EXAMPLES:
  ai \"find all rust files\"
  ai --run \"show disk usage by directory\"
  ai --explain \"ls | filter size > 1000 | sort size desc\"";
        send_string(tx, help.to_string());
        return Ok(());
    }

    if raw.first().map(|s| s.as_str()) == Some("--list-providers") {
        let mut msg = String::from("Available providers:\n");
        for (name, desc) in list_providers() {
            msg.push_str(&format!("  {name}: {desc}\n"));
        }
        msg.push_str("\nSet FSH_AI_PROVIDER to select one. Default: nvidia-nim");
        send_string(tx, msg);
        return Ok(());
    }

    let mut args_iter = raw.into_iter().peekable();
    let mut run_mode = false;
    let mut explain_mode = false;
    let mut chat_mode = false;
    let mut provider_override = None;
    let mut model_override = None;

    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "--run" | "-r" => run_mode = true,
            "--explain" | "-e" => explain_mode = true,
            "--chat" | "-c" => chat_mode = true,
            "--provider" | "-p" => provider_override = args_iter.next(),
            "--model" | "-m" => model_override = args_iter.next(),
            _ => {
                let mut rest = vec![arg];
                rest.extend(args_iter);
                args_iter = rest.into_iter().peekable();
                break;
            }
        }
    }

    let user_prompt: String = args_iter.collect::<Vec<_>>().join(" ");

    if chat_mode || user_prompt.is_empty() {
        let config = ChatConfig {
            provider_override,
            model_override,
        };
        if let Ok(mut chat) = env.chat_mode.lock() {
            *chat = Some(config);
        }
        println!("fsh-ai chat mode. Type '/exit' to quit, '/clear' to reset.");
        return Ok(());
    }

    if explain_mode {
        return run_explain(provider_override, model_override, &user_prompt, tx);
    }

    let provider = resolve_provider_with_overrides(provider_override, model_override);

    if !is_provider_configured(&*provider) {
        let msg = format!(
            "No API key configured for {}. Set NVIDIA_API_KEY or FSH_AI_API_KEY env var.",
            provider.name()
        );
        send_string(tx, msg);
        return Ok(());
    }

    let system_prompt = load_system_prompt();
    let context = collect_context(env);
    let full_prompt = format!(
        "Natural language: {}\n\nCurrent context:\n{}",
        user_prompt, context
    );

    match provider.generate(&system_prompt, &full_prompt) {
        Ok(response) => {
            // No code fences → natural language response, display directly
            if !response.contains("```") {
                send_string(tx, response.trim().to_string());
                return Ok(());
            }

            let command = extract_fsh_command(&response);
            match safety_check(&command) {
                SafetyVerdict::Blocked(reason) => {
                    let msg = format!(
                        "\u{26A0} BLOCKED \u{2014} {}\n\nThe AI generated this command:\n  {}\n\nRun it manually if you are sure it is safe.",
                        reason, command
                    );
                    send_string(tx, msg);
                }
                SafetyVerdict::RequireConfirm(reason) => {
                    println!("\u{26A0} {reason}");
                    println!("Generated: {command}");
                    print!("Run? [y/N] ");
                    let _ = std::io::stdout().flush();
                    let mut input = String::new();
                    if std::io::stdin().read_line(&mut input).is_ok()
                        && input.trim().eq_ignore_ascii_case("y")
                    {
                        return execute_generated(&command, env, tx);
                    }
                }
                SafetyVerdict::Safe => {
                    if run_mode {
                        return execute_generated(&command, env, tx);
                    }
                    println!("\u{2192} {command}");
                    print!("Run this command? [y/N] ");
                    let _ = std::io::stdout().flush();
                    let mut input = String::new();
                    if std::io::stdin().read_line(&mut input).is_ok()
                        && input.trim().eq_ignore_ascii_case("y")
                    {
                        return execute_generated(&command, env, tx);
                    }
                }
            }
        }
        Err(e) => {
            let msg = format!("AI error: {e}");
            send_string(tx, msg);
        }
    }

    Ok(())
}

fn send_string(tx: PipeSender, s: String) {
    let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(s))));
}

pub fn extract_fsh_command(response: &str) -> String {
    let trimmed = response.trim();
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        if let Some(end) = after.find("```") {
            let inner = after[..end].trim();
            if let Some(nl) = inner.find('\n') {
                return inner[nl + 1..].trim().to_string();
            }
            return inner.to_string();
        }
    }
    trimmed.to_string()
}

pub fn safety_check(command: &str) -> SafetyVerdict {
    let mut parser = Parser::new(command);
    let stmts = match parser.parse_statements() {
        Ok(s) => s,
        Err(_) => return SafetyVerdict::Blocked("Could not parse the generated command".into()),
    };

    for stmt in &stmts {
        let stmt = stmt.unpack();
        let expr = match stmt {
            Stmt::Expr(e) => e.unpack(),
            _ => continue,
        };
        let pipeline = match expr {
            Expr::Pipeline(p) | Expr::InlinePipeline(p) => p,
            _ => continue,
        };
        for stage in &pipeline.stages {
            if let PipelineStage::CommandCall { name, .. } = stage
                && is_destructive_cmd(name)
            {
                return SafetyVerdict::Blocked(format!(
                    "'{name}' is a destructive command and is blocked for safety"
                ));
            }
        }
    }
    SafetyVerdict::Safe
}

fn is_destructive_cmd(name: &str) -> bool {
    matches!(
        name,
        "rm" | "rmdir" | "mv" | "cp" | "dd" | "mkfs" | "format" | "sudo"
    )
}

pub fn collect_context(env: &Env) -> String {
    let cwd = env.cwd().display().to_string();
    let os = std::env::consts::OS;
    let mut ctx = format!("cwd: {cwd}\nos: {os}");

    if let Ok(entries) = std::fs::read_dir(&cwd) {
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
            .take(20)
            .collect();
        names.sort();
        ctx.push_str("\ncwd_contents:");
        for name in names {
            ctx.push_str(&format!("\n  {name}"));
        }
    }
    ctx
}

fn execute_generated(command: &str, env: &Env, tx: PipeSender) -> Result<(), ShellError> {
    let stmts = Parser::new(command)
        .parse_statements()
        .map_err(|e| format!("Parse error in generated command: {e}"))?;

    let handle = tokio::runtime::Handle::current();
    for stmt in &stmts {
        let inner = stmt.unpack();
        let expr = match inner {
            Stmt::Expr(e) => e.unpack(),
            _ => {
                tokio::task::block_in_place(|| {
                    handle.block_on(fshell_engine::eval_stmt(stmt, env, false))
                })
                .map_err(ShellError::from)?;
                continue;
            }
        };
        match expr {
            Expr::Pipeline(pipeline) | Expr::InlinePipeline(pipeline) => {
                let h = handle.clone();
                let env_clone = env.clone();
                let tx_clone = tx.clone();
                let pipeline_clone = pipeline.clone();
                tokio::task::block_in_place(move || {
                    h.block_on(fshell_engine::execute_pipeline(
                        &pipeline_clone,
                        &env_clone,
                        tx_clone,
                    ))
                })
                .map_err(ShellError::from)?;
            }
            _ => {
                tokio::task::block_in_place(|| {
                    handle.block_on(fshell_engine::eval_stmt(stmt, env, false))
                })
                .map_err(ShellError::from)?;
            }
        }
    }
    Ok(())
}

pub fn resolve_provider_with_overrides(
    provider_override: Option<String>,
    model_override: Option<String>,
) -> Box<dyn AiProvider> {
    if let Some(p) = provider_override {
        match p.as_str() {
            "anthropic" => {
                let api_key = std::env::var("ANTHROPIC_API_KEY")
                    .or_else(|_| std::env::var("FSH_ANTHROPIC_API_KEY"))
                    .unwrap_or_default();
                let model = model_override.unwrap_or_else(|| {
                    std::env::var("FSH_AI_MODEL")
                        .unwrap_or_else(|_| "claude-3-5-haiku-latest".into())
                });
                return Box::new(AiProviderAnthropic::new(api_key, model));
            }
            "ollama" => {
                let endpoint = std::env::var("FSH_OLLAMA_ENDPOINT")
                    .unwrap_or_else(|_| "http://localhost:11434".into());
                let model = model_override.unwrap_or_else(|| {
                    std::env::var("FSH_AI_MODEL").unwrap_or_else(|_| "llama3.2".into())
                });
                return Box::new(AiProviderOllama::new(endpoint, model));
            }
            _ => {
                let api_key = api_key_fallback();
                let model = model_override.unwrap_or_else(|| {
                    std::env::var("FSH_AI_MODEL")
                        .unwrap_or_else(|_| "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning".into())
                });
                return Box::new(AiProviderNvidia::new(api_key, model));
            }
        }
    }

    let provider = resolve_provider();
    if let Some(m) = model_override {
        let api_key = api_key_fallback();
        return Box::new(AiProviderNvidia::new(api_key, m));
    }
    provider
}

pub fn api_key_fallback() -> String {
    std::env::var("NVIDIA_API_KEY")
        .or_else(|_| std::env::var("FSH_AI_API_KEY"))
        .unwrap_or_default()
}

fn run_explain(
    provider_override: Option<String>,
    model_override: Option<String>,
    command: &str,
    tx: PipeSender,
) -> Result<(), ShellError> {
    if command.is_empty() {
        let msg = "Usage: ai --explain \"<fsh command>\"".to_string();
        send_string(tx, msg);
        return Ok(());
    }

    let provider = resolve_provider_with_overrides(provider_override, model_override);

    if !is_provider_configured(&*provider) {
        let msg = format!(
            "No API key configured for {}. Set NVIDIA_API_KEY or FSH_AI_API_KEY env var.",
            provider.name()
        );
        send_string(tx, msg);
        return Ok(());
    }

    let system_prompt = load_explain_prompt();
    let prompt = format!("Explain this fsh command: {command}");

    match provider.generate(&system_prompt, &prompt) {
        Ok(response) => {
            send_string(tx, response);
        }
        Err(e) => {
            let msg = format!("Error: {e}");
            send_string(tx, msg);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_fsh_command_with_fences() {
        let input = "Here is the command:\n```\nls | count\n```\n";
        assert_eq!(extract_fsh_command(input), "ls | count");
    }

    #[test]
    fn test_extract_fsh_command_with_language_tag() {
        let input = "```fsh\nls | sort name\n```";
        assert_eq!(extract_fsh_command(input), "ls | sort name");
    }

    #[test]
    fn test_extract_fsh_command_no_fences() {
        let input = "ls -la";
        assert_eq!(extract_fsh_command(input), "ls -la");
    }

    #[test]
    fn test_extract_fsh_command_empty() {
        assert_eq!(extract_fsh_command(""), "");
    }

    #[test]
    fn test_safety_check_blocks_rm() {
        let result = safety_check("rm \"file\"");
        assert_eq!(
            result,
            SafetyVerdict::Blocked(
                "'rm' is a destructive command and is blocked for safety".to_string()
            )
        );
    }

    #[test]
    fn test_safety_check_blocks_sudo() {
        let result = safety_check("sudo echo \"test\"");
        assert_eq!(
            result,
            SafetyVerdict::Blocked(
                "'sudo' is a destructive command and is blocked for safety".to_string()
            )
        );
    }

    #[test]
    fn test_safety_check_blocks_mv() {
        let result = safety_check("mv \"old\" \"new\"");
        assert_eq!(
            result,
            SafetyVerdict::Blocked(
                "'mv' is a destructive command and is blocked for safety".to_string()
            )
        );
    }

    #[test]
    fn test_safety_check_blocks_cp() {
        let result = safety_check("cp \"src\" \"dst\"");
        assert_eq!(
            result,
            SafetyVerdict::Blocked(
                "'cp' is a destructive command and is blocked for safety".to_string()
            )
        );
    }

    #[test]
    fn test_safety_check_allows_safe_command() {
        let result = safety_check("ls -la");
        assert_eq!(result, SafetyVerdict::Safe);
    }

    #[test]
    fn test_safety_check_allows_pipeline_with_safe_commands() {
        let result = safety_check("ls | grep foo | count");
        assert_eq!(result, SafetyVerdict::Safe);
    }

    #[test]
    fn test_safety_check_rejects_unparseable() {
        let result = safety_check("|");
        assert!(matches!(result, SafetyVerdict::Blocked(_)));
    }

    #[test]
    fn test_is_destructive_cmd_list() {
        for cmd in &["rm", "rmdir", "mv", "cp", "dd", "mkfs", "format", "sudo"] {
            assert!(is_destructive_cmd(cmd), "{cmd} should be destructive");
        }
        for cmd in &["ls", "cat", "echo", "ai", "cd"] {
            assert!(!is_destructive_cmd(cmd), "{cmd} should not be destructive");
        }
    }

    #[test]
    fn test_ai_help_via_main() {
        let env = fshell_engine::Env::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let result = ai_main(None, vec![Val::String("--help".into())], &env, tx, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ai_list_providers_via_main() {
        let env = fshell_engine::Env::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let result = ai_main(
            None,
            vec![Val::String("--list-providers".into())],
            &env,
            tx.clone(),
            None,
        );
        assert!(result.is_ok());
        // Should receive a message listing providers
        let received = rx.try_recv();
        assert!(received.is_ok());
        if let Ok(PipelinePayload::Data(val)) = received {
            if let Val::String(s) = &*val {
                assert!(s.contains("nvidia-nim"));
                assert!(s.contains("anthropic"));
                assert!(s.contains("ollama"));
            } else {
                panic!("Expected String payload");
            }
        }
    }

    #[test]
    fn test_ai_chat_sets_env_flag() {
        let env = fshell_engine::Env::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let result = ai_main(None, vec![Val::String("--chat".into())], &env, tx, None);
        assert!(result.is_ok());
        let chat_mode = env.chat_mode.lock().unwrap();
        assert!(chat_mode.is_some());
    }

    #[test]
    fn test_ai_empty_prompt_sets_env_flag() {
        let env = fshell_engine::Env::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let result = ai_main(None, vec![], &env, tx, None);
        assert!(result.is_ok());
        let chat_mode = env.chat_mode.lock().unwrap();
        assert!(chat_mode.is_some());
    }

    #[test]
    fn test_ai_chat_with_provider_and_model() {
        let env = fshell_engine::Env::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let result = ai_main(
            None,
            vec![
                Val::String("--chat".into()),
                Val::String("--provider".into()),
                Val::String("ollama".into()),
                Val::String("--model".into()),
                Val::String("llama3.2".into()),
            ],
            &env,
            tx,
            None,
        );
        assert!(result.is_ok());
        let chat_mode = env.chat_mode.lock().unwrap();
        let config = chat_mode.as_ref().unwrap();
        assert_eq!(config.provider_override.as_deref(), Some("ollama"));
        assert_eq!(config.model_override.as_deref(), Some("llama3.2"));
    }
}
