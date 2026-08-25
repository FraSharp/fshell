# fshell language reference

fshell (`fsh`) is a structured-data shell with a rust-like scripting language and a polyglot posix engine.

this document is the definitive language reference for `.fsh` syntax, expressions, statements, types, pipelines, and evaluation semantics.

---

## table of contents

- [lexical structure](#lexical-structure)
  - [comments](#comments)
  - [identifiers & keywords](#identifiers--keywords)
  - [bare words & paths](#bare-words--paths)
- [type system (`Val`)](#type-system-val)
  - [runtime representation](#runtime-representation)
  - [type variants](#type-variants)
  - [structural & gradual type constraints](#structural--gradual-type-constraints)
- [literals & data structures](#literals--data-structures)
  - [primitives](#primitives)
  - [strings & interpolation](#strings--interpolation)
  - [ansi-c quoting](#ansi-c-quoting)
  - [multi-line strings & heredocs](#multi-line-strings--heredocs)
  - [lists & maps](#lists--maps)
  - [brace expansion](#brace-expansion)
- [operators & expressions](#operators--expressions)
  - [arithmetic & binary operators](#arithmetic--binary-operators)
  - [logical & unary operators](#logical--unary-operators)
  - [regex matching](#regex-matching)
  - [member access](#member-access)
  - [inline pipelines & command substitution](#inline-pipelines--command-substitution)
  - [arithmetic expansion](#arithmetic-expansion)
  - [process substitution](#process-substitution)
  - [parameter expansion & modifiers](#parameter-expansion--modifiers)
- [statements & bindings](#statements--bindings)
  - [variable declarations (`let`, `local`)](#variable-declarations-let-local)
  - [assignment & compound updates](#assignment--compound-updates)
  - [inline environment variables](#inline-environment-variables)
  - [statement chaining (`&&`, `||`, `;`) & background (`&`)](#statement-chaining-----and-background-)
- [functions & signatures](#functions--signatures)
  - [definition syntax](#definition-syntax)
  - [parameter type constraints](#parameter-type-constraints)
  - [return values & scoping](#return-values--scoping)
- [control flow](#control-flow)
  - [`if` / `else`](#if--else)
  - [`while` loops](#while-loops)
  - [`for` loops](#for-loops)
  - [`break` and `continue`](#break-and-continue)
  - [`return` and `exit`](#return-and-exit)
- [pattern matching (`match`)](#pattern-matching-match)
- [error handling (`try` / `catch`)](#error-handling-try--catch)
- [posix blocks & interop](#posix-blocks--interop)
  - [inline blocks (`sh { ... }`, `posix { ... }`, `bash { ... }`)](#inline-blocks-sh----posix----bash---)
  - [`source` and `source --bash`](#source-and-source---bash)
- [pipelines & stream processing](#pipelines--stream-processing)
  - [pipeline keywords vs external commands](#pipeline-keywords-vs-external-commands)
  - [built-in stages (`filter`, `map`, `sort`, `grep`, `mark`, `count`, `limit`, `traverse`, `hash`)](#built-in-stages)
  - [redirections, heredocs, and here-strings](#redirections-heredocs-and-here-strings)
  - [boundary serialization operators (`@json`, `@yaml`, `@msgpack`, etc.)](#boundary-serialization-operators)
- [reactive streams & periodic tasks](#reactive-streams--periodic-tasks)
  - [reactive cells (`$=`)](#reactive-cells-)
  - [`every` loops](#every-loops)
- [security, capabilities & signal hooks](#security-capabilities--signal-hooks)
  - [scoped capabilities (`with caps(...) { ... }`)](#scoped-capabilities-with-capstext---)
  - [`unsafe` blocks](#unsafe-blocks)
  - [signal hooks (`on <signal> { ... }`)](#signal-hooks-on-signal----)

---

## lexical structure

### comments

line comments start with `#` and continue to the end of the line:

```fsh
# standard line comment
let workers = 8 # inline comment
```

shebang lines (`#!/usr/bin/env fsh`) at the beginning of a script file are stripped automatically.

### identifiers & keywords

identifiers begin with an ascii letter or underscore, followed by letters, digits, or underscores:

```fsh
count, _tmp, worker_id, totalRows
```

reserved statement keywords:
```fsh
let local fn match try catch with unsafe break continue return for in while if else exit on source sh posix bash every
```

reserved pipeline keywords:
```fsh
filter map sort grep mark count hash limit traverse
```

> [!NOTE]
> if a reserved pipeline keyword is immediately followed by a flag starting with `-` (e.g. `sort -n` or `grep -E`), the parser treats it as an external command call rather than the built-in fsh pipeline stage.

### bare words & paths

to preserve shell ergonomics, unquoted command arguments, file paths, and environment variable references are parsed as string expressions:

```fsh
cd /usr/local/bin
git status -s
cat ./config/app.toml
```

paths beginning with `~` are expanded against the user's home directory.

---

## type system (`Val`)

fshell uses a gradual type system centered on the `Val` enum (`crates/fshell-core/src/val.rs`). values flow through the evaluator and pipeline stages as `Arc<Val>` instances.

### runtime representation

```rust
pub enum Val {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Vec<Val>),
    Map(FxIndexMap<Ustr, Val>),
    DateTime(chrono::DateTime<chrono::Utc>),
    Blob(Vec<u8>),
    ObjectGraph { root: NodeId, graph: Arc<GraphStorage> },
    Capability(ResourceHandle),
    ReactiveStream(tokio::sync::watch::Receiver<Vec<Val>>),
}
```

- **`Val::Map`**: uses `FxIndexMap<Ustr, Val>`, pairing order preservation with `Ustr` interned string keys. looking up a key like `record.size` across 100,000 items is an $O(1)$ pointer comparison instead of string hashing.
- **`Val::Float`**: supports `NaN`, `inf`, `-inf`. `NaN == NaN` evaluates to `true` for equality consistency.
- **`Val::ObjectGraph`**: represents directed graphs of nodes and edge properties, traversed with the `traverse` pipeline stage.

### type variants

| type name | example literal | description |
|---|---|---|
| `Null` | `null` | absence of a value |
| `Bool` | `true`, `false` | boolean truth value |
| `Int` | `42`, `-10`, `0xFF` | 64-bit signed integer (`i64`) |
| `Float` | `3.14`, `-0.05` | 64-bit floating point (`f64`) |
| `String` | `"hello"`, `'world'` | utf-8 string |
| `List` | `[1, 2, 3]` | ordered array of `Val` |
| `Map` | `{ id: 1, name: "root" }` | ordered dictionary with interned `Ustr` keys |
| `DateTime` | internal / pipeline | utc timestamp from chrono |
| `Blob` | internal / binary streams | raw byte buffer (`Vec<u8>`) |
| `ObjectGraph` | internal / graph queries | directed graph storage |
| `Capability` | `process.spawn`, `net.all` | capability token for security sandbox |
| `ReactiveStream` | internal (`$=`) | dynamic tokio watch stream of values |

### structural & gradual type constraints

function parameters can be unconstrained (`Any`), constrained to a primitive type name, or constrained to a structural map layout:

```fsh
# primitive constraint
fn format_id(id: Int) -> String {
    return "id: {id}"
}

# structural constraint
fn process_file(entry: { name: String, size: Int, .. }) {
    echo "{entry.name} ({entry.size} bytes)"
}
```

the `..` inside a structural pattern allows extra fields (open struct). omitting `..` strictly rejects any map with undeclared fields.

---

## literals & data structures

### primitives

```fsh
let a = null
let b = true
let c = 100_000
let d = 3.14159
```

### strings & interpolation

double-quoted strings support variable and expression interpolation via `{...}`:

```fsh
let user = "antigravity"
let port = 8080
let url = "http://{user}@localhost:{port}/api"
```

literal braces inside double quotes can be escaped with backslashes: `"\{not interpolated\}"`.

single-quoted strings are raw and do not perform interpolation:

```fsh
let raw = 'literal {no_interp} string'
```

### ansi-c quoting

strings prefixed with `$'...'` interpret c-style escape sequences (`\n`, `\t`, `\r`, `\xHH`, `\uXXXX`):

```fsh
let greeting = $'hello\tworld\n'
```

### multi-line strings & heredocs

triple-quoted strings (`"""..."""` or `'''...'''`) preserve newlines and support dedenting:

```fsh
let query = """
    select pid, command, cpu
    from processes
    where cpu > 50.0
"""
```

heredocs feed multi-line text into a command or pipeline:

```fsh
cat <<EOF
server: 127.0.0.1
port: {port}
EOF
```

- `<<EOF`: multi-line string with variable interpolation.
- `<<'EOF'`: raw multi-line string (no interpolation).
- `<<-EOF`: strips leading tab characters from each line.

here-strings (`<<<`) feed a single expanded string into standard input:

```fsh
grep "pattern" <<< "{config_payload}"
```

### lists & maps

```fsh
# list literal
let ports = [80, 443, 8080, 8443]

# map literal
let server = {
    host: "127.0.0.1",
    port: 8080,
    active: true,
}

# access
let p = server.port
```

### brace expansion

brace expansions expand strings cartesian-style or across numeric ranges:

```fsh
echo file_{a,b,c}.txt      # file_a.txt file_b.txt file_c.txt
echo {1..5}                # 1 2 3 4 5
```

in `for` loops, brace ranges yield numeric sequences:

```fsh
for i in {1..5} {
    echo "step {i}"
}
```

---

## operators & expressions

### arithmetic & binary operators

```fsh
let sum  = 10 + 5
let diff = 10 - 5
let prod = 10 * 5
let quot = 10 / 5
```

comparisons evaluate to `Val::Bool`:

| operator | description | example |
|---|---|---|
| `==` | structural equality | `status == 200` |
| `!=` | inequality | `status != 404` |
| `<` | less than | `cpu < 20.0` |
| `<=` | less than or equal | `retries <= 3` |
| `>` | greater than | `mem > 1024` |
| `>=` | greater than or equal | `load >= 1.0` |

### logical & unary operators

```fsh
let ok = !false && (cpu < 80.0 or mem < 90.0)
```

- `!expr`: unary boolean negation.
- `and` / `&&`: logical and (short-circuiting).
- `or` / `||`: logical or (short-circuiting).

### regex matching

the `~` operator matches a string against a regular expression pattern:

```fsh
if filename ~ r"\.rs$" {
    echo "rust source file"
}
```

### member access

use dot notation (`.`) to access fields in `Val::Map`, traverse `Val::ObjectGraph`, or query capability namespaces:

```fsh
let user = { name: "alice", role: "admin" }
let username = user.name
```

if a pipeline stage returns a single-item list containing a map, member access automatically unwraps the record.

### inline pipelines & command substitution

fshell provides three syntax forms to capture pipeline output into a value:

1. **capture pipeline `$| ... |`**: returns the captured `Val` or `List` directly:
   ```fsh
   let pids = $| ps | filter cpu > 20.0 | map pid |
   ```
2. **command substitution `$( ... )`**: evaluates commands and captures their text/value:
   ```fsh
   let branch = $(git rev-parse --abbrev-ref HEAD)
   ```
3. **backtick substitution `` `...` ``**: posix-compatible shorthand:
   ```fsh
   let now = `date +%s`
   ```

### arithmetic expansion

arithmetic expansion evaluates math expressions inside `$(( ... ))`:

```fsh
let offset = 4
let next = $(( offset * 2 + 1 ))
```

### process substitution

connects the stream output or input of a pipeline to a file path argument:

- `<(pipeline)`: runs pipeline and passes a readable file path.
- `>(pipeline)`: runs pipeline and passes a writable file path.

```fsh
diff <(ps | filter cpu > 10.0 | @json) <(ps | filter mem > 10.0 | @json)
```

### parameter expansion & modifiers

fshell supports comprehensive parameter modifiers on `${var:...}` and `${var#...}`:

#### path & string transformations

| modifier | description | example (`path = "/usr/local/bin/fsh.tar.gz"`) | result |
|---|---|---|---|
| `${var:t}` | tail (basename) | `${path:t}` | `"fsh.tar.gz"` |
| `${var:h}` | head (dirname) | `${path:h}` | `"/usr/local/bin"` |
| `${var:r}` | root (remove last extension) | `${path:r}` | `"/usr/local/bin/fsh.tar"` |
| `${var:e}` | extension | `${path:e}` | `"gz"` |
| `${var:u}` | uppercase | `${user:u}` | `"ALICE"` |
| `${var:l}` | lowercase | `${user:l}` | `"alice"` |
| `${#var}` | string character length | `${#user}` | `5` |

#### substrings & defaults

```fsh
# substring: offset or offset:length
let sub = ${text:0:5}

# default values
let host = ${HOST:-"127.0.0.1"}     # use default if unset or empty
let port = ${PORT:="8080"}          # assign default if unset or empty
let val  = ${CRITICAL:?"missing"}   # error if unset or empty
let alt  = ${DEBUG:+"verbose"}      # alternate value if set
```

#### pattern trims & replacements

```fsh
let f = "image.png.bak"
echo ${f%.bak}          # remove shortest suffix -> "image.png"
echo ${f%%.*}           # remove longest suffix  -> "image"

let p = "/a/b/c"
echo ${p#/a/}           # remove shortest prefix -> "b/c"
echo ${p##*/}           # remove longest prefix  -> "c"

let msg = "foo bar foo"
echo ${msg/foo/baz}     # replace first occurrence -> "baz bar foo"
echo ${msg//foo/baz}    # replace all occurrences  -> "baz bar baz"
```

---

## statements & bindings

### variable declarations (`let`, `local`)

```fsh
# global / script-scoped binding
let max_retries = 3

# local binding (scoped to function or block)
local temp_count = 0
```

`let` defines or re-binds a variable. if a local variable exists with that name, `let` updates the local scope; otherwise it updates the environment scope.

### assignment & compound updates

```fsh
# re-assignment (fails if variable is not declared)
max_retries = 5

# compound assignment operators
max_retries += 1
max_retries -= 2
max_retries *= 3
max_retries /= 2
```

### inline environment variables

commands can be prefixed with inline environment variable assignments without polluting the caller's environment:

```fsh
RUST_LOG=debug cargo build
PORT=3000 node server.js
```

### statement chaining (`&&`, `||`, `;`) & background (`&`)

```fsh
# sequential execution
cargo build; cargo test

# conditional chaining
cargo test && echo "tests passed" || echo "tests failed"

# background execution
long_running_job &
```

---

## functions & signatures

### definition syntax

functions can be declared with standard parenthesized parameters or bare parameter lists:

```fsh
# parenthesized syntax with return type
fn calculate_load(cpu: Float, mem: Float) -> Float {
    return (cpu + mem) / 2.0
}

# bare parameter syntax
fn deploy target env {
    echo "deploying {target} to {env}"
}
```

### parameter type constraints

constraints validate arguments at call time:

```fsh
fn start_service(cfg: { host: String, port: Int, .. }) {
    echo "starting service at {cfg.host}:{cfg.port}"
}
```

### return values & scoping

- `return <expr>` stops function execution and returns the given `Val`.
- a function without an explicit `return` returns `Val::Null`.
- functions execute in their own local scope (`local_vars`), inheriting reads from the outer `Env`.

---

## control flow

### `if` / `else`

`if` branches on boolean values:

```fsh
if cpu > 80.0 {
    echo "high cpu usage: {cpu}%"
} else if cpu > 50.0 {
    echo "moderate cpu usage: {cpu}%"
} else {
    echo "normal cpu usage"
}
```

### `while` loops

```fsh
let counter = 0
while counter < 5 {
    echo "counter = {counter}"
    counter += 1
}
```

### `for` loops

`for` iterates over `Val::List`, character strings, or brace expansion ranges:

```fsh
let servers = ["srv-1", "srv-2", "srv-3"]

for server in servers {
    echo "pinging {server}..."
}

for i in {1..3} {
    echo "attempt {i}"
}
```

### `break` and `continue`

- `break`: terminates the innermost `for` or `while` loop.
- `continue`: skips to the next iteration.

### `return` and `exit`

- `return [expr]`: exits the current function frame.
- `exit [code]`: terminates the script or shell process with the specified integer status code (defaults to `0`).

---

## pattern matching (`match`)

the `match` statement performs structural pattern matching against values:

```fsh
match response {
    { status: 200, data: d, .. } => {
        echo "success: {d}"
    }
    { status: 404, .. } => {
        echo "not found"
    }
    { status: s, error: e, .. } => {
        echo "error {s}: {e}"
    }
    _ => {
        echo "unhandled response"
    }
}
```

supported pattern types:
- wildcard: `_`
- literal: `null`, `true`, `false`, `42`, `"ready"`
- structural map: `{ field: pattern, .. }`

---

## error handling (`try` / `catch`)

fshell captures failures using `try` / `catch` blocks. runtime errors populate a structured diagnostic map into the catch binding:

```fsh
try {
    rm /nonexistent/file
} catch |err| {
    echo "failed with message: {err.message}"
}
```

the catch variable contains error metadata (e.g. `err.message`, `err.code`). loop control signals (`break`, `continue`, `return`, `exit`) pass through `try` / `catch` transparently.

---

## posix blocks & interop

fshell integrates a polyglot posix engine (`fshell-posix`). scripts can mix fsh constructs with raw posix code seamlessly.

### inline blocks (`sh { ... }`, `posix { ... }`, `bash { ... }`)

embed posix syntax directly inside `.fsh` scripts without spawning external shell processes. `sh`, `posix`, and `bash` are interchangeable keywords for the exact same block construct:

```fsh
let target = "build"

# using `sh` keyword
sh {
    if [ -d "$target" ]; then
        echo "target exists"
    fi
}

# using `posix` keyword
posix {
    export BUILD_DIR="$target"
    for item in "$BUILD_DIR"/*; do
        [ -f "$item" ] && echo "file: $item"
    done
}

# using `bash` keyword
bash {
    echo "current target: $target"
}
```

all three keywords execute in-process via `fshell-posix` against the live `Env`. variables defined in the enclosing fshell environment are shared directly with the block, and changes to variables inside the block update the shell environment.

### `source` and `source --bash`

```fsh
# source an fsh script
source ./lib/helpers.fsh

# source a posix / bash script (e.g. python virtualenv activate)
source --bash .venv/bin/activate
```

when sourcing files, fshell inspects shebang lines and syntax automatically. if a sourced file is detected as posix/bash, it routes through the posix engine automatically even without the `--bash` flag.

---

## pipelines & stream processing

data flows through pipelines as an asynchronous stream of `Arc<Val>` items across tokio channels.

### pipeline keywords vs external commands

every built-in pipeline stage is a first-class language keyword:

```fsh
ps | filter cpu > 50.0 | map pid command | sort cpu desc | limit 5
```

if a command name matches a reserved stage keyword but starts with a command flag (e.g. `sort -n -k 2`), the parser automatically delegates to external process execution.

### built-in stages

#### `filter`
evaluates a boolean expression against each incoming map or value:
```fsh
ls | filter size > 1048576 and name ~ r"\.log$"
```

#### `map`
projects specific columns or transformed fields:
```fsh
ps | map pid command cpu
```

#### `sort`
sorts records by column name in ascending or descending order:
```fsh
ls | sort size desc
ls | sort -name asc
```

#### `grep`
filters rows matching a string pattern or regular expression:
```fsh
cat server.log | grep "ERROR 500"
```

#### `mark`
adds visual terminal highlighting or markers to matched rows while passing all records downstream:
```fsh
cat server.log | mark "WARN"
```

#### `count`
aggregates and emits the total count of incoming stream records as an integer `Val::Int`:
```fsh
ls | filter size == 0 | count
```

#### `limit`
restricts the stream to the first `N` elements:
```fsh
ls | sort size desc | limit 10
```

#### `traverse`
traverses directed graph edges on `Val::ObjectGraph` records:
```fsh
service_graph | traverse "depends_on"
```

#### `hash`
computes cryptographic hashes on incoming data using `fshell-hash`:
```fsh
cat archive.tar | hash -a 256
cat records.json | hash --per-record -a 512
```

### redirections, heredocs, and here-strings

| syntax | description |
|---|---|
| `> file` / `1> file` | redirect standard output to file (overwrite) |
| `>> file` / `1>> file` | redirect standard output to file (append) |
| `2> file` | redirect standard error to file |
| `&> file` / `>& file` | redirect both stdout and stderr to file |
| `2>&1` | redirect stderr file descriptor to stdout |
| `1>&2` | redirect stdout file descriptor to stderr |
| `0< file` / `< file` | redirect standard input from file |
| `<<EOF ... EOF` | heredoc input stream |
| `<<< "text"` | here-string input stream |

### boundary serialization operators

when passing structured `Val` streams to external unix tools or formatting output for users, append a boundary serialization operator:

```fsh
# output json array
ps | filter cpu > 10.0 | @json

# output yaml document
ls | limit 3 | @yaml

# render terminal tables or bar charts
ps | sort cpu desc | limit 5 | @table
ps | map command cpu | @bar
```

supported formats:
- `@json`: formatted or compact json
- `@yaml`: yaml document stream
- `@msgpack`: binary messagepack encoding
- `@text`: raw plain text extraction
- `@csv`: comma-separated values
- `@table`: terminal ascii/unicode table
- `@bar`: terminal horizontal bar chart

---

## reactive streams & periodic tasks

### reactive cells (`$=`)

reactive cells declare live computational cells that automatically update when dependencies or intervals change:

```fsh
# cell tied to a continuous pipeline
$= hot_processes = ps | filter cpu > 50.0

# cell polled periodically
$= disk_free = every 10s { df | filter mount == "/" | map free }
```

reading `$hot_processes` anywhere in fshell evaluates to the latest live snapshot.

### `every` loops

the `every` statement executes a block periodically at a fixed duration (`s` seconds, `m` minutes, `h` hours):

```fsh
every 5s {
    clear
    ps | sort cpu desc | limit 10 | @table
}
```

pressing `Ctrl+C` cleanly cancels an `every` loop.

---

## security, capabilities & signal hooks

### scoped capabilities (`with caps(...) { ... }`)

fshell features a fine-grained capability security model. scripts running under strict sandboxing can temporarily acquire privileges for a scoped block:

```fsh
with caps(net.all, process.spawn) {
    curl "https://api.github.com/status"
}
```

### `unsafe` blocks

bypasses read-only query protections in reactive cells or sandbox checks:

```fsh
unsafe {
    rm -rf /tmp/scratch_dir
}
```

### signal hooks (`on <signal> { ... }`)

register signal handlers and event callbacks using `on`:

```fsh
# inline block handler
on exit {
    echo "cleaning up temporary files..."
    rm -f /tmp/session.lock
}

# named function handler
fn handle_sigint {
    echo "aborted by user"
}
on sigint handle_sigint
```
