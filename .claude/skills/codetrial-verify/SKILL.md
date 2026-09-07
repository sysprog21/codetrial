---
name: codetrial-verify
description: How a CodeTrial change is validated - scripts/test.sh as the offline gate, which generated artifacts have to be regenerated before it passes, the checks that need credentials or a running service and therefore sit outside it, the browser and mutation lanes, and how to bring the server up for a live look without taking the maintainer's port. Use before calling work done, when a gate fails on drift rather than on a bug, when adding a test, or when a fix needs to be seen working in the app.
---

# Validating a CodeTrial change

One gate runs offline and is the thing to run:

```sh
./scripts/test.sh     # the gate CI's `check` job runs
make check            # the same, plus a live Gemini credential check
```

`scripts/test.sh` is a list of named gates that all run before it reports, so
one failure does not hide the next; the summary line names every gate that
failed. It covers `cargo fmt --check`, `clippy -D warnings`, `cargo test`, the
Python unittest suites, the Node browser tests, ESLint, `ruff check`,
`shellcheck`, the generated-artifact drift checks, the hook suite, `cargo-audit`
and `actionlint`.

Five of those lanes are optional, and only those five: ESLint, `ruff`,
`shellcheck`, `cargo-audit` and `actionlint` say they skipped when the tool is
absent rather than failing. The drift checks sitting between them in the output
always run. That skipping is why a green local run is weaker evidence than a
green CI run, and so is what this script leaves out: CI also holds a pull
request's own commit messages to the rules, mutation-tests the diff in its own
job, and builds release binaries for three targets.

The `indent` gate is the one that surprises people. `scripts/indent.sh --check`
copies the tree, runs the whole formatter chain over the copy, and diffs:
comment reflow with `commentflow`, then `cargo fmt`, `ruff format` and `shfmt`.
Checking the composition rather than each tool is not a flourish. commentflow
puts a blank line before a comment inside a method chain and `cargo fmt` takes
it straight back out, so `commentflow --check` alone can never be satisfied on
Rust. `make indent` runs the same script with `--write`, so the fix for a
failure is always that one command. Never pass `shfmt` a style flag; it reads
`.editorconfig`.

## Drift is the usual failure

Several trees are generated, and the gate compares the committed bytes against
what the generator would write now. A failure here is not a bug in your change;
it means the source moved and the output did not:

```sh
python3 scripts/gen-problems.py        # web/problems, web/judges
python3 scripts/gen-problem-cards.py   # the problem cards in web/index.html
node scripts/gen-wire-fixtures.mjs     # browser/agent wire fixtures
node scripts/gen-recording-fixtures.mjs
python3 scripts/gen-calibration-fixtures.py
```

Each takes `--check`, which is the form the gate runs. Edit `problem-bank/`,
never the files under `web/problems/` or `web/judges/`. `scripts/gen-problems.py
--sync-study-plan` refuses to write while the plan and `problem-bank/` disagree
and names what each side is missing, so port those first.

## What is deliberately outside the gate

These need credentials, a network service, or a browser, and are run on their
own when the area they cover is touched:

```sh
scripts/browser-check.sh            # needs npm ci and playwright chromium
scripts/server-check.sh             # needs a running server
scripts/gemini-check.sh             # needs GOOGLE_API_KEY
scripts/parity-check.sh scripts/report-parity-check.sh
scripts/visual-parity-check.sh
scripts/recording-integration.sh scripts/recording-provision-check.sh
```

CI additionally mutation-tests the diff: `plan-mutants` counts what the change
is worth and `mutants` runs `cargo-mutants` over it. A surviving mutant means a
line changed behavior with no test noticing, so the answer is a test, not a
retry.

## The git hooks

`make hooks` installs the fast half of the gate at commit time:
`scripts/git-pre-commit.sh` runs `rustfmt`, ESLint, `ruff` and `shellcheck`
over a checkout of the index, so an unstaged edit neither fails a commit nor
sneaks through one, plus `commentflow --check` and `shfmt -d` on staged shell.
It does not build, test or check generated-artifact drift; that is what the
gate is for. `scripts/git-commit-msg.sh` holds the message to the rules in
codetrial-conventions, and `scripts/git-pre-push.sh` replays them over commits
a rebase or an amend rewrote after the fact. CI runs the same list over a pull
request's own commits, so the rules bind someone who never installed the hooks
as well.

The hooks have their own suite. `scripts/test-git-hooks.sh` builds a scratch
repository, installs the hooks into it and drives every case: the messages that
must be rejected, the template splice above a `commit -v` scissors line, a
staged file failing while the same edit unstaged does not, and a push carrying a
commit that skipped the hook. It runs as the `git-hooks` gate, so editing a hook
without running it is caught here.

## Writing a test

No test code goes under `src/`; codetrial-conventions has the rule and the
four things that bite when it is applied carelessly. Which of the two kinds you
are writing is decided by what the test needs to see:

- Reaching a private or `pub(crate)` item makes it a unit test. It goes in
  `tests/unit/`, mirroring the path under `src/`, and the `src/` file declares
  it with `#[cfg(test)] #[path = "..."] mod tests;`. Adding a new one means
  adding that declaration too, or the file compiles and the test never runs.
- Reaching only the public API makes it an integration test, and it goes in
  `tests/*.rs` beside the suites already there. Those link the plain rlib, so a
  `#[cfg(test)]` item is invisible to them: the two halves cannot share a
  module, which is why `tests/common/` exists for what the integration tests
  share and why the same credential predicate is deliberately written twice.

Browser tests go in `tests/browser/*.test.js` under `node --test`, Python tests
in `tests/test_*.py` under `scripts/run-python-tests.py`, which runs a file's
cases across a thread pool; a suite added there must keep every case owning its
own sandbox, or it races. The golden fixtures in `tests/golden/` are the
compatibility contract: a diff there is a claim that the observable output
changed on purpose, and it belongs in the commit body.

A test that passes without running anything is the failure mode this tree has
already been bitten by, hence commits like "Prove an empty test run is not a
pass". Assert on the count as well as the content when a suite discovers its
own cases.

The quieter version of that is a test that runs and cannot fail, and a sweep of
this suite found every shape of it worth knowing: a refusal asserted against a
double that was *scripted* to refuse, so the script answered and not the code; a
hash compared against one the test computed with the function under test; a
traversal guard asked for files that did not exist, so "not found" read as
"refused"; a `/enforce/` regex over a seventy-line document; and a module's whole
wiring pinned by source-text matches that a rename walked straight past. The
check that separates them is cheap and is the one to run before believing a new
test: break the thing it names, watch it fail, put it back.

## Seeing it work in the app

When a fix needs a live look, build the release binary and start the server
yourself, then say it is ready to test. Do not hand over a command to run.

```sh
make build
./target/release/codetrial web --web-addr 127.0.0.1:3100
```

Port 3000 is the maintainer's own instance. Never bind, restart or kill it;
pass `--web-addr` with another port and name that port in the report. Running
from the checkout picks up `web/` edits with no environment variable set.
