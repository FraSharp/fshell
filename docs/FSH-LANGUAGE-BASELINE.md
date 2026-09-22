# Native fsh language baseline

Run the golden-output smoke suite from the repository root:

```sh
cargo build
scripts/fsh-language-baseline.sh
```

The runner accepts a path to an already-built fsh binary. Set `FSH_BIN` to use another binary. Each case runs in its own temporary directory, once through `fsh -c` and once from a `.fsh` file. Both executions must match the committed stdout and exit status with no stderr.

## Current scope

The 19 cases cover arithmetic and interpolation, map member access, list filtering and counting, sorting and limiting, map projection, loops, function calls and returns, local scope and shadowing, argument type errors, `match`, string modifiers, command substitution, `try`/`catch`, JSON serialization, command exit status, statement chaining, dotted input/output redirection, and structural parameter constraints.

Native fsh has no separate shell implementation to use as a behavioral oracle. These cases are executable regression examples grounded in the documented language and its current intended behavior. They are not a complete language conformance test. As the language grows, add cases by feature family and make expected behavior explicit in `docs/LANGUAGE.md` before treating the result as a compatibility promise.

## Current result

On 2026-09-23, all 19 cases passed with the workspace `fsh` binary in both command and script-file modes. Cases live in `scripts/fsh-language-baseline/`; expected output and exit status are stored beside each script.
