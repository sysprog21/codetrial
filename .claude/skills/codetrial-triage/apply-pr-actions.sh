#!/bin/sh
# Apply the pull request actions a sweep may take without asking (pulls.md,
# "What a sweep may apply without asking"), each behind the check that section
# names, and print a receipt for every one so the report states what happened
# instead of what was intended.
#
# Usage: apply-pr-actions.sh <sweep.json> [repo] [--hygiene]
#        Prints one JSON object per action: {action, pr, result, why}, with
#        result applied, already, skipped, confirm or failed. Exits 1 when any
#        result is failed, 2 on bad arguments, 0 otherwise. Without --hygiene
#        only approvals and rebase requests run; --hygiene (a bare sweep) adds
#        hygiene.sh for every open PR and posts a non-empty body.
#
# A run no longer waiting for approval counts as already approved only when its
# live status is queued or in_progress; an unread or finished run remains
# unverified and is reported as failed.
#
# confirm means the action is not one a sweep takes alone (a held run that is
# not a first-time contributor's, one that touches workflows or whose file list
# was cut, or any approval while a workflow runs on pull_request_target or
# workflow_run): it goes on the confirmed list. A rebase request is skipped
# unless the head still matches the sweep and, for a conflict, GitHub still says
# CONFLICTING. A PR in `rebase` whose request was not posted keeps all of its
# earlier requests visible (hide needs the new one to have landed), and hide_ask
# is never touched: it is the maintainer's list.

set -eu

usage='usage: apply-pr-actions.sh <sweep.json> [repo] [--hygiene]'
[ $# -ge 1 ] || {
    echo "$usage" >&2
    exit 2
}
sweep=$1
repo=sysprog21/codetrial
hygiene=false
shift
for arg in "$@"; do
    case $arg in
        --hygiene) hygiene=true ;;
        -*)
            echo "apply-pr-actions.sh: $arg is not an option" >&2
            exit 2
            ;;
        *) repo=$arg ;;
    esac
done

# Every list an action reads is checked before the first write, so a sweep cut
# off or hand-edited into the wrong shape fails here and not halfway through.
# The approval flags must be exactly true or false: a string or a missing key is
# no evidence either way.
jq -e '(.prs | arrays) and (.maintainers_known | type == "boolean")
    and all(.prs[]; (.number | type == "number") and (.head | type == "string"))
    and all(.approve[]; (.pr | type == "number") and (.run | type == "number")
        and (.first_time | type == "boolean")
        and (.touches_workflows | type == "boolean")
        and (.files_truncated | type == "boolean"))
    and all(.rebase[]; (.pr | type == "number")
        and (.reason | IN("conflicts", "stale_base")))
    and all(.hide[]; (.pr | type == "number") and (.id | type == "string"))' \
    "$sweep" > /dev/null 2>&1 || {
    echo "apply-pr-actions.sh: $sweep is not a whole sweep" >&2
    exit 2
}
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

receipt()
{
    jq -nc --arg action "$1" --argjson pr "$2" --arg result "$3" --arg why "$4" \
        '{action: $action, pr: $pr, result: $result, why: $why}'
}

live_head()
{
    gh pr view "$1" --repo "$repo" --json headRefOid --jq .headRefOid
}

# Approvals. The run is held for a reason the repository's policy states, and a
# base-token trigger would run a fork's code with secrets, so a workflow that
# uses one sends every approval to the confirmed list. The workflows read are
# this checkout's, so they speak for the default repository only; a clean answer
# needs grep to run (status 1), not to fail.
workflows=${WORKFLOWS_DIR:-$root/.github/workflows}
risky=
if [ "$repo" != sysprog21/codetrial ]; then
    risky="workflows of $repo were not inspected"
elif ! ls "$workflows"/* > /dev/null 2>&1; then
    risky="no workflow files were found to inspect"
else

    # pr-description.yml is the one reviewed exception. It runs the base
    # branch's copy on a read-only token, checks the pull request out as data
    # and runs only the base branch's checker, so a fork's code never runs with
    # the base's token. Any other workflow on these triggers is still a reason
    # to confirm.
    status=0
    found=$(grep -lE 'pull_request_target|workflow_run' "$workflows"/* 2> /dev/null) \
        || status=$?
    if [ "$status" -gt 1 ]; then
        risky="the workflows could not be searched"
    elif [ -n "$(printf '%s\n' "$found" | grep -vE '(^|/)pr-description[.]yml$' || true)" ]; then
        risky="a workflow runs on pull_request_target or workflow_run"
    elif grep -q 'workflow_run' "$workflows/pr-description.yml" 2> /dev/null; then
        # The review covered pull_request_target and nothing else.
        risky="pr-description.yml runs on workflow_run"
    fi
fi
# One row per held run: the PR, the run, and whether every guard holds exactly.
jq -r '.approve[] | [.pr, .run, (.first_time == true and .touches_workflows == false
    and .files_truncated == false)] | @tsv' "$sweep" | while read -r pr id guards; do
    if [ -n "$risky" ]; then
        receipt approve "$pr" confirm "$risky"
    elif [ "$guards" != true ]; then
        receipt approve "$pr" confirm "not a first-time contributor, touches workflows, or the file list was cut"
    elif err=$(gh api -X POST "repos/$repo/actions/runs/$id/approve" 2>&1); then
        receipt approve "$pr" applied "run $id"
    elif printf '%s\n' "$err" | grep -q 'not waiting for approval'; then
        status=$(gh run view "$id" --repo "$repo" --json status --jq .status) || status=unread
        case $status in
            queued | in_progress)
                receipt approve "$pr" already "run $id is $status after approval"
                ;;
            *) receipt approve "$pr" failed "run $id is not waiting, but approval could not be verified (status: $status)" ;;
        esac
    else
        receipt approve "$pr" failed "$err"
    fi
done > "$work/approve.out"

# Rebase requests, then the earlier requests they supersede.
: > "$work/posted"
jq -r '.prs as $p | .rebase[] | . as $r
    | "\($r.pr) \($r.reason) \(first($p[] | select(.number == $r.pr) | .head) // "")"' \
    "$sweep" | while read -r pr reason want; do
    if ! now=$(gh pr view "$pr" --repo "$repo" --json mergeable,headRefOid \
        --jq '.mergeable + " " + .headRefOid'); then
        receipt rebase "$pr" failed "could not read the pull request"
        continue
    fi
    if [ "${now#* }" != "$want" ]; then
        receipt rebase "$pr" skipped "the head moved since the sweep"
    elif [ "$reason" = conflicts ] && [ "${now% *}" != CONFLICTING ]; then
        receipt rebase "$pr" skipped "GitHub no longer reports a conflict (${now% *})"
    elif gh api "repos/$repo/pulls/$pr/reviews" -f commit_id="$want" \
        -f event=REQUEST_CHANGES -F body=@"$here/rebase.md" > /dev/null; then
        echo "$pr" >> "$work/posted"
        receipt rebase "$pr" applied "$reason, review left on $want"
    else
        receipt rebase "$pr" failed "the review was not posted"
    fi
done > "$work/rebase.out"

# The PRs with a rebase request, and whether the maintainers are known, are the
# same for every hide, so they are read once.
jq -r '.rebase[].pr' "$sweep" > "$work/requested"
known=$(jq -r '.maintainers_known' "$sweep")
# shellcheck disable=SC2016  # $id below is a GraphQL variable
jq -r '.hide[] | [.pr, .id, (.by // "unknown")] | @tsv' "$sweep" | while read -r pr id by; do
    if [ "$known" != true ] || [ "$by" = unknown ]; then
        receipt hide "$pr" confirm "maintainer membership or the request's author is unverified: $by"
    elif grep -qx "$pr" "$work/requested" && ! grep -qx "$pr" "$work/posted"; then
        receipt hide "$pr" skipped "its rebase request was not posted, so the earlier one stays"
    elif gh api graphql -F id="$id" -f query='
      mutation($id: ID!) { minimizeComment(input: {subjectId: $id,
        classifier: OUTDATED}) { minimizedComment { isMinimized } } }' > /dev/null; then
        receipt hide "$pr" applied "$id"
    else
        receipt hide "$pr" failed "$id"
    fi
done > "$work/hide.out"

# Contributor hygiene, one review per PR at the head the script read. The reads
# run at once, as hygiene.sh is meant to; the posts follow one at a time.
hygiene_reviews()
{
    prs=$(jq -r '.prs[].number' "$sweep")
    started=0
    for pr in $prs; do
        {
            sh "${APPLY_HYGIENE:-$here/hygiene.sh}" "$pr" "$repo" > "$work/hygiene-$pr.json" 2> /dev/null \
                || rm -f "$work/hygiene-$pr.json"
        } &

        # Eight at a time: each read makes several gh calls, and a repository
        # with many open PRs would otherwise start them all together.
        started=$((started + 1))
        [ $((started % 8)) -ne 0 ] || wait
    done
    wait
    for pr in $prs; do
        file=$work/hygiene-$pr.json
        if [ ! -f "$file" ]; then
            receipt hygiene "$pr" failed "hygiene.sh failed, so the PR is unchecked"
            continue
        fi
        jq -er '.body | select(length > 0)' "$file" > "$work/hygiene-$pr.md" || continue
        head=$(jq -r .head "$file")
        if [ "$(live_head "$pr")" != "$head" ]; then
            receipt hygiene "$pr" skipped "the head moved after it was read"
        elif gh api "repos/$repo/pulls/$pr/reviews" -f commit_id="$head" \
            -f event=REQUEST_CHANGES -F body=@"$work/hygiene-$pr.md" > /dev/null; then
            receipt hygiene "$pr" applied "$(jq -r '[.items[].key] | join(",")' "$file")"
        else
            receipt hygiene "$pr" failed "the review was not posted"
        fi
    done
}
if $hygiene; then
    hygiene_reviews > "$work/hygiene.out"
fi

cat "$work"/*.out
# The subshells that print receipts cannot set failed here, so read it back.
! grep -q '"result":"failed"' "$work"/*.out
