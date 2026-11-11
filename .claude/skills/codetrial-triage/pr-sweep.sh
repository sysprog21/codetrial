#!/bin/sh
# Read-only snapshot of the open pull requests for the triage sweep. Writes
# nothing to GitHub; pulls.md says which of the actions it lists a sweep applies
# on its own and which wait for the user.
#
# Usage: pr-sweep.sh [repo] [stale-days] [previous.json]
#        (default sysprog21/codetrial, 7, none)
#
# Prints one JSON object:
#   maintainers_known  false when the push listing failed and the COLLABORATOR
#             badge, which read-only collaborators also carry, stood in for it
#   approve   runs waiting for approval on the current head of an open PR, and
#             whether its author is a first-time contributor
#   outdated  unresolved review threads whose lines the PR no longer touches,
#             to recheck at the head before resolving
#   overlap   PR pairs that close the same issue
#   stale     PRs, not the viewer's or a deleted account's, whose last visible
#             human review or comment from someone other than the author is
#             newer than anything the author did and older than stale-days,
#             unless the latest verdict is an approval, it is the rebase
#             request on a PR that still conflicts, or the activity was cut
#             off at 100
#   rebase    PRs to send the fixed rebase request, each with its reason:
#             conflicts, or stale_base when its only failures are what
#             ci-failure.sh calls a stale base. Neither is a draft, the
#             viewer's, stacked on another branch, or cut off at 100. A
#             conflict already holding a current changes-requested review
#             that mentions rebase, and a stale base holding a current request
#             of either kind, are left out. PR_SWEEP_CI names the
#             ci-failure.sh to ask, for tests
#   hide      superseded rebase requests in a known wording, to hide
#   hide_ask  other rebase requests, to confirm before hiding
#             (all three: pulls.md, "Conflicting branches")
#   hide_resolved  reviews to hide as resolved: a COMMENTED review, with no
#             body of a person's own, whose every inline comment sits in a
#             thread that is resolved or outdated. Skipped when a thread is
#             too long to read whole
#   refine_stale  the viewer's "Some review comments ... not settled yet"
#             comments (refine.md) to delete: all of them on a PR with no open
#             thread left (why settled), and every one but the newest on a PR
#             that still has open threads (why superseded)
#   unknown   PRs whose mergeability GitHub has still not computed after one
#             retry; rerun
#   status    every open PR, smallest first: lines changed split into source
#             and tests, mergeability, review decision, failing, pending and
#             held checks, the gates lanes that skipped, whether a gates lane
#             passed, open threads by human and bot reviewers, whether its
#             author may push (null when that listing failed), and a grade
#             with its reasons. The grade is
#             the worst row of the table in grade() that the PR matches, from
#             blocked, low, medium and medium-high down to high when none
#             does; draft PRs are set aside. rework is true while a long-series
#             request (hygiene.sh) stands at the head. pulls.md, "Rank by size and
#             merge confidence", says how to read it. waits_on names whose
#             move it is (author, maintainer, ci, approval), the first of
#             waits_on_all, which names everyone with a move left; why names a
#             failing job main's latest run also fails, which ci-failure.sh
#             settles
#   review    the PRs, not the viewer's, whose waits_on is maintainer,
#             smallest first: nothing recorded blocks them, so they wait on a
#             first review, a re-review after a push, or a merge
#   refs_only PRs that close no issue and name one with Refs, which
#             CONTRIBUTING.md asks PRs to avoid; asked says a maintainer
#             already asked for Close #N on that PR
#   viewer    the account that ran the sweep
#   prerequisites  PRs that must land first: stacked on the other's branch,
#             or stated in the body as depending on it
#   collide   PR pairs whose diffs change the same or adjacent base lines of
#             a file, with the hunk's function; collide_unread names the PRs
#             whose diff could not be read
#   superseded  runs awaiting approval on a head no open PR has any more,
#             which need nothing
#   moved     with previous.json, a saved earlier output: the PRs whose head,
#             mergeability, decision, checks, threads, grade or waits_on
#             changed
#   gone      with previous.json: PRs no longer open, with merged or closed
#   prs       number, title, updatedAt, head, author, closing issues and files
#             of every open PR, for judging overlap that shares no issue and
#             for checking a PR again before writing to it

set -eu

repo=${1:-sysprog21/codetrial}
days=${2:-7}
prev=${3:-}
previous=null

# Slurped, so an empty file fails too; jq 1.6 -e alone passes it. Only an array
# is a status list: moved and gone below iterate it.
if [ -n "$prev" ]; then
    previous=$(jq -sce 'select(length == 1) | .[0].status | arrays' "$prev" 2> /dev/null) || {
        echo "pr-sweep.sh: $prev is not an earlier pr-sweep.sh output" >&2
        exit 2
    }
fi

# No fallback when /user cannot be read, unlike issue-sweep.sh: an unknown
# viewer would put the viewer's own pull requests in rebase, a list a sweep
# posts to without asking.
me=$(gh api user --jq .login)

# Who may push, which is who may ask for a rebase. The COLLABORATOR badge also
# marks read-only collaborators, so it stands in only when this listing, which
# needs push access, fails; gh prints a refusal's body to stdout, so only a
# successful call is read.
if logins=$(gh api --paginate "repos/$repo/collaborators?permission=push" \
    --jq '.[].login' 2> /dev/null); then
    pushers=$(printf '%s\n' "$logins" | jq -Rn '[inputs | select(length > 0)]')
else
    pushers=null
fi
text=$(dirname "$0")/rebase.md

# Shorter wordings maintainers type by hand for the request rebase.md makes;
# matched, like rebase.md, after trimming and dropping a final period.
# shellcheck disable=SC2016
known='["Rebase latest `main` and resolve conflicts",
  "Rebase latest `main` branch",
  "Rebase latest `main` branch and resolve conflicts",
  "Rebase onto latest `main` branch",
  "Rebase onto latest `main` branch and resolve conflicts",
  "Rebase onto latest main branch and resolve conflicts",
  "Before asking someone to review, you MUST resolve any regressions flagged by GitHub Actions"]'

# A thread reply is a review by its writer, so author replies show up in
# reviews. A review records the head it was left on, so a head that moved since
# says exactly that someone pushed after it. pushedDate is always null now, so
# after a plain comment the push is dated by GitHub's clock: the head commit's
# first check suite, created when the push reached a workflow or app, or the
# latest force push. The commit date, which the contributor's clock sets and
# which can run ahead and make every later request look stale, is used only for
# a head with no check suite. A PR force-pushed back to its base has no commit
# at all, and dates only its force push.
# shellcheck disable=SC2016
query='query($owner: String!, $name: String!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    defaultBranchRef { name }
    pullRequests(states: OPEN, first: 50, after: $endCursor) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number title body isDraft authorAssociation headRefOid mergeable
        baseRefName updatedAt headRefName headRepository { nameWithOwner }
        additions deletions reviewDecision
        author { login }
        closingIssuesReferences(first: 100) { totalCount nodes { number } }
        files(first: 100) { totalCount nodes { path additions deletions } }
        commits(last: 1) {
          nodes { commit { committedDate
            checkSuites(first: 10) { nodes { createdAt } }
            statusCheckRollup { contexts(first: 50) { totalCount nodes {
              __typename
              ... on CheckRun { databaseId name status conclusion }
              ... on StatusContext { context state } } } } } }
        }
        reviews(last: 100) {
          totalCount
          nodes {
            id author { __typename login } authorAssociation state
            submittedAt isMinimized body
            commit { oid }
            comments { totalCount }
          }
        }
        comments(last: 100) {
          totalCount
          nodes {
            id author { __typename login } authorAssociation createdAt
            isMinimized body
          }
        }
        reviewThreads(first: 100) {
          totalCount
          nodes { id isResolved isOutdated path
            comments(first: 20) { totalCount
              nodes { author { __typename login } pullRequestReview { id } } } }
        }
        timelineItems(last: 1, itemTypes: [HEAD_REF_FORCE_PUSHED_EVENT]) {
          nodes { ... on HeadRefForcePushedEvent { createdAt } }
        }
      }
    }
  }
}'

pages=$(gh api graphql --paginate -F owner="${repo%/*}" -F name="${repo#*/}" \
    -f query="$query")
prs=$(printf '%s\n' "$pages" | jq -s '[.[].data.repository.pullRequests.nodes[]]')
base=$(printf '%s\n' "$pages" | jq -rs '.[0].data.repository.defaultBranchRef.name')

# GitHub computes mergeability on first request, and every merge to main resets
# it, so one read often answers UNKNOWN for most PRs. The first read starts the
# computation; a few seconds later one small query asks again for just those
# PRs, rather than refetching everything above.
unknown=$(printf '%s\n' "$prs" | jq -r '.[] | select(.mergeable == "UNKNOWN") | .number')
if [ -n "$unknown" ]; then
    sleep "${PR_SWEEP_RETRY_DELAY:-5}"
    asks=$(printf '%s\n' "$unknown" | sed 's/.*/p&: pullRequest(number: &) { number mergeable }/')
    again=$(gh api graphql -F owner="${repo%/*}" -F name="${repo#*/}" -f query="
      query(\$owner: String!, \$name: String!) {
        repository(owner: \$owner, name: \$name) { $asks } }" \
        --jq '[.data.repository[] | select(. != null)]') || again='[]'
    prs=$(printf '%s\n' "$prs" | jq --argjson again "$again" \
        '($again | INDEX(.number)) as $a
         | map(.mergeable = ($a[.number | tostring].mergeable // .mergeable))')
fi

# Fork runs carry an empty pull_requests array, so a run is tied to its PR by
# head repository, branch and SHA; the SHA alone would let two forks' PRs at one
# commit share a run and its first_time. A pending run on an older SHA is
# superseded and needs nothing. The listing is captured before jq reads it,
# since in a pipe set -e sees only jq, and a failed listing would read as no
# held runs at all.
runs=$(gh api --paginate \
    "repos/$repo/actions/runs?status=action_required&event=pull_request&per_page=100" \
    --jq '.workflow_runs[] | {id, name, head_sha, head_branch,
      head_repo: .head_repository.full_name, actor: .actor.login}')
runs=$(printf '%s\n' "$runs" | jq -s .)

# The jobs main's latest finished run failed, read once the way ci-failure.sh
# reads them, so a PR's reason can say main fails the same job. Null when the
# lookup fails, which names nothing.
main_failed=null
if main_run=$(gh run list --repo "$repo" --workflow check.yml --branch main \
    --event push --status completed --limit 10 --json databaseId,conclusion \
    --jq '[.[] | select(.conclusion != "cancelled")][0].databaseId // empty' \
    2> /dev/null) && [ -n "$main_run" ]; then
    main_failed=$(gh run view "$main_run" --repo "$repo" --json jobs \
        --jq '[.jobs[] | select(.conclusion
            | IN("failure", "cancelled", "timed_out", "startup_failure"))
            | .name]' 2> /dev/null) || main_failed=null
fi

# What counts as a rebase request, when it is current, and which copies are
# hidden or only offered for hiding: pulls.md, "Conflicting branches".
out=$(printf '%s\n%s\n' "$prs" "$runs" | jq -s --argjson days "$days" --arg me "$me" \
    --arg base "$base" --rawfile text "$text" --argjson known "$known" \
    --argjson pushers "$pushers" --argjson previous "$previous" \
    --argjson main_failed "$main_failed" --arg repo "$repo" \
    --arg refine "$(cat "$(dirname "$0")/refine.md")" '
def human: .author != null and .author.__typename != "Bot";
def ts: if . == null then 0 else fromdateiso8601 end;
def pushed: (.commits.nodes[0].commit // {checkSuites: {nodes: []}}) as $c
  | [([$c.checkSuites.nodes[].createdAt | ts] | min) // ($c.committedDate | ts),
     (.timelineItems.nodes[0].createdAt | ts)] | max;
def trim: gsub("^\\s+|\\s+$"; "");
def norm: trim | sub("\\.$"; "");
def whole: .reviews.totalCount <= 100 and .comments.totalCount <= 100;
def nudgeable: .author != null and .author.login != $me;
def staff: if $pushers != null then .author.login | IN($pushers[])
  else .authorAssociation | IN("OWNER", "MEMBER", "COLLABORATOR") end;
.[0] as $prs | .[1] as $runs | ([$text | norm] + $known) as $wordings |
def owns($r): $r.head_sha == .headRefOid and $r.head_branch == .headRefName
  and $r.head_repo == .headRepository.nameWithOwner;
def runs_of($p): $runs[] | select(. as $r | $p | owns($r));
def opens_with_rebase: test("^\\s*(@[\\w-]+[,:]?\\s+)*rebase\\b"; "i");
# The CI regression request this sweep and hygiene.sh used before rebase.md
# answered it; an earlier copy counts as the same request.
def old_ci_request: test("^\\s*Before asking someone to review, you MUST resolve any regressions");
def requests: .author.login as $a | .headRefOid as $head | pushed as $p |
  [(.reviews.nodes[]
     | select(.state == "CHANGES_REQUESTED"
         and (.body | test("\\brebase"; "i") or old_ci_request))
     | {at: .submittedAt, inline: .comments.totalCount, review: true,
        current: (.commit.oid == $head)} + .),
   (.comments.nodes[]
     | select(.body | opens_with_rebase or old_ci_request)
     | {at: .createdAt, inline: 0, review: false,
        current: ((.createdAt | ts) >= $p)} + .)]
  | map(select(human and staff and .author.login != $a and (.isMinimized | not))
      | {id, by: .author.login, at, body, inline, review, current});
def fixed_only: .inline == 0 and (.body | norm | IN($wordings[]));
def asks_rebase: [requests[] | select(fixed_only)] | sort_by(.at | ts);
# A dismissed review is no request any more, but it stays on the page: the fixed
# wordings left behind after a dismissal are outdated by definition.
def dismissed: [.reviews.nodes[]
  | select(.state == "DISMISSED" and human and staff and (.isMinimized | not)
      and .comments.totalCount == 0 and (.body | norm | IN($wordings[])))
  | {id, by: .author.login, at: .submittedAt, kind: "review", why: "outdated",
     act: "hide"}];
def asked_here: any(requests[]; .current);
# A conflict is asked about with a Request changes review, so a current
# comment alone, whatever its wording, does not hold a conflicting PR back from
# getting one; a fixed-wording comment is then hidden as an earlier copy.
def reviewed_here: any(requests[]; .current and .review);
# A PR whose only request on its current head is hand-written: that request is
# the standing one, so no fixed copy is kept beside it and it is never offered
# for hiding. A conflicting PR whose hand-written request is only a comment
# still gets the review (needs_rebase), and the comment goes to hide_ask.
def hand_asked: asked_here and all(asks_rebase[]; .current | not);
# Who the fixed text may go to at all: it names main, so a stacked PR is left
# to its reviewer; a draft is still being written; and a PR whose history was
# cut may hold a request past what was read.
def rebase_eligible: (.isDraft | not) and .baseRefName == $base
  and $base == "main" and nudgeable and whole;
def needs_rebase: .mergeable == "CONFLICTING" and rebase_eligible
  and (reviewed_here | not);
# A test is anything under a tests/ directory, the browser check, or a file
# named as one (test_x.py, x.test.js); a substring would also catch problem
# files such as tightest-channel-spacing.json. With more than 100 files the
# split undercounts tests.
def size: ((.additions // 0) + (.deletions // 0)) as $total
  | ([.files.nodes[] | select(.path | test(
        "(^|/)tests/|browser-check|(^|/)test_[^/]*$|\\.test\\.[a-z]+$"))
      | (.additions // 0) + (.deletions // 0)] | add // 0) as $tests
  | {total: $total, source: ($total - $tests), tests: $tests};
# ACTION_REQUIRED is a run held for a first-time contributor, not a failure.
# SKIPPED and NEUTRAL are neither a pass nor a failure: the mutation shards
# skip on a diff with no Rust in it, but a gates lane that skipped tested
# nothing, so only a gates lane that passed says the gates ran.
# A held run often has no check run at all, only its entry in $runs, so a
# match there counts as held too. A rerun adds a check run of the same name
# beside the old one, so only the latest of each name counts, or a failure
# rerun green would still read as failing.
def checks: [runs_of(.) | .name] as $waiting
  | ((.commits.nodes[0].commit.statusCheckRollup.contexts.totalCount // 0)
      > 50) as $cut
  | [.commits.nodes[0].commit.statusCheckRollup.contexts.nodes[]?
    | if .__typename == "CheckRun" then
        {name, s: (if .status != "COMPLETED" then "pending"
          elif .conclusion == "ACTION_REQUIRED" then "held"
          elif .conclusion | IN("FAILURE", "TIMED_OUT", "CANCELLED",
            "STARTUP_FAILURE") then "failing"
          elif .conclusion | IN("SKIPPED", "NEUTRAL") then "skipped"
          else "ok" end), kind: "run", id: .databaseId}
      else
        {name: .context, s: (if .state | IN("PENDING", "EXPECTED") then "pending"
          elif .state | IN("FAILURE", "ERROR") then "failing" else "ok" end),
         kind: "status"}
      end]
  # A rerun leaves the old run beside the new one under the same name, so the
  # latest run of each name stands; a commit status keeps one entry per name
  # anyway, and is never merged with a check run that happens to share it.
  | group_by([.kind, .name]) | map(max_by(.id))
  | [.[] | select(.name | startswith("gates ("))] as $lanes
  | def named($s): [.[] | select(.s == $s) | .name];
  {failing: named("failing"), pending: named("pending"),
     held: (named("held") + $waiting | unique),
     skipped: [$lanes[] | select(.s == "skipped") | .name],
     gates: any($lanes[]; .s == "ok"),
     truncated: $cut};
# A thread the PR author opened is a note to reviewers, not a finding left
# for the author, so only threads reviewers opened count.
def threads: .author.login as $a
  | [.reviewThreads.nodes[] | select(.isResolved | not)
    | .comments.nodes[0].author // {__typename: "Ghost", login: "ghost"}
    | select(.login != $a)]
  | {human: [.[] | select(.__typename != "Bot") | .login],
     bot: [.[] | select(.__typename == "Bot") | .login]}
  | .by = (.human | unique);
# The visible changes requests of each maintainer since their last approval,
# since one approval does not answer a changes request from another maintainer,
# and a later rebase request from the same account does not withdraw the
# substantive one before it. A request on the current head waits on
# the author; one the author pushed past waits on a re-review. A hidden review
# still counts in reviewDecision, which is what says a re-review is owed. A
# request is a rebase only when it opens with the word, as a rebase comment is
# read above, so "no rebase is needed" is not one, and only when it does not ask
# for functionally minimal commits (the long-series request of hygiene.sh opens
# with the word too, but the author has to rework the series). requests reads a
# review that mentions rebase anywhere as asking for one, which only keeps the
# sweep from asking twice; here the mistake would hide a substantive request.
def verdicts: [.reviews.nodes[]
    | select(human and staff and (.isMinimized | not)
      and (.state == "APPROVED" or .state == "CHANGES_REQUESTED"))]
  | group_by(.author.login);
def open_requests: .headRefOid as $head
  | verdicts
  | map(sort_by(.submittedAt | ts)
      | (map(.state == "APPROVED") | rindex(true)) as $ok
      | if $ok == null then .[] else .[$ok + 1:][] end)
  | [.[] | select(.state == "CHANGES_REQUESTED" and .commit.oid == $head)
    | (.body | test("functionally minimal"; "i")) as $series
    | {series: $series, rebase: ((.body | opens_with_rebase) and ($series | not))}];
# An approval is of the code it was left on. GitHub keeps it after a push
# unless the branch protection dismisses stale reviews, so reviewDecision can
# read APPROVED for a head nobody approved: true when every maintainer whose
# latest verdict is an approval left it on an older head.
def approval_stale: .headRefOid as $head
  | [verdicts[] | max_by(.submittedAt | ts) | select(.state == "APPROVED")]
  | length > 0 and all(.[]; .commit.oid != $head);
# One row per condition: when it holds, the reason it adds, the grade it caps
# the PR at, and whose move it makes it. The grade is the worst row that holds,
# so adding a problem never raises it, and the reasons and the grade cannot
# disagree. A row with no reason only strengthens one that already gave its
# own. waits_on is read off the same rows, so a new row cannot leave a PR in
# the review queue while grading it down: a held run outranks a pending check,
# which outranks anything the author must fix, and a PR no row hands to anyone
# waits on the maintainer. A failure main shares still reads as one for the author
# here, since only ci-failure.sh can tell it apart.
def grade($c; $t): .isDraft as $draft | open_requests as $r
  | (.reviewDecision == "CHANGES_REQUESTED") as $cr
  | ($r | length > 0) as $cr_head
  | any($r[]; .rebase | not) as $cr_work
  | ($t.human | length > 0) as $talk
  | [{when: (.mergeable == "CONFLICTING"), why: "conflicts", grade: "medium",
      who: "author"},
     {when: $cr_work, why: "changes requested on this head", grade: "blocked",
      who: "author"},
     {when: ($cr_head and ($cr_work | not)),
      why: "changes requested on this head (rebase)", grade: "medium",
      who: "author"},
     {when: ($cr and ($cr_head | not)),
      why: "changes requested before the last push, or hidden",
      grade: "medium-high"},
     {when: approval_stale, why: "approved before the last push",
      grade: "medium-high", who: "maintainer"},
     {when: $talk, grade: "low", who: "author",
      why: "\($t.human | length) open thread(s) by \($t.by | join(", "))"},
     {when: ($talk and (.mergeable == "CONFLICTING" or $cr_head)),
      why: null, grade: "blocked"},
     {when: ($t.bot | length > 0), grade: "medium", who: "author",
      why: "\($t.bot | length) open bot thread(s)"},
     # A job main also fails may still fail here for a reason of the PR,
     # since a lane runs several gates, so the author keeps it and the reason
     # says main shares the name, for ci-failure.sh to settle.
     {when: ($c.failing | length > 0), grade: "low", who: "author",
      why: ("failing: \($c.failing | join(", "))"
        + ([$c.failing[] | select(IN(($main_failed // [])[]))]
          | if length > 0 then "; main also fails \(join(", "))" else "" end))},
     {when: ($c.held | length > 0), why: "held for approval", grade: "medium",
      who: "approval"},
     {when: ($c.pending | length > 0), grade: "medium-high", who: "ci",
      why: "pending: \($c.pending | join(", "))"},
     # A lane still running, held or failing has its own row above, which
     # says whose move it is; only a head no lane ran on waits on the author.
     {when: (($c.gates or any(($c.pending + $c.held + $c.failing)[];
         startswith("gates ("))) | not), grade: "medium", who: "author",
      why: ("no gates lane passed" + if $c.skipped == [] then ""
        else " (skipped: \($c.skipped | join(", ")))" end)},
     {when: $c.truncated, why: "more than 50 checks, not all read",
      grade: "medium"},
     # GitHub is still computing it, which the sweep retries; nobody should
     # review a PR that may not merge.
     {when: (.mergeable == "UNKNOWN"), why: "mergeability unknown",
      grade: "medium", who: "ci"},
     # Past 100 reviews or threads the newest may be the ones not read, so
     # nothing above can be trusted to have seen them.
     {when: (.reviews.totalCount > 100 or .reviewThreads.totalCount > 100),
      why: "more than 100 reviews or threads, not all read", grade: "medium"}]
  | map(select(.when)) as $hit
  | ["high", "medium-high", "medium", "low", "blocked"] as $ladder
  | ["maintainer", "author", "ci", "approval"] as $turn
  | {grade: (if $draft then "draft"
      else $ladder[[$hit[].grade as $g | $ladder | index($g)] | max // 0] end),
     why: [$hit[].why | strings],
     rework: any($r[]; .series),
     waits_on_all: (if $draft then ["author"]
       else [$turn | reverse[] | select(IN($hit[].who))]
         | if length == 0 then ["maintainer"] else . end end)}
  | .waits_on = .waits_on_all[0];
def status: checks as $c | threads as $t
  | {pr: .number, title, author: .author.login,
     author_is_maintainer: (if $pushers == null then null else staff end),
     head: .headRefOid,
     size: size, files_truncated: (.files.totalCount > 100),
     mergeable, decision: .reviewDecision, checks: $c,
     threads: {human: ($t.human | length), bot: ($t.bot | length), by: $t.by},
     closes: [.closingIssuesReferences.nodes[].number]} + grade($c; $t);

# An issue named as #N, owner/repo#N or its URL, the forms related.sh in
# codetrial-contribute reads; issue_ref is the regex after a keyword, and
# captures the number. The dot a repository name may hold is escaped.
def repo_re: $repo | gsub("\\."; "\\.");
def ref_prefix($kind): repo_re as $r
  | "(?:#|\($r)#|https://github\\.com/\($r)/\($kind)/)";
def ref_prefix: ref_prefix("issues");
def issue_ref: ref_prefix + "([0-9]+)\\b";

# A pull request that has to land first: one whose head branch in this
# repository is the branch this one is based on, or one the body names as
# what it depends on or builds on. A bare "after #N" also dates a symptom
# ("broke after #412"), so it is not read as one. Words are the only record of
# the second kind, so a PR that waits without saying so is not found.
def prerequisites:
  ("(?i)\\b(?:depends on|builds? on|based on|on top of|stacked on)\\s+"
     + "(?:PR\\s+)?" + ref_prefix("pull") + "([0-9]+)\\b") as $says
  | [$prs[] as $p
    | [$p.body // "" | scan($says) | .[0] | tonumber] as $named
    | $prs[] as $q | select($p.number != $q.number)
    | ($p.baseRefName == $q.headRefName
        and $q.headRepository.nameWithOwner == $repo) as $stacked
    | ($q.number | IN($named[])) as $stated
    | select($stacked or $stated)
    | {pr: $p.number, on: $q.number,
       how: (if $stacked then "stacked" else "stated" end)}];

# Issues the body names with Refs, which the merge leaves open, including each
# of a list such as "Refs #1, #2 and #3".
def refs: (ref_prefix + "[0-9]+\\b") as $one
  | [.body // ""
    | scan("(?i)\\brefs?:?\\s+(\($one)(?:[ \\t]*,?[ \\t]*(?:and\\s+)?\($one))*)")
    | .[0] | scan(issue_ref) | .[0] | tonumber]
  | unique;
([$prs[] | status] | sort_by(.size.total)) as $status |
{
  approve: [$prs[] as $p | runs_of($p)
    | {pr: $p.number, run: .id, workflow: .name, actor,
       touches_workflows: any($p.files.nodes[].path; startswith(".github/")),
       files_truncated: ($p.files.totalCount > 100),
       first_time: ($p.authorAssociation
         | . == "FIRST_TIME_CONTRIBUTOR" or . == "FIRST_TIMER")}],
  outdated: [$prs[] | .number as $n | .reviewThreads.nodes[]
    | select(.isOutdated and (.isResolved | not))
    | {pr: $n, thread: .id, path}],
  overlap: [$prs[] as $a | $prs[] as $b | select($a.number < $b.number)
    | [$a.closingIssuesReferences.nodes[].number
       | select(IN($b.closingIssuesReferences.nodes[].number))] as $shared
    | select($shared | length > 0)
    | {a: $a.number, b: $b.number, issues: $shared}],
  stale: [$prs[] | select(nudgeable and whole)
    | .author.login as $author
    | ([.reviews.nodes[], .comments.nodes[]]
       | map(select(human and (.isMinimized | not)))) as $acts
    | ([$acts[] | select(.author.login != $author
          and (.state == "APPROVED" or .state == "CHANGES_REQUESTED"))]
       | max_by(.submittedAt | ts).state) as $verdict
    | ([$acts[] | select(.author.login != $author)
        | {id, by: .author.login, at: (.submittedAt // .createdAt), state,
           commit: .commit.oid}]
       | max_by(.at | ts)) as $review
    | ([$acts[] | select(.author.login == $author)
        | .submittedAt // .createdAt | ts] | max // 0) as $mine
    | ($review.at | ts) as $at
    | (if $review.commit then $review.commit != .headRefOid
       else pushed > $at end) as $pushed_since
    | select($review != null and $verdict != "APPROVED"
        and ((.mergeable == "CONFLICTING"
          and any(requests[]; .id == $review.id)) | not)
        and $at > $mine and ($pushed_since | not)
        and now - $at > $days * 86400)
    | {pr: .number, author: $author, review_by: $review.by,
       review_at: $review.at, idle_days: ((now - $at) / 86400 | floor)}],
  rebase: [$prs[] | select(needs_rebase)
    | {pr: .number, author: .author.login, reason: "conflicts"}],

  # Merging cleanly, failing for its author, and otherwise as needs_rebase:
  # ci-failure.sh decides below whether that failure is a stale base.
  stale_candidates: [$prs[] | .number as $n
    | select(.mergeable == "MERGEABLE" and rebase_eligible
        and (asked_here | not)
        and any($status[]; .pr == $n and .waits_on == "author"
          and (.checks.failing | length > 0)))
    | {pr: .number, author: .author.login, reason: "stale_base",
       head: .headRefOid}],
  review: [$status[] | select(.waits_on == "maintainer" and .author != $me)
    | .pr],
  refs_only: [$prs[] | . as $p
    | select(.closingIssuesReferences.totalCount == 0)
    | refs[] as $n
    | {pr: $p.number, issue: $n,
       asked: any($p.reviews.nodes[], $p.comments.nodes[];
         human and staff and (.isMinimized | not)
         and (.body // "" | [scan("(?i)\\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?):?\\s+" + issue_ref)
           | .[0] | tonumber] | any(.[]; . == $n)))}],
  hide: ([$prs[] | .number as $n | select(whole)
    | asks_rebase as $r
    # The copy kept is the one that holds the PR out of rebase: on a conflict
    # that is the newest current review, which a later comment must not hide.
    # A PR that merges keeps only a request still current at its head; the rest
    # are outdated. A conflict nobody can ask about yet (a draft, a stack) keeps
    # the newest, as it has nobody else to ask.
    | ($r | map(.current and .review) | rindex(true)
        // ($r | map(.current) | rindex(true))) as $keepc
    | (needs_rebase or hand_asked) as $replaced
    | (if $replaced then null
       elif .mergeable == "MERGEABLE" then $keepc
       else $keepc // (($r | length) - 1) end) as $at
    | ($r | to_entries[] | select(.key != $at)) as $e
    # A request a newer copy (or the one about to be posted) replaces is
    # repeated; one nothing replaces is outdated. A comment the viewer wrote
    # that is repeated can be deleted, a review never can.
    | $e.value
    | (if $replaced or ($at != null and (.at | ts) < ($r[$at].at | ts))
       then "repeated" else "outdated" end) as $why
    | {pr: $n, id, at, by, kind: (if .review then "review" else "comment" end),
       why: $why,
       act: (if $why == "repeated" and (.review | not) and .by == $me
             then "delete" else "hide" end)}]
    + [$prs[] | .number as $n | select(whole) | dismissed[] | {pr: $n} + .]),
  hide_ask: [$prs[] | .number as $n
    | select(whole and (needs_rebase
        or (asks_rebase | length > 0) and (hand_asked | not)))
    | requests[] | select(fixed_only | not)
    | {pr: $n, id, at, by, body}],
  hide_resolved: [$prs[] | .number as $n
    | select(whole and .reviewThreads.totalCount <= 100
        and all(.reviewThreads.nodes[]; .comments.totalCount <= 20))
    | ([.reviewThreads.nodes[] | . as $t | .comments.nodes[]
        | select(.pullRequestReview != null)
        | {r: .pullRequestReview.id, settled: ($t.isResolved or $t.isOutdated)}]
       | group_by(.r)
       | map({key: .[0].r, value: {n: length, ok: all(.[]; .settled)}})
       | from_entries) as $by
    | .reviews.nodes[]
    | select((.isMinimized | not) and .state == "COMMENTED"
        and .comments.totalCount > 0 and $by[.id] != null
        and $by[.id].ok and $by[.id].n == .comments.totalCount
        and (.body == "" or .author.__typename == "Bot"))
    | {pr: $n, id, by: .author.login}],
  refine_stale: [$prs[] | .number as $n
    | select(whole and .reviewThreads.totalCount <= 100)
    | ([.comments.nodes[]
        | select(.author.login == $me and (.body | startswith($refine)))]
       | sort_by([.createdAt, .id])) as $mine
    | (all(.reviewThreads.nodes[]; .isResolved)) as $settled
    | (if $settled then $mine else $mine[:-1] end)[]
    | {pr: $n, id, why: (if $settled then "settled" else "superseded" end)}],
  maintainers_known: ($pushers != null),
  viewer: $me,
  prerequisites: prerequisites,
  unknown: [$prs[] | select(.mergeable == "UNKNOWN") | .number],
  status: $status,
  superseded: ([$runs[] | . as $r | select(any($prs[]; owns($r)) | not)]
    | length),
  moved: (if $previous == null then null else
    ($previous | INDEX(.pr)) as $old
    | [$status[] | . as $now | $old[.pr | tostring] as $was
      | if $was == null then {pr, new: true}
        else {pr, changed: ([
            (("head", "mergeable", "decision", "grade", "waits_on", "checks",
              "threads")
              as $k
              | select($was[$k] != $now[$k]) | {($k): [$was[$k], $now[$k]]})]
          | add)}
          | select(.changed != null) end]
    end),
  gone: (if $previous == null then null
    else [$previous[].pr] - [$status[].pr] end),
  prs: [$prs[] | {number, title, updatedAt, head: .headRefOid,
    author: .author.login, isDraft, authorAssociation,
    closes: [.closingIssuesReferences.nodes[].number],
    closes_truncated: (.closingIssuesReferences.totalCount > 100),
    files: [.files.nodes[].path], files_truncated: (.files.totalCount > 100),
    reviews_truncated: (.reviews.totalCount > 100),
    comments_truncated: (.comments.totalCount > 100),
    threads_truncated: (.reviewThreads.totalCount > 100)}]
}')

# Two PRs that change the same base lines of a file conflict as soon as one
# merges, and a shared file alone is too weak a sign: web/interview.js is in
# most UI PRs. So the diff of each PR sharing a path with another is read,
# started here so the lookups below overlap it and capped so GitHub does not
# throttle the burst, and each run of added or removed lines on a shared path
# becomes the base lines it replaces: an insertion, the two around it; a file
# with no hunk (binary, renamed, mode only), all of it. Paths are the ones the
# PR leaves, so a rename meets an edit of its old path only when both PRs also
# share another file. pulls.md says how to read a collision.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# shellcheck disable=SC2016
shared='[.prs[] | .number as $n | .files[] | {n: $n, path: .}]
    | group_by(.path) | map(select(length > 1))'
printf '%s\n' "$out" | jq -r "$shared | .[][0].path" > "$work/paths"
printf '%s\n' "$out" | jq -r "$shared | [.[][].n] | unique[]" > "$work/prs"
# shellcheck disable=SC2016
xargs -P 6 -I{} sh -c 'gh pr diff "$1" --repo "$2" > "$3/$1.diff" 2> /dev/null ||
    rm -f "$3/$1.diff"' _ {} "$repo" "$work" < "$work/prs" &
fetching=$!

# A PR that left the open list either merged or closed; one call each says
# which, and gone stays short. A lookup that fails, from a network blip or a
# previous output saved for another repository, marks that PR UNKNOWN rather
# than losing the sweep.
gone=$(printf '%s\n' "$out" | jq -r '.gone // [] | .[]')
states=$(for n in $gone; do
    gh pr view "$n" --repo "$repo" --json number,state,mergedAt,mergeCommit \
        --jq '{pr: .number, state, mergedAt, merge: .mergeCommit.oid}' \
        || jq -n --argjson n "$n" '{pr: $n, state: "UNKNOWN"}'
done | jq -s .)

# One ci-failure.sh run per candidate, a handful at most: it reads job logs,
# which the sweep does not. A run that fails leaves its PR out, since this list
# posts without asking.
ci=${PR_SWEEP_CI:-$(dirname "$0")/../codetrial-verify/ci-cached.sh}

# A diagnosis of another head, after a push mid-sweep, says nothing about the
# head this sweep read, so it is dropped rather than trusted.
stale=$(printf '%s\n' "$out" | jq -r '.stale_candidates[] | "\(.pr) \(.head)"' \
    | while read -r n head; do
        if found=$(sh "$ci" "$n" "$repo" < /dev/null 2> /dev/null); then
            printf '%s\n' "$found" | jq -c --argjson n "$n" --arg head "$head" \
                'select(.pr == $n and .head == $head and .stale_base == true)
                  | .pr' || true
        fi
    done | jq -s .)

wait "$fetching" || true
unread=$(while read -r n; do [ -f "$work/$n.diff" ] || echo "$n"; done \
    < "$work/prs" | jq -s .)
collide=$(find "$work" -name '*.diff' -exec awk \
    "$(cat "$(dirname "$0")/../codetrial-review/diff-path.awk")"'
    function flush() {
        if (first != "" && path in shared)
            print pr "\t" path "\t" first "\t" last "\t" ctx
        first = ""
    }
    # A section with no hunk changes the file as a whole.
    function end_section() {
        flush()
        if (section && !hunks && path in shared)
            print pr "\t" path "\t0\t2147483647\t"
        section = 0
    }
    # The shared paths come first, then one diff per file, named for its PR.
    FILENAME == paths { shared[$0] = 1; next }
    FNR == 1 {
        end_section()
        pr = FILENAME; sub(/^.*\//, "", pr); sub(/\.diff$/, "", pr)
    }
    /^diff --git / {
        end_section()
        section = 1; header = 1; hunks = 0; path = git_path($0)
        next
    }
    header && /^rename to / { path = substr($0, 11); next }
    header && /^(---|\+\+\+) / {
        if ((p = header_path($0)) != "/dev/null") path = p
        next
    }
    /^@@ / {
        flush(); header = 0; hunks = 1
        split(substr($2, 2), r, ",")
        old = r[1] + 0
        ctx = $0; sub(/^@@ [^@]* @@ ?/, "", ctx)
        next
    }
    header { next }
    /^-/ { if (first == "") first = old; last = old; old++; next }
    # A new file has no base lines; 0 stands for the start of one.
    /^\+/ { if (first == "") { first = old > 0 ? old - 1 : 0; last = old }; next }
    /^\\/ { next }
    { flush(); old++ }
    END { end_section() }
' paths="$work/paths" "$work/paths" {} + | jq -Rn '[inputs | split("\t")
    | {pr: (.[0] | tonumber), path: .[1], first: (.[2] | tonumber),
       last: (.[3] | tonumber), context: (.[4:] | join("\t"))}]
    | [group_by(.path)[] | sort_by(.first) | . as $on
      | range(length) as $i | $on[$i] as $a | $on[$i + 1:][]
      | select(.first <= $a.last + 1 and .pr != $a.pr)
      | [$a, .] | sort_by(.pr) | {a: .[0].pr, b: .[1].pr, path: .[0].path,
         lines: map([.first, .last]),
         context: (map(.context | select(length > 0)) | unique)}]
    | unique_by([.a, .b, .path])')

printf '%s\n' "$out" | jq --argjson states "$states" --argjson stale "$stale" \
    --argjson collide "$collide" --argjson unread "$unread" \
    '.collide = $collide | .collide_unread = $unread
     | if .gone == null then . else .gone = $states end
     | .rebase += [.stale_candidates[] | select(.pr | IN($stale[]))
         | del(.head)]
     | del(.stale_candidates)'
