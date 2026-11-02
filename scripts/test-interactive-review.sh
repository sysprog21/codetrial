#!/bin/sh

# Run every test that covers the interactive review workflow in one command.
#
# scripts/test.sh runs the same suites as separate gates among everything else;
# this is the entry point for someone editing the workflow who wants its whole
# safety net and nothing more. Each suite names what it covers:
#
#   test_hashchain.py                  Change-Id generation, chain verification,
#                                      merges, bootstrap, rechain, rebase, bots
#   test_agent_guard.py                the agent hook-bypass guard
#   test_interactive_review_hook.py    200-line and 500-line thresholds, evidence,
#                                      certificates, markers, real hook runs
#   test_review_workbench.py           the local server, state, stale requests,
#                                      reflections, diff and size handling
#   test_review_worker.py              the optional Codex worker's lifecycle
#   test_review_lifecycle.py           the review receipt that certification reads
#   test_skill_scripts.py              review-bundle and experience scripts
#   test-git-hooks.sh                  the commit and push hooks in a scratch repo
#   review-workbench.test.js           the page in a real browser
#
# The browser suite needs Playwright's Chromium and is reported as skipped,
# never as passed, when it cannot start.

set -u

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
failures=
skipped=
python=${PYTHON:-}

python_ok()
{
    "$1" -c 'import sys; sys.exit(sys.version_info < (3, 11))' > /dev/null 2>&1
}

if [ -z "$python" ]; then
    for candidate in python3 python3.15 python3.14 python3.13 python3.12 python3.11; do
        if command -v "$candidate" > /dev/null 2>&1 && python_ok "$candidate"; then
            python=$candidate
            break
        fi
    done
fi
if [ -z "$python" ] || ! python_ok "$python"; then
    echo "the interactive review tests need Python 3.11+; set PYTHON to one." >&2
    exit 2
fi

run()
{
    name=$1
    shift
    printf '== %s\n' "$name"
    if ! "$@"; then
        failures="$failures $name"
    fi
}

# The workbench and worker suites patch process-wide state, so their cases run
# one at a time; the others run their cases across a thread pool.
suite()
{
    file=$1
    shift
    run "$file" "$python" "$ROOT/scripts/run-python-tests.py" "$@" "$ROOT/tests/$file"
}

suite test_hashchain.py
suite test_agent_guard.py
suite test_interactive_review_hook.py
suite test_review_workbench.py --jobs 1
suite test_review_worker.py --jobs 1
suite test_review_lifecycle.py --jobs 1
suite test_skill_scripts.py
run test-git-hooks.sh sh "$ROOT/scripts/test-git-hooks.sh"

if command -v node > /dev/null 2>&1 && [ -d "$ROOT/node_modules" ]; then
    run review-workbench.test.js env PYTHON="$python" \
        node --test "$ROOT/tests/browser/review-workbench.test.js"
else
    echo "== review-workbench.test.js: skipped (node or node_modules missing)"
    skipped="$skipped review-workbench.test.js"
fi

if [ -n "$failures" ]; then
    echo "FAILED:$failures" >&2
    exit 1
fi
if [ -n "$skipped" ]; then

    # Said as what it is: a green run that did not run the page is not the whole
    # net.
    echo "Interactive review tests passed, but not run:$skipped" >&2
    exit 0
fi
echo "Interactive review tests passed."
