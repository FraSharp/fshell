# fshell built-in commands reference

this document is the definitive reference for all built-in commands in fshell (`fsh`).

---

## table of contents

- [built-in architecture & handler type](#built-in-architecture--handler-type)
- [navigation & directory stack](#navigation--directory-stack)
- [environment & configuration](#environment--configuration)
- [filesystem & file inspection](#filesystem--file-inspection)
- [process management & job control](#process-management--job-control)
- [data streams & transformation](#data-streams--transformation)
- [shell introspection & assistance](#shell-introspection--assistance)
- [terminal multiplexing & integrations](#terminal-multiplexing--integrations)
- [feature-gated builtins](#feature-gated-builtins)
  - [sandbox & capabilities (`feature = "sandbox"`)](#sandbox--capabilities-feature--sandbox)
  - [secrets vault (`feature = "vault"`)](#secrets-vault-feature--vault)
  - [ai assistant (`feature = "ai"`)](#ai-assistant-feature--ai)
  - [fuzzy filter & replace (`features = "ff", "replace"`)](#fuzzy-filter--replace-features--ff-replace)
- [posix standard builtins](#posix-standard-builtins)

---

## built-in architecture & handler type

a command is implemented as a builtin if:
1. it emits structured `Val` objects (`Val::Map`, `Val::List`, `Val::Int`) rather than opaque byte streams, or
2. it requires direct access to the shell runtime (`Env`, working directory, capabilities, or job control).

### handler signature

all builtins implement the `BuiltinHandler` signature (`crates/fshell-engine/src/lib.rs`):

```rust
Arc<dyn Fn(
    Option<PipeStream>, // Upstream input stream (if piped)
    Vec<Val>,           // Evaluated argument list
    &Env,               // Runtime environment reference
    PipeSender,         // Downstream output channel
) -> Result<(), ShellError> + Send + Sync>
```

builtins are registered into the environment at startup via `fshell_builtins::init(&env)` or dynamically via `env.register_builtin(name, handler)`.

---

## navigation & directory stack

### `cd`
changes the current working directory and updates `$PWD`, `$OLDPWD`, and the frecency database.

```fsh
cd /usr/local/bin
cd -            # switch to previous directory ($OLDPWD)
cd ~            # switch to home directory
```

### `pwd`
prints the absolute path of the current working directory.

### `z` / `zi`
frecency-based directory jumping powered by SQLite (`frecency.db`).

```fsh
z fshell        # jump to highest-scoring match containing "fshell"
zi              # open interactive fuzzy selector for frequent directories
```

### `pushd` / `popd` / `dirs`
manages the shell directory stack:
- `pushd <dir>`: pushes current directory to stack and switches to `<dir>`.
- `popd`: pops the top directory off stack and navigates to it.
- `dirs`: prints current directory stack as a `Val::List`.

---

## environment & configuration

### `export`
exports a variable to child processes.

```fsh
export EDITOR="vim"
export PATH="/custom/bin:$PATH"
```

### `env`
inspects or modifies environment variables. when run without arguments, returns the environment map as structured `Val::Map` entries.

### `set` / `unset`
- `set <name>=<val>`: assigns a shell variable.
- `unset <name>`: removes a variable from the environment.

### `setopt` / `unsetopt`
toggles fshell runtime options (`crates/fshell-engine/src/options.rs`):

```fsh
setopt errexit          # exit on error (set -e)
setopt pipefail         # non-zero exit if any pipeline stage fails
setopt noexec           # parse syntax without executing
setopt quiet_aliases    # silence shadow warnings
unsetopt did_you_mean   # disable "did you mean" suggestions
```

### `config`
inspects and edits shell configuration files (`config.toml`):

```fsh
config get prompt.theme
config set prompt.transient true
config edit
```

### `load_env_file` / `eval_direnv`
- `load_env_file [path]`: loads key-value pairs from `.env` files.
- `eval_direnv`: executes local `.envrc` policies via `direnv`.

---

## filesystem & file inspection

### `ls`
git-aware directory listing emitting `Val::Map` records with typed fields (`name: String`, `type: String` ("file"/"dir"), `size: Int`, `modified: DateTime`, `git_status: String`, `is_executable: Bool`, `is_symlink: Bool`):

```fsh
ls
ls -l /etc
ls | filter type == "file" and size > 1K | sort size desc | @table
```

### `files`
recursively scans directory trees and yields structured file metadata records.

```fsh
files src | filter name ~ r"\.rs$" | map name size
```

### `mkdir` / `touch` / `cat`
- `mkdir [-p] <path>`: creates directories.
- `touch <path>`: updates access/modification timestamps or creates empty file.
- `cat [files...]`: streams file contents or standard input.

### `head` / `tail` / `uniq`
- `head [-n N]`: yields the first `N` records or lines.
- `tail [-n N]`: yields the last `N` records or lines.
- `uniq`: filters adjacent duplicate values from the stream.

### `extract`
auto-detects archive format (`.tar.gz`, `.tar.bz2`, `.zip`, `.7z`, `.tar.xz`) and extracts it into the target directory in a single command.

```fsh
extract bundle.tar.gz
```

---

## process management & job control

### `ps`
inspects running system processes and yields structured `Val::Map` items (`pid: Int`, `ppid: Int`, `cpu: Float`, `mem: Float`, `command: String`, `user: String`):

```fsh
ps | filter cpu > 10.0 | sort cpu desc | @table
```

### `jobs` / `fg` / `bg` / `disown`
manages asynchronous background jobs:
- `jobs`: lists all active background jobs with IDs and statuses.
- `fg [%id]`: brings a background job to the foreground.
- `bg [%id]`: resumes a stopped job in the background.
- `disown [%id]`: detaches a job from the shell's lifecycle.

### `kill`
sends signals to processes by PID or job ID.

```fsh
kill -9 1234
kill -s SIGTERM %1
```

### `wait`
waits for background processes or jobs to complete.

### `sleep`
pauses execution for a specified duration (`10s`, `500ms`, `2m`).

### `exec`
replaces the current shell process with the specified command.

---

## data streams & transformation

### `json`
parses or queries raw JSON data using jq-like path expressions:

```fsh
cat payload.json | json .users[0].name
```

### `csv`
parses CSV input into structured maps or formats maps into CSV.

### `diff`
compares two files or stream values and outputs visual diff records.

### `select`
projects specific columns from incoming maps (companion to `map`).

### `serve`
starts an instant local HTTP static file server in the current directory on a given port:

```fsh
serve 8080
```

### `string`
comprehensive string transformations:

```fsh
string upper "hello"           # HELLO
string lower "HELLO"           # hello
string trim "  data  "         # data
string split "," "a,b,c"       # ["a", "b", "c"]
string length "antigravity"    # 12
string replace "foo" "bar" "foobar" # barbar
```

### `watch`
periodically executes a command and displays updated output in the terminal.

---

## shell introspection & assistance

### `help`
displays documentation for built-in commands and syntax topics:

```fsh
help
help filter
help setopt
```

### `which` / `type`
- `which <name>`: finds the binary path of an external command.
- `type <name>`: introspects a name, identifying whether it is a builtin, alias, user function, keyword, or external executable.

### `explain`
breaks down complex pipelines and explains each stage in plain language:

```fsh
explain "ps | filter cpu > 50.0 | map pid command"
```

### `reload`
reloads configuration or performs a full process handoff (`reload --full`) while preserving state.

### `alias`
defines or lists command aliases:

```fsh
alias g="git"
alias ll="ls -l"
```

### `hook`
manages event hooks (`precmd`, `preexec`, `chpwd`):

```fsh
hook chpwd update_status_bar
```

### `profile`
controls execution profiling:

```fsh
profile report          # prints latency breakdown across subsystems
```

### `prompt` / `theme`
- `prompt [cmd]`: manages segment prompt rendering.
- `theme [name]`: switches the active color theme.

### `session`
saves or restores interactive session workspaces.

---

## terminal multiplexing & integrations

### `mux`
terminal multiplexer commands for split panes and windows:

```fsh
mux split -h            # split pane horizontally
mux split -v            # split pane vertically
mux new-tab             # create new tab
```

### `direnv_init` / `zoxide_init` / `starship_init` / `fzf_init`
helper builtins that emit initialization scripts for third-party shell tools.

---

## feature-gated builtins

### sandbox & capabilities (`feature = "sandbox"`)

builtins for managing the capability security subsystem:
- `caps-profile`: loads or inspects YAML capability profiles (`caps.yaml`).
- `caps-audit`: displays the audit log of all capability checks.
- `strict`: toggles strict capability authorization.
- `sandbox <profile> <command>`: executes a command inside a Landlock (Linux) or SBPL (macOS) sandbox.
- `fs-read`, `fs-write`, `fs-readwrite`: path capability helpers.
- `net-connect`, `net-all`: network capability helpers.
- `env-read`, `env-write`: environment variable capability helpers.
- `process-spawn`: process spawning capability helper.
- `unsafe`: runs an operation bypassing reactive query safety checks.

### secrets vault (`feature = "vault"`)

local encrypted secrets management backed by system keychains or AES-256-GCM:

```fsh
vault set API_KEY "secret_value"
vault get API_KEY
vault list
```

### ai assistant (`feature = "ai"`)

natural language translation and shell assistant:

```fsh
ai "find all files larger than 100mb modified in the last 2 days"
ai chat
```

### fuzzy filter & replace (`features = "ff", "replace"`)

- `ff`: interactive fuzzy filter for incoming stream records.
- `replace <search> <replace>`: stream search and replace across map values.

---

## posix standard builtins

built-in commands for POSIX compatibility:
- `test` / `[`: POSIX conditional expression evaluation.
- `printf <format> [args...]`: formatted output.
- `echo [-n] [-e] [args...]`: standard string output.
- `true` / `false`: exit with status 0 or 1.
- `trap [handler] [signals...]`: signal trap management.
- `funced <name>` / `funcsave <name>`: interactive function editing and persistence.
