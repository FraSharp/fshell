// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! The intent envelope: what the user asked for, and how it should be handled.
//!
//! The whole point of this layer is that *what to do* (`Action`) is kept separate from
//! *how to treat the request* (`IntentMode`). "kill PID 1234", "how do I kill PID 1234?"
//! and "what does SIGTERM do?" are three different intents even when two of them share an
//! action.

use crate::action::{Action, Signal};
use serde::{Deserialize, Serialize};

/// How a request should be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentMode {
    /// Carry the action out (subject to validation, safety and confirmation).
    #[default]
    Perform,
    /// Show how the action *would* be performed, without executing it.
    Explain,
    /// Answer an informational question; nothing is executed.
    Inform,
    /// The request could not be mapped to a known action.
    Unsupported,
}

/// The complete semantic interpretation of a natural-language request.
///
/// This is exactly the shape a small function-calling model is expected to produce: a
/// mode, an optional typed action, and any issues (missing/ambiguous information) that
/// prevent execution. It round-trips through JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Intent {
    /// How the request should be handled.
    #[serde(default)]
    pub mode: IntentMode,
    /// The operation to perform or explain (for `Perform`/`Explain`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
    /// The informational question (for `Inform`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<InfoQuery>,
    /// Problems that must be resolved before the intent can be executed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<Issue>,
}

impl Intent {
    /// A complete `Perform` intent for an action.
    pub fn perform(action: Action) -> Self {
        Self {
            mode: IntentMode::Perform,
            action: Some(action),
            info: None,
            issues: Vec::new(),
        }
    }

    /// An `Explain` intent for an action.
    pub fn explain(action: Action) -> Self {
        Self {
            mode: IntentMode::Explain,
            action: Some(action),
            info: None,
            issues: Vec::new(),
        }
    }

    /// An informational intent.
    pub fn inform(query: InfoQuery) -> Self {
        Self {
            mode: IntentMode::Inform,
            action: None,
            info: Some(query),
            issues: Vec::new(),
        }
    }

    /// An intent that could not be mapped to a known action.
    pub fn unsupported() -> Self {
        Self {
            mode: IntentMode::Unsupported,
            action: None,
            info: None,
            issues: Vec::new(),
        }
    }

    /// A borrowed view of the intent's payload.
    pub fn semantics(&self) -> Option<Semantics<'_>> {
        match self.mode {
            IntentMode::Perform | IntentMode::Explain => {
                self.action.as_ref().map(Semantics::Action)
            }
            IntentMode::Inform => self.info.as_ref().map(Semantics::Inform),
            IntentMode::Unsupported => Some(Semantics::Unsupported),
        }
    }
}

/// A borrowed view over the payload of an [`Intent`].
#[derive(Debug, Clone, PartialEq)]
pub enum Semantics<'a> {
    /// A concrete operation.
    Action(&'a Action),
    /// An informational question.
    Inform(&'a InfoQuery),
    /// Nothing understood.
    Unsupported,
}

/// An informational question, e.g. "what does SIGTERM do?".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InfoQuery {
    /// The meaning of a signal.
    Signal {
        /// The signal being asked about.
        signal: Signal,
        /// The original question text, when available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        question: Option<String>,
    },
    /// What a tool or command does.
    Tool {
        /// Tool name.
        name: String,
        /// The original question text, when available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        question: Option<String>,
    },
    /// A general shell or operating-system concept.
    Concept {
        /// Concept name.
        name: String,
        /// The original question text, when available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        question: Option<String>,
    },
}

/// A problem preventing an intent from being executed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Issue {
    /// A required parameter was not provided.
    Missing {
        /// Parameter name.
        param: String,
        /// Human description of what is missing.
        description: String,
    },
    /// A parameter could be read more than one way.
    Ambiguous {
        /// Parameter name.
        param: String,
        /// The candidate readings.
        candidates: Vec<String>,
        /// Why the request is ambiguous.
        reason: String,
    },
    /// A parameter was provided but is not valid.
    Invalid {
        /// Parameter name.
        param: String,
        /// Why the value is invalid.
        reason: String,
    },
}

impl Issue {
    /// Build a [`Issue::Missing`].
    pub fn missing(param: impl Into<String>, description: impl Into<String>) -> Self {
        Issue::Missing {
            param: param.into(),
            description: description.into(),
        }
    }

    /// Build an [`Issue::Ambiguous`].
    pub fn ambiguous(
        param: impl Into<String>,
        candidates: Vec<String>,
        reason: impl Into<String>,
    ) -> Self {
        Issue::Ambiguous {
            param: param.into(),
            candidates,
            reason: reason.into(),
        }
    }

    /// Build an [`Issue::Invalid`].
    pub fn invalid(param: impl Into<String>, reason: impl Into<String>) -> Self {
        Issue::Invalid {
            param: param.into(),
            reason: reason.into(),
        }
    }

    /// The parameter this issue concerns.
    pub fn param(&self) -> &str {
        match self {
            Issue::Missing { param, .. }
            | Issue::Ambiguous { param, .. }
            | Issue::Invalid { param, .. } => param,
        }
    }
}

/// How dangerous an action is.
///
/// This is a *structural* classification of what the action does, independent of how it
/// is lowered, so it can be inspected before any shell code exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Read-only / informational.
    Safe,
    /// Changes local state but is recoverable.
    Caution,
    /// Destructive or privilege-affecting.
    Destructive,
}

impl RiskLevel {
    /// A short lowercase label.
    pub const fn label(self) -> &'static str {
        match self {
            RiskLevel::Safe => "safe",
            RiskLevel::Caution => "caution",
            RiskLevel::Destructive => "destructive",
        }
    }
}
