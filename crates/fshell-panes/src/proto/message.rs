// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! IPC message types for client-daemon communication.
//!
//! All messages are serialized with bincode and framed with a 5-byte header:
//! `[length: u32 BE] [type: u8] [payload: bytes]`

use serde::{Deserialize, Serialize};

// Client → Daemon

/// Messages sent from the client to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Client reports its terminal window has resized.
    Resize { cols: u16, rows: u16 },
    /// Keystrokes or escape sequences captured from stdin.
    Input(Vec<u8>),
    /// Attach to a specific session (creates if it doesn't exist).
    Attach {
        session_name: String,
        cols: u16,
        rows: u16,
    },
    /// Gracefully detach the client without killing the session.
    Detach,
    /// Scroll the viewport of the focused pane.
    Scroll { lines: i32 },
    /// Toggle prefix mode on/off.
    PrefixToggle { active: bool },
    /// Prefix-mode command (split, kill, focus, help, quit).
    PrefixCommand(PrefixCommand),
    /// Key pressed while help overlay is open.
    HelpKey(u8),
    /// List all sessions and print to client stdout.
    ListSessions,
    /// Kill a specific session by name.
    KillSession { session_name: String },
    /// Shutdown the daemon gracefully.
    KillServer,
    /// Mouse click at (col, row) — request focus switch.
    MouseClick { col: u16, row: u16 },

    // Rename mode
    /// Enter rename mode. Sends the current label so the daemon can
    /// render the rename bar pre-filled.
    RenameStart { current_label: String },
    /// A character was typed in rename mode.
    RenameChar(char),
    /// Backspace in rename mode.
    RenameBackspace,
    /// Confirm the rename with the accumulated label.
    RenameConfirm { label: String },
    /// Cancel rename mode (Esc).
    RenameCancel,

    // Window & Pane management
    /// Create a new window in the current session.
    WindowNew,
    /// Switch to the next window.
    WindowNext,
    /// Switch to the previous window.
    WindowPrevious,
    /// Switch to a window by index.
    WindowSwitch { index: u32 },
    /// Close the current window.
    WindowClose,
    /// Rename the current window (immediate or start).
    WindowRename { label: String },
    /// Confirm window rename.
    WindowRenameConfirm { label: String },
    /// Split the current pane (horizontal = top/bottom, vertical = left/right).
    SplitPane { horizontal: bool },
    /// Kill/close the current focused pane.
    KillPane,
}

/// Prefix-mode commands (tmux-style keybindings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrefixCommand {
    SplitHorizontal,
    SplitVertical,
    KillPane,
    FocusUp,
    FocusDown,
    FocusLeft,
    FocusRight,
    ShowHelp,
    Quit,
    // Window management
    WindowNew,
    WindowNext,
    WindowPrevious,
    WindowSwitch(u32),
    WindowClose,
    WindowRename,
}

// Daemon → Client

/// Messages sent from the daemon to the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Raw terminal bytes to write directly to the client's stdout.
    Draw(Vec<u8>),
    /// Tell the client to exit.
    ExitClient,
    /// Response to ListSessions command.
    SessionList(Vec<SessionInfo>),
    /// Acknowledgment that a command was processed.
    Ack,
    /// Response to list windows in a session.
    WindowList {
        windows: Vec<WindowInfo>,
        active: usize,
    },
}

/// Lightweight session info for listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub name: String,
    pub window_count: usize,
    pub pane_count: usize,
    pub attached: bool,
    pub created_at_secs: u64,
}

/// Lightweight window info for listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub index: u32,
    pub name: String,
    pub pane_count: usize,
}

// Message Type IDs

pub mod wire {
    // Client → Daemon
    pub const CLIENT_RESIZE: u8 = 0x01;
    pub const CLIENT_INPUT: u8 = 0x02;
    pub const CLIENT_ATTACH: u8 = 0x03;
    pub const CLIENT_DETACH: u8 = 0x04;
    pub const CLIENT_COMMAND: u8 = 0x05;
    pub const CLIENT_LIST_SESSIONS: u8 = 0x06;
    pub const CLIENT_KILL_SESSION: u8 = 0x07;
    pub const CLIENT_KILL_SERVER: u8 = 0x08;
    pub const CLIENT_SCROLL: u8 = 0x09;
    pub const CLIENT_PREFIX_TOGGLE: u8 = 0x0a;
    pub const CLIENT_HELP_KEY: u8 = 0x0b;
    pub const CLIENT_MOUSE_CLICK: u8 = 0x0c;
    pub const CLIENT_RENAME_START: u8 = 0x0d;
    pub const CLIENT_RENAME_CHAR: u8 = 0x0e;
    pub const CLIENT_RENAME_BACKSPACE: u8 = 0x0f;
    pub const CLIENT_RENAME_CONFIRM: u8 = 0x10;
    pub const CLIENT_RENAME_CANCEL: u8 = 0x11;
    pub const CLIENT_WINDOW_NEW: u8 = 0x12;
    pub const CLIENT_WINDOW_NEXT: u8 = 0x13;
    pub const CLIENT_WINDOW_PREV: u8 = 0x14;
    pub const CLIENT_WINDOW_SWITCH: u8 = 0x15;
    pub const CLIENT_WINDOW_CLOSE: u8 = 0x16;
    pub const CLIENT_WINDOW_RENAME: u8 = 0x17;
    pub const CLIENT_SPLIT_PANE: u8 = 0x18;
    pub const CLIENT_KILL_PANE: u8 = 0x19;
    pub const CLIENT_WINDOW_RENAME_CONFIRM: u8 = 0x1a;

    // Daemon → Client
    pub const SERVER_DRAW: u8 = 0x80;
    pub const SERVER_EXIT_CLIENT: u8 = 0x81;
    pub const SERVER_SESSION_LIST: u8 = 0x82;
    pub const SERVER_ACK: u8 = 0x83;
    pub const SERVER_WINDOW_LIST: u8 = 0x84;
}

impl ClientMessage {
    pub fn wire_type(&self) -> u8 {
        match self {
            ClientMessage::Resize { .. } => wire::CLIENT_RESIZE,
            ClientMessage::Input(_) => wire::CLIENT_INPUT,
            ClientMessage::Attach { .. } => wire::CLIENT_ATTACH,
            ClientMessage::Detach => wire::CLIENT_DETACH,
            ClientMessage::PrefixCommand(_) => wire::CLIENT_COMMAND,
            ClientMessage::ListSessions => wire::CLIENT_LIST_SESSIONS,
            ClientMessage::KillSession { .. } => wire::CLIENT_KILL_SESSION,
            ClientMessage::KillServer => wire::CLIENT_KILL_SERVER,
            ClientMessage::Scroll { .. } => wire::CLIENT_SCROLL,
            ClientMessage::PrefixToggle { .. } => wire::CLIENT_PREFIX_TOGGLE,
            ClientMessage::HelpKey(_) => wire::CLIENT_HELP_KEY,
            ClientMessage::MouseClick { .. } => wire::CLIENT_MOUSE_CLICK,
            ClientMessage::RenameStart { .. } => wire::CLIENT_RENAME_START,
            ClientMessage::RenameChar(_) => wire::CLIENT_RENAME_CHAR,
            ClientMessage::RenameBackspace => wire::CLIENT_RENAME_BACKSPACE,
            ClientMessage::RenameConfirm { .. } => wire::CLIENT_RENAME_CONFIRM,
            ClientMessage::RenameCancel => wire::CLIENT_RENAME_CANCEL,
            ClientMessage::WindowNew => wire::CLIENT_WINDOW_NEW,
            ClientMessage::WindowNext => wire::CLIENT_WINDOW_NEXT,
            ClientMessage::WindowPrevious => wire::CLIENT_WINDOW_PREV,
            ClientMessage::WindowSwitch { .. } => wire::CLIENT_WINDOW_SWITCH,
            ClientMessage::WindowClose => wire::CLIENT_WINDOW_CLOSE,
            ClientMessage::WindowRename { .. } => wire::CLIENT_WINDOW_RENAME,
            ClientMessage::WindowRenameConfirm { .. } => wire::CLIENT_WINDOW_RENAME_CONFIRM,
            ClientMessage::SplitPane { .. } => wire::CLIENT_SPLIT_PANE,
            ClientMessage::KillPane => wire::CLIENT_KILL_PANE,
        }
    }

    pub fn from_wire(type_id: u8, payload: &[u8]) -> Result<Self, bincode::Error> {
        match type_id {
            wire::CLIENT_RESIZE => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_INPUT => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_ATTACH => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_DETACH => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_COMMAND => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_PREFIX_TOGGLE => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_LIST_SESSIONS => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_KILL_SESSION => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_KILL_SERVER => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_SCROLL => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_HELP_KEY => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_MOUSE_CLICK => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_RENAME_START => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_RENAME_CHAR => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_RENAME_BACKSPACE => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_RENAME_CONFIRM => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_RENAME_CANCEL => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_WINDOW_NEW => Ok(ClientMessage::WindowNew),
            wire::CLIENT_WINDOW_NEXT => Ok(ClientMessage::WindowNext),
            wire::CLIENT_WINDOW_PREV => Ok(ClientMessage::WindowPrevious),
            wire::CLIENT_WINDOW_SWITCH => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_WINDOW_CLOSE => Ok(ClientMessage::WindowClose),
            wire::CLIENT_WINDOW_RENAME => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_WINDOW_RENAME_CONFIRM => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_SPLIT_PANE => Ok(bincode::deserialize(payload)?),
            wire::CLIENT_KILL_PANE => Ok(ClientMessage::KillPane),
            _ => Err(bincode::Error::new(bincode::ErrorKind::Custom(format!(
                "unknown client wire type: 0x{:02x}",
                type_id
            )))),
        }
    }
}

impl ServerMessage {
    pub fn wire_type(&self) -> u8 {
        match self {
            ServerMessage::Draw(_) => wire::SERVER_DRAW,
            ServerMessage::ExitClient => wire::SERVER_EXIT_CLIENT,
            ServerMessage::SessionList(_) => wire::SERVER_SESSION_LIST,
            ServerMessage::Ack => wire::SERVER_ACK,
            ServerMessage::WindowList { .. } => wire::SERVER_WINDOW_LIST,
        }
    }

    pub fn from_wire(type_id: u8, payload: &[u8]) -> Result<Self, bincode::Error> {
        match type_id {
            wire::SERVER_DRAW => Ok(bincode::deserialize(payload)?),
            wire::SERVER_EXIT_CLIENT => Ok(bincode::deserialize(payload)?),
            wire::SERVER_SESSION_LIST => Ok(bincode::deserialize(payload)?),
            wire::SERVER_ACK => Ok(bincode::deserialize(payload)?),
            wire::SERVER_WINDOW_LIST => Ok(bincode::deserialize(payload)?),
            _ => Err(bincode::Error::new(bincode::ErrorKind::Custom(format!(
                "unknown server wire type: 0x{:02x}",
                type_id
            )))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_message_roundtrip() {
        let messages = vec![
            ClientMessage::Resize { cols: 80, rows: 24 },
            ClientMessage::Input(vec![0x1b, 0x5b, 0x41]),
            ClientMessage::Attach {
                session_name: "test-session".to_string(),
                cols: 80,
                rows: 24,
            },
            ClientMessage::Detach,
            ClientMessage::PrefixCommand(PrefixCommand::SplitVertical),
            ClientMessage::PrefixCommand(PrefixCommand::KillPane),
            ClientMessage::ListSessions,
            ClientMessage::KillSession {
                session_name: "foo".to_string(),
            },
            ClientMessage::KillServer,
            ClientMessage::RenameStart {
                current_label: "bash".to_string(),
            },
            ClientMessage::RenameChar('h'),
            ClientMessage::RenameBackspace,
            ClientMessage::RenameConfirm {
                label: "server".to_string(),
            },
            ClientMessage::RenameCancel,
            ClientMessage::WindowNew,
            ClientMessage::WindowNext,
            ClientMessage::WindowPrevious,
            ClientMessage::WindowSwitch { index: 2 },
            ClientMessage::WindowClose,
            ClientMessage::WindowRename {
                label: "vim".to_string(),
            },
            ClientMessage::WindowRenameConfirm {
                label: "work".to_string(),
            },
            ClientMessage::SplitPane { horizontal: true },
            ClientMessage::KillPane,
        ];

        for msg in &messages {
            let type_id = msg.wire_type();
            let payload = bincode::serialize(msg).unwrap();
            let decoded = ClientMessage::from_wire(type_id, &payload).unwrap();
            assert_eq!(bincode::serialize(&decoded).unwrap(), payload);
        }
    }

    #[test]
    fn server_message_roundtrip() {
        let messages = vec![
            ServerMessage::Draw(vec![0x1b, 0x5b, 0x32, 0x4a]),
            ServerMessage::ExitClient,
            ServerMessage::SessionList(vec![SessionInfo {
                name: "test".to_string(),
                window_count: 1,
                pane_count: 2,
                attached: true,
                created_at_secs: 1234567890,
            }]),
            ServerMessage::Ack,
            ServerMessage::WindowList {
                windows: vec![WindowInfo {
                    index: 0,
                    name: "bash".to_string(),
                    pane_count: 1,
                }],
                active: 0,
            },
        ];

        for msg in &messages {
            let type_id = msg.wire_type();
            let payload = bincode::serialize(msg).unwrap();
            let decoded = ServerMessage::from_wire(type_id, &payload).unwrap();
            assert_eq!(bincode::serialize(&decoded).unwrap(), payload);
        }
    }

    #[test]
    fn wire_type_ids_dont_overlap() {
        const {
            assert!(wire::CLIENT_RESIZE < 0x80);
            assert!(wire::CLIENT_WINDOW_RENAME < 0x80);
            assert!(wire::SERVER_DRAW >= 0x80);
            assert!(wire::SERVER_WINDOW_LIST >= 0x80);
        }
    }
}
