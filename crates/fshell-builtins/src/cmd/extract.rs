// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::utils::resolve_user_path;
use fshell_archive::Limits;
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_core::diagnostic::ErrorCode;
use fshell_engine::{CapAction, Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;
use std::sync::Arc;

fn invalid(message: impl Into<String>, span: Option<SourceSpan>) -> ShellError {
    ShellError::new(ErrorCode::InvalidArgument, message.into()).maybe_with_span(span)
}

/// Extract one archive into the current directory or an existing destination.
/// The entire operation runs on the engine's blocking pool, so a failing decode
/// returns Err to the pipeline instead of reporting success from a detached task.
pub fn extract_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let mut archive = None;
    let mut destination = None;
    let mut limits = Limits::default();
    let mut options = true;
    let mut args = args.iter();
    while let Some(value) = args.next() {
        let Val::String(arg) = value else {
            return Err(invalid("extract: arguments must be strings", span));
        };
        if options && arg == "--" {
            options = false;
            continue;
        }
        if options
            && matches!(
                arg.as_str(),
                "-C" | "--directory" | "--max-bytes" | "--max-entries"
            )
        {
            let next = args
                .next()
                .ok_or_else(|| invalid(format!("extract: {arg} needs a value"), span))?;
            let value = match next {
                Val::String(s) => s.clone(),
                Val::Int(n) => n.to_string(),
                _ => return Err(invalid(format!("extract: invalid value for {arg}"), span)),
            };
            match arg.as_str() {
                "-C" | "--directory" if destination.is_none() => destination = Some(value),
                "--max-bytes" => {
                    limits.max_bytes = value.parse::<u64>().map_err(|_| {
                        invalid("extract: --max-bytes must be a nonnegative integer", span)
                    })?;
                }
                "--max-entries" => {
                    limits.max_entries = value.parse::<usize>().map_err(|_| {
                        invalid("extract: --max-entries must be a nonnegative integer", span)
                    })?;
                }
                _ => return Err(invalid(format!("extract: repeated {arg}"), span)),
            }
        } else if options && arg.starts_with('-') {
            return Err(invalid(
                format!("extract: unknown option {arg} (use -- before a path starting with -)"),
                span,
            ));
        } else if archive.replace(arg.clone()).is_some() {
            return Err(invalid("extract: exactly one archive is required", span));
        }
    }
    let archive = archive.ok_or_else(|| {
        invalid(
            "usage: extract [-C DIR] [--max-bytes N] [--max-entries N] <archive>",
            span,
        )
    })?;
    let raw_path = resolve_user_path(&archive, env);
    let archive_path = std::fs::canonicalize(&raw_path).map_err(|e| {
        ShellError::new(
            ErrorCode::IoError,
            format!("extract: cannot resolve archive {:?}: {e}", raw_path),
        )
        .maybe_with_span(span)
    })?;
    env.enforce_capability("extract", CapAction::ReadFile(archive_path.clone()))?;
    let dest_path = destination
        .as_deref()
        .map_or_else(|| env.cwd(), |p| resolve_user_path(p, env));
    let dest_path = std::fs::canonicalize(&dest_path).map_err(|e| {
        ShellError::new(
            ErrorCode::IoError,
            format!("extract: cannot resolve destination {:?}: {e}", dest_path),
        )
        .maybe_with_span(span)
    })?;
    env.enforce_capability("extract", CapAction::WriteDir(dest_path.clone()))?;
    env.track_read(archive_path.clone());

    let mut file = std::fs::File::open(&archive_path).map_err(|e| {
        ShellError::new(
            ErrorCode::IoError,
            format!("extract: cannot open {:?}: {e}", archive_path),
        )
        .maybe_with_span(span)
    })?;
    if !file
        .metadata()
        .map_err(|e| ShellError::new(ErrorCode::IoError, e.to_string()))?
        .is_file()
    {
        return Err(invalid(
            format!("extract: {:?} is not a regular file", archive_path),
            span,
        ));
    }
    let count = fshell_archive::extract(
        &mut file,
        archive_path.file_name().unwrap_or_default(),
        &dest_path,
        limits,
    )
    .map_err(|e| {
        ShellError::new(ErrorCode::IoError, format!("extract: {e}")).maybe_with_span(span)
    })?;
    let _ = tx.blocking_send(PipelinePayload::Data(Arc::new(Val::String(format!(
        "Extracted {count} entries from {} into {}",
        archive_path.display(),
        dest_path.display(),
    )))));
    Ok(())
}
