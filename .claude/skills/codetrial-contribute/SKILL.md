---
name: codetrial-contribute
description: Help CodeTrial contributors turn observations into clear English GitHub issues, small contribution plans, and pull request descriptions, and see them through. Use to draft, improve or file a CodeTrial issue, answer what a maintainer asked on it (a revision, a log, an initial analysis, a retest), prepare a PR description and check the open PRs and issues it will meet before opening it, retrieve and address PR review comments, respond to requests such as rebase, squash or commit message fixes, check which of your own issues and PRs are waiting on you, ask a focused maintainer question, or choose a bounded first contribution with an AI agent. The deliverable is copy-ready text, or verified fixes when asked to address review feedback; judging an existing backlog for duplicates is codetrial-triage. Opening a PR goes through gh-submit when it is installed; this skill covers it otherwise.
---

# Contribute to CodeTrial

CodeTrial is a mock interview platform, and contributing with an AI agent is a
supported way to learn. Start from the contributor's observation or intended
outcome. Explain unfamiliar terms briefly in their language, then produce an
English issue or PR draft they can understand and check. Never make a
questionnaire, skills survey or quiz a prerequisite for help.

Titles, body formatting, issue references and redaction follow
[CONTRIBUTING.md](../../../CONTRIBUTING.md); the rules for writing to GitHub,
approval included, are in the GitHub section of
[codetrial-conventions](../codetrial-conventions/SKILL.md). Keep technical
identifiers and quoted errors exact.

Example requests: "Turn these notes into an issue", "Help me make my first
CodeTrial contribution", or "Write a PR description from this diff". Produce
the artifact the user asked for.

How work moves on this tracker, from who the maintainers are to what each
one-line review request means, is in [collaboration.md](collaboration.md).
Read it before planning work, scoping a PR or answering a review.

## Draft or improve an issue

Check existing open and closed issues and related PRs first, using the retrieval
and comparison workflow in
[codetrial-triage](../codetrial-triage/SKILL.md). If a report already
covers the problem, offer a focused addition to that thread with new evidence.
When the deliverable is instead a verdict on someone else's thread, that comment
is codetrial-triage's. If GitHub access is unavailable, still prepare the
draft and explicitly mark the duplicate search as unverified.

Use the information already available in notes, logs and the checkout. Ask only
for missing facts that change the report's meaning. Never invent reproduction
steps, versions, root causes or successful tests. A plausible explanation is a
hypothesis; observed behavior is evidence.

Choose the smallest useful structure:

- **Bug**: concrete symptom, expected and actual results, numbered minimal
  reproduction steps, relevant version/environment, and a short sanitized error
  or other evidence. Identify the build by revision rather than by date, since
  published binaries move under a rolling tag (`docs/install.md`); triage has why
  that makes a date one-directional. Ask first what the binary reports about
  itself. When it cannot report a revision, a download date alone is not
  enough: a maintainer answers it with "Build from source instead." (#221;
  #171), so reproduce on a build from source and report its `--version`.
  For an interview failure, `.github/ISSUE_TEMPLATE/bug.yml` is the field list;
  ask only for the ones that matter. The problem language matters because Python
  and JavaScript run in the browser while C, C++ and Java run remotely, so one
  symptom means two different things across that split. Record frequency or
  regression range when known.
  Unknown details can stay explicitly unknown, but include the initial
  analysis below when the code path can be found, since a maintainer asks for
  it otherwise ([collaboration.md](collaboration.md#start-with-an-actionable-contribution)).
  Retest on a build from source of current `main` before filing: the lobby
  and the interviewer change daily, and #146 and #171 both reported behavior
  that had already been fixed after the reporter's download.
- **Feature**: who is affected (candidate, operator or contributor) and in which
  workflow, desired outcome, present workaround, scope and observable acceptance
  criteria. Keep a suggested implementation separate from the need it serves,
  and compare the approaches rather than asking whether one is acceptable.
- **Documentation**: page or section, what is missing or misleading, and the
  intended correction or reader outcome. Do not demand runtime logs for prose.

Do not split a single problem across multiple issues just to fill categories.
Do not combine two problems in one issue either; #151 and #205 each had to be
split after the fact.

## Get the facts a maintainer asks for

These are the requests that recur on this tracker. The contributor may never
have done any of them, so give the exact steps for their platform, run what can
be run in the checkout, and check the result before it is posted.

| Asked for | How to get it |
| --- | --- |
| The revision | `codetrial --version` with the binary that ran, or `cargo run -- --version` in a checkout. It prints `codetrial 0.1.0 (<sha>)`; post that line. |
| The revision of a binary too old for `--version` | Builds before 3ec7cb4 (2026-09-29) print nothing. A saved report's header shows `Contract: bundle N`. Put that number in place of `N` in `git log -S'INTERVIEW_CONTRACT_BUNDLE_VERSION: u32 = N;' --format='%h %cI' -- src/agent.rs`, which then gives the commit that set it and the one that replaced it; the build lies between them, as #188 shows. Without a report, the exact download time bounds the build from above, but expect to be asked to build from source. |
| The agent terminal output | The terminal window where `codetrial` is running prints the agent log. Copy the lines around the failure, starting at `starting interview: room=...`. Remove API keys and tokens, and every line that quotes what the candidate said: personal interview content stays out of a public thread even when the contributor would not mind. |
| The browser console | F12 or Ctrl+Shift+J (Cmd+Option+J on macOS) in Chrome and Edge, Ctrl+Shift+K in Firefox; Safari needs Settings, Advanced, "Show features for web developers" first. Copy the text rather than a screenshot. |
| A retest of an open PR | From a checkout: `gh pr checkout <N> --repo sysprog21/codetrial`, or `git fetch https://github.com/sysprog21/codetrial.git pull/<N>/head:pr-<N> && git switch pr-<N>`, then `cargo run -- web`. Report the result on the PR, not on the issue. |
| A retest after a merge | `git pull` on `main`, rebuild, and confirm `cargo run -- --version` names the merge commit or later. A downloaded binary gets "Always build from source." (#216) or "Build from source instead." (#221). |
| An edit to the issue | On the issue page, the `...` menu on the first post, then Edit; or `gh issue edit <N> --repo sysprog21/codetrial --body-file <file>`. Edit in place rather than filing again. |

## Write the initial analysis a maintainer asked for

"Provide an initial analysis and a list of follow-up actions" asks the
contributor to read the code, not the UI. A useful answer has four parts, and
#66, #133 and #211 are worked examples on this tracker:

1. The path: which file and function handle the behavior, found with `git grep`
   on a log line or a UI string the contributor saw. Give `path:line` at a named
   revision.
2. The evidence: the log line, measurement or state that shows where the
   behavior diverges from the expected one.
3. One hypothesis, labeled as such, and the experiment that would refute it.
4. Follow-up actions, each small enough to be a commit or a question.

"Identify the root cause" asks for where the failure starts, not the line
that fails. Reproduce at a named revision, then trace back: what passed the bad
value, and what passed it that, until one caller made the wrong choice. For an
interview failure, read the browser console and the agent log for the same
moment and name the boundary where they disagree. After three hypotheses are
refuted, post what each experiment showed as a question instead of a fourth
guess. When the cause lies outside the tree, say what was
ruled out and what stays unverified.

An agent's summary of the codebase is not an analysis. A maintainer rejected
one on #66 because it had no motivation and no evidence. Post the analysis as a
comment, or fold it into the body, then say which one you did.

Return a copy-ready title and body, followed by any remaining questions outside
the draft. Link related work and explain whether it overlaps or differs. For a
maintainer question, give the context, what was inspected or attempted, and one
specific decision needed to continue.

## Make a first contribution manageable

Offer one bounded task matched to the contributor's stated experience and time,
or a small first step when neither is given. Inspect current code and tests
before naming likely files; a `good first issue` label alone is not proof of
suitable scope.

Give the contributor:

- a visible result, such as a minimal reproduction, focused docs correction or
  regression test and fix;
- likely entry points and a concrete acceptance check;
- a stopping condition and fallback, such as drafting the exact maintainer
  question if the expected behavior is still undecided.
- the one setup step the first commit needs, `make hooks` in the clone, so the
  message rules run on that commit rather than surfacing in CI
  ([conventions](../codetrial-conventions/SKILL.md#commit-messages)).

Keep unrelated refactoring out of the task. Do not require filing an issue for
every small, well-understood fix. For uncertain product direction, establish
the desired behavior before investing in a large implementation. When coaching,
help the contributor explain what fails before and succeeds after the change.

## Prepare a pull request people can review

Commit with the hooks installed: `make hooks` once per clone, read as
[conventions](../codetrial-conventions/SKILL.md#commit-messages) says, before
the first commit. Read the full branch diff against its intended base, relevant
commits and linked issues. Check that the branch contains the intended work only, and use a topic
branch as described in conventions. Scope follows
[collaboration.md](collaboration.md#scope-one-pull-request): unrelated fixes
and unrequested behavior go to their own PR, review follow-ups stay in this
one, and an open PR doing the same thing is reviewed rather than competed with. Preserve unrelated working-tree edits.
AI-generated summaries are starting points: validate every claim against the
diff and actual test results. Do not paste a conversation transcript.

Use [codetrial-verify](../codetrial-verify/SKILL.md) for the appropriate validation
and [codetrial-web](../codetrial-web/SKILL.md) when browser or wire behavior is
involved, [codetrial-livekit](../codetrial-livekit/SKILL.md) when the room or
the LiveKit crates are. Then review the committed branch with
[codetrial-review](../codetrial-review/SKILL.md) before the description is
drafted: it reads what the gate cannot (the accepted scope, the contracts in
`docs/`, what maintainers already ruled on the same files), and a finding fixed
now costs nobody a review round. Fix what it confirms, or say in the
description why a finding does not apply; its report is never pasted into the
PR. Report commands and outcomes, including failures and skipped or unrun
checks with reasons. Local success is not evidence that remote CI passed, and
a PR is not ready for review until CI is green.

Before handing the draft to gh-submit or opening it directly, apply
[the large-change explanation requirement](../../../docs/large-change-explanation.md)
to the complete committed PR diff and every proposed commit. If any commit
has at least 200 added plus deleted text lines, or the whole PR has more than
500 (raw counts, as the hook and CI measure them), and the pull request is not
a code owner's own work (opener and every commit's author in CODEOWNERS)
or Dependabot's alone, the local review must include the contributor's explanation and
follow-up answers checked against this base and head. If absent, incorrect, or
stale, ask for the missing account and keep submission pending while continuing
independent preparation. An approval to publish or an agent-written summary
does not replace the answers. Preserve the useful rationale and reading order
in the PR body, but keep the private report and transcript out. The pull
request has no review block to fill in: CI measures the change itself, and a
change over the thresholds still needs the interactive review before the push;
the check fails unless the pre-push hook sent its completed record, signed with
an SSH key on the contributor's GitHub account. For a refusal by the hook or
the check, quote the fix from [the troubleshooting
list](../../../docs/large-change-explanation.md#troubleshooting) rather than
restating it. A later code or
base change requires rechecking the count, and a new review session: the
evidence is bound to the exact base, head and diff, so the earlier session is
refused and the answers are written and sent again, which can start from the
earlier text.

The description should cover:

- the concrete problem and resulting behavior, with a before/after example
  when it clarifies the change;
- the essential implementation choice or tradeoff a reviewer needs to assess;
- validation evidence and material limitations; include screenshots or sanitized
  logs only when they help verify the result;
- relevant compatibility or follow-up work, then the issue reference line.

For a simple PR, one or two paragraphs plus validation is enough. Rewrite the
title and body when scope changes.

Before the draft is shown, check what the branch is about to meet. Commit
first, since only commits are read. Run on the branch, it compares against
upstream `main` even from a fork, and takes issue numbers when the commits name
none:

```sh
.claude/skills/codetrial-contribute/related.sh > "$SCRATCH/related.json"
```

Each field it prints has one consequence for the draft, and the hint goes to
the user with the draft rather than being acted on alone:

- `same_issue`: another open PR closes the same issue. Read its diff first; an
  open PR doing the same thing is reviewed, not competed with
  ([collaboration.md](collaboration.md#scope-one-pull-request)). When the two
  differ, the description says how, linking the other PR.
- `issues[].others_hold`: the issue is assigned to someone else, so the work is
  theirs until a maintainer reassigns it. Ask in the issue before opening.
- `issues[].state` closed, or merged when the number is a pull request: the
  work may already be done or declined; read the closing comment and say which
  in the draft.
- `closes` empty while `refs` names the issue the branch finishes: the last
  line should be `Close #N`, not `Refs #N`, or the merge will not close it.
  `refs` leaves out what `closes` already has, and both read `#N`,
  `sysprog21/codetrial#N` and the issue URL.
- `same_files`: overlap by file is weak alone, since `web/interview.js` and
  `web/styles.css` sit in most UI PRs, but a shared harness file in
  `web/compiler-explorer.js` or `web/runners.js` means whichever merges second
  rebases (#376 and #396). Name such a PR in the description when it rewrites
  the same function.
- `own_pr`: a PR whose head is this commit, or the viewer's PR from a branch of
  this name, is already open; update it instead of opening a second.

`RELATED_BASE=<ref>` skips the fetch, but the ref has to be as current as
upstream `main`: against a stale one, commits that already merged count as the
branch's own.

## Publish only after the user sees the text

A request to file an issue or open a PR authorizes preparing it; posting still
waits for the approval described in conventions. Show the target repository and
the title and body verbatim. Feedback that changes the draft without saying to
post it gets the revised draft shown again; "change the title to X and submit"
approves the result and needs no second round.

To open a PR, pass the drafted title and body to gh-submit when it is
installed, and tell it the base is `sysprog21/codetrial`, since on a fork its
`origin` is the contributor's copy. Its single confirmation of repository,
branches, commits, title and body is the approval; do not ask separately first.
Without gh-submit, show the same five things, create the PR with
`--repo sysprog21/codetrial --head <fork-owner>:<branch>` after the yes, and
follow with `gh pr checks <N> --repo sysprog21/codetrial --watch` so the
contributor learns whether CI passed rather than just receiving a URL.

For an issue, repeat a narrow search on the title terms just before creating it,
in case someone filed it meanwhile:

```sh
gh issue list --repo sysprog21/codetrial --state all --search '<title terms>' --limit 5 --json number,title
gh issue create --repo sysprog21/codetrial --title "$(cat "$SCRATCH/title.txt")" \
  --body-file "$SCRATCH/issue.md"
```

Finish with the verified URL and the next concrete step, including any
outstanding evidence the contributor can supply. Tell the contributor that
filing starts the work rather than finishing it: a maintainer usually asks
something within a day. A question left unanswered gets one dated reminder
after about a week, naming a deadline; past it the issue may close as not
planned, or proceed as already decided when the thread settled that.

## Follow through on what you filed

When a contributor asks what needs their attention, or comes back to an issue
or PR, start from the same snapshots triage uses, filtered to them:

```sh
me=$(gh api user --jq .login)
.claude/skills/codetrial-triage/issue-sweep.sh |
  jq --arg me "$me" '.waiting[] | select(.author == $me)'
gh pr list --repo sysprog21/codetrial --author "@me" \
  --json number,title,reviewDecision,mergeable,updatedAt
```

`pr-sweep.sh` leaves the viewer's PRs out of its nudges and rebase requests,
which are a maintainer's to send, but its `status` covers every open PR, with
what is still open on each:

```sh
.claude/skills/codetrial-triage/pr-sweep.sh |
  jq --arg me "$me" '.status[] | select(.author == $me) | {pr, grade, why}'
```

A PR whose `reviewDecision` is `CHANGES_REQUESTED` or whose `mergeable` is
`CONFLICTING` is waiting on its author, unless that decision comes only from a hidden rebase
request the branch has since answered, which waits on a maintainer to
re-review. Checks held as "awaiting approval" are waiting on a maintainer
instead
([collaboration.md](collaboration.md#make-ci-green-before-asking-for-review)).

For each hit, read the newest maintainer comment and do what it asks, using the
table above. Answer it in that thread, quoting the question when the thread has
several. A fact that belongs in the report also goes into the body by editing
it, so the next reader does not need the comments. When a maintainer links a
PR that may fix the issue, retest it and say on the PR whether it does. When a
fix merged and the retest passes, say so with the `--version` line, and close
the issue, with the user's approval, if a maintainer has not. An open issue nobody can confirm stays on
everyone's list.

## Answer a review

Before pushing the answer, review the fix with
[codetrial-review](../codetrial-review/SKILL.md) on the local branch, with the
head the maintainer reviewed as its base (`review-bundle.sh --local <dir>
<that sha>`), so it reads exactly the new commits and catches a fix that breaks
something else before the maintainer has to.

Fetching every thread, including long ones, and rechecking the head after a
push, and where failed checks are diagnosed, are in
[review-feedback.md](review-feedback.md).

Git and commit-message requests recur alongside substantive code review. Read
the current inline threads, unanswered cubic findings and failing checks as
well as the review summary. The one-line requests are decoded in
[collaboration.md](collaboration.md#what-the-one-line-review-requests-mean);
the git steps behind them:

- "Rebase latest `main` and resolve conflicts": synchronize before making the
  review changes, then inspect what the updated base already supplies. Find the
  remote that points at
  `sysprog21/codetrial` in `git remote -v`. On a fork it is usually missing, so
  add it with
  `git remote add upstream https://github.com/sysprog21/codetrial.git`; in a
  clone of upstream itself it is `origin`, so use that name wherever `upstream`
  appears below. Then `git fetch upstream && git rebase upstream/main`,
  resolve each conflict, `./scripts/test.sh`, and
  `git push --force-with-lease --force-if-includes`, which also refuses when a
  background fetch moved the branch under you. A merge commit from `main` is
  not a rebase.
- "Squash" or "fold similar commits": one commit per functional change, with the
  review fixups folded into the commit they fix. `git rebase -i upstream/main`
  is the tool. An agent that cannot drive an interactive editor can run
  `git reset --soft "$(git merge-base HEAD upstream/main)"` and commit again,
  but only when the result is meant to be a single commit and `git status`
  shows a clean index and work tree first, since anything staged would join
  it. Show the contributor the new `git log` before pushing.
- "Read cbea.ms/git-commit" or "refine commit messages": run
  `./scripts/git-commit-msg.sh --rules`, then `make hooks` so the next commit is
  checked before it leaves the machine. The body says what and why. The
  conversation that produced the change stays out of it, and so do the AI
  attribution trailers conventions names.
- "Run `make indent`": run it, check that `git diff` shows only formatting, and
  amend the commit that introduced the code rather than adding a "format"
  commit.
- "Append `Closes #N`" or "avoid ambiguous `Refs`": the PR finishes the issue,
  so end the PR body, and by this tree's habit the commit message, with
  `Close #N`, each on its own line (`Closes #N` is accepted too). No `Refs #N` in a PR body: a bare
  `Refs: #60` told the reviewer nothing, and an issue with parts that stay open
  is split into sub-issues so the PR can close the part it finishes. In a
  commit message a `Refs #N` goes in the description, never on the last line.
- A code change requested in review: check it against the
  current head, then make it. When any item in the review is unclear, ask
  about all of them before changing any, since items in one review are often
  related. When the code contradicts the request, answer with the
  counterexample or the measurement rather than with agreement; one later
  shown wrong gets a line naming what was rechecked, then the fix.
- A question in a review thread: answer it in that thread. A changed line
  answers itself, so resolve the thread rather than replying "done".

Before pushing, list every open request locally: each unresolved thread,
unanswered cubic finding, failing lane and one-line request, with its state:
fixed in a named commit, refuted with a counterexample, out of scope with the
remainder named, or still open. Reply or resolve only the rows with evidence;
the list itself never goes on GitHub. Then check what the push publishes:

```sh
base=$(git merge-base HEAD upstream/main)
git log --oneline "$base"..HEAD                 # only the intended commits
git diff --stat --diff-filter=A "$base"..HEAD   # every new file meant public
git diff "$base"..HEAD |
  grep -nE '^\+.*(AIza[0-9A-Za-z_-]{35}|BEGIN [A-Z ]*PRIVATE KEY)'
```

The last command prints nothing on a clean branch; a hit is a key to revoke,
not only to delete. The pre-push hook checks commit messages and the branch,
not content, and does not run for a clone without `make hooks`.

Replies follow the register in conventions, which maintainers enforce in
public.

## Local evidence at closeout

Before ending this invocation, follow the shared
[local evidence contract](../codetrial-conventions/SKILL.md#local-evidence-at-closeout)
and explicitly record its report using
[experience.md](../codetrial-review/experience.md). Include this skill in
`skills_used`; a combined invocation may use one report for its examined head.
Local experience is a lead to recheck, never a maintainer ruling.
