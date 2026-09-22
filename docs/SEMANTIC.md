# Semantic layer (natural-language shell interaction)

fshell can accept a *structured* description of what the user wants — a **semantic action** —
independently of how a shell would express it. The layer exists so that a small local
function-calling model (e.g. FunctionGemma 270M) only has to understand intent and extract
typed parameters, while fshell keeps all shell semantics: flags, quoting, validation, safety,
execution and per-platform differences.

```
natural language
      │
      ▼
small function-calling model            ← not integrated yet (see "Deferred")
      │
      ▼
typed fshell semantic action            ← crates/fshell-semantic
      │
      ├──▶ fsh   representation / execution   (native builtins + pipeline operators)
      ├──▶ POSIX representation / execution   (portable shell script)
      └──▶ clarification / explanation / rejection
```

**There is no `execute_shell(command: string)` entry point.** The input is always a typed
action. A raw-command fallback for requests the semantic system cannot represent is
deliberately out of scope; the design leaves room for one later without contaminating the
structured path.

## Where it lives

| Piece | Location |
|---|---|
| Semantic core (types, validation, risk, lowering, schema) | `crates/fshell-semantic` |
| `intent` builtin (the integration point) | `crates/fshell-builtins/src/intent.rs` |
| Evaluation corpus | `tests/fixtures/semantic/corpus.json` |
| Tests | `crates/fshell-semantic/src/tests.rs`, `tests/semantic_tests.rs` |

Dependency order: `fshell-core` → `fshell-engine` → `fshell-semantic` → `fshell-builtins`.
The semantic crate does not depend on `fshell-posix`; POSIX execution goes through the
existing registered handler. Lowering to fsh builds a `fshell_core::Pipeline` **AST
directly** (re-parsing generated source would reintroduce quoting bugs); lowering to POSIX
produces a script string. Execution reuses `execute_pipeline`/`spawn_pipeline_stream` and
`posix_handler()` — no shell logic is duplicated.

## The semantic model

```rust
enum IntentMode { Perform, Explain, Inform, Unsupported }

struct Intent {
    mode:   IntentMode,
    action: Option<Action>,     // for Perform/Explain
    info:   Option<InfoQuery>,  // for Inform
    issues: Vec<Issue>,         // empty ⇒ complete
}
```

An `Action` is `#[serde(tag = "kind")]`, so its JSON form is flat and easy for a small model
to emit, e.g.:

```json
{ "kind": "find_files", "root": ".", "extension": "log", "min_size": "500MB", "modified_within": "7d" }
```

Every parameter is optional in the type, so a partially understood request is representable
without inventing values. Requiredness is declared **once** per action in `spec.rs` and
consumed by both validation and schema generation.

`ByteSize` accepts an integer number of bytes or a string like `"500MB"`/`"1GiB"`; `TimeSpan`
accepts seconds or `"7d"`/`"2h"`/`"30m"`.

## Intent vs execution mode

The distinction is explicit and testable:

| Request | mode |
|---|---|
| "kill pid 1234" | `Perform` |
| "how do I kill pid 1234?" | `Explain` (same action, never executed) |
| "what does SIGTERM do?" | `Inform` (answered by fshell, no action) |
| anything unmappable | `Unsupported` |

An explanatory question cannot accidentally become an executable operation, because the
mode — not the presence of an action — decides what happens.

## Incomplete and ambiguous requests

```rust
enum Issue { Missing { param, description }, Ambiguous { param, candidates, reason }, Invalid { param, reason } }
```

`validate(&Action)` derives `Missing` issues from the spec's required list and checks a few
cross-field invariants (`min_size ≤ max_size`, every `run_container` port mapping has a host
port, ...). `render_clarification(&Intent)` turns issues into user-facing text, e.g.:

> I need more information before I can do that: image (the container image to run).
> Understood so far: {"kind":"run_container","ports":[{"host_port":8000}],"detached":false}

Ambiguity ("remove chrome") is represented as `Ambiguous` with candidate readings. Detecting
ambiguity needs world knowledge, so that belongs to the model/caller; the layer provides the
representation and the clarification rendering.

## Initial supported operations (18)

One general operation per task, parameterised — not one tool per phrasing.

| kind | category | notes |
|---|---|---|
| `list_processes` | process | filter by name/user/min-memory/min-cpu, sort, limit |
| `inspect_process` | process | by PID |
| `signal_process` | process | by PID or name; **destructive** |
| `memory_info` | system | |
| `system_info` | system | |
| `disk_usage` | system | filesystem summary or per-directory |
| `find_files` | files | name/extension/type/size/age/hidden/depth/limit |
| `search_text` | files | recursive content search |
| `list_directory` | files | |
| `delete_path` | files | **destructive** |
| `listening_ports` | network | optional port/protocol |
| `network_interfaces` | network | |
| `git_status` | git | |
| `git_branches` | git | |
| `run_container` | containers | image, ports, env; **destructive** |
| `list_containers` | containers | |
| `service_control` | services | status/start/stop/restart; destructive unless `status` |
| `environment_info` | environment | |

## fsh and POSIX targets

The same action lowers differently per target and per host:

| Action | fsh | POSIX |
|---|---|---|
| `find_files` | `ff … \| filter (name ~ "…") \| limit N` (native) | `find … -name … -size +Nc -mmin … \| head` |
| `list_processes` | `ps -a \| filter … \| sort rss desc \| limit 5` | `ps aux \| awk … \| sort -k6 -n -r \| head -n 5` |
| `memory_info` | `vm_stat` (macOS) / `free -h` (Linux) | same |
| `listening_ports` | `lsof -nP -i:8000`, else `ss -lntup` | `lsof -nP -i:8000` |
| `service_control` | `systemctl …` / `launchctl list …` | same; unsupported where no manager exists |

Platform awareness lives in `platform.rs` (`Os` + a probeable, test-overridable
`UtilitySet`). Lowering never hardcodes GNU assumptions: sizes are emitted **byte-precise**
(`find -size +500000000c`) rather than relying on diverging unit suffixes, and unsupported
combinations return `LowerError::Unsupported { reason }`.

The fsh lowering prefers native builtins (`ps`, `ls`, `ff`, `env`, `kill`) and native
pipeline operators. One detail worth recording: fsh expands globs and braces even inside
quoted strings, so a glob passed as a *command argument* would be rewritten against the
working directory. `find_files` therefore matches names with a native `filter name ~ <regex>`
stage (regex derived from the glob/extension) instead of `ff`'s `name = <glob>` argument.

## Safety

The semantic layer does not add an execution path — it lowers to ordinary fsh/external
commands, so everything still runs through `enforce_capability`, `check_destructive_command`,
the sandbox and strict mode. On top of that it adds *pre-execution* structure:

- `Action::risk() → Safe | Caution | Destructive` — a structural classification (we *know*
  that signalling, deleting, running containers and mutating services are destructive), which
  is stronger than the existing name-string block lists.
- `Action::required_caps() → Vec<ResourceHandle>` — inspectable before any shell code exists.
- The `intent` builtin prints the risk, the required capabilities and both renderings before
  doing anything; `--run` executes only after the risk gate. A destructive action is
  confirmed interactively and **refused when stdin is not a terminal**, and the decision is
  recorded with `Env::log_audit` (visible via `caps-audit`).

## FunctionGemma schemas

`fshell_semantic::schema` derives one tool per action kind from the Rust types via `schemars`,
with `required` taken from the spec table — so the types and the schemas cannot drift:

```rust
functiongemma_tools() -> Vec<ToolSchema>   // one per kind
tools_json() -> serde_json::Value          // {"tools":[{ "type":"function","function":{…}}]}
parameter_schema(kind) -> Option<Value>
```

The output is the OpenAI-style function-calling format, which a thin adapter reshapes into
FunctionGemma's exact call syntax. `intent --schema [--pretty]` prints it. Tool count is kept
small (18) with behaviour grouped through parameters, so a 270M model never has to choose
between near-identical actions.

## Worked example

```
$ intent --file plan.json       # {"kind":"find_files","root":"docs","extension":"md","min_size":"1KB"}
find_files — Find files under a directory by name, type, size and modification time. (risk: safe)
capabilities: ReadDir("docs")
fsh:   ff "docs" "size" ">=" 1000 | filter (name ~ "\.md$")
posix: find 'docs' -name '*.md' -size +1000c
(target: fsh, re-run with --run to execute)

$ intent --file plan.json --run | count
10
```

## Tests

- Unit tests in `crates/fshell-semantic/src/tests.rs`: value parsing, serde round-trips,
  validation, risk, exact fsh/POSIX renderings, platform divergence, schema contents.
- `tests/semantic_tests.rs`: the corpus contract (every prompt's expected action validates to
  the expected issues and lowers to both targets), in-process execution of a lowered pipeline
  against the real engine, and subprocess tests of the builtin (actions, schema, plan,
  clarification, fsh + POSIX execution, and the destructive-action refusal).
- `tests/fixtures/semantic/corpus.json` is the seed evaluation corpus: prompt → expected
  semantics. It is intentionally small; it is the contract a model will later be scored
  against.

Run scoped:

```sh
cargo test -p fshell-semantic
cargo test --test semantic_tests
cargo clippy -p fshell-semantic --all-targets -- -D warnings
```

## Deferred (intentionally not implemented yet)

- Any FunctionGemma integration, adapter, model download or fine-tuning.
- A raw command-generation fallback (a separate small code model may add it later, isolated
  from the structured path).
- Wiring the REPL agent overlay to emit structured `Intent`s.
- Native builtins for `free`/`du`/`ss`/`docker`/`systemctl` — external lowerings prove the
  model first; adding native builtins later only improves the fsh backend.
- Larger corpora and automatic ambiguity detection.
- In-shell ergonomics for `--json`: because fsh expands braces inside quotes, embedding
  comma-containing JSON in a `-c` string is unreliable; use `--file` (or stdin) instead.

## Recommended next step

Expose `intent --schema` to **stock** FunctionGemma 270M (no fine-tuning), run the seed
prompts from `tests/fixtures/semantic/`, and diff the emitted `kind`/params against the
expected semantics to build the failure analysis that drives dataset construction — *then*
fine-tune.
