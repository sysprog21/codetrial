# Explain a large change before submitting it

This page owns the large-change rules: the thresholds, who is exempt, what
reaches the pull request, and how to recover from each refusal. CONTRIBUTING.md
gives the one-time setup and links here; the skills follow this page.

A large pull request needs the contributor's account of what changed and why,
before it is opened. Interactive web review is required when **any proposed
commit has at least 200 added plus deleted text lines**, or when
the **whole PR has more than 500**. Exactly 200 triggers the commit rule;
exactly 500 does not trigger the PR rule unless a commit also reaches 200.
Measure each commit against its parent (a merge against its first parent).
Measure the whole branch against the merge base with the intended PR base,
using the final diff, not the sum of commit diffs or only commits since the
last review. For example, adding 120 lines and deleting 80 reaches 200 even
when a later commit reduces the final PR diff to a few lines. This is a practical
review budget, not a measure of difficulty or proof of understanding.

Count hand-maintained implementation, tests, scripts, configuration, and prose:
all take attention to review. The hook and CI count raw text lines and grant no
exclusion, so what decides whether the review is required is the raw count.
Generated output, vendored content, lockfile churn, or mechanical moves may be
pointed out to the reviewer, with the paths and how their origin or unchanged
behavior was checked, but that does not lower the count. Count changes to their generators and inputs. Do not subtract an
entire mixed file when only part is mechanical. Binary files have no text line
count; name them and how they were checked separately. Unknown counts or an
incomplete file list cannot establish that the branch is below the threshold.
The hook therefore requires interactive review for unmeasured committed files,
including paths Git marks binary. A `-diff` attribute is not a size exemption.
Missing per-commit history is likewise unknown coverage, not evidence that all
commits are small. Splitting commits does not remove the PR-wide check.

No one sets a different budget for a single change: the thresholds are in the
checker. What gets past them is the exemptions below (Dependabot, and a code
owner's own work) and the maintainer's ruleset bypass on the maintainer's own
pull requests. A contributor or agent cannot raise the threshold or
split a single change into commits to avoid it. Smaller changes
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
ambiguous. The reference may include a diagram, a table and quoted code: every
line it cites is checked against the reviewed diff. A link it could not cite is
written as an inferred (dashed) arrow; an uncited solid one is refused. The
contributor compares the reference with their explanation and records their own
assessment. A partial or missed answer requires a correction or a remaining
uncertainty. AI does not grade or approve this interaction.
The original question, submitted explanation and reflection stay bound to the
review snapshot as context for subsequent agent work.

While you write, the page counts words against a minimum of 12 and offers the
identifiers from the changed lines as buttons. Sending is refused for an
answer that is too short, that repeats a few words to reach the length, or that
repeats the question or the code word for word. Shorter overlapping passages
and possible relevance mismatches are hints, since automatic checks cannot
reliably judge their meaning or every language. "I don't know"
is acceptable however short; the review can begin with what you would read or
run first. An answer that names nothing from the changed lines is
accepted with a hint, and the reviewer then asks how the quoted code behaves.
Pasting a prompt or an agent-written summary is the thing being discouraged: the
explanation is yours.

A pull request often goes through several rounds of review, with more commits,
a rebase or a force push. Each new head over the thresholds needs its own
interactive review, and the post-rewrite hook says so after a rebase or an
amend. The core question changes between rounds: it goes to a change or an angle
the earlier rounds did not ask about, so the explanation covers the branch and
not one passage again.

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

The workspace also offers an optional, private intent record: the contributor's
own short answers on why the change exists and which check fails if it breaks. It is for the contributor to copy into the
English PR body and is never read by the hook or CI
([the workbench](../.claude/skills/codetrial-review/workbench.md#three-debts-what-the-change-leaves-behind)).

The pull request carries no review block and no label to ask for. CI measures
the commits and the diff itself, from the base branch's workflow and checker, so
nothing in the description can be edited out and a pull request cannot change
the check. The interactive review itself is required before the push, as above.
The pre-push hook refuses a large branch without it, and once the evidence
verifies it sends a small record with the branch: the review marker described in
[the workbench](../.claude/skills/codetrial-review/workbench.md#inspect-the-requirement),
which holds the counts, the head and base, a hash of the reviewed diff and
`complete`, and none of your answers. The hook signs it on your machine with
two fingerprints in it:

- the **review fingerprint**, a SHA-256 over this project, the head, the base,
  the diff hash and a digest of your answers, so it names exactly this code and
  this explanation without carrying a word of it; and
- the **key fingerprint** (`SHA256:...`) of the SSH key that signed it, which
  has to be a key on your GitHub account.

The signature is one `ssh-keygen -Y sign` over the review fingerprint. The key
is `git config codetrial.signingKey` (a private key, or its `.pub` with the key
in `ssh-agent`), else `user.signingKey` when Git already signs with SSH, else
the first key in `ssh-agent`. Setting one up is a one-time step, listed in
[CONTRIBUTING.md](../CONTRIBUTING.md#pull-requests): a key of its own,
registered on GitHub as a Signing Key, leaves the way you push untouched. The
push prints the key fingerprint it used.

On the pull request the required `interactive-review` check reads that record
from your fork and checks the signature against the keys GitHub publishes for
you: your authentication keys (`github.com/<you>.keys`) and your signing keys
(`GET /users/<you>/ssh_signing_keys`). A change under the thresholds
passes. A change over them passes with a "Large change" notice when the record
is there, matches the code exactly and is signed by one of your keys, and fails,
saying how to fix it, when it is missing, unsigned, signed by another key, or
for other code. A merge that GitHub's
"Update branch" makes, bringing in only the base, keeps the record of the commit
it was made on. A pull request that Dependabot opened and whose every commit is
Dependabot's needs no record, however large; a person's commit on its branch
removes the exemption. Nor does a code owner's own pull request: when the
account that opened it and the GitHub account GitHub links to every commit's
author are all named in the base branch's `.github/CODEOWNERS`, the review is
opt-in. A contributor's commit in it, or one GitHub links to no account,
removes the exemption, so a code owner opening a pull request for someone else's
work does not excuse it; the contributor signs the record, and the check accepts
the keys of the commits' authors as well as the opener's (the opener and up
to nine other commit authors). The check reads the
record from the repository the pull request's branch is in, so the code owner
carries the contributor's record there:
`git fetch <contributor-fork> refs/notes/codetrial-review`, then
`git notes --ref=codetrial-review merge -s theirs FETCH_HEAD` (the
contributor's record wins over a pending one the code owner's own push left) and
`git push <remote> refs/notes/codetrial-review`, with the contributor's commits
unchanged. GitHub links a commit
to an account by its author email, so a code owner's commits need an email on
their account, and a pull request of more than 250 commits gets no exemption. A code owner who does the review gets the same signed
record; one who skips it tells the pre-push hook once with `git config
codetrial.codeOwner <login>`, which the hook checks against the same base file.
The check never reads that setting, so claiming someone else's login only moves
the refusal from the push to the pull request. Everyone else goes through the
interactive review for every large change.

The signed record shows that the holder of your key declared a completed review
of this code with these answers. It does not show who wrote the answers, or
that you understand them: anyone who reads the checker can still produce one,
but now only as a statement signed in their own name. The approval the branch
ruleset requires is separate, and a push dismisses it, so a later change is
approved again. What that approval confirms about the explanation is not
settled ([development.md](development.md#what-cannot-be-enforced-locally)).

An unanswered explanation or incomplete reflection means submission is pending,
even if the code review and tests pass. Continue useful review and validation
while answers are pending. After a change to the branch or its base, check the
explanation against the new diff and revisit the affected parts; do not reuse
an earlier answer for behavior that changed.

This is a contributor workflow requirement. Git hooks and CI do not establish
who wrote an answer or whether the contributor understands it.

## Troubleshooting

- `Interactive review missing` on the `interactive-review` check: the pull
  request reaches a threshold and no completed review record for its code came
  with the push, usually because the hooks were not installed or were skipped.
  Run `make hooks` and finish `/codetrial-review` on the branch. If that
  changed the branch, push it; the record goes with it and the push reruns the
  check. If the branch is already up to date, send the record alone with
  `sh scripts/git-pre-push.sh --publish-review <fork-remote> [<branch>]`, then
  close and reopen the pull request: a record sent without the branch does not
  rerun the check. The command refuses when the fork's branch is not your
  `HEAD`, and says so when there is nothing to send. A first-time
  contributor's reopened run may wait for a maintainer to approve it, as the
  first one did; a maintainer can also rerun the failed job, which reads the
  record again. A record "made for different code" means the branch changed
  after the review: review the new head.
- `Large change` notice: the record is there and matches the code.
- `No SSH key to sign the interactive review with` at push: the one-time
  signing key setup is missing. Follow the three steps the message lists
  (generate, register as a Signing Key, `git config codetrial.signingKey`) and
  push again; nothing was pushed.
- `signed with SHA256:..., which does not verify against any SSH key on GitHub
  of <you> or the commits' authors` or `have an SSH key on GitHub` on the check: the record was
  signed with a key that is not on your account, often a key `ssh-agent` offered.
  Set `codetrial.signingKey` to a key you registered (the same three steps),
  then run `sh scripts/git-pre-push.sh --publish-review <fork-remote> [<branch>]`,
  which signs the record again and sends it without changing the branch, and
  close and reopen the pull request. The push prints the key fingerprint it
  signed with; `ssh-keygen -lf <key>` beside your keys on GitHub shows whether
  it is one of them.
- `could not read the SSH keys of <you> and the commits' authors from GitHub`: GitHub did not answer while the
  check ran. Nothing is wrong with your setup; close and reopen the pull
  request to run it again.
- `cannot send the interactive review record` at push: the hook could not push
  `refs/notes/codetrial-review` to the remote the branch goes to. Fix access to
  that remote and push again.
- `Cannot fetch the change`: the head the run was started for is gone, usually
  because the branch was pushed again. The newer push starts its own run.
- `Interactive review required` at push: the same thresholds, found before the
  pull request exists. Finish the explanation and durable report for the exact
  head being pushed; amending or rechaining changes the head and needs them
  again. When a large commit was reverted later and the final diff is empty,
  there is nothing left to explain: drop or squash the commit and its revert,
  so no commit reaches the threshold, and push again.
- `no remote-tracking main to measure`: the hook has no base to size the change
  against, and does not skip the check for that. Run `git fetch` for the
  upstream remote (or add it) and push again.
- `Fetch complete history` or `cannot inspect local history`: the base is not
  fully present locally. Fetch upstream `main` and retry.

The required status check is the `interactive-review` job in the `pr-description`
workflow, which keeps its name from when it read the description. It runs when a
pull request is opened, reopened or pushed, and for each merge group. An
independent maintainer approval is a separate GitHub ruleset requirement, with
stale approvals dismissed after a push. Neither the approval nor the signed
record may be described as proof of understanding.
