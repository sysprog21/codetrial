#!/bin/sh
# Report main push runs of check.yml that ended without all of their jobs.
#
# An aggregator that never got created cannot report its own absence, so this
# counts from outside: a finished run that has no `check` job is listed. A
# `check` that ran and failed is already red, so it is not. A cancelled run is
# listed only when it has no jobs at all (the per-commit group no longer lets a
# push replace a main run, so that is not a replacement); a cancelled run with
# jobs and a skipped run are not. Read-only. Exits 1 when a run is listed, 2
# when the API or the arguments failed, which wins over a finding: a partial
# scan is not an answer.
#
#   scripts/check-run-jobs.sh [REPO [LIMIT]]      # LIMIT at most 100
#   scripts/check-run-jobs.sh --run ID [REPO]     # one run, any event
set -eu

if [ "${1:-}" = --run ]; then
    ids=${2:?usage: check-run-jobs.sh --run ID [REPO]}
    repo=${3:-sysprog21/codetrial}
else
    repo=${1:-sysprog21/codetrial}
    limit=${2:-50}
    case $limit in
        [1-9] | [1-9][0-9] | 100) ;;
        *)
            echo "LIMIT must be 1..100" >&2
            exit 2
            ;;
    esac
    ids=$(gh api "repos/$repo/actions/workflows/check.yml/runs?branch=main&event=push&status=completed&per_page=$limit" \
        --jq '.workflow_runs[].id') || exit 2
fi

bad=0
err=0
for id in $ids; do

    # The listing can race a rerun, and --run takes any id: read the state
    # first.
    state=$(gh api "repos/$repo/actions/runs/$id" --jq '"\(.status) \(.conclusion)"') || state=error
    case $state in
        "completed skipped") continue ;;
        completed*) ;;
        error)
            echo "run $id: run API error"
            err=1
            continue
            ;;
        *) continue ;;
    esac
    # Latest attempt only: that is what the run page and the check suite show.
    if ! names=$(gh api --paginate "repos/$repo/actions/runs/$id/jobs?per_page=100" --jq '.jobs[].name'); then
        echo "run $id: jobs API error"
        err=1
    elif [ "$state" = "completed cancelled" ] && [ -n "$names" ]; then
        continue
    elif ! printf '%s\n' "$names" | grep -qx check; then
        echo "run $id: $(printf '%s\n' "$names" | grep -c .) jobs, check: missing"
        bad=1
    fi
done
[ "$err" -eq 0 ] || exit 2
exit "$bad"
