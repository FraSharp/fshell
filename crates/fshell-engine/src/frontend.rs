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
/// The native parser wins when it accepts. When it reports a definite error we
/// ask the POSIX classifier: if POSIX accepts, the input is runnable; if POSIX
/// is itself unterminated, it is a continuation. When the native parser only
/// reports "incomplete", we keep prompting (native has priority for its own
/// syntax), which avoids turning native typos into POSIX commands.
pub fn classify_input(input: &str) -> InputClass {
    match fshell_core::validate_input(input) {
        ValidationResult::Complete => InputClass::Complete,
        ValidationResult::Incomplete { prompt_hint } => {
            InputClass::Incomplete { hint: prompt_hint }
        }
        ValidationResult::Invalid { message, .. } => match crate::classify_posix(input) {
            Some(PosixSyntax::Complete) => InputClass::Complete,
            Some(PosixSyntax::Incomplete) => InputClass::Incomplete { hint: "posix" },
            _ => InputClass::Invalid { message },
        },
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

/// First whitespace/separator-delimited word of the input, lowercased.
fn first_word(input: &str) -> String {
    input
        .trim_start()
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '(' | '{' | '&' | '|'))
        .find(|w| !w.is_empty())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Whether the input opens with a keyword that exists only in fsh-native syntax.
///
/// For these, a parse error is a genuine native error and must not be
/// reinterpreted as POSIX — otherwise a native typo like `let x = | invalid`
/// would silently run as a shell command. Keywords shared with POSIX
/// (`if`, `for`, `while`, …) are deliberately absent so real POSIX scripts
/// still fall back.
fn starts_with_native_keyword(input: &str) -> bool {
    matches!(
        first_word(input).as_str(),
        "let" | "local" | "fn" | "match" | "try" | "catch" | "with" | "unsafe" | "caps"
    )
}

/// Fall back to POSIX when the mode allows it and the input is valid POSIX.
///
/// Returns `None` when the caller should surface its own (native) error.
pub async fn try_posix_fallback(input: &str, env: &Env) -> Result<Option<Flow>, EngineError> {
    if env.options.read().interp_mode == InterpMode::Native {
        return Ok(None);
    }
    if starts_with_native_keyword(input) {
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
    fn first_word_skips_separators() {
        assert_eq!(first_word("  cd /tmp && ls"), "cd");
        assert_eq!(first_word("(echo a); (echo b)"), "echo");
        assert_eq!(first_word("P=$!; echo x"), "p=$!");
        assert_eq!(first_word(""), "");
    }

    #[test]
    fn native_only_keywords_do_not_fall_back() {
        for input in [
            "let x = | invalid",
            "match x { }",
            "try { } catch { }",
            "with caps(fs) { }",
        ] {
            assert!(
                starts_with_native_keyword(input),
                "should stay native: {input:?}"
            );
        }
    }

    #[test]
    fn posix_shared_keywords_may_fall_back() {
        for input in [
            "for f in a b; do echo $f; done",
            "if true; then echo; fi",
            "echo hi",
        ] {
            assert!(
                !starts_with_native_keyword(input),
                "should be eligible for POSIX: {input:?}"
            );
        }
    }
}
