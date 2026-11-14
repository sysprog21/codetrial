---
name: codetrial-conventions
description: The CodeTrial conventions the gate does not settle on its own, several of them enforced by the git hooks instead - the register a comment, a commit message, an issue or PR title and a PR reply are written in, what stays out of a public issue or PR, the untracked working docs at the repo root, where multi-byte characters are allowed and where they are not, deleting a redundant surface instead of deprecating it, the contracts in docs/ that refuse a change, and the repository layout. Use when drafting a commit message, choosing an issue or PR title, pushing a branch to open a pull request from, adding a file or a public surface, removing a flag or a subcommand, proposing a change to how an interview spends, caches, records or reports, drafting or editing any public issue, PR or comment, or writing a comment longer than a line.
---

# CodeTrial conventions

The gate settles formatting and correctness. `cargo fmt`, Prettier, ESLint,
`ruff` and `shellcheck` run in `scripts/test.sh`, so none of that is here. What
is here is what a reviewer would otherwise have to say out loud, plus the rules
the git hooks enforce, so an agent knows them before a hook refuses. Install
them with `make hooks`.

Two tracked files carry the rest, and this one restates neither. `README.md`
wins on what the project is and how it runs.
[CONTRIBUTING.md](../../../CONTRIBUTING.md) is the public half of the rules
below, the commit message, the issue and pull request register, and the branch a
pull request comes from, and it wins where the two disagree. Read it. What is
left here is the agent's half: the calibration this tree's own log gives, which
repository `gh` is pointed at, how drafted text reaches it, and what needs the
user's approval first.

The order to work in, and what an agent leaves alone without being asked, is
[codetrial-improve](../codetrial-improve/SKILL.md).

## Commit messages

Before the first commit in a clone, run `make hooks`. It is safe to repeat, and
a worktree shares the clone's hooks, so once per clone covers every branch. It
verifies what it installed, and exit 0 means every hook is in effect: one `OK`
line per `scripts/git-*.sh`. A nonzero exit means at least one is not, and the
lines say which. `FOREIGN` means a hook of the user's own already sits there and
ours is not installed, and `SHARED` means `core.hooksPath` points outside the
repository, where nothing was written: say so, and leave it for the user to
settle with the integration the output prints, since deleting their hook or
writing into a shared directory is how a local workflow disappears. `STALE`
is an older wrapper of ours, which `make hooks` rewrites without asking.
`./scripts/install-git-hooks.sh --check` re-asks without writing. Never
commit with `--no-verify` or `-n`, and when the commit-msg hook refuses a
message, rewrite the message: the pre-push hook and CI replay the same rules
later, when fixing them costs a rebase.

`./scripts/git-commit-msg.sh --rules` prints what the hook enforces, and
`CONTRIBUTING.md` says what the body has to carry. Neither gives the
calibration, so run this when a subject is hard to fit:

```
git log --no-merges --format=%s | awk '{print length}' | sort -n
```

One thing reviewers reject that the hook cannot see: the body stands on its
own. "Related to #263" in place of the reason got "explain the rationale and
context" (#268). The hook now refuses the two other things maintainers used to
ask for by hand: a `Co-authored-by` or "Generated with" trailer attributing an
AI tool (#251, #256, #285; a human co-author's credit stays), and a subject that
names only the file, such as GitHub's web-editor default "Update web/app.js"
(#276), which says where and not what and which the refusal explains.

## Prose register

Markdown files are exempt from the ASCII rule and use ordinary GitHub Markdown,
so a backtick belongs in `docs/` and not in a commit subject.

Chinese, and CJK generally, stays out of `src/`, `web/` and `tests/` even when
the conversation that produced the change was in Chinese. The product's
surfaces are English. A test that genuinely needs multi-byte input to pin a
character-boundary rule should use characters that read as a fixture rather
than as prose in any language: Greek letters, accented Latin, an emoji. Say in
the comment that the byte width is the point.

The hooks enforce this outside Markdown through `scripts/check-cjk.py`. Where
CJK is the input under test (a CJK word counter, an IME composition), keep it
in a quoted string on one line, or between two ```` ``` ```` lines in a comment
for a multi-line sample; anything else is refused, a one-line docstring and a
string spanning lines included. Rust and the browser's JavaScript, HTML and
CSS allow none at all, quoted or not, through the rust-style gate.

## Comments

Every line of a comment says something the code cannot, and this tree carries
long ones where the reasoning is long. Delete anything restating the statement
below it.

- Bad: `let deadline = start + duration; // add the duration`
- Good: `let deadline = start + duration; // truncated to whole minutes above,
  so the page and the interviewer end at the same number`

Never point a comment, a docstring or a `docs/` page at `TODO.md`. It is
untracked and per-developer, so the reference dangles for everyone else. Code
may name the path where it genuinely reads the file, and a runtime diagnostic
may print it so the reader can navigate.

## Working docs at the repo root

`TODO.md`, `DONE.md` and `CLAUDE.md` are excluded through `.git/info/exclude`,
not through `.gitignore`, because the exclusion is one person's habit and
`.gitignore` would push it onto everybody. Never `git add`, stage, commit or
delete them, and never assume a clone has them. Reports and analyses stay out
of the tree the same way: a scratch directory, not a new tracked file. Every
skill's `$SCRATCH` means that directory, the session's scratch space.

## Local evidence at closeout

Every CodeTrial skill invocation closes with an auditable local report following
[codetrial-review/experience.md](../codetrial-review/experience.md). Include the
full examined commit, explicit complete/incomplete outcome, relevant paths,
skills actually used, confirmed/refuted findings and unresolved questions with
concrete checks. Record canonical history as used, consulted or rejected with
reasons and a concrete check against the examined head (or why code applicability
is not relevant). Open
and check the canonical source itself before assigning any of those roles.
Search snippets and earlier local agent reports are leads, never maintainer
rules. Open the owning contract rather than restating it from memory.

Use one report for a combined invocation, listing every skill used; a separate
subtask with another examined head needs its own report. A conversational
answer with no code findings still records its scope and evidence, with empty
case arrays. Incomplete work records its limits and next checks, not a successful
review. Review bundle preparation now automates pinned begin and bounded private
experience context; review closeout explicitly runs `review-lifecycle.py finish`
and retains durable pending data for an explicit retry on failure. Other skills
close with `experience-close.sh`, a wrapper over `experience record` that fills
only the fixed fields, including combined invocations. There
is no universal automatic hook or finalizer. Keep failed record attempts and
diagnostics and disclose them rather than claiming durable storage. This local write authorizes no GitHub write.

## Contracts that refuse a change

`CONTRIBUTING.md` names the owning documents and the rule that a document
refusing your change is the answer rather than an oversight. Two things it does
not say.

Never restate one of these rules from memory or from this file: a paraphrase
drifts, and the document is what binds.

And what being refused looks like. "Persist the finished session and regenerate
the report later" reads as an obvious improvement, and the caching rule in
`docs/provider-cost-and-degradation.md` refuses it. The move is to propose
changing that document on purpose, with the reason, or to leave it alone.

## Deleting a surface

In `CONTRIBUTING.md`, together with the compatibility promise it answers to.

## GitHub issues and pull requests

`CONTRIBUTING.md` has the register, the titles, the bodies, the redaction rule
and the branch rules. What follows is only what an agent needs on top of them.

Conversation may use the contributor's language; everything posted to GitHub is
clear English, with identifiers and error messages kept verbatim. The target is
`sysprog21/codetrial`; pass `--repo sysprog21/codetrial` to `gh`, because on a
fork or a copy the checkout's default resolves elsewhere.

A vulnerability report goes to the user to hand to a maintainer, never into a
public issue. A public thread may carry a request for a private contact and
nothing else.

Nothing is written to GitHub until the user has approved the exact text, by
saying yes to it or by dictating it and asking for it to be posted. The approval
covers the writes shown. Approving a PR submission also covers later fix pushes
to that branch and the review replies gh-submit makes while getting its checks
green; any other comment, edit, close or merge needs its own.

Never paste drafted text into shell source, where backticks and `$()` in a
quoted error would run. Write the body to a UTF-8 file in a scratch directory
with a file tool and pass `--body-file`; put the title in a file too and pass
`--title "$(cat "$SCRATCH/title.txt")"`. Read the result back, and after a
timeout check whether the write landed before retrying.

Drafting an issue or PR body, or planning a first contribution, is
[codetrial-contribute](../codetrial-contribute/SKILL.md). Reviewing the backlog
for duplicates and incomplete reports, and sweeping the open pull requests, is
[codetrial-triage](../codetrial-triage/SKILL.md). Approving a held workflow
run, resolving an outdated review thread, posting the fixed rebase request and
hiding the copies it supersedes need no per-thread approval of text; that
skill says when a sweep applies them without a separate confirmation. A bare
`/codetrial-triage` also posts, without a per-thread approval, the fixed-text
hygiene review ([pulls.md](../codetrial-triage/pulls.md#contributor-hygiene)), and the inline findings and similar-work notices
of its review of low and blocked PRs ([review-prs.md](../codetrial-triage/review-prs.md)), each verified at the
head first; the maintainer approved those forms by asking for them.

## Pull request branches and review replies

The branch rules and what a review thread may carry are in `CONTRIBUTING.md`.
Only "not `main`" binds a branch name; `fix/...`, `feat/...` and bare words
have all merged.

The addition here is why the thread stays thin: the commit body already carries
what and why, so a reply that re-summarizes the diff is telling the reviewer
what git is showing them. A reply opens with the fact. It carries no greeting,
thanks, apology ("Sorry for the delay"), self-introduction ("my first issue"),
or list of what the last push changed; maintainers called out each of those on
#60, #83, #94, #123, #185, #271 and #285. A counterexample to a review finding
is a correction in CONTRIBUTING.md's sense and belongs in its thread.
[collaboration.md](../codetrial-contribute/collaboration.md#review-evidence-including-bot-findings)
has how to answer a bot finding.

## Layout

`README.md` has the tree under "Repository layout" and wins on it. Three
directories it does not name matter as soon as you add a test:

```text
tests/unit/     Unit test bodies, compiled into src/ by `#[path]`
tests/common/   What the integration tests share
tests/stubs/    Fake executables the Python suites install, read by support.stub()
```

## No test bodies under src/

`src/` holds implementation. A file there may declare a test module, and may
carry a `#[cfg(test)]` item that a test needs to reach, but must not contain a
test body. (Rust idiom, lint suppression and file ceilings are in the
[Rust profile of codetrial-improve](../codetrial-improve/rust.md).)
The `src-test-bodies` gate in `scripts/test.sh` rejects test attributes
and inline test modules under `src/`; test-only helpers remain allowed.

```rust
#[cfg(test)]
#[path = "../tests/unit/gemini.rs"]
mod tests;
```

Three lines, and the body lives in `tests/unit/`, mirroring the path under
`src/`. `tests/unit/web/pool.rs` holds what `src/web/pool.rs` declares.

The indirection is what keeps both halves of the promise. A test under
`tests/*.rs` is an integration test: it links the plain rlib and can reach only
the public API, so moving these outright would have meant publishing internals
for the tests' benefit. Declared this way they are still unit tests (same
module path, `super::` still the containing module, private and `pub(crate)`
items still in reach), and none of it is visible to a dependent. The two halves
cannot share a module, which is why `tests/common/` exists and why the credential
predicate is deliberately written twice.

Four rules follow, and each of them cost something to learn:

- **Never name a file `tests/<dir>/main.rs`.** Cargo reads that as an
  integration-test target and compiles it a second time as a crate root, where
  every `super::` in it fails. `src/main.rs` declares `tests/unit/bin.rs` for
  this reason. Any other name under `tests/unit/` is inert to autodiscovery.
- **`include_str!` and `include_bytes!` resolve against the file that writes
  them**, so a test reading its own production source needs the path rewritten
  when it moves. Two such tests failed loudly on the move and two more kept
  passing while reading the test file instead of the source, which is worse.
- A second test module in one file keeps its name:
  `src/accounts/schema.rs` declares `tests/unit/accounts/schema.migration_tests.rs`.
- `tests/unit/` is not scanned by the gate's browser or Python lanes and has no
  `main.rs`, so nothing under it becomes a target on its own. It compiles only
  because something in `src/` names it.

Generated files live under `web/` but are owned by `problem-bank/` and by the
generators in `scripts/`; editing one by hand is a drift the gate catches.
codetrial-verify names the generators.
