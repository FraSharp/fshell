// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Validation and completeness checking for semantic actions and intents.

use crate::action::Action;
use crate::intent::{Intent, IntentMode, Issue};
use crate::spec::param_description;

/// Check an action for missing or invalid information.
///
/// An empty result means the action is complete and internally consistent.
pub fn validate(action: &Action) -> Vec<Issue> {
    let mut issues = Vec::new();

    for &param in action.required() {
        if action.param_missing(param) {
            issues.push(Issue::missing(
                param,
                param_description(action.kind(), param),
            ));
        }
    }

    match action {
        Action::FindFiles(query) => {
            if let (Some(min), Some(max)) = (query.min_size, query.max_size)
                && min.bytes() > max.bytes()
            {
                issues.push(Issue::invalid(
                    "min_size",
                    format!("minimum size {min} is greater than maximum size {max}"),
                ));
            }
        }
        Action::RunContainer(run) => {
            for (index, mapping) in run.ports.iter().enumerate() {
                if mapping.host_port.is_none() {
                    issues.push(Issue::missing(
                        format!("ports[{index}].host_port"),
                        "the host port to publish",
                    ));
                }
            }
        }
        Action::ListeningPorts(ports) if ports.port == Some(0) => {
            issues.push(Issue::invalid("port", "port 0 is not a valid listen port"));
        }
        Action::ListeningPorts(_) => {}
        _ => {}
    }

    issues
}

/// Whether an action is complete enough to be lowered and executed.
pub fn is_complete(action: &Action) -> bool {
    validate(action).is_empty()
}

/// The full issue list for an intent: its own declared issues plus any discovered while
/// validating the action.
pub fn validate_intent(intent: &Intent) -> Vec<Issue> {
    let mut issues = intent.issues.clone();
    if matches!(intent.mode, IntentMode::Perform | IntentMode::Explain) {
        if let Some(action) = &intent.action {
            issues.extend(validate(action));
        } else {
            issues.push(Issue::missing("action", "an operation to perform"));
        }
    }
    // Drop exact duplicates (same parameter and same kind of issue) while preserving
    // order, so a model-declared "missing image" is not repeated by validation, but an
    // "ambiguous image" and a derived "missing image" both survive.
    issues.dedup_by(|a, b| {
        a.param() == b.param() && std::mem::discriminant(a) == std::mem::discriminant(b)
    });
    issues
}

/// Whether an intent is complete enough to be executed.
pub fn intent_is_complete(intent: &Intent) -> bool {
    matches!(intent.mode, IntentMode::Perform | IntentMode::Explain)
        && intent.action.is_some()
        && validate_intent(intent).is_empty()
}
