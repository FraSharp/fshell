// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::{CapAction, Env, PipeSender, PipeStream, PipelinePayload};
use std::sync::Arc;
use ustr::ustr;

pub fn replace_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    let (old_text, new_text, globs, dry_run) = parse_args(&args)?;

    let tx = tx.clone();
    let env = env.clone();

    tokio::spawn(async move {
        if let Err(e) = run_replace(in_rx, &old_text, &new_text, &globs, dry_run, &env, tx).await {
            eprintln!("replace error: {}", e);
        }
    });

    Ok(())
}

fn parse_args(args: &[Val]) -> Result<(String, String, Vec<String>, bool), String> {
    let mut raw: Vec<String> = Vec::new();
    for arg in args {
        raw.push(match arg {
            Val::String(s) => s.clone(),
            Val::Int(i) => i.to_string(),
            Val::Float(f) => f.to_string(),
            other => other.to_text(),
        });
    }

    if raw.len() < 2 {
        return Err(BuiltinError::MissingArgument {
            cmd: "replace".into(),
            description: "old and new text: <old> <new> [in <glob>...]".into(),
            span: None,
        }
        .to_string());
    }

    let old_text = raw[0].clone();
    let new_text = raw[1].clone();
    let mut globs = Vec::new();
    let mut dry_run = false;
    let mut i = 2;

    while i < raw.len() {
        match raw[i].as_str() {
            "in" => {
                i += 1;
                while i < raw.len() && !raw[i].starts_with("--") {
                    globs.push(raw[i].clone());
                    i += 1;
                }
            }
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            other => {
                return Err(BuiltinError::UnexpectedArgument {
                    cmd: "replace".into(),
                    arg: other.to_string(),
                    span: None,
                }
                .to_string());
            }
        }
    }

    Ok((old_text, new_text, globs, dry_run))
}

async fn run_replace(
    in_rx: Option<PipeStream>,
    old_text: &str,
    new_text: &str,
    globs: &[String],
    dry_run: bool,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    if !globs.is_empty() {
        // Expand globs and process files directly
        for glob in globs {
            process_glob(glob, old_text, new_text, dry_run, env, &tx).await?;
        }
    } else if let Some(mut rx) = in_rx {
        // Read paths from pipeline
        while let Some(payload) = rx.recv().await {
            let path = match payload {
                PipelinePayload::Data(data) => data.to_text().trim().to_string(),
                PipelinePayload::Bytes(_) => {
                    // Bytes payloads are dropped by this stage.
                    continue;
                }
                PipelinePayload::Structured(d) => {
                    eprintln!("replace: warning: {}", d.report);
                    continue;
                }
            };
            if path.is_empty() {
                continue;
            }
            process_file(&path, old_text, new_text, dry_run, env, &tx).await?;
        }
    } else {
        return Err(BuiltinError::MissingArgument {
            cmd: "replace".into(),
            description: "files to process: use 'in <glob>' or pipe file paths".into(),
            span: None,
        }
        .into());
    }

    Ok(())
}

async fn process_glob(
    pattern: &str,
    old_text: &str,
    new_text: &str,
    dry_run: bool,
    env: &Env,
    tx: &PipeSender,
) -> Result<(), ShellError> {
    // Simple glob expansion by scanning directories
    let (base_dir, glob_part) = if let Some(idx) = pattern.rfind(std::path::MAIN_SEPARATOR) {
        (pattern[..idx].to_string(), pattern[idx + 1..].to_string())
    } else {
        (".".to_string(), pattern.to_string())
    };

    let base_path = std::path::PathBuf::from(&base_dir);
    if !base_path.exists() {
        return Err(BuiltinError::FileNotFound {
            cmd: "replace".into(),
            path: base_dir.to_string(),
            span: None,
        }
        .into());
    }

    // Check read capability on the search directory
    env.enforce_capability("replace", CapAction::ReadDir(base_path.clone()))?;
    env.track_read(base_path);

    // Walk the directory tree and match the glob
    for entry in walkdir::WalkDir::new(&base_dir)
        .max_depth(32)
        .follow_links(false)
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        if !crate::ff::glob_match(&glob_part, &name) {
            continue;
        }

        let full_path = entry.path().to_string_lossy().to_string();
        process_file(&full_path, old_text, new_text, dry_run, env, tx).await?;
    }

    Ok(())
}

async fn process_file(
    path: &str,
    old_text: &str,
    new_text: &str,
    dry_run: bool,
    env: &Env,
    tx: &PipeSender,
) -> Result<(), ShellError> {
    let file_path = std::path::PathBuf::from(path);

    // Capability checks
    env.enforce_capability("replace", CapAction::ReadFile(file_path.clone()))?;
    env.track_read(file_path.clone());

    // Read the file
    let content = std::fs::read_to_string(&file_path)
        .map_err(|e| format!("replace: failed to read {:?}: {}", path, e))?;

    let count = content.matches(old_text).count();
    if count == 0 {
        return Ok(()); // no matches, skip
    }

    let new_content = content.replace(old_text, new_text);

    // Emit result
    let mut map = fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map.insert(ustr("path"), Val::String(path.to_string()));
    map.insert(ustr("replacements"), Val::Int(count as i64));
    if tx
        .send(PipelinePayload::Data(Arc::new(Val::Map(map))))
        .await
        .is_err()
    {
        return Ok(());
    }

    // Write back (unless dry run)
    if !dry_run {
        env.enforce_capability("replace", CapAction::WriteFile(file_path.clone()))?;
        std::fs::write(file_path, &new_content)
            .map_err(|e| format!("replace: failed to write {:?}: {}", path, e))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args_basic() {
        let args = vec![
            Val::String("old".to_string()),
            Val::String("new".to_string()),
            Val::String("in".to_string()),
            Val::String("*.rs".to_string()),
        ];
        let (old, new, globs, dry) = parse_args(&args).unwrap();
        assert_eq!(old, "old");
        assert_eq!(new, "new");
        assert_eq!(globs, vec!["*.rs"]);
        assert!(!dry);
    }

    #[test]
    fn test_parse_args_dry_run() {
        let args = vec![
            Val::String("foo".to_string()),
            Val::String("bar".to_string()),
            Val::String("in".to_string()),
            Val::String("*.txt".to_string()),
            Val::String("--dry-run".to_string()),
        ];
        let (old, new, globs, dry) = parse_args(&args).unwrap();
        assert_eq!(old, "foo");
        assert_eq!(new, "bar");
        assert_eq!(globs, vec!["*.txt"]);
        assert!(dry);
    }

    #[test]
    fn test_parse_args_too_few() {
        let args = vec![Val::String("only".to_string())];
        assert!(parse_args(&args).is_err());
    }

    #[test]
    fn test_glob_match() {
        assert!(crate::ff::glob_match("*.rs", "main.rs"));
        assert!(!crate::ff::glob_match("*.rs", "main.c"));
        assert!(crate::ff::glob_match("test*", "test_ff.rs"));
        assert!(crate::ff::glob_match("*.txt", "file.txt"));
    }
}
