# Contributing to fshell

fshell is both a shell language and a Unix runtime, so changes can affect parsing, execution, external commands, and compatibility with other shells. Make the intended behavior clear first, then test it at the layer that owns it.

## Start with the behavior

For a bug, include the smallest command or script that demonstrates it and the behavior you expect. For a language or builtin change, describe what users will observe, including exit status, stdout/stderr, and interactions with pipelines where relevant. If behavior differs between the native and POSIX frontends, say so explicitly.

For changes that cross parser, evaluator, and runtime boundaries, sketch the intended semantics before making a broad refactor. An issue or draft pull request is a useful place to resolve design questions early.

## Find the right part of the tree

The Cargo workspace contains the root `fshell` package, 16 crates under `crates/`, and the `fshell-fuzz` package:

| Package | Responsibility |
|---|---|
| `fshell` (workspace root) | Binary startup, CLI wiring, and top-level integration tests |
| `fshell-core` | Shared values, AST, and parser foundations |
| `fshell-capabilities` | Capability tokens and authorization registry |
| `fshell-engine` | Native evaluation, runtime environment, and pipeline execution |
| `fshell-semantic` | Shell-independent semantic actions and POSIX/native lowering |
| `fshell-builtins` | Builtin implementations and registration |
| `fshell-archive` | Bounded in-process archive extraction; optional through the builtins `extract` feature |
| `fshell-bridge` | External-command fallback and command-not-found behavior |
| `fshell-repl` | Interactive REPL, configuration, and history |
| `fshell-ls` | Git-aware directory listing library |
| `fshell-render` | Structured shell error rendering |
| `fshell-tty` | Unix terminal input, raw mode, ANSI primitives, and lifecycle guards |
| `fshell-terminal` | Ratatui backend, terminal sessions, and TUI runner |
| `fshell-sandbox` | Linux Landlock and macOS Seatbelt sandboxing |
| `fshell-hash` | Sponge-based hash provider |
| `fshell-git` | Git integration shared by listing and builtins |
| `fshell-posix` | POSIX shell parser and evaluator on the shared runtime |
| `fshell-fuzz` | Cargo-fuzz targets; intentionally excluded from ordinary test binaries |

For deeper detail, see `docs/LANGUAGE.md` for language semantics, `docs/ARCHITECTURE.md` for runtime structure, and `docs/LOCK-ORDERING.md` for lock safety.

## Build on a supported platform

fshell supports Unix systems: macOS and Linux. The workspace uses Rust edition 2024 and declares Rust 1.85 as its minimum version.

```sh
git clone https://github.com/FraSharp/fshell.git
cd fshell
cargo build
```

The default build enables no optional features and needs only the Rust toolchain and a C compiler. The `full` feature set (`cargo build --features full`) adds native archive extraction and the other optional builtins, and requires `pkg-config`, Clang/libclang, libarchive 3.6+ development headers, and static libarchive and codec libraries. See the installation section in [README.md](README.md) for platform-specific packages.

## Choose checks for your change

At minimum, check formatting and run the tests that directly cover your change. Contributors are not expected to reproduce the full multi-platform CI matrix locally; run broader workspace tests, fuzz checks, audits, or benchmarks when the change warrants them.

The root package contains the command-line application and domain-split integration tests. `cargo test` runs that package; `cargo test --workspace` also runs available tests across workspace members.

| Command | What it checks |
|---|---|
| `cargo fmt --check` | Rust formatting |
| `cargo build` | Default debug build |
| `cargo test` | Root package unit and integration tests (default features) |
| `cargo test --features full` | Root package tests with the full feature set (native archive dependencies required) |
| `cargo test --workspace` | Available tests across workspace members |
| `cargo test --test conformance_tests conformance_composition` | Pipeline and composition compatibility cases |
| `cargo clippy --all-targets -- -D warnings` | Clippy checks for all root-package targets |
| `cargo check -p fshell-fuzz` | Fuzz-target compilation |
| `cargo audit` | Dependency vulnerability audit |
| `cargo bench` | Criterion benchmarks |

CI's Clippy gate covers the root package's targets. A workspace-wide Clippy run currently reports existing `unwrap_used` and `panic` lint violations in member-crate tests, so it is not a required local check.

CI runs on pushes and pull requests to `main`. Both the default (no optional features) and `--features full` configurations build, test, and lint the root package on all four native targets; the `full` job also verifies portable archive linkage:

| Runner | Rust target |
|---|---|
| `ubuntu-latest` | `x86_64-unknown-linux-gnu` |
| `ubuntu-24.04-arm` | `aarch64-unknown-linux-gnu` |
| `macos-15-intel` | `x86_64-apple-darwin` |
| `macos-latest` | `aarch64-apple-darwin` |

Formatting, the native-language baseline, fuzz-target checking, and `cargo audit` run on `ubuntu-latest` only. Releases build and package all four targets on `v*` tags with `--features full`; the exact jobs are in [the CI workflow](.github/workflows/ci.yml) and [the release workflow](.github/workflows/release.yml).

## Treat shell compatibility as test data

Integration tests are split across `tests/*_tests.rs`. The compatibility corpus is `tests/conformance_tests.rs`, with its harness under `tests/common/conformance/`. There are two frontends, `Native` and `Posix`; `Auto` invokes fsh without forcing either and tests runtime routing (including POSIX shebang detection), rather than representing a third language implementation.

- Each case names the engine or engines it covers (`Native`, `Posix`, or `Auto`). Test each affected engine independently; success in one must not conceal a defect in another.
- The POSIX oracle compares `bash --posix` with `dash` and judges a case only when they agree. Use the Bash-specific oracles for behavior POSIX shells do not define.
- Mark an expected current failure with `known_failure(id, reason, engines)`. The marker is strict: remove it once the case passes.
- Keep `argvdump` output non-JSON. The native engine can decode and re-encode JSON output before the harness observes it.
- The fixture resets its environment and working tree between invocations. Preserve that isolation for cases that mutate files or depend on environment state.

The native golden baseline lives in `scripts/fsh-language-baseline/` and is run in CI by `scripts/fsh-language-baseline.sh` on Ubuntu x86_64.

## Make review straightforward

Keep a pull request focused. Its description should identify the behavior changed, why the change is needed, which engines or platforms are affected, and the checks that were run. Include documentation updates for user-visible language, builtin, or CLI changes. For terminal UI changes, a short recording or screenshot can help reviewers understand the interaction.

## Commit subjects

Project history generally uses `<scope>: <lowercase imperative summary>`, with scopes for a crate or area such as `builtins`, `parser`, `ci`, `tests`, or `docs`. Explain the motivation and behavior change in the commit body; keep unrelated changes in separate commits.

## License

fshell is distributed under the [GPL-3.0-or-later](LICENSE) license.
