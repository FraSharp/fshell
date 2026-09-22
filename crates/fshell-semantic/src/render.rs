// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Rendering semantic actions back to human-facing text and shell source.

use crate::action::{Action, Signal};
use crate::intent::{InfoQuery, Intent, Issue};
use crate::lower::{LowerError, lower_fsh, lower_posix};
use crate::platform::Platform;

/// Render the fsh form of an action using the engine's AST printer.
pub fn render_fsh(action: &Action, platform: &Platform) -> Result<String, LowerError> {
    Ok(fshell_engine::format_pipeline(&lower_fsh(
        action, platform,
    )?))
}

/// Render the POSIX form of an action.
pub fn render_posix(action: &Action, platform: &Platform) -> Result<String, LowerError> {
    lower_posix(action, platform)
}

/// A one-line risk summary for an action.
pub fn render_risk(action: &Action) -> String {
    format!(
        "{} — {} (risk: {})",
        action.kind(),
        action.spec().summary,
        action.risk().label()
    )
}

/// Human-readable clarification text for an intent that is missing information.
///
/// Empty when there is nothing to clarify. The wording is produced by fshell, not the
/// model — the model only has to report *what* is missing.
pub fn render_clarification(intent: &Intent) -> String {
    let issues = crate::validate::validate_intent(intent);
    if issues.is_empty() {
        return String::new();
    }

    let mut lines = Vec::new();

    let missing: Vec<String> = issues
        .iter()
        .filter_map(|issue| match issue {
            Issue::Missing { param, description } => Some(format!("{param} ({description})")),
            _ => None,
        })
        .collect();
    if !missing.is_empty() {
        lines.push(format!(
            "I need more information before I can do that: {}.",
            missing.join(", ")
        ));
    }

    for issue in &issues {
        match issue {
            Issue::Ambiguous {
                param,
                candidates,
                reason,
            } => {
                if candidates.is_empty() {
                    lines.push(format!("The request is ambiguous: {reason} ({param})."));
                } else {
                    lines.push(format!(
                        "The request is ambiguous ({reason}); did you mean {}?",
                        candidates.join(", ")
                    ));
                }
            }
            Issue::Invalid { param, reason } => {
                lines.push(format!(
                    "The value supplied for {param} is invalid: {reason}."
                ));
            }
            Issue::Missing { .. } => {}
        }
    }

    if let Some(action) = &intent.action
        && let Ok(extracted) = serde_json::to_string(action)
    {
        lines.push(format!("Understood so far: {extracted}"));
    }

    lines.join("\n")
}

/// Deterministic answers for informational questions, where fshell can answer without a
/// model. Returns `None` when the topic needs world knowledge fshell does not have.
pub fn render_info(query: &InfoQuery) -> Option<String> {
    match query {
        InfoQuery::Signal { signal, .. } => Some(signal_meaning(*signal).to_string()),
        InfoQuery::Tool { .. } | InfoQuery::Concept { .. } => None,
    }
}

fn signal_meaning(signal: Signal) -> &'static str {
    match signal {
        Signal::Term => {
            "SIGTERM asks a process to terminate gracefully so it can clean up. It can be caught or ignored."
        }
        Signal::Kill => {
            "SIGKILL terminates a process immediately and cannot be caught, blocked or ignored."
        }
        Signal::Interrupt => {
            "SIGINT is the interrupt signal sent by Ctrl-C; it asks a program to stop."
        }
        Signal::Hup => {
            "SIGHUP originally meant the controlling terminal hung up; daemons often reload their configuration on it."
        }
        Signal::Quit => "SIGQUIT asks a process to quit and produce a core dump.",
        Signal::Stop => "SIGSTOP suspends a process and cannot be caught or ignored.",
        Signal::Cont => "SIGCONT resumes a previously stopped process.",
        Signal::Usr1 => "SIGUSR1 is a user-defined signal; its meaning is chosen by the program.",
        Signal::Usr2 => "SIGUSR2 is a user-defined signal; its meaning is chosen by the program.",
    }
}
