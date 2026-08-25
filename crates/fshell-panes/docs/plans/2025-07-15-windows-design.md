# Multi-Window Support — Design Document

> Design for adding tmux-style windows (tabs) to fsh-tmux sessions.

## Overview

A `Session` currently contains one flat set of panes and one BSP layout. This design adds a `Window` abstraction layer: each session owns multiple windows, each window has its own panes, BSP tree, and focus state. Windows run independently — PTY actors stay alive in background windows.

## Architecture

```
Session
  ├── name: String
  ├── windows: Vec<Window>
  ├── active_window: usize
  ├── terminal_size: (u16, u16)
  ├── next_pane_id: u32
  └── attached_client: Option<Sender<Vec<u8>>>

Window
  ├── id: u32
  ├── name: String
  ├── panes: HashMap<u32, PaneState>
  ├── bsp: BspLayout
  ├── focus: FocusController
  ├── show_help: bool
  ├── help_scroll: u16
  └── rename_buffer: Option<String>
```

## Key Design Decisions

1. **Pane IDs are globally unique per session** via monotonic `next_pane_id` counter
2. **`active_window: usize`** — index into Vec, not ID-based lookup (simpler, O(1), no edge cases since we control all mutations)
3. **Each window owns its own BSP tree, focus, and UI state** — switching windows is a pointer swap
4. **PTY actors run independently** — background windows accumulate output in their grids
5. **Cancel rename on window switch** — no need to carry target IDs in rename enum
6. **Last window closed = session destroyed** — no empty sessions

## Proto Changes

New client→daemon messages:
- `WindowNew`, `WindowNext`, `WindowPrevious`, `WindowSwitch(u32)`, `WindowClose`, `WindowRename`

New wire type IDs: `0x12` through `0x17`.

## Keybinding Changes

| Key | Old | New |
|-----|-----|-----|
| `Ctrl-A c` | SplitHorizontal | WindowNew |
| `Ctrl-A n` | FocusDown | WindowNext |
| `Ctrl-A p` | FocusUp | WindowPrevious |
| `Ctrl-A 0-9` | — | WindowSwitch(N) |
| `Ctrl-A &` | — | WindowClose |
| `Ctrl-A W` | — | WindowRename |

`Ctrl-A "` remains SplitHorizontal. `Ctrl-A %` remains SplitVertical.

## Status Bar

Window tabs rendered inline in the status bar (tmux-style):
```
 0:bash- 1:vim* 2:logs │ default │ p1 vim │ 3 panes │ 14:32
```

Truncation: greedy outward expansion from active window. Use `UnicodeWidthStr` for width calculation.
