// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_panes::pty::grid_manager::{GridManager, PtyCommand};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[tokio::test]
async fn grid_manager_processes_text() {
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let dirty = Arc::new(AtomicBool::new(false));
    let (mut manager, grid_ref) = GridManager::new(80, 24, 1000, rx, dirty);

    tx.send(PtyCommand::Data(b"hello world".to_vec()))
        .await
        .unwrap();
    tx.send(PtyCommand::Shutdown).await.unwrap();

    manager.run().await;

    let guard = grid_ref.read().await;
    let vp = guard.viewport();
    assert_eq!(vp[0].cells()[0].character, 'h');
    assert_eq!(vp[0].cells()[4].character, 'o');
}

#[tokio::test]
async fn grid_manager_handles_resize() {
    let (tx, rx) = tokio::sync::mpsc::channel(256);
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
async fn grid_manager_handles_sgr_across_chunks() {
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let dirty = Arc::new(AtomicBool::new(false));
    let (mut manager, grid_ref) = GridManager::new(80, 24, 1000, rx, dirty);

    // Bold escape in first chunk, text in second
    tx.send(PtyCommand::Data(b"\x1b[1m".to_vec()))
        .await
        .unwrap();
    tx.send(PtyCommand::Data(b"bold text".to_vec()))
        .await
        .unwrap();
    tx.send(PtyCommand::Shutdown).await.unwrap();

    manager.run().await;

    let guard = grid_ref.read().await;
    let vp = guard.viewport();
    assert!(vp[0].cells()[0].pen.bold);
    assert_eq!(vp[0].cells()[0].character, 'b');
}

#[tokio::test]
async fn grid_manager_process_exit() {
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let dirty = Arc::new(AtomicBool::new(false));
    let (mut manager, grid_ref) = GridManager::new(80, 24, 1000, rx, dirty);

    tx.send(PtyCommand::Data(b"before exit".to_vec()))
        .await
        .unwrap();
    tx.send(PtyCommand::ProcessExit).await.unwrap();
    // Data after exit should be ignored
    tx.send(PtyCommand::Data(b"after exit".to_vec()))
        .await
        .unwrap();

    manager.run().await;

    assert!(manager.is_exited());
    let guard = grid_ref.read().await;
    let vp = guard.viewport();
    // Should contain "before exit" and the exit message
    let row0: String = vp[0].cells().iter().map(|c| c.character).collect();
    assert!(
        row0.contains("before exit"),
        "Expected 'before exit' in row 0: {row0:?}"
    );
    // The exit message should be somewhere in the viewport
    let all_text: String = vp
        .iter()
        .flat_map(|r| r.cells().iter().map(|c| c.character))
        .collect();
    assert!(
        all_text.contains("Process exited"),
        "Expected 'Process exited' in viewport"
    );
}

#[tokio::test]
async fn grid_manager_grid_is_shared() {
    let (_tx, rx) = tokio::sync::mpsc::channel(256);
    let dirty = Arc::new(AtomicBool::new(false));
    let (_manager, grid_ref) = GridManager::new(80, 24, 1000, rx, dirty);

    // Both handles point to the same grid
    let handle1 = grid_ref.clone();
    let handle2 = grid_ref.clone();

    // Write through one handle
    {
        let mut grid = handle1.write().await;
        grid.write_str("shared test");
    }

    // Read through the other
    let guard = handle2.read().await;
    let vp = guard.viewport();
    assert_eq!(vp[0].cells()[0].character, 's');
}
