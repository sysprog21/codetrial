# Explain a large change before submitting it

A large pull request needs the contributor's account of what changed and why,
before it is opened. Interactive web review is required when **any proposed
commit has at least 200 added plus deleted reviewable text lines**, or when
the **whole PR has more than 500**. Exactly 200 triggers the commit rule;
exactly 500 does not trigger the PR rule unless a commit also reaches 200.
Measure each commit against its parent (a merge against its first parent).
Measure the whole branch against the merge base with the intended PR base,
using the final diff, not the sum of commit diffs or only commits since the
last review. For example, adding 120 lines and deleting 80 reaches 200 even
when a later commit reduces the final PR diff to a few lines. This is a practical
review budget, not a measure of difficulty or proof of understanding.

Count hand-maintained implementation, tests, scripts, configuration, and prose:
all take attention to review. Report the raw total and the reviewable total.
Exclude generated output, vendored content, lockfile churn, or mechanical moves
only when you list the paths and explain how their origin or unchanged behavior
was checked. Count changes to their generators and inputs. Do not subtract an
entire mixed file when only part is mechanical. Binary files have no text line
count; name them and how they were checked separately. Unknown counts or an
incomplete file list cannot establish that the branch is below the threshold.
The hook therefore requires interactive review for unmeasured committed files,
including paths Git marks binary. A `-diff` attribute is not a size exemption.
Missing per-commit history is likewise unknown coverage, not evidence that all
commits are small. Record each triggering full commit ID, its raw and reviewable
totals, and any exclusions; splitting commits does not remove the PR-wide check.

A maintainer may choose a different budget for a particular change; record the
decision and its reason. A contributor or agent cannot silently raise the
threshold or split a single change into commits to avoid it. Smaller changes
still need the usual problem, rationale, and validation in their PR description.

## What the contributor explains

The reviewer selects **one key modification** from the actual diff, identifies
its file and a short code passage, and asks the contributor to explain its
motivation or central tradeoff. This replaces the generic “Why is this change
needed?” prompt. There is one required explanation for the change, not an
answer per file. Up to two optional questions may accompany other consequential
files; the web workspace shows one question at a time as the diff changes, and
keeps each answer separate. Browsing alone does not invalidate a confirmed
explanation. Do not hide a checklist inside a compound question.

AI returns one grounded reference note after the contributor submits. A clear
author goal guides later code adjustments; it need not be rewritten to justify
an implementation that misses it. Ask at most one clarification when intent is
ambiguous. The contributor compares the reference with their explanation and
records their own assessment. A partial or missed answer requires a correction
or a remaining uncertainty. AI does not grade or approve this interaction.
The original question, submitted explanation and reflection stay bound to the
review snapshot as context for subsequent agent work.

While you write, the page counts words against a minimum of 12 and offers the
identifiers from the changed lines as buttons. Sending is refused only for an
answer that is too short or that repeats the question or the code word for
word; "I don't know" is acceptable however short, though it does not excuse
pasting the question or the code after it. An answer that names nothing from
the changed lines is accepted with a hint, and the reviewer then asks how the
quoted code behaves. Pasting a prompt or an agent-written summary is the thing
being discouraged: the explanation is yours.

The agent still inspects all changed areas, contracts, failure paths and
executed checks; reducing the form does not waive code review.

The contributor supplies this explanation in their own words, in conversation
or a local document, and answers focused questions grounded in the actual diff.
An agent can teach, check claims, and help edit the eventual PR body, but cannot
answer on the contributor's behalf or mark its own explanation as their answer.
A confirmation such as "yes", a file checklist, or approving an agent-written
summary does not supply the explanation. No word count or quiz score establishes
understanding. "I do not know" is useful: investigate that point together and
record the correction or unresolved uncertainty before completing the interaction.

With `codetrial-review`, use its [local web workspace](../.claude/skills/codetrial-review/workbench.md)
to read the diff, write your explanation, and send it to the active review
agent. It uses CodeTrial's existing visual styles. Saving a draft and sending
it for feedback are separate actions. The agent reads the saved submission,
checks it against the code, and sends feedback back to the page. Managed mode
uses an installed Codex CLI worker that remains active with the server after
the chat turn ends. In manual mode, resume the review task to receive further
AI responses. The browser itself never runs a model.

## What reaches the pull request

Keep the problem, reading order, essential design tradeoffs, and validation
limits in the English PR body. Preserve the final issue-closing line. Keep the
question-and-answer transcript and the local review report private; do not post
a score or claim that a contributor has been certified. The explanation can be
in the contributor's language locally.

The pull request carries no review block. CI measures the commits and the diff
itself, from the base branch's workflow and checker, so nothing in the
description can be edited out and a pull request cannot change the check. A change
under the thresholds passes. A change over them passes only once a maintainer has
added the `explained` label, which only people with triage access can add. The
label means a maintainer has seen the contributor explain the change. CI cannot
see the local evidence, so the label is the maintainer's word and not a
verification. It is not tied to a head: a later push keeps it, and the ruleset's
dismissal of stale approvals is what asks for a fresh review of that push.

An unanswered explanation or incomplete reflection means submission is pending,
even if the code review and tests pass. Continue useful review and validation
while answers are pending. After a change to the branch or its base, check the
explanation against the new diff and revisit the affected parts; do not reuse
an earlier answer for behavior that changed.

This is a contributor workflow requirement. Git hooks and CI do not establish
who wrote an answer or whether the contributor understands it.

## Troubleshooting

Each rejection names its reason in brackets.

- `[interactive-review/required]`: the pull request reaches a threshold and has
  no `explained` label. Finish the interactive review on the committed branch and
  show the explanation to a maintainer, who adds the label.
- `Interactive review required` at push: the same thresholds, found before the
  pull request exists. Finish the explanation and durable report for the exact
  head being pushed; amending or rechaining changes the head and needs them
  again.
- `no remote-tracking main to measure`: the hook has no base to size the change
  against, and does not skip the check for that. Run `git fetch` for the
  upstream remote (or add it) and push again.
- `Fetch complete history` or `cannot inspect local history`: the base is not
  fully present locally. Fetch upstream `main` and retry.

The required status check is the `interactive-review` job in the `pr-description`
workflow, which keeps its name from when it read the description. It reruns when a
label is added or removed. Merge groups recheck each included pull request's
current labels with the base checker. An independent maintainer approval is a
separate GitHub ruleset requirement, with stale approvals dismissed after a push.
The label and any future signature must not be described as proof of
understanding.
