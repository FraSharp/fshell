// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! The `intent` builtin — the non-invasive integration point for the semantic layer.
//!
//! It consumes a *structured* [`Intent`] (the shape a small function-calling model emits)
//! and validates, renders and optionally executes it through fshell's ordinary machinery.
//! There is deliberately no `execute_shell(string)` entry point here: the only input is a
//! typed semantic action.

use fshell_core::ShellError;
use fshell_core::Val;
use fshell_core::diagnostic::ErrorCode;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use fshell_semantic::{
    Action, Intent, IntentMode, Platform, RiskLevel, ShellTarget, all_specs, lower_fsh,
    lower_posix, render_clarification, render_fsh, render_info, render_posix, render_risk,
    tools_json, validate_intent,
};
use miette::SourceSpan;
use std::io::IsTerminal;
use std::io::Write;
use std::sync::Arc;

const HELP: &str = "\
intent — run a structured semantic action

USAGE:
  intent --json '<intent json>' [--target fsh|posix] [--run] [--explain] [--risk]
  intent --file <path> [--target fsh|posix] [--run]
  intent --actions
  intent --schema [--pretty]

FLAGS:
  --json <JSON>    The Intent to process (may also arrive on stdin)
  --file <PATH>    Read the Intent JSON from a file
  --target <T>     Execution target: fsh (default) or posix
  --run            Execute the action (default: print the plan only)
  --explain        Print both renderings and stop (never executes)
  --risk           Print only the risk classification
  --actions        List the supported semantic actions
  --schema         Print FunctionGemma tool schemas
  --pretty         Pretty-print --schema output
  --help, -h       Show this help

EXAMPLES:
  intent --json '{\"kind\":\"memory_info\"}' --run
  intent --file plan.json --target posix
  intent --actions";

struct Options {
    json: Option<String>,
    file: Option<String>,
    target: ShellTarget,
    run: bool,
    explain: bool,
    risk_only: bool,
    actions: bool,
    schema: bool,
    pretty: bool,
    help: bool,
}

pub fn intent_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let Some(options) = parse_options(&args)? else {
        send_string(&tx, HELP.to_string());
        return Ok(());
    };

    if options.help {
        send_string(&tx, HELP.to_string());
        return Ok(());
    }
    if options.actions {
        send_string(&tx, actions_text());
        return Ok(());
    }
    if options.schema {
        let value = tools_json();
        let text = if options.pretty {
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
        } else {
            value.to_string()
        };
        send_string(&tx, text);
        return Ok(());
    }

    let json = if let Some(path) = &options.file {
        std::fs::read_to_string(path).map_err(|error| {
            ShellError::new(
                ErrorCode::IoError,
                format!("intent: cannot read '{path}': {error}"),
            )
        })?
    } else if let Some(json) = &options.json {
        json.clone()
    } else {
        collect_stream(in_rx)
    };

    process(&json, &options, env, &tx)
}

fn parse_options(args: &[Val]) -> Result<Option<Options>, ShellError> {
    let raw: Vec<String> = args.iter().map(Val::to_text).collect();
    let mut options = Options {
        json: None,
        file: None,
        target: ShellTarget::Fsh,
        run: false,
        explain: false,
        risk_only: false,
        actions: false,
        schema: false,
        pretty: false,
        help: false,
    };
    let mut positional: Vec<String> = Vec::new();

    let mut index = 0;
    while index < raw.len() {
        match raw[index].as_str() {
            "--help" | "-h" => {
                options.help = true;
                return Ok(Some(options));
            }
            "--json" => {
                options.json = Some(required_value(&raw, index, "--json")?);
                index += 2;
            }
            "--file" => {
                options.file = Some(required_value(&raw, index, "--file")?);
                index += 2;
            }
            "--target" => {
                let value = required_value(&raw, index, "--target")?;
                options.target = ShellTarget::parse(&value).ok_or_else(|| {
                    ShellError::new(
                        ErrorCode::InvalidArgument,
                        format!("intent: unknown target '{value}' (expected fsh or posix)"),
                    )
                })?;
                index += 2;
            }
            "--run" => {
                options.run = true;
                index += 1;
            }
            "--explain" => {
                options.explain = true;
                index += 1;
            }
            "--risk" => {
                options.risk_only = true;
                index += 1;
            }
            "--actions" => {
                options.actions = true;
                index += 1;
            }
            "--schema" => {
                options.schema = true;
                index += 1;
            }
            "--pretty" => {
                options.pretty = true;
                index += 1;
            }
            other if other.starts_with("--") => {
                return Err(ShellError::new(
                    ErrorCode::InvalidArgument,
                    format!("intent: unknown flag '{other}'"),
                ));
            }
            other => {
                positional.push(other.to_string());
                index += 1;
            }
        }
    }

    if options.json.is_none() && !positional.is_empty() {
        options.json = Some(positional.join(" "));
    }
    Ok(Some(options))
}

fn required_value(raw: &[String], index: usize, flag: &str) -> Result<String, ShellError> {
    raw.get(index + 1).cloned().ok_or_else(|| {
        ShellError::new(
            ErrorCode::InvalidArgument,
            format!("intent: {flag} requires a value"),
        )
    })
}

/// Drain the incoming stream (the Intent JSON) to completion.
fn collect_stream(in_rx: Option<PipeStream>) -> String {
    let Some(mut rx) = in_rx else {
        return String::new();
    };
    let handle = tokio::runtime::Handle::current();
    tokio::task::block_in_place(move || {
        handle.block_on(async move {
            let mut buffer = String::new();
            while let Some(payload) = rx.recv().await {
                match payload {
                    PipelinePayload::Data(value) => {
                        buffer.push_str(&value.to_text());
                        buffer.push('\n');
                    }
                    PipelinePayload::Bytes(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                    }
                    PipelinePayload::Structured(_) => {}
                }
            }
            buffer
        })
    })
}

fn process(json: &str, options: &Options, env: &Env, tx: &PipeSender) -> Result<(), ShellError> {
    let json = json.trim();
    if json.is_empty() {
        return Err(ShellError::new(
            ErrorCode::InvalidArgument,
            "intent: no Intent supplied (use --json, --file, or pipe one in)",
        ));
    }

    let intent = parse_intent(json)?;
    let platform = Platform::detect();
    let issues = validate_intent(&intent);

    match intent.mode {
        IntentMode::Inform => {
            let text = intent
                .info
                .as_ref()
                .and_then(render_info)
                .unwrap_or_else(|| {
                    "I cannot answer that deterministically; this build has no language model attached yet."
                        .to_string()
                });
            send_string(tx, text);
            Ok(())
        }
        IntentMode::Unsupported => {
            send_string(
                tx,
                "I could not map that request to a supported operation.".to_string(),
            );
            Ok(())
        }
        IntentMode::Perform | IntentMode::Explain => {
            let Some(action) = intent.action.as_ref() else {
                return Err(ShellError::new(
                    ErrorCode::InvalidArgument,
                    "intent: a perform/explain request needs an action",
                ));
            };

            if options.risk_only {
                send_string(tx, render_risk(action));
                return Ok(());
            }

            if !issues.is_empty() {
                send_string(tx, render_clarification(&intent));
                if options.run {
                    return Err(ShellError::new(
                        ErrorCode::InvalidArgument,
                        "intent: cannot run an incomplete action; information is missing",
                    ));
                }
                return Ok(());
            }

            let explain_only = options.explain || intent.mode == IntentMode::Explain;
            if explain_only || !options.run {
                send_string(tx, plan_text(action, &platform, options.target));
                return Ok(());
            }

            // `--run`: apply the structural risk gate before touching the shell.
            let risk = action.risk();
            if risk != RiskLevel::Safe && !confirm(action.kind(), risk, env) {
                send_string(
                    tx,
                    format!("intent: cancelled ({} was not confirmed)", action.kind()),
                );
                return Ok(());
            }

            env.log_audit(format!(
                "intent RUN {} (risk: {})",
                action.kind(),
                risk.label()
            ));
            execute(action, &platform, options.target, env, tx)
        }
    }
}

fn parse_intent(json: &str) -> Result<Intent, ShellError> {
    // Accept either a full Intent envelope or a bare Action. The envelope is detected by
    // the presence of an `action`/`mode`/`info` key; a bare action by its `kind` tag.
    let value: serde_json::Value = serde_json::from_str(json).map_err(|error| {
        ShellError::new(
            ErrorCode::InvalidArgument,
            format!("intent: invalid JSON: {error}"),
        )
    })?;
    let is_envelope =
        value.get("action").is_some() || value.get("mode").is_some() || value.get("info").is_some();
    if is_envelope {
        serde_json::from_value(value).map_err(|error| {
            ShellError::new(
                ErrorCode::InvalidArgument,
                format!("intent: invalid Intent: {error}"),
            )
        })
    } else if value.get("kind").is_some() {
        let action: Action = serde_json::from_value(value).map_err(|error| {
            ShellError::new(
                ErrorCode::InvalidArgument,
                format!("intent: invalid Action: {error}"),
            )
        })?;
        Ok(Intent::perform(action))
    } else {
        Err(ShellError::new(
            ErrorCode::InvalidArgument,
            "intent: JSON must be an Intent (with an action) or an Action (with a kind)",
        ))
    }
}

/// Execute a validated action, blocking until completion so results stream into `tx`.
fn execute(
    action: &Action,
    platform: &Platform,
    target: ShellTarget,
    env: &Env,
    tx: &PipeSender,
) -> Result<(), ShellError> {
    let handle = tokio::runtime::Handle::current();
    match target {
        ShellTarget::Fsh => {
            let pipeline =
                lower_fsh(action, platform).map_err(|e| ShellError::from(e.to_string()))?;
            let env = env.clone();
            // Drain the pipeline to completion and forward every payload: some builtins
            // (e.g. `ff`, `ps`) spawn their producer, so the stream only closes once that
            // producer has finished.
            let mut stream = fshell_engine::spawn_pipeline_stream(&pipeline, &env);
            let tx = tx.clone();
            tokio::task::block_in_place(move || {
                handle.block_on(async move {
                    while let Some(payload) = stream.recv().await {
                        if tx.send(payload).await.is_err() {
                            break;
                        }
                    }
                })
            });
            Ok(())
        }
        ShellTarget::Posix => {
            let script =
                lower_posix(action, platform).map_err(|e| ShellError::from(e.to_string()))?;
            let handler = fshell_engine::posix_handler().ok_or_else(|| {
                ShellError::from("intent: the POSIX handler is not registered in this build")
            })?;
            let env = env.clone();
            let result = tokio::task::block_in_place(move || {
                handle.block_on(handler(script, Vec::new(), env, true))
            })
            .map_err(|e| ShellError::from(e.to_string()))?;
            if let (_, Some(bytes)) = result {
                // Emit the captured output one line per record so downstream stages
                // (`count`, `grep`, ...) compose the same way as on the fsh target.
                let text = String::from_utf8_lossy(&bytes);
                for line in text.lines() {
                    send_string(tx, line.to_string());
                }
            }
            Ok(())
        }
    }
}

fn plan_text(action: &Action, platform: &Platform, target: ShellTarget) -> String {
    let mut lines = vec![render_risk(action)];
    let caps: Vec<String> = action
        .required_caps()
        .into_iter()
        .map(|handle| format!("{handle:?}"))
        .collect();
    if !caps.is_empty() {
        lines.push(format!("capabilities: {}", caps.join(", ")));
    }
    lines.push(format!(
        "fsh:   {}",
        render_fsh(action, platform).unwrap_or_else(|e| format!("<unavailable: {e}>"))
    ));
    lines.push(format!(
        "posix: {}",
        render_posix(action, platform).unwrap_or_else(|e| format!("<unavailable: {e}>"))
    ));
    lines.push(format!(
        "(target: {}, re-run with --run to execute)",
        target.label()
    ));
    lines.join("\n")
}

fn confirm(kind: &str, risk: RiskLevel, env: &Env) -> bool {
    if !std::io::stdin().is_terminal() {
        env.log_audit(format!(
            "intent DENIED {} (non-interactive {} action)",
            kind,
            risk.label()
        ));
        return false;
    }
    println!("\u{26A0}  '{kind}' is a {} operation.", risk.label());
    print!("Run it? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut input = String::new();
    let confirmed =
        std::io::stdin().read_line(&mut input).is_ok() && input.trim().eq_ignore_ascii_case("y");
    env.log_audit(format!(
        "intent {} {}",
        if confirmed { "CONFIRMED" } else { "DENIED" },
        kind
    ));
    confirmed
}

fn actions_text() -> String {
    let mut text = String::from("Supported semantic actions:\n");
    for spec in all_specs() {
        let required = if spec.required.is_empty() {
            String::new()
        } else {
            format!(" (requires: {})", spec.required.join(", "))
        };
        text.push_str(&format!(
            "  {:<20} [{}] {}{}\n",
            spec.kind,
            spec.base_risk.label(),
            spec.summary,
            required
        ));
    }
    text
}

fn send_string(tx: &PipeSender, text: String) {
    let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(text))));
}
