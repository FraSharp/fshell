// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! A shell-independent semantic layer for fshell.
//!
//! This crate captures *what the user wants* as typed [`Action`]s, validates them, and
//! lowers them to a concrete shell target: fsh (native pipelines) or POSIX (shell source).
//! It contains no model integration — its job is to make fshell structurally suitable for
//! a small function-calling model by keeping intent separate from shell semantics.
//!
//! ```
//! use fshell_semantic::{Action, FindFiles, Platform, ShellTarget, render_fsh};
//! # use fshell_semantic::{ByteSize, TimeSpan};
//!
//! let action = Action::FindFiles(FindFiles {
//!     root: Some(".".to_string()),
//!     extension: Some("log".to_string()),
//!     min_size: Some(ByteSize::parse("500MB").unwrap()),
//!     modified_within: Some(TimeSpan::parse("7d").unwrap()),
//!     ..Default::default()
//! });
//! let platform = Platform::detect();
//! let fsh = render_fsh(&action, &platform).unwrap();
//! assert!(fsh.contains("ff"));
//! # let _ = ShellTarget::Fsh;
//! ```

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

pub mod action;
pub mod error;
pub mod intent;
pub mod lower;
pub mod platform;
pub mod render;
pub mod spec;
pub mod validate;

#[cfg(feature = "schema")]
pub mod schema;

pub use action::*;
pub use error::SemanticError;
pub use intent::{InfoQuery, Intent, IntentMode, Issue, RiskLevel, Semantics};
pub use lower::{LowerError, Lowered, ShellTarget, lower, lower_fsh, lower_posix};
pub use platform::{Os, Platform, UtilitySet};
pub use render::{render_clarification, render_fsh, render_info, render_posix, render_risk};
pub use spec::{ActionSpec, Category, all_specs, spec_for};
pub use validate::{intent_is_complete, is_complete, validate, validate_intent};

#[cfg(feature = "schema")]
pub use schema::{ToolSchema, functiongemma_tools, parameter_schema, tools_json};

/// Initialize the semantic layer.
pub fn init() {}

#[cfg(test)]
mod tests;
