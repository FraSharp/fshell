// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::ShellError;
use fshell_core::Val;
use fshell_core::diagnostic::ErrorCode;
use fshell_engine::{PipeSender, PipeStream, PipelinePayload};
use std::sync::Arc;

pub fn string_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    _env: &fshell_engine::Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    let mut raw_args: Vec<String> = Vec::new();
    for arg in &args {
        raw_args.push(arg.to_text());
    }

    if raw_args.is_empty() {
        return Err(ShellError::new(
            ErrorCode::InvalidArgument,
            "string: expected a subcommand (split, trim, upper, lower, contains, starts-with, ends-with, substring)",
        ));
    }

    let subcommand = raw_args[0].clone();
    let sub_args_owned = raw_args[1..].to_vec();

    let tx_clone = tx.clone();

    tokio::spawn(async move {
        if let Err(e) = run_string_op(&subcommand, &sub_args_owned, in_rx, &tx_clone).await {
            eprintln!("string error: {}", e);
        }
    });

    Ok(())
}

async fn run_string_op(
    subcommand: &str,
    sub_args: &[String],
    in_rx: Option<PipeStream>,
    tx: &PipeSender,
) -> Result<(), ShellError> {
    match subcommand {
        "split" => {
            let (text, delimiter) = parse_split_args(sub_args)?;
            let lines = process_input_text(in_rx, &text).await;
            for line in &lines {
                let parts: Vec<Val> = line
                    .split(&delimiter)
                    .map(|s| Val::String(s.to_string()))
                    .collect();
                let result = Val::List(parts);
                if tx.send(PipelinePayload::Data(Arc::new(result))).await.is_err() {
                    return Ok(());
                }
            }
            Ok(())
        }
        "trim" => {
            let lines = process_input_text(in_rx, &sub_args.first().cloned().unwrap_or_default()).await;
            for line in &lines {
                let trimmed = line.trim();
                if tx.send(PipelinePayload::Data(Arc::new(Val::String(trimmed.to_string())))).await.is_err() {
                    return Ok(());
                }
            }
            Ok(())
        }
        "upper" => {
            let lines = process_input_text(in_rx, &sub_args.first().cloned().unwrap_or_default()).await;
            for line in &lines {
                let upper = line.to_uppercase();
                if tx.send(PipelinePayload::Data(Arc::new(Val::String(upper)))).await.is_err() {
                    return Ok(());
                }
            }
            Ok(())
        }
        "lower" => {
            let lines = process_input_text(in_rx, &sub_args.first().cloned().unwrap_or_default()).await;
            for line in &lines {
                let lower = line.to_lowercase();
                if tx.send(PipelinePayload::Data(Arc::new(Val::String(lower)))).await.is_err() {
                    return Ok(());
                }
            }
            Ok(())
        }
        "contains" => {
            let (text, needle) = parse_two_args(sub_args, "contains")?;
            let lines = process_input_text(in_rx, &text).await;
            for line in &lines {
                let result = line.contains(&needle);
                if tx.send(PipelinePayload::Data(Arc::new(Val::Bool(result)))).await.is_err() {
                    return Ok(());
                }
            }
            Ok(())
        }
        "starts-with" => {
            let (text, prefix) = parse_two_args(sub_args, "starts-with")?;
            let lines = process_input_text(in_rx, &text).await;
            for line in &lines {
                let result = line.starts_with(&prefix);
                if tx.send(PipelinePayload::Data(Arc::new(Val::Bool(result)))).await.is_err() {
                    return Ok(());
                }
            }
            Ok(())
        }
        "ends-with" => {
            let (text, suffix) = parse_two_args(sub_args, "ends-with")?;
            let lines = process_input_text(in_rx, &text).await;
            for line in &lines {
                let result = line.ends_with(&suffix);
                if tx.send(PipelinePayload::Data(Arc::new(Val::Bool(result)))).await.is_err() {
                    return Ok(());
                }
            }
            Ok(())
        }
        "substring" => {
            let (text, start, length) = parse_substring_args(sub_args)?;
            let lines = process_input_text(in_rx, &text).await;
            for line in &lines {
                let chars: Vec<char> = line.chars().collect();
                let start_idx = start.max(0) as usize;
                let end_idx = if let Some(len) = length {
                    (start_idx + len as usize).min(chars.len())
                } else {
                    chars.len()
                };
                if start_idx > chars.len() {
                    if tx.send(PipelinePayload::Data(Arc::new(Val::String(String::new())))).await.is_err() {
                        return Ok(());
                    }
                } else {
                    let substr: String = chars[start_idx..end_idx].iter().collect();
                    if tx.send(PipelinePayload::Data(Arc::new(Val::String(substr)))).await.is_err() {
                        return Ok(());
                    }
                }
            }
            Ok(())
        }
        _ => Err(format!(
            "string: unknown subcommand '{}'. Expected: split, trim, upper, lower, contains, starts-with, ends-with, substring",
            subcommand
        ).into()),
    }
}

async fn process_input_text(in_rx: Option<PipeStream>, fallback: &str) -> Vec<String> {
    if let Some(mut rx) = in_rx {
        let mut texts = Vec::new();
        while let Some(payload) = rx.recv().await {
            match payload {
                PipelinePayload::Data(data) => {
                    texts.push(data.to_text());
                }
                PipelinePayload::Bytes(_) => {
                    // Bytes payloads are dropped by this stage.
                    continue;
                }
                PipelinePayload::Structured(d) => {
                    eprintln!("string: warning: {}", d.report);
                }
            }
        }
        if texts.is_empty() {
            texts.push(fallback.to_string());
        }
        texts
    } else if !fallback.is_empty() {
        vec![fallback.to_string()]
    } else {
        vec![]
    }
}

fn parse_split_args(args: &[String]) -> Result<(String, String), ShellError> {
    match args.len() {
        0 => Err("string split: expected <delimiter> or <text> <delimiter>"
            .to_string()
            .into()),
        // 1 arg: delimiter only (text comes from pipe)
        1 => Ok((String::new(), args[0].clone())),
        // 2+ args: text and delimiter
        _ => Ok((args[0].clone(), args[1].clone())),
    }
}

fn parse_two_args(args: &[String], subcmd: &str) -> Result<(String, String), ShellError> {
    match args.len() {
        0 => Err(format!("string {}: expected <needle> or <text> <needle>", subcmd).into()),
        // 1 arg: needle only (text comes from pipe)
        1 => Ok((String::new(), args[0].clone())),
        // 2+ args: text and needle
        _ => Ok((args[0].clone(), args[1].clone())),
    }
}

fn parse_substring_args(args: &[String]) -> Result<(String, i64, Option<i64>), ShellError> {
    match args.len() {
        0 => Err(
            "string substring: expected <start> [length] or <text> <start> [length]"
                .to_string()
                .into(),
        ),
        // 1 arg: start only (text comes from pipe)
        1 => {
            let start = args[0]
                .parse::<i64>()
                .map_err(|e| format!("string substring: invalid start: {}", e))?;
            Ok((String::new(), start, None))
        }
        // 2 args: could be (text, start) or (start, length) depending on context
        2 => {
            // Try to parse first as start index; if it fails, assume it's text
            if let Ok(start) = args[0].parse::<i64>() {
                let length = args[1]
                    .parse::<i64>()
                    .map_err(|e| format!("string substring: invalid length: {}", e))?;
                Ok((String::new(), start, Some(length)))
            } else {
                // (text, start) - non-piped mode
                let start = args[1]
                    .parse::<i64>()
                    .map_err(|e| format!("string substring: invalid start: {}", e))?;
                Ok((args[0].clone(), start, None))
            }
        }
        // 3+ args: text start [length]
        _ => {
            let start = args[1]
                .parse::<i64>()
                .map_err(|e| format!("string substring: invalid start: {}", e))?;
            let length = args[2]
                .parse::<i64>()
                .map_err(|e| format!("string substring: invalid length: {}", e))?;
            Ok((args[0].clone(), start, Some(length)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_args() {
        let args = vec!["hello world".to_string(), " ".to_string()];
        let (text, delimiter) = parse_split_args(&args).unwrap();
        assert_eq!(text, "hello world");
        assert_eq!(delimiter, " ");
    }

    #[test]
    fn test_split_args_too_few() {
        let args = vec![];
        assert!(parse_split_args(&args).is_err());
    }

    #[test]
    fn test_two_args() {
        let args = vec!["hello world".to_string(), "world".to_string()];
        let (text, needle) = parse_two_args(&args, "contains").unwrap();
        assert_eq!(text, "hello world");
        assert_eq!(needle, "world");
    }

    #[test]
    fn test_two_args_too_few() {
        let args = vec![];
        assert!(parse_two_args(&args, "contains").is_err());
    }

    #[test]
    fn test_substring_args() {
        let args = vec!["hello".to_string(), "1".to_string(), "3".to_string()];
        let (text, start, length) = parse_substring_args(&args).unwrap();
        assert_eq!(text, "hello");
        assert_eq!(start, 1);
        assert_eq!(length, Some(3));
    }

    #[test]
    fn test_substring_args_no_length() {
        let args = vec!["hello".to_string(), "2".to_string()];
        let (text, start, length) = parse_substring_args(&args).unwrap();
        assert_eq!(text, "hello");
        assert_eq!(start, 2);
        assert_eq!(length, None);
    }

    #[test]
    fn test_substring_args_invalid() {
        let args = vec!["hello".to_string(), "abc".to_string()];
        assert!(parse_substring_args(&args).is_err());
    }
}
