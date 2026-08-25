# Phase 3: TUI, Layout Manager & Event System

## Gemini Feedback Applied

1. **crossterm** — confirmed. EventStream + tokio::select! is the correct async pattern. termion rejected.
2. **BSP ratios** — configurable `f32`, default 0.5 (50/50). Future pane-resize keystrokes trivial.
3. **Pane borders** — 1-char thin Unicode (`│─┌┐└┘`), active pane gets green border, unfocused dim gray.
4. **Event loop** — replaced blocking `poll()` with `EventStream` + `tokio::select!` + render tick. No deadlocks.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      HOST TERMINAL                              │
│                  (raw mode via crossterm)                        │
└──────────────────────────┬──────────────────────────────────────┘
                           │ EventStream (async, non-blocking)
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│                    GLOBAL EVENT LOOP (tokio::select!)           │
│                                                                 │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐  │
│  │ Key Events   │  │ Resize Event │  │ PTY Output            │  │
│  │ (keyboard)   │  │ (SIGWINCH)   │  │ (Arc<RwLock<Grid>>)   │  │
│  └──────┬───────┘  └──────┬───────┘  └──────────┬───────────┘  │
│         │                 │                      │               │
│         ▼                 ▼                      ▼               │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐  │
│  │Action Router │  │ Resize       │  │ GridManager           │  │
│  │ (prefix key, │  │ Dispatcher   │  │ (locks grid ~µs)      │  │
│  │  hotkeys)    │  │              │  │                      │  │
│  └──────┬───────┘  └──────┬───────┘  └──────────┬───────────┘  │
│         │                 │                      │               │
│         ▼                 ▼                      ▼               │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │                   BSP LAYOUT TREE                        │    │
│  │  - Recursive vertical/horizontal splits                  │    │
│  │  - Configurable ratio (f32, default 0.5)                 │    │
│  │  - compute_layout(area) → Vec<(PaneId, Rect)>           │    │
│  │  - Focus tracking (active pane)                          │    │
│  └──────────────────────────┬──────────────────────────────┘    │
│                             │                                    │
│                             ▼                                    │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │                    RENDER PASS (~60fps)                   │    │
│  │  for each (pane_id, rect) in layout:                     │    │
│  │    border_style = if focused { GREEN } else { DIM_GRAY } │    │
│  │    frame.render_widget(Block::thin_border().style(...))  │    │
│  │    GridWidget::new(&grid).render(inner_rect, buf)         │    │
│  └─────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

## Pre-Phase 3: Zero-Allocation Viewport (Prerequisite)

Before building the TUI, fix the heap allocation in the render hot path.

### Viewport Iterator

Replace `Grid::viewport() -> Vec<&Row>` with `Grid::viewport_iter() -> Viewport<'_>`:

```rust
pub struct Viewport<'a> {
    iter: std::collections::vec_deque::Iter<'a, Row>,
}

impl<'a> Iterator for Viewport<'a> {
    type Item = &'a Row;
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

impl ExactSizeIterator for Viewport<'_> {
    fn len(&self) -> usize {
        self.iter.len()
    }
}
```

Keep the old `viewport()` for tests (returns `Vec<&Row>`), add `viewport_iter()` for the widget hot path.

---

## Dependencies

```toml
crossterm = { version = "0.29", features = ["event-stream"] }
tokio-stream = "0.1"          # StreamExt for EventStream
```

Note: `tokio-stream` provides `StreamExt` that works natively with tokio's `select!`. We already depend on tokio.

---

## Tasks

### Task 0: Zero-Allocation Viewport Iterator

**Files**: `src/grid/scrollback.rs`, `src/grid/widget.rs`
**Tests**: `tests/grid_test.rs`

**What**:
- Add `Viewport<'a>` struct implementing `Iterator<Item = &'a Row>` + `ExactSizeIterator`
- Add `Grid::viewport_iter() -> Viewport<'_>` — zero-allocation
- Keep existing `viewport() -> Vec<&Row>` for backward compatibility in tests
- Update `GridWidget` to use `viewport_iter()` instead of `viewport()`

**Why**: Hot rendering path at 60fps should not allocate. Single biggest performance win before Phase 3.

**Verification**:
- All 38 existing tests pass
- `cargo clippy -- -D warnings` clean
- `GridWidget::render` no longer calls `.collect()` on the viewport

---

### Task 1: Recursive BSP Layout Tree

**Files**: `src/layout/mod.rs`, `src/layout/bsp.rs`, `tests/layout_test.rs`

**What**:
- `Split` enum: `Horizontal`, `Vertical`
- `LayoutNode` enum:
  ```rust
  enum LayoutNode {
      Pane { id: u32 },
      Split {
          direction: Split,
          ratio: f32,        // 0.0..1.0, default 0.5
          left: Box<LayoutNode>,
          right: Box<LayoutNode>,
      },
  }
  ```
- `BspLayout` struct holding the root `LayoutNode`, next pane ID, and pane count
- `BspLayout::new()` — default: single pane filling the area
- `BspLayout::split(pane_id, direction, ratio)` — split an existing pane, returns new pane ID
- `BspLayout::remove(pane_id)` — remove a pane, sibling takes its space
- `BspLayout::compute_layout(area: Rect) -> Vec<(u32, Rect)>` — recursive traversal, returns (pane_id, rect) pairs
- All panes get 1-char border for visual separation (borders shared between adjacent panes)

**Border style**:
- Focused pane: `Style::default().fg(Color::Green)` on border
- Unfocused pane: `Style::default().fg(Color::DarkGray)` on border

**Tests**:
- Single pane fills area
- Horizontal split divides correctly (50/50, 70/30 ratios)
- Vertical split divides correctly
- Nested splits (split left half again)
- Remove pane — sibling expands
- compute_layout respects area bounds
- Border styles differ for focused vs unfocused

**Verification**:
- `cargo test --test layout_test` passes
- Ratatui `Rect` arithmetic is correct (no off-by-one on borders)

---

### Task 2: Focus Controller & Action Router

**Files**: `src/app/mod.rs`, `src/app/focus.rs`, `src/app/actions.rs`, `tests/actions_test.rs`

**What**:

**Actions enum**:
```rust
enum Action {
    // Pane management
    SplitHorizontal,
    SplitVertical,
    ClosePane,
    FocusUp,
    FocusDown,
    FocusLeft,
    FocusRight,
    // Layout
    NextLayout,
    // Scrollback
    ScrollUp,
    ScrollDown,
    // Application
    Quit,
    // Passthrough
    Input(Vec<u8>),  // raw bytes to send to focused PTY
}
```

**Key bindings** (tmux-style prefix `Ctrl-A`):
```rust
fn resolve_key(key: KeyEvent, prefix_active: bool) -> Option<Action> {
    if !prefix_active {
        return match key.code {
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::EnterPrefix)
            }
            _ => Some(Action::Input(key_to_bytes(key))),
        };
    }
    // Prefix mode active
    match key.code {
        KeyCode::Char('"') => Some(Action::SplitHorizontal),
        KeyCode::Char('%') => Some(Action::SplitVertical),
        KeyCode::Char('x') => Some(Action::ClosePane),
        KeyCode::Up    => Some(Action::FocusUp),
        KeyCode::Down  => Some(Action::FocusDown),
        KeyCode::Left  => Some(Action::FocusLeft),
        KeyCode::Right => Some(Action::FocusRight),
        KeyCode::Char('c') => Some(Action::SplitHorizontal),
        KeyCode::Char('n') => Some(Action::FocusDown),
        KeyCode::Char('p') => Some(Action::FocusUp),
        KeyCode::Char('q') => Some(Action::Quit),
        _ => None,  // exit prefix mode
    }
}
```

**FocusController**:
```rust
struct FocusController {
    focused_pane: u32,
    all_panes: Vec<u32>,
    prefix_active: bool,
}
```

**Tests**:
- Prefix key activates prefix mode
- Passthrough keys generate `Action::Input`
- Split/Close/Focus actions resolve correctly
- Focus cycling (up/down/left/right through panes)
- Quit action always available

---

### Task 3: Signal Orchestration (SIGWINCH)

**Files**: `src/app/signals.rs`, `tests/signals_test.rs`

**What**:
- `TerminalSize` struct: `cols: u16, rows: u16`
- `get_terminal_size() -> TerminalSize` via `crossterm::terminal::size()`
- Resize events come through the `EventStream` as `Event::Resize(cols, rows)` — no separate signal handler needed
- `resize_all_panes(panes: &HashMap<u32, PaneState>, cols, rows)` — recomputes BSP layout, sends `PtyCommand::Resize` to each pane's channel

**Tests**:
- `get_terminal_size()` returns reasonable values
- Resize action triggers `PtyCommand::Resize` on all pane channels

---

### Task 4: Application Shell & Async Render Loop

**Files**: `src/app/mod.rs`, `src/app/render.rs`, `tests/app_test.rs`

**What**:

**App struct**:
```rust
struct App {
    bsp: BspLayout,
    focus: FocusController,
    panes: HashMap<u32, PaneState>,
    running: bool,
}

struct PaneState {
    grid: Arc<RwLock<Grid>>,
    pty_tx: mpsc::Sender<PtyCommand>,
}
```

**Async main loop** (tokio::select! — non-blocking, no deadlocks):
```rust
use crossterm::event::EventStream;
use tokio_stream::StreamExt;

async fn run_app(terminal: &mut Terminal) -> Result<()> {
    let mut event_stream = EventStream::new();
    let mut render_tick = interval(Duration::from_millis(16)); // ~60 FPS
    let mut app = App::new();

    loop {
        tokio::select! {
            // 1. Handle keyboard + resize events (non-blocking)
            maybe_event = event_stream.next() => {
                if let Some(Ok(event)) = maybe_event {
                    match event {
                        Event::Key(key) => {
                            if let Some(action) = resolve_key(key, app.focus.prefix_active) {
                                if action == Action::Quit { break; }
                                app.handle_action(action).await;
                            }
                        }
                        Event::Resize(cols, rows) => {
                            app.resize_all(cols, rows).await;
                        }
                        _ => {}
                    }
                }
            }

            // 2. Throttled UI redraw (~60 FPS)
            _ = render_tick.tick() => {
                terminal.draw(|frame| {
                    app.render(frame);
                })?;
            }
        }
    }
    Ok(())
}
```

**Why this is correct**:
- `EventStream.next()` is async — yields to tokio when no events, doesn't block
- `render_tick.tick()` fires every 16ms — ensures PTY output renders even without user input
- `tokio::select!` races both futures — whichever resolves first gets handled
- No blocking `poll()` call — PTY read tasks and GridManager run concurrently

**Tests**:
- App creation with single pane
- Action dispatch (split, focus, close)
- Render produces output without panic

---

## Post-Phase 3 State

```
src/
  grid/          (Phase 1 — complete, hardened)
  pty/           (Phase 2 — complete)
  layout/        (Phase 3 — this phase)
    mod.rs         Module root
    bsp.rs         Recursive BSP layout tree
  app/           (Phase 3 — this phase)
    mod.rs         Application shell + async event loop
    focus.rs       Focus controller + prefix key
    actions.rs     Action enum + key resolution
    signals.rs     Terminal resize handling
    render.rs      Render loop
```

The three layers are fully decoupled:
- **Grid layer** — pure data structures, knows nothing about PTYs or UI
- **PTY layer** — feeds bytes into Grid via actor channel
- **Layout/App layer** — reads Grids via `Arc<RwLock>`, renders via `GridWidget`, sends input back to PTYs
