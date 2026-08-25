// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::Val;
use fshell_core::diagnostic::StringError;
use fshell_engine::{CapAction, Env, PipeSender, PipeStream, PipelinePayload};
use std::sync::Arc;
use ustr::ustr;

pub fn files_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    let path_str = match args.first() {
        Some(Val::String(s)) => s.clone(),
        _ => ".".to_string(),
    };

    let path = crate::utils::expand_tilde(&path_str);
    let path = std::fs::canonicalize(&path).unwrap_or(path);

    env.enforce_capability("files", CapAction::ReadDir(path.clone()))?;
    env.track_read(path.clone());

    let env_clone = env.clone();
    tokio::spawn(async move {
        let _ = scan_dir(path, &tx, &env_clone).await;
    });

    Ok(())
}

async fn scan_dir(
    start_path: std::path::PathBuf,
    tx: &PipeSender,
    env: &Env,
) -> Result<(), StringError> {
    let mut stack = vec![(start_path, 0)];

    while let Some((path, depth)) = stack.pop() {
        if depth > 20 {
            continue;
        }

        // Enforce ReadDir capability for the directory being scanned
        if env
            .enforce_capability("files", CapAction::ReadDir(path.clone()))
            .is_err()
        {
            continue;
        }

        let mut entries = match tokio::fs::read_dir(&path).await {
            Ok(e) => e,
            Err(_) => continue,
        };

        let mut subdirs = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let metadata = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };

            let entry_path = entry.path();
            let name = entry_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            // Skip hidden files and directories
            if name.starts_with('.') {
                continue;
            }

            let extension = entry_path
                .extension()
                .map(|e| e.to_string_lossy().to_string())
                .unwrap_or_default();

            let file_type = if metadata.is_dir() {
                "dir"
            } else if metadata.is_file() {
                "file"
            } else {
                "other"
            };

            let mut map =
                fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            map.insert(ustr("name"), Val::String(name.clone()));
            map.insert(
                ustr("path"),
                Val::String(entry_path.to_string_lossy().to_string()),
            );
            map.insert(ustr("extension"), Val::String(extension));
            map.insert(ustr("type"), Val::String(file_type.to_string()));
            map.insert(ustr("size"), Val::Int(metadata.len() as i64));

            let modified = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| Val::Int(d.as_secs() as i64))
                .unwrap_or(Val::Null);
            map.insert(ustr("last_modified"), modified);

            if tx
                .send(PipelinePayload::Data(Arc::new(Val::Map(map))))
                .await
                .is_err()
            {
                return Ok(());
            }

            if metadata.is_dir() {
                subdirs.push(entry_path);
            }
        }

        // Push subdirectories onto stack in reverse order to maintain DFS traversal ordering
        for sd in subdirs.into_iter().rev() {
            stack.push((sd, depth + 1));
        }
    }

    Ok(())
}
