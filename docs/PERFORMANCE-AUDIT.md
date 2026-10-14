# Interactive performance audit

## Verdict

The small-command fast path is already responsive. The main problems are **work amplification, asynchronous result delivery, and behavior at scale**, not a universal slow Enter path.

Highest-value fixes, in order:

1. Recover incremental rendering after external-command handoff.
2. Stop dispatching a blocking formatter task for every displayed record.
3. Wake the editor when completions finish; make cancellation effective while work is running.
4. Give completion caches a correct freshness/completeness contract.
5. Remove synchronous history database operations from input/submission handling.
6. Stream pipeline stderr promptly and publish background prompt changes.

This is an audit, not a runtime patch. No test harness, specifications, or runtime source were changed. The concurrent test-infrastructure/POSIX work was left alone.

## Method and scope

- Optimized binary rebuilt with `cargo build --release --bin fsh`, default features. Build completed successfully in 2m12s.
- Binary SHA-256: `cb7a8db711011ea97a258fae049003cdfee6bf429f6520d5aab6742db9ba8e5b`.
- Audit began at `834d09f`; other-agent commits advanced HEAD during the audit. Interactive sources were unchanged during measurement. The binary hash, rather than inherited build-version environment variables, identifies the measured artifact.
- Host: aarch64 Linux, kernel `7.1.13-3-2-ARCH`.
- Real Unix PTY, 100 columns × 24 rows. Real raw/cooked transitions and child terminal handoff: `FSH_TEST_ENV` and `CI` unset.
- Temporary HOME, config directory, history database, and generated fixtures; splash and session logging disabled. Status bar off except the explicitly labelled run.
- Baselines used the existing `scripts/audit-interactive.py`. Additional **temporary, standalone** Python probes reused its PTY session API; they were not integrated into the changing test harness.
- Key latency ends at the last received frame byte. Enter waits for a **new prompt anchor**, not the cursor-show sequence emitted before execution. Quiet intervals delimit output but are excluded from reported latency.
- Completion probes allowed 150 ms of quiet, or collected timestamped chunks for a fixed observation window. A 30 ms quiet gap can mistake the loading placeholder for a completed Tab response.
- Most short-operation distributions have 20 repetitions. Default-prompt comparisons have 10; streaming workloads have 3, delayed producers 5. Small diagnostic experiments are identified below.

These are warm-filesystem, local PTY measurements, not screen-to-photon latency. The host was not CPU-isolated and another agent was active. Treat absolute timings as a local baseline, not a portable SLA. ANSI bytes prove repaint/erase behavior; they do **not** establish visible flicker in every terminal emulator.

## Measured baseline

### Ordinary interaction

Minimal fixed prompt, no status bar; milliseconds:

| Operation | p50 | p95 |
|---|---:|---:|
| Keystrokes while entering `echo audit` | 0.613 | 3.370 |
| Keystrokes while entering `pwd` | 0.506 | 0.833 |
| Enter: `echo audit` | 3.651 | 5.771 |
| Enter: `pwd` | 1.923 | 3.006 |
| Enter: `/bin/true` | 4.585 | 10.419 |
| Enter: `/bin/echo audit` | 4.756 | 8.769 |
| Enter: `echo audit \| /bin/cat` | 4.731 | 11.145 |

Additional isolated-directory run:

| Operation | p50 | p95 |
|---|---:|---:|
| Empty Enter | 0.464 | 2.460 |
| Whitespace-only Enter | 0.710 | 5.498 |
| Enter: builtin `true` | 1.330 | 2.698 |
| Enter: `cd .` | 2.050 | 3.055 |
| Enter: `ls`, 2,504 entries | 8.967 | 14.257 |
| Enter: `ls \| count`, same directory | 18.659 | 23.968 |

Function/alias/control-flow checks, another 20 repetitions per command:

| Operation | p50 | p95 |
|---|---:|---:|
| Function containing `echo audit` | 2.258 ms | 5.898 ms |
| Function containing `true` | 1.413 ms | 4.903 ms |
| Alias expanding to `echo audit` | 1.632 ms | 4.199 ms |
| Builtin `false` | 1.328 ms | 2.692 ms |
| `echo audit \| cat` | 1.598 ms | 4.546 ms |
| `/bin/true \| /bin/true` | 5.202 ms | 8.073 ms |
| `pwd && true` | 1.531 ms | 6.183 ms |
| POSIX `for x in a b; do /bin/true; done` | 6.003 ms | 9.297 ms |

Control input checks (20 repetitions, except bracketed paste: 10):

| Operation | p50 | p95 |
|---|---:|---:|
| Ctrl-C to prompt during builtin `sleep 1` | 5.352 ms | 8.937 ms |
| Ctrl-C to prompt during `/bin/sleep 1` | 1.166 ms | 3.148 ms |
| Ctrl-C to prompt during `sleep 1 \| cat` | 5.213 ms | 10.203 ms |
| Ctrl-C to prompt during `/bin/sleep 1 \| /bin/cat` | 1.508 ms | 3.597 ms |
| Enter on an incomplete quoted command, opening continuation | 0.798 ms | 2.531 ms |
| Bracketed paste of 1,005 characters | 1.155 ms | 3.623 ms |

Interrupt timing starts when Ctrl-C is sent, after the command is already running. The builtin sleep checks interruption at its 10 ms sleep quantum; these results do not indicate a general signal-handling stall.

Warm new-session startup was approximately 5.2 ms median, 9.0 ms maximum across eight launches of the configured probe session. This is not a cold/login-profile startup benchmark.

Idle output was **zero bytes** in the minimal-prompt 1.2 s and 2 s observations and in the default-prompt/no-status-bar 1.2 s observation. The opt-in status clock emitted 97 bytes during its 1.2 s observation, as expected.

**No blanket Enter delay or default idle repaint storm was found.**

### Default prompt comparison

Default prompt, no status bar, tracing off:

| Workload | Key p50 / p95, ms | Enter p50 / p95, ms | Median bytes/key |
|---|---:|---:|---:|
| `echo audit` | 0.589 / 2.279 | 1.915 / 4.596 | 93 |
| `pwd` | 0.571 / 1.236 | 1.780 / 6.712 | 121 |
| `/bin/true` | 0.644 / 2.786 | 4.174 / 8.736 | 558.5 |
| `/bin/echo audit` | 0.622 / 2.164 | 3.884 / 6.038 | 569 |
| `echo audit \| /bin/cat` | 0.654 / 2.299 | 5.887 / 7.766 | 654 |

The increase in bytes after an external command is not ordinary command-length growth: the controlled experiment below isolates terminal-origin state.

## Findings

### 1. P1 — External handoff disables incremental rendering for the following prompt

**Measured:** type the identical `echo abc` after each preceding operation:

| Preceding operation | Median bytes/key | Prompt writes/key | Line erases/key |
|---|---:|---:|---:|
| Empty Enter | 97.5 | 0 | 0 |
| Builtin `echo audit` | 97.5 | 0 | 0 |
| `/bin/true` | 214 | 2 | 2 |
| `/bin/echo audit` | 214 | 2 | 2 |
| `echo audit \| /bin/cat` | 225 | 2 | 2 |

Cursor-only movement after a builtin cost 29 bytes and did not rewrite the prompt. After an external command it cost about 123 bytes and rewrote the prompt once. Backspace also rewrote it twice. Ten repetitions per preceding operation.

**Cause:** `crates/fshell-bridge/src/lib.rs:871` conservatively invalidates cursor state for a child with a controlling terminal. `crates/fshell-repl/src/ftui/mod.rs:3179` consequently retains an unknown origin. In `crates/fshell-terminal/src/backend.rs:395`, range patching requires a known column-zero origin; the fallback redraws the surface. `position_relative_cursor` at line 481 replays text again when the origin is unknown.

Even `/bin/true`, which produces no output, enters this state. The conservative invalidation itself is justified: inherited stderr and `/dev/tty` can affect the cursor. The missing piece is a way to recover efficient rendering afterward.

**Impact:** redundant prompt traffic, selection invalidation, extra work with larger prompts/popups, and flicker risk on terminals without synchronized-output support. An enabled status clock can also exercise the full-repaint path while the user is otherwise idle.

**Recommendation:** establish a deliberate post-child origin contract. Choices include a visible fresh-line boundary, a carefully coordinated cursor-position query, or a terminal model. Do not simply assume a child ended at column zero. Preserve unterminated-output and exact-right-margin behavior when choosing a solution.

### 2. P1 — Displaying builtin output pays one blocking-task round trip per record

**Measured:** generated files containing 80-byte lines, displayed through the same continuously drained PTY. Median Enter-to-next-prompt times, three repetitions:

| Lines | Builtin `cat file` | `/bin/cat file` | `cat file \| /bin/cat` | `cat file \| count` |
|---|---:|---:|---:|---:|
| 1,000 | 45.0 ms | 4.9 ms | 24.3 ms | 5.4 ms |
| 10,000 | 361.6 ms | 21.3 ms | 52.7 ms | 22.9 ms |
| 100,000 | 3,062.6 ms | 95.2 ms | 324.5 ms | 138.8 ms |

The native file reader is not the sole bottleneck: routing the same records through an external terminal sink or counting them is substantially faster than displaying them through the REPL formatter.

**Cause:** `crates/fshell-repl/src/lib.rs:1903` awaits `print_item_streaming_async` for every non-map `Data` payload. `crates/fshell-repl/src/format.rs:744` performs `spawn_blocking(...).await` for **each item**, even a short string that cannot open the pager. The consumer also clones and retains every value in `vals`, including strings already printed. The duplicated display path at `lib.rs:2222` has the same pattern.

**Impact:** a commonly used builtin can be tens of times slower than the external utility; avoidable allocations and retained memory scale with total output. The 100,000-line session's process high-water RSS grew from roughly 25 MiB to roughly 43 MiB; this is corroboration, not an isolated allocator attribution.

**Recommendation:** use one bounded output writer/batching boundary per command, not one blocking task per record. Keep pager-capable complex formatting separate from simple byte/string output. Retain only data that actually requires aggregate presentation, with a bounded/spooled policy for large tables. Fix both display paths or consolidate them.

**Related terminal-UX defect:** the `Bytes` branches at `lib.rs:1916` and `lib.rs:2296` convert bytes to a string and call `println!`. `/bin/printf foo | cat` therefore prints `foo\r\n`, whereas direct `/bin/printf foo` leaves `foo` unterminated. Chunk boundaries should not introduce newlines or lossy UTF-8 conversion; these cause unintended scrolling and corrupt progress-style output.

### 3. P1 — Fast Tab completion has an artificial ~50 ms delivery floor

**Measured:** first Tab to resolved result, including a no-match result:

| Context | p50 | p95 |
|---|---:|---:|
| Empty line | 53.562 ms | 55.525 ms |
| `he` | 54.135 ms | 55.367 ms |
| `echo ./` | 53.747 ms | 56.025 ms |
| `echo ./zzzzzznomatch` | 51.652 ms | 53.869 ms |

The first loading frame arrived in roughly 0.2–2 ms in focused probes, followed by the resolved frame around 51 ms. The no-match case also opens and then removes a loading popup. Empty-line Tab emitted about 6 KiB and 30 erase operations across its two paints in this setup.

**Cause:** `ftui/mod.rs:400` only applies completion results at loop boundaries. The loop then awaits a blocking terminal poll at line 1771. `ftui/prompt.rs:430` selects a 50 ms timeout while completion is busy; the result channel does not wake that poll. The worker skips its 60 ms typing debounce for explicit Tab, but this does not remove the UI delivery delay.

**Recommendation:** multiplex input readiness and completion notifications. Keep terminal-reader ownership explicit: a detached input reader must not steal input during child handoff or cursor queries. Delay the loading placeholder until work actually exceeds a small display budget, so cheap completions do not expand/shrink the prompt twice.

### 4. P1 — In-flight completion cancellation is not actually effective

**Measured:** register a provider that sleeps one second. Start its completion, wait 150 ms, cancel the editor line, and immediately request an ordinary file completion. The unrelated file result arrived **802.7 ms** later with builtin `sleep`, **807.3 ms** later with `/bin/sleep`. The UI remained responsive, but completion stayed on the loading placeholder.

**Cause:** in `ftui/completions.rs:459`, the generation atomic is updated by the worker before computation. The same worker then awaits `task.await` at line 510 and does not observe new requests until the old computation finishes. Cancellation tokens polled by filesystem scans therefore cannot see a newer generation while that computation is running. `clear()` changes the request channel, not that atomic.

Discarding an obsolete response protects correctness, but it does not cancel work or avoid head-of-line blocking.

**Recommendation:** advance cancellation state at request submission/clear, independently of the awaiting worker. Separate provider process lifetime from filesystem-scan cancellation, impose deadlines/output limits, and ensure a hung provider cannot indefinitely block later cheap completions. Aborting a `spawn_blocking` handle alone is insufficient.

Provider reproduction:

```fsh
fn slowchoices() { sleep 1; return ["slowalpha", "slowbeta"] }
complete slowcmd -a "(slowchoices)"
```

Type `slowcmd `, Tab, then quickly Ctrl-C and `echo ./` + Tab.

### 5. P1 — Completion reuse caches filtered/truncated answers as if they were complete source sets

Three independently reproduced problems:

1. With `a-one`, `a-two`, `b-one`, `b-two` in a directory, open completion on `echo ./a-`, then backspace to `echo ./`. Only the `a-` candidates remain; the `b-` files do not reappear.
2. Open `echo ./` completion, create another matching file, wait beyond the 400 ms directory-snapshot TTL, and press Tab. The existing session only cycles its old candidates; the new file is absent.
3. In a directory with 3,000 files, choose a filename beyond the first 2,000 entries returned by `scandir`. Even completing that **exact existing filename** yields no result. The focused probe used `file0499`, independently verified to be outside the captured prefix of directory entries.

**Cause:** `ftui/completions.rs:707` reuses `all_suggestions` whenever its context key matches and the vector is nonempty. But `autocomplete/files.rs:180` already filters by the initial query and truncates results; `directory_snapshot` at line 260 stops enumeration after 2,000 entries **before matching**. The manager's reuse bypasses snapshot TTL/provider refresh entirely. Registry completions similarly filter choices before returning them.

There is also a source-backed pending-state hazard: the context key is updated when a request is submitted, while `all_suggestions` can still belong to the preceding context. Another keystroke in the new context can enter the local-reuse branch, filter the wrong source set, and set `loading = false` while its replacement request is still running. That also removes the completion-busy polling deadline. Candidate-set identity must follow the **published result**, not just the latest requested context.

**Recommendation:** distinguish source snapshots from query results, and distinguish complete from partial snapshots. Refiltering is only valid against a sufficiently complete, fresh source set. Widening a query must requery when that guarantee is absent. Bound expensive ranking/materialization rather than silently skipping arbitrary directory entries. Keep visible old results non-accepting if they belong to an obsolete context.

### 6. P1 — Background completion providers can finish without refreshing the open menu

**Measured with a deterministic fake Carapace executable:** availability probing succeeded; its completion request finished after 100 ms and logged completion. The menu continued showing fallback filesystem candidates. Pressing Tab after the provider had finished only cycled the fallback candidates. Closing and reopening the menu finally displayed `ASYNC_CARAPACE`.

There is an additional cold-start round trip: discovering Carapace availability does not itself schedule the original completion request again.

**Cause:** `autocomplete/carapace.rs:85` fills a cache in a detached thread but does not publish a new candidate generation. The completed manager request is treated as final, and same-context reuse prevents a provider requery. Cached Git tags and external Docker/Kubernetes/Brew loaders have the same general cache-fill-without-result-delivery pattern; those integrations were source-reviewed, not network-benchmarked.

**Recommendation:** providers should publish generation-tagged updates to the owning completion session, including availability-probe completion. A cache write alone is not an observable result. Key provider caches by relevant cwd/repository/environment context and refresh them without silently replacing current input with obsolete candidates.

### 7. P1 at scale — History persistence/search blocks the interactive runtime

**Measured:**

- Holding a SQLite write transaction for 300 ms delayed `echo contention` Enter by **329.5 ms**.
- With 100,000 seeded history rows, six unmatched Ctrl-R search keystrokes took **22.8 ms median**, **39.4 ms maximum**. Opening recent history itself remained cheap in this experiment.

**Cause:** `crates/fshell-repl/src/lib.rs:1683` inserts history synchronously before execution; line 2165 updates it synchronously afterward. The optional duplicate check is another synchronous query. `ftui/history.rs:77` runs `query_history` synchronously for every search edit. The query in `history.rs:375` uses `LIKE '%query%'`; limiting returned rows does not bound scanning work for an absent substring. Pool construction/acquisition can also wait up to five seconds (`history.rs:232`).

Normal-history writes were inexpensive in the traced run: insert p50 0.286 ms and update p50 0.161 ms. This is a **tail-latency and scaling problem**, not evidence of an fsync on every normal key. Connections already use WAL/NORMAL configuration; simply changing those pragmas is not the missing fix.

**Recommendation:** a serialized history writer with immediate in-memory updates; preserve explicit durability barriers for exec/reload and orderly exit. Run cancellable/debounced history search off the editor runtime and use an index suited to substring search, or document/implement prefix search instead. Bound unavailable-storage retries and degrade gracefully rather than imposing repeated foreground waits.

### 8. P1 — Pipeline stderr progress is held until EOF

**Measured:** a producer flushes stdout and stderr immediately, sleeps 350 ms, then writes more stdout:

| Invocation | First stdout, median | First stderr, median |
|---|---:|---:|
| Direct producer | 11.9 ms | 11.9 ms |
| Producer `\| /bin/cat` | 11.5 ms | 363.9 ms |
| Producer `\| cat` | 10.7 ms | 363.9 ms |

Five repetitions each. The producer explicitly flushed; this is not application stdio buffering.

**Cause:** `crates/fshell-bridge/src/lib.rs:1160` reads piped stderr into one string using `take(limit).read_to_string(...)`, then sends a diagnostic only when reading ends. A running build/download/network command can therefore appear silent even though it is producing progress or asking the user something.

**Recommendation:** forward stderr incrementally through its own byte stream/destination. Separate diagnostic rendering from transport of an external command's stderr. Maintain an explicit bounded tail only when a later error summary needs it. Verify that reaching a size limit does not accidentally turn ordinary stderr volume into command failure or a stalled writer.

### 9. P2 — Background prompt work lacks reliable publication; Git worktree freshness is slow

**Measured:**

- A custom prompt segment ran `sleep 0.1; printf ASYNC_READY`. The initial prompt omitted it; **no output arrived during two seconds of idle**. Typing a character immediately revealed `ASYNC_READY`.
- Editing a tracked file externally while a clean-repository prompt was idle took **9.76 seconds** to display `±1` in one controlled run.

**Cause:** custom/host/Kubernetes segments in `prompt.rs` fill caches from background threads without a prompt dirty notification. `PromptManager::update` does not compare those segment outputs. Git watches `.git`, not ordinary worktree edits (`prompt.rs:67`), and relies on a ten-second safety refresh; full Git-status publication at line 281 does not explicitly notify the prompt either. Incidental filesystem events can change the exact observed delay.

**Recommendation:** notify the prompt when a relevant segment's value changes, not merely when its subprocess starts. Make worktree dirtiness and command-end refresh explicit, coalesced events; do not solve freshness by repainting the whole prompt on a fast timer. Skip Git discovery/status entirely when no visible prompt/status consumer needs it.

### 10. P2 — Input bursts and unchanged-text redraws repeat avoidable whole-buffer work

**Measured:** an unbracketed burst of 1,005 characters produced **1,005 synchronized-output frames** and took **157.4 ms** to settle. A 105-character burst produced 105 frames and took 27.5 ms. Bracketed paste is already processed as one paste event; the 1,005-character bracketed-paste probe took **1.155 ms median** across ten repetitions.

A diagnostic 100,000-character single-line buffer took 9.1 ms for insertion, 17.5 ms for cursor-left, and 17.2 ms for backspace. These are single observations, not percentile estimates.

**Cause:** the main loop renders before polling the next buffered event. Every redraw calls `validate_input` (`ftui/mod.rs:434`), which constructs and runs the full parser (`fshell-core/src/validator.rs:154`), even if only the cursor or clock changed. Highlighting, string reconstruction, prompt interpolation and cursor-layout work repeat too. Grapheme navigation in `ftui/buffer.rs:136` reconstructs/scans the full string.

Despite its comment, `highlighter.rs:119` still builds `StyledText` with Nu styles and converts them to Ratatui styles; it is not a direct native-style tokenizer.

**Recommendation:** drain a bounded batch of already-available input before painting, without starving Enter, signals or provider updates. Cache parse/highlight/layout results by buffer/theme/alias revisions; cursor-only and clock-only changes should not reparse the buffer. Maintain incremental text/grapheme indexes where long-buffer measurements justify them. Do not replace authoritative parsing with unsound delimiter heuristics.

### 11. P2 — Resize and Escape latency are explicit policy costs

**Measured:** alternating an idle PTY between 80 and 100 columns took **170.5 ms median**, **171.2 ms p95** to repaint, 20 repetitions with 30 ms quiet spacing. The result reflects the phase of the polling cycle, not a fixed 170 ms sleep. Dismissing a completion popup with a lone Escape took **51.4 ms median**, **53.9 ms maximum** across ten repetitions.

**Cause:** `fshell-tty/src/unix.rs:31` polls window size at a 200 ms quantum. `fshell-tty/src/parse.rs:29` deliberately uses a 50 ms Escape disambiguation timeout. `ftui/mod.rs:1815` also drops resize events within its 80 ms throttle rather than scheduling an explicit trailing resize.

**Recommendation:** deliver SIGWINCH/resize as an actual editor event and retain the last requested size during throttling. Consider configurable/adaptive Escape disambiguation, while preserving fragmented Alt/CSI input over SSH. Keep the Escape decoder cost separate from the completion-result polling cost: they happen to be similar but have different causes.

## Additional source-backed risks, not quantified here

- **Synchronous filesystem work in command highlighting:** `highlighter.rs:82` calls `is_external_command_cached_at`; cache misses can perform PATH metadata probes in the render path (`fshell-engine/src/eval.rs:3830`). The exact-probe tier correctly avoids directory enumeration, but a slow/NFS PATH can still freeze first-use highlighting. Prefer cached/unknown-now classification with asynchronous validation.
- **Unbounded provider subprocess lifetimes:** Carapace, custom prompt segments, and several external completion loaders use blocking `.output()` without deadlines/output limits. `autocomplete/help.rs:516` waits for `--help` to exit before draining stdout, so output exceeding pipe capacity can consume its five-second timeout; the `-h` fallback at line 557 has no analogous timeout. A cache/update flag can remain effectively wedged behind a hung process. The Carapace result map also checks TTL on lookup without an entry-count/eviction bound; expired per-input keys can accumulate over a long session.
- **Opt-in anchored capture is not streaming or memory-bounded:** `ftui/capture.rs:81` reads all output to EOF and joins the reader at completion. Truncating to a bounded tail afterward in `ftui/mod.rs:3116` does not bound peak capture memory or show early output. This is not the default command path.
- **Function-body output bypasses bounded backpressure:** `fshell-engine/src/pipeline.rs:1778` forwards function-body output through an unbounded channel. A fast body feeding the slow per-record terminal formatter can queue substantial output even though other pipeline channels are bounded. Preserve streaming/control-flow semantics when replacing this sink; merely reducing a channel capacity without redesigning producers can introduce deadlocks.
- **Picker startup does eager work:** `picker.rs:705` queries all history despite its “last 500” comment, and collects filesystem/Git data before the picker appears. `get_recursive_files` checks its count cap between directories rather than within a huge directory. Picker filtering is recomputed in both event handling and rendering. Large histories/trees deserve a separate first-paint budget.
- **Clipboard operations are synchronous:** kill/copy/paste widgets call arboard directly (`ftui/clipboard.rs`). Desktop/remote clipboard failure paths need latency measurements; no universal fast-path guarantee follows from local success.

## What is already working well

- Empty Enter bypasses command execution and history insertion; it is sub-millisecond at the median here.
- Known-origin rendering patches dirty ranges, avoids prompt rewrites on ordinary typing, and emits no bytes for no-op frames.
- Frame output is batched and wrapped in DEC synchronized-output sequences (`fshell-terminal/src/backend.rs:906`). Preserve this; extra bytes do not automatically imply visible blank frames on supporting terminals.
- Default idle rendering is quiet. Spinners only request animation work when referenced; the status clock is opt-in.
- Exact external resolution avoids full PATH enumeration; enumeration/prefetch runs in the background and deduplicates alias directories.
- External stdout uses readiness-driven pipes rather than a blocking-pool dispatch for every pipe read. Direct external stdout/stderr reached the PTY promptly.
- Synchronous builtin handlers are dispatched off the current-thread runtime. The especially expensive per-record formatter boundary should not be confused with all builtin execution blocking the runtime.
- Autosuggestions normally query bounded in-memory history, not SQLite per key. History search overlays and persistence are the separate blocking paths identified above.

## Suggested acceptance criteria for the implementation pass

These are proposed behavior/budget checks, not additions to the concurrently changing harness:

1. **Rendering:** type, move the cursor, backspace and tick a status clock after a builtin, silent external, newline-terminated external, unterminated external and exact-margin output. No unnecessary PS1 text replay; no blank intermediate frame; preserve terminal geometry/selection where possible.
2. **Submission:** empty/whitespace Enter must avoid command-history writes, preexec hooks, subprocesses and terminal-query round trips. Configured prompt hooks need their own budget. Report Enter-to-first-output and Enter-to-prompt separately for builtins, functions, aliases, externals and pipelines.
3. **Completion:** cheap warm first-Tab results should no longer cluster at 50 ms; target a single local frame budget. A one-second obsolete provider must not delay an unrelated cached/file completion. Verify widening/backspacing, new files, >2,000-entry directories, and provider publication while the user is idle.
4. **History:** hold a database writer lock and simulate unavailable storage. Editor input must remain responsive; define bounded execution/durability behavior explicitly. Measure search on 100k and 1M rows, including no-match queries.
5. **Output:** compare builtin/external sinks at 1k/10k/100k records, check bounded retained memory, preserve bytes and missing final newlines, and make first stderr arrive while the child is still running.
6. **Input:** one paste/burst should require a bounded number of paints, not one per queued character. Cursor-only motion should not parse unchanged text. Check Unicode/grapheme correctness while optimizing.
7. **Background state:** custom segments/provider results should appear shortly after completion without requiring another key. Idle clocks/spinners remain opt-in and no-op refreshes remain silent.
8. **Terminal matrix:** validate actual visible behavior in supported emulators, an emulator without synchronized output, tmux, and a high-latency/fragmented SSH input path. PTY receipt timings alone do not cover this matrix.

## Reproduction and local artifacts

Untraced baseline:

```sh
cargo build --release --bin fsh
python3 scripts/audit-interactive.py target/release/fsh --samples 20
python3 scripts/audit-interactive.py target/release/fsh --samples 10 --default-prompt
```

Diagnostic tracing, separate from baseline timing:

```sh
python3 scripts/audit-interactive.py target/release/fsh \
  --samples 10 --default-prompt --status-bar --trace /tmp/fsh-perf-trace.jsonl
```

Use a new trace pathname. Trace instrumentation and the status bar add overhead; do not compare that run as if it changed only the prompt.

The standalone probes and raw timestamped results from this audit are retained locally under `/tmp/fshell-perf-audit/`:

- `baseline.json`, `default-untraced.json`, `default-traced.json`, `trace.jsonl`
- `extended.py` / `extended.json`: Enter, controlled post-child input, Tab, Escape, bursts, long input, history contention/search
- `commands.py` / `commands.json`: functions, aliases, logical control flow and native/POSIX pipelines
- `control.py` / `control.json`: running-command interruption, incomplete-input Enter and bracketed paste
- `completion-cases.py` / `completion-cases.json`: cancellation, widening/freshness, directory-cap cases
- `streaming.py` / `streaming-corrected.json`: output throughput, first stdout/stderr, byte-boundary behavior
- `background.py` / `background-corrected.json`: prompt publication, deterministic Carapace publication, resize
- `git-prompt.py` / `git-prompt.json`: external worktree edit freshness

The report uses the **corrected** streaming probe, which accumulates received chunks in a list; repeatedly concatenating a growing byte string in the observer distorted large-output timings. Earlier `streaming.json` and initial availability-only Carapace observations are superseded. These local files are audit artifacts, not a proposed replacement test harness.
