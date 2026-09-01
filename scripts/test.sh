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

# The Rust half of this repo runs clippy at `-D warnings`; this is the other
# half. It needs node_modules, which CI installs before running this script, so
# a checkout that never ran `npm install` says so and moves on rather than
# failing a gate it has no way to run. That is the same contract the browser
# tier above already works under.
eslint_gate() {
  if [ ! -x "$ROOT/node_modules/.bin/eslint" ]; then
    echo "skipping eslint: run \`npm install\` to enable it locally" >&2
    return 0
  fi

  # Run from $ROOT. eslint resolves both eslint.config.mjs and the ignore
  # patterns in it against the working directory, not against the paths it is
  # handed, so pointing it at "$ROOT" from anywhere else fails with "all of the
  # files are ignored". Every other gate here works from any directory and this
  # one has to as well. Subshell, so the cd does not leak into the gates below.
  (cd "$ROOT" && ./node_modules/.bin/eslint .)
}

# The workflow is code and has been wrong in ways review did not catch: a
# one-based shard matrix that silently skipped a quarter of the mutants, a tool
# version written twice that could drift, an expression naming an output that no
# job produced. actionlint reads the YAML, the shell inside a run block, and the
# workflow expressions, so all three are the kind of thing it refuses.
#
# Skipped when absent, like eslint above. It is a single Go binary rather than
# something `npm ci` brings in, and a checkout without it should say so and move
# on rather than fail a gate it cannot run.
actionlint_gate() {
  if ! command -v actionlint >/dev/null 2>&1; then
    echo "skipping actionlint: install it to check .github/workflows locally" >&2
    return 0
  fi

  (cd "$ROOT" && actionlint)
}

cargo_audit_gate() {
  if ! command -v cargo-audit >/dev/null 2>&1; then
    echo "skipping cargo-audit: run \`cargo install cargo-audit\` to enable it locally" >&2
    return 0
  fi

  cargo audit --file "$ROOT/Cargo.lock"
}

# `sh -n` says whether a script parses. shellcheck says whether it means what it
# reads like, which matters here because these scripts include the supply-chain
# gate. Repo-wide false positives are answered once in `.shellcheckrc`; the few
# local ones carry a directive naming why, beside the code it is about.
#
# Optional the way cargo-audit is: CI installs it, and a contributor without it
# gets a note rather than a failure they cannot act on.
shell_syntax() {
  status=0
  for script in "$ROOT"/scripts/*.sh; do
    sh -n "$script" || status=1
  done

  if command -v shellcheck >/dev/null 2>&1; then
    (cd "$ROOT" && shellcheck scripts/*.sh) || status=1
  else
    echo "skipping shellcheck: install it to enable the rest of this gate locally" >&2
  fi

  return "$status"
}

gate fetch-vendor "$ROOT/scripts/fetch-vendor.sh"
gate verify-vendor "$ROOT/scripts/verify-vendor.sh"

# `--locked` on the two that resolve dependencies: Cargo.lock is committed, and
# without it a semver-compatible upstream release changes what the gate actually
# built while the lockfile in the repo keeps claiming otherwise. `cargo fmt`
# does not take the flag because it never touches the graph.
gate fmt cargo fmt --check --manifest-path "$ROOT/Cargo.toml"
gate clippy cargo clippy --locked --all-targets --manifest-path "$ROOT/Cargo.toml" -- -D warnings
gate cargo-test cargo test --locked --manifest-path "$ROOT/Cargo.toml"

gate gen-problems python3 "$ROOT/scripts/gen-problems.py" --check
gate gen-problems-tests python3 -m unittest "$ROOT/tests/test_gen_problems.py"
gate gen-problem-cards python3 "$ROOT/scripts/gen-problem-cards.py" --check
gate wire-fixtures node "$ROOT/scripts/gen-wire-fixtures.mjs" --check
gate recording-fixtures node "$ROOT/scripts/gen-recording-fixtures.mjs" --check
gate recording-provision-check-tests python3 -m unittest "$ROOT/tests/test_recording_provision_check.py"
gate recording-integration-harness-tests python3 -m unittest "$ROOT/tests/test_recording_integration_harness.py"
gate todo-deps python3 "$ROOT/scripts/check-todo-deps.py"
gate study-plan-guards python3 "$ROOT/scripts/check-study-plan-guards.py"
gate calibration-fixtures python3 "$ROOT/scripts/gen-calibration-fixtures.py" --check
gate calibration-tests python3 -m unittest "$ROOT/tests/test_calibrate_framework.py"
gate browser-tests browser_tests
gate eslint eslint_gate
gate cargo-audit cargo_audit_gate
gate actionlint actionlint_gate

gate browser-check-syntax node --check "$ROOT/scripts/browser-check.cjs"
gate shell-syntax shell_syntax
gate browser-check-env env BROWSER_CHECK_VALIDATE_ENV_ONLY=1 sh "$ROOT/scripts/browser-check.sh"

# Same shape as the line above, and the mode was built for exactly this: the
# full script needs a browser and live credentials, so it stays manual, but the
# fixture it grades against is checked in and can rot on its own. Nothing was
# running this, so a stale tests/golden/report-python.json would only have
# surfaced the next time somebody ran the whole thing by hand.
gate report-parity-fixtures env REPORT_PARITY_CHECK_VALIDATE_FIXTURES_ONLY=1 sh "$ROOT/scripts/report-parity-check.sh"

[ -z "$failed" ] || {
  echo "failed gates:$failed" >&2
  exit 1
}
