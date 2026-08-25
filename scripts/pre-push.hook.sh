#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# pre-push hook — never push red. Install with: ./scripts/pre-push.hook.sh --install
# Runs the lightweight gate: fmt --check + clippy -D warnings + test --lib
# For full gate before release, run: ./scripts/ci-local.sh

set -euo pipefail

if [[ "${1:-}" == "--install" ]]; then
  HOOK=".git/hooks/pre-push"
  cp "$0" "$HOOK"
  chmod +x "$HOOK"
  echo "installed pre-push hook to $HOOK"
  exit 0
fi

echo "pre-push: cargo fmt --check"
cargo fmt --check || { echo "fix with: cargo fmt"; exit 1; }

echo "pre-push: cargo clippy -D warnings"
CI=1 TERM=dumb cargo clippy --all-targets -- -D warnings </dev/null || exit 1

# Also catch the Linux unnecessary_cast trap locally (skip on macOS without cross)
if [[ "$(uname)" == "Darwin" ]]; then
  if command -v cross >/dev/null 2>&1; then
    echo "pre-push: clippy x86_64-unknown-linux-gnu (via cross)"
    CI=1 TERM=dumb cross clippy --target x86_64-unknown-linux-gnu --all-targets -- -D warnings </dev/null || echo "pre-push: cross clippy failed (non-blocking, rely on CI)"
  else
    echo "pre-push: skip Linux clippy on macOS (needs cross)"
  fi
elif rustup target list --installed 2>/dev/null | grep -q "x86_64-unknown-linux-gnu"; then
  echo "pre-push: clippy x86_64-unknown-linux-gnu"
  CI=1 TERM=dumb cargo clippy --target x86_64-unknown-linux-gnu --all-targets -- -D warnings </dev/null || exit 1
fi

echo "pre-push: cargo test --lib"
CI=1 TERM=dumb cargo test --lib --quiet </dev/null || exit 1

echo "pre-push: OK — pushing"
