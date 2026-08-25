// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_panes::pty::async_pty::AsyncPty;
use fshell_panes::pty::grid_manager::{GridManager, PtyCommand};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::mpsc;

#[tokio::test]
async fn pty_integration_write_then_read_grid() {
    let mut pty = AsyncPty::spawn("/bin/sh", 80, 24).unwrap();

    // Drain any shell prompt
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    let mut buf = [0u8; 4096];
    let _ = pty.read(&mut buf).await;

    // Send command
    pty.write(b"echo hello_pty\n").await.unwrap();

    // Wait for output
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    let n = pty.read(&mut buf).await.unwrap();

    // Feed through GridManager
    let (tx, rx) = mpsc::channel(256);
    let dirty = Arc::new(AtomicBool::new(false));
    let (mut manager, grid_ref) = GridManager::new(80, 24, 1000, rx, dirty);

    tx.send(PtyCommand::Data(buf[..n].to_vec())).await.unwrap();
    tx.send(PtyCommand::Shutdown).await.unwrap();
    manager.run().await;

    // Assert grid contains the output
    let guard = grid_ref.read().await;
    let vp = guard.viewport();

    let row0: String = vp[0].cells().iter().map(|c| c.character).collect();
    let row1: String = vp[1].cells().iter().map(|c| c.character).collect();
    let combined = format!("{row0}{row1}");

    assert!(
        combined.contains("hello_pty"),
        "Expected 'hello_pty' in viewport.\nRow 0: {row0:?}\nRow 1: {row1:?}"
    );
}

#[tokio::test]
async fn pty_integration_resize() {
    let pty = AsyncPty::spawn("/bin/sh", 80, 24).unwrap();

    pty.resize(120, 40).unwrap();

    let (tx, rx) = mpsc::channel(256);
    let dirty = Arc::new(AtomicBool::new(false));
    let (mut manager, grid_ref) = GridManager::new(80, 24, 1000, rx, dirty);

    tx.send(PtyCommand::Resize(120, 40)).await.unwrap();
    tx.send(PtyCommand::Shutdown).await.unwrap();
    manager.run().await;

    let guard = grid_ref.read().await;
    assert_eq!(guard.width(), 120);
    assert_eq!(guard.height(), 40);
}

#[tokio::test]
async fn pty_actor_pipeline_test() {
    let pty = AsyncPty::spawn("/bin/sh", 80, 24).unwrap();
    let (pty_tx, pty_rx) = mpsc::channel(256);
    let (grid_tx, grid_rx) = mpsc::channel(256);

    let dirty = Arc::new(AtomicBool::new(false));
    let (mut manager, grid_ref) = GridManager::new(80, 24, 1000, grid_rx, dirty);
    tokio::spawn(async move { manager.run().await });

    let actor = fshell_panes::pty::pty_actor::PtyActor::new(pty, grid_tx, pty_rx, 1, None);
    tokio::spawn(async move { actor.run().await });

    // Wait a brief moment for shell prompt, then send command
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    pty_tx
        .send(PtyCommand::Data(b"echo actor_pipeline_works\n".to_vec()))
        .await
        .unwrap();

    // Wait for the command to execute and update the grid
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    // Read the grid
    let guard = grid_ref.read().await;
    let vp = guard.viewport();
    let mut found = false;
    for row_ref in vp {
        let row: String = row_ref.cells().iter().map(|c| c.character).collect();
        if row.contains("actor_pipeline_works") {
            found = true;
            break;
        }
    }
    assert!(found, "Expected 'actor_pipeline_works' in grid rows");

    // Clean shutdown
    pty_tx.send(PtyCommand::Shutdown).await.unwrap();
}
