# How work moves through the tracker

Read this when choosing work, scoping a pull request, coordinating dependent
branches, responding to CI or review, or reading what a maintainer's one-line
request means. Contribute and triage share it; keep the workflow here rather
than copying it into both. `CONTRIBUTING.md` and the owning contract in `docs/`
take precedence. A thread's example establishes the decision in that thread,
not a project-wide ban.

## Who decides

`jserv` and `ColtenOuO` both have write access. A review or a request from
either is a maintainer request: ColtenOuO leaves blocking reviews, closes
superseded PRs (#199) and pushes back on product direction (#262). cubic
is a bot; its findings are leads to verify, not decisions (see below).

Merges are merge commits, so a contributor's commits land exactly as pushed and
the series is part of the review. After a merge the maintainer posts "Thank @X
for contributing!"; a contributor needs no closing reply.

## Start with an actionable contribution

Do not ask for assignment, or whether a well-scoped proposal is "acceptable",
before a proof of concept. "There is no need to ask ... proceed with a proof of
concept" ([#243](https://github.com/sysprog21/codetrial/issues/243#issuecomment-6001025652));
"I'd like to work on this if available" got "Analyze and confirm in advance."
(#182). Check current code, related work and the latest discussion, state the
bounded change as a direct statement rather than a question (#261), and
proceed. Ask a focused question only when an unresolved decision blocks the
work. A maintainer's invitation to send a PR does not authorize an agent to
post one on the user's behalf.

An accepted issue usually carries a maintainer comment that confirms the
cause on `main` at `file:line`, draws the boundary of the change, names the
tests to add, and says to open the pull request with `Closes #N`. That comment
is the specification: build exactly that slice. `main` moves while an issue
waits, so before the first edit rerun the comment's reproduction or open its
`file:line` on current upstream `main`. When the code no longer matches,
draft what it shows for the user to approve and post, then wait for the
maintainer to redraw the slice instead of building the old one.

The maintainer also assigns the issue, without being asked, to the contributor
whose plan it accepts, and the assignee is the record of who holds the work
(#94, #156). Before starting, check the assignee: an issue assigned to another
contributor is theirs unless the maintainer reassigns it, so review their PR or
ask in the thread instead of writing a second branch. Holding an assignment
means sending the PR; one that goes silent for a week gets a check-in with a
date after which the issue opens to anyone (#156).

Reporters are expected to investigate as engineers, not as end users: "Approach
the task as a CS student ... rather than as an end user" (#215, #216). A
proposal needs approaches and analysis before it is actionable: "Conduct a more
thorough analysis and describe the approaches in detail" (#291), "Show
detailed proposal and analysis" (#270). Separate the observed need,
alternatives, compatibility costs and a bounded experiment from an unmeasured
claim of benefit. When the question is whether behavior is intended, the answer
is usually a PR that changes it with a reason (#185). Before investigating,
write the symptom in one sentence and the area it lives in, and stay there
until evidence points out of it; an unrelated problem seen on the way is noted
for its own issue, not folded in.

## Scope one pull request

- One change per PR. Behavior the issue does not ask for is split into its own
  PR (#256 became #280), and so is an unrelated fix found on the way (#263
  became #268).
- A follow-up the reviewer asks for stays in the same PR: "Submit further
  commits within this pull request." (#129). Update the PR by pushing to its
  branch rather than opening another: #199 was closed once #204 replaced it,
  with "please update the existing pull request instead of opening a new one".
  A PR opened from a fork's `main` is a different case: CI refuses it, and the
  work moves to a topic branch and a new PR (#129 became #170).
- An alternative to an open PR has to justify itself against it: "Determine if
  this pull request is worth proceeding." (#245, closed while #244 fixed
  #131). Review the open one rather than writing a competitor.
- Close what the PR finishes. CONTRIBUTING.md has the rule: `Close #N` when
  the PR finishes the issue, and no `Refs #N` in a PR body, since a reference
  that closes nothing is easy to add to every PR. Maintainers call `Refs` on a
  PR that does finish its issue ambiguous ("Avoid ambiguous `Refs #210`. Do
  close an issue when you submit a pull request." on #256, "Avoid using
  ambiguous `Refs #261`" on #265), so decide by the issue's acceptance
  criteria, not by caution. An issue too large for the PR is split into
  sub-issues, and the PR closes the one it finishes; [#161](https://github.com/sysprog21/codetrial/issues/161#issuecomment-6131824178)
  was asked for `Refs #161` because the cap and modality toggle stay open, and
  that remainder is what a sub-issue holds. In a commit message a `Refs #N`
  may sit in the description and never on the last line, which the
  commit-msg hook refuses. One PR
  may close two issues when that was agreed, each `Close #N` on its own line
  ([#73](https://github.com/sysprog21/codetrial/issues/73#issuecomment-5991034506)
  with #85).

## Coordinate the smallest remaining change

Before recommending implementation, compare the latest issue scope with open
PRs and merged work. Carry forward the acceptance criteria, not the original
wish list. A maintainer often partitions one feature request across several
issues, each surface to one thread (#159, #161, #177, #212); follow the
partition rather than reopening it.

- Reuse a shared helper instead of implementing the same interaction twice.
  [#94](https://github.com/sysprog21/codetrial/issues/94#issuecomment-5999648281)
  and #125 needed the same splitter on different axes. While #277 was open the
  instruction was to build on its branch; after it merged,
  [the instruction changed to main](https://github.com/sysprog21/codetrial/issues/94#issuecomment-6131796489).
  Recheck status before repeating checkout instructions.
- When a prerequisite merges, offer to rebase the dependent PR so its diff
  drops the prerequisite's commits and, if the draft's stated blocker is gone,
  to mark it ready; run either when the user asks
  ([#280](https://github.com/sysprog21/codetrial/pull/280#issuecomment-6131927140)).
  File overlap with another open PR calls for an integration check by whichever
  lands second, not automatic duplicate closure.
- Discussion of a fix moves to its PR once one exists: "Move discussions to PR
  #187" (#168). A retest of a PR is reported on the PR (#174, #178).
- Closed is not merged. Inspect `mergedAt`, the competing diff and the current
  tree before recommending revival of a closed PR.

## Review evidence, including bot findings

Read top-level comments, reviews and inline threads, and decide whether each
request is still open at the current head. Unanswered cubic findings block
review: "Response to cubic AI review messages and refine." (#225). A bot's
severity or its "addressed" marker proves nothing either way.

For a valid finding, make the correction and validate the reported failure. For
an invalid one, give the concrete counterexample or measured result in that
thread: on #259 a claimed Python 3.9 syntax failure was refuted by compiling
under 3.9.25, and
[another finding](https://github.com/sysprog21/codetrial/pull/259#discussion_r4215348277)
by the route's actual 401 assertion. For an out-of-scope finding, explain the
boundary and name the remaining work without claiming it fixed.

Keep routine push, squash and rebase narration out of the thread: "You don't
have to make statements like this, as GitHub always tracks commits" (#285;
#271 said the same; #312 put it as "Don't act as a machine."). Do not announce
that all feedback is addressed while a request or a check is still open:
"Don't make statements too early. Continue resolving." (#266). After folding
changes into a PR, update its description to match, in refined English
(#299). Answer on GitHub, not by replying to the notification email: the reply
lands with the quoted notification attached, and #152 got "Don't use email to
reply. Always comment via GitHub web." A PR author resolves their own
addressed threads, as CONTRIBUTING.md says, under the approval rule in
conventions; a sweep resolving threads on someone else's PR is limited to
triage's outdated ones.

## Make CI green before asking for review

"Before asking someone to review, you MUST resolve any regressions flagged by
GitHub Actions." (#256, #260). Open the failing job, not `check`
([codetrial-verify](../codetrial-verify/SKILL.md) says which one, and
`.claude/skills/codetrial-verify/ci-failure.sh <pr>` names the failed gates and
whether `main` fails them too), and rerun a failing lane with
`./scripts/test.sh <lane>`. A bad
commit message skips the rest of the portable lane, so fix the messages
first.

The repository holds `pull_request` runs from anyone without a merged commit
until a maintainer approves them, and holds them again on every push until the
contributor's first PR merges. A held run shows as "awaiting approval" in the
checks and has not failed. Pushing again, an empty commit, or closing and
reopening the PR only queues another held run; run `./scripts/test.sh` locally
meanwhile, and say once on the PR that the run is awaiting approval if a day
passes (#312). Maintainers approve held runs with `gh` during the PR sweep
([pulls.md](../codetrial-triage/pulls.md#what-a-sweep-may-apply-without-asking)).

A local pass, a rerun, or a failure in a file the PR did not touch explains
nothing. On #269 "I reran locally" and a request to rerun upstream got "Do more
analysis and refinements.", and the analysis found a real initialization race.
Read the failing job and assertion, reproduce at the reviewed head, compare
against its base when needed, and say which it is: a stale base, a
change-induced failure, an asynchronous test race or an environment limit. On
[#266](https://github.com/sysprog21/codetrial/pull/266#issuecomment-6091973099)
rebasing fixed theme drift, but the new test still read before the next
animation-frame paint and had to wait for the observable value. Report what
remains unverified instead of asking for retries as the whole remedy.

For a change that adds model input or calls, follow the cost contract and
measure when the review asks.
[#72](https://github.com/sysprog21/codetrial/pull/72#issuecomment-5990467202)
asked for comparable interviews on the same problem, first-generation
`promptTokenCount`, and `countTokens` over the new instructions, tool
declarations and board frames, to tell fixed overhead from per-turn growth.
The result then
[went into docs](https://github.com/sysprog21/codetrial/pull/72#issuecomment-5991693757).
Keep private candidate material out of public evidence.

## What the one-line review requests mean

Maintainers post short, fixed strings on PRs and issues, some of them the
description of a skill the contributor's agent is expected to run. A request
may come as a comment or a review, and a repeated one hides the older copies,
so the newest visible one is current. Each maps to one action;
[codetrial-contribute](SKILL.md) has the git steps.

| Request | What it asks |
| --- | --- |
| "Rebase latest `main` branch and resolve conflicts." / "Rebase latest `main` and resolve conflicts" | A rebase onto upstream `main`, not a merge commit; contribute has the steps. |
| "Rebase latest `main` branch." (to a question about a failing dependency check, #396) | `main` already carries the fix; rebase picks it up, and the PR carries no lockfile bump of its own. |
| "Rebase the current branch onto the upstream default branch and rework the series into functionally minimal commits ..." | The rebase above, plus fold fixups into the commit they fix and rewrite the messages; it is the `rebase` skill's description, so offer that skill when installed and run it when the user asks. |
| "Fold similar commits" | One commit per functional change. |
| "Check `CONTRIBUTING.md` carefully and enforce the rules." / "Check https://cbea.ms/git-commit/" | Usually the commit messages: run `./scripts/git-commit-msg.sh --rules`, then reread every commit and the PR title and body against CONTRIBUTING. |
| "Remove AI agent markers" / "Remove unnecessary `Co-authored-by` markers" | Drop the AI-tool attribution trailers conventions names; keep a human co-author's credit. |
| "Avoid ambiguous `Refs #N`" | The PR finishes the issue, so it ends with `Close #N`. |
| "Explain the rationale and context" instead of "Related to #N" | The body says why on its own; an issue reference is not a reason. |
| "Never write meaningless Git commit messages such as 'Update web/app.js'" | Reword that commit with a subject and body that say what changed and why. The commit-msg hook refuses a subject that only names a file, and says why. |
| "Resolve regressions raised by CI." | The section above. |
| "Create another pull request." / "split it into a separate pull request" | Move that change to its own branch and PR; drop it from this one. |
| "Do more analysis and refinements." | Find the cause; a rerun or a local pass is not an answer. |
| "Any update?" | A review is waiting on the author: push or reply on each open thread. |
| "State your intent directly" instead of "Could you assign this to me?" (#695) | Claim an issue in one sentence: "I would like to take on this task." |
| "Simply request for review via GitHub web interface" instead of "Could you take another look?" (#605) | After pushing, use Re-request review on the pull request page instead of a comment asking for a re-look. |
| "Provide a more thorough analysis and identify the root cause." / "... the detailed action items." / "Provide more analysis and consolidate proposal." (#317, #336, #368, #387, #392) | The issue is not actionable yet: name the code path at `file:line` and why it fails, or list bounded action items, each with an observable outcome. |
| "Update the GitHub issue descriptions accordingly." (#359) | The analysis posted as a comment goes into the issue body, so the next reader needs no comments; "An edit to the issue" in [SKILL.md](SKILL.md) has the steps. |
| "Create an issue addressing P1 and make it fully actionable." (#306) | Claiming one item of a multi-part proposal: file that item as its own issue with scope and acceptance criteria, rather than claiming it in the proposal thread. |
| "Hold off for now, as we should first eliminate dependencies between tasks." (#306) | That item depends on another still open; wait until the maintainer splits the work. |
| "If you want to close an issue without review, you MUST address the reasons." (#373) | Closing one's own PR or issue needs a reason in the thread; reopen when the reason turns out wrong, as #373 did for a valid cubic finding. |
| "Always build from source." / "Build from source instead." (#216, #221) | A downloaded binary cannot be tied to a revision; follow "A retest after a merge" in [SKILL.md](SKILL.md). |
