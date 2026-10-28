// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Subshells and asynchronous lists, run as child `fsh` processes.
//!
//! The child receives the already-parsed list, not source text: brush records
//! no source span for a redirection, and parsed shell code cannot be rendered
//! back to text faithfully, so re-parsing a fragment loses meaning. With the
//! list it receives the parent's positional parameters, `$0` and `$$`, which a
//! subshell shares with the shell it was forked from.

use brush_parser::ast::{AndOrList, CompoundList, CompoundListItem, Program, SeparatorOperator};
use fshell_core::Val;
use fshell_engine::background::ChildPayload;
use fshell_engine::{EngineError, Env};
use serde::{Deserialize, Serialize};

use crate::eval::{EvalConfig, PosixError, eval_source};
use crate::parser::ParsedScript;

#[derive(Serialize, Deserialize)]
struct ChildProgram {
    program: Program,
    positional: Vec<String>,
    arg0: String,
    shell_pid: u32,
}

/// The payload that runs `list` in a child shell of `env`.
pub(crate) fn list_payload(
    list: &CompoundList,
    env: &Env,
    cfg: &EvalConfig,
) -> Result<ChildPayload, PosixError> {
    let program = ChildProgram {
        program: Program {
            complete_commands: vec![list.clone()],
        },
        positional: crate::expand::get_effective_positional(env, &cfg.positional),
        arg0: env
            .vars
            .read()
            .get("0")
            .map(Val::to_text)
            .unwrap_or_else(|| "fsh".to_string()),
        shell_pid: fshell_engine::background::shell_pid(),
    };
    serde_json::to_string(&program)
        .map(ChildPayload::PosixProgram)
        .map_err(|error| {
            PosixError::Engine(EngineError::Generic {
                message: format!("child shell: cannot serialize program: {error}"),
                span: None,
            })
        })
}

/// The payload that runs one and-or list, as `list &` does.
pub(crate) fn and_or_payload(
    list: &AndOrList,
    env: &Env,
    cfg: &EvalConfig,
) -> Result<ChildPayload, PosixError> {
    list_payload(
        &CompoundList(vec![CompoundListItem(
            list.clone(),
            SeparatorOperator::Sequence,
        )]),
        env,
        cfg,
    )
}

/// Evaluate a program written by [`list_payload`], in the child shell.
pub async fn run_child_program(serialized: &str, env: &Env) -> Result<i32, EngineError> {
    let child: ChildProgram =
        serde_json::from_str(serialized).map_err(|error| EngineError::Generic {
            message: format!("child shell: malformed program: {error}"),
            span: None,
        })?;
    fshell_engine::background::inherit_shell_pid(child.shell_pid);
    {
        let mut vars = env.vars.write();
        vars.insert("0".to_string(), Val::String(child.arg0));
        crate::positional::replace(
            &mut vars,
            child.positional.into_iter().map(Val::String).collect(),
        );
    }
    let parsed = ParsedScript {
        program: child.program,
        source: String::new(),
    };
    eval_source(&parsed, env, &EvalConfig::default()).await
}
