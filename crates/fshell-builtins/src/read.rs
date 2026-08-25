// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crossterm::event::{self, Event, KeyCode, KeyEvent};
use fshell_core::Val;
use fshell_core::diagnostic::StringError;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::time::timeout;

pub fn read_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    let mut prompt_str = None;
    let mut timeout_secs = None;
    let mut silent = false;
    let mut var_name = "REPLY".to_string();

    let mut idx = 0;
    while idx < args.len() {
        let s = match &args[idx] {
            Val::String(st) => st.clone(),
            other => other.to_text(),
        };

        if s == "-p" {
            if idx + 1 < args.len() {
                idx += 1;
                prompt_str = match &args[idx] {
                    Val::String(st) => Some(st.clone()),
                    other => Some(other.to_text()),
                };
            } else {
                return Err("read: expected prompt string after -p".to_string().into());
            }
        } else if s == "-t" {
            if idx + 1 < args.len() {
                idx += 1;
                let t_str = match &args[idx] {
                    Val::String(st) => st.clone(),
                    other => other.to_text(),
                };
                let t = t_str
                    .parse::<u64>()
                    .map_err(|_| "read: invalid timeout value".to_string())?;
                timeout_secs = Some(t);
            } else {
                return Err("read: expected timeout value after -t".to_string().into());
            }
        } else if s == "-s" {
            silent = true;
        } else {
            // Treat as variable name
            var_name = s;
        }
        idx += 1;
    }

    let env_clone = env.clone();
    let tx_clone = tx.clone();

    tokio::spawn(async move {
        // Print prompt if provided, but only when not reading from pipeline
        if in_rx.is_none()
            && let Some(ref p) = prompt_str
        {
            print!("{}", p);
            let _ = std::io::stdout().flush();
        }

        let input_res = if let Some(mut rx) = in_rx {
            let read_fut = async {
                match rx.recv().await {
                    Some(PipelinePayload::Data(val)) => Ok(val.to_text()),
                    Some(PipelinePayload::Bytes(b)) => Ok(String::from_utf8_lossy(&b).into_owned()),
                    Some(PipelinePayload::Structured(d)) => Err(d),
                    None => Ok(String::new()),
                }
            };

            if let Some(t) = timeout_secs {
                match timeout(Duration::from_secs(t), read_fut).await {
                    Ok(res) => res,
                    Err(_) => Ok(String::new()),
                }
            } else {
                read_fut.await
            }
        } else if silent {
            read_line_silent(timeout_secs).await.map_err(|e| e.into())
        } else {
            read_line_standard(timeout_secs).await.map_err(|e| e.into())
        };
        match input_res {
            Ok(line) => {
                env_clone
                    .vars
                    .write()
                    .insert(var_name, Val::String(line.clone()));
                let _ = tx_clone
                    .send(PipelinePayload::Data(Arc::new(Val::String(line))))
                    .await;
            }
            Err(e) => {
                let _ = tx_clone.send(PipelinePayload::Structured(e)).await;
            }
        }
    });

    Ok(())
}

async fn read_line_standard(timeout_secs: Option<u64>) -> Result<String, String> {
    let read_fut = async {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();
        let _ = reader.read_line(&mut line).await;
        // Trim trailing newline
        if line.ends_with('\n') {
            line.pop();
        }
        if line.ends_with('\r') {
            line.pop();
        }
        line
    };

    if let Some(t) = timeout_secs {
        match timeout(Duration::from_secs(t), read_fut).await {
            Ok(res) => Ok(res),
            Err(_) => Ok(String::new()), // timeout returns empty string
        }
    } else {
        Ok(read_fut.await)
    }
}

async fn read_line_silent(timeout_secs: Option<u64>) -> Result<String, String> {
    crossterm::terminal::enable_raw_mode()
        .map_err(|e| format!("Failed to enable raw mode: {}", e))?;

    let mut line = String::new();
    let start = std::time::Instant::now();

    let result = loop {
        if let Some(t) = timeout_secs
            && start.elapsed().as_secs() >= t
        {
            break Ok(String::new());
        }

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let stdin_fd = std::io::stdin().as_raw_fd();
            if unsafe { libc::isatty(stdin_fd) } == 0 {
                break Ok(line);
            }
        }

        let poll_duration = Duration::from_millis(100);
        if let Ok(true) = event::poll(poll_duration)
            && let Ok(Event::Key(KeyEvent { code, .. })) = event::read()
        {
            match code {
                KeyCode::Enter => {
                    println!(); // Print newline to mimic enter press behavior
                    break Ok(line);
                }
                KeyCode::Esc => {
                    break Ok(String::new());
                }
                KeyCode::Backspace => {
                    line.pop();
                }
                KeyCode::Char(c) => {
                    line.push(c);
                }
                _ => {}
            }
        }
    };

    let _ = crossterm::terminal::disable_raw_mode();
    result
}
