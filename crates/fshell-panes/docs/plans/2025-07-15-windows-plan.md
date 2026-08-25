# Multi-Window Support — Implementation Plan

**Goal:** Add tmux-style windows to fsh-tmux sessions. Each session owns multiple independent windows, each with its own panes, BSP layout, and focus.

**Key binding changes:**
| Key | Old | New |
|-----|-----|-----|
| `Ctrl-A c` | SplitHorizontal | WindowNew |
| `Ctrl-A n` | FocusDown | WindowNext |
| `Ctrl-A p` | FocusUp | WindowPrevious |
| `Ctrl-A 0-9` | (passthrough) | WindowSwitch(N) |
| `Ctrl-A &` | (passthrough) | WindowClose |
| `Ctrl-A W` | (passthrough) | WindowRename |

`Ctrl-A "` (split horizontal), `Ctrl-A %` (split vertical), arrows (focus) unchanged.

**Data flow:** `client/mod.rs` key → `ClientMessage` → `daemon/client_handler.rs` handles state + spawns PTY actors → renderer reads active window → ANSI to client.

---

## Task 1: Create `src/daemon/window.rs`

New file. Contains `PaneState` (moved from session.rs) and `Window` struct.

```rust
pub struct PaneState {
    pub grid: Arc<RwLock<Grid>>,
    pub pty_tx: mpsc::Sender<PtyCommand>,
    pub label: Option<String>,
    pub shell_name: String,
}

pub struct Window {
    pub id: u32,
    pub name: String,
    pub panes: HashMap<u32, PaneState>,
    pub bsp: BspLayout,
    pub focus: FocusController,
    pub show_help: bool,
    pub help_scroll: u16,
    pub rename_buffer: Option<String>,
}
```

Methods: `new(id, initial_pane_id)`, `add_pane(...)`, `remove_pane(id)`, `is_empty()`, `resize_panes(area)`, `compute_layout(area)`, `spawn_pane(direction, session_name, event_tx)`, `close_focused_pane()`, `resize_to_terminal(cols, rows)`.

`spawn_pane` does the full PTY pipeline: split BSP → compute rect → spawn GridManager → spawn AsyncPty+PtyActor → register pane. Extracted from current `Session::spawn_pane`.

Add `pub mod window;` to `src/daemon/mod.rs`.

---

## Task 2: Rewrite `src/daemon/session.rs`

Remove `PaneState` (moved to window.rs). Session becomes:

```rust
pub struct Session {
    pub name: String,
    pub created_at: Instant,
    pub windows: Vec<Window>,
    pub active_window: usize,
    pub next_pane_id: u32,
    pub next_window_id: u32,
    pub attached_client: Option<mpsc::Sender<Vec<u8>>>,
    pub terminal_size: (u16, u16),
    daemon_event_tx: Option<mpsc::Sender<DaemonEvent>>,
}
```

Key methods:
- `active_window()`, `active_window_mut()` — index into vec
- `new_window()` — allocates IDs, spawns PTY, appends to vec, sets active
- `next_window()` / `previous_window()` — wraps around
- `switch_window(index)` — no-op if out of bounds
- `close_active_window()` — shutdown PTYs, remove from vec, adjust index, return `is_empty`
- `spawn_pane(direction)` — delegates to `active_window_mut().spawn_pane(...)`
- `close_pane()` — delegates to `active_window_mut().close_focused_pane()`, closes window if empty
- `remove_pane(id)` — finds pane in any window, removes it
- `resize_all(cols, rows)` — updates `terminal_size`, resizes all windows
- `info()` — returns `SessionInfo` with `window_count` and `pane_count`

Tests: window creation, next/prev wrapping, switch out-of-bounds, close shifts index, close last = empty, remove_pane across windows.

---

## Task 3: Update `src/proto/message.rs`

**SessionInfo** — add `window_count: usize` field.

**New struct:**
```rust
pub struct WindowInfo {
    pub index: u32,
    pub name: String,
    pub pane_count: usize,
}
```

**ClientMessage** — add variants:
```rust
WindowNew, WindowNext, WindowPrevious,
WindowSwitch { index: u32 }, WindowClose, WindowRename { label: String },
```

**PrefixCommand** — add:
```rust
WindowNew, WindowNext, WindowPrevious,
WindowSwitch(u32), WindowClose, WindowRename,
```

**Wire type IDs:** `0x12`–`0x17` for client, `0x84` for server `WindowList`.

**ServerMessage** — add `WindowList { windows: Vec<WindowInfo>, active: usize }`.

Update `wire_type()`, `from_wire()`, and roundtrip tests for all new variants.

---

## Task 4: Update `src/client/mod.rs`

In the prefix key match block, add:

```rust
KeyCode::Char('c') => {
    let _ = cmd_tx_clone2.send(ClientMessage::PrefixCommand(
        PrefixCommand::WindowNew,
    )).await;
    continue;
}
KeyCode::Char('n') => { /* PrefixCommand::WindowNext */ continue; }
KeyCode::Char('p') => { /* PrefixCommand::WindowPrevious */ continue; }
KeyCode::Char('0')..=KeyCode::Char('9') => {
    if let KeyCode::Char(c) = code {
        let _ = cmd_tx_clone2.send(ClientMessage::PrefixCommand(
            PrefixCommand::WindowSwitch((c as u32) - ('0' as u32),
        )).await;
    }
    continue;
}
KeyCode::Char('&') => { /* PrefixCommand::WindowClose */ continue; }
KeyCode::Char('W') => {
    rename_active = true;
    rename_buffer.clear();
    let _ = cmd_tx_clone2.send(ClientMessage::WindowRename { label: String::new() }).await;
    let _ = cmd_tx_clone2.send(ClientMessage::PrefixToggle { active: false }).await;
    continue;
}
```

Remove `'c'` from the existing `SplitHorizontal` arm (`'"') | KeyCode::Char('c') | KeyCode::Char('-')` → `'"') | KeyCode::Char('-')`).

---

## Task 5: Update `src/daemon/client_handler.rs`

**PrefixCommand match** — add window arms:

```rust
PrefixCommand::WindowNew => {
    let mut s = sess.write().await;
    s.new_window();
}
PrefixCommand::WindowNext => { sess.write().await.next_window(); }
PrefixCommand::WindowPrevious => { sess.write().await.previous_window(); }
PrefixCommand::WindowSwitch(i) => { sess.write().await.switch_window(i as usize); }
PrefixCommand::WindowClose => {
    let mut s = sess.write().await;
    if s.close_active_window() {
        let n = session_name.clone().unwrap_or_default();
        drop(s);
        session_manager.write().await.kill_session(&n).await.ok();
    }
}
```

**ClientMessage::WindowRename** — new arm:
```rust
ClientMessage::WindowRename { label } => {
    if let Some(w) = s.windows.get_mut(s.active_window) {
        if !label.is_empty() { w.name = label; }
    }
}
```

**Scope all pane operations to active window.** Every place that reads `s.panes` or `s.focus` now reads through `s.windows[s.active_window]`:
- `Input` handler: `window.panes.get(window.focus.focused_pane)`
- `Scroll` handler: same pattern
- `MouseClick` handler: compute layout from `window.bsp`, map click to pane
- `PrefixToggle`: sets `window.focus.prefix_active`
- `ShowHelp`: toggles `window.show_help`
- `HelpKey`: sets `window.show_help = false`

**Render tick** — change `r.render_frame(&s.bsp, &s.panes, ...)` to:
```rust
if let Some(window) = s.windows.get(s.active_window) {
    r.render_frame(&s, window, cols, rows)
}
```

**SplitHorizontal/SplitVertical/KillPane** — replace DaemonEvent sends with direct calls:
```rust
PrefixCommand::SplitHorizontal => {
    sess.write().await.spawn_pane(Split::Horizontal);
}
PrefixCommand::SplitVertical => {
    sess.write().await.spawn_pane(Split::Vertical);
}
PrefixCommand::KillPane => {
    sess.write().await.close_pane();
}
```

**Remove** `DaemonEvent::SplitPane` and `DaemonEvent::KillPane` variants from `src/daemon/mod.rs` and their handlers in the daemon loop. Keep `PaneExited` and `WindowNew`/`WindowClose` (though WindowNew/WindowClose can also be handled inline since Session methods do the PTY spawning).

---

## Task 6: Update `src/daemon/renderer.rs`

Change `render_frame` signature:
```rust
pub fn render_frame(
    &mut self,
    session: &Session,
    active_window: &Window,
    cols: u16, rows: u16,
) -> io::Result<Vec<u8>>
```

Replace all field access: `bsp` → `active_window.bsp`, `panes` → `active_window.panes`, `focus` → `active_window.focus`, `show_help` → `active_window.show_help`, `help_scroll` → `active_window.help_scroll`, `rename_buffer` → `active_window.rename_buffer`, `session_name` → `session.name`.

Status bar call passes session + window data.

Update `help_entries()` to add Windows section, remove n/p from Navigation.

Update test helpers to create `Session` + `Window`.

---

## Task 7: Rewrite `src/app/statusbar.rs` + `src/app/theme.rs`

**theme.rs** — add:
```rust
pub fn statusbar_window_active() -> Style { ... }      // bold accent
pub fn statusbar_window_inactive() -> Style { ... }     // dim
pub fn statusbar_window_active_prefix() -> Style { ... }
pub fn statusbar_window_inactive_prefix() -> Style { ... }
```

**statusbar.rs** — new signature:
```rust
pub fn render_status_bar(
    frame: &mut Frame, area: Rect,
    session_name: &str,
    windows: &[Window], active_window_idx: usize,
    prefix_active: bool,
    panes: &HashMap<u32, PaneState>, focus: &FocusController,
)
```

Implement `compute_visible_windows(windows, active_idx, available_width) -> Vec<VisibleTab>`:
- Always show active window tab
- Greedy outward expansion alternating left/right
- Use `UnicodeWidthStr` for width calculation
- Format: ` {index}:{name}{*|-} `

Build spans: window tabs → `│` → session name → `│` → pane info → `│` → pane count → `│` → clock → fill.

---

## Task 8: Update `src/app/help.rs`

Add Windows section before Help:
```
Ctrl-A c      New window
Ctrl-A n      Next window
Ctrl-A p      Previous window
Ctrl-A 0-9    Switch to window N
Ctrl-A &      Close window
Ctrl-A W      Rename window
```

Remove `Ctrl-A n`/`Ctrl-A p` from Navigation (they're window commands now).

---

## Task 9: Verify

```bash
cargo check 2>&1        # fix remaining errors
cargo test 2>&1          # fix failing tests
cargo build --release    # confirm release builds
```

Smoke test:
```
fsh-tmuxd &
fsh-tmux
Ctrl-A c     → new window (tab appears)
Ctrl-A n/p   → cycle windows
Ctrl-A 1     → switch to window 1
Ctrl-A W vim → rename
Ctrl-A "     → split (works in any window)
Ctrl-A &     → close window
Ctrl-A ?     → help shows window commands
```

Update README keybindings table and architecture description.
