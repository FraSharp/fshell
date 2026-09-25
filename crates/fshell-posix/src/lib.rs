// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
#![allow(clippy::result_large_err)]

//! `fshell-posix` — Polyglot POSIX compatibility engine for fshell.
//!
//! Provides a dedicated POSIX parsing, expansion, and evaluation frontend that
//! shares the unified `fshell-engine` runtime (dual-stream PipelinePayload,
//! Env, capabilities, diagnostics).
//!
//! Phases per POSIX-COMPATIBILITY-DESIGN.md:
//!   1 — brush-parser AST
//!   2 — dual-stream (owned by fshell-engine::PipelinePayload::Bytes)
//!   3 — 4-phase word expansion (IFS splitting, glob, arith, cmdsubst)
//!   4 — evaluator + builtins (test, ., eval, export, ...)
//!   5 — source bridge replacement
//!   6 — compliance tests

pub mod arithmetic;
pub mod bridge;
pub mod eval;
pub mod expand;
pub mod parser;
pub mod posix_builtins;

pub use arithmetic::eval_arithmetic_expr;
pub use eval::{EvalConfig, PosixExit, eval_source, eval_source_capture, eval_source_stream};
pub use expand::{ExpansionConfig, expand_word, split_ifs};
pub use parser::{ParsedScript, PosixSyntax, classify_posix, parse_posix_script};

/// POSIX shell exit status — 0 success, non-zero failure, 127 not found, 126 not executable.
pub type ExitCode = i32;

/// Install the POSIX engine into `fshell-engine`.
///
/// Registers both the evaluation handler and the syntax classifier, so the
/// engine's unified front-end can fall back to POSIX and decide continuation
/// state. Idempotent; call once per process (the binary and the test harness
/// both do).
pub fn install_engine_hooks() {
    fshell_engine::register_posix_handler(
        |content: String, args: Vec<String>, env: fshell_engine::Env, capture: bool| async move {
            let parsed = parse_posix_script(&content)?;
            let cfg = EvalConfig {
                positional: args,
                ..Default::default()
            };
            let result = eval_source_stream(&parsed, &env, &cfg, capture).await;
            // Whatever way the script left — fell off the end, `exit`, or a hard
            // error — the EXIT handler runs: that is what `trap … EXIT` means.
            let trap = eval::run_exit_trap(&env).await;
            match result {
                Err(error) => Err(error),
                Ok(value) => trap.map(|()| value),
            }
        },
    );
    fshell_engine::register_posix_classifier(|input: &str| match classify_posix(input) {
        PosixSyntax::Complete => fshell_engine::PosixSyntax::Complete,
        PosixSyntax::Incomplete => fshell_engine::PosixSyntax::Incomplete,
        PosixSyntax::Invalid => fshell_engine::PosixSyntax::Invalid,
    });
}
