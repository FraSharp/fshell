# fsh-tmux Bug Report

Generated: 2025-07-15

## Fixed Bugs

### Bug 1: F5-F12 key encoding broken in client ✅
- **File**: `src/client/mod.rs`
- **Severity**: High
- **Description**: F5-F12 all sent incorrect escape sequences due to broken byte mutation logic. Each F-key was using the same base sequence and mutating the wrong byte position.
- **Fix**: Each F-key now sends its correct xterm escape sequence explicitly.
- **Commit**: 30d119e

### Bug 2: FocusLeft/FocusRight mapped to FocusUp/FocusDown ✅
- **File**: `src/daemon/client_handler.rs`
- **Severity**: High
- **Description**: `PrefixCommand::FocusLeft` called `focus_up()` and `FocusRight` called `focus_down()`, making them functionally identical to FocusUp/FocusDown.
- **Fix**: Implemented geometric focus navigation using BSP layout positions for all four directions.
- **Commit**: b5e08c4

### Bug 3: Wide continuation characters rendered as spaces ✅
- **File**: `src/daemon/renderer.rs`
- **Severity**: Medium
- **Description**: Wide continuation characters (character '\0') were converted to spaces instead of being skipped, causing display artifacts for CJK/emoji characters.
- **Fix**: Added `wide_continuation` check to skip these cells, matching `GridWidget::render()` behavior.
- **Commit**: de27d99

### Bug 4: Missing hidden modifier in daemon renderer ✅
- **File**: `src/daemon/renderer.rs`
- **Severity**: Low
- **Description**: The daemon renderer didn't handle `pen.hidden` attribute, unlike `GridWidget::render()`.
- **Fix**: Added `Modifier::HIDDEN` for cells with `pen.hidden=true`.
- **Commit**: de27d99

### Bug 5: Missing Default implementation for SessionManager ✅
- **File**: `src/daemon/session_manager.rs`
- **Severity**: Low
- **Description**: Clippy warning: `SessionManager::new()` existed but `Default` trait was not implemented.
- **Fix**: Added `impl Default for SessionManager`.
- **Commit**: aeae820

### Bug 6: Legacy keybindings didn't match help text ✅
- **File**: `src/app/actions.rs`
- **Severity**: Medium (legacy mode)
- **Description**: 'c' mapped to SplitHorizontal (should be WindowNew), 'n' to FocusDown (should be WindowNext), 'p' to FocusUp (should be WindowPrevious).
- **Fix**: Added WindowNew/WindowNext/WindowPrevious Action variants and corrected key mappings.
- **Commit**: 0920ecd

### Bug 7: cursor_col allowed to equal width (out of bounds) ✅
- **File**: `src/grid/scrollback.rs`
- **Severity**: Medium
- **Description**: `set_cursor` allowed `cursor_col` to equal `self.width`, which is out of bounds for a 0-indexed grid.
- **Fix**: Clamped to `self.width.saturating_sub(1)`, consistent with `cursor_row` clamping.
- **Commit**: 9bfb709

### Bug 8: Clippy warnings (collapsible_if, manual arithmetic, too_many_arguments) ✅
- **Files**: Multiple
- **Severity**: Low
- **Description**: Various clippy warnings for code style and potential issues.
- **Fix**: Fixed all warnings including collapsing nested ifs, using saturating_sub, and refactoring render_status_bar to use StatusBarContext struct.
- **Commit**: a72b984

---

## Verification

- All 148 tests pass (2 renderer tests ignored as they need a terminal)
- Clippy passes with zero warnings
- No remaining unwrap()/expect() issues (all are justified)

## Accepted Risk Items

The following items were reviewed and found to be acceptable:

1. **Mutex unwrap in renderer**: `SharedBuffer::write()` and `take_bytes()` use `unwrap()` on mutex locks. These could panic if the mutex is poisoned, but this is extremely unlikely in practice.

2. **expect() on serialization**: `Frame::from_client()` and `Frame::from_server()` use `expect()` on bincode serialization. This is acceptable because serialization should never fail for our known message types.

3. **expect() on signal handlers**: Signal handler registration uses `expect()`. This is acceptable because failing to register is a fatal error.

4. **unwrap() on alt_rows**: `set_alt_buffer` uses `unwrap()` on `alt_rows.as_mut()`. This is safe because it's guarded by `is_alt` which is only true when `alt_rows` is `Some`.
