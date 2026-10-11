// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::{ErrorCode, ShellError, Val};
use fshell_engine::{Env, PipeSender, PipeStream};
use miette::SourceSpan;

/// Native explicit external invocation. POSIX's `command` frontend retains its
/// own standard lookup semantics. The fallback owns capabilities, process
/// lifecycle, status, output draining, and sandboxing exactly as for a bare
/// external command; this builtin does not launch its own subprocess.
pub async fn command_builtin(
    input: Option<PipeStream>,
    mut args: Vec<Val>,
    env: Env,
    output: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    if matches!(args.first(), Some(Val::String(s)) if s == "--") {
        args.remove(0);
    }
    let Some(Val::String(name)) = args.first() else {
        return Err(ShellError::new(
            ErrorCode::MissingArgument,
            "command: expected an external command name",
        )
        .with_exit_status(2)
        .maybe_with_span(span));
    };
    if name.is_empty() || name.starts_with('-') {
        return Err(ShellError::new(
            ErrorCode::InvalidArgument,
            "command: expected an external command name",
        )
        .with_exit_status(2)
        .maybe_with_span(span));
    }
    let name = name.clone();
    args.remove(0);
    let handler = env.get_async_fallback_handler().ok_or_else(|| {
        ShellError::new(
            ErrorCode::CommandNotFound,
            "external command execution is unavailable",
        )
        .maybe_with_span(span)
    })?;
    let has_next = !env.is_last_stage;
    handler(&name, args, input, env, output, has_next, span).await
}
