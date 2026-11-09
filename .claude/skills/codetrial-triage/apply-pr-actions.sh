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
#        only approvals, rebase requests and the hides run; --hygiene (a bare
#        sweep) adds hygiene.sh for every open PR, posting a non-empty body as a
#        review and each comment it names as a comment, and a thanks on each
#        pull request merged in the last three days that has none.
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
    and all(.hide[]; (.pr | type == "number") and (.id | type == "string"))
    and all(.hide_resolved[]; (.pr | type == "number") and (.id | type == "string"))
    and all(.refine_stale[]; (.pr | type == "number") and (.id | type == "string"))' \
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

# A write that failed on the network is tried again twice before it is reported:
# a sweep makes hundreds of them, and one timeout (as on #584) is not a failure
# of the action. Only a transient error is retried; a missing id or a refusal
# fails at once and is not slept on, unless the id vanished after a retry.
retry()
{
    tries=1
    while ! err=$("$@" 2>&1 > /dev/null); do
        # A node gone after a retried attempt means the first one did land.
        if [ "$tries" -gt 1 ] && printf '%s\n' "$err" | grep -qi 'could not resolve to a node'; then
            return 0
        fi
        if [ "$tries" -ge 3 ] || ! printf '%s\n' "$err" \
            | grep -qiE 'timed out|timeout|connection|EOF|rate limit|HTTP 5[0-9][0-9]'; then
            printf '%s\n' "$err" >&2
            return 1
        fi
        tries=$((tries + 1))
        sleep "${RETRY_DELAY:-2}"
    done
}

# Hide one comment or review with a GitHub classifier (OUTDATED, RESOLVED).
minimize()
{
    # shellcheck disable=SC2016  # $id below is a GraphQL variable
    retry gh api graphql -F id="$1" -f query="
      mutation(\$id: ID!) { minimizeComment(input: {subjectId: \$id,
        classifier: $2}) { minimizedComment { isMinimized } } }" > /dev/null
}

# Delete the viewer's own comment; a review cannot be deleted once submitted.
delete_comment()
{
    # shellcheck disable=SC2016  # $id below is a GraphQL variable
    retry gh api graphql -F id="$1" -f query='
      mutation($id: ID!) { deleteIssueComment(input: {id: $id}) {
        clientMutationId } }' > /dev/null
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
jq -r '.hide[] | [.pr, .id, (.by // "unknown"), (.act // "hide")] | @tsv' \
    "$sweep" | while read -r pr id by act; do
    if [ "$known" != true ] || [ "$by" = unknown ]; then
        receipt hide "$pr" confirm "maintainer membership or the request's author is unverified: $by"
    elif grep -qx "$pr" "$work/requested" && ! grep -qx "$pr" "$work/posted"; then
        receipt hide "$pr" skipped "its rebase request was not posted, so the earlier one stays"
    elif [ "$act" = delete ]; then
        if delete_comment "$id"; then
            receipt hide "$pr" applied "deleted $id"
        else
            receipt hide "$pr" failed "$id"
        fi
    elif minimize "$id" OUTDATED; then
        receipt hide "$pr" applied "$id"
    else
        receipt hide "$pr" failed "$id"
    fi
done > "$work/hide.out"

# A review whose every thread is resolved or outdated is settled, so it is
# hidden as resolved; the thread stays where it is and the review is one click
# from readable. A refine request left on a PR with no open thread is stale, so
# it is deleted: a reminder that no longer asks for anything only misleads. On a
# PR that still has open threads every copy but the newest is deleted too, so
# one current reminder stands.
live_threads()
{
    # shellcheck disable=SC2016  # GraphQL variables
    gh api graphql -F owner="${repo%/*}" -F name="${repo#*/}" -F pr="$1" -f query='
          query($owner: String!, $name: String!, $pr: Int!) {
            repository(owner: $owner, name: $name) { pullRequest(number: $pr) {
              reviewThreads(first: 100) { totalCount nodes { isResolved isOutdated
                comments(first: 20) { totalCount nodes { pullRequestReview { id } } } } }
            } }
          }'
}

# shellcheck disable=SC2016  # $id below is a GraphQL variable
jq -r '.hide_resolved[] | [.pr, .id] | @tsv' "$sweep" | while read -r pr id; do
    if ! state=$(live_threads "$pr"); then
        receipt hide_resolved "$pr" failed "could not read threads"
    elif ! printf '%s\n' "$state" | jq -e --arg id "$id" '
        .data.repository.pullRequest.reviewThreads as $t
        | ($t.nodes | type == "array") and ($t.totalCount == ($t.nodes | length))
          and all($t.nodes[]; .comments.totalCount == (.comments.nodes | length))
          and ([$t.nodes[] | select(any(.comments.nodes[]; .pullRequestReview.id == $id))] as $matched
            | ($matched | length) > 0 and all($matched[]; .isResolved or .isOutdated))' > /dev/null; then
        receipt hide_resolved "$pr" skipped "threads changed or could not be verified"
    elif minimize "$id" RESOLVED; then
        receipt hide_resolved "$pr" applied "$id"
    else
        receipt hide_resolved "$pr" failed "$id"
    fi
done > "$work/hide-resolved.out"

# The viewer's refine reminders on a PR, oldest first, each with its node id.
# Read for every row, so a copy review-finish.sh trimmed in between is seen.
live_reminders()
{
    # shellcheck disable=SC2016  # GraphQL variables
    gh api graphql -F owner="${repo%/*}" -F name="${repo#*/}" -F pr="$1" -f query='
          query($owner: String!, $name: String!, $pr: Int!) {
            repository(owner: $owner, name: $name) { pullRequest(number: $pr) {
              comments(last: 100) { totalCount nodes { id createdAt body author { login } } }
            } }
          }' | jq --arg me "$(jq -r .viewer "$sweep")" --arg m "$(cat "$here/refine.md")" '
        .data.repository.pullRequest.comments.nodes |= (
          map(select(.author.login == $me and (.body | startswith($m))))
          | sort_by([.createdAt, .id]))'
}

# Delete a reminder the sweep found superseded, whatever the threads say, but
# only while the live PR still has it as the viewer's reminder with a newer one
# standing: review-finish.sh may have trimmed the PR since the sweep.
delete_superseded()
{
    mine=$(live_reminders "$1") || {
        receipt refine_stale "$1" failed "could not read comments"
        return
    }
    place=$(printf '%s\n' "$mine" | jq -r --arg id "$2" '
        [.data.repository.pullRequest.comments.nodes[].id] as $ids
        | ($ids | index($id)) as $at
        | if $at == null and .data.repository.pullRequest.comments.totalCount > 100 then "unverified"
          elif $at == null then "gone"
          elif $at == ($ids | length) - 1 then "newest"
          else "ok" end') || place=
    case $place in
        gone) receipt refine_stale "$1" already "$2 is gone" ;;
        unverified) receipt refine_stale "$1" skipped "$2 is outside the last 100 comments" ;;
        newest) receipt refine_stale "$1" skipped "$2 is now the only current reminder" ;;
        ok)
            if delete_comment "$2"; then
                receipt refine_stale "$1" applied "deleted $2"
            else
                receipt refine_stale "$1" failed "$2"
            fi
            ;;
        *) receipt refine_stale "$1" failed "could not verify $2" ;;
    esac
}

# shellcheck disable=SC2016  # $id below is a GraphQL variable
jq -r '.refine_stale[] | [.pr, .id, (.why // "settled")] | @tsv' "$sweep" | while read -r pr id why; do
    if [ "$why" = superseded ]; then
        delete_superseded "$pr" "$id"
    elif ! state=$(live_threads "$pr"); then
        receipt refine_stale "$pr" failed "could not read threads"
    elif ! printf '%s\n' "$state" | jq -e '
        .data.repository.pullRequest.reviewThreads as $t
        | ($t.nodes | type == "array") and ($t.totalCount == ($t.nodes | length))
          and all($t.nodes[]; .isResolved == true)' > /dev/null; then
        receipt refine_stale "$pr" skipped "threads changed or could not be verified"
    elif delete_comment "$id"; then
        receipt refine_stale "$pr" applied "deleted $id"
    else
        receipt refine_stale "$pr" failed "$id"
    fi
done > "$work/refine-stale.out"

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
        jq -e '(.body | length > 0) or ((.comments // []) | length > 0)
            or ((.hide // []) | length > 0)' "$file" > /dev/null || continue
        head=$(jq -r .head "$file")
        moved=false
        [ "$(live_head "$pr")" = "$head" ] || moved=true
        replaced=true
        if jq -er '.body | select(length > 0)' "$file" > "$work/hygiene-$pr.md"; then
            if $moved; then
                receipt hygiene "$pr" skipped "the head moved after it was read"
            elif gh api "repos/$repo/pulls/$pr/reviews" -f commit_id="$head" \
                -f event=REQUEST_CHANGES -F body=@"$work/hygiene-$pr.md" > /dev/null; then
                receipt hygiene "$pr" applied "$(jq -r '[.items[].key] | join(",")' "$file")"
            else
                receipt hygiene "$pr" failed "the review was not posted"
                replaced=false
            fi
        fi

        # The copies this review replaces, hidden only after it is up, so a
        # failed post never leaves the PR with no visible request. Hiding does
        # not dismiss a review; the PR stays at changes requested until a
        # maintainer re-reviews.
        if [ "$replaced" = true ] && ! $moved; then
            jq -r '(.hide // [])[] | [.id, .why] | @tsv' "$file" | while IFS="$(printf '\t')" read -r id why; do
                if minimize "$id" OUTDATED; then
                    receipt hygiene_hide "$pr" applied "$why $id"
                else
                    receipt hygiene_hide "$pr" failed "$id"
                fi
            done
        fi

        # Requests that are a plain comment: one file per comment, since the
        # text is posted as it stands.
        jq -r '(.comments // [])[].key' "$file" | while read -r key; do
            jq -r --arg k "$key" '.comments[] | select(.key == $k) | .text' "$file" \
                > "$work/hygiene-$pr-$key.md"
            if $moved; then
                receipt "hygiene_$key" "$pr" skipped "the head moved after it was read"
            elif gh pr comment "$pr" --repo "$repo" --body-file "$work/hygiene-$pr-$key.md" > /dev/null; then
                receipt "hygiene_$key" "$pr" applied "comment posted"
            else
                receipt "hygiene_$key" "$pr" failed "the comment was not posted"
            fi
        done
    done
}

# A thanks on each pull request a non-maintainer got merged, as the maintainers
# leave by hand on every one. Three days back, so a sweep never backfills old
# merges, and a comment from any maintainer that opens with the thanks counts.
thanks()
{
    if ! pushers=$(gh api --paginate "repos/$repo/collaborators?permission=push" \
        --jq '.[].login' 2> /dev/null); then
        receipt thanks 0 failed "the push listing failed, so nobody can be told apart as a maintainer"
        return
    fi
    since=$(jq -nr 'now - 3 * 86400 | strftime("%Y-%m-%d")')
    if ! merged=$(gh pr list --repo "$repo" --state merged --limit 200 \
        --search "merged:>=$since" --json number,author,comments); then
        receipt thanks 0 failed "the merged list could not be read"
        return
    fi
    printf '%s\n' "$merged" | jq -r --argjson pushers \
        "$(printf '%s\n' "$pushers" | jq -Rn '[inputs | select(length > 0)]')" '
        .[] | .author as $a | select($a != null and ($a.is_bot | not))
        | select(any($pushers[]; . == $a.login) | not)
        | select(any(.comments[]; (.author.login | IN($pushers[]))
            and (.body | test("^\\s*thank[^\n]*@\($a.login)\\b"; "i"))) | not)
        | "\(.number) \($a.login)"' | while read -r pr login; do
        printf 'Thank @%s for contributing!\n' "$login" > "$work/thanks-$pr.md"
        if gh pr comment "$pr" --repo "$repo" --body-file "$work/thanks-$pr.md" > /dev/null; then
            receipt thanks "$pr" applied "thanked $login"
        else
            receipt thanks "$pr" failed "the comment was not posted"
        fi
    done
}
if $hygiene; then
    hygiene_reviews > "$work/hygiene.out"
    thanks > "$work/thanks.out"
fi

cat "$work"/*.out
# The subshells that print receipts cannot set failed here, so read it back.
! grep -q '"result":"failed"' "$work"/*.out
