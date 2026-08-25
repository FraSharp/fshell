# line editor widgets & terminal ui reference

this document specifies fshell's interactive line editor widgets, terminal user interface (FTUI) components, keybindings, and Vi editing modes.

---

## table of contents

- [overview](#overview)
- [line editor widgets](#line-editor-widgets)
  - [navigation](#navigation)
  - [history search & traversal](#history-search--traversal)
  - [completion & menu navigation](#completion--menu-navigation)
  - [killing, deletion & clipboard](#killing-deletion--clipboard)
  - [text transformations](#text-transformations)
  - [execution, multiline & external editor](#execution-multiline--external-editor)
- [vi mode keybindings & motions](#vi-mode-keybindings--motions)
- [interactive ftui components](#interactive-ftui-components)
  - [sqlite history explorer (`Ctrl+H`)](#sqlite-history-explorer-ctrlh)
  - [fuzzy search & picker (`Ctrl+R`)](#fuzzy-search--picker-ctrlr)
  - [bottom status bar](#bottom-status-bar)
  - [interactive config tui (`config edit`)](#interactive-config-tui-config-edit)
- [custom keybinding configuration (`bind`)](#custom-keybinding-configuration-bind)

---

## overview

fshell provides a custom, transactional terminal line editor engine (`crates/fshell-repl/src/ftui/`) built on `crossterm` and `ratatui`.

key features:
- **transactional buffer**: multiline text buffer with syntax highlighting, visual selection, and bracket matching.
- **dual keymaps**: first-class Emacs and Vi modal keymaps.
- **embedded sqlite explorer**: search and filter historic commands by directory, exit code, and time duration.
- **interactive widgets**: interactive completion grids, fuzzy pickers, status bars, and configuration TUI.

---

## line editor widgets

widgets are named actions dispatched by key chords or macros (`bind <key> <widget>`).

### navigation

| widget name | emacs / default chord | vi normal mode | description |
|---|---|---|---|
| `beginning-of-line` | <kbd>Ctrl+A</kbd>, <kbd>Home</kbd> | `0`, `^` | move cursor to start of current line |
| `end-of-line` | <kbd>Ctrl+E</kbd>, <kbd>End</kbd> | `$` | move cursor to end of current line |
| `forward-char` | <kbd>Right</kbd>, <kbd>Ctrl+F</kbd> | `l`, <kbd>Space</kbd> | move cursor one character forward |
| `backward-char` | <kbd>Left</kbd>, <kbd>Ctrl+B</kbd> | `h` | move cursor one character backward |
| `forward-word` | <kbd>Alt+F</kbd>, <kbd>Ctrl+Right</kbd> | `w`, `e` | move cursor one word forward |
| `backward-word` | <kbd>Alt+B</kbd>, <kbd>Ctrl+Left</kbd> | `b` | move cursor one word backward |
| `beginning-of-buffer` | <kbd>Alt+Home</kbd>, <kbd>Alt+<</kbd> | `gg` | move cursor to start of multi-line buffer |
| `end-of-buffer` | <kbd>Alt+End</kbd>, <kbd>Alt+></kbd> | `G` | move cursor to end of multi-line buffer |

### history search & traversal

| widget name | default chord | description |
|---|---|---|
| `up-line-or-history` | <kbd>Up</kbd> | move up in multiline buffer or search previous history matching current prefix |
| `down-line-or-history` | <kbd>Down</kbd> | move down in multiline buffer or search next history matching current prefix |
| `interactive-history-search` | <kbd>Ctrl+R</kbd> | open interactive fuzzy history search popup |
| `history-explorer` | <kbd>Ctrl+H</kbd> | launch full-screen SQLite history explorer |
| `aborted-history-search` | <kbd>Alt+R</kbd> | recall and restore aborted commands from the current session |

### completion & menu navigation

| widget name | default chord | description |
|---|---|---|
| `expand-or-complete` | <kbd>Tab</kbd> | trigger context-aware completion or advance to next item in menu |
| `reverse-menu-complete` | <kbd>Shift+Tab</kbd> / <kbd>BackTab</kbd> | cycle backward through completion menu |
| `clear-completion` | <kbd>Esc</kbd> | dismiss completion popup menu |

### killing, deletion & clipboard

| widget name | default chord | vi normal mode | description |
|---|---|---|---|
| `delete-char` | <kbd>Delete</kbd> | `x` | delete character under cursor |
| `backward-delete-char` | <kbd>Backspace</kbd> | <kbd>Backspace</kbd> | delete character before cursor |
| `delete-char-or-list` | <kbd>Ctrl+D</kbd> | — | exit shell if line is empty; delete character if non-empty |
| `kill-line` | <kbd>Ctrl+K</kbd> | `D` | delete from cursor to end of line, copying to kill-ring |
| `backward-kill-line` | <kbd>Ctrl+U</kbd> | `d0` | delete from cursor to start of line, copying to kill-ring |
| `kill-word` | <kbd>Alt+D</kbd> | `dw`, `de` | delete word forward, copying to kill-ring |
| `backward-kill-word` | <kbd>Ctrl+W</kbd>, <kbd>Alt+Backspace</kbd> | `db` | delete word backward, copying to kill-ring |
| `kill-buffer` | <kbd>Ctrl+X</kbd> <kbd>Ctrl+K</kbd> | `dd` (whole buffer) | delete entire buffer content |
| `yank` | <kbd>Ctrl+Y</kbd> | `p`, `P` | paste text from kill-ring / system clipboard |

### text transformations

| widget name | default chord | description |
|---|---|---|
| `capitalize-word` | <kbd>Alt+C</kbd> | capitalize next word and advance cursor |
| `upcase-word` | <kbd>Alt+U</kbd> | convert next word to uppercase and advance cursor |
| `downcase-word` | <kbd>Alt+L</kbd> | convert next word to lowercase and advance cursor |
| `transpose-chars` | <kbd>Ctrl+T</kbd> | swap adjacent characters |
| `transpose-words` | <kbd>Alt+T</kbd> | swap adjacent words |
| `undo` | <kbd>Ctrl+Z</kbd>, <kbd>Ctrl+_</kbd> | undo last buffer modification (Vi: `u`) |
| `redo` | <kbd>Ctrl+Y</kbd> (in undo state) | redo last undone modification (Vi: <kbd>Ctrl+R</kbd>) |

### execution, multiline & external editor

| widget name | default chord | description |
|---|---|---|
| `accept-line` | <kbd>Enter</kbd> | validate syntax and execute command |
| `newline-and-indent` | <kbd>Ctrl+J</kbd>, <kbd>Shift+Enter</kbd> | insert newline with auto-indentation |
| `abort` / `interrupt` | <kbd>Ctrl+C</kbd> | cancel current edit buffer or terminate foreground job |
| `clear-screen` | <kbd>Ctrl+L</kbd> | clear terminal screen and redraw prompt |
| `edit-command-line` | <kbd>Alt+E</kbd> (Vi: `v`) | open current buffer in `$EDITOR` / `$VISUAL` |
| `toggle-help` | <kbd>F1</kbd> | display context-sensitive help popup for current command |

---

## vi mode keybindings & motions

switch between editing modes:

```fsh
setopt keymap vi       # switch to Vi modal editing
setopt keymap emacs    # switch to Emacs modeless editing (default)
```

### mode transitions

- **to Normal mode**: press <kbd>Esc</kbd> or <kbd>Ctrl+[</kbd>
- **to Insert mode**:
  - `i`: insert before cursor
  - `a`: append after cursor
  - `I`: insert at start of line
  - `A`: append at end of line
  - `o`: insert newline below and enter insert mode
  - `O`: insert newline above and enter insert mode
  - `s`: substitute character (delete char and enter insert mode)
  - `C`: change to end of line
  - `cc`: change entire line

---

## interactive ftui components

### sqlite history explorer (`Ctrl+H`)

an interactive full-screen TUI for analyzing command history:

```
┌─── History Explorer (SQLite) ───────────────────────────────────────────────────────────┐
│ Filter: git commit                                                    Total: 4,821 rows │
├──────┬─────────────────────┬──────┬──────────┬──────────────────────────────────────────┤
│ ID   │ Timestamp           │ Code │ Duration │ Command                                  │
├──────┼─────────────────────┼──────┼──────────┼──────────────────────────────────────────┤
│ 4821 │ 2026-08-24 16:15:22 │ 0    │ 42ms     │ git commit -am "docs: update language"   │
│ 4820 │ 2026-08-24 16:12:01 │ 0    │ 120ms    │ git commit -am "fix: resolver bug"       │
│ 4815 │ 2026-08-24 15:45:10 │ 1    │ 890ms    │ cargo test -p fshell-engine              │
└──────┴─────────────────────┴──────┴──────────┴──────────────────────────────────────────┘
```

- **interactive navigation**: <kbd>Up</kbd> / <kbd>Down</kbd> / <kbd>PageUp</kbd> / <kbd>PageDown</kbd>
- **search filter**: type to filter by command text, directory, or exit status
- **actions**: <kbd>Enter</kbd> to copy command into current prompt; <kbd>Ctrl+Y</kbd> to copy to system clipboard; <kbd>Esc</kbd> to exit

### fuzzy search & picker (`Ctrl+R`)

in-line fuzzy picker over history with real-time substring highlighting and instant buffer insertion.

### bottom status bar

displays real-time system and session telemetry (enabled via `setopt status_bar` or `$FSH_STATUS_BAR=1`):

```text
 ~/dev/fshell [main*] ── venv:(fsh-dev) ── jobs:0 ── 2026-08-24 16:20 ── [sess:default]
```

### interactive config tui (`config edit`)

built-in Ratatui interface for inspecting and toggling settings in `config.toml`:

```fsh
config edit
```

allows interactive navigation across option categories (General, Prompt, Capabilities, Colors, Aliases), with real-time documentation and immediate persistence.

---

## custom keybinding configuration (`bind`)

configure keybindings in `~/.config/fsh/init.fsh`:

```fsh
# bind key chord to widget
bind "ctrl-h" "history-explorer"
bind "alt-e" "edit-command-line"

# bind key to text macro
bind "ctrl-g" "git status\n"

# list all active keybindings
bind --list
```
