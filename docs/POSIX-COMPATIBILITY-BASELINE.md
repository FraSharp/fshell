# POSIX compatibility baseline

The differential baseline for non-interactive POSIX behaviour lives in the Rust conformance
harness: suite `conformance_legacy_migrated` in `tests/conformance_tests.rs`, with the harness
itself in `tests/common/conformance/`. Each case runs with `fsh --posix`, `dash`, and
`bash --posix`; the reference shells must agree with each other before an fshell result is counted.

Run it from the repository root:

```sh
cargo test --test conformance_tests
```

There is no separate shell runner. `cargo test` is the authoritative compatibility signal, so the
suite runs in CI with no extra wiring, and there is only ever one implementation to keep honest.

## How it differs from the retired shell script

This suite replaces `scripts/posix-compat-baseline.sh` and its committed fixtures. The behaviour it
checks is the same; the enforcement is stronger:

- **Expectations are derived from the reference shells at run time**, not from committed
  `.stdout`/`.status` files. There is no golden file that can drift away from what `dash` and
  `bash --posix` actually do.
- **A missing `dash` downgrades, it does not abort.** The case is still judged against
  `bash --posix`. The old script treated a missing reference as a fatal error (exit 2), which is why
  it could only run on Linux.
- **Reference disagreement is a skip, not a failure.** fshell is not judged against a contested
  expectation.
- **Every invocation runs in a fresh fixture**, so the file-creating cases no longer share one
  working directory with each other. That removes an ordering coupling the old suite had.
- **Stderr emptiness is still asserted** for every case, via `compare_stderr()` — a reference
  produces no stderr, so a leaked fshell diagnostic fails the case.

## Current scope

The 19 cases cover quoting and positionals; parameter defaulting, assignment, and pattern removal;
IFS splitting; pathname expansion; assignment-value whitespace and glob behavior; loops and
conditionals; functions; command substitution; arithmetic; subshell isolation; `case`; pipeline
status; quoted and expanding here-documents; file redirection; `break`/`continue`; negation; and
`&&`/`||` short-circuiting. They use POSIX shell syntax and compare output, exit status, and
unexpected stderr.

This is a starter baseline, not a POSIX conformance suite. Passing it means only that these examples
match the two reference shells. It does not cover interactive behavior, job control, traps, the full
builtin set, locale behavior, or all standards edge cases. Bash-only syntax belongs with the
`bash_posix_only`/`bash_only` oracles in the same harness.

## Case history worth keeping

Small reproducers are why these cases exist; keep compatibility corrections tied to them.

- **`legacy/05-command-substitution`** first exposed that assignment values were being field-split:
  a captured embedded newline became a space. Assignment words now use their own expansion path,
  which preserves whitespace and skips field splitting and pathname expansion.
- **`legacy/19-assignment-no-globbing`** pins the companion rule: a glob metacharacter in an
  assignment value stays literal.
- **`legacy/07-pipeline-status`** pins that a pipeline's status is the status of its **last** stage
  (`false | true` is `0`).
- **`legacy/12-pathname-expansion`** exposed that a redirect target was only opened when the command
  actually wrote something, so the script's `: > baseline-a.glob` never created the file and the
  following glob stayed literal. POSIX establishes the redirection *before* the command runs, so
  `: > file` (and `> build.log` on any silent command) must create or truncate it. Fixed by applying
  redirections in source order to a descriptor table, which opens targets eagerly. The focused
  reproducers are `redirect/silent-command-still-creates-target` and
  `redirect/silent-command-still-truncates-target`.
- **Redirection order is semantics.** `redirect/stdout-then-merge-sends-both-to-file` and
  `redirect/merge-then-stdout-keeps-stderr` pin the pair that must stay distinguishable: `2>&1`
  duplicates whatever stdout points at *at that moment*, so `> out 2>&1` and `2>&1 > out` differ.
  That is only expressible with a table of descriptor destinations applied in order, which is why the
  POSIX evaluator now uses `FdTarget` (`crates/fshell-posix/src/eval.rs`) rather than a pair of
  `bool` flags — the flags were assigned and never read, which silently disabled `2>&1` entirely.
