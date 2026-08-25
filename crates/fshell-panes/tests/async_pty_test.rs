// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_panes::pty::async_pty::AsyncPty;

#[tokio::test]
async fn pty_spawns_shell() {
    let pty = AsyncPty::spawn("/bin/sh", 80, 24).unwrap();
    assert!(std::mem::size_of_val(&pty) > 0); // struct is non-zero sized
}

#[tokio::test]
async fn pty_read_yields_data() {
    let mut pty = AsyncPty::spawn("/bin/sh", 80, 24).unwrap();

    // Write a command
    pty.write(b"echo test_read\n").await.unwrap();

    // Read response — should contain "test_read"
    let mut buf = [0u8; 4096];
    let n = pty.read(&mut buf).await.unwrap();
    let output = String::from_utf8_lossy(&buf[..n]);
    assert!(
        output.contains("test_read"),
        "Expected 'test_read' in output: {output:?}"
    );
}

#[tokio::test]
async fn pty_resize_sends_winsize() {
    let pty = AsyncPty::spawn("/bin/sh", 80, 24).unwrap();
    // Resize should not error
    pty.resize(120, 40).unwrap();
}
