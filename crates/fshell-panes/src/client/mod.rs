// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Client process for fshell-panes.
//!
//! The client is a minimal "dumb pipe" that:
//! 1. Connects to the daemon via Unix socket.
//! 2. Forwards terminal input to the daemon.
//! 3. Receives rendered ANSI bytes from the daemon and writes to stdout.
//!
//! The client does NOT perform any rendering itself — all terminal state
//! management happens in the daemon.

use std::io;

use futures::{SinkExt, StreamExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_util::codec::Framed;

use crate::proto::codec::FshCodec;
use crate::proto::message::{ClientMessage, ServerMessage};
use crate::proto::{Frame, get_socket_path};
use fshell_terminal::input::{
    CrosstermEventStream, InputEvent, InputPoll, Key, Modifiers, MouseAction, MouseButton,
};

/// Guard that restores the terminal on drop (including panic).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::event::DisableMouseCapture,
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    }
}

/// Run the client, connecting to the daemon.
///
/// This is the main entry point for the client process.
pub async fn connect_or_spawn_daemon(
    socket_path: &std::path::Path,
) -> Result<UnixStream, io::Error> {
    // Locate fshell-panesd executable
    let daemon_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("fshell-panesd")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| std::path::PathBuf::from("fshell-panesd"));

    // Check if the running daemon process is outdated compared to daemon_exe on disk
    if socket_path.exists() {
        let is_stale = (|| {
            let daemon_mtime = daemon_exe.metadata().ok()?.modified().ok()?;
            let socket_mtime = socket_path.metadata().ok()?.modified().ok()?;
            Some(daemon_mtime > socket_mtime)
        })()
        .unwrap_or(false);

        if is_stale {
            // Attempt clean shutdown of stale daemon
            if let Ok(mut stream) = UnixStream::connect(socket_path).await {
                use crate::proto::codec::FshCodec;
                use crate::proto::message::ClientMessage;
                use futures::SinkExt;
                use tokio_util::codec::Framed;
                let mut framed = Framed::new(&mut stream, FshCodec);
                let _ = framed
                    .send(Frame::from_client(&ClientMessage::KillServer))
                    .await;
            }
            let _ = std::fs::remove_file(socket_path);
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        } else if let Ok(stream) = UnixStream::connect(socket_path).await {
            return Ok(stream);
        }
    }

    let spawn_result = std::process::Command::new(&daemon_exe)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    if let Err(e) = spawn_result {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "fshell-panesd daemon executable not found ({:?}): {}. Run 'cargo build --workspace' first.",
                daemon_exe, e
            ),
        ));
    }

    // Poll for socket readiness up to 2 seconds (40 * 50ms)
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(stream) = UnixStream::connect(socket_path).await {
            return Ok(stream);
        }
    }

    UnixStream::connect(socket_path).await
}

/// Run the client, connecting to the daemon.
///
/// This is the main entry point for the client process.
pub async fn run_client(
    session_name: Option<String>,
    shell: Option<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let socket_path = get_socket_path();

    // Try to connect to or auto-start the daemon.
    let stream = match connect_or_spawn_daemon(&socket_path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "fshell-panes: failed to connect to daemon at {}: {}",
                socket_path.display(),
                e
            );
            return Err(e.into());
        }
    };

    eprintln!(
        "fshell-panes: connected to daemon at {}",
        socket_path.display()
    );

    // Setup terminal.
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::cursor::Hide
    )?;
    let _guard = TerminalGuard;

    // Get initial terminal size.
    let (cols, rows) = crossterm::terminal::size()?;

    // Split the socket into sink/stream.
    let framed = Framed::new(stream, FshCodec);
    let (mut sink, mut stream) = framed.split();

    // Send initial attach command with terminal size.
    let name = session_name.unwrap_or_else(|| "default".to_string());
    let attach_msg = ClientMessage::Attach {
        session_name: name.clone(),
        cols,
        rows,
        shell,
    };
    sink.send(Frame::from_client(&attach_msg)).await?;

    // Channel for forwarding stdin bytes to the sink.
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(256);

    // Channel for structured commands (scroll, etc.).
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<ClientMessage>(64);
    let cmd_tx_clone2 = cmd_tx.clone();

    // Channel for resize events from the event stream.
    let (resize_tx, mut resize_rx) = mpsc::channel::<(u16, u16)>(16);
    let resize_tx_clone = resize_tx.clone();
    let cmd_tx_clone = cmd_tx.clone();

    // Spawn stdin reader task.
    tokio::spawn(async move {
        let mut event_stream = CrosstermEventStream::new();
        let mut prefix_active = false;
        let mut help_active = false;
        let mut rename_active = false;
        let mut rename_target_window = false;
        let mut rename_buffer = String::new();

        loop {
            match event_stream.next().await {
                Ok(InputPoll::Event(InputEvent::Key(key))) => {
                    let code = key.key;
                    let modifiers = key.modifiers;
                    // When help overlay is open, only Esc/q/?/j/k work.
                    if help_active {
                        match code {
                            Key::Escape | Key::Character('q') | Key::Character('?') => {
                                help_active = false;
                                let _ = cmd_tx_clone2.send(ClientMessage::HelpKey(0)).await;
                                continue;
                            }
                            _ => continue,
                        }
                    }

                    // Rename mode: capture characters as the new label.
                    if rename_active {
                        match code {
                            Key::Enter => {
                                rename_active = false;
                                let label = rename_buffer.clone();
                                rename_buffer.clear();
                                if rename_target_window {
                                    let _ = cmd_tx_clone2
                                        .send(ClientMessage::WindowRenameConfirm { label })
                                        .await;
                                } else {
                                    let _ = cmd_tx_clone2
                                        .send(ClientMessage::RenameConfirm { label })
                                        .await;
                                }
                                continue;
                            }
                            Key::Escape => {
                                rename_active = false;
                                rename_buffer.clear();
                                let _ = cmd_tx_clone2.send(ClientMessage::RenameCancel).await;
                                continue;
                            }
                            Key::Backspace => {
                                rename_buffer.pop();
                                let _ = cmd_tx_clone2.send(ClientMessage::RenameBackspace).await;
                                continue;
                            }
                            Key::Character(c) => {
                                rename_buffer.push(c);
                                let _ = cmd_tx_clone2.send(ClientMessage::RenameChar(c)).await;
                                continue;
                            }
                            _ => continue,
                        }
                    }
                    // Ctrl+A: toggle prefix mode.
                    if code == Key::Character('a') && modifiers.contains(Modifiers::CONTROL) {
                        prefix_active = !prefix_active;
                        let _ = cmd_tx_clone2
                            .send(ClientMessage::PrefixToggle {
                                active: prefix_active,
                            })
                            .await;
                        continue;
                    }

                    // If prefix is active, interpret the next key as a command.
                    if prefix_active {
                        prefix_active = false;
                        // Always tell daemon to exit prefix mode.
                        let _ = cmd_tx_clone2
                            .send(ClientMessage::PrefixToggle { active: false })
                            .await;
                        let cmd = match code {
                            Key::Character('%') | Key::Character('|') => {
                                Some(crate::proto::message::PrefixCommand::SplitVertical)
                            }
                            Key::Character('"') | Key::Character('-') => {
                                Some(crate::proto::message::PrefixCommand::SplitHorizontal)
                            }
                            Key::Character('c') => {
                                Some(crate::proto::message::PrefixCommand::WindowNew)
                            }
                            Key::Character('x') => {
                                Some(crate::proto::message::PrefixCommand::KillPane)
                            }
                            Key::Up | Key::Character('k') => {
                                Some(crate::proto::message::PrefixCommand::FocusUp)
                            }
                            Key::Down | Key::Character('j') => {
                                Some(crate::proto::message::PrefixCommand::FocusDown)
                            }
                            Key::Left | Key::Character('h') => {
                                Some(crate::proto::message::PrefixCommand::FocusLeft)
                            }
                            Key::Right | Key::Character('l') => {
                                Some(crate::proto::message::PrefixCommand::FocusRight)
                            }
                            Key::Character('?') => {
                                help_active = true;
                                Some(crate::proto::message::PrefixCommand::ShowHelp)
                            }
                            Key::Character('q') => Some(crate::proto::message::PrefixCommand::Quit),
                            Key::Character(',') => {
                                // Enter pane rename mode — send RenameStart to daemon
                                rename_active = true;
                                rename_target_window = false;
                                rename_buffer.clear();
                                let _ = cmd_tx_clone2
                                    .send(ClientMessage::RenameStart {
                                        current_label: String::new(),
                                    })
                                    .await;
                                let _ = cmd_tx_clone2
                                    .send(ClientMessage::PrefixToggle { active: false })
                                    .await;
                                continue;
                            }
                            Key::Character('n') => {
                                Some(crate::proto::message::PrefixCommand::WindowNext)
                            }
                            Key::Character('p') => {
                                Some(crate::proto::message::PrefixCommand::WindowPrevious)
                            }
                            Key::Character('0')
                            | Key::Character('1')
                            | Key::Character('2')
                            | Key::Character('3')
                            | Key::Character('4')
                            | Key::Character('5')
                            | Key::Character('6')
                            | Key::Character('7')
                            | Key::Character('8')
                            | Key::Character('9') => {
                                if let Key::Character(c) = code {
                                    let n = (c as u32) - ('0' as u32);
                                    Some(crate::proto::message::PrefixCommand::WindowSwitch(n))
                                } else {
                                    None
                                }
                            }
                            Key::Character('&') => {
                                Some(crate::proto::message::PrefixCommand::WindowClose)
                            }
                            Key::Character('W') => {
                                // Enter window rename mode
                                rename_active = true;
                                rename_target_window = true;
                                rename_buffer.clear();
                                let _ = cmd_tx_clone2
                                    .send(ClientMessage::WindowRename {
                                        label: String::new(),
                                    })
                                    .await;
                                let _ = cmd_tx_clone2
                                    .send(ClientMessage::PrefixToggle { active: false })
                                    .await;
                                continue;
                            }
                            _ => None,
                        };
                        if let Some(cmd) = cmd {
                            let _ = cmd_tx_clone2.send(ClientMessage::PrefixCommand(cmd)).await;
                            continue;
                        }
                        // Unknown prefix key: fall through and forward normally.
                    }

                    // PageUp/PageDown: send as scroll command (daemon-side scrolling).
                    match code {
                        Key::PageUp => {
                            let _ = cmd_tx_clone.send(ClientMessage::Scroll { lines: -1 }).await;
                            continue;
                        }
                        Key::PageDown => {
                            let _ = cmd_tx_clone.send(ClientMessage::Scroll { lines: 1 }).await;
                            continue;
                        }
                        _ => {}
                    }

                    // Shift+Up/Down: also scroll.
                    if modifiers.contains(Modifiers::SHIFT) {
                        match code {
                            Key::Up => {
                                let _ =
                                    cmd_tx_clone.send(ClientMessage::Scroll { lines: -3 }).await;
                                continue;
                            }
                            Key::Down => {
                                let _ = cmd_tx_clone.send(ClientMessage::Scroll { lines: 3 }).await;
                                continue;
                            }
                            _ => {}
                        }
                    }

                    // Forward all other key events to the daemon as raw bytes.
                    let mut bytes = Vec::new();

                    // Prepend ESC for Alt modifier if set
                    if modifiers.contains(Modifiers::ALT) && code != Key::Escape {
                        bytes.push(0x1b);
                    }

                    match code {
                        Key::Character(c) => {
                            if modifiers.contains(Modifiers::CONTROL) {
                                // Ctrl+char: send the control character.
                                bytes.push((c as u8) & 0x1f);
                            } else {
                                let mut buf = [0u8; 4];
                                bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                            }
                        }
                        Key::Enter => bytes.push(b'\r'),
                        Key::Tab => bytes.push(b'\t'),
                        Key::BackTab => bytes.extend_from_slice(b"\x1b[Z"),
                        Key::Backspace => bytes.push(0x08),
                        Key::Escape => bytes.push(0x1b),
                        Key::Up => bytes.extend_from_slice(b"\x1b[A"),
                        Key::Down => bytes.extend_from_slice(b"\x1b[B"),
                        Key::Right => bytes.extend_from_slice(b"\x1b[C"),
                        Key::Left => bytes.extend_from_slice(b"\x1b[D"),
                        Key::Home => bytes.extend_from_slice(b"\x1b[H"),
                        Key::End => bytes.extend_from_slice(b"\x1b[F"),
                        Key::Delete => bytes.extend_from_slice(b"\x1b[3~"),
                        Key::Insert => bytes.extend_from_slice(b"\x1b[2~"),
                        Key::Function(n) => {
                            // F1-F12: send correct xterm escape sequences.
                            match n {
                                1 => bytes.extend_from_slice(b"\x1bOP"),
                                2 => bytes.extend_from_slice(b"\x1bOQ"),
                                3 => bytes.extend_from_slice(b"\x1bOR"),
                                4 => bytes.extend_from_slice(b"\x1bOS"),
                                5 => bytes.extend_from_slice(b"\x1b[15~"),
                                6 => bytes.extend_from_slice(b"\x1b[17~"),
                                7 => bytes.extend_from_slice(b"\x1b[18~"),
                                8 => bytes.extend_from_slice(b"\x1b[19~"),
                                9 => bytes.extend_from_slice(b"\x1b[20~"),
                                10 => bytes.extend_from_slice(b"\x1b[21~"),
                                11 => bytes.extend_from_slice(b"\x1b[23~"),
                                12 => bytes.extend_from_slice(b"\x1b[24~"),
                                _ => {}
                            }
                        }
                        _ => {}
                    }

                    if !bytes.is_empty() {
                        let _ = stdin_tx.send(bytes).await;
                    }
                }
                Ok(InputPoll::Event(InputEvent::Mouse(mouse))) => match mouse.action {
                    MouseAction::ScrollUp => {
                        let _ = cmd_tx_clone.send(ClientMessage::Scroll { lines: -1 }).await;
                    }
                    MouseAction::ScrollDown => {
                        let _ = cmd_tx_clone.send(ClientMessage::Scroll { lines: 1 }).await;
                    }
                    MouseAction::Down(MouseButton::Left) => {
                        let _ = cmd_tx_clone
                            .send(ClientMessage::MouseClick {
                                col: mouse.column,
                                row: mouse.row,
                            })
                            .await;
                    }
                    _ => {}
                },
                Ok(InputPoll::Event(InputEvent::Resize {
                    columns: new_cols,
                    rows: new_rows,
                })) => {
                    // Send resize through the channel to the main loop.
                    let _ = resize_tx_clone.send((new_cols, new_rows)).await;
                }
                Ok(InputPoll::Closed) => break,
                Ok(InputPoll::Timeout) => {}
                Ok(InputPoll::Event(InputEvent::Paste(_))) => {}
                Err(e) => {
                    eprintln!("fshell-panes: event stream error: {}", e);
                    break;
                }
            }
        }
    });

    // Spawn resize poller (backup for terminals that don't send Resize events).
    let resize_tx_poller = resize_tx.clone();
    tokio::spawn(async move {
        let mut last_size = crossterm::terminal::size().unwrap_or((80, 24));
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            if let Ok(size) = crossterm::terminal::size()
                && size != last_size
            {
                last_size = size;
                let _ = resize_tx_poller.send(size).await;
            }
        }
    });

    // Main client loop.
    loop {
        tokio::select! {
            // Incoming frames from daemon.
            frame = stream.next() => {
                match frame {
                    Some(Ok(frame)) => {
                        let msg = match frame.into_server() {
                            Ok(m) => m,
                            Err(e) => {
                                eprintln!("fshell-panes: decode error: {}", e);
                                continue;
                            }
                        };

                        match msg {
                            ServerMessage::Draw(bytes) => {
                                // Write raw ANSI bytes directly to stdout.
                                let mut stdout = io::stdout();
                                if let Err(e) = io::Write::write_all(&mut stdout, &bytes) {
                                    eprintln!("fshell-panes: write error: {}", e);
                                    break;
                                }
                                let _ = io::Write::flush(&mut stdout);
                            }
                            ServerMessage::ExitClient => {
                                eprintln!("fshell-panes: daemon requested exit");
                                break;
                            }
                            ServerMessage::SessionList(list) => {
                                // Print session list and exit.
                                for session in &list {
                                    println!(
                                        "{}: {} panes{}",
                                        session.name,
                                        session.pane_count,
                                        if session.attached { " (attached)" } else { "" }
                                    );
                                }
                                break;
                            }
                            ServerMessage::Ack => {
                                // Command acknowledged.
                            }
                            ServerMessage::WindowList { .. } => {
                                // Not used in interactive mode.
                            }
                        }
                    }
                    Some(Err(e)) => {
                        eprintln!("fshell-panes: connection error: {}", e);
                        break;
                    }
                    None => {
                        eprintln!("fshell-panes: daemon disconnected");
                        break;
                    }
                }
            }

            // Stdin input from the reader task.
            input = stdin_rx.recv() => {
                if let Some(bytes) = input {
                    let msg = ClientMessage::Input(bytes);
                    if let Err(e) = sink.send(Frame::from_client(&msg)).await {
                        eprintln!("fshell-panes: send error: {}", e);
                        break;
                    }
                }
            }

            // Structured commands (scroll, etc.).
            cmd = cmd_rx.recv() => {
                if let Some(msg) = cmd
                    && let Err(e) = sink.send(Frame::from_client(&msg)).await {
                        eprintln!("fshell-panes: cmd send error: {}", e);
                        break;
                    }
            }

            // Resize events.
            resize = resize_rx.recv() => {
                if let Some((cols, rows)) = resize {
                    let msg = ClientMessage::Resize { cols, rows };
                    if let Err(e) = sink.send(Frame::from_client(&msg)).await {
                        eprintln!("fshell-panes: resize send error: {}", e);
                        break;
                    }
                }
            }
        }
    }

    // Send detach before exiting.
    let detach_msg = ClientMessage::Detach;
    let _ = sink.send(Frame::from_client(&detach_msg)).await;

    Ok(())
}
