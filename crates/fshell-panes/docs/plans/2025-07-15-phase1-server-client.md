# Phase 1: Server/Client Architecture + Sessions

## Current Architecture (What We Have)

```
┌─────────────────────────────────────────────────────────┐
│                      main.rs                            │
│  - crossterm raw mode + EventStream                     │
│  - ratatui Terminal                                     │
│  - tokio::select! event loop (~60 FPS)                  │
│  - handle_event() → App::handle_action()                │
└─────────────────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────┐
│                   App (app/mod.rs)                      │
│  - HashMap<u32, PaneState>  (id → grid + pty_tx)        │
│  - BspLayout                                           │
│  - FocusController                                     │
│  - spawn_initial_pane(), spawn_pane(), close_pane()     │
└─────────────────────────────────────────────────────────┘
                          │
          ┌───────────────┴───────────────┐
          ▼                               ▼
┌─────────────────────┐     ┌─────────────────────────────┐
│   PtyActor (per pane)│     │   GridManager (per pane)    │
│   - AsyncPty fd      │     │   - VTE Parser              │
│   - mpsc commands    │     │   - Arc<RwLock<Grid>>       │
│   - read → GridMgr   │     │   - write lock during parse │
└─────────────────────┘     └─────────────────────────────┘
```

**Key limitation:** Everything runs in one process. No detach, no sessions, no persistence.

---

## Target Architecture (Phase 1)

```
┌─────────────────────────────────────────────────────────────────────┐
│                         CLIENT PROCESS (fsh-tmux)                   │
│  - Minimal: raw mode + stdin/stdout forwarding                      │
│  - Unix socket connection to daemon                                 │
│  - Sends: Resize, Input, Attach(session_name), Detach, Commands    │
│  - Receives: Raw ANSI bytes (direct write to stdout)                │
│  - NO ratatui — client is a dumb pipe                               │
└─────────────────────────────────────────────────────────────────────┘
                                    │
                          Unix Domain Socket
                          (binary protocol)
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────┐
│                         DAEMON PROCESS (fsh-tmuxd)                  │
│  - Persistent tokio runtime (survives detach)                       │
│  - SessionManager (HashMap<String, Session>)                        │
│  - Each Session holds:                                              │
│      - HashMap<u32, PaneState> (panes)                              │
│      - BspLayout                                                    │
│      - Session metadata (name, created_at, attached_client)         │
│  - PtyActors + GridManagers (one per pane, daemon-owned)            │
│  - Ratatui rendering (daemon renders full frame → ANSI bytes)       │
│  - Client state machine: Detached → Attached → Detached             │
│  - PTY backpressure: parse even when detached (no socket writes)    │
└─────────────────────────────────────────────────────────────────────┘
```

**Key insight:** The daemon owns rendering. Client receives raw ANSI bytes and writes them directly to stdout. This keeps client latency minimal and eliminates client-side rendering overhead.

---

## Implementation Plan

### Step 1: Binary Protocol (`src/proto/mod.rs`)

Create the framed binary protocol using `tokio-util` codec.

**Dependencies to add:**
```toml
tokio-util = { version = "0.7", features = ["codec"] }
bytes = "1"
serde = { version = "1", features = ["derive"] }
bincode = "1"
clap = { version = "4", features = ["derive"] }
```

**Files to create:**
- `src/proto/mod.rs` — Module root + `get_socket_path()` utility
- `src/proto/codec.rs` — `FshCodec` (Decoder/Encoder impl)
- `src/proto/message.rs` — `ClientMessage`, `ServerMessage`, `AdminCommand` enums

**Protocol spec:**
```
Header: [length: u32 BE] [type: u8] [payload: bytes]
```

**Message types:**
```rust
// Client → Daemon
pub enum ClientMessage {
    Resize { cols: u16, rows: u16 },
    Input(Vec<u8>),
    Attach { session_name: String },
    Detach,
    Command(AdminCommand),
}

// Daemon → Client
pub enum ServerMessage {
    Draw(Vec<u8>),       // Raw ANSI bytes to write to stdout
    ExitClient,
}

pub enum AdminCommand {
    SplitPane { vertical: bool },
    KillPane,
    KillSession,
}
```

**Socket path resolution:**
```rust
pub fn get_socket_path() -> PathBuf {
    // 1. Try XDG_RUNTIME_DIR (tmpfs, guaranteed local)
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime_dir).join("fsh-tmux.sock");
        if path.parent().map(|p| p.exists()).unwrap_or(false) {
            return path;
        }
    }
    // 2. Fall back to /tmp with UID
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/fsh-tmux-{}.sock", uid))
}
```

---

### Step 2: Daemon Core (`src/daemon/mod.rs`)

Extract the current `App` + PTY management into a daemon process.

**Files to create:**
- `src/daemon/mod.rs` — Daemon entry point + event loop
- `src/daemon/session.rs` — `Session` struct
- `src/daemon/session_manager.rs` — `SessionManager` (create/list/attach/detach)
- `src/daemon/client_handler.rs` — Per-client connection handler
- `src/daemon/renderer.rs` — Ratatui rendering (moved from `app/render.rs`)

**Daemon responsibilities:**
1. Listen on Unix socket
2. Accept client connections
3. Manage sessions (create, list, attach, detach)
4. Run PtyActors + GridManagers
5. Render full terminal frame → raw ANSI → send to client
6. Handle PTY backpressure when detached

**Session struct:**
```rust
pub struct Session {
    pub name: String,
    pub created_at: Instant,
    pub panes: HashMap<u32, PaneState>,
    pub bsp: BspLayout,
    pub focus: FocusController,
    pub attached_client: Option<mpsc::Sender<ServerMessage>>,
}
```

**Rendering flow (daemon-side):**
```
Grid state → ratatui render → crossterm backend → ANSI bytes → ServerMessage::Draw → client socket
```

The daemon uses a `CrosstermBackend<Vec<u8>>` to render into a memory buffer, then sends the raw ANSI bytes to the client. This is the same approach tmux uses internally.

---

### Step 3: Client Process (`src/client/mod.rs`)

Minimal client — a dumb pipe between terminal and daemon.

**Files to create:**
- `src/client/mod.rs` — Client entry point + event loop

**Client responsibilities:**
1. Connect to daemon socket
2. Send `Attach { session_name }` (or auto-create)
3. Enter raw mode
4. Forward crossterm events → `ClientMessage::Input` / `ClientMessage::Resize`
5. Receive `ServerMessage::Draw` → write directly to stdout
6. Handle `ServerMessage::ExitClient` → cleanup and exit

**Client event loop:**
```rust
loop {
    tokio::select! {
        // Stdin → Daemon
        event = event_stream.next() => {
            match event {
                Some(Ok(Key(k))) => send(ClientMessage::Input(k)),
                Some(Ok(Resize(c, r))) => send(ClientMessage::Resize { cols: c, rows: r }),
                _ => {}
            }
        }
        // Daemon → Stdout
        msg = client_rx.recv() => {
            match msg {
                ServerMessage::Draw(bytes) => stdout.write_all(&bytes),
                ServerMessage::ExitClient => break,
            }
        }
    }
}
```

---

### Step 4: CLI Interface (`src/bin/`)

Two binaries via `[[bin]]` in Cargo.toml:

**`src/bin/fsh-tmux.rs`** — Client (default entry point)
**`src/bin/fsh-tmuxd.rs`** — Daemon

**CLI commands:**
```bash
fsh-tmux                    # Attach to most recent session, or create new
fsh-tmux new [name]         # Create new session with name (or auto-generate)
fsh-tmux attach <name>      # Attach to existing session
fsh-tmux ls                 # List sessions (queries daemon)
fsh-tmux kill-session <name># Kill a session
fsh-tmux kill-server        # Kill daemon (saves sessions first)
```

**Session naming (auto-generate):**
```rust
fn generate_session_name() -> String {
    let adjectives = ["focused", "rusty", "swift", "calm", "bright", ...];
    let nouns = ["ferret", "eagle", "tiger", "falcon", "wolf", ...];
    format!("{}-{}", adjectives[rand()], nouns[rand()])
}
```

---

### Step 5: Detach/Reattach Flow

**Detach flow:**
1. Client sends `Detach` (via `Ctrl-A d`)
2. Daemon sets `session.attached_client = None`
3. Daemon keeps PtyActors running (PTY backpressure active)
4. Client exits cleanly

**Reattach flow:**
1. New client connects
2. Sends `Attach { session_name }`
3. Daemon sets `session.attached_client = Some(client_tx)`
4. Daemon renders current state → sends full ANSI repaint
5. Client enters event loop

**PTY backpressure (detached state):**
- PtyActor continues reading from PTY
- GridManager continues parsing and updating Grid
- No bytes written to socket (no client attached)
- Scrollback buffer has hard limit (1000 lines default)
- On reattach, client gets full viewport redraw

---

### Step 6: Session Persistence (`src/daemon/persistence.rs`)

Save/restore session state to disk using KDL format.

**Files to create:**
- `src/daemon/persistence.rs` — KDL serialization/deserialization

**Persisted state:**
- Session name
- BSP layout tree (KDL)
- Pane commands (for restart after daemon restart)
- Pane focus state

**Format (KDL):**
```kdl
session name="focused-ferret" {
    layout {
        node direction="vertical" ratio="0.5" {
            pane id="0" command="/bin/bash"
            node direction="horizontal" ratio="0.6" {
                pane id="1" command="nvim"
                pane id="2" command="htop"
            }
        }
    }
    focus pane_id="1"
}
```

**Persistence triggers:**
- On `kill-server` (SIGTERM/SIGINT)
- On `kill-session`
- Periodic autosave (every 60 seconds)

---

## Signal Handling

| Signal | Action |
|--------|--------|
| `SIGTERM` | Graceful shutdown: save sessions, send `ExitClient` to all clients, kill PTYs, delete socket |
| `SIGINT` | Same as SIGTERM |
| `SIGHUP` | **Ignore** — daemon survives terminal close |

**Graceful shutdown sequence:**
1. Save all sessions to disk (KDL)
2. Send `ExitClient` to all connected clients
3. Send `PtyCommand::Shutdown` to all PtyActors
4. Wait for actors to exit (with timeout)
5. Delete socket file
6. Exit

---

## Files Changed/Created

### New files:
```
src/
├── proto/
│   ├── mod.rs              # Module root + get_socket_path()
│   ├── codec.rs            # FshCodec (Decoder/Encoder)
│   └── message.rs          # ClientMessage, ServerMessage enums
├── daemon/
│   ├── mod.rs              # Daemon entry point + event loop
│   ├── session.rs          # Session struct
│   ├── session_manager.rs  # Session lifecycle management
│   ├── client_handler.rs   # Per-client connection handler
│   ├── renderer.rs         # Ratatui rendering (moved from app/render.rs)
│   └── persistence.rs      # KDL save/load
├── client/
│   └── mod.rs              # Client entry point (dumb pipe)
src/bin/
├── fsh-tmux.rs             # Client binary
└── fsh-tmuxd.rs            # Daemon binary
```

### Modified files:
```
Cargo.toml                  # Add tokio-util, bytes, serde, bincode, clap
src/lib.rs                  # Add proto, daemon, client modules
```

### Unchanged (reused as-is):
```
src/grid/                   # Terminal grid, VTE parser, widget
src/layout/bsp.rs           # BSP layout
src/pty/async_pty.rs        # Async PTY wrapper
src/pty/pty_actor.rs        # PTY actor
src/pty/grid_manager.rs     # Grid manager
src/app/actions.rs          # Key resolution
src/app/focus.rs            # Focus controller
src/app/signals.rs          # Terminal size utilities
src/app/help.rs             # Help overlay
```

---

## Implementation Order

| Order | Task | Estimated LOC | Notes |
|-------|------|---------------|-------|
| 1 | Proto module (codec + messages + socket path) | ~200 | Foundation for everything |
| 2 | Daemon core (session + manager + renderer) | ~450 | Extract from App, move rendering |
| 3 | Client process (dumb pipe) | ~200 | Minimal — stdin/stdout forwarding |
| 4 | CLI interface (clap) | ~150 | Two binaries |
| 5 | Detach/reattach flow | ~200 | PTY backpressure logic |
| 6 | Session persistence (KDL) | ~250 | Save/restore sessions |
| 7 | Signal handling | ~100 | SIGTERM/SIGINT/SIGHUP |
| **Total** | | **~1550** | |

---

## Verification Checklist

- [ ] Daemon starts and listens on Unix socket
- [ ] Client connects and can attach to session
- [ ] PTY I/O works (shell is usable)
- [ ] Detach works (`Ctrl-A d`)
- [ ] Reattach works (session state preserved)
- [ ] Multiple panes work after reattach
- [ ] Scrollback preserved across detach/reattach
- [ ] `fsh-tmux ls` shows active sessions
- [ ] `fsh-tmux kill-session` works
- [ ] `fsh-tmux kill-server` saves sessions and exits cleanly
- [ ] Daemon survives SIGHUP (terminal close)
- [ ] No panics on daemon restart (client exits gracefully)
- [ ] PTY backpressure works (no memory leak when detached)
- [ ] Auto-generated session names are mnemonic
- [ ] Session persistence works (KDL format)

---

## Resolved Open Questions

1. **Socket location:** `XDG_RUNTIME_DIR/fsh-tmux.sock` (tmpfs) with fallback to `/tmp/fsh-tmux-$UID.sock`
2. **Session naming:** Auto-generate mnemonic names (adjective-noun), allow explicit override
3. **Default session:** Attach to most recent if exists, otherwise create new
4. **Signal handling:** SIGTERM/SIGINT → graceful shutdown, SIGHUP → ignore
5. **Rendering model:** Daemon renders full frame → raw ANSI → client pipe (no client-side ratatui)
