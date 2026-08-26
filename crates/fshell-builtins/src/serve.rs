// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::{Env, PipeSender, PipeStream};
use miette::SourceSpan;

pub fn serve_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let port = match args.first() {
        Some(Val::Int(p)) => *p as u16,
        Some(Val::String(s)) => s.parse().unwrap_or(8080),
        _ => 8080,
    };

    let live_reload = args
        .iter()
        .any(|a| matches!(a, Val::String(s) if s == "--live"));

    let cwd = env.cwd();
    tokio::spawn(async move {
        if let Err(e) = run_server(port, live_reload, cwd).await {
            eprintln!("serve error: {}", e);
        }
    });

    Ok(())
}

async fn run_server(
    port: u16,
    _live_reload: bool,
    current_dir: std::path::PathBuf,
) -> Result<(), ShellError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .map_err(|e| format!("Failed to bind to port {}: {}", port, e))?;

    println!("Serving on http://localhost:{}", port);

    loop {
        let (mut socket, _addr) = listener
            .accept()
            .await
            .map_err(|e| format!("Failed to accept connection: {}", e))?;

        let dir = current_dir.clone();
        tokio::spawn(async move {
            let mut buffer = [0; 1024];
            let _ = socket.read(&mut buffer).await;

            let request = String::from_utf8_lossy(&buffer);
            let path = extract_path(&request);
            let stripped = path.trim_start_matches('/');
            // Resolve and reject anything that escapes the served directory
            // (e.g. /../../etc/passwd) instead of joining raw segments.
            let candidate = dir.join(stripped);
            let file_path = std::fs::canonicalize(&candidate).unwrap_or(candidate);
            let allowed = file_path.starts_with(&dir);

            let (status, content_type, body) =
                if allowed && file_path.exists() && file_path.is_file() {
                    match tokio::fs::read(&file_path).await {
                        Ok(content) => {
                            let ct = guess_content_type(&file_path);
                            ("200 OK", ct, content)
                        }
                        Err(_) => (
                            "500 Internal Server Error",
                            "text/plain",
                            b"Internal Server Error".to_vec(),
                        ),
                    }
                } else {
                    ("404 Not Found", "text/plain", b"Not Found".to_vec())
                };

            let response = format!(
                "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n",
                status,
                content_type,
                body.len()
            );

            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.write_all(&body).await;
        });
    }
}

fn extract_path(request: &str) -> &str {
    request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
}

fn guess_content_type(path: &std::path::Path) -> &str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") | Some("htm") => "text/html",
        Some("css") => "text/css",
        Some("js") => "application/javascript",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("txt") => "text/plain",
        _ => "application/octet-stream",
    }
}
