# Phase 4: Production Entry Point & Geometric Focus

## Gemini Verification Applied

1. **`src/pane/router.rs` does not exist** — hallucinated file, skipped
2. **ProcessExit already implemented** — `PtyCommand::ProcessExit` in `grid_manager.rs`, test passing
3. **Geometric focus** — valid improvement, implemented in `App::handle_action` (not FocusController)
4. **TerminalGuard** — standard pattern, sound
5. **main.rs API mismatches corrected** — all signatures match actual codebase

## Actual API Signatures (source of truth)

```rust
// App
App::new() -> Self                                    // no parameters
app.running: bool                                     // public field, not method
app.handle_action(action: Action)                     // sync, not async
app.resize_all(cols: u16, rows: u16)                  // sync, not async
app.add_pane(id, grid: Arc<RwLock<Grid>>, pty_tx)    // register pane

// Render (free function, not method)
render::render(app: &App, frame: &mut Frame)

// Key resolution (two-step, sync)
resolve_key(key: KeyEvent, prefix_active: bool) -> Option<Action>
app.handle_action(action)                             // after resolution

// BSP
bsp.compute_layout(area: Rect) -> Vec<(u32, Rect)>   // returns pane IDs + rects

// Focus
focus.focused_pane: u32                               // public field
focus.set_focused_pane(id: u32)                       // NEW — to be added
```

---

## Tasks

### Task 0: Add `set_focused_pane` to FocusController

**File**: `src/app/focus.rs`

Add a public setter so `App::handle_action` can update focus from geometric calculations:

```rust
pub fn set_focused_pane(&mut self, id: u32) {
    self.focused_pane = id;
}
```

**Tests**: Add to `tests/actions_test.rs` or `tests/app_test.rs`

---

### Task 1: Geometric Focus Navigation

**File**: `src/app/mod.rs`

Replace the flat `focus_up`/`focus_down`/`focus_left`/`focus_right` with BSP-aware geometric navigation:

```rust
Action::FocusUp | Action::FocusDown | Action::FocusLeft | Action::FocusRight => {
    self.focus.prefix_active = false;
    self.resolve_geometric_focus(action);
}
```

**Geometric algorithm** (implemented in `App`):

```rust
fn resolve_geometric_focus(&mut self, direction: Action) {
    let area = Rect::new(0, 0, 80, 24); // will use actual terminal size
    let layout = self.bsp.compute_layout(area);

    // Find current pane's rect
    let current_rect = match layout.iter().find(|(id, _)| *id == self.focus.focused_pane) {
        Some((_, rect)) => *rect,
        None => return,
    };

    // Calculate midpoint of current pane
    let mid_x = current_rect.x + current_rect.width / 2;
    let mid_y = current_rect.y + current_rect.height / 2;

    // Direction vector
    let (dx, dy) = match direction {
        Action::FocusUp => (0, -1),
        Action::FocusDown => (0, 1),
        Action::FocusLeft => (-1, 0),
        Action::FocusRight => (1, 0),
        _ => return,
    };

    // Find nearest pane in that direction
    let mut best: Option<(u32, u32)> = None; // (pane_id, distance_squared)
    for (id, rect) in &layout {
        if *id == self.focus.focused_pane {
            continue;
        }

        let other_mid_x = rect.x + rect.width / 2;
        let other_mid_y = rect.y + rect.height / 2;

        // Check if other pane is in the correct direction
        let in_direction = match direction {
            Action::FocusUp => other_mid_y < mid_y,
            Action::FocusDown => other_mid_y > mid_y,
            Action::FocusLeft => other_mid_x < mid_x,
            Action::FocusRight => other_mid_x > mid_x,
            _ => false,
        };

        if !in_direction {
            continue;
        }

        // Euclidean distance squared (no sqrt needed for comparison)
        let dist_sq = (other_mid_x as i32 - mid_x as i32).unsigned_abs().pow(2)
            + (other_mid_y as i32 - mid_y as i32).unsigned_abs().pow(2);

        match best {
            None => best = Some((*id, dist_sq)),
            Some((_, best_dist)) if dist_sq < best_dist => best = Some((*id, dist_sq)),
            _ => {}
        }
    }

    if let Some((target_id, _)) = best {
        self.focus.set_focused_pane(target_id);
    }
}
```

**Tests** (`tests/app_test.rs`):
- Focus right finds the pane to the right
- Focus left finds the pane to the left
- Focus with no neighbor stays on current pane
- Geometric focus with nested BSP splits

---

### Task 2: TerminalGuard for Panic Safety

**File**: `src/main.rs`

```rust
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    }
}
```

This ensures the terminal is restored even on panic.

---

### Task 3: Wire Up `main.rs` with Correct API

**File**: `src/main.rs`

```rust
use std::io;
use std::time::Duration;
use tokio::time::interval;
use tokio_stream::StreamExt;
use crossterm::event::{EventStream, Event, KeyCode, KeyModifiers};
use crossterm::terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use fsh_tmux::app::{App, render};
use fsh_tmux::app::actions::resolve_key;
use fsh_tmux::pty::async_pty::AsyncPty;
use fsh_tmux::pty::grid_manager::{GridManager, PtyCommand};

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            io::stdout(),
            terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Enter raw mode safely
    terminal::enable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        terminal::EnterAlternateScreen,
        crossterm::cursor::Hide
    )?;
    let _guard = TerminalGuard;

    // 2. Setup ratatui terminal
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    // 3. Get initial terminal size
    let (cols, rows) = terminal::size()?;

    // 4. Initialize app
    let mut app = App::new();

    // 5. Spawn initial pane (shell)
    let mut pty = AsyncPty::spawn("/bin/sh", cols, rows)?;
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let (mut manager, grid_ref) = GridManager::new(cols as usize, rows as usize, 1000, rx);

    // Spawn GridManager task
    tokio::spawn(async move {
        manager.run().await;
    });

    // Spawn PTY read loop
    let read_tx = tx.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            match pty.read(&mut buf).await {
                Ok(n) => {
                    if read_tx.send(PtyCommand::Data(buf[..n].to_vec())).await.is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = read_tx.send(PtyCommand::ProcessExit).await;
                    break;
                }
            }
        }
    });

    // Register the pane
    app.add_pane(0, grid_ref, tx);

    // 6. Event loop
    let mut event_stream = EventStream::new();
    let mut render_tick = interval(Duration::from_millis(16)); // ~60 FPS

    while app.running {
        tokio::select! {
            maybe_event = event_stream.next() => {
                if let Some(Ok(event)) = maybe_event {
                    match event {
                        Event::Key(key) => {
                            // Ctrl-C always quits
                            if key.code == KeyCode::Char('c')
                                && key.modifiers.contains(KeyModifiers::CONTROL)
                            {
                                app.running = false;
                                continue;
                            }
                            // Two-step sync resolution
                            if let Some(action) = resolve_key(key, app.focus.prefix_active) {
                                app.handle_action(action);
                            }
                        }
                        Event::Resize(cols, rows) => {
                            app.resize_all(cols, rows);
                        }
                        _ => {}
                    }
                }
            }
            _ = render_tick.tick() => {
                terminal.draw(|frame| {
                    render::render(&app, frame);
                })?;
            }
        }
    }

    Ok(())
}
```

**Key differences from Gemini's proposal**:
- `App::new()` — no parameters (matches our API)
- `resolve_key(key, prefix)` → `app.handle_action(action)` — two-step sync (matches our API)
- `app.running` — field access, not method (matches our API)
- `render::render(&app, frame)` — free function (matches our API)
- `app.resize_all(cols, rows)` — sync not async (matches our API)
- PTY read loop spawned separately, sends `ProcessExit` on EOF (matches our architecture)

---

## Verification

After implementation:
1. `cargo build` — compiles without errors
2. `cargo test` — all 69+ tests pass
3. `cargo clippy -- -D warnings` — zero warnings
4. `cargo run` — launches the multiplexer, shows a shell prompt with green border
5. Type commands — output renders correctly
6. `Ctrl-A "` — splits horizontally
7. `Ctrl-A k`/`j` — scrolls through scrollback
8. `Ctrl-C` — exits cleanly, terminal restored
