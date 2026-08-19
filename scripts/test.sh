#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

# Vendored bytes first: cheap, so it fails before the slow gates rather than
# after them.
"$ROOT/scripts/verify-vendor.sh"

cargo fmt --check --manifest-path "$ROOT/Cargo.toml"
cargo clippy --all-targets --manifest-path "$ROOT/Cargo.toml" -- -D warnings
cargo test --manifest-path "$ROOT/Cargo.toml"

# Browser-side checks. Node ships the test runner, so there is no install step.
# The shell expands the pattern, not node: `node --test` only matches globs
# itself on Node 21 and newer, and CI does not pin a node version.
python3 "$ROOT/scripts/gen-problems.py" --check
python3 "$ROOT/scripts/gen-problem-cards.py" --check
node --test "$ROOT"/tests/browser/*.test.js

# The credentialed gates need LiveKit keys, a Gemini key and a Chromium
# download, so they stay out of the default run. Syntax-check what CI cannot
# execute, and prove the shell wrappers still parse their own arguments.
node --check "$ROOT/scripts/browser-check.cjs"
for script in "$ROOT"/scripts/*.sh; do
  sh -n "$script"
done
BROWSER_CHECK_VALIDATE_ENV_ONLY=1 sh "$ROOT/scripts/browser-check.sh"
