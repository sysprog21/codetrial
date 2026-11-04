---
name: codetrial-verify
description: How a CodeTrial change is validated - scripts/test.sh as the credential-free gate, which generated artifacts have to be regenerated before it passes, the checks that need credentials or a browser and therefore sit outside it, the browser and mutation lanes, and how to bring the server up for a live look without taking the maintainer's port. Use before calling work done, when diagnosing failed GitHub Actions checks, when a gate fails on drift rather than on a bug, when adding a test, when a fix needs to be seen working in the app, or after editing a skill or anything a skill names (scripts/check-skill-refs.py). Reviewing a diff or a pull request is codetrial-review.
---

# Validating a CodeTrial change

The order of work around the gate, test first and small steps, is
[codetrial-change](../codetrial-change/SKILL.md); this skill is the gate itself.

One gate needs no credentials and is the thing to run:

```sh
./scripts/test.sh         # every gate, as CI's three lanes run them
./scripts/test.sh browser # one lane: rust, portable or browser
make check                # every gate, plus a live Gemini credential check
```

CI runs one `gates (<lane>)` job per lane in parallel: `gates (rust)`,
`gates (portable)` and, in three slices, `gates (browser 1/3)`,
`gates (browser 2/3)` and `gates (browser 3/3)`
(`BROWSER_SHARD=1/3 ./scripts/test.sh browser` runs one
locally). The `check` job only collects their
results, the independent `audit` job and the mutation jobs', so its log does
not say what failed: open the failing lane, `audit`, `plan-mutants` job or
`mutants` shard. On a pull request the portable lane checks the commit
messages first, and a bad message skips the rest of that lane only. A gate
names its lane where it is declared, one naming none is portable, and a test
proves the three lanes plus the independent audit cover the full run.
`cargo-audit` runs only in the default local gate; CI uses `scripts/audit.py`
for merge-relative PR checks and full main checks. See
[dependency security checks](../../../docs/development.md#dependency-security-checks).
Its regression suite is `tests/test_audit.py`.
Reproduce a failure with the same lane locally.

`scripts/test.sh` is a list of named gates that all run before it reports, so
one failure does not hide the next; the summary line names every gate that
failed. It covers `cargo fmt --check`, `clippy -D warnings`, `cargo test`,
`cargo doc --no-deps` (the Rust lane's `rustdoc` gate), the Rust style checker
(codetrial-rust), the Rust lane's Cargo-aware `workspace-lints` inheritance
audit, the Python unittest suites, the Node browser tests, ESLint,
`ruff check`, `shellcheck`, the generated-artifact drift checks, the hook suite,
the skill references, `cargo-audit`, `cargo-deny` and `actionlint`.

It is not offline. The `fetch-vendor` gate downloads missing assets named by a
`web/vendor/**/FETCH` manifest; it cannot restore committed vendor files.
`actionlint` publishes itself as a container, so that lane may reach for docker.
Both are checksum-pinned or version-pinned; neither needs a credential.

A lane whose tool is absent skips instead of failing. Every skip the gate can
see, its own, a formatter's in `scripts/indent.sh`, a Node browser test's and
a Python case's, appears under the final `not checked by this run:` summary.
Read it before claiming coverage; never treat the skip set or its size as
fixed. It cannot list a Rust test that skips itself, such as the retention test
without sqlite3: `cargo test` keeps a passing test's output to itself, so
locally that skip shows nowhere. With `CI=true` the summary fails the run, and
those Rust tests fail instead of skipping; CI installs every tool, so there a
skip means a check stopped running. The drift checks always run. That skipping
is why a green local run is weaker evidence than a green CI run, and so is what
this script leaves out: on a pull request CI also holds its own commit messages
to the rules and mutation-tests the diff in its own job. Release binaries for
three targets are built only after a merge to `main`, so no pull request
proves them.

The `indent` gate is the one that surprises people. `scripts/indent.sh --check`
copies the tree, runs the whole formatter chain over the copy, and diffs:
comment reflow with `commentflow`, then `rustfmt`, `ruff format` and `shfmt`.
Checking the composition rather than each tool is not a flourish. commentflow
puts a blank line before a comment inside a method chain and rustfmt takes
it straight back out, so `commentflow --check` alone can never be satisfied on
Rust. `make indent` runs the same script with `--write`, so the fix for a
failure is always that one command. Never pass `shfmt` a style flag; it reads
`.editorconfig`. Prettier sits beside the chain rather than in it: it shares no
file with the others, so the check runs it in place with `--cache`, alongside
the copy. Its file set is in `scripts/indent.sh` and includes the issue forms,
which ride along to be parsed. A formatter the files need is required
everywhere except the gate, which passes `--skip-missing` and lists the skip:
`make indent-check` is the same check failing on a missing tool, and the
pre-commit hook runs it as `--staged` against the index. A pull request whose
commits are all Dependabot's has its drift reported in CI rather than failed;
with no merge queue on main yet, it is fixed before merging or it fails main.

## Drift is the usual failure

Several trees are generated, and the gate compares the committed bytes against
what the generator would write now. A failure here is not a bug in your change;
it means the source moved and the output did not:

```sh
python3 scripts/gen-problems.py
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

These exercise live server, external-service, or browser flows and are run on
their own when the area they cover is touched:

One command per line: two names on one line runs the first and passes the
second as an argument it ignores. The entry requirements are:

```sh
scripts/browser-check.sh              # Playwright + Chromium; rust/dispatch also need LiveKit and Gemini credentials
scripts/server-check.sh               # cargo, node, curl; starts a server unless CODETRIAL_WEB_URL is set
scripts/gemini-check.sh               # GOOGLE_API_KEY(S) in the selected CodeTrial config
scripts/parity-check.sh                # the credentialed rust browser-check prerequisites
scripts/report-parity-check.sh         # the credentialed rust browser-check prerequisites
scripts/visual-parity-check.sh         # Playwright + Chromium; no service credentials
scripts/recording-provision-check.sh   # gcloud credentials and the CODETRIAL_RECORDING_* values checked at its start
scripts/recording-integration.sh       # --help lists per-phase credentials, tools and required --phase
scripts/interview-behavior-check.sh    # GOOGLE_API_KEY from the selected config; makes Gemini requests
scripts/evidence-replay-check.sh       # no credentials; the one cargo test the gate also runs, for a quick rerun
```

Read the script's validation or usage block before a credentialed run; it is
the source of truth for optional modes and the complete environment-variable
list.

CI additionally mutation-tests the diff: `plan-mutants` counts what the change
is worth and `mutants` runs `cargo-mutants` over it. A surviving mutant means a
line changed behavior with no test noticing, so the answer is a test, not a
retry.

## Browser investigation

Drive pages with the repository's Node Playwright dependency; no Python
Playwright environment and no second server wrapper. `scripts/browser-check.sh`,
whose validation block lists its `BROWSER_CHECK_FLOW` values, and
`scripts/visual-parity-check.sh` pick a free port and clean up after themselves;
for an exploratory session use the separate-port recipe in
[Seeing it work in the app](#seeing-it-work-in-the-app) and stop only the
server you started. LiveKit pages never go network-idle, so wait for the
specific control or state the action needs. Attach console and page-error
listeners before navigating, keep screenshots in `$SCRATCH` with credentials and
interview content removed, and look at a screenshot before claiming a visual
result. Never refresh `tests/golden/visual` merely to silence a difference;
establish that the new output is intended. A behavior change gets its
regression check in the existing lane.

## Failed GitHub Actions checks

The lanes and the aggregate `check` job are described above;
telling a stale base, a regression, a race and an environment limit apart is in
[collaboration.md](../codetrial-contribute/collaboration.md#make-ci-green-before-asking-for-review);
reading the job log, and why it is `--log`, is the recipe in
[pulls.md](../codetrial-triage/pulls.md#recheck-dependencies-and-substantive-review).
What this adds is one call that reads all of it at the PR's head:

```sh
.claude/skills/codetrial-verify/ci-failure.sh "$pr" > "$SCRATCH/ci-$pr.json"
```

A sweep reads the same diagnosis through `ci-cached.sh`, which takes the
same arguments, serves a finished one (nothing pending or held) again for the
same head within 5 minutes, and so reads each PR's job logs once for the
sweep and hygiene together; call `ci-failure.sh` itself for a live read.

Read its `failing` entries in this order. `current` false is history, not the
current failure: say so and stop there. `current` or `on_main` null means its
lookup failed, and `gates`, `errors` and `excerpt` null mean the job log could
not be read; check either with the raw calls below. `on_main` true means the
latest finished run on `main` fails the same job. Read whether the cause is
shared before asking the author for a fix: the PR `audit` compares added
findings, while main runs a full scan, so both can fail for different reasons.
Otherwise `gates` names what `scripts/test.sh` failed and `errors` the lines
that say why; rerun that lane locally with the same head. For historical runs
that still put `cargo-audit` in the portable lane, a failure on a PR whose base
predates the advisory fix is a stale base, answered by a rebase (#334), not a
code change; `stale_base` (defined in the script's header) says exactly that,
and the PR sweep lists such a historical PR in `rebase` on its own.
The independent `audit` job already compares the proposed merge with the base
tip: inherited findings are informational, while added findings and incomplete
dependency-change scans require investigation rather than an automatic rebase.
The aggregate `check` job only collects results, so its entry points at the
failing job it collected, as in the lanes section above. A check outside
Actions has only its link, and
`held` checks have no log until a maintainer approves the run.

The raw calls behind it, when one answer needs checking:

```sh
gh pr view "$pr" --repo sysprog21/codetrial --json headRefOid,files,changedFiles
gh pr checks "$pr" --repo sysprog21/codetrial --json name,state,bucket,link
gh run view "$run" --repo sysprog21/codetrial --json headSha,status,conclusion,jobs
gh run list --repo sysprog21/codetrial --workflow check.yml --branch main \
  --event push --status completed --limit 10 --json databaseId,conclusion
gh run view "$run" --repo sysprog21/codetrial --job "$job" --log
```

A run whose `headSha` is not the PR head is history, not the current failure.
`gh pr checks` exits non-zero when a check fails and still prints its rows. An
empty list after `gh auth status` fails is an access gap; a held run has no log
yet. A third-party check is reported by name and details URL. Reproduce with the
lane at the same head, fix the cause, run the lane and then the full gate, and
after an authorized push reread the head and checks: pending stays pending in
the report.

## The git hooks

`make hooks` installs the fast half of the gate at commit time, once per clone
and before the first commit there; how to read its output is in
[conventions](../codetrial-conventions/SKILL.md#commit-messages):
`scripts/git-pre-commit.sh` runs ESLint, `ruff check` and `shellcheck`, then
`scripts/indent.sh --staged`, over a checkout of the index, so an unstaged edit
neither fails a commit nor sneaks through one; a staged formatter setting
widens the formatting check to every file. No script of ours runs while a merge
is concluded, since it would execute the merged-in scripts. A formatter a staged file needs refuses
the commit when missing; a missing linter is named in the closing line instead
of reported as passed. Changes to hooks, the installer or the commit-log
checker also run the hook suite against that snapshot; the suite's scratch
repository sets `codetrial.hookSuite` so its nested commits do not launch it
again. Changes to skills or repository paths they can reference run
`scripts/check-skill-refs.py` against the snapshot when this skill exists.
Those checks include deletions. The hook does not build Rust, run the general
test suites or check generated-artifact drift; that is what the gate is for.
`scripts/git-commit-msg.sh` holds the message to the rules it prints with
`--rules`, `scripts/git-prepare-commit-msg.sh` splices the template above a
`commit -v` scissors line, and `scripts/git-pre-push.sh` replays the rules over
commits a rebase or an amend rewrote after the fact. Small topic heads without
[codetrial-review](../codetrial-review/SKILL.md) get an advisory note; large ones
require matching interactive evidence from `scripts/check-interactive-review.py`.
`scripts/hashchain.py` generates and verifies Change-Ids after the anchor in
`scripts/hashchain-base`. The post-commit hook repairs only checked candidates;
history repair is explicit and preserves the worktree. `tests/test_hashchain.py`
and `tests/test_interactive_review_hook.py` run in the `hashchain-tests` and
`interactive-review-hook-tests` gates, covering real hooks, stale receipts,
199/200 and 500/501 boundaries and durable lifecycle evidence.
A push to `origin` also refreshes the rolling `latest` and
`desktop-latest` tags first, bounded to a minute and skipped with a note when
neither `timeout` nor `gtimeout` exists; an unreachable `origin` refuses that
push. The installer adds a `+refs/tags/*latest:refs/tags/*latest` fetch refspec
so a plain pull refreshes them too ([development](../../../docs/development.md)
has the details).
`make hooks` installs every `scripts/git-*.sh`, so adding one there installs
itself. CI runs the same list over
a pull request's own commits, so the rules bind someone who never installed the
hooks as well.

`scripts/check-secrets.py` checks staged blobs before the pre-commit snapshot
is materialized, and pre-push checks every outgoing commit, including merges.
It compares selected provider-key formats and literal local credential values
without printing matches or invoking a provider. Pull-request CI repeats the
history check. `tests/test_check_secrets.py` exercises the scanner with real Git
objects; it runs as the `secret-check-tests` gate.

The hooks have their own suite. `scripts/test-git-hooks.sh` builds a scratch
repository, installs the hooks into it and drives every case: the messages that
must be rejected, the template splice above a `commit -v` scissors line, a
staged file failing while the same edit unstaged does not, and a push carrying a
commit that skipped the hook. It runs as the `git-hooks` gate, so editing a hook
without running it is caught here.

## Writing a test

No test code goes under `src/`; codetrial-conventions has that rule and the four
things that bite when it is applied carelessly. Which of the two kinds you are
writing follows from what the test needs to see:

- Reaching a private or `pub(crate)` item makes it a unit test. It goes under
  `tests/unit/`, mirroring the path under `src/`, declared from the `src/` file
  with `#[cfg(test)] #[path = "..."] mod tests;`.
- Reaching only the public API makes it an integration test, so it goes in
  `tests/*.rs` beside the suites already there.

Browser tests go in `tests/browser/*.test.js` under `node --test`, Python tests
in `tests/test_*.py` under `scripts/run-python-tests.py`, which runs a file's
cases across a thread pool; a suite added there must keep every case owning its
own sandbox, or it races. The compatibility promise is the versioned contract
in `docs/interview-contract-versions.md`; the prompt and report goldens in
`tests/golden/` are how a change to it gets noticed, so a diff there claims the
output changed on purpose, comes with that version bump, and is explained in
the commit body. The other fixtures there answer to the tests that own them.

A test that passes without running anything is the failure mode this tree has
already been bitten by, hence commits like "Prove an empty test run is not a
pass". Assert on the count as well as the content when a suite discovers its
own cases.

The quieter version is a test that runs and cannot fail: a refusal asserted
against a double that was scripted to refuse, or a hash compared against one the
test computed with the function under test. The check that separates them is
cheap and is the one to run before believing a new test: break the thing it
names, watch it fail, put it back.

Two shapes reviewers reject in browser tests. A test that reads source text
with a regex instead of driving the page passes against code that never runs
(#225, #266). And a test that asserts right after an action races the page:
a value set on the next animation frame, or a handler bound only after
`init()` finishes its top-level `await`, reads stale on a loaded CI runner and
fresh on a laptop (#226, #266, #269). Wait for the observable value, not a
timeout. A CI failure that does not reproduce locally may be such a race, a
stale base or an environment limit;
[collaboration.md](../codetrial-contribute/collaboration.md#make-ci-green-before-asking-for-review)
has how to tell them apart. A rerun is not the fix.

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

## Before saying it works

Each claim needs evidence from the head being pushed, gathered after the last
rebase or amend:

| Claim | Evidence | Not evidence |
| --- | --- | --- |
| fixed | the original reproduction now behaves | the code changed |
| tests pass | `./scripts/test.sh` at this head, its skips read | an earlier run |
| CI is green | `gh pr checks` passing, and each run's `headSha` from `gh run view` at this head | a local pass, or a run on an older head |
| review addressed | each open thread reread at this head | a pushed commit |
| reviewed | [codetrial-review](../codetrial-review/SKILL.md) at this head, its confirmed findings fixed or answered | a review of an older head, or the gate passing |
| the agent did it | `git diff` shows the change | the agent's summary |

"Should", "probably" and "seems" go out only with the check that would settle
them.

## Reviewing a change

A passing gate is not a review; reviewing a diff, one's own or an open pull
request, is [codetrial-review](../codetrial-review/SKILL.md).

A skill names gates, lanes, scripts, flags, make targets and documents, and
each one can move. The `skill-refs` gate runs `scripts/check-skill-refs.py`,
which prints every reference a skill makes that no longer resolves and every
check script this skill does not list, and rejects a SKILL.md whose
frontmatter `name` differs from its directory or whose `description` is empty;
run it on its own after changing either
side. It catches a rename, not a change of meaning, so a changed rule still
needs the skill reread. The scripts the skills run are tested in
`tests/test_skill_scripts.py`.

The explanation workbench has its own regression suite,
`tests/test_review_workbench.py`, run by the `review-workbench-tests` gate.
It checks answer durability, stale-session refusal, feedback invalidation and
local HTTP access boundaries, plus actual Git history with commits below and
at 200 lines whose final diff is small and binary/unknown coverage.
The `review-worker-tests` gate runs `tests/test_review_worker.py` with an
isolated fake CLI over real subprocesses: deduplication, explicit retry, stale
results, interrupted calls, ownership and shutdown. It needs no provider login.
The browser lane also runs `tests/browser/review-workbench.test.js` against the
real local server: browser submission to the waiting agent, feedback back to
the page, cross-tab conflicts, and the shared lobby themes.

Routing is the other half. Run `scripts/check-skill-routing.py` after changing
a description (its header says why), with `--skill` naming the edited skill
and each neighbor it hands off to (the near-misses run too), and the whole set
once before committing. Add a request that routed wrong to
`scripts/skill-routing-queries.json`. Fix a miss by having one description
claim the phrase and its neighbor disclaim it, then confirm with `--only`
naming the failing queries and `--runs 3`. For the same reason, do not install
another skill set into this checkout whose descriptions claim requests these
skills own.

## Local evidence at closeout

Before ending this invocation, follow the shared
[local evidence contract](../codetrial-conventions/SKILL.md#local-evidence-at-closeout)
and explicitly record its report using
[experience.md](../codetrial-review/experience.md). Include this skill in
`skills_used`; a combined invocation may use one report for its examined head.
Local experience is a lead to recheck, never a maintainer ruling.
