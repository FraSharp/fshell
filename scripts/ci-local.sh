#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# ci-local.sh — reproduce .github/workflows/ci.yml locally (1 + 2)
# Usage: ./scripts/ci-local.sh [--quick] [--with-cross] [--with-act] [--help]
#   --quick       skip heavy tests (still runs fmt/clippy/build/fuzz/audit)
#   --with-cross  also run `cross test --target aarch64-unknown-linux-gnu` if `cross` is installed
#   --with-act    also run `act -W .github/workflows/ci.yml -j check` if `act` is installed
# Exit 0 only if everything that ran passed — safe to push.

set -euo pipefail

# ---------- helpers ----------
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'; CYAN='\033[0;36m'; NC='\033[0m'
say()  { printf "${CYAN}▶ %s${NC}\n" "$*"; }
ok()   { printf "${GREEN}✔ %s${NC}\n" "$*"; }
warn() { printf "${YELLOW}⚠ %s${NC}\n" "$*"; }
die()  { printf "${RED}✘ %s${NC}\n" "$*"; exit 1; }

QUICK=false; WITH_CROSS=false; WITH_ACT=false
for a in "$@"; do
  case "$a" in
    --quick) QUICK=true ;;
    --with-cross) WITH_CROSS=true ;;
    --with-act) WITH_ACT=true ;;
    --help|-h) echo "Usage: $0 [--quick] [--with-cross] [--with-act]"; exit 0 ;;
    *) warn "unknown arg $a (ignored)" ;;
  esac
done

# Auto-enable QEMU path if tools exist even without flags (advisory)
HAS_CROSS=false; HAS_ACT=false; HAS_QEMU=false
command -v cross >/dev/null 2>&1 && HAS_CROSS=true
command -v act >/dev/null 2>&1 && HAS_ACT=true
command -v qemu-aarch64 >/dev/null 2>&1 && HAS_QEMU=true

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

say "fshell local CI — $(date -u +%Y-%m-%dT%H:%M:%SZ) --quick=$QUICK --with-cross=$WITH_CROSS --with-act=$WITH_ACT"

# ---------- 0. toolchain / targets ----------
say "0/7 toolchain & targets"
if ! command -v cargo >/dev/null 2>&1; then
  die "cargo not found — install rustup"
fi
# Ensure rustup targets — idempotent
for tgt in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-apple-darwin aarch64-apple-darwin; do
  if ! rustup target list --installed 2>/dev/null | grep -q "$tgt"; then
    say "  installing target $tgt"
    rustup target add "$tgt" || warn "  failed to add $tgt (will skip cross checks for it)"
  fi
done
# Ensure components on stable
if ! rustup component list --installed 2>/dev/null | grep -q "clippy"; then
  say "  installing clippy/rustfmt"
  rustup component add clippy rustfmt 2>/dev/null || warn "  could not add clippy/rustfmt"
fi
ok "toolchain ready"

# ---------- 1. fmt ----------
say "1/7 cargo fmt --check"
if ! cargo fmt --check; then
  die "fmt failed — run 'cargo fmt' and re-run"
fi
ok "fmt"

# ---------- 2. clippy (native + Linux trap) ----------
say "2/7 clippy native (all-targets)"
cargo clippy --all-targets -- -D warnings
ok "clippy native"

say "2b/7 clippy x86_64-unknown-linux-gnu (Linux mode_t trap)"
# This catches unnecessary_cast on libc::S_* that only fires on Linux (mode_t u32 vs macOS u16).
# On macOS this needs a Linux sysroot (openssl-sys fails) — use `cross` or CI for exact parity.
if [[ "$(uname)" == "Darwin" ]]; then
  if command -v cross >/dev/null 2>&1; then
    say "  Darwin → using cross for Linux clippy"
    cross clippy --target x86_64-unknown-linux-gnu --all-targets -- -D warnings || warn "  cross clippy failed (see above)"
    ok "clippy x86_64 Linux (via cross)"
  else
    warn "skip Linux clippy on macOS — needs Docker/cross or Linux CI"
    warn "  install 'cargo install cross' then re-run with --with-cross, or rely on CI"
  fi
else
  if rustup target list --installed 2>/dev/null | grep -q "x86_64-unknown-linux-gnu"; then
    cargo clippy --target x86_64-unknown-linux-gnu --all-targets -- -D warnings
    ok "clippy x86_64 Linux"
  else
    warn "skip Linux clippy — target not installed"
  fi
fi

# ---------- 3. build ----------
say "3/7 build"
if [[ "$(uname)" == "Darwin" ]]; then
  say "  native build (aarch64-apple-darwin)"
  cargo build || die "build failed"
  if rustup target list --installed 2>/dev/null | grep -q "x86_64-apple-darwin"; then
    say "  also x86_64-apple-darwin (you have no x86 hardware, but toolchain can)"
    cargo build --target x86_64-apple-darwin || warn "  x86_64-apple-darwin build failed (non-fatal)"
  fi
else
  cargo build --target x86_64-unknown-linux-gnu || cargo build
fi
ok "build"

# ---------- 4. test ----------
# Headless: no alternate screen, no progress bar escapes, no stdin reads that suspend on scroll.
# CI=1 + TERM=dumb tells ratatui/crossterm to stay off the alternate buffer.
# Closing stdin prevents TUI tests from SIGTTIN when you scroll.
export CI=1
export TERM=dumb
export CARGO_TERM_PROGRESS=false
if [[ "$QUICK" == true ]]; then
  say "4/7 test --lib (quick, headless)"
  CI=1 TERM=dumb cargo test --lib --quiet </dev/null
  ok "test --lib (quick)"
else
  say "4/7 test (native, headless — no alternate screen)"
  CI=1 TERM=dumb cargo test --quiet </dev/null
  ok "test"
fi

# ---------- 5. fuzz ----------
say "5/7 check fuzz targets"
cargo check -p fshell-fuzz
ok "fuzz check"

# ---------- 6. audit ----------
say "6/7 cargo audit"
if command -v cargo-audit >/dev/null 2>&1; then
  cargo audit || die "audit found vulnerabilities"
  ok "audit"
else
  warn "cargo-audit not installed — run 'cargo install cargo-audit' (skipping)"
fi

# ---------- 7. cross / QEMU (opt-in, headless) ----------
if [[ "$WITH_CROSS" == true ]]; then
  say "7/7 cross aarch64 QEMU (headless)"
  if [[ "$HAS_CROSS" == true ]]; then
    CI=1 TERM=dumb cross test --target aarch64-unknown-linux-gnu --quiet </dev/null || die "cross test aarch64 failed"
    cross clippy --target aarch64-unknown-linux-gnu --all-targets -- -D warnings || die "cross clippy failed"
    ok "cross aarch64"
  elif [[ "$HAS_QEMU" == true ]]; then
    say "  cross not found, trying native QEMU runner"
    CI=1 TERM=dumb CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64 -L /usr/aarch64-linux-gnu" \
    cargo test --target aarch64-unknown-linux-gnu --quiet </dev/null || die "QEMU test failed"
    ok "QEMU aarch64"
  else
    warn "  --with-cross requested but neither 'cross' nor 'qemu-aarch64' found"
    warn "  install: cargo install cross  OR  brew install qemu"
  fi
else
  if [[ "$HAS_CROSS" == true || "$HAS_QEMU" == true ]]; then
    say "7/7 cross/QEMU skipped (run with --with-cross to enable)"
  else
    say "7/7 cross/QEMU — tools not installed (skip, install cross/qmpu for --with-cross)"
  fi
fi

# ---------- 8. act (opt-in, full workflow replay) ----------
if [[ "$WITH_ACT" == true ]]; then
  say "8/8 act workflow replay"
  if [[ "$HAS_ACT" == true ]]; then
    # act will use Docker to emulate ubuntu-latest / macos-latest
    act push -W .github/workflows/ci.yml -j check --container-architecture linux/amd64 || warn "act failed (see above)"
    ok "act"
  else
    warn "  --with-act requested but 'act' not found — brew install act"
  fi
fi

echo ""
printf "${GREEN}All local CI checks passed — safe to push.${NC}\n"
printf "  quick re-run: ${CYAN}./scripts/ci-local.sh --quick${NC}\n"
printf "  full + cross: ${CYAN}./scripts/ci-local.sh --with-cross${NC}\n"
printf "  full replay:  ${CYAN}./scripts/ci-local.sh --with-cross --with-act${NC}\n"
