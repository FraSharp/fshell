# fsh-tmux

A terminal multiplexer written in Rust with a daemon/client architecture. Runs your shell in tiled panes with async I/O, full VT100/xterm escape sequence support, and a binary space partitioning (BSP) layout system. Supports tmux-style windows (tabs) for organizing multiple workspaces within a session.

## Features

- **Daemon/Client Architecture** — Persistent background daemon manages sessions; lightweight client is a dumb pipe
- **Multi-Window Sessions** — Each session owns multiple independent windows, each with its own panes and layout
- **Session Management** — Sessions survive client detach/reattach; PTY actors keep running in background
- **BSP Layout** — Binary space partitioning tree for automatic, conflict-free pane arrangement
- **Pane Splitting** — Horizontal and vertical splits via tmux-style prefix commands
- **Mouse Focus** — Click any pane to focus it
- **Async PTY I/O** — Non-blocking terminal emulation via `tokio` + `portable-pty` + `AsyncFd`
- **Full VT100/xterm Support** — VTE-powered parser with comprehensive CSI/SGR escape sequence handling
- **Scrollback Buffer** — Circular scrollback history with viewport navigation
- **Dynamic Resize** — Real-time terminal resize propagation to all panes and PTYs
- **Wide Character Support** — Proper handling of CJK and emoji double-width characters
- **Alternate Buffer** — Full alt-screen support for programs like `vim`, `less`, `htop`
- **60 FPS Rendering** — Daemon renders via ratatui with frame diffing; client forwards ANSI bytes
- **Status Bar** — Bottom bar showing window list, session name, pane count, clock, and prefix mode indicator
- **Pane & Window Titles** — Auto-detects shell name for each pane; user-settable labels for panes and windows

## Installation

```bash
git clone <repo-url>
cd fsh-tmux
cargo build --release
```

Binaries will be at `target/release/fsh-tmux` and `target/release/fsh-tmuxd`.

## Usage

```bash
# Start the daemon (background process)
fsh-tmuxd &

# Connect a client
fsh-tmux

# List sessions
fsh-tmux list-sessions

# Kill a session
fsh-tmux kill-session <name>

# Kill the daemon
fsh-tmux kill-server
```

On first connect, the daemon creates a session named `default` (or a name you specify). The client enters raw mode, enables mouse capture, and forwards all ANSI output from the daemon to stdout.

## Keybindings

`fsh-tmux` uses a tmux-style prefix key model. Press **Ctrl-A** to enter prefix mode, then press the command key.

### Pane Management

| Key          | Action                    |
|--------------|---------------------------|
| `Ctrl-A "`   | Split pane horizontally   |
| `Ctrl-A %`   | Split pane vertically     |
| `Ctrl-A x`   | Close current pane        |
| `Ctrl-A ,`   | Rename pane               |

### Window Management

| Key          | Action                    |
|--------------|---------------------------|
| `Ctrl-A c`   | Create new window         |
| `Ctrl-A n`   | Next window               |
| `Ctrl-A p`   | Previous window           |
| `Ctrl-A 0-9` | Switch to window N        |
| `Ctrl-A &`   | Close current window      |
| `Ctrl-A W`   | Rename window             |

### Navigation

| Key              | Action                    |
|------------------|---------------------------|
| `Ctrl-A ↑↓←→`   | Focus adjacent pane       |
| `Click`          | Focus the clicked pane    |

### Scrolling

| Key              | Action                    |
|------------------|---------------------------|
| `Shift+Up/Down`  | Scroll 3 lines            |
| `PageUp/PageDown`| Scroll 1 page             |
| `Mouse Wheel`    | Scroll 1 line             |

### Mode

| Key          | Action                    |
|--------------|---------------------------|
| `Ctrl-A`     | Enter prefix mode         |
| `Esc`        | Exit prefix mode          |
| `Ctrl-A ?`   | Show help overlay         |

### General

| Key          | Action                    |
|--------------|---------------------------|
| `Ctrl-A q`   | Quit fsh-tmux             |

All other keys are passed directly to the focused pane's PTY.

## Architecture

```
┌──────────────┐         ┌──────────────────────────────────────────────┐
│   fsh-tmux   │  Unix   │                  fsh-tmuxd                   │
│   (client)   │ socket  │                 (daemon)                     │
│              │◄───────►│  ┌─────────────┐  ┌──────────────────────┐  │
│  stdin ──►   │         │  │ ClientHandler│  │    SessionManager    │  │
│  stdout ◄──  │  ANSI   │  │  (per conn)  │  │  ┌───────────────┐  │  │
│              │  bytes  │  └──────┬───────┘  │  │    Session     │  │  │
└──────────────┘         │         │          │  │  ┌───────────┐ │  │  │
                         │         ▼          │  │  │  Window 0 │ │  │  │
                         │  ┌──────────────┐  │  │  │  Window 1 │ │  │  │
                         │  │   Renderer   │  │  │  │  ...      │ │  │  │
                         │  │  (ratatui)   │  │  │  ├───────────┤ │  │  │
                         │  └──────────────┘  │  │  │BSP Layout │ │  │  │
                         │                    │  │  │PtyActor 0 │ │  │  │
                         │  ┌──────────────┐  │  │  │PtyActor 1 │ │  │  │
                         │  │  DaemonEvent  │  │  │  └───────────┘ │  │  │
                         │  │    Loop       │  │  └───────────────┘  │  │
                         │  └──────────────┘  └──────────────────────┘  │
                         └──────────────────────────────────────────────┘
```

### Protocol

Binary IPC over Unix domain sockets (`/run/user/$UID/fsh-tmux.sock` or `/tmp/fsh-tmux-$UID.sock`).

- **Wire format**: 5-byte header `[u32 BE length][u8 type][payload]`
- **Serialization**: bincode
- **Message types**: Resize, Input, Attach, Detach, PrefixCommand, PrefixToggle, Scroll, MouseClick, HelpKey, RenamePane, RenameWindow, WindowNew, WindowNext, WindowPrevious, WindowSwitch, WindowClose, ListSessions, KillSession, KillServer
- **Server responses**: Draw (raw ANSI bytes), ExitClient, SessionList, WindowList, Ack

### Client

The client is a dumb pipe — it does zero rendering. It:
1. Connects to the daemon via Unix socket
2. Parses keyboard/mouse events via crossterm
3. Forwards structured messages to the daemon
4. Writes raw ANSI bytes from the daemon to stdout

### Daemon

The daemon is the persistent background process. It:
1. Manages sessions and windows (create, attach, detach, kill)
2. Runs PtyActor + GridManager for each pane
3. Renders terminal state via ratatui at 60 FPS
4. Sends pre-rendered ANSI frames to attached clients

### Window Model

Each session contains an ordered list of windows. Each window has its own BSP layout, focus state, and panes. Windows run independently — background windows continue accumulating output in their grids even when not visible. Switching windows is instant; no PTY actors are spawned or killed.

### BSP Layout

Panes within a window are arranged in a binary tree. Each internal node defines a split direction and ratio.

```
┌─────────┬─────────┐
│    0    │    1    │  ← horizontal split
├─────────┼────┬────┤
│         │  2 │ 3  │  ← pane 1 split vertically
└─────────┴────┴────┘
```

Clicking a pane focuses it. Prefix arrows navigate the BSP tree.

## Testing

```bash
cargo test
```

## Dependencies

| Crate           | Purpose                                      |
|-----------------|----------------------------------------------|
| `ratatui`       | TUI framework and rendering                  |
| `crossterm`     | Terminal raw mode, events, cursor             |
| `tokio`         | Async runtime, channels, I/O                 |
| `tokio-util`    | Codec framing for binary protocol            |
| `portable-pty`  | Cross-platform PTY management                |
| `vte`           | VT100/xterm escape sequence parser           |
| `serde`         | Serialization framework                      |
| `bincode`       | Binary serialization for IPC                 |
| `clap`          | CLI argument parsing                         |
| `unicode-width` | Character width calculation (CJK/emoji)      |

## License

TODO
