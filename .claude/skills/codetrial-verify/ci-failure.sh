#!/bin/sh
# Read-only diagnosis of a pull request's failing checks: which failed on the
# current head, which gates scripts/test.sh named in its last line, the error
# lines around them, and whether the same job also fails on main, so a stale
# run, a regression the branch brought and a failure main shares are told apart
# before anyone is asked to fix anything. Writes nothing.
#
# Usage: ci-failure.sh <pr> [repo] (repo defaults to sysprog21/codetrial)
#
# Prints one JSON object:
#   pr, head    the PR and its current head commit
#   pending     checks still running; held: checks awaiting approval, which
#               have no log yet
#   failing     each failed, cancelled or timed-out check: name, run and job
#               ids, current (its run is on the head), gates (from "failed
#               gates:"), errors (the lines of its job log that mark a
#               failure), excerpt (the last lines before the gate summary),
#               on_main (the job of that name also failed in the latest
#               finished check run on main), or, for a check outside GitHub
#               Actions, its link only. current and on_main are null when the
#               lookup behind them failed, and gates, errors and excerpt when
#               the job log could not be read. untouched is true when every
#               gate the job failed reads only the paths its entry in the
#               confined table below names, the PR changes none of them,
#               renames nothing and its file list is whole: no change in the branch can have made
#               that failure, so it is a flake or main's, never the author's
#               (#785 was asked to fix a desktop test that hit "Text file
#               busy" on a change to web/ alone). False otherwise.
#   stale_base  legacy portable-audit runs only; the independent audit job
#               reports inherited findings without failing. True when every
#               failing lane failed only gates that read
#               outside data (the table in the last jq below: cargo-audit
#               reads the advisory database), on the current head, main's
#               latest run started after those runs and succeeded in each of
#               those lanes, nothing is pending or held, no check outside
#               Actions fails, and the PR changes none of the files those gates
#               read: main has since moved past the data, and a rebase is the
#               whole answer. False otherwise, including when a lookup failed.
#               This is the one definition; the docs point here.

set -eu

pr=${1:?usage: ci-failure.sh <pr> [repo]}
repo=${2:-sysprog21/codetrial}
case $pr in
    *[!0-9]*)
        echo "ci-failure.sh: $pr is not a pull request number" >&2
        exit 2
        ;;
esac

# The head, and the files the PR changes: a PR that changes what a gate reads
# may have brought the failure itself, so it is never a stale base. gh reads at
# most 100 files, and cut says the list may hide one.
meta=$(gh pr view "$pr" --repo "$repo" --json headRefOid,files,changedFiles --jq \
    '{head: .headRefOid, files: [.files[].path],
      cut: (.changedFiles > (.files | length)),
      renamed: any(.files[]; .changeType == "RENAMED")}')
head=$(printf '%s\n' "$meta" | jq -r .head)

# gh pr checks exits non-zero when a check fails, and still prints them all, so
# its status says nothing; only an array on stdout says the lookup worked. A PR
# with no checks at all prints an empty one. Slurped, so empty output fails too;
# jq 1.6 -e alone passes it.
checks=$(gh pr checks "$pr" --repo "$repo" --json name,state,bucket,link || true)
printf '%s\n' "$checks" | jq -se 'map(type) == ["array"]' > /dev/null 2>&1 || {
    echo "ci-failure.sh: could not read the checks of #$pr" >&2
    exit 1
}

# The latest finished run of the check workflow on main, for comparison. A
# lookup that fails leaves on_main null rather than false, which would put a
# failure main shares on the author. --branch alone also matches a fork PR
# opened from its own main, so only a push counts; a run cancelled whole says
# nothing about its jobs and is passed over. A job counts as failed by the same
# reading as the PR's checks below, so a lane that times out on both is shared.
# A lane absent from the failures may also have been skipped or never run, which
# says nothing about whether main passes, so stale_base below reads the
# successes from main_jobs itself. A main run older than the PR's says nothing
# about data published since (a new advisory fails every PR before main runs
# again), so its start time is kept too.
main_at=
if ! main_run=$(gh run list --repo "$repo" --workflow check.yml --branch main \
    --event push --status completed --limit 10 \
    --json databaseId,conclusion,createdAt \
    --jq '[.[] | select(.conclusion != "cancelled")][0] // empty
      | "\(.databaseId) \(.createdAt)"'); then
    main_jobs=null
elif [ -z "$main_run" ]; then
    main_jobs='[]'
else
    main_at=${main_run#* }
    main_run=${main_run% *}
    main_jobs=$(gh run view "$main_run" --repo "$repo" --json jobs \
        --jq '[.jobs[] | {name, conclusion}]') || main_jobs=null
fi
main_failed=$(printf '%s\n' "$main_jobs" | jq -c 'if . == null then null
    else [.[] | select(.conclusion
      | IN("failure", "cancelled", "timed_out", "startup_failure")) | .name] end')

# Which checks failed, by the same reading as checks in pr-sweep.sh: failed,
# cancelled or timed out counts, a run held for approval does not (gh files it
# under fail), and a startup failure counts though gh files it as pending. job
# is the run and job of an Actions check; a check from an app has none and is
# reported by name and link alone.
# shellcheck disable=SC2016
defs='def failed: (.bucket == "fail" or .bucket == "cancel"
    or .state == "STARTUP_FAILURE") and .state != "ACTION_REQUIRED";
  def job: .link | capture("/actions/runs/(?<run>[0-9]+)/job/(?<job>[0-9]+)");'

# The whole job log rather than --log-failed: a job cancelled at its time limit
# has no failed step, so --log-failed prints nothing for it (#300, whose log
# shows the SyntaxError that hung every lobby test). A real failure is found by
# its markers, since every passing Rust suite prints "0 failed" and the browser
# lane prints "Error:" for tests that expect one.
failing=$(printf '%s\n' "$checks" | jq -r "$defs"'.[] | select(failed)
    | job as $id | [.name, $id.run, $id.job] | @tsv' \
    | while IFS="$(printf '\t')" read -r name run job; do
        ran=$(gh run view "$run" --repo "$repo" --json headSha,createdAt \
            --jq '.headSha + " " + .createdAt' || true)
        sha=${ran% *}
        ran_at=${ran#* }

        # Each log line is job, step and a timestamped message; keep the
        # message. A log that cannot be read leaves gates, errors and excerpt
        # null, so an unread log is not taken for one with nothing in it.
        if raw=$(gh run view "$run" --repo "$repo" --job "$job" --log 2> /dev/null); then
            read_log=true
        else
            read_log=false
        fi
        log=$(printf '%s\n' "$raw" | cut -f3-)
        printf '%s\n' "$log" | jq -Rs --arg name "$name" --arg run "$run" \
            --arg job "$job" --arg sha "$sha" --arg head "$head" \
            --arg ran_at "$ran_at" --arg main_at "$main_at" \
            --argjson main "$main_failed" --argjson read "$read_log" '
          # The timestamp goes, and with it the byte order mark the first
          # line of a job log starts with.
          split("\n") | map(sub("^\ufeff?[0-9T:.-]*Z "; "")) as $lines
          | ([$lines | to_entries[] | select(.value | startswith("failed gates:"))
              | .key] | last) as $at
          | {name: $name, run: ($run | tonumber), job: ($job | tonumber),
             current: (if $sha == "" then null else $sha == $head end),
             gates: (if $at == null then []
               else $lines[$at] | ltrimstr("failed gates:") | split(" ")
                 | map(select(length > 0)) end),
             errors: ([$lines[] | select(test(
                 "##\\[error\\]|^error(\\[|:)|^FAIL:|^not ok|✖|panicked at"
                 + "|test result: FAILED|SyntaxError|vulnerabilities found|^Crate: "))
               | sub("^##\\[error\\]"; "")]
               # Duplicates dropped in log order, so the first 20 are the
               # first failures, not the first alphabetically.
               | reduce .[] as $e ([]; if index([$e]) then . else . + [$e] end)
               | .[:20]),
             excerpt: (if $at == null then $lines[-30:]
               else $lines[[$at - 30, 0] | max:$at] end
               | map(select(length > 0)) | join("\n")),
             on_main: (if $main == null then null else $name | IN($main[]) end),
             before_main: ($ran_at != "" and $main_at != ""
               and $ran_at < $main_at)}
          | if $read then . else .gates = null | .errors = null | .excerpt = null end'
    done | jq -s .)

# The check job only collects the lanes and the mutation jobs, so it says
# nothing of its own. A failing check outside Actions (cubic) has no gates to
# read, so a rebase is not known to clear it, and it rules a stale base out.
# Each gate that reads data from outside the tree maps to the files whose change
# would make the failure the PR's own; a gate added here needs nothing else.
printf '%s\n' "$checks" | jq --argjson failing "$failing" --arg head "$head" \
    --argjson pr "$pr" --argjson meta "$meta" --argjson jobs "$main_jobs" \
    "$defs"'
  # The gate itself, the CI that installs it and the cargo config it reads
  # count as well as the lockfile: a PR that changes how the audit runs owns
  # its failure even when main passes.
  {"cargo-audit": "(^|/)Cargo\\.(lock|toml)$|^\\.cargo/|^scripts/test\\.sh$|^\\.github/"}
    as $outside |
  # Gates whose every input sits under these paths: the desktop crate builds on
  # its own (desktop/Cargo.toml, its ui/ frontend) and reads from the root only
  # the toolchain, the lint and format configs, the gate script, CI and the
  # root manifest, whose workspace exclude lets it build alone. A gate
  # missing here reads more than a list can bound, as clippy reads src/, so its
  # failure is never ruled out by the files a PR changes. rustfmt and clippy
  # also read the dot-prefixed names of their configs. A rename is listed under
  # its new path only, so a PR with one is never untouched: the file it moved
  # may have left desktop/.
  ("^desktop/|^vendor/|^\\.cargo/|^Cargo\\.toml$|^(rust-toolchain|\\.?rustfmt|\\.?clippy)\\.toml$|^scripts/test\\.sh$|^\\.github/"
    as $desktop | {"desktop": $desktop, "desktop-fmt": $desktop}) as $confined |
  def untouched: ($meta.cut | not) and ($meta.renamed | not)
    and ((.gates // []) | length > 0)
    and all(.gates[]; $confined[.] != null)
    and all(.gates[]; $confined[.] as $re | all($meta.files[]; test($re) | not));
  [($jobs // [])[] | select(.conclusion == "success") | .name] as $passed |
  [$failing[] | select(.name != "check")] as $lanes |
  ([$lanes[] | (.gates // [])[]] | unique) as $gates |
  [.[] | select(failed and ([job] == []))] as $apps |
{
  pr: $pr, head: $head,
  pending: [.[] | select(.bucket == "pending" and (failed | not)) | .name],
  held: [.[] | select(.state == "ACTION_REQUIRED") | .name],
  failing: ([$failing[] | del(.before_main) | .untouched = untouched]
    + [$apps[] | {name, link}]),
  stale_base: (($meta.cut | not) and ($lanes | length > 0)
    and ($apps | length == 0)
    and ([.[] | select((.bucket == "pending" and (failed | not))
        or .state == "ACTION_REQUIRED")]
      | length == 0)
    and all($lanes[]; .current == true and .before_main
      and (.name | IN($passed[]))
      and (.gates | length > 0) and all(.gates[]; $outside[.] != null))
    and all($gates[]; $outside[.] as $re
      | all($meta.files[]; test($re) | not)))
}'
