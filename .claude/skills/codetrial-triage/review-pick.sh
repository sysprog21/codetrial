#!/bin/sh
# Reads the sweep and touches the ledger file, nothing else: the low and blocked
# pull requests, and any other PR with an open review thread (a draft included),
# that a bare sweep reviews, with what each review agent's brief needs, so the
# briefs are filled from the sweep instead of typed (review-prs.md, "Pick the
# PRs").
#
# Usage: review-pick.sh <sweep.json>
#        Prints one JSON object per PR: {pr, head, title, open, since, why,
#        outdated, collide}. open is the open-thread count the ledger records. since is the head the ledger last recorded for the PR, or
#        null; outdated holds this PR's {thread, path} entries; collide holds
#        the other PR, the file and the function each collision names. A PR by
#        the viewer, whose current head is in the ledger, or with a series
#        rework requested (rework in the sweep) and no open thread, is left out;
#        the newest ledger line for the head, if it records an open-thread
#        count, counts only while the PR still has that many, so a thread
#        opened without a push is read.
#        Exits 2 on bad arguments.

set -eu

usage='usage: review-pick.sh <sweep.json>'
[ $# -eq 1 ] || {
    echo "$usage" >&2
    exit 2
}
sweep=$1
[ -f "$sweep" ] || {
    echo "review-pick.sh: $sweep is not a file" >&2
    exit 2
}
ledger="$(git rev-parse --git-common-dir)/codetrial-triage/reviewed"
mkdir -p "$(dirname "$ledger")"
touch "$ledger"

jq -c --rawfile ledger "$ledger" '
  ($ledger | split("\n") | map(select(length > 0) | split(" "))) as $seen
  | . as $s
  | $s.status[]
  | select(.author != $s.viewer)
  | . as $p
  | ((.threads.human // 0) + (.threads.bot // 0)) as $open
  | select(((.grade == "low" or .grade == "blocked") and (.rework | not)) or $open > 0)
  | ([$seen[] | select(.[0] == ($p.pr | tostring) and .[1] == $p.head)] | last) as $last
  | select($last == null
           or ((($last[2] // "") | tonumber? // null) as $n
               | $n != null and $n < $open))
  | {pr, head, title, why, open: $open,
     since: ([$seen[] | select(.[0] == ($p.pr | tostring)) | .[1]] | last),
     outdated: [$s.outdated[] | select(.pr == $p.pr) | {thread, path}],
     collide: [$s.collide[] | select(.a == $p.pr or .b == $p.pr)
       | {other: (if .a == $p.pr then .b else .a end), path,
          context: .context}]}' "$sweep"
