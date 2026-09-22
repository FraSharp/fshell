# Native fsh language baseline

Run the golden-output smoke suite from the repository root:

```sh
cargo build
scripts/fsh-language-baseline.sh
```

The runner accepts a path to an already-built fsh binary. Set `FSH_BIN` to use another binary. Each case runs in its own temporary directory with color disabled, then checks stdout, exit status, and stderr against committed expectations.

## Current scope

The 12 cases cover arithmetic and interpolation, map member access, list filtering and counting, sorting and limiting, loops, function calls and returns, `match`, string modifiers, command substitution, `try`/`catch`, and JSON serialization.

Native fsh has no separate shell implementation to use as a behavioral oracle. These cases are executable regression examples grounded in the documented language and its current intended behavior. They are not a complete language conformance test. As the language grows, add cases by feature family and make expected behavior explicit in `docs/LANGUAGE.md` before treating the result as a compatibility promise.

## Current result

On 2026-09-22, all 12 cases passed with the workspace `fsh` binary. Cases live in `scripts/fsh-language-baseline/`; expected output and exit status are stored beside each script.
