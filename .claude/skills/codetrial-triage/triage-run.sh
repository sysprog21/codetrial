#!/bin/sh
# The opening call of a bare sweep: the issue sweep and the pull request sweep
# at once, then one digest of what there is to do, so a run reads a few hundred
# bytes instead of diffing two large snapshots by hand. Reads GitHub only; it
# writes the scratch directory and the two baselines in the git directory.
#
# Usage: triage-run.sh <scratch-dir> [repo] (default sysprog21/codetrial)
#        Writes <scratch-dir>/sweep.json and issues.json and prints one JSON
#        object: {sweep, issues, issues_since, open_prs, truncated_prs, moved,
#        gone, unknown,
#        actions, retitled, picks, issue_counts, issue_truncated, issue_changes}.
#        actions names the PRs in each non-empty action list of the sweep.
#        retitled names the open PRs whose title differs from the saved sweep's;
#        a description edit is not seen. picks is review-pick.sh's, cut to
#        {pr, head, since, open, needs_agent}, needs_agent being false when since
#        equals head; the whole objects, one per line, go to
#        <scratch-dir>/picks.jsonl for review-brief.sh.
#        issue_changes holds, per issue list that changed since the previous
#        run, the issue numbers added, removed and changed (age_days and
#        idle_days aside); null on a first run. issues_since is when that
#        previous run read the issues, or null: an edit that moves no issue
#        between lists shows only in a search for issues updated since then.
#        Each half is tried three times. Exits 1, printing nothing, when
#        either half still failed or no digest could be made, and both
#        baselines are then left as they were; 2 on
#        bad arguments. TRIAGE_ISSUE_SWEEP names the issue sweep to run, for
#        tests.

set -eu

usage='usage: triage-run.sh <scratch-dir> [repo]'
if [ $# -lt 1 ] || [ $# -gt 2 ] || [ -z "$1" ]; then
    echo "$usage" >&2
    exit 2
fi
scratch=$1
repo=${2:-sysprog21/codetrial}
here=$(cd "$(dirname "$0")" && pwd)
state=$(git rev-parse --git-common-dir)/codetrial-triage
mkdir -p "$scratch" "$state"
scratch=$(cd "$scratch" && pwd)
slug=$(printf '%s' "$repo" | tr / -)
base=$state/issue-sweep-$slug.json

# Both reads fail now and then on a GitHub 504, so each is run again.
thrice()
{
    for try in 1 2 3; do
        if "$@"; then
            return 0
        fi
        [ "$try" = 3 ] || sleep "${RETRY_DELAY:-20}"
    done
    return 1
}
issue_half()
{
    # Stamped before the read, so an edit made while either half runs is still
    # newer than issues_since on the next run.
    date -u +%Y-%m-%dT%H:%M:%SZ > "$scratch/issues-taken-at" || return 1
    sh "${TRIAGE_ISSUE_SWEEP:-$here/issue-sweep.sh}" "$repo" > "$scratch/issues.json" \
        && jq -e '.waiting | arrays' "$scratch/issues.json" > /dev/null 2>&1
}
pr_half()
{
    snap=$(sh "$here/../codetrial-odds/snapshot.sh" "$repo") && cp "$snap" "$scratch/sweep.json"
}

# The sweep snapshot.sh is about to replace, for the titles.
saved=$state/pr-sweep-$slug.json
echo '{}' > "$scratch/previous-sweep.json"
first=true
if [ -s "$saved" ]; then
    first=false
    cp "$saved" "$scratch/previous-sweep.json"
fi

# snapshot.sh moves its baseline on its own, so a run that prints no digest puts
# the old one back, or none on a first run: the changes in between were never
# shown to anyone. A signal can end this script while a half still runs, so both
# are waited for before the baseline they may replace is restored.
printed=false
issues_saved=false
restore()
{
    if $printed; then
        return
    fi
    wait
    if $issues_saved; then
        if [ -f "$scratch/previous-issues-baseline.json" ]; then
            cp "$scratch/previous-issues-baseline.json" "$base"
        else
            rm -f "$base"
        fi
    fi
    if $first; then
        rm -f "$saved"
    elif jq -e '.status' "$scratch/previous-sweep.json" > /dev/null 2>&1; then
        cp "$scratch/previous-sweep.json" "$saved"
    fi
}
trap restore EXIT
trap 'exit 1' INT TERM HUP

thrice issue_half &
issues_pid=$!
thrice pr_half &
sweep_pid=$!

failed=0
wait "$issues_pid" || {
    echo "triage-run.sh: the issue sweep failed three times" >&2
    failed=1
}
wait "$sweep_pid" || {
    echo "triage-run.sh: the pull request sweep failed three times" >&2
    failed=1
}
[ "$failed" = 0 ] || exit 1

sh "$here/review-pick.sh" "$scratch/sweep.json" > "$scratch/picks.jsonl" || exit 1
previous=$base
[ -s "$previous" ] || previous=/dev/null

digest=$(jq -n --arg sweep "$scratch/sweep.json" --arg issues "$scratch/issues.json" \
    --slurpfile s "$scratch/sweep.json" --slurpfile now "$scratch/issues.json" \
    --slurpfile old "$previous" --slurpfile was "$scratch/previous-sweep.json" --slurpfile picks "$scratch/picks.jsonl" '
  $s[0] as $s | $now[0] as $now
  | ["unanswered", "waiting", "linked", "claimed", "lint"] as $lists
  # claimed has one entry per assignee, so an issue is compared as the set of
  # its entries.
  | def by_issue: map(del(.age_days, .idle_days)) | group_by(.issue)
      | map({key: (.[0].issue | tostring), value: sort}) | from_entries;
    def numbers: map(tonumber) | sort;
  {sweep: $sweep, issues: $issues,
   issues_since: ($old[0].taken_at // null),
   open_prs: ($s.prs | length),
   truncated_prs: [$s.prs[]
     | select(any(to_entries[]; (.key | endswith("_truncated")) and .value == true))
     | .number],
   moved: [($s.moved // [])[]
     | if .changed then
         {pr} + (.changed | with_entries(select(.key | IN("grade", "mergeable", "decision", "waits_on"))))
         + {also: (.changed | keys - ["grade", "mergeable", "decision", "waits_on"])}
       else . end],
   gone: [($s.gone // [])[] | {pr, state}],
   unknown: ($s.unknown // []),
   actions: ([("approve", "rebase", "hide", "hide_ask", "hide_resolved",
       "refine_stale", "outdated", "stale", "refs_only", "overlap") as $k
     | {key: $k, value: [($s[$k] // [])[] | .pr // (.a, .b)] | unique}
     | select(.value | length > 0)] | from_entries),
   retitled: (($was[0].prs // []) as $before
     | [$s.prs[] as $p | $before[]
         | select(.number == $p.number and .title != $p.title) | .number]),
   picks: [$picks[] | {pr, head, since, open, needs_agent: (.since != .head)}],
   issue_counts: ([$lists[] | {key: ., value: ($now[.] | length)}] | from_entries),
   issue_truncated: ($now.truncated // []),
   issue_changes: (if ($old | length) == 0 then null else
     [$lists[] as $l
       | (($old[0][$l] // []) | by_issue) as $a | (($now[$l] // []) | by_issue) as $b
       | {key: $l, value: {
           added: (($b | keys) - ($a | keys) | numbers),
           removed: (($a | keys) - ($b | keys) | numbers),
           changed: ([$b | keys[] | select($a[.] != null and $a[.] != $b[.])] | numbers)}}
       | select(any(.value[]; length > 0))] | from_entries
   end)}') || exit 1

# Renamed into place, so an interrupted run never leaves half a baseline; the
# old one is kept until the digest is out, for restore.
rm -f "$scratch/previous-issues-baseline.json"
[ ! -f "$base" ] || cp "$base" "$scratch/previous-issues-baseline.json"
next=$(mktemp "$base.XXXXXX")
if ! {
    jq --arg at "$(cat "$scratch/issues-taken-at")" '. + {taken_at: $at}' \
        "$scratch/issues.json" > "$next" && mv "$next" "$base"
}; then
    rm -f "$next"
    echo "triage-run.sh: the issue baseline could not be saved" >&2
    exit 1
fi
issues_saved=true
printf '%s\n' "$digest"
printed=true
