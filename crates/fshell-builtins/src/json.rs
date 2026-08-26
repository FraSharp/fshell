// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use std::sync::Arc;
use ustr::ustr;

pub fn json_builtin(
    in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    _env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    tokio::spawn(async move {
        if let Some(mut rx) = in_rx {
            while let Some(payload) = rx.recv().await {
                if let PipelinePayload::Data(v) = payload {
                    let text = v.to_text();
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    // Debug: print what we're trying to parse
                    fshell_core::debug_log!("json: parsing '{}' (len={})", trimmed, trimmed.len());

                    // Try to parse the entire text as JSON
                    match serde_json::from_str::<serde_json::Value>(trimmed) {
                        Ok(json_val) => {
                            let fshell_val = json_to_val(json_val);
                            if tx
                                .send(PipelinePayload::Data(Arc::new(fshell_val)))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(e) => {
                            // If single object fails, try parsing as multiple JSON objects (one per line)
                            let mut parsed_any = false;
                            for line in trimmed.lines() {
                                let line = line.trim();
                                if line.is_empty() {
                                    continue;
                                }
                                match serde_json::from_str::<serde_json::Value>(line) {
                                    Ok(json_val) => {
                                        let fshell_val = json_to_val(json_val);
                                        if tx
                                            .send(PipelinePayload::Data(Arc::new(fshell_val)))
                                            .await
                                            .is_err()
                                        {
                                            return;
                                        }
                                        parsed_any = true;
                                    }
                                    Err(_) => {
                                        // Skip unparseable lines
                                    }
                                }
                            }
                            if !parsed_any {
                                fshell_core::debug_log!("json parse error: {}", e);
                            }
                        }
                    }
                }
            }
        }
    });
    Ok(())
}

/// Converts a standard `serde_json::Value` into an fshell `Val`.
///
/// NOTE: Direct serde deserialization into `Val` is not possible for arbitrary JSON inputs
/// because the `Val` enum uses a tagged serialization format (`#[serde(tag = "type", content = "value")]`)
/// to preserve gradual typing. As a result, this function performs a tree walk to map generic
/// JSON structures into the type-tagged representation, which incurs small allocation and CPU overhead.
fn json_to_val(json: serde_json::Value) -> Val {
    match json {
        serde_json::Value::Null => Val::Null,
        serde_json::Value::Bool(b) => Val::Bool(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Val::Int(i)
            } else if let Some(f) = n.as_f64() {
                Val::Float(f)
            } else {
                Val::Null
            }
        }
        serde_json::Value::String(s) => Val::String(s),
        serde_json::Value::Array(arr) => Val::List(arr.into_iter().map(json_to_val).collect()),
        serde_json::Value::Object(obj) => {
            let mut map =
                fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            for (k, v) in obj {
                map.insert(ustr(&k), json_to_val(v));
            }
            Val::Map(map)
        }
    }
}
