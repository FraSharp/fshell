// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::{CapAction, Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

pub fn hash_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let mut algo = "256".to_string();
    let mut xof_len = 32;
    let mut files = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match &args[i] {
            Val::String(s) => {
                if s == "-h" || s == "--help" {
                    return Err("Usage: hash [-a 256|512|xof] [-o N] [FILE...]\nOptions:\n  -a <algo>  Algorithm: 256, 512, xof (default: 256)\n  -o <len>   XOF output bytes (default: 32)\n  -h, --help Show help".to_string().into());
                } else if s == "-a" {
                    if i + 1 < args.len() {
                        match &args[i + 1] {
                            Val::String(a) => {
                                algo = a.clone();
                                i += 2;
                            }
                            Val::Int(n) => {
                                algo = n.to_string();
                                i += 2;
                            }
                            _ => {
                                return Err("hash: -a option requires algorithm (256, 512, xof)".into());
                            }
                        }
                    } else {
                        return Err("hash: -a option requires an argument".into());
                    }
                } else if s == "-o" {
                    if i + 1 < args.len() {
                        match &args[i + 1] {
                            Val::Int(n) => {
                                xof_len = *n as usize;
                                i += 2;
                            }
                            Val::String(n_str) => {
                                if let Ok(n) = n_str.parse::<usize>() {
                                    xof_len = n;
                                    i += 2;
                                } else {
                                    return Err(BuiltinError::InvalidArgument {
                                        cmd: "hash".into(),
                                        arg: format!("invalid -o value: {}", n_str),
                                        span,
                                    }
                                    .into());
                                }
                            }
                            other => {
                                return Err(BuiltinError::InvalidArgument {
                                    cmd: "hash".into(),
                                    arg: format!("invalid -o value: {:?}", other),
                                    span,
                                }
                                .into());
                            }
                        }
                    } else {
                        return Err("hash: -o option requires an argument".into());
                    }
                } else if s.starts_with('-') && s != "-" {
                    return Err(BuiltinError::InvalidArgument {
                        cmd: "hash".into(),
                        arg: format!("unknown option '{}'", s),
                        span,
                    }
                    .into());
                } else {
                    files.push(s.clone());
                    i += 1;
                }
            }
            other => {
                let file_str = match other {
                    Val::String(s) => s.clone(),
                    Val::Int(i) => i.to_string(),
                    Val::Float(f) => f.to_string(),
                    other_val => format!("{:?}", other_val),
                };
                files.push(file_str);
                i += 1;
            }
        }
    }

    if algo != "256" && algo != "512" && algo != "xof" {
        return Err(BuiltinError::InvalidArgument {
            cmd: "hash".into(),
            arg: format!("unknown algorithm '{}'", algo),
            span,
        }
        .into());
    }

    let env_clone = env.clone();
    tokio::spawn(async move {
        if files.is_empty() {
            if let Some(mut rx) = in_rx {
                let (mut hasher, output_len) = match make_hasher(&algo, xof_len) {
                    Ok(res) => res,
                    Err(e) => {
                        let _ = tx.send(PipelinePayload::Structured(e.into())).await;
                        return;
                    }
                };
                while let Some(payload) = rx.recv().await {
                    match payload {
                        PipelinePayload::Data(val_arc) => match val_arc.as_ref() {
                            Val::String(s) => {
                                hasher.update(s.as_bytes());
                            }
                            other => {
                                if let Ok(bytes) = serde_json::to_vec(other) {
                                    hasher.update(&bytes);
                                }
                            }
                        },
                        PipelinePayload::Bytes(_) => {
                            // Bytes payloads are dropped by this stage.
                            continue;
                        }
                        PipelinePayload::Structured(_) => {}
                    }
                }
                let digest = hasher.finalize(output_len);
                let mut hash_hex = String::with_capacity(digest.len() * 2);
                for b in digest {
                    hash_hex.push_str(&format!("{:02x}", b));
                }
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(hash_hex))))
                    .await;
            }
        } else {
            for file in files {
                let path = PathBuf::from(&file);
                if env_clone
                    .enforce_capability("hash", CapAction::ReadFile(path.clone()))
                    .is_err()
                {
                    let _ = tx
                        .send(PipelinePayload::Data(Arc::new(Val::String(format!(
                            "{}: permission denied",
                            file
                        )))))
                        .await;
                    continue;
                }
                match std::fs::File::open(&path) {
                    Ok(mut f) => {
                        let (mut hasher, output_len) = match make_hasher(&algo, xof_len) {
                            Ok(res) => res,
                            Err(e) => {
                                let _ = tx.send(PipelinePayload::Structured(e.into())).await;
                                return;
                            }
                        };
                        let mut buf = [0u8; 4096];
                        let mut read_err = false;
                        loop {
                            match f.read(&mut buf) {
                                Ok(0) => break,
                                Ok(n) => hasher.update(&buf[..n]),
                                Err(e) => {
                                    let _ = tx
                                        .send(PipelinePayload::Data(Arc::new(Val::String(
                                            format!("{}: read error: {}", file, e),
                                        ))))
                                        .await;
                                    read_err = true;
                                    break;
                                }
                            }
                        }
                        if !read_err {
                            let digest = hasher.finalize(output_len);
                            let mut hash_hex = String::with_capacity(digest.len() * 2);
                            for b in digest {
                                hash_hex.push_str(&format!("{:02x}", b));
                            }
                            let out_line = format!("{}  {}", hash_hex, file);
                            if tx
                                .send(PipelinePayload::Data(Arc::new(Val::String(out_line))))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(PipelinePayload::Data(Arc::new(Val::String(format!(
                                "{}: failed to open: {}",
                                file, e
                            )))))
                            .await;
                    }
                }
            }
        }
    });

    Ok(())
}

fn make_hasher(algo: &str, xof_len: usize) -> Result<(fshell_hash::Hasher, usize), BuiltinError> {
    match algo {
        "256" => Ok((fshell_hash::Hasher::new(0x00, 16), 32)),
        "512" => Ok((fshell_hash::Hasher::new(0x04, 16), 64)),
        "xof" => Ok((fshell_hash::Hasher::new(0x02, 16), xof_len)),
        _ => Err(BuiltinError::InvalidArgument {
            cmd: "hash".into(),
            arg: format!("unknown algorithm '{}'", algo),
            span: None,
        }),
    }
}
