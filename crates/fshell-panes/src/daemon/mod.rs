// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Daemon process for fshell-panes.
//!
//! The daemon is the persistent background process that:
//! - Listens on a Unix domain socket for client connections.
//! - Manages sessions (create, attach, detach, kill).
//! - Runs PtyActors + GridManagers for each pane.
//! - Renders terminal state and sends ANSI bytes to attached clients.
//! - Persists session state to disk for recovery.

pub mod client_handler;
pub mod renderer;
pub mod session;
pub mod session_manager;
pub mod window;

use std::sync::Arc;

use tokio::net::UnixListener;
use tokio::sync::{RwLock, mpsc};

use crate::proto::{cleanup_stale_socket, get_socket_path};

use self::session_manager::SessionManager;

/// Events that the daemon processes in its main loop.
#[derive(Debug)]
pub enum DaemonEvent {
    /// A session's pane exited.
    PaneExited { session_name: String, pane_id: u32 },
    /// Graceful shutdown requested.
    Shutdown,
}

/// Run the daemon.
///
/// This is the main entry point for the daemon process.
pub async fn run_daemon() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let socket_path = get_socket_path();
    eprintln!("fshell-panesd: socket path = {}", socket_path.display());

    // Clean up stale socket if present.
    cleanup_stale_socket(&socket_path)?;

    // Ensure parent directory exists.
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Remove old socket file if it exists (we just checked it's not in use).
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }

    // Bind the Unix socket.
    let listener = UnixListener::bind(&socket_path)?;
    eprintln!("fshell-panesd: listening on {}", socket_path.display());

    // Channels for daemon events.
    let (daemon_event_tx, mut daemon_event_rx) = mpsc::channel::<DaemonEvent>(256);

    // Shared session manager.
    let session_manager = Arc::new(RwLock::new(SessionManager::new()));

    // Set up signal handlers.
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to register SIGTERM handler");
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("failed to register SIGINT handler");
    let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .expect("failed to register SIGHUP handler");

    // Main daemon loop.
    loop {
        tokio::select! {
            // Accept new client connections.
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _addr)) => {
                        eprintln!("fshell-panesd: new client connected");
                        let sm = session_manager.clone();
                        let event_tx = daemon_event_tx.clone();
                        tokio::spawn(async move {
                            client_handler::handle_client(stream, sm, event_tx).await;
                        });
                    }
                    Err(e) => {
                        eprintln!("fshell-panesd: accept error: {}", e);
                    }
                }
            }

            // Handle shutdown signals.
            _ = sigterm.recv() => {
                eprintln!("fshell-panesd: received SIGTERM, shutting down...");
                break;
            }
            _ = sigint.recv() => {
                eprintln!("fshell-panesd: received SIGINT, shutting down...");
                break;
            }
            _ = sighup.recv() => {
                // Ignore SIGHUP — daemon survives terminal close.
                eprintln!("fshell-panesd: ignoring SIGHUP");
            }

            // Process daemon events.
            event = daemon_event_rx.recv() => {
                match event {
                    Some(DaemonEvent::PaneExited { session_name, pane_id }) => {
                        let mgr = session_manager.read().await;
                        if let Some(session) = mgr.get_session(&session_name).await {
                            let mut s = session.write().await;
                            s.remove_pane(pane_id);

                            // If session is now empty, remove it.
                            if s.is_empty() {
                                drop(s);
                                drop(mgr);
                                let mut mgr_write = session_manager.write().await;
                                let _ = mgr_write.kill_session(&session_name).await;
                                eprintln!("fshell-panesd: session '{}' terminated (all panes exited)", session_name);
                            }
                        }
                    }

                    Some(DaemonEvent::Shutdown) | None => {
                        eprintln!("fshell-panesd: shutting down");
                        break;
                    }
                }
            }
        }
    }

    // Cleanup: remove socket file.
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    eprintln!("fshell-panesd: exited cleanly");
    Ok(())
}
