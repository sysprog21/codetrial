#!/bin/sh

# Every gate runs, even after one fails. The vendor check used to abort the
# script under `set -e`, so a missing vendored file hid the result of every test
# after it: you learned the tree was incomplete and nothing else.
set -u

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

failed=""

# What this run did not check, collected as it goes and printed at the end.
#
# Every optional lane already said so where it was skipped, several hundred
# lines above the verdict, which is not where anyone looks. A gate that is
# quieter about what it skipped than about what it ran reports a green tree to
# somebody whose checkout cannot run a tenth of it: the browser suite alone
# skips thirty-odd tests without Chromium, and the shell lane covers the scripts
# that delete cloud artifacts.
skipped=""

skip()
{
    echo "skipping $1" >&2
    skipped="$skipped
  $1"
}

gate()
{
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
browser_tests()
{
    set -- "$ROOT"/tests/browser/*.test.js
    if [ ! -e "$1" ]; then
        echo "no browser tests matched $ROOT/tests/browser/*.test.js" >&2
        return 1
    fi

    # Two reporters: `spec` to the terminal for whoever is watching, and `tap`
    # to a file for the summary below. A skip reads as "unverified", not as
    # "fine", and both the reason and the total scroll past long before the
    # verdict, so they are counted and repeated where the reader is looking.
    #
    # The parsing reads TAP rather than the human output for one reason: TAP
    # says which lines are directives. `spec` writes the reason in place of the
    # word, so a skip is `# javac 11 is older...` and cannot be told apart from
    # a passing test titled `works (1ms) # example`, which is a test that ran
    # being reported as one that did not. TAP escapes `#` inside a title and
    # spells the directive `# SKIP` or `# TODO`, so there is nothing to guess.
    #
    # Anchored on the assertion record all the same, because the YAML block
    # under a failure quotes the error verbatim: a test whose message happened
    # to contain the word counted itself as a skip from inside its own
    # diagnostic.
    #
    # No pipeline either, which is why the exit status no longer travels through
    # a second file: `tee` loses it in POSIX sh and asking node for a file does
    # not.
    tap=$(mktemp)
    node --test \
        --test-reporter=spec --test-reporter-destination=stdout \
        --test-reporter=tap --test-reporter-destination="$tap" "$@"
    status=$?

    # A heredoc rather than a pipe, so the loop runs in this shell and what it
    # appends to `skipped` survives it. A todo is counted beside the skips on
    # purpose: it checked nothing either.
    while IFS= read -r reason; do
        [ -n "$reason" ] || continue
        skipped="$skipped
  browser tests, $reason"
    done << REASONS
$(sed -n -e 's/^ *\(not \)\{0,1\}ok [0-9][0-9]* .* # SKIP */skipped: /p' \
        -e 's/^ *\(not \)\{0,1\}ok [0-9][0-9]* .* # TODO */todo: /p' "$tap" \
        | sed 's/: $/: no reason given/' \
        | sort | uniq -c | sed 's/^ *\([0-9]*\) /\1 /')
REASONS

    rm -f "$tap"
    return "$status"
}

# The Rust half of this repo runs clippy at `-D warnings`; this is the other
# half. It needs node_modules, which CI installs before running this script, so
# a checkout that never ran `npm install` says so and moves on rather than
# failing a gate it has no way to run. That is the same contract the browser
# tier above already works under.
eslint_gate()
{
    if [ ! -x "$ROOT/node_modules/.bin/eslint" ]; then
        skip "eslint: \`npm install\` enables it"
        return 0
    fi

    # Run from $ROOT. eslint resolves both eslint.config.mjs and the ignore
    # patterns in it against the working directory, not against the paths it is
    # handed, so pointing it at "$ROOT" from anywhere else fails with "all of
    # the files are ignored". Every other gate here works from any directory and
    # this one has to as well. Subshell, so the cd does not leak into the gates
    # below.
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
actionlint_gate()
{
    if ! command -v actionlint > /dev/null 2>&1; then
        skip "actionlint: installing it checks .github/workflows, which is code too"
        return 0
    fi

    (cd "$ROOT" && actionlint)
}

cargo_audit_gate()
{
    if ! command -v cargo-audit > /dev/null 2>&1; then
        skip "cargo-audit: \`cargo install cargo-audit\` audits the dependency tree"
        return 0
    fi

    cargo audit --file "$ROOT/Cargo.lock"
}

# The Python here is not incidental: it generates the problem bank, checks the
# study plan and drives two integration harnesses. It had unittest suites and no
# linter, which is how an unused import or a name that does not exist survives
# until the script runs. Optional the way shellcheck is, for the same reason.
ruff_gate()
{
    if ! command -v ruff > /dev/null 2>&1; then
        skip "ruff: installing it lints the Python under scripts/ and tests/"
        return 0
    fi

    (cd "$ROOT" && ruff check scripts tests)
}

# `sh -n` says whether a script parses. shellcheck says whether it means what it
# reads like, which matters here because these scripts include the supply-chain
# gate. Repo-wide false positives are answered once in `.shellcheckrc`; the few
# local ones carry a directive naming why, beside the code it is about.
#
# Optional the way cargo-audit is: CI installs it, and a contributor without it
# gets a note rather than a failure they cannot act on.
shell_syntax()
{
    status=0
    for script in "$ROOT"/scripts/*.sh; do
        sh -n "$script" || status=1
    done

    if command -v shellcheck > /dev/null 2>&1; then
        (cd "$ROOT" && shellcheck scripts/*.sh) || status=1
    else
        skip "shellcheck: installing it checks what 21 shell scripts mean, not just that they parse"
    fi

    return "$status"
}

# The mutation gate runs in CI against the diff, so nothing here reproduces it.
# What this checks is the one thing about it that drifts offline: the list of
# functions it has been told not to judge. Each entry is defensible on its own
# and the list only ever grows, so the total is declared in the file header and
# compared here -- an entry added without moving the number fails, which is what
# puts the trend in front of a reviewer instead of in the file's tail.
mutants_exclusions()
{
    config=$ROOT/.cargo/mutants.toml

    # Both reads below go quiet on a file that is not there, and an empty
    # declared count then equals an empty actual one, so the gate agreed with
    # itself about a file it never opened.
    if [ ! -r "$config" ]; then
        echo "$config is missing or unreadable." >&2
        return 1
    fi
    declared=$(sed -n 's/^# EXCLUSIONS: \([0-9][0-9]*\)$/\1/p' "$config")
    actual=$(grep -c '^    "' "$config")
    if [ "$declared" = "$actual" ]; then
        return 0
    fi
    echo "$config declares $declared exclusions and lists $actual." >&2
    echo "Update the EXCLUSIONS line in its header in the same change." >&2
    return 1
}

gate fetch-vendor "$ROOT/scripts/fetch-vendor.sh"
gate verify-vendor "$ROOT/scripts/verify-vendor.sh"
gate mutants-exclusions mutants_exclusions

# `--locked` on the two that resolve dependencies: Cargo.lock is committed, and
# without it a semver-compatible upstream release changes what the gate actually
# built while the lockfile in the repo keeps claiming otherwise. `cargo fmt`
# does not take the flag because it never touches the graph.
gate fmt cargo fmt --check --manifest-path "$ROOT/Cargo.toml"

# Comment reflow and shell formatting, checked against a copy of the tree so a
# gate never rewrites what it is judging. Both tools are optional locally and
# installed in CI, so the check skips a lane rather than failing on a missing
# tool; overlapping with the fmt gate above costs nothing and reads better when
# it is `cargo fmt` alone that drifted.
gate indent "$ROOT/scripts/indent.sh" --check
gate clippy cargo clippy --locked --all-targets --manifest-path "$ROOT/Cargo.toml" -- -D warnings
gate cargo-test cargo test --locked --manifest-path "$ROOT/Cargo.toml"

gate gen-problems python3 "$ROOT/scripts/gen-problems.py" --check

# Each unittest suite runs its cases across a thread pool.
# `scripts/run-python-tests.py` carries the measurement and the isolation a
# suite has to meet before it is added here; the numbers live there rather than
# in a copy that drifts. Still one gate per suite, so a failure names which one.
unittest_gate()
{
    python3 "$ROOT/scripts/run-python-tests.py" "$@"
}

gate gen-problems-tests unittest_gate "$ROOT/tests/test_gen_problems.py"
gate gen-problem-cards python3 "$ROOT/scripts/gen-problem-cards.py" --check
gate wire-fixtures node "$ROOT/scripts/gen-wire-fixtures.mjs" --check
gate recording-fixtures node "$ROOT/scripts/gen-recording-fixtures.mjs" --check
gate recording-provision-check-tests unittest_gate "$ROOT/tests/test_recording_provision_check.py"
gate recording-integration-harness-tests unittest_gate "$ROOT/tests/test_recording_integration_harness.py"
gate study-plan-guards python3 "$ROOT/scripts/check-study-plan-guards.py"
gate calibration-fixtures python3 "$ROOT/scripts/gen-calibration-fixtures.py" --check
gate calibration-tests unittest_gate "$ROOT/tests/test_calibrate_framework.py"
gate browser-tests browser_tests
gate eslint eslint_gate
gate cargo-audit cargo_audit_gate
gate actionlint actionlint_gate

gate browser-check-syntax node --check "$ROOT/scripts/browser-check.cjs"
gate ruff ruff_gate

# The hooks are the one part of this tree that runs on a contributor's machine
# rather than here, so nothing else notices when one stops rejecting. The suite
# builds a scratch repository, installs the hooks into it and drives them.
gate git-hooks "$ROOT/scripts/test-git-hooks.sh"
gate shell-syntax shell_syntax
gate browser-check-env env BROWSER_CHECK_VALIDATE_ENV_ONLY=1 sh "$ROOT/scripts/browser-check.sh"

# Same shape as the line above, and the mode was built for exactly this: the
# full script needs a browser and live credentials, so it stays manual, but the
# fixture it grades against is checked in and can rot on its own. Nothing was
# running this, so a stale tests/golden/report-python.json would only have
# surfaced the next time somebody ran the whole thing by hand.
gate report-parity-fixtures env REPORT_PARITY_CHECK_VALIDATE_FIXTURES_ONLY=1 sh "$ROOT/scripts/report-parity-check.sh"

[ -z "$skipped" ] || {
    echo "" >&2
    echo "not checked by this run:$skipped" >&2
    echo "" >&2
}

[ -z "$failed" ] || {
    echo "failed gates:$failed" >&2
    exit 1
}
