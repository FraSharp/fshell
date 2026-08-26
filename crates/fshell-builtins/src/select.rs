// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use std::io::{self, BufRead, Write};
use std::sync::Arc;

pub fn select_builtin(
    in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    _env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    tokio::spawn(async move {
        let mut items = Vec::new();

        if let Some(mut rx) = in_rx {
            while let Some(payload) = rx.recv().await {
                if let PipelinePayload::Data(v) = payload {
                    items.push((*v).clone());
                }
            }
        }

        if items.is_empty() {
            return;
        }

        // Simple numbered selection using spawn_blocking to avoid stalling the async executor
        let items_clone = items.clone();
        let selected = tokio::task::spawn_blocking(move || interactive_select(&items_clone))
            .await
            .unwrap_or(None);

        if let Some(item) = selected {
            let _ = tx.send(PipelinePayload::Data(Arc::new(item))).await;
        }
    });
    Ok(())
}

fn interactive_select(items: &[Val]) -> Option<Val> {
    // Print items with numbers
    println!("Select an item:");
    for (i, item) in items.iter().enumerate() {
        let text = item.to_text();
        let display = if text.chars().count() > 60 {
            let truncated: String = text.chars().take(57).collect();
            format!("{truncated}...")
        } else {
            text
        };
        println!("  {}) {}", i + 1, display);
    }
    print!("\nEnter number (1-{}), or 0 to cancel: ", items.len());
    let _ = io::stdout().flush();

    // Read user input
    let stdin = io::stdin();
    let mut input = String::new();
    if stdin.lock().read_line(&mut input).is_err() {
        return None;
    }

    let input = input.trim();
    if input == "0" || input.is_empty() {
        return None;
    }

    match input.parse::<usize>() {
        Ok(n) if n >= 1 && n <= items.len() => Some(items[n - 1].clone()),
        _ => {
            println!("Invalid selection");
            None
        }
    }
}
