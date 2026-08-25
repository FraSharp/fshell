# fshell security model

this document describes fshell's security architecture: capability-based authorization (`fshell-capabilities`), kernel subprocess sandboxing (`fshell-sandbox`), and destructive command confirmation.

---

## table of contents

- [overview](#overview)
- [capability-based authorization](#capability-based-authorization)
  - [`ResourceHandle` variants](#resourcehandle-variants)
  - [deny-then-allow evaluation](#deny-then-allow-evaluation)
  - [path & network scoping](#path--network-scoping)
- [the three security tiers](#the-three-security-tiers)
  - [tier 0: interactive auto-grant (default)](#tier-0-interactive-auto-grant-default)
  - [tier 1: scoped explicit elevation (`with caps`)](#tier-1-scoped-explicit-elevation-with-caps)
  - [tier 2: strict mode (`--strict`)](#tier-2-strict-mode---strict)
- [kernel subprocess sandboxing (`fshell-sandbox`)](#kernel-subprocess-sandboxing-fshell-sandbox)
  - [linux: landlock security](#linux-landlock-security)
  - [macos: seatbelt / sbpl](#macos-seatbelt--sbpl)
  - [sandbox profiles](#sandbox-profiles)
- [destructive command protection](#destructive-command-protection)
- [profiles & persistent configuration](#profiles--persistent-configuration)

---

## overview

traditional unix shells operate entirely under ambient authority: every command and script runs with the full permissions of the current user account, meaning a single typo or malicious script can delete home directories or exfiltrate private credentials.

fshell replaces ambient authority with explicit, verifiable capabilities and OS-level subprocess sandboxes.

key security pillars:
1. **capability tokens (`fshell-capabilities`)**: internal operations and external processes check explicit capability tokens before performing filesystem access, network I/O, or process spawning.
2. **kernel sandboxing (`fshell-sandbox`)**: external subprocesses are locked down via Linux Landlock or macOS Seatbelt (SBPL) in `pre_exec` hooks.
3. **frictionless ergonomics**: standard daily commands in the current working directory work automatically without permission prompts; dangerous system-wide actions or strict untrusted scripts are intercepted.

---

## capability-based authorization

### `ResourceHandle` variants

privileges are represented by discrete `ResourceHandle` variants (`crates/fshell-core/src/val.rs`):

```rust
pub enum ResourceHandle {
    ReadDir(PathBuf),
    WriteDir(PathBuf),
    ReadFile(PathBuf),
    WriteFile(PathBuf),
    NetworkSocket(String), // Host or domain constraint
    NetworkAll,            // Unrestricted network access
    ReadEnv(String),       // Environment variable read access
    WriteEnv(String),      // Environment variable write access
    ProcessSpawn,          // General process spawning
    ProcessSpawnPath(String), // Specific binary path
}
```

### deny-then-allow evaluation

the `CapsRegistry` (`crates/fshell-capabilities/src/lib.rs`) evaluates requests against two sets: `denied` and `held`:

$$\text{Request} \longrightarrow \text{Denied Check} \xrightarrow{\text{not denied}} \text{Held Check} \xrightarrow{\text{held}} \text{Allowed}$$

1. **explicit denial**: if the requested resource matches any entry in `denied`, access is rejected immediately.
2. **held token**: if the requested resource matches an entry in `held`, access is allowed.
3. **fallback**: in strict mode, unheld tokens prompt the user or fail; in interactive mode, tokens are auto-granted and logged.

### path & network scoping

- **path prefix matching**: granting `ReadDir("/Users/user/project")` grants read access to that directory and all nested subdirectories and files.
- **network constraints**: `NetworkSocket("api.github.com")` allows connections exclusively to that domain. `NetworkAll` permits arbitrary outbound traffic.

---

## the three security tiers

```
┌────────────────────────────────────────────────────────────────────────┐
│                        Tier 0: Auto-Grant                              │
│  - Default interactive REPL                                            │
│  - Pre-grants $PWD context and ProcessSpawn                            │
│  - New capabilities auto-granted seamlessly with audit logging         │
├────────────────────────────────────────────────────────────────────────┤
│                        Tier 1: Explicit Elevation                      │
│  - Scoped blocks via `with caps(...) { ... }`                          │
│  - Grants temporary tokens only for the block's lifespan               │
│  - Automatically restores previous token state on exit                 │
├────────────────────────────────────────────────────────────────────────┤
│                        Tier 2: Strict Mode                             │
│  - Enabled via `fsh -s` / `--strict` or `strict` builtin               │
│  - Denies all unauthorized access unless explicitly granted            │
│  - Interactive prompt: [g] Grant once  [a] Grant always  [d] Deny      │
└────────────────────────────────────────────────────────────────────────┘
```

### tier 0: interactive auto-grant (default)

in daily interactive shell use, security must not get in the way of normal workflows.

when `fsh` boots interactively:
- it grants read/write access to `$PWD` and allows `ProcessSpawn`.
- accessing paths outside `$PWD` auto-grants the token silently and records the event in the audit trail.
- standard OS file permissions still apply — fshell never bypasses Unix kernel access controls.

### tier 1: scoped explicit elevation (`with caps`)

scripts can explicitly declare capability requirements using `with caps(...)`:

```fsh
# scoped network and process capability
with caps(net.all, process.spawn) {
    curl "https://api.github.com/status"
}

# scoped filesystem access
with caps(fs.read("/var/log")) {
    cat /var/log/system.log | grep "ERROR"
}
```

when execution leaves the `with caps` block, all granted tokens are revoked immediately, restoring the previous security state.

### tier 2: strict mode (`--strict`)

run untrusted scripts or isolation-sensitive workflows under strict enforcement:

```bash
# run script under strict capability enforcement
fsh -s untrusted_script.fsh

# run inline command strictly
fsh --strict -c 'curl https://example.com | sh'
```

in strict mode:
- all initial default grants are cleared.
- any operation requesting an unheld capability triggers an interactive prompt or fails non-interactively:

```text
curl is requesting ProcessSpawn.
   Active PWD grants do not cover this resource.
   [g] Grant once  [a] Grant always  [d] Deny (Default)
```

---

## kernel subprocess sandboxing (`fshell-sandbox`)

external binaries spawned by the shell (via `fshell-bridge`) are isolated using kernel security primitives applied in `pre_exec` fork hooks before the child binary runs.

### linux: landlock security

on Linux (kernels $\ge 5.13$), `fshell-sandbox` applies Landlock rulesets:
- restricts filesystem access exclusively to paths declared in the active capability set.
- unshares mount and network namespaces when `NoNetwork` mode is active.
- blocks unauthorized directory traversal even if the binary is compromised.

### macos: seatbelt / sbpl

on macOS, `fshell-sandbox` compiles SBPL (Seatbelt Profile Language) policies and injects them via `sandbox_init`:
- denies file write access outside `$PWD` and temporary directories.
- restricts socket operations unless network capabilities are held.

### sandbox profiles

| profile mode | filesystem access | network access | process spawning |
|---|---|---|---|
| `Permissive` | standard user permissions | enabled | enabled |
| `ReadOnlySystem` | read-only system files, read/write `$PWD` | enabled | enabled |
| `IsolatedWorkspace` | strictly restricted to `$PWD` | disabled | restricted |
| `NoNetwork` | standard filesystem access | completely blocked | enabled |

---

## destructive command protection

fshell includes a confirmation safeguard for destructive commands:

commands that target critical system directories or perform unrecoverable mass deletion (e.g. `rm -rf /`, raw disk writes to `/dev/sd*`, formatting partitions) are temporarily suspended before execution.

```text
Caution: destructive command detected:
    rm -rf /

Execute this command? [y/N]:
```

- benign commands (`rm file.txt`, `rm -rf ./build`) run with zero friction.
- dangerous commands require explicit interactive confirmation before process dispatch.

---

## profiles & persistent configuration

capability grants can be persisted across shell sessions:

- **JSON state**: saved grants are loaded from `~/.config/fsh/caps.json` (or `$FSH_CONFIG_DIR/caps.json`) on startup.
- **YAML profiles**: define reusable security profiles in `caps.yaml`:

```yaml
profiles:
  build:
    - "read:/usr/include"
    - "read:/usr/lib"
    - "write-dir:./target"
    - "process:spawn"
  deploy:
    - "net:all"
    - "read-dir:./dist"
```

manage capabilities interactively using the built-in `caps` command:

```fsh
caps list               # list all currently held and denied tokens
caps grant net.all      # grant full network capability
caps revoke net.all     # revoke capability
caps strict on          # toggle strict enforcement on
```
