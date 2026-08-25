# FTUI Bug Tracker

Auditor: AI-assisted code audit  
Date: 2025  
Files: `crates/fshell-repl/src/ftui/` (mod.rs, buffer.rs, completions.rs, prompt.rs, history.rs, mouse.rs, cursor.rs, statusbar.rs, margins.rs, agent.rs, clipboard.rs, capture.rs)

---

## Status Key

- **[🟢 OPEN]** — Not yet fixed
- **[🟡 IN PROGRESS]** — Work started
- **[🟢 FIXED]** — Fixed and verified
- **[🔴 WONTFIX]** — Accepted as is

---

## 1. Keyboard Event Handling & Keybindings (mod.rs)

### Bug 1.1 — Completion Span Deletion
- **File:** `mod.rs` (lines ~1508, ~1588, ~1610, ~2050)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** Accepting a completion (Tab/Enter/Right with active selection) deletes back to the last whitespace/delimiter character instead of using the `Suggestion.span` field. When completing a path like `cd /usr/lo` → Tab, the code scans backward for whitespace/`|`/`>`/`<` and deletes everything from that point. This breaks path continuations because the deletion erases too much (e.g. `echo hello /usr/lo` → Tab deletes `hello /usr/lo` instead of just `/usr/lo`).

**Root cause:** Five separate code locations all compute `last_word_start` manually:
```rust
let last_word_start = chars[..cursor].iter().rposition(|&c| {
    c.is_whitespace() || c == '|' || c == '>' || c == '<'
}).map(|idx| idx + 1).unwrap_or(0);
```
They ignore the `s.span` field which the `FshellCompleter` returns with the correct byte range to replace.

**Fix:** Use `s.span` (byte offsets) from the `Suggestion` struct to determine which portion of the buffer to delete before inserting the completion value.

---

### Bug 1.2 — Ctrl+C Trapped in Overlays
- **File:** `mod.rs` (lines ~1208-1270)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** Inside History Explorer (Ctrl+R), pressing Ctrl+C is not caught by any handler — it falls through to the generic `Char(c)` arm at line ~1268 which appends `'c'` to the search query. Expected: exit the overlay without inserting 'c'.

**Root cause:** No explicit `KeyCode::Char('c') with KeyModifiers::CONTROL` handler inside the `if history_mgr.active` block.

---

### Bug 1.3 — Ctrl+D Ignored on Non-Empty Buffer
- **File:** `mod.rs` (lines ~1843-1860)
- **Severity:** 🟢 Low
- **Status:** [🟢 FIXED]

**Description:** Ctrl+D only works when `text_buf.is_empty()` (triggers exit). On a non-empty buffer, the event is silently discarded because `key.modifiers.contains(KeyModifiers::CONTROL)` doesn't match the generic char handler (which requires `modifiers.is_empty() || modifiers == SHIFT`). Standard readline behavior: Ctrl+D on non-empty buffer = `delete_right()`.

---

### Bug 1.4 — Missing Ctrl+L Clear Screen
- **File:** `mod.rs`
- **Severity:** 🟢 Low
- **Status:** [🟢 FIXED]

**Description:** No handler exists for `KeyCode::Char('l') with KeyModifiers::CONTROL`. The event is silently discarded. Expected: clear terminal screen (`\x1b[2J\x1b[1;1H`) and redraw the prompt.

---

### Bug 1.5 — Ctrl+R Cycle vs Filter
- **File:** `mod.rs` (line ~1261)
- **Severity:** 🟢 Low
- **Status:** [🟢 FIXED]

**Description:** While History Explorer is open, pressing Ctrl+R (key.modifiers.contains(CONTROL), code Char('r')) calls `history_mgr.filter_mode = history_mgr.filter_mode.next()`, toggling between Global/Host/Cwd/Session filter scopes. A user expecting to cycle to the next history match instead gets a completely different filter view. Expected: Ctrl+R while in explorer should select the next result (Down arrow behavior), not change the filter.

---

### Bug 1.6 — Home/End Multi-Line Behavior
- **File:** `mod.rs` (lines ~1655-1692), `buffer.rs` (`move_to_start`, `move_to_end`)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** Home and End jump to cursor position 0 or `chars.len()` (absolute start/end of the entire buffer). In multi-line prompts, expected behavior is to jump to the start/end of the current visual line (the line containing the cursor), not the entire buffer.

**Root cause:** `move_to_start()` = `self.cursor = 0`, `move_to_end()` = `self.cursor = self.chars.len()`.

**Fix:** Add `move_to_line_start()` / `move_to_line_end()` methods that find the previous/next `\n` boundary.

---

### Bug 1.7 — EnableBracketedPaste Missing
- **File:** `mod.rs` (lines ~41-54, ~90-105)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** `TuiGuard::drop()` and the panic hook both call `DisableBracketedPaste` on cleanup, but `TuiGuard::new()` never calls `EnableBracketedPaste` on startup. The `Event::Paste` handler exists and works, but only if the terminal has bracketed paste enabled by default (most do, but not guaranteed).

---

### Bug 1.8 — Shift+Tab Unhandled
- **File:** `mod.rs` (line ~1485, main match ~1619+)
- **Severity:** 🟢 Low
- **Status:** [🟢 FIXED]

**Description:** `KeyCode::BackTab` (Shift+Tab) is only handled inside the `if comp_mgr.visible` block (reverse cycle selection). When completions are closed, Shift+Tab falls through to the `_ => {}` wildcard and is silently discarded. Expected: reverse indent or no-op (silent discard is acceptable, but matching it explicitly is cleaner).

---

## 2. Mouse Processing (mod.rs, mouse.rs)

### Bug 2.1 — Multi-Line Mouse Clicks
- **File:** `mod.rs` (lines ~1153-1168)
- **Severity:** 🔴 High
- **Status:** [🟢 FIXED]

**Description:** Mouse click positioning ignores row (y-coordinate) entirely when positioning the cursor. The handler only uses `column >= prompt_len` to compute click column, and the `row` parameter is passed to `mouse_mgr.handle_click(row, prompt_y)` but never used for cursor positioning. In multi-line prompts, clicking on line 2+ jumps to line 1 instead.

**Additionally:** When `output_lines` is non-empty (anchored mode), the prompt is offset by `output_lines.len()` rows in the viewport, but the mouse handler doesn't account for this offset either.

---

### Bug 2.2 — Completion Mouse Clicks No-Op
- **File:** `mod.rs` (lines ~1161-1166)
- **Severity:** 🔴 High
- **Status:** [🟢 FIXED]

**Description:** When completions are visible and the user clicks on a completion item, the handler only sets `redraw = true` but does not:
1. Map the click (column, row) to a completion item index
2. Select the clicked item
3. Insert the clicked item

Expected: clicking a completion item should select and/or insert it (like VS Code, IntelliJ, etc.).

---

### Bug 2.3 — Mouse Drag Selection Unimplemented
- **File:** `mod.rs` (mouse handler)
- **Severity:** 🟡 Minor
- **Status:** [🟢 FIXED]

**Description:** `MouseEventKind::Drag` events are never matched in the mouse handler. Any mouse drag is silently discarded, making text selection by dragging impossible.

---

### Bug 2.4 — Mouse State Not Reset on Exit
- **File:** `mouse.rs`
- **Severity:** 🟢 Low
- **Status:** [🟢 FIXED]

**Description:** If the user exits FTUI while mouse capture is disabled (e.g., because they clicked above the prompt in Smart mode), the `DisableMouseCapture` in `TuiGuard::drop` is redundant but harmless. However, the `is_captured` flag is not reset between input loop iterations, which could cause stale state on re-entry.

---

## 3. Completion Popup UI (completions.rs, mod.rs)

### Bug 3.1 — Popup Height Crushing Near Bottom
- **File:** `mod.rs` (lines ~1329-1336)
- **Severity:** 🔴 High
- **Status:** [🟢 FIXED]

**Description:** The completion popup is always positioned below the prompt line (`popup_y = prompt_line.y + 1 + visual_line`). When the prompt is near the bottom of the screen, `popup_y` is close to `size.height`, and `max_h.min(size.height.saturating_sub(popup_y))` crushes the popup to 0-2 lines. There is no logic to flip the popup above the prompt line when there's insufficient space below.

**Expected:** When space below the prompt is insufficient for the popup, render it above the prompt line instead.

---

### Bug 3.2 — Unicode Width in Descriptions
- **File:** `completions.rs` (line ~389)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** Description truncation uses `.chars().take(max_desc_width)`, which counts characters, not display columns. A 5-width emoji like `🚀` is counted as 1 character, causing it to exceed the allocated width and corrupt the popup border.

**Additionally:** Bug 8.2 in completions.rs line ~384 has the same issue for value truncation.

---

### Bug 3.3 — PageUp/PageDown Index Drift
- **File:** `completions.rs` (lines ~280-295), `mod.rs` (lines ~1495-1511)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** `page_down()`/`page_up()` operate on `self.suggestions.len()` (flat suggestion count). But the grouped display adds category header lines between groups (e.g., `[ Dirs  (5) ]` + 5 dir items). The `selected_idx` and `scroll_offset` don't account for these header lines. A PageDown that should advance one screen may land on a header line or skip items unpredictably.

---

### Bug 3.4 — `render_popup` Builds All Items Unconditionally
- **File:** `completions.rs` (lines ~327-395)
- **Severity:** 🟡 Minor (performance)
- **Status:** [🟢 FIXED]

**Description:** `render_popup()` builds `ListItem` widgets for ALL items in all groups, even if only a subset is visible. Then `mod.rs` (lines ~1409-1411) slices `start..end` from the already-built items. For 500+ suggestions, this allocates 500 ListItem objects every redraw when only 15 are visible. Should short-circuit based on `_visible_lines`.

---

## 4. Prompt & Continuation Rendering (prompt.rs, mod.rs)

### Bug 4.1 — ANSI Color Bleeding (Template Path)
- **File:** `prompt.rs` (`render_prompt_left_ansi`, `render_prompt_left`), `lib.rs` (`render_prompt_template`)
- **Severity:** 🔴 High
- **Status:** [🟢 FIXED]

**Description:** When a custom `FSH_PROMPT` or `FSH_PROMPT_RIGHT` template is set, `render_prompt_template()` (lib.rs:388-470) replaces placeholders but does **not** append `\x1b[0m` to the final result. If the user's template ends with an ANSI color code (e.g., `\x1b[34m`), that color bleeds into:
1. The user's typed input (in the TUI text area)
2. The command output (when printed via `println!` at mod.rs line ~1850)

The segment-based render path (`render_segment_list`) DOES append `\x1b[0m` at prompt.rs ~809, so default prompts are safe, but custom templates are not.

---

### Bug 4.2 — Right Prompt Artifacts
- **File:** `mod.rs` (lines ~774-786)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** The right prompt is conditionally rendered only when `left_plus_input + right_width + 1 < size.width`. When input text grows past this boundary, the right prompt is no longer rendered in the new frame. However, the previous frame's right prompt text (and padding spaces) may leave visual residue on the terminal, especially when the terminal scrolls or the viewport changes between frames. Ratatui's `Inline` viewport doesn't auto-clear cells outside the current draw — stale cells from previous frames can persist.

---

### Bug 4.3 — Narrow Terminal Prompt Hiding
- **File:** `mod.rs` (line ~698)
- **Severity:** 🟢 Low
- **Status:** [🟢 FIXED]

**Description:** In multi-line mode, `available_width = size.width.saturating_sub(prompt_len).saturating_sub(4)`. If the prompt is longer than the terminal width, `available_width = 0`, then `visible_width = available_width.saturating_sub(1) = 0`. All typed text is invisible. There's no minimum width floor.

---

### Bug 4.4 — Multi-Line Right Prompt Misalignment
- **File:** `mod.rs` (lines ~721-734)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** In multi-line mode, the right prompt padding calculation at line ~729 uses `size.width` without accounting for the line numbering gutter (` │ ` prefix, ~4-5 chars) on continuation lines. The right prompt may be misaligned or not right-aligned properly on continuation lines 2+.

---

## 5. Signals, Resizes & Job Control (mod.rs)

### Bug 5.1 — Terminal Shrink Scroll Jump
- **File:** `mod.rs` (lines ~575-602)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** When the terminal height decreases, the scroll logic computes `d = (cursor_y + needed_height).saturating_sub(limit_row)` and prints `d` newlines to scroll. For large deltas (e.g., shrinking from 50 to 24 rows), `d` can be large, causing abrupt scrolling. The code also relies on `crossterm::cursor::position()` which is unreliable in raw mode (some terminals return inaccurate positions).

---

### Bug 5.2 — Job Control SIGTSTP (Ctrl+Z) Terminal Corruption
- **File:** `mod.rs` (no signal handler)
- **Severity:** 🔴 High
- **Status:** [🟢 FIXED]

**Description:** No SIGTSTP (Ctrl+Z) signal handler is installed. When the user presses Ctrl+Z, the default OS behavior suspends the process immediately, leaving the terminal in raw mode with mouse capture and bracketed paste still enabled. The terminal is corrupted (no echo, no newlines, garbled display) until the user runs `reset` or `stty sane`.

**Fix:** Install a `SIGTSTP` handler that restores terminal state before suspending, and re-enables raw mode on resume (SIGCONT).

---

### Bug 5.3 — SIGINT Not Interrupting FTUI Command Execution
- **File:** `mod.rs` (lines ~2230-2260)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** When a long-running command is executed through FTUI (in non-anchored mode, which disables raw mode for execution), Ctrl+C is handled by the OS (signal terminal process group). However the re-enabled raw mode after execution re-traps input. There's no mechanism to cancel a command mid-execution from the TUI event loop perspective if raw mode is still active. For anchored mode, the command runs inside the TUI with raw mode on, and Ctrl+C would insert a character into the buffer rather than interrupting.

---

## 6. Undo/Redo & Selection (buffer.rs)

### Bug 6.1 — Selection Bracket Wrapping
- **File:** `buffer.rs` (line ~175)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** When a selection is active and the user types `(`, `insert_char` calls `delete_selection()` first (line ~177), which deletes the selected text. Then the auto-close inserts `()` around empty space. Expected behavior: wrap the selected text like `(selected_text)` — not delete it.

**Example:** User selects "foo", types `(` → result is `()` instead of expected `(foo)`.

**Fix:** Instead of deleting first, save the selection text, insert `(`, insert selection text, insert `)`.

---

### Bug 6.2 — Single Keystroke Undo Grouping
- **File:** `buffer.rs` (commit_transaction), `mod.rs` (lines ~1640, ~1760, ~1843)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** `commit_transaction()` is only called on Enter, Up, Down (history navigation), Clear, `replace_content`, undo, and redo. Each individual character typed pushes to `current_transaction` but never triggers `commit_transaction()`. The entire typing session (potentially thousands of keystrokes) becomes one giant undo group. Pressing Ctrl+Z undoes it all at once — the user loses everything typed since they last pressed Enter.

**Expected:** Undo should operate at the word-level or sentence-level granularity (e.g., commit on: word boundary, whitespace pause > 500ms, paste, backspace word, etc.)

---

### Bug 6.3 — Empty Transaction Pushed to Undo Stack
- **File:** `buffer.rs` (replace_content), `mod.rs` (lines ~1640, ~1643)
- **Severity:** 🟢 Low
- **Status:** [🟢 FIXED]

**Description:** History navigation (Up/Down) calls `text_buf.commit_transaction()` immediately before `text_buf.replace_content()`. If no edits were made since the last commit, `commit_transaction()` pushes an empty `Vec<EditOp>` onto the undo stack. On undo, this empty transaction is popped and processed as no-op, meaning the user needs to press Ctrl+Z twice to undo the history selection. `replace_content()` also calls `commit_transaction()` internally after its own ops.

---

### Bug 6.4 — `rebuild_auto_close_state` Cannot Distinguish Manual from Auto-Inserted Pairs
- **File:** `buffer.rs` (lines ~355-374)
- **Severity:** 🟢 Low (minor UX confusion)
- **Status:** [🟢 FIXED]

**Description:** `rebuild_auto_close_state()` scans the buffer for adjacent matching bracket pairs (`()`, `[]`, `{}`, `""`, `''`, `` `` ``) and marks them all as auto-inserted. If the user manually types `()`, both `(` and `)` are marked as auto-inserted. When the user backspaces over the `(`, `delete_left()` detects a "matching closer" pair and deletes both characters — even though the user intentionally typed both. The code acknowledges this in a comment: "we can't distinguish, but tracking more pairs is safe" — but the behavior is surprising.

---

## 7. Terminal Raw Mode & Crossterm Event Loop (mod.rs)

### Bug 7.1 — Flickering LeaveAlternateScreen
- **File:** `mod.rs` (lines ~53, ~95)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** `TuiGuard::drop()` and the panic hook unconditionally send `LeaveAlternateScreen` (`\x1b[?1049l`). Since FTUI uses `Viewport::Inline` (not `Viewport::Fullscreen`/alternate screen), this escape sequence is spurious. Most terminals briefly swap screen buffers (showing the alternate screen contents for one frame) before realizing there's nothing to restore, causing visible flicker.

---

### Bug 7.2 — Prompt Spinner Animation Freeze
- **File:** `mod.rs` (lines ~1088-1098)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** The poll timeout logic checks for active animations via `prompt_mgr.widgets.iter().any(|w| ...is_running...)` but does NOT check `prompt_mgr.animations`. The `{SPINNER}` animation (the braille spinner in prompt templates) is implemented as a `PromptAnimation`, not a `PromptWidget`. When the spinner is the only active animation (no async widgets running), the poll timeout stays at 1000ms instead of 50ms, making the spinner appear frozen between updates.

**Fix:** Also check `prompt_mgr.animations` (or check the prompt template for `{SPINNER}`/`{spinner}`) when computing the poll timeout.

---

### Bug 7.3 — `EnableFocusChange`/`DisableFocusChange` Mismatch
- **File:** `mod.rs` (lines ~53, ~97)
- **Severity:** 🟢 Low
- **Status:** [🟢 FIXED]

**Description:** `TuiGuard::drop()` sends `DisableFocusChange`, but `TuiGuard::new()` never sends `EnableFocusChange`. Like bracketed paste, focus change events may be useful for pause/resume for background animations or job notifications, but they aren't used. The mismatch is harmless but indicates a pattern inconsistency with bracketed paste.

---

## 8. Additional Bugs Found During Audit

### Bug 8.1 — Paste Doesn't Delete Active Selection
- **File:** `mod.rs` (lines ~1173-1177)
- **Severity:** 🔴 High
- **Status:** [🟢 FIXED]

**Description:** The `Event::Paste` handler calls `text_buf.insert_str(&pasted_text)` directly without first calling `delete_selection()`. If the user has an active selection and pastes (Ctrl+Shift+V or middle-click), the pasted text is inserted *after* the selection, not replacing it. The selection remains visible until the next keystroke.

**Fix:** Call `text_buf.delete_selection()` before `insert_str()` in the paste handler.

---

### Bug 8.2 — Completion Value Truncation Uses `chars().take()` Instead of Width
- **File:** `completions.rs` (line ~384)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** Same fundamental issue as Bug 3.2 but for the value (file name / command name) field:
```rust
let truncated: String = value.chars().take(max_value_width.saturating_sub(2)).collect();
```
Uses character count instead of display column width. A CJK character (width 2) or emoji (width 2) is counted as 1, causing the truncated string to overflow the allocated width and corrupt the popup border.

---

### Bug 8.3 — History Aborted Active Display Order Mismatch
- **File:** `mod.rs` (lines ~1240-1255), `history.rs` (`get_selected` line ~134-138)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** When `history_mgr.aborted_active` is true, the commands are displayed in reverse order (`.iter().rev()` at line ~1244 in mod.rs), so the most recently aborted command appears first. But `history_mgr.get_selected()` (history.rs ~134-138) processes `selected_idx` as `aborted_commands.len() - 1 - selected_idx` — this correctly maps the forward index to the reversed display. However, `select_next`/`select_prev` (history.rs ~93-108) operate on the raw `aborted_commands` index, which is forward. The visual order and the selection index order are inconsistent.

**Example:** Aborted commands = `["A", "B", "C"]` (A oldest, C newest). Display shows `[C, B, A]` (C at top). `select_next()` increments `selected_idx` from 0 to 1. Visually, the selection moves from C (idx 0) to B (idx 1). This is correct by accident because the reversal happens at the display layer.

But `adjust_scroll` (history.rs ~139-145) operates on the forward index without accounting for the reversed display. When `selected_idx = 0` (visually = C at top), and max_h = 10 with `aborted_commands.len()` = 3, it works fine. But if `selected_idx = 2` (visually = A at bottom) and max_h is small, the scroll offset computation may be wrong because it's using forward indexing while the display uses reversed indexing.

---

### Bug 8.4 — Mouse Scroll Doesn't Update Completion Scroll Offset
- **File:** `mod.rs` (lines ~1143-1155)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** ScrollUp/ScrollDown mouse events when completions are visible call `comp_mgr.select_next()` / `comp_mgr.select_prev()` and set `redraw = true`. But the completion rendering code (lines ~1354-1368) recalculates `selected_flat_idx` and adjusts `comp_mgr.scroll_offset` — this logic only runs in the draw path on redraw, not in the mouse handler. The scroll offset may be stale in some edge cases, causing the selection to scroll off-screen.

---

### Bug 8.5 — `render_popup` Scroll Offset Uses Raw Suggestion Index Instead of Flat Index
- **File:** `completions.rs` (lines ~327-395), `mod.rs` (lines ~1348-1378)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** The scroll offset calculation in `mod.rs` (lines ~1348-1378) computes `selected_flat_idx` by iterating through groups. But `comp_mgr.scroll_offset` is updated based on this flat index, while `render_popup()` (completions.rs) uses `self.selected_idx` to determine which item is selected for the grouped rendering. If `selected_flat_idx` != `selected_idx` (which is always true when there are header lines), the `render_popup` selection highlighting and the scroll offset drift.

The scrollbar at line ~1428 uses `selected_flat_idx` for position, but the list items use `self.selected_idx` for highlighting. These are inconsistent when group headers are present.

---

### Bug 8.6 — `ansi_to_spans` Doesn't Handle All ANSI Sequences
- **File:** `prompt.rs` (lines ~42-130)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** The `ansi_to_spans()` function makes several assumptions that can fail on complex ANSI prompt strings:

1. **Assumes SGR always ends with `m`** — CSI sequences ending with other letters (e.g., cursor movements like `\x1b[2K`, `\x1b[1A`) will be partially parsed as SGR codes, producing mangled output.
2. **Assumes text after `m`** — `\x1b[31m\x1b[32mtext` (two consecutive SGR without text between) would lose the first SGR code.
3. **`for cmd_idx in part.find(|c: char| c.is_ascii_alphabetic())`** — This checks the first alphabetic char, not the command letter (which must be in a specific range: `@A–Z[\]^_`a–z{|}~`). Lowercase `a` is valid for some CSI but would be treated as command letter, causing mismatches.
4. **Multiple semicolons** — `\x1b[38;5;166m` is handled, but `\x1b[38;2;255;128;0m` assumes `ci + 4` exists in `codes`. If the RGB values are space-padded or the terminal uses alternative syntax, it fails.

---

### Bug 8.7 — `char_index_to_column` / `column_to_char_index` Tab Handling
- **File:** `buffer.rs` (lines ~105-129)
- **Severity:** 🟢 Low
- **Status:** [🟢 FIXED]

**Description:** Tab width is hardcoded to 8 (no configuration option). `column_to_char_index()` returns the index of the character *before* the tab when clicking on a tab's column position (because `current_col + w > col` returns `i`, the tab index, instead of `i + 1`). This means clicking on a tab's visual position positions the cursor before the tab, which is inconsistent with the visual appearance.

---

### Bug 8.8 — `split_spans_by_newline` Handles Consecutive Newlines Incorrectly
- **File:** `mod.rs` (lines ~2615-2658)
- **Severity:** 🟢 Low
- **Status:** [🟢 FIXED]

**Description:** When the display text contains multiple consecutive newlines (e.g., "line1\n\nline3"), the `split_spans_by_newline` function at line ~2638 adds an extra empty line entry. The comment says "each newline creates a blank line entry", but the logic is: if `!line_has_content && byte_offset > 0`, push another empty line. This is triggered on the second `\n` in a row because at that point `current_line` is empty and `byte_offset` of the `\n` within the content is > 0. However, this only works for content that has been accumulated — for spans that are purely empty (no content at all), consecutive newlines may not produce the correct number of blank lines.

---

### Bug 8.9 — CaptureGuard Stderr Redirect May Deadlock
- **File:** `capture.rs` (lines ~38-120)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** `CaptureGuard::new()` redirects both stdout AND stderr to the same pipe write fd. If the reader thread blocks on reading stdout and a background async task writes a panic message to stderr, the pipe buffer may fill up. Since the reader is single-threaded and also reading stdout, the stderr write blocks, and the process deadlocks because the pipe write buffer is full. The condition is rare (requires the pipe buffer, usually 64KB, to fill up) but can happen with verbose debugging output.

**Fix:** Use separate pipes for stdout and stderr with separate reader threads, or use a larger/bounded pipe buffer.

---

### Bug 8.10 — Status Bar Terminal Recreated on Every Resize
- **File:** `mod.rs` (lines ~619-650)
- **Severity:** 🟡 Minor
- **Status:** [🟢 FIXED]

**Description:** On resize, `status_terminal` is set to `None` (line ~622), which triggers a full recreation of the status bar terminal on the next draw. The overhead is minimal but unnecessary — the `Viewport::Fixed` rect can be updated without recreating the Terminal.

---

### Bug 8.11 — Agent Mode Enter Key Confusion
- **File:** `mod.rs` (lines ~1280-1288)
- **Severity:** 🟠 Medium
- **Status:** [🟢 FIXED]

**Description:** When agent mode is active and a result is loaded (`agent_state.result_command.is_some()`), pressing Enter closes agent mode and inserts the result. When no result is loaded yet, pressing Enter triggers a new query. But if the agent is still loading (`agent_state.is_loading == true`), pressing Enter triggers a second query (re-triggers the same prompt). Expected: disable Enter while loading, or show a "loading" message.

---

### Bug 8.12 — Help Tooltip Not Recalculated on Every Keystroke
- **File:** `mod.rs` (lines ~1036-1090)
- **Severity:** 🟢 Low
- **Status:** [🟢 FIXED]

**Description:** The help tooltip (F1) shows info for the command at the cursor when F1 was pressed. But if the cursor moves after F1 is pressed, the tooltip still shows the old command's help. Expected: help tooltip should recalculate `get_command_for_cursor()` on every redraw while `help_visible` is true, showing help for the current cursor position.

---

## Summary

| Category | Reported Bugs | Additional Bugs Found | Total |
|----------|---------------|----------------------|-------|
| 1. Keyboard/Keybindings | 8 (1.1–1.8) | — | 8 |
| 2. Mouse | 3 (2.1–2.3) | 1 (2.4) | 4 |
| 3. Completion Popup | 3 (3.1–3.3) | 1 (3.4) | 4 |
| 4. Prompt/Rendering | 3 (4.1–4.3) | 1 (4.4) | 4 |
| 5. Signals/Resize/Jobs | 2 (5.1–5.2) | 1 (5.3) | 3 |
| 6. Undo/Redo/Selection | 2 (6.1–6.2) | 2 (6.3–6.4) | 4 |
| 7. Terminal/Raw Mode | 2 (7.1–7.2) | 1 (7.3) | 3 |
| 8. Additional | — | 12 (8.1–8.12) | 12 |
| **Total** | **23** | **19** | **42** |
