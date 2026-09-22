// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::{CapAction, Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;
use std::io::Read;
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
                                return Err(
                                    "hash: -a option requires algorithm (256, 512, xof)".into()
                                );
                            }
                        }
                    } else {
                        return Err("hash: -a option requires an argument".into());
                    }
                } else if s == "-o" {
                    if i + 1 < args.len() {
                        match &args[i + 1] {
                            Val::Int(n) => {
                                let Ok(parsed_len) = usize::try_from(*n) else {
                                    return Err(BuiltinError::InvalidArgument {
                                        cmd: "hash".into(),
                                        arg: format!("invalid -o value: {}", n),
                                        span,
                                    }
                                    .into());
                                };
                                xof_len = validate_xof_len(parsed_len, span)?;
                                i += 2;
                            }
                            Val::String(n_str) => {
                                if let Ok(n) = n_str.parse::<usize>() {
                                    xof_len = validate_xof_len(n, span)?;
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
                            Val::Blob(bytes) => {
                                hasher.update(bytes);
                            }
                            other => {
                                if let Ok(bytes) = serde_json::to_vec(other) {
                                    hasher.update(&bytes);
                                }
                            }
                        },
                        PipelinePayload::Bytes(bytes) => {
                            hasher.update(&bytes);
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
                let path = env_clone.resolve_path(&file);
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
        "xof" => Ok((
            fshell_hash::Hasher::new(0x02, 16),
            validate_xof_len(xof_len, None)?,
        )),
        _ => Err(BuiltinError::InvalidArgument {
            cmd: "hash".into(),
            arg: format!("unknown algorithm '{}'", algo),
            span: None,
        }),
    }
}

fn validate_xof_len(len: usize, span: Option<SourceSpan>) -> Result<usize, BuiltinError> {
    if len > fshell_core::MAX_HASH_XOF_OUTPUT_BYTES {
        return Err(BuiltinError::InvalidArgument {
            cmd: "hash".into(),
            arg: format!(
                "XOF output length exceeds the {} byte shell limit",
                fshell_core::MAX_HASH_XOF_OUTPUT_BYTES
            ),
            span,
        });
    }
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stream_hash_includes_all_raw_byte_payloads() {
        let env = Env::new();
        let (input_tx, input_rx) = tokio::sync::mpsc::channel(2);
        let (output_tx, mut output_rx) = tokio::sync::mpsc::channel(2);

        hash_builtin(Some(input_rx), Vec::new(), &env, output_tx, None).unwrap();
        input_tx
            .send(PipelinePayload::Bytes(b"raw".to_vec().into()))
            .await
            .unwrap();
        input_tx
            .send(PipelinePayload::Bytes(b"\0bytes".to_vec().into()))
            .await
            .unwrap();
        drop(input_tx);

        let expected = fshell_hash::fhash256(b"raw\0bytes")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        match output_rx.recv().await.unwrap() {
            PipelinePayload::Data(value) => {
                assert_eq!(value.as_ref(), &Val::String(expected));
            }
            other => panic!("expected hash output, got {other:?}"),
        }
    }

    #[test]
    fn xof_shell_output_limit_is_enforced() {
        assert!(make_hasher("xof", fshell_core::MAX_HASH_XOF_OUTPUT_BYTES + 1).is_err());
    }

    #[test]
    fn hash_command_rejects_negative_xof_output() {
        let env = Env::new();
        let (output_tx, _output_rx) = tokio::sync::mpsc::channel(1);
        let args = vec![
            Val::String("-a".into()),
            Val::String("xof".into()),
            Val::String("-o".into()),
            Val::Int(-1),
        ];
        assert!(hash_builtin(None, args, &env, output_tx, None).is_err());
    }
}
