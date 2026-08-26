# fshell architecture

this document describes the internal architecture of fshell (`fsh`) — workspace crate structure, execution model, memory layout, and runtime subsystems.

---

## table of contents

- [overview](#overview)
- [workspace map & dependency graph](#workspace-map--dependency-graph)
- [binary & startup lifecycle](#binary--startup-lifecycle)
  - [entry point & multicall routing](#entry-point--multicall-routing)
  - [initialization sequence](#initialization-sequence)
  - [light boot vs full boot](#light-boot-vs-full-boot)
  - [session handoff persistence](#session-handoff-persistence)
- [core data model & memory layout](#core-data-model--memory-layout)
  - [the `Val` enum](#the-val-enum)
  - [interned map keys with `Ustr`](#interned-map-keys-with-ustr)
  - [`ObjectGraph` storage](#objectgraph-storage)
  - [capabilities & `ResourceHandle`](#capabilities--resourcehandle)
- [environment state (`Env`)](#environment-state-env)
  - [modular composition](#modular-composition)
  - [variable scoping](#variable-scoping)
  - [runtime lock ordering](#runtime-lock-ordering)
- [pipeline execution & async dataflow](#pipeline-execution--async-dataflow)
  - [tokio streaming model](#tokio-streaming-model)
  - [command dispatch hierarchy](#command-dispatch-hierarchy)
  - [stage execution](#stage-execution)
  - [boundary serialization](#boundary-serialization)
- [posix polyglot integration (`fshell-posix`)](#posix-polyglot-integration-fshell-posix)
  - [in-process execution](#in-process-execution)
  - [transparent dispatch](#transparent-dispatch)
- [crate deep dives](#crate-deep-dives)
  - [fshell-core](#fshell-core)
  - [fshell-capabilities](#fshell-capabilities)
  - [fshell-hash](#fshell-hash)
  - [fshell-engine](#fshell-engine)
  - [fshell-builtins](#fshell-builtins)
  - [fshell-bridge](#fshell-bridge)
  - [fshell-posix](#fshell-posix)
  - [fshell-ls](#fshell-ls)
  - [fshell-git](#fshell-git)
  - [fshell-render](#fshell-render)
  - [fshell-sandbox](#fshell-sandbox)
  - [fshell-config-tui](#fshell-config-tui)
  - [fshell-panes](#fshell-panes)
  - [fshell-repl](#fshell-repl)
- [security & sandboxing](#security--sandboxing)
- [performance architecture](#performance-architecture)

---

## overview

fshell is built as a modular rust workspace (edition 2024, unix-only for macOS and Linux).

instead of treating all data as untyped byte streams, fshell routes structured `Arc<Val>` payloads across asynchronous tokio channels while maintaining zero-friction compatibility with unix external binaries and posix scripts.

key architecture traits:
- **single binary**: the root package builds one binary (`fsh`). symlinked utility invocations (e.g. `ls`) trigger multicall dispatch without launching the full REPL.
- **in-process polyglot engine**: `.fsh` scripts, inline POSIX blocks (`sh { ... }`, `bash { ... }`), and POSIX shell scripts run against the same live `Env` state without spawning intermediate subprocesses.
- **fast data structures**: `Ustr` interned string keys eliminate string hashing and allocation on field lookups; `FxIndexMap` maintains record insertion order.

---

## workspace map & dependency graph

the workspace is organized into 14 crates with strict dependency layering:

```
┌───────────────────────────────────────────────────────────────────┐
│                           fshell (fsh)                            │
└─────────────────────────────────┬─────────────────────────────────┘
                                  │
      ┌───────────────────────────┼───────────────────────────┐
      │                           │                           │
┌─────▼───────────┐      ┌────────▼──────────┐      ┌─────────▼────────┐
│   fshell-repl   │      │ fshell-config-tui │      │   fshell-panes   │
└─────┬───────────┘      └────────┬──────────┘      └─────────┬────────┘
      │                           │                           │
      ├───────────────────────────┴───────────────────────────┤
      │
┌─────▼───────────┐      ┌───────────────────┐      ┌──────────────────┐
│  fshell-bridge  │      │  fshell-builtins  │      │   fshell-posix   │
└─────┬───────────┘      └────────┬──────────┘      └─────────┬────────┘
      │                           │                           │
      ├───────────────────────────┼───────────────────────────┤
      │                           │                           │
      │                  ┌────────▼──────────┐                │
      │                  │     fshell-ls     │                │
      │                  └────────┬──────────┘                │
      │                           │                           │
      │                  ┌────────▼──────────┐                │
      │                  │    fshell-git     │                │
      │                  └────────┬──────────┘                │
      │                           │                           │
      ├───────────────────────────┴───────────────────────────┤
      │
┌─────▼───────────┐      ┌───────────────────┐      ┌──────────────────┐
│  fshell-render  │      │  fshell-sandbox   │      │  fshell-engine   │
└─────┬───────────┘      └────────┬──────────┘      └─────────┬────────┘
      │                           │                           │
      ├───────────────────────────┴───────────────────────────┤
      │
┌─────▼───────────────┐                             ┌─────────▼────────┐
│ fshell-capabilities │                             │   fshell-hash    │
└─────┬───────────────┘                             └─────────┬────────┘
      │                                                       │
      └───────────────────────────┬───────────────────────────┘
                                  │
                         ┌────────▼────────┐
                         │   fshell-core   │
                         └─────────────────┘
```

| crate | role | key dependencies |
|---|---|---|
| `fshell-core` | parser, AST, `Val` types, diagnostics, `RwLock` | `ustr`, `indexmap`, `chrono`, `miette` |
| `fshell-capabilities` | capability token validation and strict-mode checks | `fshell-core` |
| `fshell-hash` | sponge hash algorithms, fast hashing helpers | `fshell-core` |
| `fshell-render` | miette-based graphical / compact / json error renderers | `fshell-core` |
| `fshell-sandbox` | landlock (linux) and SBPL (macOS) subprocess sandbox hooks | `fshell-core`, `fshell-engine` |
| `fshell-git` | git metadata extraction and status detection | `fshell-core` |
| `fshell-ls` | directory listing, metadata formatting, icons, tree layout | `fshell-core`, `fshell-git`, `fshell-hash` |
| `fshell-engine` | evaluator, pipeline executor, reactive streams, `Env` | `fshell-core`, `fshell-capabilities`, `fshell-hash` |
| `fshell-builtins` | ~117 built-in commands registered into `Env` | `fshell-core`, `fshell-engine`, `fshell-ls`, `fshell-capabilities` |
| `fshell-bridge` | external command fallback, globbing, path caching | `fshell-core`, `fshell-engine`, `fshell-capabilities` |
| `fshell-posix` | POSIX/Bash syntax parser and runtime engine | `fshell-core`, `fshell-engine` |
| `fshell-config-tui` | ratatui-based configuration visual editor | `fshell-core`, `fshell-engine`, `ratatui` |
| `fshell-panes` | terminal multiplexer library and daemon binaries | `fshell-core`, `fshell-engine`, `crossterm`, `ratatui` |
| `fshell-repl` | interactive prompt, Reedline line-editor, FTUI, SQLite history | `fshell-engine`, `fshell-builtins`, `fshell-bridge`, `reedline` |

---

## binary & startup lifecycle

### entry point & multicall routing

`src/main.rs` builds a single-threaded tokio runtime (`Builder::new_current_thread().enable_all()`) and enters `fshell::run()`.

before initializing the shell, argv[0] is inspected for multicall routing:
- if argv[0] is `ls` (via symlink or direct exec), `fshell::run_utility()` runs the standalone utility and terminates immediately without allocating the engine or REPL.
- otherwise, the process continues into CLI argument parsing (`src/lib.rs`).

### initialization sequence

every session initializes components in strict bottom-up dependency order:

1. **`fshell_core::init()`**: registers core diagnostics and error formatting hooks.
2. **`fshell_capabilities::init()`**: prepares the security capability subsystem.
3. **`Env::new()`** or **`Env::for_command()`**: creates the central runtime environment.
4. **`fshell_builtins::init(&env)`**: registers all active builtin command handlers.
5. **`fshell_bridge::init(&env)`**: registers external command fallback dispatch and glob expansion.
6. **`init_posix_handler()`**: registers the `fshell-posix` execution callback into `fshell-engine`.
7. **login & profile loading**: loads host login environment and evaluates startup profiles (`init.fsh` / `config.toml`).
8. **path cache warmup**: runs `warmup_path_cache()` in the background to build the binary lookup index.
9. **REPL or execution**:
   - `fsh -c <cmd>`: executes the inline command and exits.
   - `fsh <script.fsh>`: executes the file non-interactively.
   - `fsh`: starts `fshell_repl::init(&env)` and launches the interactive FTUI.

### light boot vs full boot

- **`-c` path (`Env::for_command()`)**: lightweight boot omitting session handoff, reactive stream schedulers, history database open, and FTUI terminal raw mode.
- **interactive path (`Env::new()`)**: full boot with reactive schedulers, SQLite history (`history.db`), frecency tracking (`frecency.db`), prompt segment renderers, and signal traps.

### session handoff persistence

fshell implements seamless runtime reloading (`reload --full`) via `fshell_engine::handoff`.

the running session serializes its live state to JSON:
- global and local variables
- defined user functions
- capability tokens
- reactive pipeline definitions
- shell options and cwd

the new binary is launched with `--handoff <path>`, which deserializes the state into the fresh `Env` before the user interacts with the prompt.

---

## core data model & memory layout

### the `Val` enum

the universal data currency across all fshell crates is `Val` (`crates/fshell-core/src/val.rs`):

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
    ObjectGraph {
        root: NodeId,
        graph: Arc<GraphStorage>,
    },
    Capability(ResourceHandle),
    ReactiveStream(tokio::sync::watch::Receiver<Vec<Val>>),
}
```

### interned map keys with `Ustr`

`Val::Map` uses `FxIndexMap<Ustr, Val>`.

`Ustr` is an interned string handle representing an immutable string in a global string pool. looking up a field like `item.status` or `record.cpu` performs an integer/pointer comparison ($O(1)$) rather than a string hash and equality loop across thousands of items.

### `ObjectGraph` storage

`Val::ObjectGraph` stores directed property graphs:

```rust
pub struct GraphStorage {
    pub nodes: FxHashMap<NodeId, NodeData>,
    pub edges: FxHashMap<NodeId, Vec<EdgeData>>,
}
```

graph nodes and edges store property dictionaries (`FxIndexMap<Ustr, Val>`). equality checks between two graphs execute an `Arc::ptr_eq` fast path before performing structural comparisons.

### capabilities & `ResourceHandle`

security privileges are represented by `ResourceHandle` variants:

```rust
pub enum ResourceHandle {
    ReadDir(PathBuf),
    WriteDir(PathBuf),
    ReadFile(PathBuf),
    WriteFile(PathBuf),
    NetworkSocket(String),
    NetworkAll,
    ReadEnv(String),
    WriteEnv(String),
    ProcessSpawn,
    ProcessSpawnPath(String),
}
```

---

## environment state (`Env`)

### modular composition

the central `Env` struct in `crates/fshell-engine/src/lib.rs` delegates state across modular sub-structures:

```rust
pub struct Env {
    pub vars: Arc<RwLock<FxHashMap<String, Val>>>,
    pub local_vars: Option<Arc<RwLock<FxHashMap<String, Val>>>>,
    pub fns: Arc<RwLock<FxHashMap<String, (Vec<Param>, Option<String>, Vec<Stmt>)>>>,
    pub aliases: Arc<RwLock<FxHashMap<String, String>>>,
    pub builtins: Arc<RwLock<FxHashMap<String, BuiltinHandler>>>,
    pub caps: Arc<CapsSubsystem>,
    pub hooks: Arc<HooksSubsystem>,
    pub reactive: Arc<ReactiveSubsystem>,
    pub prompt: Arc<PromptSubsystem>,
    pub job_control: Arc<JobControlSubsystem>,
    pub options: Arc<RwLock<ShellOptions>>,
    pub profiler: Arc<RwLock<ProfilerState>>,
    pub ast_cache: Arc<RwLock<AstCache>>,
}
```

all internal synchronization uses `parking_lot::RwLock` (re-exported via `fshell_core::RwLock`).

### variable scoping

- **`env.push_scope(locals)`**: creates a cloned child `Env` pointing to a local variable map (`local_vars`). reads fall through to outer environment variables; writes via `local` update only the active frame.
- **built-in variables**: special shell variables (`$?`, `$#`, `$@`, `$*`, `$0`..`$9`) are resolved dynamically during variable lookup.

### runtime lock ordering

to guarantee deadlock freedom across concurrent pipeline stages and background tasks, the engine enforces a strict lock acquisition hierarchy (`docs/LOCK-ORDERING.md`):

$$\text{caps} \longrightarrow \text{vars} \longrightarrow \text{fns} \longrightarrow \text{jobs} \longrightarrow \text{reactive} \longrightarrow \text{tracked} \longrightarrow \text{options}$$

in debug builds, acquiring locks out of order panics immediately via runtime instrumentation macros (`lock_caps!`, `lock_vars!`, `lock_fns!`, etc.). locks are never held across `.await` yield points or subprocess spawns.

---

## pipeline execution & async dataflow

### tokio streaming model

pipelines execute asynchronously. each stage runs in its own tokio task, communicating with downstream consumers via bounded `mpsc` channels (`crates/fshell-engine/src/pipeline.rs`):

```rust
pub enum PipelinePayload {
    Data(Arc<Val>),
    Bytes(Vec<u8>),
    Structured(FshDiag),
}
```

- structured data is passed as `PipelinePayload::Data(Arc<Val>)`, avoiding deep memory clones between stages.
- byte streams from external binaries are converted into line-buffered `Val::String` items or forwarded directly to stdout.

### command dispatch hierarchy

when executing a command stage in a pipeline, resolution follows this order:

1. **alias expansion**: expands registered aliases unless shadowed by built-in keywords.
2. **user-defined function**: executes matching `fn` definitions in a scoped local frame.
3. **built-in command**: invokes the matching in-process `BuiltinHandler`.
4. **fallback handler (`fshell-bridge`)**: searches system `$PATH` via the path cache, checks capabilities, applies sandboxing profiles, and spawns the external process.

### stage execution

pipeline keywords (`filter`, `map`, `sort`, `grep`, `mark`, `count`, `limit`, `traverse`, `hash`) run as native streaming transformers inside the engine.

when a keyword is followed by a command-line flag (`sort -u`), the parser escapes keyword handling and delegates the call to external binary execution.

### boundary serialization

when a pipeline connects to an external command or terminal output, boundary operators serialize the `Val` stream into bytes:
- `@json`: writes JSON arrays or line-delimited JSON.
- `@yaml`: serializes YAML documents.
- `@msgpack`: writes binary MessagePack streams.
- `@csv`: emits comma-separated rows.
- `@table`: formats records as Unicode/ASCII terminal tables.
- `@bar`: renders terminal horizontal distribution charts.

---

## posix polyglot integration (`fshell-posix`)

### in-process execution

`fshell-posix` provides a full POSIX-compliant parser and evaluator. it does not invoke `/bin/sh` or `/bin/bash`.

instead, the POSIX abstract syntax tree executes against the same `fshell_engine::Env`:
- POSIX shell variables read and write to `env.vars`.
- POSIX functions register into `env.fns`.
- POSIX pipelines reuse `execute_pipeline` and `fshell-bridge`.

### transparent dispatch

the shell seamlessly switches execution modes:
1. **shebang detection**: `source script.sh` automatically routes to `fshell-posix` if the file begins with `#!/bin/sh` or `#!/bin/bash`.
2. **inline blocks**: `sh { ... }`, `posix { ... }`, and `bash { ... }` blocks parse the inner string as POSIX source and evaluate it in-process.
3. **CLI flag**: `fsh --posix script.sh` evaluates the entire file in POSIX mode.

---

## crate deep dives

### fshell-core
contains no dependencies on other workspace crates. defines the AST grammar (`Expr`, `Stmt`, `PipelineStage`), the `Parser` implementation, the `Val` enum, and the unified error system (`ShellError { code: ErrorCode, message, span, help }`, `FshDiag`, `ParseError`) with the `ErrorCode` taxonomy (`FSH-*-###`), and common hashing aliases (`FxIndexMap`).

### fshell-capabilities
maintains capability tokens, tracks permission grants, and verifies file path prefixes and socket rules before operations run.

### fshell-hash
provides sponge-based hashing algorithms, SHA-2/SHA-3/BLAKE3 primitives, and exports `FxBuildHasher` used throughout the workspace for fast non-cryptographic hash tables.

### fshell-engine
the runtime engine. implements `eval_stmt`/`eval_expr` returning `Result<Flow, ShellError>` where `Flow { Normal, ConditionFalse, Break, Continue, Return(Val), Exit(i32) }` separates control flow from errors, pipeline orchestration via `PipelineFailure { ConditionFalse, Hard(FshDiag) }`, reactive watch cells, session handoffs, AST caching, and execution profiling.

### fshell-builtins
registers built-in shell utilities (`cd`, `pwd`, `cp`, `mv`, `rm`, `ps`, `df`, `kill`, `curl`, `jq`, `sql`, `vault`, etc.). feature-gated to allow building lightweight or minimal distributions.

### fshell-bridge
handles external subprocess execution, process IO pipes, argument globbing, and the "did you mean?" command suggestion engine.

### fshell-posix
contains the POSIX grammar lexer, parser, and interpreter that operates directly on `fshell_engine::Env`.

### fshell-ls
high-performance file listing library. inspects filesystem metadata, queries `fshell-git` for repo status, formats human-readable file sizes, and renders tree or grid layouts.

### fshell-git
fast git repository inspector. reads HEAD, refs, stashes, and dirty worktree states to populate prompt status segments and `ls` metadata columns.

### fshell-render
miette-based diagnostic rendering engine. converts engine and parser errors into graphical terminal snippets with source spans, compact single-line messages, or JSON error objects.

### fshell-sandbox
implements subprocess sandboxing via OS-level security primitives:
- **Linux**: Landlock security rules and namespace unsharing.
- **macOS**: SBPL (Seatbelt) sandbox profiles applied in subprocess `pre_exec` fork hooks.

### fshell-config-tui
an interactive configuration manager built with Ratatui, allowing users to toggle shell options, configure themes, and edit prompt segments visually.

### fshell-panes
terminal multiplexing core supporting split panes, tabbed windows, session attaching, and daemon communication (`fshell-panesd`).

### fshell-repl
the interactive terminal frontend. integrates Reedline for line editing, FTUI for TUI rendering, SQLite history storage, fuzzy completion menus, and real-time syntax highlighting.

---

## security & sandboxing

fshell enforces two independent layers of security:

1. **capability-based authorization**: scripts running in `--strict` mode cannot access the filesystem or network unless wrapped in explicit `with caps(...) { ... }` blocks.
2. **subprocess sandboxing (`fshell-sandbox`)**: external commands can be restricted to designated read/write directories and denied network access via OS kernels (Landlock/SBPL).
3. **destructive command confirmation**: dangerous commands (`rm -rf /`, formatting root devices, raw block writes) trigger an interactive confirmation prompt before dispatching.

---

## performance architecture

- **$O(1)$ property access**: `Ustr` string interning ensures map lookups avoid repeated string hashing.
- **zero-copy streaming**: `Arc<Val>` payloads move across tokio channel buffers without cloning underlying heap structures.
- **AST caching**: sourced scripts cache parsed AST representations keyed by path mtime and content hash (`AstCache`), avoiding repeated lexical parsing.
- **release compilation profile**: the release binary compiles with fat LTO, `codegen-units = 1`, stripped symbols, and `panic = "abort"` for minimal startup latency and optimal cache locality.
