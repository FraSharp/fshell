# fshell (fsh)

### premise
so, i've been daily driving fshell for about 1 month, there were a lot of uncaught bugs and things that broke the parser completely, but now it's stable enough to
release. it's still very work in progress so i can't guarantee the same stability as zsh or fish (but it's pretty stable). PRs, issues and contributions are more than accepted.

i obviously used LLMs to make the design process, boilerplates and debugging less tedious. it is a tool and it should be used in the right way.

---

a structured-data unix shell designed to replace bash and zsh without the painful migration.

structured like nushell, familiar like zsh, clean like rust.

---

## table of contents

- [installation & quick start](#installation--quick-start)
- [why fshell](#why-fshell)
- [side-by-side: fsh vs bash / zsh](#side-by-side-fsh-vs-bash--zsh)
  - [1. process inspection & filtering](#1-process-inspection--filtering)
  - [2. file inspection & terminal table formatting](#2-file-inspection--terminal-table-formatting)
  - [3. json api query & extraction](#3-json-api-query--extraction)
  - [4. multi-archive extraction](#4-multi-archive-extraction)
  - [5. safe destructive commands](#5-safe-destructive-commands)
- [interactive line editor & developer features](#interactive-line-editor--developer-features)
  - [instant startup](#instant-startup)
  - [categorized tab completion & parameter hints](#categorized-tab-completion--parameter-hints)
  - [3-tier predictive autosuggestions](#3-tier-predictive-autosuggestions)
  - [sqlite history explorer (`Ctrl+H`) & recall (`Ctrl+R`)](#sqlite-history-explorer-ctrlh--recall-ctrlr)
  - [aliases with instant backspace undo](#aliases-with-instant-backspace-undo)
  - [sub-millisecond git prompt & transient mode](#sub-millisecond-git-prompt--transient-mode)
  - [terminal multiplexer (`mux`)](#terminal-multiplexer-mux)
  - [visual theme engine & config tui (`config edit`)](#visual-theme-engine--config-tui-config-edit)
- [polyglot posix engine (zero migration pain)](#polyglot-posix-engine-zero-migration-pain)
- [scripting in `.fsh`](#scripting-in-fsh)
- [built-in utilities](#built-in-utilities)
- [pipeline operators reference](#pipeline-operator-reference)
- [security, guardrails & sandboxing](#security-guardrails--sandboxing)
- [systems architecture & performance](#systems-architecture--performance)
- [documentation index](#documentation-index)
- [contributing & test suite](#contributing--test-suite)
- [license](#license)

---

## installation & quick start

### installation

#### with cargo (recommended)

```bash
git clone https://github.com/FraSharp/fshell.git
# install directly into ~/.cargo/bin
cargo install --path .
```

#### build from source

```bash
git clone https://github.com/FraSharp/fshell.git
cd fshell
cargo build --release
```

the compiled binary is at `target/release/fsh`.

### basic usage

```bash
# launch interactive shell
fsh

# run an inline command and exit
fsh -c 'ps | filter cpu > 20.0 | map pid command'

# run a .fsh script
fsh deploy.fsh

# run a legacy posix shell script
fsh --posix setup.sh

# launch in strict capability mode
fsh -s
```

---

## why fshell

traditional shells pass flat, unformatted byte streams between processes. extracting information means writing brittle chains of `awk`, `sed`, `grep`, and `cut` to parse strings that were already structured in the kernel before being formatted into text.

nushell showed that structured pipelines are the right abstraction, but introduced an unfamiliar syntax that breaks shell muscle memory and abandons POSIX compatibility.

fshell keeps standard shell syntax and muscle memory, but passes typed `Arc<Val>` records through asynchronous pipelines. everything you need—completions, history database, git prompts, multiplexer, and syntax highlighting—is compiled into a single binary that boots in sub-millisecond time.

---

## side-by-side: fsh vs bash / zsh

### 1. process inspection & filtering

filtering processes consuming more than 50% CPU and projecting PID and command:

```bash
# bash / zsh: spawns 3 external processes, fragile column splitting
ps aux | awk '{if ($3 > 50.0) print $2, $11}' | head -n 5
```

```fsh
# fsh: typed fields, in-process keyword stages, zero fork overhead
ps | filter cpu > 50.0 | map pid command | limit 5
```

---

### 2. file inspection & terminal table formatting

finding regular files larger than 1KB, sorted by size descending, rendered as an auto-sized table:

```bash
# bash / zsh: complex find, xargs, awk and column pipeline
find . -maxdepth 1 -type f -size +1K | xargs ls -lh | awk '{print $9, $5}' | column -t
```

```fsh
# fsh: structured Map stream formatted directly to a terminal table
ls | filter type == "file" and size > 1K | sort size desc | limit 5 | @table
```

```text
| name            | type | size   | last_modified             | git_status | is_executable |
|-----------------|------|--------|---------------------------|------------|---------------|
| Cargo.lock      | file | 100605 | 2026-08-24T16:20:12+02:00 | clean      | false         |
| README.md       | file | 16252  | 2026-08-24T16:50:35+02:00 | clean      | false         |
| Cargo.toml      | file | 5824   | 2026-08-24T14:45:30+02:00 | clean      | false         |
```

---

### 3. json api query & extraction

querying a REST API and filtering records:

```bash
# bash: requires jq or python
curl -s https://api.github.com/repos/FraSharp/fshell/issues | jq -r '.[] | select(.comments > 5) | .title'
```

```fsh
# fsh: native boundary operator parsing JSON into typed maps
curl -s https://api.github.com/repos/FraSharp/fshell/issues | @json | filter comments > 5 | map title
```

---

### 4. multi-archive extraction

extracting an archive without remembering tar flags:

```bash
# bash / zsh: different flags for every format
tar -zxvf archive.tar.gz
tar -jxvf archive.tar.bz2
unzip bundle.zip
7z x package.7z
```

```fsh
# fsh: single command auto-detects archive format
extract archive.tar.gz
extract bundle.zip
extract package.7z
```

---

### 5. safe destructive commands

avoiding catastrophic shell typos:

```bash
# bash / zsh: executes immediately and destroys your filesystem
rm -rf /tmp / usr/local/bin
```

```text
# fsh: intercepts catastrophic commands and prompts for confirmation
Caution: destructive command detected:
    rm -rf /tmp / usr/local/bin

Execute this command? [y/N]:
```

normal everyday commands run with zero friction.

---

## interactive line editor & developer features

### instant startup

fshell boots in **sub-millisecond time**. because completions, fuzzy matchers, theming, and database drivers are compiled natively into the binary rather than sourced from dozens of shell scripts at launch, opening new terminal windows is instantaneous.

### categorized tab completion & parameter hints

pressing <kbd>Tab</kbd> opens a multi-column completion menu powered by `nucleo_matcher` fuzzy search:
- **13 categorized groups**: completions are grouped into `Dirs`, `Files`, `Commands`, `Builtins`, `Aliases`, `Functions`, `Variables`, `Jobs`, `Flags`, `Pipelines`, `Keywords`, `History`, and `Git Refs`.
- **parameter hints & signatures**: displays parameter types and argument positions directly below the cursor as you type.

### 3-tier predictive autosuggestions

ghost text suggestions are calculated asynchronously across three priority sources:
1. exact SQLite history matches
2. cached directory paths and local files
3. command-specific syntax and flag predictions

press <kbd>Right</kbd> to accept the full suggestion, or <kbd>Alt+Right</kbd> to accept word-by-word.

### sqlite history explorer (`Ctrl+H`) & recall (`Ctrl+R`)

all executed commands are stored in an embedded SQLite database (`~/.config/fsh/history.db`):
- **full-screen explorer (<kbd>Ctrl+H</kbd>)**: interactive search table displaying execution durations in milliseconds, exit codes, directory paths, and exact timestamps.
- **inline fuzzy search (<kbd>Ctrl+R</kbd>)**: real-time substring search through past commands.
- **aborted command recall (<kbd>Alt+R</kbd>)**: restores commands that were cancelled with <kbd>Ctrl+C</kbd>.

### aliases with instant backspace undo

aliases expand inline when typed so you can verify what is about to run. if you want to collapse the expansion back into the alias name, simply press <kbd>Backspace</kbd> once.

### sub-millisecond git prompt & transient mode

- **native git index inspection**: git branch names, dirty status, staged changes, and ahead/behind counts are read directly from git's binary indices without spawning slow `git status` subprocesses.
- **transient prompt**: previous prompts collapse into a compact single line upon pressing <kbd>Enter</kbd>, keeping your scrollback clean and readable.

### terminal multiplexer (`mux`)

split terminal panes and manage workspace tabs directly inside the shell without needing `tmux` or `zellij`:

```fsh
mux split -h            # split pane horizontally
mux split -v            # split pane vertically
mux new-tab             # create a new tab
```

### visual theme engine & config tui (`config edit`)

- **24-bit truecolor theming**: built-in syntax highlighting palettes (Catppuccin, Gruvbox, Nord) with custom TOML theme support (`prompt.toml`).
- **interactive config tui**: run `config edit` to open a full-screen Ratatui interface for visually toggling shell options, switching themes, and editing aliases.

---

## polyglot posix engine (zero migration pain)

fshell includes a dedicated POSIX.1-2024 compliance engine (`fshell-posix`). you do not need to rewrite your shell scripts or migration workflows:

```fsh
let release = "v1.2.0"

# embed legacy posix shell code directly in .fsh scripts
sh {
    if [ -z "$release" ]; then
        echo "missing release" >&2
        exit 1
    fi
}

# activate python virtual environments seamlessly
source --bash .venv/bin/activate
```

both `sh { ... }`, `posix { ... }`, and `bash { ... }` blocks execute in-process against the shared environment without spawning `/bin/sh` or `/bin/bash` subprocesses.

---

## scripting in `.fsh`

`.fsh` is a clean scripting language with Rust-like syntax:

```fsh
# variables with gradual typing
let target: String = "./dist"
let port: Int = 8080
let is_prod: Bool = true

# typed functions with parameter validation
fn deploy(service: String, port: Int) -> Bool {
    echo "deploying {service} on port {port}"
    return true
}

# pattern matching
match env["STAGE"] {
    "prod" => echo "production deployment",
    "staging" => echo "staging deployment",
    _ => echo "development environment",
}

# reactive streams (re-evaluates automatically when dependencies update)
let dirty_files $= ls | filter git_status == "modified"

# error handling
try {
    cat /nonexistent/file.txt
} catch err {
    echo "caught error: {err}"
}

# heredocs with interpolation
cat <<EOF > config.toml
[server]
port = {port}
host = "127.0.0.1"
EOF
```

---

## built-in utilities

everything is built into the binary:

- **`ls`**: git-aware file listing emitting typed Maps with size, permissions, and git status.
- **`ps`**: process inspector with typed CPU%, Memory, PID, User, and Command fields.
- **`files`**: recursive directory scanner emitting structured records (replaces `find`).
- **`z <dir>` / `zi`**: SQLite-backed frecency directory jumping with interactive fuzzy selector.
- **`serve <port>`**: launch an instant local HTTP static file server.
- **`vault`**: local encrypted secrets store for API tokens and passwords.
- **`extract <file>`**: auto-detects and extracts archives (`.tar.gz`, `.zip`, `.tar.xz`, `.7z`, etc.).
- **`string`**: string manipulation built-in (`upper`, `lower`, `trim`, `split`, `length`, `replace`).
- **`diff`**: structured visual diffing.
- **`json` & `csv`**: jq-like query syntax and CSV transformations.
- **`explain <pipeline>`**: plain-English pipeline explainer.

---

## pipeline operators reference

| stage / operator | description | example |
|---|---|---|
| `filter <expr>` | filter stream items by boolean condition | `ls \| filter size > 1048576` |
| `map <cols...>` | project specific fields or compute expressions | `ps \| map pid command (cpu / 100.0)` |
| `sort [col] [asc\|desc]` | sort records by property (bounded memory) | `ls \| sort size desc` |
| `grep <pattern>` | filter items matching string or regex | `cat app.log \| grep "ERROR 500"` |
| `mark <pattern>` | highlight matching rows without dropping items | `cat build.log \| mark "WARN"` |
| `count` | aggregate item count into an integer | `ls \| filter size == 0 \| count` |
| `limit <N>` | slice stream to first N items | `ps \| sort cpu desc \| limit 5` |
| `traverse <edge>` | traverse edges on `Val::ObjectGraph` structures | `deps \| traverse "depends_on"` |
| `hash [-a 256\|512]` | compute whole-stream or per-record hashes | `cat archive.tar \| hash` |
| `@table` | format records as Unicode/ASCII terminal table | `ps \| limit 10 \| @table` |
| `@json` | serialize records as JSON / parse input JSON | `ps \| @json` |
| `@yaml` | serialize records as YAML documents | `ls \| limit 3 \| @yaml` |
| `@msgpack` | encode records as binary MessagePack blobs | `records \| @msgpack > data.mpk` |
| `@csv` | format records as CSV with inferred headers | `ps \| map pid command \| @csv` |
| `@text` | extract plain text string representation | `ls \| map name \| @text` |
| `@bar` | render horizontal terminal distribution bar chart | `ps \| map command cpu \| limit 5 \| @bar` |

---

## security, guardrails & sandboxing

- **destructive command confirmation**: catastrophic commands (`rm -rf /`, formatting root devices, raw block writes) are intercepted before execution and require confirmation in the REPL (or `unsafe <cmd>` in non-interactive scripts).
- **kernel sandboxing**: external subprocesses can be isolated via Linux Landlock rulesets or macOS SBPL (Seatbelt) policies compiled directly into `pre_exec` fork hooks.
- **capability-based authorization**: granular capability tokens (`ResourceHandle`) restrict filesystem access, network sockets, environment variables, and process spawning.
- **scoped elevation**: grant temporary privileges for specific blocks with `with caps(...) { ... }`.

---

## systems architecture & performance

- **zero-subprocess keyword stages**: core pipeline stages (`filter`, `map`, `sort`, `grep`, `count`, `limit`, `traverse`, `hash`) are engine-level language keywords evaluated in-process with zero subprocess fork overhead.
- **interned identifiers (`Ustr`)**: map keys use string interning. looking up `.size` or `.name` across 100,000 files in a pipeline is an **$O(1)$ pointer comparison** rather than repeated string hashing or allocations.
- **asynchronous dataflow**: pipeline items flow through bounded tokio channels wrapped in `Arc<Val>`, providing automatic backpressure.
- **multi-call binary routing**: single binary `fsh` acts as shell, script runner, and utility suite (`ls`, etc.) depending on invocation context.

---

## documentation index

detailed documentation for every subsystem is in [`docs/`](docs/):

- **[language reference](docs/LANGUAGE.md)**: complete `.fsh` syntax, types, expressions, pattern matching, reactive variables (`$=`), and modifiers.
- **[architecture](docs/ARCHITECTURE.md)**: 14-crate workspace design, memory layout, tokio channel streaming, and multicall binaries.
- **[pipelines](docs/PIPELINES.md)**: stream backpressure, `PipelinePayload` internals, all stage transformers, boundary operators, and `pipefail`.
- **[posix compatibility](docs/POSIX-COMPATIBILITY.md)**: `fshell-posix` engine, 4-phase word expansion, arithmetic, and virtualenv sourcing.
- **[security & capabilities](docs/SECURITY.md)**: capability tokens, 3-tier permissions, Landlock (Linux) / SBPL (macOS) sandboxing, and safety prompts.
- **[built-in commands](docs/BUILTINS.md)**: complete reference for all ~117 builtins.
- **[configuration](docs/CONFIGURATION.md)**: `config.toml`, `prompt.toml`, shell options, and lifecycle hooks (`precmd`, `preexec`, `chpwd`).
- **[migration guide](docs/MIGRATION.md)**: side-by-side migration reference from Bash, Zsh, and Fish.
- **[lock ordering](docs/LOCK-ORDERING.md)**: authoritative lock hierarchy and runtime deadlock prevention.
- **[line editor & widgets](docs/WIDGETS.md)**: editor widgets, Vi modes, SQLite history explorer (<kbd>Ctrl+H</kbd>), and status bar.

---

## contributing & test suite

```bash
# build release binary
cargo build --release

# run all unit and integration tests
cargo test

# run a specific integration test suite
cargo test --test pipelines_tests

# run clippy linter across all targets
cargo clippy --all-targets -- -D warnings

# check code formatting
cargo fmt --check
```

---

## license

[GPLv3](LICENSE)
