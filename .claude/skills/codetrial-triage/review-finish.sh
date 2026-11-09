#!/bin/sh
# Finish one PR's sweep review: post its inline comments if the head is still
# the one the agent read, and record the head in the ledger only when nothing
# else is left to post (review-prs.md, "Verify and post each PR as it lands").
# The caller has already verified each finding at the head; this is the part
# that was typed by hand per PR.
#
# Usage: review-finish.sh <pr> <draft.json> [repo]
#        Exits 0 once the head is in the ledger. Exits 4, with the pending
#        work printed, when a draft thread is left to resolve (that needs the
#        checks in review-prs.md, so the head is recorded by hand once it is
#        done), a similar-work notice could not be posted, or the refine
#        comment could not be. Exits 3 when the
#        head moved and 1 when post-inline.sh failed; in both the ledger is
#        untouched, though a fallback post may already have landed some
#        comments, which a rerun does not repeat. Exits 2 on a bad argument or
#        a draft that is not {pr, sha, comments, similar, resolve} for this PR.
#        An optional refine array of {url, path, kind} (kind is unaddressed, the
#        default, or resolve for a thread the head addresses but the author left
#        open) posts refine.md with those threads listed, then deletes the
#        viewer's earlier refine comments on the PR (review-prs.md, "Check every
#        open thread").

set -eu

usage='usage: review-finish.sh <pr> <draft.json> [repo]'
[ $# -ge 2 ] || {
    echo "$usage" >&2
    exit 2
}
pr=$1
draft=$2
repo=${3:-sysprog21/codetrial}
case $pr in
    *[!0-9]*)
        echo "review-finish.sh: $pr is not a pull request number" >&2
        exit 2
        ;;
esac

# Every field the script branches on is checked here, so a malformed draft fails
# before any call and can never reach the ledger append below.
jq -se --argjson pr "$pr" 'length == 1 and (.[0]
    | type == "object" and (.sha | type == "string" and test("^[^[:space:]]+$"))
      and .pr == $pr
      and ([.comments, .similar, .resolve] | all(type == "array"))
      and ((.refine // []) | type == "array"
           and all(type == "object" and (.url | type == "string" and length > 0)
                   and (.path | type == "string" and length > 0)
                   and ((.kind // "unaddressed") | IN("unaddressed", "resolve")))))' \
    "$draft" > /dev/null 2>&1 || {
    echo "review-finish.sh: $draft is not a draft for #$pr" >&2
    exit 2
}
here=$(cd "$(dirname "$0")" && pwd)
inline=${REVIEW_FINISH_INLINE:-$here/../codetrial-review/post-inline.sh}

# One read of the draft for what the steps below branch on, taken as a checked
# assignment first: a failure inside the here-document would not stop the
# script.
fields=$(jq -r '[.sha, (.comments | length), (.similar | length),
    (.resolve | length), ((.refine // []) | length)] | @tsv' "$draft")
read -r sha comments similar resolve refine << DRAFT
$fields
DRAFT

# A draft whose notices cannot be posted is refused now, not after the inline
# review has already landed.
if [ "$similar" -gt 0 ]; then
    "$here/post-similar.sh" --check "$pr" "$draft" "$repo" || exit 2
fi
live=$(gh pr view "$pr" --repo "$repo" --json headRefOid --jq .headRefOid)
if [ "$live" != "$sha" ]; then
    echo "review-finish.sh: #$pr moved to $live; $sha was reviewed" >&2
    exit 3
fi

if [ "$comments" -gt 0 ]; then
    "$inline" "$pr" "$draft" "$repo"
fi

pending=false

# Threads the head leaves open: one plain comment, the fixed text of refine.md
# plus a link per thread, naming every thread the draft lists. It replaces the
# viewer's earlier refine comments on the PR, which are deleted once the new one
# is posted, so the PR carries one current reminder and no stale copies. A
# history that cannot be read posts nothing and leaves the head out of the
# ledger; a copy that cannot be deleted leaves it out too, so the retry finishes
# the cleanup.
post_refine()
{
    me=$(gh api user --jq .login) || return 1
    history=$(gh api --paginate "repos/$repo/issues/$pr/comments") || return 1
    body=$(mktemp)
    trap 'rm -f "$body"' EXIT
    cat "$here/refine.md" > "$body"
    found=false
    for kind in unaddressed resolve; do
        fresh=$(jq -r --arg k "$kind" '.refine[]
            | select((.kind // "unaddressed") == $k) | "- \(.url) (`\(.path)`)"' "$draft")
        [ -n "$fresh" ] || continue
        found=true
        case $kind in
            unaddressed) printf '\nNot addressed at the current head:\n' ;;
            resolve) printf '\nAddressed at the current head; reply with what changed and resolve:\n' ;;
        esac >> "$body"
        printf '%s\n' "$fresh" >> "$body"
    done
    $found || return 0
    printf '\nChecked at %s.\n' "$sha" >> "$body"

    # Every viewer reminder but one that already says exactly this is stale, a
    # second identical copy included. One identical copy is kept and not posted
    # twice, and the others still go, so a retry after a failed delete finishes
    # the cleanup.
    mine=$(printf '%s\n' "$history" | jq -rs --arg me "$me" --arg m "$(cat "$here/refine.md")" \
        --rawfile body "$body" '[.[][] | select(.user.login == $me and (.body | startswith($m)))] as $all
        | [$all[] | select(.body == $body) | .id] as $same
        | {same: ($same | length > 0),
           old: ([$all[] | select(.body != $body) | .id] + $same[1:])}') || return 1
    if [ "$(printf '%s\n' "$mine" | jq -r .same)" = false ]; then
        gh pr comment "$pr" --repo "$repo" --body-file "$body" || return 1
    fi
    status=0
    for id in $(printf '%s\n' "$mine" | jq -r '.old[]'); do
        gh api -X DELETE "repos/$repo/issues/comments/$id" > /dev/null || status=1
    done
    return $status
}
if [ "$refine" -gt 0 ]; then
    post_refine || pending=true
fi

# Similar-work notices go out through post-similar.sh, which needs SCRATCH. A
# notice that could not be posted (a head moved, a comment refused) leaves the
# head out of the ledger, like a thread still to resolve.
if [ "$similar" -gt 0 ]; then
    "$here/post-similar.sh" "$pr" "$draft" "$repo" || pending=true
fi
if [ "$resolve" -gt 0 ]; then
    jq '{resolve}' "$draft"
    pending=true
fi
if $pending; then
    exit 4
fi

ledger="$(git rev-parse --git-common-dir)/codetrial-triage/reviewed"
mkdir -p "$(dirname "$ledger")"

# The open-thread count the sweep saw, so a thread opened later without a push
# brings the PR back (review-pick.sh reads the same field); a sweep that does
# not show this PR at this head records the head alone.
threads=
if [ -n "${SCRATCH:-}" ] && [ -f "$SCRATCH/sweep.json" ]; then
    threads=$(jq -r --argjson pr "$pr" --arg sha "$sha" '[.status[]
        | select(.pr == $pr and .head == $sha)
        | (.threads.human // 0) + (.threads.bot // 0)] | first // empty' \
        "$SCRATCH/sweep.json" 2> /dev/null) || threads=
fi
# The inline comments just posted open threads of their own.
[ -z "$threads" ] || threads=$((threads + comments))
echo "$pr $sha${threads:+ $threads}" >> "$ledger"
