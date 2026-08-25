# fshell posix compatibility & polyglot engine

this document details fshell's polyglot execution engine, POSIX compliance substrate (`fshell-posix`), expansion mechanics, and environment interoperability.

---

## table of contents

- [overview](#overview)
- [polyglot substrate architecture](#polyglot-substrate-architecture)
  - [dual-stream runtime](#dual-stream-runtime)
  - [shared environment state](#shared-environment-state)
- [execution modes & dispatch](#execution-modes--dispatch)
  - [shebang auto-detection](#shebang-auto-detection)
  - [inline posix blocks (`sh`, `posix`, `bash`)](#inline-posix-blocks-sh-posix-bash)
  - [`source` and `source --bash`](#source-and-source---bash)
  - [cli posix mode (`fsh --posix`)](#cli-posix-mode-fsh---posix)
- [word expansion pipeline](#word-expansion-pipeline)
  - [phase 1: parameter, command & arithmetic expansion](#phase-1-parameter-command--arithmetic-expansion)
  - [phase 2: field splitting (`$IFS`)](#phase-2-field-splitting-ifs)
  - [phase 3: pathname expansion (globbing)](#phase-3-pathname-expansion-globbing)
  - [phase 4: quote removal](#phase-4-quote-removal)
- [c-style arithmetic engine](#c-style-arithmetic-engine)
- [built-in posix commands](#built-in-posix-commands)
  - [`test` / `[`](#test--)
  - [`eval`](#eval)
  - [`export` & `readonly`](#export--readonly)
  - [`set` & `shift`](#set--shift)
  - [`read`](#read)
  - [`trap`](#trap)
  - [`.` / `source`](#--source)
  - [`type`, `command`, `exec`, `alias`](#type-command-exec-alias)
- [subshell isolation & control structures](#subshell-isolation--control-structures)
- [compatibility matrix](#compatibility-matrix)

---

## overview

fshell eliminates migration friction from legacy shells (`bash`, `sh`, `zsh`) via a dedicated in-process POSIX engine (`crates/fshell-posix`).

rather than spawning `/bin/sh` or `/bin/bash` child processes, fshell parses and interprets POSIX shell code directly inside the Rust engine, sharing environment variables, working directories, and capability policies without IPC serialization overhead.

---

## polyglot substrate architecture

POSIX compatibility cannot be achieved by naively translating POSIX syntax into native `.fsh` AST calls. POSIX shells rely on distinct execution semantics:
- dynamic `$IFS` field splitting after variable expansion
- exit-code truthiness ($0 = \text{true}$, non-zero $= \text{false}$)
- transactional subshell environment isolation `( ... )`
- dynamic code evaluation via `eval`
- integer-based file descriptor plumbing (`2>&1`, `3< file`)

to support both worlds cleanly, fshell uses a polyglot engine architecture:

```
┌────────────────────────────────────────────────────────────────┐
│                       Interactive CLI / Script                 │
│      (Auto-detects Shebang / `fsh --posix` / `sh { ... }`)      │
└────────────────┬──────────────────────────────┬────────────────┘
                 │                              │
     ┌───────────▼───────────┐      ┌───────────▼───────────┐
     │   fshell Native AST   │      │   fshell-posix AST    │
     │   (Typed Pipelines)   │      │   (POSIX.1-2024 /     │
     │   (Gradual Types)     │      │    Bash Extensions)   │
     └───────────┬───────────┘      └───────────┬───────────┘
                 │                              │
                 └───────────────┬──────────────┘
                                 │
                     ┌───────────▼───────────┐
                     │     fshell-engine     │
                     │  - Unified Env state  │
                     │  - Pipeline executor  │
                     │  - Dual-stream pipes  │
                     │  - Sandbox/Caps checks│
                     └───────────────────────┘
```

### dual-stream runtime

pipelines in `fshell-engine` natively handle dual-stream payloads:
- `PipelinePayload::Data(Arc<Val>)`: structured data flowing between native fsh stages.
- `PipelinePayload::Bytes(Vec<u8>)`: raw byte streams flowing between POSIX pipelines and external commands.

when POSIX commands feed into fsh stages (e.g. `sh { cat log; } | filter ...`), raw bytes are automatically line-buffered into `Val::String` items.

### shared environment state

the POSIX engine operates directly on the caller's `Env`:
- `$VAR = val` in POSIX writes to `env.vars`.
- `export VAR` synchronizes with the system environment map.
- POSIX functions register in `env.fns`.
- working directory changes (`cd`) update `env.pwd`.

---

## execution modes & dispatch

### shebang auto-detection

when executing a file via `fsh script` or `source script`, fshell inspects the file header:
- if the script starts with `#!/bin/sh`, `#!/bin/bash`, `#!/usr/bin/env sh`, or `#!/usr/bin/env bash`, it routes directly to `fshell-posix`.
- if the script starts with `#!/usr/bin/env fsh` or has no POSIX shebang, it parses as native `.fsh`.

### inline posix blocks (`sh`, `posix`, `bash`)

embed POSIX shell scripts directly inside `.fsh` source files:

```fsh
let release_tag = "v1.2.0"

# inline posix block
sh {
    if [ -z "$release_tag" ]; then
        echo "missing release tag" >&2
        exit 1
    fi
    echo "packaging $release_tag"
}
```

`sh { ... }`, `posix { ... }`, and `bash { ... }` are exact keyword aliases. all three execute in-process against the current `Env`.

### `source` and `source --bash`

sourcing external scripts (such as Python virtualenv activation scripts):

```fsh
# explicit bash evaluation
source --bash .venv/bin/activate

# auto-detected bash evaluation
source ~/.nvm/nvm.sh
```

modifications made by the sourced script (`PATH` updates, virtualenv variables, shell functions) persist in the parent fshell session.

### cli posix mode (`fsh --posix`)

launch `fsh` in strict POSIX compatibility mode:

```bash
# run inline posix command
fsh --posix -c 'for i in 1 2 3; do echo "item: $i"; done'

# execute posix script file
fsh --posix deploy.sh arg1 arg2
```

---

## word expansion pipeline

the `fshell-posix` expander (`crates/fshell-posix/src/expand.rs`) executes the standard 4-phase POSIX expansion algorithm:

```
Raw Word Tokens
      │
      ▼
[ Phase 1 ] ──▶ Parameter Expansion (`$VAR`, `${VAR:-def}`)
                Command Substitution (`$(cmd)`, `` `cmd` ``)
                Arithmetic Expansion (`$(( 1 + 2 ))`)
      │
      ▼
[ Phase 2 ] ──▶ Field Splitting (splits unquoted text by `$IFS` whitespace)
      │
      ▼
[ Phase 3 ] ──▶ Pathname Expansion (glob matching `*`, `?`, `[...]`)
      │
      ▼
[ Phase 4 ] ──▶ Quote Removal (strips unquoted `'`, `"`, `\`)
      │
      ▼
Final Argument List
```

### phase 1: parameter, command & arithmetic expansion

supports all POSIX parameter modifiers:
- `${var:-default}`: use default if unset/null
- `${var:=default}`: assign default if unset/null
- `${var:?error}`: error if unset/null
- `${var:+alternate}`: use alternate if set and not null
- `${#var}`: string length
- `${var#pattern}`, `${var##pattern}`: prefix removal (shortest / longest)
- `${var%pattern}`, `${var%%pattern}`: suffix removal (shortest / longest)
- `${var/pattern/repl}`, `${var//pattern/repl}`: string replacement

### phase 2: field splitting (`$IFS`)

unquoted expansion results are split into discrete argument words based on the characters in `$IFS` (defaults to space, tab, newline). quoted expansions (`"$VAR"`) bypass field splitting.

### phase 3: pathname expansion (globbing)

unquoted words containing `*`, `?`, or `[...]` are expanded against matching filesystem entries. if no matches are found, the literal pattern is preserved.

### phase 4: quote removal

all remaining quote characters (`'`, `"`) and escape backslashes not generated by expansions are removed before argument dispatch.

---

## c-style arithmetic engine

`fshell-posix` includes a full C-style integer arithmetic evaluator (`crates/fshell-posix/src/arithmetic.rs`):

```bash
sh {
    x=10
    y=$(( (x * 2) + 5 ))
    (( y += 3 ))
    echo "result: $y"   # result: 28
}
```

supported operators (ordered by precedence):
- unary: `+`, `-`, `~`, `!`, `++`, `--`
- multiplicative: `*`, `/`, `%`
- additive: `+`, `-`
- bitwise shift: `<<`, `>>`
- comparison: `<`, `<=`, `>`, `>=`
- equality: `==`, `!=`
- bitwise AND: `&`
- bitwise XOR: `^`
- bitwise OR: `|`
- logical AND: `&&`
- logical OR: `||`
- ternary conditional: `cond ? expr1 : expr2`
- assignment: `=`, `+=`, `-=`, `*=`, `/=`, `%=`, `<<=`, `>>=`, `&=`, `^=`, `|=`

---

## built-in posix commands

`fshell-posix` executes standard shell builtins in-process:

### `test` / `[`

evaluates conditional expressions according to POSIX.1-2024:

```bash
# file checks
[ -f "$file" ]      # regular file exists
[ -d "$dir" ]       # directory exists
[ -e "$path" ]      # path exists
[ -s "$file" ]      # file exists and is not empty
[ -L "$link" ]      # symbolic link exists
[ -r "$file" ]      # readable
[ -w "$file" ]      # writable
[ -x "$file" ]      # executable

# string checks
[ -z "$str" ]       # string is empty (length 0)
[ -n "$str" ]       # string is non-empty
[ "$a" = "$b" ]     # string equality
[ "$a" != "$b" ]    # string inequality

# integer checks
[ "$x" -eq "$y" ]   # equal
[ "$x" -ne "$y" ]   # not equal
[ "$x" -lt "$y" ]   # less than
[ "$x" -le "$y" ]   # less than or equal
[ "$x" -gt "$y" ]   # greater than
[ "$x" -ge "$y" ]   # greater than or equal

# boolean combinations
[ ! -f "$file" ]
[ -f "$file" -a -r "$file" ]
[ -d "$dir" -o -d "$alt_dir" ]
```

### `eval`

parses and executes dynamic shell code within the current session:

```bash
sh {
    cmd="export TARGET=prod"
    eval "$cmd"
}
```

### `export` & `readonly`

- `export VAR=val`: marks variables for export to subprocesses.
- `readonly VAR`: marks variables as immutable.

### `set` & `shift`

- `set -- a b c`: sets positional parameters (`$1`, `$2`, `$3`, `$#`).
- `shift [N]`: shifts positional parameters left by `N`.
- `set -e` / `set -u` / `set -x` / `set -o pipefail`: configures shell execution options.

### `read`

reads a line from standard input, performing `$IFS` splitting into variables:

```bash
sh {
    echo "alice 100" | {
        read -r name score
        echo "user $name has score $score"
    }
}
```

### `trap`

registers signal handling commands:

```bash
sh {
    trap 'echo "cleaning up..."; rm -f /tmp/lock' EXIT INT TERM
}
```

### `.` / `source`

reads and executes commands from a file in the caller's environment context.

### `type`, `command`, `exec`, `alias`

- `type <name>`: displays resolution type (builtin, function, alias, file path).
- `command <name> [args]`: bypasses shell functions and aliases.
- `exec <cmd>`: replaces the shell process.
- `alias <name>=<val>` / `unalias <name>`: defines/removes command aliases.

---

## subshell isolation & control structures

### subshell blocks `( ... )`

commands executed inside parentheses run in an isolated environment scope:

```bash
sh {
    VAR="original"
    (
        VAR="modified"
        cd /tmp
        echo "inside: $VAR, in $(pwd)"
    )
    echo "outside: $VAR, in $(pwd)" # outside: original, in original directory
}
```

modifications to variables and directory changes inside `( ... )` are discarded when the subshell terminates.

### control flow

all standard POSIX control structures execute natively:
- `if ... then ... elif ... else ... fi`
- `for var in list; do ... done`
- `while condition; do ... done`
- `until condition; do ... done`
- `case word in pattern) ... ;; esac`
- `break [N]` and `continue [N]`

---

## compatibility matrix

| feature | posix standard | bash 5+ | fshell support |
|---|---|---|---|
| parameter expansions (`${var:-def}`, etc.) | yes | yes | full in-process |
| `$IFS` field splitting | yes | yes | full in-process |
| pathname expansion (globbing) | yes | yes | full in-process |
| C-style arithmetic (`$((...))`) | yes | yes | full in-process |
| `test` / `[` conditional syntax | yes | yes | full in-process |
| extended test `[[ ... ]]` | no | yes | full in-process |
| subshell scoping `( ... )` | yes | yes | transactional scope clone |
| dynamic `eval` | yes | yes | full in-process |
| signal traps (`trap`) | yes | yes | mapped to fshell hook system |
| virtualenv activation (`source activate`) | n/a | yes | verified full compatibility |
| process substitution `<(cmd)` | no | yes | supported via named pipes/tempfiles |
| arrays (`arr=(a b c)`) | no | yes | supported |
