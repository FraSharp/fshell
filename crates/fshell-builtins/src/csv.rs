// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::Val;
use fshell_core::diagnostic::StringError;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use std::sync::Arc;
use ustr::ustr;

pub fn csv_builtin(
    in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    _env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    tokio::spawn(async move {
        if let Some(mut rx) = in_rx {
            while let Some(payload) = rx.recv().await {
                if let PipelinePayload::Data(v) = payload {
                    let text = v.to_text();
                    let lines: Vec<&str> = text.lines().collect();

                    if lines.is_empty() {
                        continue;
                    }

                    // First line is headers
                    let headers: Vec<&str> = lines[0].split(',').map(|h| h.trim()).collect();

                    // Remaining lines are data
                    for line in &lines[1..] {
                        if line.trim().is_empty() {
                            continue;
                        }

                        let values: Vec<&str> = line.split(',').map(|v| v.trim()).collect();

                        let mut map = fshell_core::FxIndexMap::with_hasher(
                            fshell_hash::FxBuildHasher::default(),
                        );

                        for (i, header) in headers.iter().enumerate() {
                            let value = values.get(i).unwrap_or(&"");
                            let parsed_value = parse_csv_value(value);
                            map.insert(ustr(header), parsed_value);
                        }

                        if tx
                            .send(PipelinePayload::Data(Arc::new(Val::Map(map))))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
        }
    });
    Ok(())
}

fn parse_csv_value(value: &str) -> Val {
    // Try to parse as number
    if let Ok(i) = value.parse::<i64>() {
        return Val::Int(i);
    }
    if let Ok(f) = value.parse::<f64>() {
        return Val::Float(f);
    }

    // Try to parse as boolean
    match value.to_lowercase().as_str() {
        "true" => return Val::Bool(true),
        "false" => return Val::Bool(false),
        _ => {}
    }

    // Try to parse as null
    if value.to_lowercase() == "null" || value.is_empty() {
        return Val::Null;
    }

    // Default to string
    Val::String(value.to_string())
}
