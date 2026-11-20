# Keep filed reports moving

Read this when the issue sweep's `unanswered`, `waiting`, `linked` or `claimed`
lists are not empty: what each one asks of the maintainer, and when a
reminder or a close follows. The verdicts and notice shapes it applies are in
[SKILL.md](SKILL.md).

A report helps only while someone can still answer for it. The maintainers here
usually ask for a revision or a log within a day, and in most open issues that
question is still the newest comment. #56 was closed after six silent days
with "Lost further actions", which told the reporter nothing about what would
have kept it open.
The four sweep lists are the queue, and each one has a fixed next move. An
`unanswered` or `waiting` entry whose `claimed` entry lists an open PR waits
on review of that PR, which the PR sweep owns, not on the reporter, once the
PR is read and does what the issue asks: `open_prs` holds every PR that
cross-referenced the issue, and a mention alone does not suspend the
reporter's reminder. `closing_prs` on the `unanswered` or `waiting` entry
itself names the open PRs whose merge closes the issue, whoever wrote them,
which narrows the candidates without reading every cross-reference. A closing
reference says only what the merge will close, so read each listed PR's scope
and implementation; when none does what the issue asks, the normal
`unanswered` or `waiting` rule applies. Only an entry whose PR does gets
neither a reminder nor a first answer on the issue (#370 and #385 sat in
`unanswered`, #159 and #298 in overdue `waiting`, all with PRs open):

- `unanswered`: no maintainer has spoken. Triage these first, oldest first, with
  [the verdicts](SKILL.md#decide-duplicate-status-and-completeness-separately).
  The sweep leaves out the viewer's own issues but keeps a collaborator's, since
  a proposal that ends in "Decision needed" waits on the maintainer like any
  report (#330). The longer the first answer takes, the less likely the reporter
  still has the build and the log it will ask for.
- `waiting`: a maintainer spoke last and the reporter has not commented or
  edited the body since. Read that comment first. A correction or a note carried
  over from another thread is not a question, and neither is an acceptance
  ("Go ahead", "Accepted as scoped") on an issue the commenter then assigned:
  that opens a claim, so the entry is handled under `claimed` and gets no
  `waiting` reminder (#330 sat in overdue `waiting` after "Go ahead"). Neither
  is "see if PR #N resolves this", which is a request to the maintainer and
  belongs under `linked`. When a
  maintainer comment in the thread is already the reminder
  [SKILL.md](SKILL.md#draft-the-notice-the-other-thread-needs) shapes, the deadline
  it names governs, even when a later maintainer comment rescoped the request
  and so restarted `asked_at` (#212 kept its deadline that way); a
  reminder that names no date starts the second window at its own date. Either
  way the next move is the disposition, never a second reminder. Otherwise, once
  an entry is `overdue`, propose one reminder for the thread on the confirmed
  list. It repeats the still-missing fact, says how to get it with the recipe
  from [codetrial-contribute](../codetrial-contribute/SKILL.md), and names the
  deadline and the disposition. When the deadline passes in silence, propose
  closing as not planned only if the missing fact still blocks action. A settled
  implementation direction is not abandoned for lack of a retest: #136 proceeds
  with removing Pause if no contrary evidence arrives. Read the latest
  disposition first. When a linked PR merges or the facts change while a
  reminder is pending, rescope the request and keep its deadline. The closing
  comment names the fact again and says a maintainer reopens the issue once it
  is posted, since a comment alone does not reopen it and the sweep reads only
  open issues. An issue a maintainer opened is left out of `waiting`, because
  its author already knows the rule.
  Read the newest comments of the thread before proposing any reminder, since
  the sweep sees only who spoke last. A `handed_over` entry is held by another
  non-maintainer, so the reporter is not who it waits on and it is never
  `overdue` (#445 stayed in `waiting` for a reporter after the maintainer had
  assigned the issue to another contributor, #512 after a contributor took it
  over): read that assignee under `claimed` instead. Likewise leave an entry
  alone when its newest comment is a contributor's own question or proposal,
  such as asking whether a narrower acceptance bar is acceptable: that is the
  maintainer's move, so answer it rather than reminding, and say in the report
  that the entry was dropped for this reason.
- `linked`, with `merged` set: a merged pull request referenced the issue after
  it was filed, and GitHub did not close it, so that PR only touched it, or its
  closing line named another issue. Read the PR and the issue. When the PR does
  what the issue asks, ask for a retest in the shape [the notice
  shapes](SKILL.md#draft-the-notice-the-other-thread-needs) give, with the
  recipe from [codetrial-contribute](../codetrial-contribute/SKILL.md); #82 got
  that and was closed within a day. A merged PR that names the issue only as an
  example is a mention, not a fix: the triage tooling PR #302 cited #156 and
  changed nothing it asks for, so read the diff before asking anyone to retest.
  When the PR covers only part of it, narrow the issue to the remainder, as #85
  was. The same waiting rule then applies to the retest.
- `linked`, with `open` set: a cross-reference is only a mention, so read the
  PR. When it implements the request, the issue is superseded by that open pull
  request (see [the
  verdicts](SKILL.md#decide-duplicate-status-and-completeness-separately)). Ask
  the reporter, and anyone in the thread about to write a second branch, to test
  that PR. Say how to check it out, since many reporters have never fetched a
  pull request. A maintainer's own fix PR gets the short back-link "See if PR #N
  resolves this issue." (#140, #149, #151). When a reporter closed an issue that
  an open PR still closes, check that the defect still holds on `main`; if it
  does, propose reopening it on the confirmed list, so the merge records the fix
  (#210).
- `claimed`: a non-maintainer assignee, which is how the maintainers record who
  holds the work once they accept a plan
  ([collaboration.md](../codetrial-contribute/collaboration.md#start-with-an-actionable-contribution)).
  The outstanding fact is the PR, and the reminder rule of `waiting` applies to
  it, with these differences:
  - `open_prs` belongs to the PR sweep; `merged_prs` takes the `linked`
    merged path above.
  - `stale`: first run
    `gh pr list --repo "$repo" --author "$login" --state all --limit 1000`,
    since a PR that never names the issue is invisible to the sweep. A PR
    there for this issue means the claim is alive: link it on the issue and
    review it. Only without one, propose one check-in that asks whether the
    PR is still coming and names a date `wait-days` out (#156).
  - `checked_at` is any maintainer comment after the assignee's last act,
    including an acceptance posted just after assigning; read it, and treat
    it as the check-in only when it names a date.
  - When that date passes in silence, propose removing the assignee, and
    assigning whoever asked to take it over if their plan already meets the
    scope. The issue stays open: the claim lapsed, not the report.
  - `asked_last`: read the assignee's comment first. A question there is the
    maintainer's move; a promise to send the PR is not (#156).
  - A null `stale` means the history was cut: read all of it before proposing
    anything. With `maintainers_known` false, check that the assignee is not a
    maintainer first.
  - A `waiting` entry marked `claimed` gets one nudge: this rule when the PR
    is outstanding, the `waiting` rule when a fact or a retest is.
  - Another author's PR for the issue is compared as
    [pulls.md](pulls.md#similar-pull-requests) says.

The two windows are the sweep's `wait-days` argument, 7 by default. Report each
list's counts next to the duplicate counts, so the maintainer sees what is
waiting and on whom.

The windows bound what a sweep proposes, not what the maintainer decides. A
maintainer may close earlier, as with #62 and #270 a week early and #115 and
#169 three days before their stated dates. The closing comment then names when
the fact was asked and never implies that a deadline passed. When the
maintainer asks to move the date itself, the edited reminder goes on the
confirmed list with the close, with its new text, and is applied before the
close, so the thread does not promise a date later than its close. The REST
endpoint takes the comment's numeric id, which the sweep does not collect:

```sh
comment=$(gh api "repos/$repo/issues/$number/comments" --paginate --jq \
  '.[] | select(.body | startswith("@'"$reporter"' ")) | .id' | tail -1)
[ -n "$comment" ] && gh api -X PATCH "repos/$repo/issues/comments/$comment" \
  -F body=@"$SCRATCH/reminder.md"
```

The reminder shape only fixes the leading @mention, so check that the id
found is the reminder before patching it.

Say in the report which reporters got less time than first promised.
