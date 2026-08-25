# fshell configuration reference

this document describes fshell (`fsh`) configuration files, shell options, prompt customization, and event hooks.

---

## table of contents

- [configuration directory & file layout](#configuration-directory--file-layout)
- [cli flags & startup options](#cli-flags--startup-options)
- [shell options (`config.toml`)](#shell-options-configtoml)
  - [boolean options (`setopt` / `unsetopt`)](#boolean-options-setopt--unsetopt)
  - [value options (`config set`)](#value-options-config-set)
- [prompt customization (`prompt.toml`)](#prompt-customization-prompttoml)
  - [segment types](#segment-types)
  - [separators & styling](#separators--styling)
  - [sample configuration](#sample-configuration)
- [event hooks (`precmd`, `preexec`, `chpwd`)](#event-hooks-precmd-preexec-chpwd)
- [environment variables](#environment-variables)

---

## configuration directory & file layout

fshell resolves its configuration directory in the following order of precedence:
1. `$FSH_CONFIG_DIR`
2. `$XDG_CONFIG_HOME/fsh` (if `$XDG_CONFIG_HOME` is set)
3. `$FSH_HOME/.config/fsh` or `$HOME/.config/fsh`

```
~/.config/fsh/
├── init.fsh        # startup script: aliases, custom functions, hooks
├── config.toml     # persistent shell options & limits
├── prompt.toml     # segment-based prompt layout & colors
├── history.db      # sqlite command history
├── frecency.db     # sqlite directory frecency database (z / zi)
└── caps.json       # persistent capability tokens
```

---

## cli flags & startup options

| flag | short | value type | description |
|---|---|---|---|
| `script` | (pos) | `Option<String>` | path to `.fsh` or POSIX script to execute non-interactively |
| `--command` | `-c` | `Option<String>` | run inline command string and exit |
| `--strict` | `-s` | `bool` | strict capability mode — deny all access unless explicitly granted |
| `--posix` | | `bool` | run in POSIX compatibility mode via `fshell-posix` |
| `--login` | `-l` | `bool` | run as login shell (sources host login profiles) |
| `--resume` | `-r` | `Option<String>` | resume saved session by ID or show interactive picker |
| `--error-format` | | `String` | error format: `graphical`, `compact`, or `json` |
| `--no-color` | | `bool` | disable colored terminal output |
| `--no-dym` | | `bool` | disable "did you mean" typo suggestions |
| `--suggestion-mode`| | `String` | suggestion mode: `blocking` or `deferred` |
| `--handoff` | | `String` | internal: restore state from JSON handoff file during `reload --full` |

---

## shell options (`config.toml`)

options are persisted in `config.toml` and can be inspected or toggled at runtime using `setopt`, `unsetopt`, or `config`.

### boolean options (`setopt` / `unsetopt`)

```fsh
setopt errexit      # enable option
unsetopt pipefail   # disable option
```

| option | default | description |
|---|---|---|
| `autocd` | `true` | typing a directory path directly `cd`s into it |
| `pipefail` | `false` | pipeline returns failure if any stage fails |
| `errexit` | `false` | exit immediately when a command or pipeline returns non-zero (`set -e`) |
| `nounset` | `true` | treat unset variable references as errors (`set -u`) |
| `did_you_mean` | `true` | suggest corrections when commands are not found |
| `error_color` | `true` | colorize error diagnostics |
| `json_auto_parse`| `true` | automatically parse external JSON stdout into `Val` objects |
| `status_bar` | `false` | display interactive bottom status bar in FTUI |
| `confirm_destructive` | `true` | require confirmation for destructive commands (`rm -rf /` etc.) |
| `quiet_aliases` | `false` | suppress warnings when an alias shadows a builtin or user function |
| `nullglob` | `false` | glob patterns matching nothing expand to empty rather than literal pattern |
| `nocaseglob` | `false` | case-insensitive pathname expansion |
| `noclobber` | `false` | prevent file overwrites via `>` redirection |
| `noexec` | `false` | parse syntax and validate AST without executing |
| `xtrace` | `false` | print commands before execution (`set -x`) |
| `verbose` | `false` | print input lines as they are read |
| `ignoreeof` | `false` | prevent shell exit on `Ctrl+D` (EOF) |
| `autopushd` | `false` | automatically push old directories onto stack on `cd` |
| `histignoredups`| `false` | ignore duplicate consecutive commands in history |
| `cdable_vars` | `false` | treat variable values as directory targets for `cd` |
| `notify` | `false` | report background job status changes immediately |
| `sandbox_all` | `false` | apply default subprocess sandbox to all external commands |

### value options (`config set`)

```fsh
config set pipeline_channel_size 200
config set sort_max_items 50000
```

| option | default | type | description |
|---|---|---|---|
| `pipeline_channel_size` | `100` | `usize` | buffer capacity of inter-stage tokio channels |
| `sort_max_items` | `100000` | `usize` | max records buffered in memory during `sort` |
| `stderr_max_bytes` | `1048576`| `usize` | max bytes retained from subprocess stderr |
| `notify_threshold` | `10` | `u64` | execution duration threshold (seconds) for task notifications |
| `sandbox_mode` | `"prompt"` | `String` | sandbox enforcement mode (`prompt`, `deny-all`, `monitor`, `off`)|
| `error_format` | `"graphical"`| `String` | error display style (`graphical`, `compact`, `json`) |
| `suggestion_mode` | `"deferred"` | `String` | suggestion engine behavior (`blocking`, `deferred`) |
| `clear_on_reload` | `"ask"` | `String` | clear screen on reload (`ask`, `always`, `never`) |
| `theme` | `"default"` | `String` | active syntax highlighting and terminal UI theme |

---

## prompt customization (`prompt.toml`)

fshell features a modular, segment-based prompt engine configured in `~/.config/fsh/prompt.toml`.

### segment types

segments can be arranged across left, right, or multi-line prompts:

| segment | description |
|---|---|
| `User` | current user login name |
| `Host` | current hostname |
| `Pwd` | current working directory (supports truncation and git-root relative paths) |
| `GitBranch` | active git branch name or detached HEAD SHA |
| `GitStatus` | git worktree indicators (dirty, staged, ahead, behind, stash) |
| `ExitCode` | status code of previous command (shown conditionally on failure) |
| `Duration` | execution duration of last command (if exceeding threshold) |
| `Jobs` | number of active background jobs |
| `Char` | prompt character (e.g. `>` or `#` for root) |
| `Time` / `Date` | current time or date formatted with chrono |
| `Venv` | active Python virtualenv environment name |
| `Aws` / `Kube` | active cloud credentials or kubernetes context |
| `Shlvl` | current subshell nesting level |
| `Custom` | arbitrary shell expression or script evaluation |

### separators & styling

segments support powerline glyphs and color styling:
- **separator styles**: `Arrow`, `Chevron`, `Flame`, `Pipe`, `Slash`, `Dots`, `None`, `Custom(String)`
- **styling fields**: `fg`, `bg`, `bold`, `italic`, `prefix`, `suffix`, `hide_on_zero`, `hide_when_clean`, `show_only_in_repo`

### sample configuration

```toml
[prompt]
transient = true
multiline = false

[[left]]
type = "Pwd"
fg = "#ffffff"
bg = "#3b82f6"
prefix = " "
suffix = " "

[[left]]
type = "GitBranch"
fg = "#000000"
bg = "#10b981"
prefix = " "
suffix = " "
show_only_in_repo = true

[[left]]
type = "Char"
fg = "#3b82f6"
bg = "None"

[[right]]
type = "Duration"
fg = "#f59e0b"
bg = "None"

[[right]]
type = "ExitCode"
fg = "#ef4444"
bg = "None"
hide_on_zero = true
```

---

## event hooks (`precmd`, `preexec`, `chpwd`)

hooks execute custom functions on specific lifecycle events:

```fsh
# ~/.config/fsh/init.fsh

# executed before rendering the prompt
fn on_precmd {
    # refresh status indicators
}
hook precmd on_precmd

# executed before a command runs
fn on_preexec {
    # start timer or audit logging
}
hook preexec on_preexec

# executed whenever working directory changes
fn on_chpwd {
    # auto-list or direnv check
}
hook chpwd on_chpwd
```

you can also register signal hooks using the language-level `on` statement:

```fsh
on exit {
    echo "cleaning up..."
}
```

---

## environment variables

| variable | description |
|---|---|
| `FSH_CONFIG_DIR` | overrides base directory for configuration files |
| `FSH_CACHE_DIR` | overrides directory for cached AST and temporary files |
| `FSH_HOME` | overrides root home directory for fshell |
| `FSH_PROMPT` | overrides prompt theme path or inline specification |
| `FSH_KEYBINDING_MODE` | sets editor keymap (`emacs` or `vi`) |
| `FSH_STATUS_BAR` | enables/disables the bottom status bar (`1` or `0`) |
| `FSH_PIPELINE_CHANNEL_SIZE` | default channel capacity for pipeline stages |
| `FSH_CNF_DEBUG` | enables verbose debug logging for "did you mean" resolution |
