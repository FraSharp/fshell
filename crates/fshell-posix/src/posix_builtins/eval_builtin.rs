// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::{eval::EvalConfig, parser::parse_posix_script};
use fshell_engine::Env;

/// POSIX `eval` — re-parse and evaluate arguments as a POSIX script in the caller's context.
pub async fn eval_posix(args: &[String], env: &Env) -> Result<i32, fshell_engine::EngineError> {
    if args.is_empty() {
        return Ok(0);
    }
    let script = args.join(" ");
    let parsed = parse_posix_script(&script)?;
    let cfg = EvalConfig::default();
    crate::eval::eval_source(&parsed, env, &cfg).await
}
