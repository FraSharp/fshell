// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::CapAction;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;
use std::sync::Arc;
use ustr::ustr;

enum DiffError {
    Failure(ShellError),
    DownstreamClosed,
}

impl From<ShellError> for DiffError {
    fn from(error: ShellError) -> Self {
        Self::Failure(error)
    }
}

pub fn diff_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    if args.len() < 2 {
        return Err("diff requires two file paths".to_string().into());
    }

    let file1 = match &args[0] {
        Val::String(s) => s.clone(),
        _ => {
            return Err("diff: first argument must be a file path"
                .to_string()
                .into());
        }
    };

    let file2 = match &args[1] {
        Val::String(s) => s.clone(),
        _ => {
            return Err("diff: second argument must be a file path"
                .to_string()
                .into());
        }
    };

    let file1 = env.resolve_path(file1);
    let file2 = env.resolve_path(file2);
    env.enforce_capability("diff", CapAction::ReadFile(file1.clone()))?;
    env.enforce_capability("diff", CapAction::ReadFile(file2.clone()))?;
    let env = env.clone();
    tokio::spawn(async move {
        match run_diff(&file1, &file2, &tx).await {
            Err(DiffError::Failure(error)) => {
                env.report_stage_error();
                let _ = tx
                    .send(PipelinePayload::Structured(error.to_string().into()))
                    .await;
            }
            Err(DiffError::DownstreamClosed) | Ok(()) => {}
        }
    });

    Ok(())
}

async fn run_diff(
    file1: &std::path::Path,
    file2: &std::path::Path,
    tx: &PipeSender,
) -> Result<(), DiffError> {
    let content1 = tokio::fs::read_to_string(file1)
        .await
        .map_err(|e| ShellError::from(format!("Failed to read {}: {}", file1.display(), e)))?;
    let content2 = tokio::fs::read_to_string(file2)
        .await
        .map_err(|e| ShellError::from(format!("Failed to read {}: {}", file2.display(), e)))?;

    let lines1: Vec<&str> = content1.lines().collect();
    let lines2: Vec<&str> = content2.lines().collect();

    // Simple line-by-line diff
    let max_len = lines1.len().max(lines2.len());

    for (line_num, i) in (1..).zip(0..max_len) {
        let line1 = lines1.get(i);
        let line2 = lines2.get(i);

        match (line1, line2) {
            (Some(l1), Some(l2)) if l1 == l2 => {
                // Lines are the same, skip
            }
            (Some(l1), Some(l2)) => {
                // Lines differ
                send_diff_entry(tx, "removed", l1, line_num).await?;
                send_diff_entry(tx, "added", l2, line_num).await?;
            }
            (Some(l1), None) => {
                send_diff_entry(tx, "removed", l1, line_num).await?;
            }
            (None, Some(l2)) => {
                send_diff_entry(tx, "added", l2, line_num).await?;
            }
            (None, None) => {}
        }
    }

    Ok(())
}

async fn send_diff_entry(
    tx: &PipeSender,
    diff_type: &str,
    content: &str,
    line: usize,
) -> Result<(), DiffError> {
    let mut map = fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map.insert(ustr("type"), Val::String(diff_type.to_string()));
    map.insert(ustr("content"), Val::String(content.to_string()));
    map.insert(ustr("line_number"), Val::Int(line as i64));

    tx.send(PipelinePayload::Data(Arc::new(Val::Map(map))))
        .await
        .map_err(|_| DiffError::DownstreamClosed)
}
