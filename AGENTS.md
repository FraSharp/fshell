# fshell — AGENTS.md

HARD CONSTRAINTS FOR EVERY SESSION: No patches, no workarounds, no shortcuts. Must always take the best long term architectural approach. I am open to rewrites. We have no time constraints nor money constraints. Must do it properly.

Structured-data shell (`fsh`) for Unix only (macOS/Linux; `compile_error!` on non-unix). Rust workspace, edition 2024.

## Commands

```sh
cargo build --release          # fat LTO, codegen-units=1, stripped, panic=abort
cargo run                      # run the REPL (binary: fsh)
cargo run -- -c 'ls | count'   # inline command, then exit (lighter boot via Env::for_command())
cargo run -- -s                # strict mode — no default capabilities

cargo test                     # unit + integration tests
cargo test --test pipelines_tests        # one integration suite
cargo test --test pipelines_tests filter # suite + name filter
cargo test --test conformance_tests      # differential conformance suite (vs bash/dash)
cargo test -p fshell-engine -- filter_contains   # single crate/unit test
cargo clippy --all-targets -- -D warnings
cargo fmt                      # CI runs `cargo fmt --check`
cargo bench                    # criterion benches in crates/*/benches
```

Process-level benchmarks are shell scripts under root `benches/` (`benches/process/run_all.sh`). Fuzz targets live in `fuzz/` (workspace member, `test = false` bins — plain `cargo test` skips them; CI runs `cargo check -p fshell-fuzz`; run via `cargo fuzz`).

## CI / Release

- **CI** (`.github/workflows/ci.yml`): fmt check → build → test → `clippy --all-targets -D warnings` → `cargo audit`, on push/PR to `main`.
- **Matrix:** x86_64 Linux, native aarch64 Linux, native x86_64 macOS, and native aarch64 macOS.
- **Release** (`.github/workflows/release.yml`): on `v*` tags, builds and packages `fsh` for 4 targets (x86_64+aarch64 × Linux+macOS), along with `LICENSE` and `docs/licenses`.
- **clippy cross-platform trap:** Linux `mode_t` is `u32`; macOS `mode_t` is `u16`. Filesystem modules use a narrow `#[allow(clippy::unnecessary_cast)]` where casts are needed across both platforms.
- Root `[workspace.lints.clippy]` (`unwrap_used = warn`, `panic = warn`, `expect_used = allow`); every crate inherits it via `[lints] workspace = true`. Since CI runs `clippy --all-targets -- -D warnings`, new `unwrap()`/`panic!` calls in tests and benches fail the lint check.

## Commit messages

Convention taken from `git log`:

- **Subject:** `<scope>: <lowercase imperative summary>`, no trailing period. `scope` is a crate name (`repl`, `builtins`, `parser`, `docs`) or a comma-separated list when the change spans several (`parser, builtins, repl`). Imperative mood, present tense.
- **Body:** blank line after the subject, then lowercase prose explaining *why* the change is made and what behaviour changes, optionally followed by a `-` bullet list of the concrete edits — each bullet ends with `;`, the last with `.`.
- **No trailers:** commits carry no `Signed-off-by`/`Co-authored-by` lines — never append one, including the Command Code bot trailer.

Example:

    builtins: propagate string and replace errors as exit status

    Both builtins ran their work in a detached tokio task and always
    returned Ok, so `string frobnicate` or a `replace` with a missing
    argument printed an error yet reported success ($? = 0) — the
    vacuous-success class.

    Register them as async builtins whose handler awaits the work, so
    validation and runtime errors become the failing stage's exit status.

## Binary & startup flow

- Bin targets are **`fsh`** (`src/main.rs`) plus the two conformance instruments **`argvdump`** and **`emit`** (`tests/helpers/`). Root Cargo.toml sets `autobins = false` and `default-run = "fsh"`, so `cargo run` still launches the REPL. `src/bin/fshell.rs` is vestigial source that is NOT compiled — `cargo run --bin fshell` fails.
- Multicall: if argv[0] is neither `fsh` nor `fshell`, the binary runs utility mode (currently only `ls`).
- Init order in `src/lib.rs:run()`: `fshell_core::init()` → `fshell_capabilities::init()` → `Env::new()` (REPL/script) or `Env::for_command()` (`-c` path) → `fshell_builtins::init(&env)` → `fshell_bridge::init(&env)` → `register_posix_handler(...)`. `fshell_repl::init(&env)` runs only on the interactive path. Note: all `init()` functions take `&Env`.
- POSIX mode: `--posix` flag or shebang auto-detection routes scripts through `fshell-posix` instead of the native engine.
- Handoff persistence (`fshell_engine::handoff`): restores vars/functions/caps/reactive pipelines/hooks/options/cwd from JSON — internal, used by `reload --full` via hidden `--handoff <PATH>`.

**CLI flags** (`src/lib.rs`): `[SCRIPT]`, `-c/--command`, `-s/--strict`, `--handoff <PATH>` (hidden), `--error-format graphical|compact|json`, `--no-color`, `--no-dym`, `--suggestion-mode blocking|deferred`, `-r/--resume [ID]`, `-l/--login`, `--posix`.

## Workspace map

13 crates + `fuzz`. Dependency chain: `core → capabilities → engine → {semantic → builtins, bridge}`; repl sits on engine.

| Crate | Key exports | Notes |
|-------|-------------|-------|
| `fshell-core` | `Val`, `Parser`, AST (`Stmt`, `Expr`, `PipelineStage`), `ResourceHandle`, `FxIndexMap`, `RwLock` (re-export of parking_lot), `set_var`/`get_var` | parser/AST/types, no engine deps |
| `fshell-capabilities` | `CapsRegistry` | capability tokens |
| `fshell-engine` | `Env`, `eval_stmt`/`eval_expr`, `execute_pipeline`, `env.register_builtin`/`_alias`/`env.set_fallback_handler`, free fns `register_hook`/`register_posix_handler`, `handoff`, profiler | evaluator + pipeline executor |
| `fshell-semantic` | `Action`/`Intent`/`Issue`, `validate`, `lower_fsh`/`lower_posix`, `render_*`, `tools_json` | shell-independent semantic action layer (no model integration); see `docs/SEMANTIC.md` |
| `fshell-builtins` | `init(&env)` — registers ~117 builtin entries into the env | feature-gated modules |
| `fshell-bridge` | `init(&env)` — external process fallback, glob, command-not-found | |
| `fshell-ls` | `list_dir`, `Config`, `render` | git-aware ls library |
| `fshell-git` | git integration | used by ls/builtins |
| `fshell-hash` | sponge-based hash provider | |
| `fshell-render` | `render()`, `RenderConfig`, `RenderFormat` | miette-based error rendering |
| `fshell-sandbox` | `run_sandboxed()`, `SandboxMode`/`SandboxProfile` | Landlock (Linux) / SBPL (macOS) via `pre_exec` |
| `fshell-posix` | `parse_posix_script`, `eval_source(_stream)` | POSIX sh/bash frontend sharing the engine runtime |
| `fshell-repl` | `init(&env)`, `run_repl_with_env()` | reedline + ratatui TUI, config TUI, SQLite history |

Features: default `full` enables sandbox/vault/ai/http/sql/chart/notify/ff/replace/extract; `minimal` enables none. Builtins are gated behind these per-feature in `crates/fshell-builtins`.

## Extensibility APIs (fshell_engine)

- **Builtin handler type:** `Arc<dyn Fn(Option<PipeStream>, Vec<Val>, &Env, PipeSender) -> Result<(), StringError> + Send + Sync>`. Register via methods on the live env: `env.register_builtin(name, handler)` / bulk `env.register_builtins(vec![(name, handler), ...])`.
- **Aliases:** `env.register_alias("name", "expansion")` — expanded at dispatch before anything else, but skipped when the name shadows a builtin or user-fn (builtin/fn wins).
- **Hooks:** free fn `register_hook(event, fn_name, &env)` (precmd/preexec/chpwd).
- **Fallback handler:** `env.set_fallback_handler(...)` — invoked when no alias/user-fn/builtin matches (bridge uses it for external commands).

## Test patterns

- Shared helpers in `tests/common/`: `setup_test_env()` (sandbox off, isolated frecency DB, `FSH_TEST_ENV=1`, builtins+bridge+posix init), `TestContext` (temp dir + RAII `CwdGuard`/`EnvVarGuard`), `FshCmd`/`FshOutput` subprocess runner with `.assert_success()`, `FixtureSuite`/`FixtureSpec` fixture runner.
- Integration tests are domain-split files in `tests/*_tests.rs` (builtins, pipelines, control_flow, posix_compliance, posix_fixture, sandbox, reactive, ...); fixtures in `tests/fixtures/posix`, scripts in `tests/scripts`. There is no single `integration_tests.rs` anymore.
- Locks are **parking_lot** (via `fshell_core::RwLock`) — `.read()`/`.write()` return guards directly; do NOT append `.unwrap()`.
- `Env` is composed of sub-structs: `scope` (vars), `caps`, `hooks`, `reactive`, `prompt`, `job_control`, plus `options: Arc<RwLock<ShellOptions>>`. Access like `env.reactive.pipelines.write()` or `env.caps.caps.write()`.
- Seed vars with `fshell_core::set_var(name, &val)` or `env.vars.write().insert(...)`; pipeline tests seed `Val::List` of `Val::Map` items using `FxBuildHasher` + `ustr` keys.

## Conformance testing

`tests/conformance_tests.rs` is the shell-compatibility contract. Cases are declared as data and judged against reference shells rather than written as bespoke assertions, so a discovered incompatibility becomes a permanent regression in one line. Harness code lives in `tests/common/conformance/`.

- A case names the engines it runs in — `Engine::Native` (`--native`), `Engine::Posix` (`--posix`), `Engine::Auto` (no flag). fsh has **two** engines, so a defect in one must never hide behind a correct result in the other.
- Oracle choice decides what "correct" means. `Oracle::Posix` requires `bash --posix` **and** `dash` to agree; if they disagree the case is skipped, not failed — fsh is not judged against a contested expectation. `BashPosix`/`Bash` exist for bash extensions (`<<<`, `[[ ]]`, `<( )`) that a strict POSIX shell legitimately rejects. `Expect` carries a literal expectation for native behaviour no POSIX shell models.
- **A case that fails today must carry `known_failure(id, reason, engines)`.** The marker is strict: if the case starts passing, the suite fails and demands the marker be removed. Bugs therefore cannot regress silently, and a fix cannot land while the corpus still claims the bug is open.
- Instruments: `argvdump` prints post-expansion argv, `emit` prints chosen stdout/stderr and exits with a chosen status. Both are bin targets built by `cargo test`; the fixture puts them on `PATH`.
- **Keep `argvdump`'s output non-JSON.** The native engine decodes JSON on an external command's stdout and re-encodes it as a tagged `Val`, which would rewrite the instrument before any comparison could see it. That behaviour is itself pinned by the `native/json-stdout-is-reencoded` case.
- The fixture (`tests/common/conformance/fixture.rs`) clears the environment, pins `LANG`/`LC_ALL`/`HOME`/`PATH`, and rebuilds the tree before *every* invocation. That keeps `$HOME`-dependent expansions identical across shells while preventing file-mutating cases (`> out`) from contaminating the next shell's run.
- `scripts/posix-compat-baseline.sh` was the older shell-script differential suite. Its 19 cases were migrated into the `conformance_legacy_migrated` suite and the script was retired, so there is one authoritative compatibility implementation; see `docs/POSIX-COMPATIBILITY-BASELINE.md`.
- `scripts/fsh-language-baseline.sh` (native golden cases, no oracle) is the last separate baseline and is not yet wired into CI.

## Performance-critical types

Use throughout (never raw `HashMap`/`BTreeMap`):

```rust
use fshell_core::FxIndexMap;  // ordered map with Fx hasher
use fxhash::FxHashMap;       // unordered map with Fx hasher
use ustr::ustr;               // interned string keys

// Map literal pattern:
Val::Map({
    let mut m = FxIndexMap::with_hasher(fxhash::FxBuildHasher::default());
    m.insert(ustr("key"), Val::Int(42));
    m
})
```

## Val quirks

- Serde tagged `#[serde(tag = "type", content = "value")]`; `ReactiveStream` is `#[serde(skip)]`.
- `Val::Float` equality treats NaN == NaN as true.
- `ObjectGraph` equality is structural with an `Arc::ptr_eq` fast path.

## Language / parser notes

- Reserved pipeline stage words (`crates/fshell-core/src/parser/stmt.rs`): `filter map sort grep mark count hash limit traverse`. A following `-flag` forces external-command interpretation instead (e.g. `sort -n`).
- Boundary serialization operators: `@json @yaml @msgpack @text @csv @table @bar`.
- Don't hardcode keyword lists elsewhere — parse from/with `fshell-core`; full reference is docs/LANGUAGE.md.

## Lock ordering (runtime-enforced)

Strict hierarchy — `docs/LOCK-ORDERING.md` is authoritative. In debug builds the engine enforces it: acquire in order `caps → vars → fns → jobs → reactive → tracked → options` and use the `lock_caps!`/`lock_vars!`/… macros (`lock_ordered!`) in `crates/fshell-engine/src/lib.rs`; violations panic with a backtrace location. Never hold a lock across `.await` or child-process spawn.

## Docs

| File | Content |
|------|---------|
| `docs/ARCHITECTURE.md` | Crate deep-dive and pipeline execution model |
| `docs/LANGUAGE.md` | Complete language reference |
| `docs/MIGRATION.md` | bash/zsh/fish migration guide |
| `docs/LOCK-ORDERING.md` | Authoritative lock hierarchy |
| `docs/PIPELINES.md` | Pipeline operators and data flow |
| `docs/SECURITY.md` | Capability model |
| `docs/BUILTINS.md` | Built-in commands reference |
| `docs/SEMANTIC.md` | Semantic action layer for natural-language interaction |
| `docs/CONFIGURATION.md` | Shell configuration |
