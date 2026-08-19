#!/bin/sh

# Every gate runs, even after one fails. The vendor check used to abort the
# script under `set -e`, so a missing vendored file hid the result of every test
# after it: you learned the tree was incomplete and nothing else.
set -u

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

failed=""

gate() {
  name=$1
  shift
  if "$@"; then
    return 0
  fi
  failed="$failed $name"
}

# `node --test` exits 0 when its path arguments match nothing, so an unexpanded
# glob is a green gate reporting zero tests. Renaming the suffix or moving the
# directory would have silently dropped the whole browser suite from `make
# check`, which is the failure this file was rewritten to prevent.
browser_tests() {
  set -- "$ROOT"/tests/browser/*.test.js
  if [ ! -e "$1" ]; then
    echo "no browser tests matched $ROOT/tests/browser/*.test.js" >&2
    return 1
  fi
  node --test "$@"
}

shell_syntax() {
  status=0
  for script in "$ROOT"/scripts/*.sh; do
    sh -n "$script" || status=1
  done
  return "$status"
}

gate fetch-vendor "$ROOT/scripts/fetch-vendor.sh"
gate verify-vendor "$ROOT/scripts/verify-vendor.sh"
gate fmt cargo fmt --check --manifest-path "$ROOT/Cargo.toml"
gate clippy cargo clippy --all-targets --manifest-path "$ROOT/Cargo.toml" -- -D warnings
gate cargo-test cargo test --manifest-path "$ROOT/Cargo.toml"

gate gen-problems python3 "$ROOT/scripts/gen-problems.py" --check
gate gen-problem-cards python3 "$ROOT/scripts/gen-problem-cards.py" --check
gate wire-fixtures node "$ROOT/scripts/gen-wire-fixtures.mjs" --check
gate todo-deps python3 "$ROOT/scripts/check-todo-deps.py"
gate browser-tests browser_tests

gate browser-check-syntax node --check "$ROOT/scripts/browser-check.cjs"
gate shell-syntax shell_syntax
gate browser-check-env env BROWSER_CHECK_VALIDATE_ENV_ONLY=1 sh "$ROOT/scripts/browser-check.sh"

[ -z "$failed" ] || {
  echo "failed gates:$failed" >&2
  exit 1
}
