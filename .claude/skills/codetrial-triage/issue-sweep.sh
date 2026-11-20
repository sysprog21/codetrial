#!/bin/sh
# Read-only snapshot of the open issues for the triage sweep. Writes nothing to
# GitHub; SKILL.md says what each list asks of the maintainer and what it does
# not prove.
#
# Usage: issue-sweep.sh [repo] [wait-days] (default sysprog21/codetrial, 7)
#
# Prints one JSON object:
#   maintainers_known  false when the collaborators listing failed, leaving
#               only comment badges to tell a maintainer; a COLLABORATOR badge
#               also marks read-only collaborators, so the listing decides
#               whenever it is available
#   unanswered  issues no maintainer other than the author has commented on,
#               oldest first, so a collaborator's own follow-up is not an answer;
#               the viewer's own issues and a deleted account's are left out,
#               since nobody else answers the first and nobody can the second;
#               closing_prs lists open PRs whose merge closes the issue, which
#               makes it the PR sweep's once the PR is read (waiting.md)
#   waiting     issues whose newest maintainer comment is newer than anything
#               the reporter did since, comment or body edit; overdue once it
#               is older than wait-days. Issues a maintainer opened are left
#               out; claimed marks a reporter who also holds the issue, and
#               handed_over an issue held only by other non-maintainers
#               (false while maintainers_known is false): the
#               reporter is no longer who the work waits on, so it is never
#               overdue and its assignee is read under claimed instead;
#               closing_prs is as in unanswered
#   linked      issues a pull request cross-referenced after the issue opened,
#               split into merged and still open; a PR from another
#               repository is left out, since its number means nothing here
#   lint        mechanical gaps per issue: empty_body, placeholder (template
#               text left in), empty_section, title_prefix, not_english (more
#               CJK than Latin letters; other scripts are not detected), and
#               no_revision on a Bug with no hex revision from the reporter;
#               a bare hash must mix digits and letters so a date or a word
#               does not pass, and a pasted --version line always does
#   claimed     per assignee who is not a maintainer: their open_prs and
#               merged_prs that cross-referenced the issue, idle_days since
#               their last act or a later maintainer comment (checked_at),
#               stale past wait-days with no such PR, asked_last when theirs
#               is the newest comment; stale is null when the history is cut,
#               and idle_days too when none of their acts is in what was read
#   truncated   issues with more than 100 comments or cross-references, 10
#               assignees, 20 assignments, 20 body edits or 10 closing PRs,
#               whose lists above may miss what was cut

set -eu

repo=${1:-sysprog21/codetrial}
days=${2:-7}

# shellcheck disable=SC2016
query='query($owner: String!, $name: String!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    issues(states: OPEN, first: 50, after: $endCursor) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number title body createdAt authorAssociation
        author { login } issueType { name }
        userContentEdits(last: 20) {
          totalCount nodes { editedAt editor { login } }
        }
        assignees(first: 10) { totalCount nodes { login } }
        closedByPullRequestsReferences(first: 10, includeClosedPrs: false) {
          totalCount nodes { number repository { nameWithOwner } }
        }
        assigned: timelineItems(last: 20, itemTypes: [ASSIGNED_EVENT]) {
          totalCount
          nodes { ... on AssignedEvent { createdAt
            assignee { ... on User { login } } } }
        }
        comments(last: 100) {
          totalCount
          nodes { author { __typename login } authorAssociation createdAt body }
        }
        timelineItems(last: 100, itemTypes: [CROSS_REFERENCED_EVENT]) {
          totalCount
          nodes { ... on CrossReferencedEvent { createdAt
            source { ... on PullRequest { number state mergedAt
              author { login } repository { nameWithOwner } } } } }
        }
      }
    }
  }
}'

# The viewer, whose own issues are not unanswered. A token that may read the
# repository but not /user, such as an Actions GITHUB_TOKEN, leaves it empty,
# which excludes nobody rather than ending the sweep; gh prints a refusal's body
# to stdout, so a failed call is not read.
me=$(gh api user --jq .login 2> /dev/null) || me=

# Who may push, so a maintainer who assigned themselves without commenting is
# not taken for a contributor. Listing it needs push access; on a refusal gh
# prints the error body to stdout, so only a successful call is read.
if logins=$(gh api --paginate "repos/$repo/collaborators?permission=push" \
    --jq '.[].login' 2> /dev/null); then
    pushers=$(printf '%s\n' "$logins" | jq -Rn '[inputs | select(length > 0)]')
else
    pushers=null
fi

pages=$(gh api graphql --paginate -F owner="${repo%/*}" -F name="${repo#*/}" \
    -f query="$query")

# A maintainer is whoever may push. The COLLABORATOR badge also marks
# collaborators with read or triage access, students among them, so the badge
# counts only when the push listing could not be read. A bot's comment never
# asks the reporter for anything. The reporter answering by editing the body
# counts, since the bug form and maintainers both ask for that; every edit is
# read rather than only the last, which a maintainer tidying the body would
# otherwise hide.
printf '%s\n' "$pages" | jq -s --argjson days "$days" --arg repo "$repo" \
    --argjson pushers "$pushers" --arg me "$me" '
def ts: if . == null then 0 else fromdateiso8601 end;
def days: (now - .) / 86400 | floor;
def age: ts | days;
def staff: if $pushers != null then .author.login | IN($pushers[])
  else .authorAssociation | IN("MEMBER", "OWNER", "COLLABORATOR") end;
def human: .author != null and .author.__typename != "Bot";
# Counted by code point: scan, splits and gsub over a whole body cost time
# quadratic in their matches, seconds for one long report, and the sweep reads
# every open body; split on a plain string does not. A character above ASCII
# is tested on its own, so a class keeps its meaning. ink stops at 40, the
# only threshold it is compared against.
def is(class): [.] | implode | test(class);
def cjk: [explode[] | select(. > 255 and is("[\\p{Han}\\p{Hiragana}\\p{Katakana}\\p{Hangul}]"))]
  | length;
def latin: [explode[] | select((. >= 65 and . <= 90) or (. >= 97 and . <= 122))]
  | length;
def ink: [limit(40; split("\n")[] | select(startswith("#") | not)
  | explode[] | select(if . < 128 then . != 32 and (. < 9 or . > 13)
      else is("\\s") | not end))] | length;
[.[].data.repository.issues.nodes[]] as $issues |
def maint: [.comments.nodes[] | select(human and staff)];
def cut: .comments.totalCount > 100 or .timelineItems.totalCount > 100
  or .assignees.totalCount > 10 or .assigned.totalCount > 20
  or .userContentEdits.totalCount > 20
  or (.closedByPullRequestsReferences.totalCount // 0) > 10;
def prs: [.timelineItems.nodes[] | select(.source.number != null
  and .source.repository.nameWithOwner == $repo)];
# A PR in another repository may close the issue too; as in prs, its number
# means nothing here.
def closing: [.closedByPullRequestsReferences.nodes[]?
  | select(.repository.nameWithOwner == $repo) | .number];
def split: {open: [.[] | select(.state == "OPEN") | .number] | unique,
  merged: [.[] | select(.mergedAt != null) | .number] | unique};
def mine: .author.login as $a | [.comments.nodes[]
  | select($a != null and .author.login == $a) | .createdAt | ts]
  + [.userContentEdits.nodes[] | select($a != null and .editor.login == $a)
      | .editedAt | ts] + [0] | max;
# Per line rather than one regex over the body, since "the same or a higher
# level" is not a backreference. A fence counts as content and what is inside
# it is skipped, so the comments in a pasted shell session are not headings; it
# closes only on its own character, at least as long, with nothing after it.
def empty_section: [foreach (split("\n")[] | select(test("\\S"))) as $l
    ({fence: null, level: null};
     ([$l | capture("^ {0,3}(?<m>`{3,}|~{3,})(?<rest>.*)$")] | first) as $f
     | if .fence == null then
         if $f != null and ($f.m[0:1] == "~" or ($f.rest | contains("`") | not))
         then {fence: $f.m, level: 0}
         else .level = (($l | capture("^(?<h>#{1,6}) +\\S") | .h | length) // 0)
         end
       elif $f != null and $f.m[0:1] == .fence[0:1]
           and ($f.m | length) >= (.fence | length) and ($f.rest | test("^\\s*$"))
       then {fence: null, level: 0}
       else .level = null end;
     .level | select(. != null))] as $lv
  | any(range($lv | length); . as $k | $lv[$k] > 0
      and ($lv[$k + 1] // -1 | . == -1 or (. > 0 and . <= $lv[$k])));
def gaps: (.body // "") as $b
  | ($b + " " + ([.author.login as $a | .comments.nodes[]
      | select($a != null and .author.login == $a) | .body] | join(" "))) as $said
  | [ (select($b | ink < 40) | "empty_body"),
      (select($b | test("\\[([Pp]lease )?[Ff]ill in|<[A-Z][a-z][^<>\\n]{15,}>"))
        | "placeholder"),
      (select($b | empty_section) | "empty_section"),
      (select(.title | test("^\\s*(\\[[^\\]]+\\]|[A-Za-z]+(\\([^)]*\\))?:)\\s"))
        | "title_prefix"),
      (select((.title | cjk > latin) or (($b | cjk) > ($b | latin)))
        | "not_english"),
      (select(.issueType.name == "Bug"
          and ($said | test("codetrial [0-9.]+ \\([0-9a-f]{7,40}\\)|\\b(?=[0-9a-f]*[0-9])(?=[0-9a-f]*[a-f])[0-9a-f]{7,40}\\b"; "i") | not))
        | "no_revision") ];
def claim: . as $i | maint as $m
  | (($pushers // []) + [$m[].author.login]
      + [if staff then .author.login else empty end]) as $staff
  | .assignees.nodes[].login as $a | select($a | IN($staff[]) | not)
  | [$i | prs[] | select(.source.author.login == $a)] as $xrefs
  | ([$i.assigned.nodes[] | select(.assignee.login == $a) | .createdAt]
      + [$i.comments.nodes[] | select(.author.login == $a) | .createdAt]
      + [$xrefs[].createdAt] | map(ts) | max) as $last
  | (if $last == null then null
      else [$m[] | select((.createdAt | ts) > $last) | .createdAt] | max end)
      as $check
  | (if $check then $check | ts else $last end) as $since
  | ([$i.comments.nodes[] | select(human)] | max_by(.createdAt | ts)
      | .author.login == $a) as $asked
  | ($xrefs | map(.source) | split) as $pr
  | ($since | if . then days else null end) as $idle
  | {issue: $i.number, assignee: $a,
     open_prs: $pr.open, merged_prs: $pr.merged, idle_days: $idle,
     checked_at: $check, asked_last: $asked,
     stale: (if $idle == null or ($i | cut) then null
       else $idle >= $days and ($pr.open + $pr.merged | length == 0)
       end)};
[$issues[] | claim] as $claims |
{
  maintainers_known: ($pushers != null),
  unanswered: [$issues[] | select(.author != null) | .author.login as $a
    | select($a != $me and all(maint[]; .author.login == $a))
    | {issue: .number, author: $a, age_days: (.createdAt | age),
       closing_prs: closing}]
    | sort_by(-.age_days),
  waiting: [$issues[] | select(.author != null and (staff | not))
    | .number as $n | .author.login as $a
    | (maint | max_by(.createdAt | ts)) as $ask
    | [$claims[] | select(.issue == $n) | .assignee] as $holders
    | ($pushers != null and ($holders | length > 0 and all(. != $a))) as $handed
    | select($ask != null and ($ask.createdAt | ts) > mine)
    | {issue: .number, author: .author.login, asked_by: $ask.author.login,
       asked_at: $ask.createdAt, idle_days: ($ask.createdAt | age),
       handed_over: $handed,
       overdue: ((($ask.createdAt | age) >= $days) and ($handed | not)),
       claimed: any($claims[]; .issue == $n and .assignee == $a),
       closing_prs: closing}]
    | sort_by(-.idle_days),
  linked: [$issues[] | (.createdAt | ts) as $born
    | [prs[] | select((.createdAt | ts) >= $born) | .source] as $prs
    | select($prs | length > 0)
    | {issue: .number} + ($prs | split)],
  claimed: $claims | sort_by(-(.idle_days // 1e9)),
  lint: [$issues[] | gaps as $g | select($g | length > 0)
    | {issue: .number, title, gaps: $g}],
  truncated: [$issues[] | select(cut) | .number]
}'
