# Timing traces

Set `FSH_TRACE_FILE` to append an opt-in timing trace:

```sh
FSH_TRACE_FILE=/tmp/fsh-trace.jsonl fsh
```

The file is JSON Lines. Each line is a completed span with `schema_version`,
`run_id`, typed numeric span/command/pipeline identifiers, `parent_id`, a span
name, `start_ns`, `duration_ns`, execution `mode`, stable `outcome`, and
structured `attrs`. Both timing values are monotonic nanoseconds relative to
the start of that process. A new process gets a new `run_id`; the file is
opened in append mode, so records from multiple processes can share one file.

Tracing covers process and runtime entry, CLI routing, shell initialization,
interactive session setup, first prompt readiness, command preparation and
parsing, script parsing/evaluation, pipelines and their concurrent stages,
external command resolution/capability checks/spawn/I/O/wait, history updates,
and process exit or replacement. Names and attributes are intended to describe
work without recording command text, arguments, environment values, or output.

Pipeline stages run concurrently. Their durations may overlap, and must not be
added together to estimate total pipeline time. Use parent spans and timestamps
to inspect overlap. External process I/O and process waiting are also recorded
as separate, overlapping spans.

The sink buffers writes and flushes at normal process completion, explicit
exit, and process replacement. A tracing file open/write/flush failure disables
tracing; it does not change shell command results. An open failure produces one
startup diagnostic. Active spans are written when they complete, so a hard
crash or `SIGKILL` can leave their records absent.

Tracing is disabled when `FSH_TRACE_FILE` is unset. It is separate from the
engine's existing aggregate profiler. Remove or rotate the trace file when it
is no longer needed; fshell appends and does not impose a size limit.
