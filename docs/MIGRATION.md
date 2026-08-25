# migrating to fshell from bash, zsh, and fish

this guide is a practical side-by-side migration reference for converting muscle memory, shell scripts, and startup configurations from Bash, Zsh, and Fish to fshell (`fsh`).

---

## table of contents

- [core philosophy & mental model](#core-philosophy--mental-model)
- [syntax translation cheat sheet](#syntax-translation-cheat-sheet)
- [variables & data types](#variables--data-types)
- [pipelines & data processing](#pipelines--data-processing)
  - [filtering](#filtering)
  - [column projection](#column-projection)
  - [sorting & limiting](#sorting--limiting)
  - [aggregation & counting](#aggregation--counting)
- [functions & parameters](#functions--parameters)
- [control flow & loops](#control-flow--loops)
- [string manipulation & parameter modifiers](#string-manipulation--parameter-modifiers)
- [shell options (`set` vs `setopt`)](#shell-options-set-vs-setopt)
- [startup files & configuration](#startup-files--configuration)
- [zero-rewrite migration with posix blocks](#zero-rewrite-migration-with-posix-blocks)

---

## core philosophy & mental model

traditional shells treat all command I/O as flat, untyped byte streams. getting structured information out of `ps`, `ls`, or `git` requires piping output through text-chopping utilities (`awk`, `sed`, `cut`, `grep`, `xargs`).

in fshell:
- **commands yield structured records**: `ps` and `ls` return streams of typed `Val::Map` objects (`pid: Int`, `size: Int`, `modified: DateTime`).
- **pipeline stages are language keywords**: `filter`, `map`, `sort`, `count`, and `limit` execute in-process without spawning external subprocesses.
- **polyglot posix engine**: you do not need to rewrite existing Bash scripts or virtualenv hooks — run them unchanged via `sh { ... }`, `bash { ... }`, or `source --bash`.

---

## syntax translation cheat sheet

| task | bash / zsh | fish | fshell (`fsh`) |
|---|---|---|---|
| declare variable | `x=42` | `set x 42` | `let x = 42` |
| declare local variable | `local x=42` | `set -l x 42` | `local x = 42` |
| export environment | `export VAR="val"` | `set -x VAR "val"` | `export VAR="val"` |
| define function | `deploy() { ... }` | `function deploy; ...; end` | `fn deploy(env, target) { ... }` |
| if statement | `if [ -f "$f" ]; then ...; fi` | `if test -f $f; ...; end` | `if condition { ... }` or `sh { if [ -f "$f" ]; then ...; fi }` |
| for loop | `for i in 1 2 3; do ...; done` | `for i in 1 2 3; ...; end` | `for i in [1, 2, 3] { ... }` |
| while loop | `while [ $i -lt 5 ]; do ...; done`| `while test $i -lt 5; ...; end` | `while i < 5 { ... }` |
| math / arithmetic | `$(( x * 2 + 1 ))` | `math "$x * 2 + 1"` | `(x * 2 + 1)` or `$(( x * 2 + 1 ))` |
| capture output | `out=$(cmd)` or `out=`cmd`` | `set out (cmd)` | `let out = $| cmd |` or `let out = $(cmd)` |
| string interpolation | `"hello, $USER"` | `"hello, $USER"` | `"hello, {user}"` |
| process substitution | `diff <(cmd1) <(cmd2)` | `diff (cmd1 | psub) (cmd2 | psub)` | `diff <(cmd1) <(cmd2)` |
| sequential commands | `cmd1; cmd2` | `cmd1; cmd2` | `cmd1; cmd2` |
| boolean AND chaining | `cmd1 && cmd2` | `cmd1; and cmd2` | `cmd1 && cmd2` |
| boolean OR chaining | `cmd1 \|\| cmd2` | `cmd1; or cmd2` | `cmd1 \|\| cmd2` |

---

## variables & data types

### scalars & rich types

in bash/zsh, variables are fundamentally strings. in fshell, variables have native types:

```fsh
# numbers
let count = 42
let ratio = 0.75

# booleans
let is_active = true

# lists
let servers = ["srv-1", "srv-2", "srv-3"]

# maps
let config = {
    host: "127.0.0.1",
    port: 8080,
    tls: true,
}

# access map fields with dot notation:
let p = config.port
```

### environment variables

```fsh
# set environment variable for subprocesses
export RUST_LOG="debug"

# inline environment variable for a single command
PORT=3000 node server.js
```

---

## pipelines & data processing

### filtering

filtering processes with high CPU usage:

```bash
# bash: parse text with awk
ps aux | awk '{if ($3 > 50.0) print $2, $11}'

# fish:
ps aux | awk '{if ($3 > 50.0) print $2, $11}'

# fsh: filter typed numeric property
ps | filter cpu > 50.0 | map pid command
```

filtering files larger than 10MB:

```bash
# bash
find . -maxdepth 1 -type f -size +10M

# fsh
ls | filter type == "file" and size > 10M | map name size
```

### column projection

extracting specific columns:

```bash
# bash / zsh: column numbers via awk/cut
cat /etc/passwd | cut -d: -f1,7

# fsh: project field names directly
ps | map pid command user
```

### sorting & limiting

```bash
# bash: sort by column 5 numerically in descending order, then take top 10
ls -la | sort -k5 -n -r | head -n 10

# fsh: sort by typed field name, slice with limit
ls | sort size desc | limit 10 | @table
```

### aggregation & counting

```bash
# bash: count lines
grep -c "ERROR" server.log
ls | wc -l

# fsh: count records in stream
cat server.log | grep "ERROR" | count
ls | filter size == 0 | count
```

---

## functions & parameters

### function definitions

bash / zsh:
```bash
deploy_service() {
    local service="$1"
    local port="$2"
    echo "deploying $service to port $port"
}
```

fshell:
```fsh
# typed parameters with structural validation
fn deploy_service(service: String, port: Int) {
    echo "deploying {service} to port {port}"
}

# bare parameter syntax (shell ergonomic style)
fn deploy service port {
    echo "deploying {service} to port {port}"
}
```

---

## control flow & loops

### `for` loops

bash / zsh:
```bash
for file in *.log; do
    echo "compressing $file"
    gzip "$file"
done
```

fshell:
```fsh
for file in *.log {
    echo "compressing {file}"
    gzip "{file}"
}

# iterating over explicit lists:
for server in ["prod-1", "prod-2"] {
    echo "deploying to {server}"
}

# numeric range iteration:
for i in {1..5} {
    echo "attempt {i}"
}
```

### `while` loops

bash / zsh:
```bash
count=0
while [ $count -lt 5 ]; do
    echo "count: $count"
    count=$((count + 1))
done
```

fshell:
```fsh
let count = 0
while count < 5 {
    echo "count: {count}"
    count += 1
}
```

### pattern matching (`match`)

instead of complex `case ... esac` blocks:

```fsh
match status {
    200 => echo "OK",
    404 => echo "Not Found",
    500 => echo "Internal Error",
    _   => echo "Other status: {status}",
}
```

---

## string manipulation & parameter modifiers

fshell supports the standard parameter expansion modifiers from bash/zsh:

| operation | bash / zsh | fshell (`fsh`) |
|---|---|---|
| default value if unset | `${VAR:-"default"}` | `${VAR:-"default"}` |
| assign default if unset | `${VAR:="default"}` | `${VAR:="default"}` |
| error if unset | `${VAR:?"required"}` | `${VAR:?"required"}` |
| substring (offset:len) | `${VAR:0:5}` | `${VAR:0:5}` |
| string length | `${#VAR}` | `${#VAR}` or `string length $VAR` |
| remove shortest prefix | `${VAR#prefix}` | `${VAR#prefix}` |
| remove longest prefix | `${VAR##*/}` | `${VAR##*/}` or `${VAR:t}` (tail) |
| remove shortest suffix | `${VAR%suffix}` | `${VAR%suffix}` |
| remove longest suffix | `${VAR%%.*}` | `${VAR%%.*}` or `${VAR:r}` (root) |
| replace first | `${VAR/pattern/repl}` | `${VAR/pattern/repl}` |
| replace all | `${VAR//pattern/repl}`| `${VAR//pattern/repl}` |
| uppercase | `${VAR^^}` (bash) / `${(U)VAR}` (zsh) | `${VAR:u}` or `string upper $VAR` |
| lowercase | `${VAR,,}` (bash) / `${(L)VAR}` (zsh) | `${VAR:l}` or `string lower $VAR` |

---

## shell options (`set` vs `setopt`)

| bash / zsh flag | bash command | zsh command | fshell (`fsh`) command |
|---|---|---|---|
| exit on error | `set -e` | `setopt errexit` | `setopt errexit` |
| error on unset variable | `set -u` | `setopt nounset` | `setopt nounset` |
| pipeline fail on any error | `set -o pipefail` | `setopt pipefail` | `setopt pipefail` |
| print commands before exec | `set -x` | `setopt xtrace` | `setopt xtrace` |
| case-insensitive globbing | `shopt -s nocaseglob` | `unsetopt caseglob`| `setopt nocaseglob` |
| empty glob expansion | `shopt -s nullglob` | `setopt nullglob` | `setopt nullglob` |
| prevent overwrite redirection | `set -C` / `set -o noclobber` | `setopt noclobber` | `setopt noclobber` |
| auto-cd into directories | `shopt -s autocd` | `setopt autocd` | `setopt autocd` (on by default) |

---

## startup files & configuration

| purpose | bash | zsh | fish | fshell (`fsh`) |
|---|---|---|---|---|
| interactive startup | `~/.bashrc` | `~/.zshrc` | `~/.config/fish/config.fish` | `~/.config/fsh/init.fsh` |
| persistent shell options | inside rc script | inside rc script | inside config.fish | `~/.config/fsh/config.toml` |
| prompt customization | `$PS1` in `.bashrc` | `$PROMPT` in `.zshrc` | `fish_prompt` function | `~/.config/fsh/prompt.toml` |
| command aliases | `alias ll='ls -l'` | `alias ll='ls -l'` | `alias ll='ls -l'` | `alias ll="ls -l"` in `init.fsh` |
| directory jump database | autojump / z / zoxide | z / zoxide | zoxide | built-in `z` / `zi` (`frecency.db`) |
| command history | `~/.bash_history` | `~/.zsh_history` | `~/.local/share/fish/fish_history` | SQLite `~/.config/fsh/history.db` |

---

## zero-rewrite migration with posix blocks

you do not need to rewrite legacy shell scripts or tools when switching to fshell.

### 1. inline posix blocks

embed existing complex bash/sh code directly into `.fsh` scripts:

```fsh
let build_env = "production"

# runs directly via fshell-posix in the same process
sh {
    if [ "$build_env" = "production" ]; then
        export CFLAGS="-O3"
        make release
    fi
}
```

### 2. sourcing python virtualenvs

Python `venv` activation scripts execute seamlessly:

```fsh
source --bash .venv/bin/activate
```

the updated `$PATH`, `$VIRTUAL_ENV`, and prompt indicators apply directly to your interactive session.

### 3. third-party tool integrations

initialize standard developer tools using built-in helpers in `~/.config/fsh/init.fsh`:

```fsh
# starship prompt
starship_init

# zoxide directory jumper
zoxide_init

# direnv environment switcher
direnv_init

# fzf fuzzy finder
fzf_init
```
