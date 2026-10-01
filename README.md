# fshell (fsh)

a structured-data unix shell.

fshell keeps the syntax you already type — pipes, redirections, `&&`, globs, job control — and moves typed values between pipeline stages instead of raw bytes. `ls`, `ps` and friends emit records whose fields you can filter and project directly; `@json`, `@csv`, `@yaml` and `@table` convert at the boundaries.

a POSIX compatibility engine is bundled for the bash/zsh scripts you already have, so migrating is optional rather than a rewrite.

**status:** work in progress. it is daily-drivable, but bash and zsh are still more battle-tested; the honest gaps are listed in [what's rough today](#whats-rough-today).

---

## what you get

- **typed pipelines** — stages carry `Map`, `List`, `Int`, `String`, `Blob` and friends, not text. `filter size > 1K`, `sort cpu desc`, `map pid command` work on fields; no `awk`/`cut` column splitting.
- **your existing scripts still run** — `fsh --posix script.sh` executes through the bundled POSIX engine, and `sh { ... }` runs POSIX code in-process against the same environment.
- **one binary** — line editor, completions, SQLite history, git-aware prompt and themes are compiled in; nothing is sourced at startup.
- **stages don't fork** — `filter`, `map`, `sort`, `grep`, `count`, `limit`, `traverse` and `hash` are language keywords evaluated in-process.
- **unix tools that understand structure** — `ls`, `ps`, `files`, `z`, `vault`, `extract`, `serve`, `string`, `diff`, plus `json` and `csv` helpers.
- **safety rails** — catastrophic commands are blocked (or confirmed interactively), and external processes can run under a Landlock (Linux) or Seatbelt (macOS) sandbox with capability gating.
- **one language for prompt and scripts** — anything you type works in a `.fsh` file and the other way round.

---

## installation

### requirements

- linux or macOS
- a stable Rust toolchain ([rustup.rs](https://rustup.rs))
- a C compiler (the bundled SQLite and stacker builds need one; on macOS it ships with the Xcode command line tools)

the default build is minimal — parser, engine, POSIX frontend and the core builtins — and needs no libraries beyond the system toolchain. the `full` feature set adds the archive extraction (`extract`), secrets (`vault`), assistant (`ai`), `http`, `sql`, `chart`, `notify`, fuzzy filter (`ff`), `replace` and sandbox builtins, and links libarchive statically:

- ubuntu / debian: `sudo apt install clang libclang-dev pkg-config libarchive-dev liblzma-dev libzstd-dev liblz4-dev libb2-dev`
- macOS: `brew install pkg-config libarchive libb2 xz zstd lz4`

### build from source

```bash
git clone https://github.com/FraSharp/fshell
cd fshell
cargo build --release                   # minimal; binary at target/release/fsh
cargo install --path .                  # ...or install it into ~/.cargo/bin
cargo install --path . --features full  # everything above, with the native packages installed
```

there are no prebuilt binaries yet; release archives for linux and macOS are produced by the release workflow on `v*` tags and carry the full feature set.

### use it as your login shell

```bash
fsh_path="$(command -v fsh)"   # confirm the absolute path is executable
grep -Fx "$fsh_path" /etc/shells   # login tools normally require this entry
chsh -s "$fsh_path"
```

if the path is missing from `/etc/shells`, add it using your system's administrator procedure first (the file is administrator-owned on macOS and most Linux distributions). sign out and back in to exercise the login path; to revert, `chsh -s /bin/zsh` (or another shell you know is installed).

---

## quick start

```bash
fsh                            # interactive shell
fsh -c 'ps | filter cpu > 20.0 | @table'   # run a pipeline and exit
fsh deploy.fsh                 # run a .fsh script
fsh --posix setup.sh           # run an existing posix script
fsh -s                         # start with capability checks enabled
```

---

## side by side with bash / zsh

### 1. process inspection

filtering processes by CPU and projecting fields:

```bash
# bash / zsh: spawns external processes and parses columns by position
ps aux | awk '{if ($3 > 1.0) print $2, $11, $3}' | head -n 5
```

```fsh
# fsh: typed fields, in-process stages
ps | filter cpu > 1.0 | sort cpu desc | map pid command cpu | limit 5 | @table
```

```text
| pid   | command                                        | cpu  |
|-------|------------------------------------------------|------|
| 39063 | command-code                                   | 14.8 |
| 4116  | /Applications/cmux.app/Contents/MacOS/cmux     | 6.5  |
| 90016 | /bin/sh -c ...                                 | 2.1  |
```

### 2. file inspection & terminal tables

finding regular files larger than 1KB, sorted by size, rendered as a table:

```bash
# bash / zsh: find, xargs, awk and column
find . -maxdepth 1 -type f -size +1K | xargs ls -lh | awk '{print $9, $5}' | column -t
```

```fsh
# fsh: structured records, projected fields, table formatter
ls | filter type == "file" and size > 1K | sort size desc | limit 3 | map name type size git_status | @table
```

```text
| name            | type | size    | git_status |
|-----------------|------|---------|------------|
| test_validator  | file | 1417888 | clean      |
| Cargo.lock      | file | 121665  | clean      |
| AUDIT_REPORT.md | file | 50735   | clean      |
```

### 3. json without jq

`@json` reads JSON — a whole document or a line-delimited stream — into typed records, and serializes records back out. A top-level array becomes one record per element, so the stages downstream iterate it:

```bash
# bash / zsh: needs jq or python
cat requests.ndjson | jq -r 'select(.status >= 500) | .path'
```

```fsh
# fsh: parsed records are ordinary pipeline data
cat requests.ndjson | @json | filter status >= 500 | map path ms | @table
```

```text
| path | ms   |
|------|------|
| /api | 940  |
| /db  | 1520 |
```

the other direction works the same way — `ps | filter cpu > 20.0 | @json` emits one JSON object per line for downstream tools — and `json` selects fields with a jq-like path (`json '.users[0].name'`, `json '.items[].id'`).

### 4. archive extraction

```bash
# bash / zsh: different spelling for every format
tar -zxvf archive.tar.gz
unzip bundle.zip
```

```fsh
# fsh: one command detects the format
extract archive.tar.gz
extract bundle.zip
```

### 5. destructive commands

catastrophic commands are intercepted before they run. interactively, fshell asks for confirmation:

```text
[!] DANGEROUS OPERATION DETECTED: rm -rf /tmp / usr/local/bin
    Warning: recursive delete of '/'
    Type 'yes' to proceed, or press Enter to cancel:
```

non-interactively (scripts, `-c`) the command is refused outright:

```text
Dangerous operation 'rm -rf /tmp / usr/local/bin' (recursive delete of '/') blocked by
default safety guard. Run with 'unsafe <cmd>' or unsetopt confirm_destructive to bypass.
```

everyday commands run with no friction.

---

## scripting in `.fsh`

`.fsh` is a small rust-ish language; types are inferred by default and can be pinned wherever a value is bound:

```fsh
let port: Int = 8080

fn deploy(service: String, port: Int) -> Bool {
    echo "deploying {service} on port {port}"
    return true
}

let stage = "prod"
match stage {
    "prod" => {
        echo "production deployment"
    }
    _ => {
        echo "development environment"
    }
}

try {
    echo "not-json" | @json
} catch |err| {
    echo "caught {err.code}: {err.message}"
}

cat <<EOF > out.toml
[server]
port = $port
EOF

deploy "api" 3000
```

a few conventions worth knowing:

- declarations can pin types: `let port: Int = 8080`, or a structural constraint like `let cfg: { host: String, port: Int, .. } = ...`. a value that does not match fails the declaration; untyped bindings infer as before.
- `match` arms are blocks separated by newlines, not commas.
- `catch |err| { ... }` gives you the structured diagnostic.
- heredocs (`<<EOF`) expand `$var`, `$(...)` and `$((...))`; a bare `{...}` stays literal so JSON, SQL and config content is not mangled. use a double-quoted string when you want `{expr}` interpolation.
- `10KB` is 10000 bytes, `10KiB` is 10240.

---

## interactive shell

- **completion menu** — <kbd>Tab</kbd> opens a fuzzy (nucleo) multi-column menu with category badges: `dir`, `file`, `cmd`, `builtin`, `alias`, `fn`, `var`, `job`, `flag`, `pipe`, `keyword`, `history`, `ref`.
- **predictive suggestions** — ghost text from history, filesystem paths and command syntax; <kbd>Right</kbd> accepts it, <kbd>Alt+Right</kbd> word by word.
- **sqlite history** — every command is stored with its exit code, duration and directory (`~/.config/fsh/history.db`). <kbd>Ctrl+R</kbd> searches inline, <kbd>Ctrl+H</kbd> opens a full-screen explorer, <kbd>Alt+R</kbd> restores the last cancelled command.
- **aliases expand as you type** — a command-position alias expands when you hit the space after it, so you see the real command before running it. one <kbd>Backspace</kbd> puts the alias name back.
- **git prompt** — branch, dirty state and ahead/behind are read from git's indices without spawning `git status`; the previous prompt collapses to one line after <kbd>Enter</kbd>.
- **themes** — 24-bit color with palettes built on the CSS/X11, Catppuccin, Gruvbox and Nord color dictionaries, plus custom `prompt.toml`. `config edit` opens a full-screen editor for options, themes and aliases.

the full reference is in [docs/WIDGETS.md](docs/WIDGETS.md).

---

## built-in utilities

- **`ls`** — git-aware listing emitting typed records (size, permissions, git status).
- **`ps`** — process table with typed `pid`, `cpu`, `mem`, `user`, `command` fields.
- **`files`** — recursive directory scanner emitting structured records (replaces `find`).
- **`z` / `zi`** — SQLite-backed frecency directory jumping, with an interactive picker.
- **`serve`** — instant local HTTP static file server.
- **`vault`** — local encrypted secrets store.
- **`extract`** — auto-detects and extracts `.tar.gz`, `.zip`, `.tar.xz`, `.7z`, and more.
- **`string`** — `upper`, `lower`, `trim`, `split`, `length`, `replace`.
- **`diff`** — structured diff records, usable in a pipeline.
- **`json` / `csv`** — parse and format structured data.
- **`explain`** — explains diagnostics: `explain FSH-TYPE-001`, or `explain --list` for every code.
- **`ai`** (optional, with a configured provider) — generate commands from a description, or explain one with `ai --explain "..."`.

---

## pipeline reference

core stages, evaluated in-process:

| stage | description | example |
|---|---|---|
| `filter <expr>` | keep items matching a condition | `ls \| filter size > 1048576` |
| `map <cols...>` | project fields (`a.b` reaches nested fields) or compute parenthesized expressions | `ps \| map pid (cpu / 100.0)` |
| `sort [col] [asc\|desc]` | sort records by field | `ls \| sort size desc` |
| `grep <pattern>` | keep items matching a string or regex | `cat app.log \| grep "ERROR 500"` |
| `mark <pattern>` | highlight matching rows without dropping items | `cat build.log \| mark "WARN"` |
| `count` | count items into an integer | `ls \| filter size == 0 \| count` |
| `limit <N>` | first N items | `ps \| sort cpu desc \| limit 5` |
| `traverse <edge>` | walk edges of an `ObjectGraph` | `deps \| traverse "depends_on"` |
| `hash [-a 256\|512]` | whole-stream or per-record hashes | `cat archive.tar \| hash` |

boundary operators convert between typed streams and text:

| operator | description |
|---|---|
| `@table` | auto-sized terminal table |
| `@bar` | horizontal bar chart |
| `@json` | parse or emit json |
| `@yaml` | emit yaml |
| `@csv` | parse or emit csv |
| `@msgpack` | emit binary messagepack |
| `@text` | plain text extraction |

the full reference is in [docs/PIPELINES.md](docs/PIPELINES.md).

---

## security

- **destructive-command guard** — `rm -rf /`, recursive permission changes on system roots, and raw block-device writes are blocked or confirmed; `unsafe <cmd>` bypasses it in scripts, `unsetopt confirm_destructive` disables it entirely.
- **sandboxing** — external subprocesses can run under Linux Landlock rulesets or macOS Seatbelt (SBPL) profiles, installed in `pre_exec`.
- **capabilities** — granular capability tokens gate filesystem, network, environment and process access; `with caps(...) { ... }` grants them for a scope, and `fsh -s` starts strict.

details in [docs/SECURITY.md](docs/SECURITY.md).

---

## documentation

- [language reference](docs/LANGUAGE.md) — syntax, types, reactive cells (`$=`), control flow
- [pipelines](docs/PIPELINES.md) — stages, boundary operators, backpressure, `pipefail`
- [architecture](docs/ARCHITECTURE.md) — crate layout and the pipeline execution model
- [posix compatibility](docs/POSIX-COMPATIBILITY.md) — the posix engine and its coverage
- [built-ins](docs/BUILTINS.md) — every builtin
- [configuration](docs/CONFIGURATION.md) — `config.toml`, `prompt.toml`, hooks
- [security](docs/SECURITY.md) — capabilities, sandboxing, safety prompts
- [line editor & widgets](docs/WIDGETS.md) — editor, keymaps, history explorer, status bar
- [migration guide](docs/MIGRATION.md) — bash / zsh / fish side by side

---

## what's rough today

fshell is a work in progress, and i'd rather list what isn't done than pretend otherwise:

- **posix is a compatibility layer, not a drop-in bash.** sourcing existing scripts mostly works, but `set -e`, `set -u`/`-x` and `trap` aren't implemented yet, and `command`, `readonly` and `local` aren't real builtins.
- **the sandbox and the capability system are early.** external-process sandboxing falls back to doing nothing where Landlock isn't available, and capabilities stay off unless you start with `-s`.
- **`vault` isn't security-audited.** the crypto is hand-rolled; i wouldn't keep anything you'd be sad to lose in it yet.
- **`json` paths are a documented subset of jq.** members, indexes (negative counts from the end) and `[]` iteration work; filters, pipes and slicing inside a query do not.
- **`select` and `exec` do less than the docs imply.** `select` is an interactive picker, not a column projector, and `exec` runs the command as a normal job instead of replacing the shell process.
- **`$?` in native scripts is unreliable right now** — it can get reset before a command's arguments are expanded. the posix layer's `$?` is separate.

if something on this list matters to you, open an issue — contributions are more than accepted.

---

## contributing

see [CONTRIBUTING.md](CONTRIBUTING.md) for setup, testing, CI and pull-request guidance.

```bash
cargo build --release                          # build
cargo test                                     # unit + integration tests
cargo clippy --all-targets -- -D warnings      # lint
cargo fmt --check                              # formatting
```

---

## license

[GPLv3](LICENSE)
