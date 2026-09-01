# lock acquisition hierarchy & concurrency rules

this document specifies the strict lock acquisition hierarchy for concurrent state in `fshell-engine` and `fshell-core`.

violating this hierarchy leads to runtime panics in debug builds and potential deadlocks in release builds.

---

## table of contents

- [authoritative lock hierarchy](#authoritative-lock-hierarchy)
- [debug-build runtime enforcement](#debug-build-runtime-enforcement)
- [synchronization primitives & guidelines](#synchronization-primitives--guidelines)
  - [fshell_core lock primitives (`fshell_core::lock`)](#fshell_core-lock-primitives-fshell_corelock)
  - [tight scoping & immediate drops](#tight-scoping--immediate-drops)
  - [async safety rules](#async-safety-rules)
- [lock wrapper macros](#lock-wrapper-macros)
- [deadlock prevention checklist](#deadlock-prevention-checklist)

---

## authoritative lock hierarchy

when multiple locks must be held within the same thread of execution, they **must** be acquired strictly from lowest level (1) to highest level (8):

$$\text{caps (1)} \longrightarrow \text{vars (2)} \longrightarrow \text{fns (3)} \longrightarrow \text{jobs (4)} \longrightarrow \text{reactive (5)} \longrightarrow \text{caches (6)} \longrightarrow \text{tracked (7)} \longrightarrow \text{options (8)}$$

| level | lock target | type | description |
|---|---|---|---|
| **1. `Caps`** | `env.caps.caps` | `RwLock<CapsRegistry>` | capability tokens and strict-mode authorization |
| **2. `Vars`** | `env.vars`, `env.local_vars` | `RwLock<FxHashMap<String, Val>>` | global and local variable bindings |
| **3. `Fns`** | `env.fns`, `env.posix_fns` | `RwLock<FxHashMap<String, ...>>` | user-defined and POSIX function definitions |
| **4. `Jobs`** | `env.job_control.jobs` | `RwLock<FxHashMap<i32, Job>>` | background task table and process monitors |
| **5. `Reactive`** | `env.reactive.cells` | `RwLock<FxHashMap<String, ...>>` | reactive watch channels and dependency DAG |
| **6. `Caches`** | `PATH_CACHE`, `SUGGESTION_CACHE`, etc. | `Mutex<Option<...>>` | bounded execution and path lookup caches |
| **7. `Tracked`** | `env.reactive.tracked_cells` | `RwLock<FxHashSet<String>>` | dynamic variable access tracking registers |
| **8. `Options`** | `env.options` | `RwLock<ShellOptions>` | shell configuration options (read-mostly) |

---

## debug-build runtime enforcement

in debug builds (`cfg(debug_assertions)`), `fshell-engine` tracks lock acquisition order per-thread using a thread-local call stack (`crates/fshell-engine/src/lib.rs`):

```rust
#[track_caller]
pub(crate) fn check_lock_order(level: LockLevel) {
    let loc = std::panic::Location::caller();
    LOCK_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if let Some(&last) = stack.last() {
            assert!(
                level >= last,
                "Lock ordering violation at {loc}:\n  \
                 trying to acquire '{name}' (level {level:?}) after '{last_name}' (level {last:?}).\n  \
                 Order must be: \
                 caps(1) < vars(2) < fns(3) < jobs(4) < reactive(5) < caches(6) < tracked(7) < options(8)",
                name = level.name(),
                last_name = last.name(),
            );
        }
        stack.push(level);
    });
}
```

if a thread attempts to acquire a higher-priority lock (e.g. `vars`) while already holding a lower-priority lock (e.g. `options`), the engine panics with the exact caller location in the backtrace.

---

## synchronization primitives & guidelines

### fshell_core lock primitives (`fshell_core::lock`)

all synchronous mutual exclusion and read-write locks across all crates use `parking_lot` re-exported through `fshell_core::lock::{Mutex, RwLock, Condvar, Once}`.

- `.lock()`, `.read()`, and `.write()` return guards directly.
- **never** append `.unwrap()` to lock acquisitions (unlike `std::sync::Mutex`, parking_lot does not poison locks on panic).
- 1-byte overhead per mutex, no allocation, futex-backed fast path on Linux and macOS.

```rust
// correct:
let vars = env.vars.read();
let mut cache = PATH_CACHE.lock();

// incorrect:
let vars = env.vars.read().unwrap();
let mut cache = PATH_CACHE.lock().unwrap();
```

### tight scoping & immediate drops

never keep locks open longer than necessary. extract the required value, clone or copy it, and drop the guard immediately:

```rust
// extract value in a localized block so the lock guard is dropped immediately
let current_path = {
    let vars = lock_vars!(env.vars.read());
    vars.get("PATH").cloned()
}; // lock is released here
```

### async safety rules

1. **never hold a lock across an `.await` yield point**: holding a synchronous `RwLockGuard` or `MutexGuard` across an async boundary will block the tokio worker thread and cause deadlocks when other tasks on the same worker attempt to acquire the lock.
2. **never hold a lock across subprocess spawning**: release all locks before calling `tokio::process::Command::spawn()` or blocking child process wait loops.

---

## lock wrapper macros

always use the engine's lock wrapper macros to ensure debug assertions and order tracking are applied:

```rust
// acquiring capability lock:
let caps = lock_caps!(env.caps.caps.read());

// acquiring variables lock:
let mut vars = lock_vars!(env.vars.write());

// acquiring function registry lock:
let fns = lock_fns!(env.fns.read());

// acquiring job control lock:
let mut jobs = lock_jobs!(env.job_control.jobs.write());

// acquiring reactive cell lock:
let cells = lock_reactive!(env.reactive.cells.read());

// acquiring cache lock:
let mut cache = lock_caches!(PATH_CACHE.lock());
```

---

## deadlock prevention checklist

- [ ] locks acquired strictly in increasing order: `caps` $\to$ `vars` $\to$ `fns` $\to$ `jobs` $\to$ `reactive` $\to$ `caches` $\to$ `tracked` $\to$ `options`.
- [ ] no locks held across any `.await` expression.
- [ ] no locks held across `std::process::Command` or `tokio::process::Command` calls.
- [ ] guards dropped immediately after reading or mutating data.
- [ ] no `.unwrap()` on `parking_lot` lock guards.
