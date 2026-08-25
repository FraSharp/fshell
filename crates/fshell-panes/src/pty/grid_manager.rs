// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Actor task that owns the Grid and processes PTY output.
//!
//! The GridManager receives raw byte chunks over an mpsc channel,
//! feeds them through the VTE parser, and updates the shared Grid.
//! The grid is write-locked only during `parser.process()` (~microseconds),
//! so contention with UI reads is negligible.

use crate::grid::{Grid, parser::GridParser};
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};

/// Commands sent from the PTY read loop to the GridManager.
pub enum PtyCommand {
    /// Raw bytes from the PTY — fed through VTE parser.
    Data(Vec<u8>),
    /// Terminal resize request — adjusts grid dimensions.
    Resize(u16, u16),
    /// The shell process has exited (EOF on PTY master).
    /// GridManager stops processing and writes "\r\n[Process exited]\r\n" to the grid.
    ProcessExit,
    /// Clean shutdown signal (programmatic, not from PTY EOF).
    Shutdown,
}

/// Actor that processes PTY output into a shared Grid.
///
/// Holds an `Arc<RwLock<Grid>>` clone. The write lock is acquired
/// only during `parser.process()` — fast enough that contention
/// with concurrent UI reads is negligible.
pub struct GridManager {
    grid: Arc<RwLock<Grid>>,
    parser: GridParser,
    rx: mpsc::Receiver<PtyCommand>,
    pty_tx: Option<mpsc::Sender<PtyCommand>>,
    /// Whether the child process has exited.
    exited: bool,
    dirty: Arc<std::sync::atomic::AtomicBool>,
}

impl GridManager {
    /// Create a new GridManager and return it alongside a shared grid handle.
    pub fn new(
        width: usize,
        height: usize,
        scrollback_limit: usize,
        rx: mpsc::Receiver<PtyCommand>,
        dirty: Arc<std::sync::atomic::AtomicBool>,
    ) -> (Self, Arc<RwLock<Grid>>) {
        let grid = Arc::new(RwLock::new(Grid::new(width, height, scrollback_limit)));
        let grid_clone = grid.clone();
        (
            Self {
                grid,
                parser: GridParser::new(),
                rx,
                pty_tx: None,
                exited: false,
                dirty,
            },
            grid_clone,
        )
    }

    /// Set PTY channel for forwarding ANSI query replies back to the shell process.
    pub fn set_pty_tx(&mut self, tx: mpsc::Sender<PtyCommand>) {
        self.pty_tx = Some(tx);
    }

    /// Access the shared grid handle (for passing to UI or tests).
    pub fn grid_handle(&self) -> Arc<RwLock<Grid>> {
        self.grid.clone()
    }

    /// Whether the child process has exited.
    pub fn is_exited(&self) -> bool {
        self.exited
    }

    /// Main actor loop. Call this from a `tokio::spawn` task.
    ///
    /// Processes `PtyCommand` messages until `Shutdown`, `ProcessExit`,
    /// or the channel closes.
    pub async fn run(&mut self) {
        use std::sync::atomic::Ordering;
        while let Some(cmd) = self.rx.recv().await {
            match cmd {
                PtyCommand::Data(bytes) => {
                    if self.exited {
                        continue; // ignore data after exit
                    }
                    let mut grid = self.grid.write().await;
                    let replies = self.parser.process(&mut grid, &bytes);
                    drop(grid);
                    self.dirty.store(true, Ordering::Relaxed);

                    if let Some(ref tx) = self.pty_tx {
                        for reply in replies {
                            let _ = tx.try_send(PtyCommand::Data(reply));
                        }
                    }
                }
                PtyCommand::Resize(cols, rows) => {
                    let mut grid = self.grid.write().await;
                    grid.resize(cols as usize, rows as usize);
                    drop(grid);
                    self.dirty.store(true, Ordering::Relaxed);
                }
                PtyCommand::ProcessExit => {
                    self.exited = true;
                    // Write exit message to the grid
                    let mut grid = self.grid.write().await;
                    grid.write_str("\r\n[Process exited]");
                    drop(grid);
                    self.dirty.store(true, Ordering::Relaxed);
                    break;
                }
                PtyCommand::Shutdown => {
                    break;
                }
            }
        }
    }
}
