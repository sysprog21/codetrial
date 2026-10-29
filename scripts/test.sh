#!/bin/sh

# Every gate runs, even after one fails. The vendor check used to abort the
# script under `set -e`, so a missing vendored file hid the result of every test
# after it: you learned the tree was incomplete and nothing else.
set -u

# One gate list for local runs and CI's parallel lanes. A gate names its lane
# where it is declared; one that names none is portable, so a new gate cannot
# escape CI.
lane=${1:-all}
case "$lane" in
    all | rust | portable | browser) ;;
    *)
        echo "usage: ${0##*/} [all | rust | portable | browser]" >&2
        exit 2
        ;;
esac

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

# The gate needs Python 3.11 for the Rust checker's standard-library TOML
# parser. A distribution's python3 can be older than that while a newer one sits
# beside it, and running the old one fails every Python gate on the interpreter
# rather than on the code, so the first suitable one is taken: python3 itself,
# then the newest versioned name. PYTHON names one outright.
python_ok()
{
    "$1" -c 'import sys; sys.exit(sys.version_info < (3, 11))' > /dev/null 2>&1
}

has_cryptography()
{
    "$1" -c 'import cryptography' > /dev/null 2>&1
}

# The instructor tool's own environment comes first when it holds the
# `cryptography` the packaging tests need. Without it, it is set up from the
# Python found, once, here: the packaging suites run in parallel and must not
# each set it up. Only the portable lane runs them, so no other lane installs
# it. An environment CODETRIAL_TASK_TOOLS names, as CI's does, is set up even
# when the Python found has the package: a runner's system Python can, and the
# tests that need a venv would skip under it. A PYTHON named outright is used as
# it is.
failed=""

# The Rust lane audits workspace membership with Python too. Only the browser
# lane runs no Python gates and can use a machine with an older interpreter.
lane_runs_python=true
if [ "$lane" = browser ]; then
    lane_runs_python=false
fi
TASK_PYTHON=${CODETRIAL_TASK_TOOLS:-$ROOT/target/task-tools}/bin/python
if [ -z "${PYTHON:-}" ] && python_ok "$TASK_PYTHON" \
    && has_cryptography "$TASK_PYTHON"; then
    PYTHON=$TASK_PYTHON
fi
if [ -z "${PYTHON:-}" ]; then
    for candidate in python3 python3.15 python3.14 python3.13 python3.12 python3.11; do
        if python_ok "$candidate"; then
            PYTHON=$candidate
            break
        fi
    done
    if [ -n "${PYTHON:-}" ] && [ "$lane" != rust ] && [ "$lane" != browser ]; then
        if [ -n "${CODETRIAL_TASK_TOOLS:-}" ] || ! has_cryptography "$PYTHON"; then
            if "$PYTHON" "$ROOT/scripts/problem_bank/runtime.py"; then
                PYTHON=$TASK_PYTHON
            else
                failed=" task-tools"
            fi
        fi
    fi
fi

PYTHON=${PYTHON:-}
if $lane_runs_python && {
    [ -z "$PYTHON" ] || ! python_ok "$PYTHON"
}; then
    echo "the test gate needs Python 3.11+ (tomllib); set PYTHON to a supported interpreter." >&2
    exit 2
fi

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

# Run a command that reports its own skips on stderr, and add each one the sed
# script $2 extracts to this run's summary under label $1. Its stderr is shown
# once the command finishes.
with_reported_skips()
{
    label=$1
    pattern=$2
    shift 2
    log=$(mktemp)
    "$@" 2> "$log"
    status=$?
    cat "$log" >&2
    while IFS= read -r reason; do
        [ -n "$reason" ] || continue
        skipped="$skipped
  $label, $reason"
    done << REASONS
$(sed -n "$pattern" "$log")
REASONS
    rm -f "$log"
    return "$status"
}

# Whether this run includes lane $1. `any` is a gate every lane needs.
in_lane()
{
    [ "$1" = any ] || [ "$lane" = all ] || [ "$lane" = "$1" ]
}

# gate [--lane LANE] NAME COMMAND...
gate()
{
    want=portable
    if [ "$1" = --lane ]; then
        want=$2
        shift 2
    fi
    name=$1
    shift
    in_lane "$want" || return 0
    if "$@"; then
        return 0
    fi
    failed="$failed $name"
}

# `node --test` exits 0 when its path arguments match nothing, so an unexpanded
# glob is a green gate reporting zero tests. Renaming the suffix or moving the
# directory would have silently dropped the whole browser suite from `make
# check`, which is the failure this file was rewritten to prevent.
#
# $1 picks a tier. `server` is the suite that starts the Rust server, so it runs
# after the build; `static` is every other suite.
browser_tests()
{
    tier=$1
    shift
    server_suite=$ROOT/tests/browser/face-detector.test.js
    for test_file in "$ROOT"/tests/browser/*.test.js; do
        [ -e "$test_file" ] || continue
        if [ "$test_file" = "$server_suite" ]; then
            [ "$tier" = server ] && set -- "$@" "$test_file"
        else
            [ "$tier" = static ] && set -- "$@" "$test_file"
        fi
    done
    if [ "$#" -eq 0 ]; then
        echo "no $tier browser tests matched $ROOT/tests/browser/*.test.js" >&2
        return 1
    fi

    # BROWSER_SHARD=i/n runs one slice of the static suites, so CI can spread
    # the lane over runners. The slices partition the files: each goes to the
    # slice with the least source so far, largest first, which keeps the slow
    # editor suites apart without a hand-kept list. An unset variable runs them
    # all, which is what a local run wants.
    if [ "$tier" = static ] && [ -n "${BROWSER_SHARD:-}" ]; then
        case $BROWSER_SHARD in
            [1-9]/[1-9]) ;;
            *)
                echo "BROWSER_SHARD must look like 1/2, not $BROWSER_SHARD" >&2
                return 1
                ;;
        esac
        if [ "${BROWSER_SHARD%/*}" -gt "${BROWSER_SHARD#*/}" ]; then
            echo "BROWSER_SHARD $BROWSER_SHARD names a slice past the last" >&2
            return 1
        fi
        slice=$(wc -c "$@" | sort -k1,1nr -k2 | awk \
            -v index_="${BROWSER_SHARD%/*}" -v total="${BROWSER_SHARD#*/}" '
            $2 == "total" { next }
            {
                pick = 0
                for (k = 1; k < total; k++) if (load[k] < load[pick]) pick = k
                load[pick] += $1
                if (pick == index_ - 1) { sub(/^ *[0-9]+ /, ""); print }
            }')
        set --
        while IFS= read -r test_file; do
            [ -n "$test_file" ] && set -- "$@" "$test_file"
        done << SLICE
$slice
SLICE
        if [ "$#" -eq 0 ]; then
            echo "browser shard $BROWSER_SHARD is empty" >&2
            return 1
        fi
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

    # Node applies this limit to the file as well as each test. The editor
    # history suite legitimately exceeds five minutes on a busy runner. Keep the
    # file budget below the job ceiling and bound concurrent files so each
    # Chromium process gets CPU instead of slowing every suite at once.
    if [ "${CI:-}" = true ] || node --test-timeout=600000 --version > /dev/null 2>&1; then
        set -- --test-timeout=600000 "$@"
    fi
    # Older Node rejects the option outright, so probe it as the timeout is.
    if node --test-concurrency=2 --version > /dev/null 2>&1; then
        set -- --test-concurrency=2 "$@"
    fi
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
# The image when the binary is absent, because a skip here leaves the workflow
# read by nothing and actionlint publishes itself as a container. The version
# comes out of the workflow rather than being written here too: a tool version
# in two places that can drift is one of the things this gate exists to catch.
#
# `docker info` and not just the client: a host with docker installed and no
# daemon reachable would otherwise turn an optional lane into a failing one.
# Both absent is still a skip, on the same contract as eslint above.
actionlint_gate()
{
    if command -v actionlint > /dev/null 2>&1; then
        (cd "$ROOT" && actionlint)
        return "$?"
    fi

    version=$(sed -n 's/^ *ACTIONLINT_VERSION: *//p' \
        "$ROOT/.github/workflows/check.yml")
    if [ -n "$version" ] && command -v docker > /dev/null 2>&1 \
        && docker info > /dev/null 2>&1; then
        docker run --rm --volume "$ROOT:/repo" --workdir /repo \
            "rhysd/actionlint:$version"
        return "$?"
    fi

    skip "actionlint: installing it, or docker, checks .github/workflows"
    return 0
}

release_msrv_matches()
{
    manifest=$(sed -n 's/^rust-version *= *"\([0-9][0-9.]*\)"$/\1/p' \
        "$ROOT/Cargo.toml")
    image=$(sed -n 's/^[[:space:]]*image: rust:\([0-9][0-9.]*\)-bullseye$/\1/p' \
        "$ROOT/.github/workflows/check.yml")
    if [ -n "$manifest" ] && [ -n "$image" ] \
        && [ "$(printf '%s' "$manifest" | cut -d. -f1-2)" = \
            "$(printf '%s' "$image" | cut -d. -f1-2)" ]; then
        return 0
    fi
    echo "release Rust image $image disagrees with rust-version $manifest" >&2
    return 1
}

cargo_audit_gate()
{
    if ! command -v cargo-audit > /dev/null 2>&1; then
        skip "cargo-audit: \`cargo install cargo-audit\` audits the dependency tree"
        return 0
    fi

    # The binary rather than `cargo audit`; scripts/audit.py says why.
    cargo-audit audit --file "$ROOT/Cargo.lock" \
        && cargo-audit audit --file "$ROOT/desktop/Cargo.lock"
}

cargo_deny_gate()
{
    if ! command -v cargo-deny > /dev/null 2>&1; then
        skip "cargo-deny: \`cargo install cargo-deny\` checks dependency licenses and sources"
        return 0
    fi

    cargo deny --manifest-path "$ROOT/Cargo.toml" check licenses sources
}

# The desktop app is a crate of its own with its own lockfile, so none of the
# root's fmt, clippy or test gates reach it. Building it needs the platform's
# webview, which on Linux is WebKitGTK's development package: a machine without
# one skips the build and says so, the way a missing linter does. CI installs it
# on every run of the Rust lane, since a skip there fails the run.
desktop_gate()
{
    manifest=$ROOT/desktop/Cargo.toml
    if [ "$(uname -s)" = Linux ] \
        && ! pkg-config --exists webkit2gtk-4.1 > /dev/null 2>&1; then
        skip "desktop clippy and tests: libwebkit2gtk-4.1-dev builds desktop/"
        return 0
    fi
    target=$ROOT/target/desktop
    cargo clippy --locked --workspace --all-targets --manifest-path "$manifest" \
        --target-dir "$target" -- -D warnings \
        && cargo test --locked --workspace --manifest-path "$manifest" --target-dir "$target"
}

# The crates under vendor/ carry fixes upstream has not released. A bump that
# outruns a vendored copy brings the registry crate back beside it, or in its
# place with only a Cargo warning, so no vendored name may resolve from a
# registry.
vendor_patches_gate()
{
    # One pass, the manifests before the lockfile: awk -v cannot carry a
    # multi-line list portably. The [package] name only: [lib], [[test]] and
    # [[example]] have names too. Deduplicated in awk rather than by a pipe, so
    # an unreadable input fails the gate instead of passing it empty.
    unpatched=$(awk -v lock="$ROOT/Cargo.lock" '
        FILENAME != lock && /^\[/ { package = ($0 == "[package]") }
        FILENAME != lock && package && /^name = / { gsub(/"/, "", $3); vendored[$3] = 1 }
        FILENAME == lock && /^name = / { name = $3; gsub(/"/, "", name) }
        FILENAME == lock && /^source = "registry\+/ && (name in vendored) && !seen[name]++ { print name }
    ' "$ROOT"/vendor/*/Cargo.toml "$ROOT/Cargo.lock") || return 1
    if [ -n "$unpatched" ]; then
        echo "vendored crates also resolve from a registry; re-vendor them from" >&2
        echo "the version Cargo.lock now wants (see vendor/*/README.md):" >&2
        echo "$unpatched" >&2
        return 1
    fi
}

# The Python here is not incidental: it generates the problem bank, checks the
# study plan and drives two integration harnesses. It had unittest suites and no
# linter, which is how an unused import or a name that does not exist survives
# until the script runs. Optional the way shellcheck is, for the same reason.
ruff_gate()
{
    if ! command -v ruff > /dev/null 2>&1; then
        skip "ruff: installing it lints the Python under scripts/, tests/ and the skills"
        return 0
    fi

    (cd "$ROOT" && ruff check scripts tests .claude/skills)
}

# `sh -n` says whether a script parses, in whatever `sh` is: dash on Debian and
# Ubuntu, but bash in POSIX mode on macOS, which accepts an array, a here-string
# and the `function` keyword that dash refuses. So dash is asked first where it
# is installed. Neither parser objects to `[[`, `local` or `echo -e`; shellcheck
# does, and it says whether a script means what it reads like, which matters
# here because these scripts include the supply-chain gate, and because the
# fakes under tests/stubs/ stand in for gh, gcloud and curl, where a fake that
# means something other than it reads passes a test for the wrong reason.
# Repo-wide false positives are answered once in `.shellcheckrc`; the few local
# ones carry a directive naming why, beside the code it is about.
#
# Optional: CI installs it, and a contributor without it gets a note rather than
# a failure they cannot act on. That note counts the scripts off the same glob
# the loop walks, because the number was typed into the sentence once and adding
# a script left it describing a tree that no longer existed.
shell_syntax()
{
    status=0
    scripts=0
    posix_sh="sh"
    if command -v dash > /dev/null 2>&1; then
        posix_sh=dash
    fi
    for script in "$ROOT"/scripts/*.sh "$ROOT"/.claude/skills/*/*.sh "$ROOT"/tests/stubs/*/*.sh; do
        scripts=$((scripts + 1))
        "$posix_sh" -n "$script" || status=1
    done

    if command -v shellcheck > /dev/null 2>&1; then
        (cd "$ROOT" && shellcheck scripts/*.sh .claude/skills/*/*.sh tests/stubs/*/*.sh) || status=1
    else
        skip "shellcheck: installing it checks what $scripts shell scripts mean, not just that they parse"
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

gate --lane any fetch-vendor "$ROOT/scripts/fetch-vendor.sh"
gate --lane any verify-vendor "$ROOT/scripts/verify-vendor.sh"
gate mutants-exclusions mutants_exclusions

# `--locked` on the two that resolve dependencies: Cargo.lock is committed, and
# without it a semver-compatible upstream release changes what the gate actually
# built while the lockfile in the repo keeps claiming otherwise. `cargo fmt`
# does not take the flag because it never touches the graph.
gate fmt cargo fmt --check --manifest-path "$ROOT/Cargo.toml"

# Comment reflow and shell formatting, checked against a copy of the tree so a
# gate never rewrites what it is judging. This is the one caller that lets a
# missing formatter skip its lane rather than fail, and the skip joins this
# run's summary, which CI fails on; overlapping with the fmt gate above costs
# nothing and reads better when it is `cargo fmt` alone that drifted.
#
# Dependabot bumps Prettier with nobody at a keyboard to run `make indent`, so
# when it wrote every commit in INDENT_ADVISORY_RANGE (CI's base..head of the
# pull request) the drift is printed rather than failed, for whoever merges the
# pull request to fix. A commit of a person's on the same branch, a merge from
# the "Update branch" button included, is held to it again.
indent_gate()
{
    with_reported_skips formatting \
        's/^indent\.sh: \(.* not installed\), skipping that lane.*/\1/p' \
        "$ROOT/scripts/indent.sh" --check --skip-missing
    status=$?
    if [ "$status" -eq 1 ] && [ -n "${INDENT_ADVISORY_RANGE:-}" ] \
        && (cd "$ROOT" && git rev-list "$INDENT_ADVISORY_RANGE" \
            | ./scripts/check-commit-log.sh --bot-only); then
        echo "Formatting drift reported, not failed: Dependabot wrote every commit." >&2
        return 0
    fi
    return "$status"
}
gate indent indent_gate

# CONTRIBUTING.md keeps test bodies out of src/: a module declares its tests
# with #[path] and the body lives under tests/unit/. A test attribute, or a
# cfg(test) module with its body inline, compound cfgs and an attribute on the
# same line included, is the mechanical half of that rule. A negated cfg is
# never judged a test, which misses `all(test, not(x))` rather than refuse a
# production module behind `not(any(test, x))`. cfg(test) items that are not
# modules stay allowed, since a test-only recorder or constructor is
# implementation the tests reach through `super::`.
src_test_bodies()
{
    (cd "$ROOT" && find src -name '*.rs' -exec awk '
        function flag() { print FILENAME ":" FNR ": " $0; bad = 1 }
        FNR == 1 { cfg = 0 }
        /^[[:space:]]*#\[([a-z_]+::)*test[\](]/ { flag(); next }
        /^[[:space:]]*#\[/ && /#\[cfg\(([^)]*[(, ])?test[,)]/ && !/not\(/ { cfg = 1 }
        /^[[:space:]]*(#\[.*\][[:space:]]*)?(pub(\([a-z]+\))? )?mod [A-Za-z_0-9]+ *\{/ {
            if (cfg) flag()
            cfg = 0
            next
        }
        /^[[:space:]]*(#\[|\/\/)/ { next }
        { cfg = 0 }
        END { exit bad }
    ' {} +) && return 0
    echo "Move these test bodies under tests/unit/; CONTRIBUTING.md shows the #[path] form." >&2
    return 1
}
gate src-test-bodies src_test_bodies

# What clippy has no lint for: the lint table staying strict, suppressions that
# say why, no markers or CJK, and no file growing past its ceiling.
gate rust-style "$PYTHON" "$ROOT/scripts/check-rust-style.py" --baseline origin/main

# webrtc-sys needs Clang 21+ on Linux, and without one clippy and cargo-test
# fail on a build script rather than on anything this tree did. The Makefile's
# default, for a run that did not come through `make check`: a caller's own CXX
# wins, and the pinned one is a no-op once it is on disk. Here rather than at
# the top, so the gates above it report before a first run's download, and a
# fetch that fails leaves CXX unset for the build to say why.
if in_lane rust && [ -z "${CXX:-}" ] && [ "$(uname -sm)" = "Linux x86_64" ] \
    && sh "$ROOT/scripts/fetch-clang.sh" "$ROOT/target/clang" > /dev/null; then
    CXX=$ROOT/target/clang/bin/clang++
    export CXX
fi
gate --lane rust clippy cargo clippy --locked --workspace --all-targets --manifest-path "$ROOT/Cargo.toml" -- -D warnings
gate --lane rust workspace-lints "$PYTHON" "$ROOT/scripts/check-rust-style.py" --workspace
gate --lane rust rustdoc cargo doc --locked --workspace --no-deps --manifest-path "$ROOT/Cargo.toml"
gate release-msrv release_msrv_matches
gate --lane rust cargo-test cargo test --locked --workspace --manifest-path "$ROOT/Cargo.toml"

# These tests start the Rust server; running them before its build silently
# skips.
gate --lane rust browser-server-tests browser_tests server
gate desktop-fmt cargo fmt --check --manifest-path "$ROOT/desktop/Cargo.toml"
gate --lane rust desktop desktop_gate

gate gen-problems "$PYTHON" "$ROOT/scripts/gen-problems.py" --check

# Each unittest suite runs its cases across a thread pool.
# `scripts/run-python-tests.py` states the isolation a suite has to meet before
# it is added here. Still one gate per suite, so a failure names which one.
unittest_gate()
{
    with_reported_skips "python tests" 's/^SKIP: //p' \
        "$PYTHON" "$ROOT/scripts/run-python-tests.py" "$@"
}

gate gen-problems-tests unittest_gate "$ROOT/tests/test_gen_problems.py"
gate task-mode-tests unittest_gate "$ROOT/tests/test_task_mode.py"

if in_lane portable && ! has_cryptography "$PYTHON"; then
    skip "instructor packaging tests: $PYTHON has no \`cryptography\`; see docs/development.md"
else
    gate task-package-tests unittest_gate "$ROOT/tests/test_task_package.py"
    gate task-authoring-tests unittest_gate "$ROOT/tests/test_task_authoring.py"
fi
gate gen-problem-cards "$PYTHON" "$ROOT/scripts/gen-problem-cards.py" --check
gate wire-fixtures node "$ROOT/scripts/gen-wire-fixtures.mjs" --check
gate recording-fixtures node "$ROOT/scripts/gen-recording-fixtures.mjs" --check
gate recording-provision-check-tests unittest_gate "$ROOT/tests/test_recording_provision_check.py"
gate recording-integration-harness-tests unittest_gate "$ROOT/tests/test_recording_integration_harness.py"
gate study-plan-guards "$PYTHON" "$ROOT/scripts/check-study-plan-guards.py"
gate calibration-fixtures "$PYTHON" "$ROOT/scripts/gen-calibration-fixtures.py" --check
gate calibration-tests unittest_gate "$ROOT/tests/test_calibrate_framework.py"
gate gemini-usage-tests unittest_gate "$ROOT/tests/test_analyze_gemini_usage.py"
gate audit-tests unittest_gate "$ROOT/tests/test_audit.py"
gate rust-style-tests unittest_gate "$ROOT/tests/test_check_rust_style.py"
gate test-gate-tests unittest_gate "$ROOT/tests/test_test_gate.py"
gate mutants-plan-tests unittest_gate "$ROOT/tests/test_mutants_plan.py"
gate mutants-install-tests unittest_gate "$ROOT/tests/test_mutants_install.py"
gate latency-tests unittest_gate "$ROOT/tests/test_analyze_latency.py"
gate audio-latency-tests unittest_gate "$ROOT/tests/test_audio_latency_report.py"
gate interview-behavior-script-tests unittest_gate "$ROOT/tests/test_interview_behavior_script.py"
gate wait-for-server-tests unittest_gate "$ROOT/tests/test_wait_for_server.py"

# These cases enforce real one-second permit and process cleanup deadlines.
# Thirty-two concurrent launchers starve their own guards on a loaded host.
gate run-budget-tests unittest_gate --jobs 4 "$ROOT/tests/test_interview_run_budget.py"
gate recovery-matrix-tests unittest_gate "$ROOT/tests/test_check_recovery_matrix.py"
gate gap-state-table "$PYTHON" "$ROOT/scripts/check-gap-state.py" check
gate gap-state-tests unittest_gate "$ROOT/tests/test_check_gap_state.py"

# The skills name scripts, lanes, make targets and each other's headings, and an
# agent follows a dangling one without noticing. Their own scripts are tested
# like any other, against a fake gh and a scratch repository.
gate skill-refs "$PYTHON" "$ROOT/scripts/check-skill-refs.py"
gate skill-scripts-tests unittest_gate "$ROOT/tests/test_skill_scripts.py"
gate review-memory-tests unittest_gate "$ROOT/tests/test_review_memory.py"
gate review-memory-publisher-tests unittest_gate "$ROOT/tests/test_review_memory_publish.py"
gate review-memory-benchmark-tests unittest_gate "$ROOT/tests/test_review_memory_benchmark.py"
gate review-memory-evaluation-tests unittest_gate "$ROOT/tests/test_review_memory_evaluation.py"
gate review-memory-retrieval-tests unittest_gate "$ROOT/tests/test_review_memory_retrieval.py"
gate review-memory-experience-tests unittest_gate "$ROOT/tests/test_review_memory_experience.py"
# Lifecycle cases patch process globals; they cannot share the runner threads.
gate review-lifecycle-tests unittest_gate --jobs 1 "$ROOT/tests/test_review_lifecycle.py"
gate --lane browser browser-tests browser_tests static
gate test-timing node "$ROOT/scripts/check-test-timing.mjs"
gate test-timing-tests unittest_gate "$ROOT/tests/test_check_test_timing.py"
gate browser-stress-tests unittest_gate "$ROOT/tests/test_browser_stress.py"
gate eslint eslint_gate
gate --lane all cargo-audit cargo_audit_gate

# The rust lane, because resolving the graph fetches every crate in Cargo.lock
# and that lane is the one that restores them from the cargo cache.
gate --lane rust cargo-deny cargo_deny_gate
gate vendor-patches vendor_patches_gate
gate actionlint actionlint_gate

gate browser-check-syntax node --check "$ROOT/scripts/browser-check.cjs"

# `node --check` parses; it does not resolve. `scripts/browser-check.cjs` spent
# months requiring `livekit-server-sdk`, which is in neither package.json nor
# node_modules, inside a function only the credentialed lanes reach: every full
# dispatch run died on it after spending a whole interview, and no gate here
# could say so. Every other JS file in the tree is loaded by some gate, so a
# missing package fails that gate; this one is the exception, because the gate
# that runs it needs credentials and a browser.
#
# Resolution, not loading: the require sat behind a credential check, so even
# importing the module would have walked past it. Needs node_modules, and says
# so rather than failing, on the same contract as eslint above.
script_requires()
{
    if [ ! -d "$ROOT/node_modules" ]; then
        skip "script-requires: \`npm install\` enables it"
        return 0
    fi
    # shellcheck disable=SC2016
    # The `${file}` and `${spec}` below are JavaScript template placeholders,
    # not shell expansions, so the single quotes are what has to hold.
    (cd "$ROOT" && node -e '
      const fs = require("fs");
      const { isBuiltin } = require("node:module");
      let unresolved = 0;
      for (const file of process.argv.slice(1)) {
        // Comments first: a `require("x")` inside one is prose, and failing
        // the gate on it would teach people to delete the comment. Only a
        // marker that opens its own line, because one mid-line is as likely
        // to be inside a string: `browser-check.cjs` holds `"    /* setup {"`
        // as a fixture, and a strip that started there ran to a `*/` six
        // lines below it and took a real require with it. The block strip
        // ends at the first `*/` and is not anchored to the line end either,
        // because anchoring it there forced the lazy body past that `*/` to
        // find one that did end a line, which is the same miss again. Every
        // shape left over errs towards a false alarm, which is visible and
        // one line to fix, rather than a miss, which is what the gate is for.
        const source = fs
          .readFileSync(file, "utf8")
          .replace(/^[ \t]*\/\*[\s\S]*?\*\//gm, "")
          .replace(/^[ \t]*\/\/.*$/gm, "");
        for (const [, , spec] of source.matchAll(/require\s*\((["\x27])([^"\x27]+)\1\)/g)) {
          if (spec.startsWith(".") || isBuiltin(spec)) continue;
          try {
            require.resolve(spec, { paths: [process.cwd()] });
          } catch {
            console.error(`${file}: cannot resolve "${spec}"`);
            unresolved = 1;
          }
        }
      }
      process.exit(unresolved);
    ' scripts/*.cjs)
}
gate script-requires script_requires
gate ruff ruff_gate

# The hooks are the one part of this tree that runs on a contributor's machine
# rather than here, so nothing else notices when one stops rejecting. The suite
# builds a scratch repository, installs the hooks into it and drives them.
gate secret-check-tests unittest_gate "$ROOT/tests/test_check_secrets.py"
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
    if [ "${CI:-}" = true ]; then
        failed="$failed skipped-gates"
    fi
}

[ -z "$failed" ] || {
    echo "failed gates:$failed" >&2
    exit 1
}
