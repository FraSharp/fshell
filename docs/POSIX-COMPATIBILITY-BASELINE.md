# POSIX compatibility baseline

This small differential smoke suite checks a defined set of non-interactive POSIX shell behaviors. It runs each case with `fsh --posix`, `dash`, and `bash --posix`. The reference shells must both match the committed expected output and status before an fshell result is counted.

Run it from the repository root:

```sh
cargo build
scripts/posix-compat-baseline.sh
```

The runner also accepts a path to an already-built fshell binary. `DASH_BIN`, `BASH_BIN`, and `FSH_BIN` can override the selected binaries. A missing reference shell is an error; install `dash` and `bash` to run the comparison.

## Current scope

The 19 cases cover quoting and positionals; parameter defaulting, assignment, and pattern removal; IFS splitting; pathname expansion; assignment-value whitespace and glob behavior; loops and conditionals; functions; command substitution; arithmetic; subshell isolation; `case`; pipeline status; quoted and expanding here-documents; file redirection; `break`/`continue`; negation; and `&&`/`||` short-circuiting. These cases use POSIX shell syntax and compare output, exit status, and unexpected stderr.

This is a starter baseline, not a POSIX conformance suite. Passing it means only that these examples match the two reference shells. It does not cover interactive behavior, job control, traps, the full builtin set, locale behavior, or all standards edge cases. Bash-only syntax belongs in a separate future Bash compatibility suite.

## Current result

On 2026-09-22, all 19 cases agreed across `fsh --posix`, `dash`, and `bash --posix`.

The command-substitution case first exposed that assignment values were being field-split: a captured embedded newline became a space. Assignment words now use their own expansion path, which preserves whitespace and skips field splitting and pathname expansion. The newline regression is in `scripts/posix-compat-baseline/05-command-substitution.sh`; the no-globbing assignment check is in `19-assignment-no-globbing.sh`. Keep compatibility corrections tied to small reproducers like these cases, and rerun this script after changes.
