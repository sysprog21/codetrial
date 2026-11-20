---
name: codetrial-odds
description: >
  List every open CodeTrial pull request with its merge confidence, size and
  who it waits on, and what moved since the last run. Read-only.
disable-model-invocation: true
---

# How close each open pull request is to merging

A shortcut for one question the triage skill already answers: "which open PRs
could merge, and how sure is that". It runs only when typed as
`/codetrial-odds`, so it never competes with
[codetrial-triage](../codetrial-triage/SKILL.md) for a request described in
words; the grading rules stay in one place, [Rank by size and merge
confidence](../codetrial-triage/pulls.md#rank-by-size-and-merge-confidence),
and this file only says how to present them. It posts, approves and resolves
nothing.

## Take the snapshot

[snapshot.sh](snapshot.sh) runs the sweep against the last snapshot, which
fills `moved` and `gone`, and replaces it only with a whole sweep. When it
fails, run it again up to three times with a pause between: a GitHub 504 or
`unexpected end of JSON input` usually passes on the next try. If it still
fails, stop and say so: a report from the old snapshot would read as today's.
Then read the fields the report needs from the snapshot file:

```sh
snap=$(.claude/skills/codetrial-odds/snapshot.sh) &&
jq '{viewer, maintainers_known, unknown, moved, gone, prerequisites,
     overlap, collide, collide_unread,
     status: [.status[] | {pr, title, author, author_is_maintainer, size,
       mergeable, decision, grade, waits_on, waits_on_all, why,
       files_truncated, checks_truncated: .checks.truncated}],
     truncated: [.prs[] | select(.reviews_truncated or .comments_truncated
       or .threads_truncated or .closes_truncated) | .number]}' "$snap"
```

Report the open count, and every PR in `unknown`, `truncated`, or with
`files_truncated` or `checks_truncated` set, since its grade rests on less
than the others'. When `maintainers_known` is false, `author_is_maintainer` is
null and a read-only collaborator's comment may have counted as a request; say
so.

## Present it

Group the PRs by `waits_on`, maintainer first, since those are the ones a merge
decision can act on today; within a group keep the sweep's order, smallest
first, and say the size is total lines split into source and tests. Give each
PR its grade, title and the `why` in plain words, and when `waits_on_all` names
more than `waits_on`, say who else still owes a move. Say that `high` with no
review yet means ready for a first review, not approved. Mark a PR whose
`author_is_maintainer` is true as a maintainer's own, and one by `viewer` as
the reader's own, which needs another reviewer.

End with the merge order the snapshot records, `prerequisites` and `collide`
(with each collision's path and function), read as
[pulls.md](../codetrial-triage/pulls.md#similar-pull-requests) says, and name
`collide_unread` PRs as not compared. Say nothing about order the snapshot
does not record. `overlap` sets no order: list its pairs apart, as two PRs
closing one issue.

Then what moved, read as the end of [Rank by size and merge
confidence](../codetrial-triage/pulls.md#rank-by-size-and-merge-confidence)
says. On a first run `moved` and `gone` are null: say this run is the first
baseline.

The grade reads only what GitHub records. Reading a PR's non-test diff can
lower it, never raise it without a named reason; do that only for the PRs the
user asks about, and say which grades rest on a diff read.

## Local evidence at closeout

Before ending this invocation, follow the shared
[local evidence contract](../codetrial-conventions/SKILL.md#local-evidence-at-closeout)
and explicitly record its report using
[experience.md](../codetrial-review/experience.md). Include this skill in
`skills_used`; a combined invocation may use one report for its examined head.
Local experience is a lead to recheck, never a maintainer ruling.
