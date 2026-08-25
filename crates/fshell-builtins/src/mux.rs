// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! `mux` builtin command for controlling terminal multiplexer sessions.

use fshell_core::Val;
use fshell_core::diagnostic::StringError;
use fshell_engine::{Env, PipeSender, PipeStream};
use futures::{SinkExt, StreamExt};
use tokio::net::UnixStream;
use tokio_util::codec::Framed;

use fshell_panes::proto::codec::FshCodec;
use fshell_panes::proto::message::{ClientMessage, ServerMessage};
use fshell_panes::proto::{Frame, get_socket_path};

/// Ensure child panes can find this binary, then attach to (or create) the
/// named session via the panes client. Shared by `attach`, `new`, and the
/// bare-session fallback.
fn run_client_attached(session_name: Option<String>) -> Result<(), StringError> {
    if let Ok(exe) = std::env::current_exe() {
        // SAFETY: single-threaded builtin dispatch before any threads read the env.
        unsafe {
            std::env::set_var("FSHELL_BIN", exe);
        }
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| StringError::from(format!("Failed to create runtime: {e}")))?;

    rt.block_on(async {
        fshell_panes::client::run_client(session_name)
            .await
            .map_err(|e| StringError::from(format!("mux error: {e}")))
    })
}

pub fn mux_builtin(
    _stream: Option<PipeStream>,
    args: Vec<Val>,
    _env: &Env,
    _sender: PipeSender,
) -> Result<(), StringError> {
    let mut args_iter = args.into_iter();
    let subcmd = args_iter
        .next()
        .map(|v| v.as_str().unwrap_or("attach").to_string())
        .unwrap_or_else(|| "attach".to_string());

    match subcmd.as_str() {
        "attach" | "a" => {
            let session_name = args_iter
                .next()
                .map(|v| v.as_str().unwrap_or("default").to_string());
            run_client_attached(session_name)?;
        }
        "new" | "n" => {
            let session_name = args_iter
                .next()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .filter(|s| !s.is_empty());
            run_client_attached(session_name)?;
        }
        "ls" | "list" => {
            let json_mode = args_iter.any(|v| v.as_str() == Some("--json"));
            let socket_path = get_socket_path();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| StringError::from(format!("Failed to create runtime: {e}")))?;

            rt.block_on(async {
                let stream = UnixStream::connect(&socket_path)
                    .await
                    .map_err(|e| StringError::from(format!("Daemon not running: {e}")))?;

                let framed = Framed::new(stream, FshCodec);
                let (mut sink, mut stream_rx) = framed.split();

                sink.send(Frame::from_client(&ClientMessage::ListSessions))
                    .await
                    .map_err(|e| StringError::from(format!("IPC send error: {e}")))?;

                while let Some(res) = stream_rx.next().await {
                    match res {
                        Ok(frame) => {
                            if let Ok(ServerMessage::SessionList(sessions)) = frame.into_server() {
                                if json_mode {
                                    let json_val = serde_json::to_string_pretty(&sessions)
                                        .unwrap_or_else(|_| "[]".to_string());
                                    println!("{json_val}");
                                } else {
                                    if sessions.is_empty() {
                                        println!("No active mux sessions.");
                                    } else {
                                        println!("Active Mux Sessions:");
                                        for s in sessions {
                                            println!(
                                                "  • {} (windows: {}, panes: {})",
                                                s.name, s.window_count, s.pane_count
                                            );
                                        }
                                    }
                                }
                                break;
                            }
                        }
                        Err(e) => return Err(StringError::from(format!("IPC recv error: {e}"))),
                    }
                }
                Ok(())
            })?;
        }
        "kill-session" | "kill" => {
            let target = args_iter
                .next()
                .ok_or_else(|| StringError::from("Usage: mux kill-session <name>".to_string()))?
                .as_str()
                .unwrap_or("")
                .to_string();

            send_ipc_cmd(ClientMessage::KillSession {
                session_name: target.clone(),
            })?;
            println!("Session '{target}' terminated.");
        }
        "kill-server" => {
            send_ipc_cmd(ClientMessage::KillServer)?;
            println!("Mux daemon server shutdown.");
        }
        "new-window" | "nw" => {
            send_ipc_cmd(ClientMessage::WindowNew)?;
        }
        "next-window" | "next" => {
            send_ipc_cmd(ClientMessage::WindowNext)?;
        }
        "prev-window" | "prev" => {
            send_ipc_cmd(ClientMessage::WindowPrevious)?;
        }
        "select-window" | "select" => {
            let index: u32 = args_iter
                .next()
                .and_then(|v| v.as_str().and_then(|s| s.parse().ok()))
                .ok_or_else(|| StringError::from("Usage: mux select-window <index>".to_string()))?;
            send_ipc_cmd(ClientMessage::WindowSwitch { index })?;
        }
        "close-window" | "cw" => {
            send_ipc_cmd(ClientMessage::WindowClose)?;
        }
        "split-window" | "split" => {
            let flag = args_iter
                .next()
                .map(|v| v.as_str().unwrap_or("").to_string())
                .unwrap_or_default();
            let horizontal = flag == "-h" || flag == "--horizontal";
            send_ipc_cmd(ClientMessage::SplitPane { horizontal })?;
        }
        "kill-pane" | "kp" => {
            send_ipc_cmd(ClientMessage::KillPane)?;
        }
        "rename-window" | "rw" => {
            let name = args_iter
                .next()
                .map(|v| v.as_str().unwrap_or("").to_string())
                .unwrap_or_default();
            send_ipc_cmd(ClientMessage::WindowRenameConfirm { label: name })?;
        }
        "rename-pane" | "rp" => {
            let label = args_iter
                .next()
                .map(|v| v.as_str().unwrap_or("").to_string())
                .unwrap_or_default();
            send_ipc_cmd(ClientMessage::RenameConfirm { label })?;
        }
        "help" | "--help" | "-h" => {
            println!("fshell mux commands:");
            println!("  mux attach [session]             Attach to a session");
            println!("  mux new [session]                Create a new session");
            println!("  mux ls [--json]                  List sessions");
            println!("  mux kill-session <name>          Kill a session");
            println!("  mux kill-server                  Shutdown daemon");
            println!("  mux new-window | nw              New window");
            println!("  mux next-window | next           Next window");
            println!("  mux prev-window | prev           Previous window");
            println!("  mux select-window <index>        Select window by index");
            println!("  mux close-window                 Close active window");
            println!("  mux split-window [-v|-h]         Split pane");
            println!("  mux kill-pane                    Kill focused pane");
            println!("  mux rename-window <name>         Rename active window");
            println!("  mux rename-pane <label>          Rename focused pane");
        }
        other => {
            run_client_attached(Some(other.to_string()))?;
        }
    }

    Ok(())
}

fn send_ipc_cmd(msg: ClientMessage) -> Result<(), StringError> {
    let socket_path = get_socket_path();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| StringError::from(format!("Failed to create runtime: {e}")))?;

    rt.block_on(async {
        let stream = UnixStream::connect(&socket_path)
            .await
            .map_err(|e| StringError::from(format!("Daemon not running: {e}")))?;

        let framed = Framed::new(stream, FshCodec);
        let (mut sink, mut stream_rx) = framed.split();

        sink.send(Frame::from_client(&msg))
            .await
            .map_err(|e| StringError::from(format!("IPC send error: {e}")))?;

        let _ = stream_rx.next().await;
        Ok::<(), StringError>(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mux_builtin_registration() {
        let env = Env::new();
        crate::init(&env);
        let registry = env.builtins.read();
        assert!(
            registry.contains_key("mux"),
            "mux builtin should be registered in Env"
        );
    }
}
