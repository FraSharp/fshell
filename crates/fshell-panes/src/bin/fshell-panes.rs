// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! fshell-panes — the fshell-panes client.
//!
//! This binary connects to a running fshell-panesd daemon and provides
//! an interactive terminal session.
//!
//! Usage:
//!   fshell-panes                   # Attach to default session (or create)
//!   fshell-panes new [name]        # Create and attach to a new session
//!   fshell-panes attach <name>     # Attach to an existing session
//!   fshell-panes ls                # List sessions
//!   fshell-panes kill-session <name>  # Kill a session
//!   fshell-panes kill-server       # Shutdown the daemon

use clap::{Parser, Subcommand};
use fshell_panes::proto::Frame;
use fshell_panes::proto::codec::FshCodec;
use fshell_panes::proto::message::{ClientMessage, ServerMessage};
use futures::{SinkExt, StreamExt};
use tokio::net::UnixStream;
use tokio_util::codec::Framed;

#[derive(Parser)]
#[command(
    name = "fshell-panes",
    about = "fshell-panes — a modern terminal multiplexer",
    version
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new session and attach to it.
    New {
        /// Session name (auto-generated if omitted).
        name: Option<String>,
    },

    /// Attach to an existing session.
    Attach {
        /// Session name.
        name: Option<String>,
    },

    /// List all active sessions.
    Ls {
        /// Output in JSON format.
        #[arg(long)]
        json: bool,
    },

    /// Kill a session by name.
    KillSession {
        /// Session name to kill.
        name: String,
    },

    /// Shutdown the daemon gracefully.
    KillServer,

    /// Create a new window in current session.
    #[command(alias = "nw")]
    NewWindow,

    /// Switch to the next window.
    #[command(alias = "next")]
    NextWindow,

    /// Switch to the previous window.
    #[command(alias = "prev")]
    PrevWindow,

    /// Switch to a window by index.
    #[command(alias = "select")]
    SelectWindow { index: usize },

    /// Close current window.
    CloseWindow,

    /// Split focused pane.
    #[command(alias = "split")]
    SplitPane {
        /// Vertical split (side-by-side)
        #[arg(short = 'v', long = "vertical")]
        vertical: bool,

        /// Horizontal split (top/bottom)
        #[arg(short = 'h', long = "horizontal")]
        horizontal: bool,
    },

    /// Kill focused pane.
    KillPane,

    /// Rename current window.
    #[command(alias = "rw")]
    RenameWindow { name: String },

    /// Rename focused pane.
    #[command(alias = "rp")]
    RenamePane { label: String },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();

    match args.command {
        // Default: attach to a session (create if needed).
        None => fshell_panes::client::run_client(None).await,

        Some(Command::New { name }) => fshell_panes::client::run_client(name).await,

        Some(Command::Attach { name }) => fshell_panes::client::run_client(name).await,

        Some(Command::Ls { json }) => {
            let socket_path = fshell_panes::proto::get_socket_path();
            if !socket_path.exists() {
                eprintln!("fshell-panes: daemon not running");
                std::process::exit(1);
            }

            let stream = UnixStream::connect(&socket_path).await?;
            let mut framed = Framed::new(stream, FshCodec);

            framed
                .send(Frame::from_client(&ClientMessage::ListSessions))
                .await?;

            if let Some(Ok(frame)) = framed.next().await
                && let Ok(ServerMessage::SessionList(list)) = frame.into_server()
            {
                if json {
                    println!("{}", serde_json::to_string_pretty(&list)?);
                } else if list.is_empty() {
                    println!("No active sessions.");
                } else {
                    println!("{:<20} {:<10} {:<10}", "NAME", "PANES", "STATUS");
                    println!("{:<20} {:<10} {:<10}", "----", "-----", "------");
                    for session in &list {
                        println!(
                            "{:<20} {:<10} {:<10}",
                            session.name,
                            session.pane_count,
                            if session.attached {
                                "attached"
                            } else {
                                "detached"
                            }
                        );
                    }
                }
            }

            Ok(())
        }

        Some(Command::KillSession { name }) => {
            send_ipc(ClientMessage::KillSession {
                session_name: name.clone(),
            })
            .await?;
            println!("Session '{}' killed.", name);
            Ok(())
        }

        Some(Command::KillServer) => {
            send_ipc(ClientMessage::KillServer).await?;
            println!("Daemon shutdown requested.");
            Ok(())
        }

        Some(Command::NewWindow) => send_ipc(ClientMessage::WindowNew).await,
        Some(Command::NextWindow) => send_ipc(ClientMessage::WindowNext).await,
        Some(Command::PrevWindow) => send_ipc(ClientMessage::WindowPrevious).await,
        Some(Command::SelectWindow { index }) => {
            send_ipc(ClientMessage::WindowSwitch {
                index: index as u32,
            })
            .await
        }
        Some(Command::CloseWindow) => send_ipc(ClientMessage::WindowClose).await,
        Some(Command::SplitPane { vertical, .. }) => {
            // vertical flag means side-by-side (SplitHorizontal in BSP layout terms)
            send_ipc(ClientMessage::SplitPane {
                horizontal: !vertical,
            })
            .await
        }
        Some(Command::KillPane) => send_ipc(ClientMessage::KillPane).await,
        Some(Command::RenameWindow { name }) => {
            send_ipc(ClientMessage::WindowRenameConfirm { label: name }).await
        }
        Some(Command::RenamePane { label }) => {
            send_ipc(ClientMessage::RenameConfirm { label }).await
        }
    }
}

async fn send_ipc(msg: ClientMessage) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let socket_path = fshell_panes::proto::get_socket_path();
    if !socket_path.exists() {
        eprintln!("fshell-panes: daemon not running");
        std::process::exit(1);
    }
    let stream = UnixStream::connect(&socket_path).await?;
    let mut framed = Framed::new(stream, FshCodec);
    framed.send(Frame::from_client(&msg)).await?;
    let _ = framed.next().await;
    Ok(())
}
