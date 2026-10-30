// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Unified interpretation front-end.
//!
//! fsh runs two engines over overlapping syntax: the native structured engine
//! and the POSIX engine. This module is the single place that decides which
//! engine handles a piece of input — for both execution and the interactive
//! continuation prompt — so the two decisions can never disagree.
//!
//! The fallback happens strictly at the *parse* boundary: nothing has executed
//! yet, so choosing POSIX cannot double-run side effects.

use crate::{EngineError, Env, Flow, PosixSyntax};
use fshell_core::ValidationResult;

/// Which engine should interpret shell input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum InterpMode {
    /// Native first; fall back to POSIX when the input is valid POSIX.
    #[default]
    Auto,
    /// Native only — never fall back to POSIX.
    Native,
    /// POSIX only.
    Posix,
}

/// Completion status of a piece of input, engine-agnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputClass {
    /// Some engine can run it.
    Complete,
    /// Valid so far; the editor should keep prompting.
    Incomplete { hint: &'static str },
    /// A definite error.
    Invalid { message: String },
}

/// Decide whether `input` is complete, incomplete, or invalid, consulting both
/// engines.
///
/// The native parser wins when it accepts. When it reports a definite error,
/// the input is only offered to POSIX if it carries no native-only construct:
/// `let x = 1` is a native declaration even though a POSIX parser can read it
/// as a command named `let`. When POSIX is merely unterminated, the editor
/// keeps prompting only if the input opens with a POSIX construct; a native
/// error such as a stray `}` must not become an endless continuation just
/// because brush reports it as an unterminated program.
pub fn classify_input(input: &str) -> InputClass {
    let inspection = fshell_core::inspect_input(input);
    match inspection.result {
        ValidationResult::Complete => InputClass::Complete,
        ValidationResult::Incomplete { prompt_hint } => {
            InputClass::Incomplete { hint: prompt_hint }
        }
        ValidationResult::Invalid { message, .. } => {
            if inspection.native_intent {
                return InputClass::Invalid { message };
            }
            match crate::classify_posix(input) {
                Some(PosixSyntax::Complete) => InputClass::Complete,
                Some(PosixSyntax::Incomplete) if starts_with_posix_construct(input) => {
                    InputClass::Incomplete { hint: "posix" }
                }
                _ => InputClass::Invalid { message },
            }
        }
    }
}

/// Whether the first meaningful line opens with a construct only POSIX owns.
///
/// This is consulted only for *incomplete* input: a complete POSIX script is
/// accepted by the POSIX classifier on its own. Leading comments and plain
/// assignments are skipped so a script that starts with `X=1` can still
/// continue into its first loop or conditional.
fn starts_with_posix_construct(input: &str) -> bool {
    let line = input
        .lines()
        .map(str::trim_start)
        .find(|line| !line.is_empty() && !line.starts_with('#') && !is_posix_assignment_line(line))
        .unwrap_or("");
    let word = line
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '(' | '{' | '&' | '|'))
        .find(|word| !word.is_empty())
        .unwrap_or("");
    matches!(
        word,
        "if" | "for" | "while" | "until" | "case" | "function" | "select"
    )
}

/// A leading `NAME=value` assignment, which POSIX allows before a command.
fn is_posix_assignment_line(line: &str) -> bool {
    let word = line.split_whitespace().next().unwrap_or("");
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// Environment variable naming a file to append routing decisions to.
///
/// Deliberately unset in normal use: this is a test instrument, not a feature.
/// When unset the whole cost is one environment lookup and nothing is written,
/// so it cannot change what a user observes.
pub const ENGINE_TRACE_VAR: &str = "FSH_ENGINE_TRACE";

/// Why the router chose an engine.
///
/// The reason matters as much as the engine: `native-incompatible:PosixTildeUser`
/// and `native-parse-failed` both end in POSIX but mean opposite things about
/// native's capabilities. Recording only the engine would let a case pass for
/// the wrong reason — and would let the router drift into "prefer POSIX when
/// possible" without anyone noticing.
pub mod reason {
    /// `--posix` was passed: the user chose POSIX.
    pub const EXPLICIT_POSIX: &str = "explicit-posix";
    /// A script's shebang asked for POSIX, not the flag.
    pub const POSIX_SHEBANG: &str = "posix-shebang";
    /// `--native` was passed: the user chose native.
    pub const EXPLICIT_NATIVE: &str = "explicit-native";
    /// Auto mode, the native parser accepted, and native owns these semantics.
    pub const NATIVE_SAFE: &str = "native-safe";
    /// Auto mode and the native parser rejected the input, so POSIX got it.
    pub const NATIVE_PARSE_FAILED: &str = "native-parse-failed";
    /// Auto mode, native parsed the input, but the construct needs POSIX
    /// semantics native deliberately does not provide.
    pub fn native_incompatible(requirement: &str) -> String {
        format!("native-incompatible:{requirement}")
    }
}

/// Append one routing decision to `$FSH_ENGINE_TRACE`, when that is set.
///
/// Best effort by construction — tracing must never change shell semantics, so
/// an unset, empty, or unwritable path is silently ignored. Nothing is written
/// to stdout or stderr: the trace is a file, so it cannot contaminate a
/// pipeline or perturb a comparison.
pub fn trace_engine(engine: &str, reason: &str) {
    use std::io::Write as _;

    let Some(path) = std::env::var_os(ENGINE_TRACE_VAR) else {
        return;
    };
    if path.is_empty() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "engine={engine} reason={reason}");
    }
}

/// Run `input` through the POSIX engine.
pub async fn run_posix(input: &str, env: &Env) -> Result<Flow, EngineError> {
    let handler = crate::posix_handler().ok_or_else(|| EngineError::Generic {
        message: "POSIX engine is not available".to_string(),
        span: None,
    })?;
    if env.options.read().posix_fallback_notice && crate::is_stdout_a_tty() {
        eprintln!("\x1b[2m⟨posix⟩\x1b[0m");
    }
    let (code, _) = handler(input.to_string(), Vec::new(), env.clone(), false).await?;
    env.set_exit_code(code as i64);
    Ok(Flow::Normal)
}

/// Fall back to POSIX when the mode allows it and the input is valid POSIX.
///
/// Returns `None` when the caller should surface its own (native) error. The
/// caller has already refused the fallback for input with native intent.
pub async fn try_posix_fallback(input: &str, env: &Env) -> Result<Option<Flow>, EngineError> {
    if env.options.read().interp_mode == InterpMode::Native {
        return Ok(None);
    }
    if crate::posix_handler().is_none() {
        return Ok(None);
    }
    if !matches!(crate::classify_posix(input), Some(PosixSyntax::Complete)) {
        return Ok(None);
    }
    trace_engine("posix", reason::NATIVE_PARSE_FAILED);
    Ok(Some(run_posix(input, env).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_construct_detection_skips_comments_and_assignments() {
        for input in [
            "for i in 1 2; do",
            "if true; then",
            "case x in",
            "# setup\nfor i in 1 2; do",
            "X=1\nfor i in 1 2; do",
        ] {
            assert!(
                starts_with_posix_construct(input),
                "should be a POSIX continuation: {input:?}"
            );
        }
        for input in ["}", "let x = 1", "echo hi", "sh script.sh", "done"] {
            assert!(
                !starts_with_posix_construct(input),
                "should not be a POSIX continuation: {input:?}"
            );
        }
    }

    #[test]
    fn native_intent_is_never_reinterpreted_as_posix() {
        // A POSIX parser can read `let`/`fn` as command names, but the input
        // began in native syntax; the classifier must report the native error.
        for input in [
            "let x = | invalid",
            "fn foo )",
            "# setup\nlet x = 1\nfor i in 1 2; do",
        ] {
            assert!(
                matches!(classify_input(input), InputClass::Invalid { .. }),
                "should stay native: {input:?}"
            );
        }
    }

    #[test]
    fn definite_native_errors_are_not_posix_continuations() {
        for input in ["}", "let x = |", "items |?"] {
            assert!(
                matches!(classify_input(input), InputClass::Invalid { .. }),
                "should be invalid, not a continuation: {input:?}"
            );
        }
    }
}
