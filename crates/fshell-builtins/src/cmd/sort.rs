// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub fn sort_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    let mut reverse = false;
    let mut key = None;
    let mut direct_list = None;
    let mut end_of_opts = false;

    let mut idx = 0;
    while idx < args.len() {
        match &args[idx] {
            Val::String(s) if !end_of_opts && (s == "-r" || s == "--reverse") => {
                reverse = true;
            }
            Val::String(s) if !end_of_opts && (s == "-k" || s == "--key") => {
                if idx + 1 < args.len() {
                    if let Val::String(k) = &args[idx + 1] {
                        key = Some(k.clone());
                        idx += 1;
                    } else {
                        return Err(BuiltinError::InvalidArgument {
                            cmd: "sort".into(),
                            arg: format!(
                                "value for -k/--key must be a string, got: {:?}",
                                args[idx + 1]
                            ),
                            span: None,
                        }
                        .into());
                    }
                } else {
                    return Err(BuiltinError::MissingArgument {
                        cmd: "sort".into(),
                        description: "key field name after -k/--key".into(),
                        span: None,
                    }
                    .into());
                }
            }
            Val::String(s) if !end_of_opts && s.starts_with("--key=") => {
                let k = s["--key=".len()..].to_string();
                if k.is_empty() {
                    return Err(BuiltinError::InvalidArgument {
                        cmd: "sort".into(),
                        arg: s.clone(),
                        span: None,
                    }
                    .into());
                }
                key = Some(k);
            }
            Val::String(s) if !end_of_opts && s == "--" => {
                end_of_opts = true;
            }
            Val::String(s) if !end_of_opts && s.starts_with('-') => {
                return Err(BuiltinError::InvalidArgument {
                    cmd: "sort".into(),
                    arg: s.clone(),
                    span: None,
                }
                .into());
            }
            Val::List(l) => {
                if direct_list.is_some() {
                    return Err(BuiltinError::InvalidArgument {
                        cmd: "sort".into(),
                        arg: "multiple list arguments".into(),
                        span: None,
                    }
                    .into());
                }
                direct_list = Some(l.clone());
            }
            other => {
                return Err(BuiltinError::InvalidArgument {
                    cmd: "sort".into(),
                    arg: format!("{:?}", other),
                    span: None,
                }
                .into());
            }
        }
        idx += 1;
    }

    let sort_max_items = env.options.read().sort_max_items;

    let env_clone = env.clone();
    tokio::spawn(async move {
        let mut items = Vec::new();

        if let Some(mut rx) = in_rx {
            while let Some(payload) = rx.recv().await {
                if env_clone.job_control.cancellation.load(Ordering::Relaxed) {
                    break;
                }
                match payload {
                    PipelinePayload::Data(val_arc) => {
                        if items.len() >= sort_max_items {
                            env_clone.report_stage_error();
                            let _ = tx
                                .send(PipelinePayload::Structured(
                                    format!("sort: too many items (limit {})", sort_max_items)
                                        .into(),
                                ))
                                .await;
                            // Drain remaining input to avoid back-pressure deadlock
                            while rx.recv().await.is_some() {}
                            return;
                        }
                        items.push(val_arc);
                    }
                    PipelinePayload::Bytes(_) => {
                        // Bytes payloads are dropped by this stage.
                        continue;
                    }
                    PipelinePayload::Structured(d) => {
                        if tx.send(PipelinePayload::Structured(d)).await.is_err() {
                            return;
                        }
                    }
                }
            }
        } else if let Some(list) = direct_list {
            if list.len() > sort_max_items {
                env_clone.report_stage_error();
                let _ = tx
                    .send(PipelinePayload::Structured(
                        format!("sort: too many items (limit {})", sort_max_items).into(),
                    ))
                    .await;
                return;
            }
            items = list.into_iter().map(Arc::new).collect();
        }

        // Perform the sort
        items.sort_by(|a, b| {
            let cmp = if let Some(ref col) = key {
                let val_a = match &**a {
                    Val::Map(map) => map.get(&ustr::ustr(col)).unwrap_or(&Val::Null),
                    _ => &Val::Null,
                };
                let val_b = match &**b {
                    Val::Map(map) => map.get(&ustr::ustr(col)).unwrap_or(&Val::Null),
                    _ => &Val::Null,
                };
                fshell_engine::cmp_vals(val_a, val_b)
            } else {
                fshell_engine::cmp_vals(a, b)
            };
            if reverse { cmp.reverse() } else { cmp }
        });

        // Send sorted items downstream
        for item in items {
            if env_clone.job_control.cancellation.load(Ordering::Relaxed) {
                break;
            }
            if tx.send(PipelinePayload::Data(item)).await.is_err() {
                break;
            }
        }
    });

    Ok(())
}
