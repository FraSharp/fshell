# fshell pipelines

pipelines are the core execution vehicle of fshell (`fsh`).

unlike traditional unix pipes that pass unstructured byte streams and require brittle text parsing (`awk`, `sed`, `cut`), fshell pipelines stream typed `Arc<Val>` records through asynchronous, bounded tokio channels.

---

## table of contents

- [overview & comparison](#overview--comparison)
- [architecture & channel topology](#architecture--channel-topology)
  - [streaming model & backpressure](#streaming-model--backpressure)
  - [payload representation (`PipelinePayload`)](#payload-representation-pipelinepayload)
  - [pipeline lifecycles](#pipeline-lifecycles)
- [built-in pipeline stages](#built-in-pipeline-stages)
  - [`filter`](#filter)
  - [`map`](#map)
  - [`sort`](#sort)
  - [`grep`](#grep)
  - [`mark`](#mark)
  - [`count`](#count)
  - [`limit`](#limit)
  - [`traverse`](#traverse)
  - [`hash`](#hash)
- [external command disambiguation](#external-command-disambiguation)
- [redirections, heredocs & here-strings](#redirections-heredocs--here-strings)
  - [file & descriptor redirections](#file--descriptor-redirections)
  - [heredocs (`<<`)](#heredocs-)
  - [here-strings (`<<<`)](#here-strings-)
- [boundary serialization operators](#boundary-serialization-operators)
  - [`@json`](#json)
  - [`@yaml`](#yaml)
  - [`@msgpack`](#msgpack)
  - [`@csv`](#csv)
  - [`@text`](#text)
  - [`@table`](#table)
  - [`@bar`](#bar)
- [exit codes, `pipefail` & error handling](#exit-codes-pipefail--error-handling)
- [performance architecture](#performance-architecture)

---

## overview & comparison

in traditional shells, extracting a column or filtering a table relies on whitespace splitting and external binaries:

```bash
# bash: spawns 3 external processes, fragile column splitting
ps aux | awk '{if ($3 > 50.0) print $2, $11}' | head -n 5
```

in fshell, pipeline stages are language-level keywords operating on structured `Val::Map` items:

```fsh
# fsh: in-process, typed fields, zero process-spawn overhead
ps | filter cpu > 50.0 | map pid command | limit 5
```

advantages:
- **typed fields**: values retain their types (`Int`, `Float`, `String`, `DateTime`). comparisons like `cpu > 50.0` operate on numbers, not strings.
- **no string parsing**: fields are addressed by identifier name (`map pid command`), not column index (`$2`, `$11`).
- **concurrent streaming**: stages run concurrently across tokio tasks. upstream producers stream items to downstream consumers as they become available.
- **unix interop**: boundary operators (`@json`, `@csv`, `@text`) bridge structured streams to external tools seamlessly.

---

## architecture & channel topology

### streaming model & backpressure

each pipeline stage runs in its own spawned `tokio` task connected by bounded `tokio::sync::mpsc` channels:

```
┌──────────────┐    channel(N)    ┌──────────────┐    channel(N)    ┌──────────────┐
│   stage 0    │─────────────────▶│   stage 1    │─────────────────▶│   stage 2    │
│ (producer)   │  Arc<Val> stream │ (transformer)│  Arc<Val> stream │    (sink)    │
└──────────────┘                  └──────────────┘                  └──────────────┘
       │                                 │                                 │
       ▼                                 ▼                                 ▼
  tokio::spawn                      tokio::spawn                      tokio::spawn
```

- **bounded buffer**: default buffer capacity is 100 items (configurable via `FSH_PIPELINE_CHANNEL_SIZE` or `setopt pipeline_channel_size <N>`).
- **backpressure**: if a downstream consumer (e.g. `limit 10` or a slow disk writer) stops reading, upstream stages automatically pause on `.send().await`, preventing memory exhaustion.

### payload representation (`PipelinePayload`)

channels pass `PipelinePayload` items (`crates/fshell-engine/src/pipeline.rs`):

```rust
pub enum PipelinePayload {
    Data(Arc<Val>),
    Bytes(Vec<u8>),
    Structured(FshDiag),
}
```

- **`Data(Arc<Val>)`**: structured values wrapped in `Arc` for $O(1)$ zero-copy forwarding between stages.
- **`Bytes(Vec<u8>)`**: raw binary chunks from external subprocess stdout pipes. when fed into structured transformers (`filter`, `map`), bytes are line-buffered into `Val::String` instances automatically.
- **`Structured(FshDiag)`**: structured diagnostics and runtime errors propagated in-band through the stream.

### pipeline lifecycles

the engine provides three execution modes for pipelines:

1. **`spawn_pipeline_stream(pipeline, env)`**: returns a `PipeStream` receiver that yields items asynchronously as they are produced (used by the interactive REPL to render streaming tables).
2. **`collect_pipeline(pipeline, env)`**: collects all emitted payloads into a `Vec<Val>` (used by variable capture `$| ... |` and command substitution `$( ... )`).
3. **`execute_pipeline(pipeline, env, tx)`**: internal orchestration connecting stages, wiring cancellation tokens, and normalizing redirections.

---

## built-in pipeline stages

pipeline stages are first-class keywords evaluated by the engine.

### `filter`

filters records using a boolean condition expression.

```fsh
# filter structured maps
ls | filter size > 1048576 and name ~ r"\.log$"

# filter process list
ps | filter cpu > 20.0 or mem > 1024
```

**execution:**
1. receives `Val::Map` records from the upstream channel.
2. binds map fields as local variables in a scoped sub-environment.
3. evaluates the condition expression.
4. if `true`, forwards `Arc<Val>` downstream; if `false`, drops the item.

### `map`

projects specified fields or computes transformed fields.

```fsh
# project specific fields
ps | map pid command cpu

# project and compute expressions
ps | map pid (cpu / 100.0)
```

**execution:**
1. extracts field references from each incoming `Val::Map`.
2. evaluates projected expressions against the record's fields.
3. emits a new `Val::Map` preserving column ordering.

### `sort`

sorts records by a field name in ascending or descending order.

```fsh
# descending sort
ls | sort size desc

# ascending sort (default)
ls | sort name asc

# shorthand negative prefix for descending
ls | sort -size
```

**execution:**
1. buffers incoming stream items into memory.
2. applies `cmp_vals` on the target field using `Ustr` key lookup.
3. bounded by `sort_max_items` (default 50,000) to protect against memory exhaustion on infinite streams.
4. emits sorted records downstream.

### `grep`

filters incoming stream items matching a string or regular expression pattern.

```fsh
cat server.log | grep "ERROR 500"
ls | grep r"\.tmp$"
```

**execution:**
- for strings: checks substring containment or regex match.
- for maps: serializes fields to text representation before matching.

### `mark`

adds visual terminal annotations to matched rows without discarding non-matching items.

```fsh
cat build.log | mark "WARNING"
```

**execution:**
- matches incoming records against the pattern.
- matched items are formatted with visual markers (`> <text>`) and terminal color highlights when stdout is a TTY.
- all items (matched and unmatched) are forwarded downstream.

### `count`

aggregates the incoming stream and emits a single integer `Val::Int`.

```fsh
ls | filter size == 0 | count
```

### `limit`

truncates the stream to the first `N` records.

```fsh
ps | sort cpu desc | limit 5
```

**execution:**
- counts emitted items up to `N`.
- immediately drops the upstream channel receiver upon reaching `N`, triggering cooperative cancellation in upstream stages.

### `traverse`

traverses directed graph edges on `Val::ObjectGraph` structures.

```fsh
dependency_graph | traverse "depends_on"
```

**execution:**
- inspects `Val::ObjectGraph` root nodes.
- looks up outgoing edges matching the label.
- emits new `Val::ObjectGraph` records with updated root node pointers.

### `hash`

computes cryptographic or sponge hashes over the stream via `fshell-hash`.

```fsh
# whole-stream hash (default 256-bit)
cat archive.tar | hash

# 512-bit algorithm
cat archive.tar | hash -a 512

# per-record hashing (appends `_hash` field to each incoming map)
cat records.json | hash --per-record -a 256
```

---

## external command disambiguation

fshell distinguishes built-in pipeline keywords from external binaries using flag lookahead:

```fsh
# built-in pipeline stages:
cat file.txt | sort size desc
cat file.txt | grep "pattern"

# external unix binaries (flag forces external process dispatch):
cat file.txt | sort -n -k 2
cat file.txt | grep -E -i "pattern"
```

if a keyword (`sort`, `grep`, `limit`) is followed by an argument starting with `-`, the parser treats the stage as an external binary call via `fshell-bridge`.

---

## redirections, heredocs & here-strings

redirections can be placed anywhere in a stage or pipeline.

### file & descriptor redirections

| operator | description | example |
|---|---|---|
| `> file` / `1> file` | redirect stdout to file (overwrite) | `ls > files.txt` |
| `>> file` / `1>> file` | redirect stdout to file (append) | `echo "log" >> app.log` |
| `2> file` | redirect stderr to file | `cargo build 2> errors.log` |
| `&> file` / `>& file` | redirect both stdout and stderr to file | `make &> build.log` |
| `2>&1` | duplicate stderr file descriptor to stdout | `cmd 2>&1 | filter ...` |
| `1>&2` | duplicate stdout file descriptor to stderr | `echo "error" 1>&2` |
| `< file` / `0< file` | redirect stdin from file | `grep "main" < src/main.rs` |

### heredocs (`<<`)

heredocs stream multi-line text into a command or stage:

```fsh
# interpolated heredoc
cat <<EOF > config.toml
[server]
host = "127.0.0.1"
port = {port}
EOF

# raw heredoc (no interpolation)
cat <<'EOF' > script.sh
echo "$HOME is not evaluated here"
EOF

# tab-stripped heredoc (strips leading tabs)
cat <<-EOF
	indented line 1
	indented line 2
EOF
```

### here-strings (`<<<`)

feeds an expanded string into standard input:

```fsh
grep "target" <<< "{payload}"
```

---

## boundary serialization operators

boundary operators convert structured `Val` streams into formatted byte streams for terminal viewing or external process consumption.

### `@json`

serializes records as JSON:

```fsh
ps | filter cpu > 20.0 | @json
```

when placed upstream of a command, `@json` parses raw JSON input into typed `Val` structures.

### `@yaml`

serializes records as YAML documents:

```fsh
ls | limit 3 | @yaml
```

### `@msgpack`

encodes records as binary MessagePack payloads (`Val::Blob`):

```fsh
records | @msgpack > data.mpk
```

### `@csv`

formats records as comma-separated values with inferred headers:

```fsh
ps | map pid command cpu | @csv > processes.csv
```

### `@text`

extracts plain text representations from incoming records.

### `@table`

renders records as an ASCII/Unicode terminal table with auto-sized columns:

```fsh
ps | sort cpu desc | limit 10 | @table
```

### `@bar`

renders a horizontal terminal distribution bar chart from two-column map records (label and numeric value):

```fsh
ps | map command cpu | sort cpu desc | limit 5 | @bar
```

---

## exit codes, `pipefail` & error handling

### status register

every pipeline execution updates the shell status register (`$?`):
- if all stages succeed, exit status is `0`.
- if a stage fails, its exit status or error code is recorded.

### `pipefail` option

by default, the exit status of a pipeline is the status of the **last stage**.

when `pipefail` is enabled (`setopt pipefail`), the pipeline fails if **any** stage returns a non-zero exit code:

```fsh
setopt pipefail
false | true     # exit status is 1 (fails due to first stage)
```

### `errexit` integration

when `errexit` (`setopt errexit` / `set -e`) is active, any non-zero pipeline exit status triggers immediate termination of the script or enclosing block unless guarded inside `if`, `while`, `try / catch`, or an `||` condition.

---

## performance architecture

1. **`Arc<Val>` zero-copy channels**: structured values pass between tokio tasks as atomic reference counters. no heap reallocation occurs when streaming maps across stages.
2. **`Ustr` key comparisons**: field resolution in `filter`, `map`, and `sort` performs direct pointer equality rather than hashing string buffers.
3. **channel memory bounds**: bounded channels enforce backpressure at the task level, ensuring memory consumption remains $O(\text{buffer\_size})$ regardless of stream length.
4. **early termination cleanup**: stages like `limit` immediately close channel receivers, propagating cancellation upstream and stopping unnecessary CPU work.
