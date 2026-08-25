// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Actor task that owns the AsyncPty, handles reads/writes, resizes, and lifecycles.

use tokio::sync::mpsc;

/// Event emitted by a pane's PTY pipeline when its shell exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    PaneExited(u32),
}

use crate::pty::async_pty::AsyncPty;
use crate::pty::grid_manager::PtyCommand;

/// Actor that coordinates with the AsyncPty and forwards outputs to the GridManager.
pub struct PtyActor {
    pty: AsyncPty,
    grid_tx: mpsc::Sender<PtyCommand>,
    rx: mpsc::Receiver<PtyCommand>,
    id: u32,
    app_tx: Option<mpsc::Sender<AppEvent>>,
}

impl PtyActor {
    /// Create a new PtyActor.
    pub fn new(
        pty: AsyncPty,
        grid_tx: mpsc::Sender<PtyCommand>,
        rx: mpsc::Receiver<PtyCommand>,
        id: u32,
        app_tx: Option<mpsc::Sender<AppEvent>>,
    ) -> Self {
        Self {
            pty,
            grid_tx,
            rx,
            id,
            app_tx,
        }
    }

    /// Main actor loop. Run this inside a tokio task.
    pub async fn run(mut self) {
        let mut buf = [0u8; 8192];
        loop {
            tokio::select! {
                // 1. Read from PTY and forward to GridManager
                read_res = self.pty.read(&mut buf) => {
                    match read_res {
                        Ok(n) if n > 0 => {
                            if self.grid_tx.send(PtyCommand::Data(buf[..n].to_vec())).await.is_err() {
                                break;
                            }
                        }
                        Ok(_) | Err(_) => {
                            let _ = self.grid_tx.send(PtyCommand::ProcessExit).await;
                            if let Some(ref tx) = self.app_tx {
                                let _ = tx.send(AppEvent::PaneExited(self.id)).await;
                            }
                            break;
                        }
                    }
                }
                // 2. Receive commands from App and execute them
                maybe_cmd = self.rx.recv() => {
                    match maybe_cmd {
                        Some(PtyCommand::Data(bytes)) => {
                            if self.pty.write(&bytes).await.is_err() {
                                break;
                            }
                        }
                        Some(PtyCommand::Resize(cols, rows)) => {
                            let _ = self.pty.resize(cols, rows);
                            if self.grid_tx.send(PtyCommand::Resize(cols, rows)).await.is_err() {
                                break;
                            }
                        }
                        Some(PtyCommand::Shutdown) => {
                            let _ = self.pty.kill();
                            let _ = self.grid_tx.send(PtyCommand::Shutdown).await;
                            break;
                        }
                        Some(PtyCommand::ProcessExit) => {
                            let _ = self.grid_tx.send(PtyCommand::ProcessExit).await;
                            if let Some(ref tx) = self.app_tx {
                                let _ = tx.send(AppEvent::PaneExited(self.id)).await;
                            }
                            break;
                        }
                        None => {
                            // Sender dropped, clean up PTY
                            let _ = self.pty.kill();
                            let _ = self.grid_tx.send(PtyCommand::Shutdown).await;
                            break;
                        }
                    }
                }
            }
        }
    }
}
