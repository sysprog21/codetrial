# Contributing to CodeTrial

[README.md](README.md) covers what CodeTrial is, how to build it, and how to run
it. [docs/development.md](docs/development.md) covers the test gate, the
formatter chain, the git hooks, and the generated files. This page covers what
neither does: how a change gets in, and the rules a reviewer would otherwise
have to say out loud every time.

Everything posted to GitHub is written in clear English, with identifiers and
error messages kept verbatim. The conversation that produced a change can be in
any language; the issue, the pull request, and the code cannot.

## Reporting a bug

Use the [bug report form](https://github.com/sysprog21/codetrial/issues/new/choose).
It asks for the facts a maintainer always needs, because asking for them
afterwards costs a round trip that usually never completes.

Before filing, search
[open and closed issues](https://github.com/sysprog21/codetrial/issues?q=is%3Aissue+sort%3Aupdated-desc).
If a thread already covers the symptom, add the new evidence there instead of
opening a second one.

A feature idea, a question, or a design objection goes in a blank issue, which
the chooser offers beside the form. Only bug reports get a form, because only
bug reports have a fixed set of facts a maintainer always needs.

Nothing public carries credentials, session tokens, candidate recordings, or
personal interview content. Sanitize logs and screenshots before attaching
them. This repository has no security mailbox and GitHub's private
vulnerability reporting is turned off, so there is no private channel to send a
report to. Do not put the details anywhere public: open an issue that says only
that you have a security report and asks for a private contact, and wait for
one.

## Before changing code

`docs/` holds decisions that are already made, and several of them refuse a
change that looks obviously right. Find the document that owns the area and
read the rule there first:
[provider cost and degradation](docs/provider-cost-and-degradation.md) owns what
an interview may spend and what may be cached, the
[observable delivery policy](docs/observable-delivery-policy.md) owns what a
report may say about a person, the
[recording contract](docs/recording-contract.md) owns what happens to candidate
media, and the rest of `docs/` owns its area the same way.

A document that refuses your change is the answer, not an oversight. The move
is to propose changing that document on purpose, with the reason, rather than
working around it.

A subcommand, flag, or helper that turns out to be redundant is deleted, along
with its call sites in the Makefile, the scripts, the README, and the tests. It
does not become an alias and it does not get a deprecation warning. The
compatibility promise here is the versioned contract in
[docs/interview-contract-versions.md](docs/interview-contract-versions.md), not
the convenience surface of the CLI. The prompt and report goldens in
`tests/golden/` are how a change to that contract gets noticed, not a promise of
their own: they are regenerated on purpose when a version is bumped. The other
fixtures there answer to the tests that own them.

## Tests

Every non-trivial change leaves a runnable check behind: the smallest thing
that fails if the change breaks.

`src/` holds implementation and must not contain a test body. A module declares
its tests and the body lives under `tests/unit/`, mirroring the path:

```rust
#[cfg(test)]
#[path = "../tests/unit/gemini.rs"]
mod tests;
```

Declared this way they are still unit tests, with `super::` and private items
in reach. A standalone integration target under `tests/` compiles as its own
crate and reaches only the public API, which would have meant publishing
internals for the tests' benefit. Never name such a file `tests/<dir>/main.rs`:
Cargo compiles that as a second crate root, where every `super::` in it fails.

The `src-test-bodies` gate in `scripts/test.sh` enforces it: a test attribute
anywhere under `src/`, or a `#[cfg(test)]` module with its body inline, fails.
Test-only helpers that are not modules stay allowed.

## Running the gate

```bash
make hooks           # once per clone; installs every scripts/git-*.sh hook
./scripts/test.sh    # the credential-free gate CI runs
```

`make hooks` installs a wrapper for each `scripts/git-*.sh`, so the push is
gated as well as the commit and a rebase that rewrote a message is caught before
it reaches GitHub. It exits 0 only when every hook is in effect; a hook of your
own already in the way, or a `core.hooksPath` shared with other repositories, is
left alone and reported with how to integrate it. Your own hook keeps running
once moved to `.git/hooks/<name>.local`, after ours.
`./scripts/install-git-hooks.sh --check` asks the same question without writing.
`cargo build` and `cargo check` run the same installer on every build, and
fail when the hooks cannot be put in effect, printing what needs you. Set
`CODETRIAL_SKIP_HOOKS=1` for a checkout that cannot take hooks.

Formatting is required of every contributor; only a pull request Dependabot
opened and wrote has formatting drift reported rather than failed. The pre-commit
hook refuses a commit whose staged files need a formatter you have not
installed, and `make indent-check` checks the tree the same way without
rewriting it.

The gate ends by naming any lane it skipped; read that rather than assuming
green means covered. [docs/development.md](docs/development.md) has
what each hook and lane does, which generated files have to be regenerated
first, and the checks that need credentials or a browser.

## Commit messages
Clear and descriptive commit messages are crucial for maintaining a transparent
history of changes and for facilitating effective debugging and tracking.
Please adhere to the guidelines outlined in [How to Write a Git Commit Message](https://cbea.ms/git-commit/).
1. Separate the subject from the body with a blank line.
2. Limit the subject line to 50 characters.
3. Capitalize the subject line.
4. Do not end the subject line with a period.
5. Use the imperative mood in the subject line.
6. Wrap the body at 72 characters.
7. Use the body to explain what and why, not how.

An example (derived from Chris' blog post) looks like the following:
```text
Summarize changes in around 50 characters or less

More detailed explanatory text, if necessary. Wrap it to about 72
characters or so. In some contexts, the first line is treated as the
subject of the commit and the rest of the text as the body. The
blank line separating the summary from the body is critical (unless
you omit the body entirely); various tools like `log`, `shortlog`
and `rebase` can get confused if you run the two together.

Explain the problem that this commit is solving. Focus on why you
are making this change as opposed to how (the code explains that).
Are there side effects or other unintuitive consequences of this
change? Here's the place to explain them.

Further paragraphs come after blank lines.

- Bullet points are okay, too

- Typically a hyphen or asterisk is used for the bullet, preceded
  by a single space, with blank lines in between, but conventions
  vary here

If you use an issue tracker, put references to them at the bottom,
like this:
Close #123
```

`scripts/git-commit-msg.sh` both prints the enforced list and enforces it, so
read it from there rather than from a copy that can drift:

```bash
./scripts/git-commit-msg.sh --rules
```

The `prepare-commit-msg` hook puts the same list in your editor. Subjects from
this tree's log read like this:

```
Bound a video frame by the memory it will take
Refuse a permission listing that may be truncated
Prove an empty test run is not a pass
```

A commit needs a body: the hook and CI refuse a subject alone. Merge commits are
exempt, and so are Dependabot's. A `fixup!`, `squash!` or `amend!` subject is
refused at commit, at push and in CI alike: fold the change into the commit it
fixes, with `git rebase -i --autosquash` for an older one. The body carries the
premise and the trade, usually in a single paragraph. This tree keeps its
detailed reasoning in the comment next to the code, so a body retelling the
mechanics duplicates that comment and then goes stale on its own.
The hook rejects the shape that does it outright: a body line opening with
`How:`, `Step:`, `Steps:`, `Changes:` or an `Implementation:` heading. Whether
what is left says anything is the part only a reviewer can judge.

Body lines wrap at 72 characters. The one exception is a token that cannot be
wrapped without breaking it: a link or a file name longer than a whole line may
run past the limit, and so may a trailer such as `Co-authored-by`, which git
reads only as one line. A link or file name that fits on a line of its own goes
there, and the sentence beside it never runs past.

A subject that only names a file, such as `Update web/app.js` or `Fix app.js`,
is refused. It says where the change is and not what it does, so the log, a
later `git blame` and a bisect tell the reader nothing, and the web editor's
default subject is the usual way one gets in. Say what behaves differently:
`Recheck setup after the capture picker closes`. A specific verb with a path
(`Remove web/old.js`, `Rename a.js to b.js`) still says what happened and is
fine, and so is any subject that goes on to say what changed.

A message carries no AI attribution: no `Co-Authored-By` naming an agent, no
`Generated with` footer, no `Assisted-by` trailer, no link to a tool. Such a
line says which tool ran, not whom to ask, and helps nobody collaborate. A human
co-author keeps the credit. The hook refuses it at commit, at push and in CI
alike. It recognizes a tool by the address agents commit with, or by a trailer
or footer that names the tool itself; a person who shares a tool's first name,
such as `Claude Shannon`, or works where one is made, is unaffected.

An issue a commit finishes is closed with `Close #N` on the last prose line,
before the generated Change-Id trailer;
`Closes #N` is accepted and closes it the same way. A `Refs #N` may appear in
the description, where it reads as context, and never as the last line, which
the hook refuses: a last line that closes nothing reads like the closing line,
and the next reader cannot tell whether the issue is done. An issue too large
for one change is split into sub-issues, each closed by one pull request,
instead of being touched by several that each say `Refs`.

Comments follow from the same rule: every line says something the code cannot.
The ASCII rule the hook enforces covers the commit message, and source comments
follow it by convention, with no em dash, typographic quote, arrow character, or
CJK.

## Pull requests

Open a pull request from a topic branch, never from `main`, including the
`main` of a fork. A pull request follows its head branch, so one opened from
`main` takes in every later push there and blocks the next change until it
merges. The pre-push hook refuses new work on a fork's `main` once upstream is
a remote in the clone, and CI fails a pull request opened from one either way.

Branch from an up-to-date `main`, keep one change per branch, and update the
pull request by pushing to that same branch, force-pushing after a rebase.
Syncing a fork's `main` with upstream is still allowed, since it carries only
commits upstream already has.

A title carries no category, type, or area prefix: no `[Bug]`, `[Feature]`,
`Bug:`, `feat:`, `fix(web):`, or `web:`. GitHub issue types do the classifying,
and brackets that are part of the text, such as `argv[0]`, stay. A pull request
title follows the commit subject rules above; a single-commit pull request
reuses its subject. An issue title names the symptom or the outcome instead:
`Interview does not resume after reconnecting`, not `[Bug] Interview broken`.

Issue and pull request bodies are read in a browser, so write each paragraph as
one line. The 72-column wrap is for commit messages only. A pull request that
finishes an issue ends with `Close #N` alone on the last line. A pull request
body has no `Refs #N`: a reference that closes nothing is easy to add to every
pull request, tells the reviewer little, and leaves open whether the issue is
finished. When the issue is too large for this change, split it into sub-issues
and close one with each pull request.

Working with Claude Code, run `/codetrial-review` on the committed branch
before opening the pull request. It reads what the gate cannot: the scope the
issue accepted, the contracts in `docs/`, and what maintainers already decided
about the same files. Fix what it confirms; its report stays out of the pull
request. The pre-push hook prints one line when the head being pushed has no
such review on record for small changes. Large topic branches require the
matching interactive explanation and durable review evidence before pushing.

Every ordinary contributor commit after the anchor in `scripts/hashchain-base`
must carry exactly one generated `Change-Id: I<40 hex digits>` trailer. The hooks compute a
SHA-256 link from the preceding Change-Id and the committed tree; pre-push and
CI recompute it. Clean two-parent GitHub integration merges do not need a
trailer: the verifier checks both histories and recomputes their merge tree.
Install the hooks with `make hooks` before committing. After
rewriting history, use `python3 scripts/hashchain.py rechain` on your work
branch, then review the new head again. This preserves code trees, staged and
unstaged changes, and adds no repair trailer. Rechaining changes commit IDs and
invalidates earlier head-specific review evidence. The anchor is a tracked
policy setting; changes to it need review like other contribution rules.
Agents must not skip the hooks (`--no-verify`, `core.hooksPath`, hand-built
commits); `.claude/settings.json` refuses those commands for Claude Code, and CI
checks the chain and the review requirement again.
It is public, and its hashes do not prove that hooks executed. CI pins the
policy and verifier to trusted history; the first installation needs explicit
maintainer approval of the exact verifier commit. See
[the hashchain trust boundary](docs/development.md#git-hooks).

Before opening a pull request, use the interactive review web page if any
commit has at least 200 added plus deleted reviewable lines, or the whole PR
has more than 500. Explain the change and its design choices in your own words,
and answer
questions grounded in the diff. Follow [the large-change explanation
requirement](docs/large-change-explanation.md) for counting, coverage, and what
belongs in the PR body. An agent-written summary or approval alone does not
meet this requirement; submission waits for your explanation and any material
corrections.
The pull request carries no review block. The `interactive-review` check measures
the change itself, and a change over the thresholds passes once a maintainer adds
the `explained` label.

A review thread carries a correction, a measurement, or nothing: no pasted
agent walkthroughs, no severity tables, no re-summarizing a diff that GitHub
already shows. Close an addressed thread with "Resolve conversation".
