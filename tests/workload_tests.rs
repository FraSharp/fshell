//! Agent workload corpus.
//!
//! The conformance corpus asks *is this construct right?* one construct at a
//! time. This suite asks the question that decides whether the shell is usable:
//! **can it run the commands a coding agent actually produces?** The inputs here
//! are real shapes taken from agent sessions — build-and-grep pipelines, probe
//! functions, heredocs, temp-dir workflows, history triage, redirection matrices
//! — and they are judged the same way the conformance corpus judges everything:
//! differentially, against the reference shells.
//!
//! Two rules make that possible without cost or risk:
//!
//! * **Nothing leaves the sandbox.** Each case runs in its own fixture with a
//!   cleared environment, `$HOME`/`$TMPDIR` inside the tree, and a fresh tree
//!   rebuilt before every invocation.
//! * **The tooling is stubbed.** `cargo`, `git` and `python3` are shell stubs on
//!   `PATH` (see [`STUBS`]), so the shape of the command is faithfully exercised
//!   while a whole suite finishes in well under a second. A workflow is about how
//!   a shell *drives* a tool, not about the tool.
//!
//! The metrics this suite reports are the ones worth watching over time: total
//! workflows, semantic passes, routing mismatches, unsupported constructs,
//! crashes and hangs. Every mismatch that is expected today carries a strict
//! `known_failure` marker, so a compatibility bug is pinned rather than tolerated,
//! and a marker whose bug is fixed fails the suite until it is removed.

mod common;

use common::conformance::{Case, Engine, assert_suite};

/// Sandbox tools, all of them stubs: instant, offline, deterministic, and unable
/// to touch anything outside the fixture.
const STUBS: &[(&str, &str)] = &[
    (
        "cargo",
        r#"#!/bin/sh
case "${1:-}" in
  fmt) exit 0 ;;
  clippy) echo 'warning: unused import: `std::fmt`' >&2; exit 0 ;;
  build)
    echo '   Compiling fshell-core v0.1.0'
    echo '    Finished `dev` profile [unoptimized] target(s) in 0.01s'
    exit 0 ;;
  test)
    if [ "${2:-}" = "--no-run" ]; then
      echo 'error[E0599]: no method named `take` found in the current scope' >&2
      echo 'warning: `fshell-engine` (lib test) generated 1 warning' >&2
      exit 0
    fi
    echo '   Compiling fshell-engine v0.1.0'
    echo '    Finished `test` profile [unoptimized] target(s) in 0.02s'
    echo 'test result: ok. 203 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out'
    echo ''
    echo 'test result: FAILED. 12 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out'
    echo 'error: test failed, to rerun pass `--lib`' >&2
    exit 101 ;;
  *) echo "cargo: no such command: ${1:-}" >&2; exit 1 ;;
esac
"#,
    ),
    (
        "git",
        r#"#!/bin/sh
case "${1:-}" in
  log)
    echo '99a7924 engine, core, posix, builtins, tests: record command outcomes explicitly'
    echo '335725e posix, engine: run subshells as child processes' ;;
  status) printf '?? src/scratch.rs\n M README.md\n' ;;
  rev-parse) pwd ;;
  *) exit 0 ;;
esac
"#,
    ),
    (
        "python3",
        r#"#!/bin/sh
# Reads its program from stdin, so a heredoc is delivered exactly as it would be.
cat > /dev/null
echo 'analysis: 3 files, 0 problems'
"#,
    ),
    (
        "fsh",
        r#"#!/bin/sh
# Stands in for the shell under test when a workflow shells out to `fsh`: the
# outer shell is the one on trial.
echo 'rc=0 out="ok"'
"#,
    ),
];

/// The checkout a workflow expects to find.
const TREE: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/fshell-core\"]\n",
    ),
    (
        "README.md",
        "# fsh-demo\n\nbuild with cargo test\nknown_failure markers live in docs and tests\n",
    ),
    ("src/main.rs", "fn main() {}\n"),
    ("crates/fshell-core/src/lib.rs", "pub fn core() {}\n"),
    ("tests/pipelines_tests.rs", "// streams\nfn pipeline() {}\n"),
    ("docs/notes.md", "known_failure: native word model\n"),
    ("cases.txt", "alpha one\nalpha two\nbeta one\n"),
];

/// A workload in the POSIX engine's own terms.
///
/// The scripts are POSIX-shaped, which is the point — they are what an agent
/// writes. Judging them means judging `--posix`; whether *auto* mode picks the
/// POSIX engine for a given one is a routing question, and the routing and
/// dispatch suites are where that is asserted, so this corpus does not restate
/// it.
fn posix_case(name: &'static str, script: &'static str) -> Case {
    Case::posix_only(name, script).tree(TREE).stubs(STUBS)
}

/// Build-and-inspect pipelines: the single most common agent command shape.
fn build_pipeline_cases() -> Vec<Case> {
    vec![
        posix_case(
            "workload/build-and-verify",
            r#"cargo fmt
cargo test -p fshell-engine --lib 2>&1 | grep -E "^test result|^error" -A 4 | head -20
echo "=== core ==="
cargo test -p fshell-core --lib 2>&1 | grep -E "^test result"
echo "=== no-run ==="
cargo test --no-run 2>&1 | grep -cE "^error""#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "build", "pipeline"]),
        posix_case(
            "workload/build-triage",
            r#"cargo build --bin fsh 2>&1 | tail -3
echo "errors: $(cargo build --bin fsh 2>&1 | grep -cE '^error')"
cargo clippy --all-targets 2>&1 | head -2"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "build", "count"]),
        // The agent idiom: a helper that runs a command, keeps its status and
        // flattens the output onto one line. Bash-only by construction (`local`,
        // `${var//…}`), which is exactly how agents write these.
        posix_case(
            "workload/probe-helper-function",
            r#"probe() { local out rc; out=$("$@" 2>/dev/null); rc=$?; out=${out//$'\n'/|}; printf '%-16s rc=%-3s %s\n' "$1" "$rc" "$out"; }
probe emit --stdout one
probe emit --exit 3
probe printf 'a\nb\n'
probe emit --stdout ab"#,
        )
        .bash_only()
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "function", "capture"])
        .known_failure(
            "posix-substitution-loses-the-command-status",
            "a command substitution records its status while expanding, but a \
             pure assignment then reports its own 0 instead of taking it, so a \
             probe that captures output and reads `$?` sees success after a failure",
            &[Engine::Posix],
        ),
        // A shell inside a shell, both of them stubs: what is under test is the
        // quoting and dispatch of the outer one.
        posix_case(
            "workload/nested-shell-invocation",
            r#"fsh --native -c 'emit --stdout inner' | head -1
fsh -c "emit --stdout quoted" 2>&1 | head -1
command -v fsh"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "nested"]),
    ]
}

/// Temp directories, heredocs and analysis scripts: the shapes that make agents
/// useful, and the ones that most often assume bash.
fn scripted_analysis_cases() -> Vec<Case> {
    vec![
        posix_case(
            "workload/temp-dir-workflow",
            r#"D=$(mktemp -d)
cp cases.txt "$D/"
if [ -f "$D/cases.txt" ]; then echo "copied"; else echo "missing"; fi
sort "$D/cases.txt" | head -2
rm -rf "$D"
echo "cleaned""#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "tempdir"]),
        posix_case(
            "workload/heredoc-analysis",
            r#"python3 - <<'PY'
import os
print("files:", len(os.listdir(".")))
PY
echo "exit=$?""#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "heredoc", "python"]),
        posix_case(
            "workload/heredoc-to-file",
            r#"cat <<'EOF' > note.txt
line one
line two
EOF
wc -l < note.txt
cat note.txt"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .files(&["note.txt"])
        .features(&["workload", "heredoc", "redirection"]),
        posix_case(
            "workload/search-and-count",
            r#"grep -rn "known_failure" docs tests | head -3
grep -c alpha cases.txt
awk '{print $1}' cases.txt | sort | uniq -c | sort -rn | head -3
sed -n '1,2p' cases.txt"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "grep", "awk"]),
        posix_case(
            "workload/history-triage",
            r#"git log --oneline -8 | head -2
git status --short | grep '^??' | head -5
if git rev-parse --show-toplevel > /dev/null 2>&1; then echo "in a repo"; fi"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "git"]),
        posix_case(
            "workload/find-and-pipe",
            r#"find . -name '*.rs' -not -path './target/*' | sort
find . -maxdepth 1 -name '*.txt' | wc -l"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "find"]),
    ]
}

/// Control flow an agent reaches for: loops with counters, case dispatch,
/// read loops, and the failure handling that surrounds them.
fn control_flow_cases() -> Vec<Case> {
    vec![
        posix_case(
            "workload/loop-with-counter",
            r#"misses=0
for f in cases.txt README.md missing.txt; do
  if grep -q alpha "$f" 2>/dev/null; then
    echo "$f: hit"
  else
    misses=$((misses + 1))
    echo "$f: miss"
  fi
done
echo "misses=$misses""#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "loop", "arithmetic"]),
        posix_case(
            "workload/case-dispatch",
            r#"for f in cases.txt README.md out.bin; do
  case "$f" in
    *.txt) echo "text $f" ;;
    *.md) echo "doc $f" ;;
    *) echo "other $f" ;;
  esac
done"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "case"]),
        posix_case(
            "workload/read-loop",
            r#"while IFS= read -r line; do
  echo "- $line"
done < cases.txt"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "read"]),
        posix_case(
            "workload/errexit-stops",
            r#"set -e
echo before
false
echo unreachable"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "errexit"]),
        posix_case(
            "workload/trap-cleanup",
            r#"trap 'echo cleanup' EXIT
echo body"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "trap"]),
        posix_case(
            "workload/subshell-scope",
            r#"( cd dir && basename "$PWD" )
echo "still $(basename "$PWD")"
{ echo one; echo two; } | wc -l"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "subshell"]),
    ]
}

/// Status, redirection and expansion: the shapes that decide whether a script
/// keeps going after something failed.
fn status_and_redirection_cases() -> Vec<Case> {
    vec![
        posix_case(
            "workload/pipeline-status-matrix",
            r#"nosuchcmd 2>/dev/null; echo "not-found=$?"
false | true; echo "last-stage=$?"
true | false; echo "last-stage=$?"
nosuchcmd 2>/dev/null && echo unexpected
nosuchcmd 2>/dev/null || echo recovered"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "status", "pipeline"]),
        posix_case(
            "workload/pipefail-matrix",
            r#"set -o pipefail
false | true; echo "pipefail=$?"
true | false; echo "pipefail=$?"
emit --exit 1 | emit --exit 0; echo "last=$?""#,
        )
        .bash_only()
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "pipefail"]),
        posix_case(
            "workload/redirection-matrix",
            r#"nosuchcmd 2>/dev/null; echo "silenced=$?"
emit --stdout kept > out.txt 2> err.txt; echo "rc=$?"
cat out.txt
wc -c < err.txt"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .files(&["out.txt", "err.txt"])
        .features(&["workload", "redirection"]),
        posix_case(
            "workload/expansion-operators",
            r#"echo "${UNSET_ONE:-fallback}"
echo "${EMPTY:-empty-is-unset-here}"
echo "${UNSET_ONE-default}"
echo "${NUMBER:0:2}"
echo "bar-length=${#BAR}"
echo "nested=$(echo "$(basename "$PWD")" | wc -c)""#,
        )
        .bash_only()
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "expansion"]),
        posix_case(
            "workload/break-continue",
            r#"for f in cases.txt README.md docs; do
  if [ ! -f "$f" ]; then
    echo "skip $f"
    continue
  fi
  echo "use $f"
  break
done"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "loop", "control"]),
        posix_case(
            "workload/exit-status-in-function",
            r#"check() { false; }
check
echo "rc=$?"
check || echo "recovered""#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "function", "status"]),
        posix_case(
            "workload/local-in-function",
            r#"setup() { local root=dir; echo "root=$root"; }
setup
echo done"#,
        )
        .tree(TREE)
        .stubs(STUBS)
        .features(&["workload", "function", "local"]),
    ]
}

/// Native workflows, reified literally: what the native engine promises for the
/// same shapes, judged on its own terms rather than against POSIX.
fn native_cases() -> Vec<Case> {
    vec![
        Case::native(
            "workload/native-composite-status",
            r#"if true { emit --exit 4 }; emit --stdout "rc=$?""#,
            "rc=4\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native)
        .features(&["workload", "native", "composite"]),
        Case::native(
            "workload/native-function-status",
            r#"fn check() { emit --exit 3 }
check
emit --stdout "rc=$?""#,
            "rc=3\n",
            0,
        )
        .engines(&[Engine::Native, Engine::Auto])
        .expect_engine(Engine::Native)
        .features(&["workload", "native", "function"]),
    ]
}

#[test]
fn workloads() {
    let mut cases = build_pipeline_cases();
    cases.extend(scripted_analysis_cases());
    cases.extend(control_flow_cases());
    cases.extend(status_and_redirection_cases());
    cases.extend(native_cases());
    assert_suite(&cases);
}
