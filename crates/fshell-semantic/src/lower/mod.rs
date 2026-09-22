// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Lowering a semantic [`Action`] to a concrete shell target.
//!
//! Two targets are supported: **fsh**, which builds a native `fshell_core::Pipeline` (and
//! prefers fshell's own builtins and pipeline operators), and **POSIX**, which produces a
//! portable shell script string. The same action may lower very differently on each, and
//! differently again per platform.

pub mod fsh;
pub mod posix;

use fshell_core::Pipeline;

use crate::action::Action;
use crate::error::SemanticError;
use crate::platform::Platform;

/// The error type produced while lowering. Aliased for readability at call sites.
pub type LowerError = SemanticError;

/// A shell backend an action can be lowered to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShellTarget {
    /// fshell's native language and builtins.
    Fsh,
    /// POSIX/Bash-compatible shell source.
    Posix,
}

impl ShellTarget {
    /// Parse a target name (`"fsh"` or `"posix"`/`"sh"`/`"bash"`).
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "fsh" => Some(ShellTarget::Fsh),
            "posix" | "sh" | "bash" => Some(ShellTarget::Posix),
            _ => None,
        }
    }

    /// The canonical lowercase name.
    pub const fn label(self) -> &'static str {
        match self {
            ShellTarget::Fsh => "fsh",
            ShellTarget::Posix => "posix",
        }
    }
}

/// An action lowered to a concrete representation.
#[derive(Debug, Clone, PartialEq)]
pub enum Lowered {
    /// A native fsh pipeline.
    Fsh(Pipeline),
    /// A POSIX shell script.
    Posix(String),
}

/// Lower an action to the given target.
pub fn lower(
    action: &Action,
    target: ShellTarget,
    platform: &Platform,
) -> Result<Lowered, LowerError> {
    match target {
        ShellTarget::Fsh => Ok(Lowered::Fsh(lower_fsh(action, platform)?)),
        ShellTarget::Posix => Ok(Lowered::Posix(lower_posix(action, platform)?)),
    }
}

/// Lower an action to a native fsh pipeline.
pub fn lower_fsh(action: &Action, platform: &Platform) -> Result<Pipeline, LowerError> {
    fsh::lower(action, platform)
}

/// Lower an action to a POSIX shell script.
pub fn lower_posix(action: &Action, platform: &Platform) -> Result<String, LowerError> {
    posix::lower(action, platform)
}

pub(crate) fn required_str(
    value: &Option<String>,
    kind: &str,
    param: &str,
) -> Result<String, LowerError> {
    match value.as_deref() {
        Some(v) if !v.trim().is_empty() => Ok(v.to_string()),
        _ => Err(LowerError::InvalidValue(format!(
            "missing required parameter '{param}' for {kind}"
        ))),
    }
}
