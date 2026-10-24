# Development

The gate, the formatters, the hooks, the generated files, and where a slow run's
time actually goes. `make` targets and the one-line summary of each are in
[README.md](../README.md#development); everything here is the detail behind
them.

Selection admission and refresh behavior are specified in
[interview selection](interview-selection.md). The Strict browser regression tests use
the existing real media preflight with a fake media grant and room transport;
they do not exercise paid LiveKit, Gemini or Egress services.

## Dependency security checks

A new RustSec advisory must reach maintainers without making an unrelated
feature look broken. The independent `audit` job uses `scripts/audit.py`:

- Pull requests compare the tested merge commit against its first parent, the
  event's base tip. Merge groups compare against `merge_group.base_sha`.
  Never use the old branch's merge-base: that can hide a reintroduced weakness.
- Both lockfiles use one freshly fetched advisory database snapshot. Findings
  are compared by advisory ID, crate and version, so replacing one vulnerable
  dependency with another cannot pass by keeping the same count.
- An unchanged lockfile gets one informational scan. Inherited vulnerabilities
  and scan errors appear in the job summary without failing the PR. With a
  changed lockfile, new findings or an incomplete scan fail the audit. Removing
  a finding passes; upgrading to another affected version remains blocking.
- Pushes to main run a full audit. Builds and the rolling release wait for it,
  so a known vulnerability or scan error blocks publication. This can delay
  unrelated fixes while the previous release remains vulnerable; prioritize
  the dependency repair rather than bypassing the audit.

Checkout and tool-installation failures still fail the job. The informational
exception covers scanner and database errors after the tool is installed.

The separate `security audit` workflow runs daily and on manual dispatch from
main. It maintains one bot-authored issue titled `Dependency security audit`,
updating its findings or closing it after a clean scan, and reopening the same
issue when findings return so its subscribers keep following it. Changes to the
findings also leave a comment to notify subscribers, once even when a retry
follows a failed update; a database refresh alone does not.
An incomplete scan fails without closing the issue. Only this trusted-main job
has `issues: write`; fork PRs use read permissions and job summaries, without
comments or secrets. Maintainers should subscribe to the tracking issue and
workflow failures, assign an owner and a repair deadline, and link the repair
PR in the issue. A recurring scan is not a repair deadline.

Enable Dependabot alerts and security updates in repository settings; the
existing `.github/dependabot.yml` groups coupled security updates. RustSec
findings may precede GitHub's advisory import, so the daily audit remains
necessary. Review security PRs promptly and link them to the audit issue.
Do not add blanket advisory ignores. Any proposed exception needs a maintainer,
reason, expiry and tracking issue; no exception is implemented by this change.

`./scripts/test.sh` retains the full local `cargo audit` when installed. Its
three CI lanes omit that gate because the separate audit job owns CI policy.
To reproduce CI locally, run `python3 scripts/audit.py` for a full scan or
`python3 scripts/audit.py --base <base-tip-sha>` on the tested merge tree.
The scanner tests run with
`python3 scripts/run-python-tests.py tests/test_audit.py`.

## What the gate checks, and what it will not

`./scripts/test.sh` finishes by printing what it did not check, because several
lanes are optional and a run that skips them still exits 0. A checkout with none
of the tools below is a green gate over roughly a tenth of the browser suite and
no static analysis of the shell, the workflows, the Python, or the dependency
tree. The summary names each one; these are what they need.

```bash
npm ci                            # ESLint and Prettier, pinned
npx playwright install chromium   # the browser tests, the whole lobby flow
cargo install cargo-audit         # the dependency advisory scan
```

CI runs `scripts/test.sh rust`, `scripts/test.sh portable` and
`scripts/test.sh browser` in parallel, with fail-fast disabled so a failure in
one lane does not hide the others. Clippy and Rust tests share the Rust lane's
build cache; formatting, Python and shell checks run in the portable
lane; Node browser tests have their own lane, except the face-detector suite,
which needs the Rust binary and runs after its build. All lanes fetch and
verify vendor assets; each restores the vendored-asset cache, and only the
portable lane saves it, after its gates pass. Each lane has its own job
timeout: 45 minutes for Rust, 20 for the others. The Rust and browser lanes
clone only `HEAD`; the portable lane keeps full history for the commit-message
check. The Rust lane deletes the runner's unused toolchains only when less than
40 GB is free. Running `./scripts/test.sh` locally still runs every gate.

`check` is the only status a branch rule for main should require. It needs every
lane and the independent security audit to succeed, and on a pull request the
mutation plan and every shard too; shards may be skipped only when the plan
found no mutants. A push to main or a merge queue entry plans no mutants, so
`check` reports as soon as the lanes finish. The workflow also runs on
`merge_group`, so once main has a merge queue, it tests each pull request on top
of the main it will land on; two pull requests that pass against their own bases
can still break main together. A merge group saves no caches, since its queue
ref is gone once it lands. Mutation planning checks the Rust diff before
installing its tools, so a PR with no Rust changes starts no mutation shards and
pays no mutation-tool setup cost. Rust diffs still use the complete
diff-selected mutant set and the existing exclusions.

Every skip the gate sees joins its `not checked by this run:` summary: a gate's
own skip, a Node browser test's skip or todo, a missing formatter in
`scripts/indent.sh --skip-missing`, and a skipped Python case. A Rust test that
skips itself when sqlite3 or Playwright is missing is not in it, because `cargo
test` hides
a passing test's output. With `CI=true` the summary fails the run, and those
Rust tests fail rather than skip. Optional tools still skip on a local run. Each Node browser test has a
five-minute timeout, so a stalled test fails by name before its lane's job
timeout. Local Node versions without that option still run the browser tests.
Browser tests that wait out a timer jump the fake clock with
`page.clock.fastForward` rather than `runFor`, which also steps every frame
and interval in between and once made a single test take eighteen minutes.

The weekly avatar check is the `avatar` job in the same workflow, started only
by the schedule or a manual dispatch, and neither starts anything else. It
restores the Cargo, vendored-asset and Chromium caches through the same
composite actions under `.github/actions/` as the lanes, and saves none of them,
so the weekly job keeps no second copy. Chromium's system dependencies are still
installed on every fresh runner.

The gate picks its own compiler and Python, so running it directly builds with
the same Clang `make` would. On Linux x86_64 with no `CXX` set, it uses the checksum-pinned
Clang that `make` fetches into `target/clang`, fetching it just before the
first lane that compiles. It runs the Python lanes with the first Python 3.9 or
newer it finds; set `PYTHON` to name one. `ruff.toml` pins the rule set the gate
was written against, so a newer `ruff` with wider defaults does not fail an
unchanged tree.

Lint levels live in `Cargo.toml` rather than only in the gate's command line.
`[lints.clippy] all = "deny"` is why an editor running a bare `cargo clippy`
agrees with `make check` instead of reporting warnings for what the gate calls
an error, and `[lints.rust] unsafe_code = "forbid"` is what keeps this crate's
`unsafe` count at zero — `forbid` rather than `deny`, so a local `#[allow]`
cannot reintroduce it.

`shellcheck` and `ruff` come from the system package manager. `actionlint` is
not packaged as widely, so the gate falls back to its container image, pinned to
the version the workflow installs, whenever the binary is absent and a docker
daemon answers. CI installs all three, so what is optional locally is enforced
on a pull request — the summary exists so that a contributor knows which of the
two they are looking at. One lane needs working `javac` and `java`; the Java
class-harness fixture uses Java 8 syntax, and nothing else depends on it.

Where the time goes, measured on this repo rather than guessed, because the
answer is not the one a first look gives. Warm, the two Rust lanes are seconds:
`cargo clippy --all-targets` takes about five and building and linking the
eleven test binaries about nine, and the whole browser suite runs in under
twenty. What costs minutes is any change that invalidates the dependency graph,
a lockfile bump, a toolchain bump, a profile edit or a fresh checkout, because
rebuilding it means building the LiveKit SDK and libwebrtc from source. That is
why CI caches `target` keyed on the lockfile and links with mold, and it is why
a local run that suddenly takes twenty minutes has usually just had its cache
invalidated rather than got slower.

Trimming debug info, which is what buys CI 40% per relink, was measured here
and buys nothing: 7.5s against 8.1s on macOS, where the system linker rather
than mold does the work. It stays a CI setting for that reason.

## Formatting

`scripts/indent.sh` holds the formatter chain: comment reflow with
`commentflow`, then `rustfmt`, `ruff format` and `shfmt`, in that order
because the formatters have to have the last word over the reflow. `make
indent` runs it with `--write`, `make indent-check` and the gate with
`--check`, against a copy of the tree so a check never rewrites what it is
judging, and the pre-commit hook with `--staged`, against a checkout of the
index. The file lists, the vendor exclusion and the order live only there.
`shfmt` takes its style from `.editorconfig` and is passed no style flags
anywhere.

A formatter that the selected files need is required: `make indent`, `make
indent-check` and the hook fail on a missing one and say how to install it,
so a commit that touches only Rust does not need Prettier, but one that
touches JavaScript does. Only the gate passes `--skip-missing`, which skips the
lane, ends with `Formatting clean for the lanes that ran; not checked:` and
the tools it skipped instead of `Formatting clean.`, and joins the
gate's summary, which CI fails on. CI reports rather than fails formatting
drift on a pull request Dependabot opened and wrote, because it can bump
Prettier but cannot run `make indent`. Nothing fails it after that until main
has a merge queue, so whoever merges such a pull request runs `make indent` on
it first, or the push to main and every later pull request fail on the drift.

`npm ci` installs the pinned Prettier version used for HTML, JavaScript (`.js`,
`.mjs` and `.cjs`) and the issue forms under `.github/ISSUE_TEMPLATE/`. The
forms ride along because Prettier is the only lane that parses their YAML, for
the reason `scripts/indent.sh` gives. It checks syntax and not the form schema,
so a mistyped key or an unknown element `type` formats clean and still breaks
the issue chooser: only GitHub, or the published schema at
`json.schemastore.org`, judges that.

Prettier's style lives in `.prettierrc.json`; `.prettierignore` excludes
vendored and generated files. Prettier shares no file with the rest of the
chain, so the check runs it in place rather than on the copy, alongside the
chain, with `--cache` so a rerun skips unchanged files. The generated
problem-card section in `web/index.html` has a `prettier-ignore` marker so the
rest of the page can be formatted without changing the generator's output. As
with the other formatter lanes, an absent Prettier is reported and skipped
in the gate; CI installs it through `npm ci`.

The commits that only reformatted are listed in `.git-blame-ignore-revs`, so
`git blame` skips them. GitHub reads the file on its own; a local blame needs
`git config blame.ignoreRevsFile .git-blame-ignore-revs` once per clone.

## Comments that count things

Comments here carry the reasoning, deliberately, and that is not the part worth
economising on. What has gone wrong repeatedly is narrower: a comment that
states a count of code, "all three routes", "written out twice", "five
caveats". Nothing checks those, the code moves, and the comment is then a
confident false statement sitting next to what it describes. One recent branch
spent twelve of its eighteen commits correcting claims an earlier commit's
prose had made.

Two shapes are safe and are the ones to reach for. A count in the past tense is
history and cannot go stale: "seven modules were each restating this" stays
true after an eighth arrives. A count a test already pins is checked by that
test, so `ReplayKind::ALL` being `[Self; 6]` makes "six kinds" safe to write.

A present-tense count of anything else is a liability. Either drop the number,
naming the things instead, or move it somewhere a run can fail on it.

`.cargo/mutants.toml` is the second shape. Its header declares how many
functions the mutation gate has been told not to judge, and `scripts/test.sh`
compares that against the list, so an entry added without moving the number
fails the gate. The number is not the point: each exclusion is defensible on its
own and the list only ever grows, and a gate that judges steadily less is the
thing worth putting in front of a reviewer. Removing an entry, by making the
function reachable from `cargo test`, is the direction it should travel.

## Git hooks

`scripts/test-git-hooks.sh` drives all four hooks against a scratch repository
and runs in the gate, so a hook that stops rejecting fails here rather than on
somebody's next commit.

`make hooks` installs a wrapper in `.git/hooks` for each `scripts/git-*.sh`
that resolves the active worktree's copy, then verifies each is the current
wrapper and executable, and exits nonzero otherwise. A wrapper names itself on
its second line, so one an older install wrote is reported `STALE` and
rewritten. A hook it did not write is left in place and reported as `FOREIGN`,
and the install exits nonzero. Once you move it to `<name>.local` and run
`make hooks` again, it runs after ours with the same arguments and stdin, and
`make uninstall-hooks` moves it back. A `core.hooksPath` outside the
repository is reported as `SHARED` and nothing is written there. A checkout
from before the scripts existed has nothing to run and commits as before.
`./scripts/install-git-hooks.sh --check` runs the verification alone.

`build.rs` runs that check whenever the build script reruns and the install
only when it fails. The script runs on the first `cargo build`, `cargo check`
or rust-analyzer pass in a clone or worktree, and again when the installer or
the hooks directory changes, so a deleted hook comes back on the next build.
A refusal reaches the terminal as a Cargo warning and never fails the build.

The hooks run the `scripts/` of the checkout they fire in, as `build.rs` does
on a build, so review a branch before committing on it as you would before
building it. No script of ours runs while a merge is concluded: by the time
its commit-msg or prepare-commit-msg fires the incoming tree is written, and
running it would make `git pull` execute what it pulled, so the wrapper skips
it, and there is no pre-merge-commit hook. CI checks a pull request's content.

The pre-commit hook runs ESLint, `ruff check` and `shellcheck`, then
`scripts/indent.sh --staged`, over a checkout of the index, so an unstaged edit
neither fails a commit nor passes one, and it never stages anything itself.
The formatting check is what `make indent` would change in the staged files,
and in every file when the commit changes a formatter setting (`.editorconfig`,
`rustfmt.toml`, `ruff.toml`, the Prettier config or the version pinned in
`package.json`, or `scripts/indent.sh`). The edition comes from `Cargo.toml`
for both, and `rustfmt.toml`, empty of settings, keeps a `~/.rustfmt.toml` out.
Paths are read with `core.quotePath` off, so a name past ASCII is checked under
its own name; one git still quotes, or one with a blank, is refused by name
rather than skipped. A file added with `--intent-to-add` carries no content
into the commit and is not judged. A missing formatter refuses the commit; a
missing linter is skipped and named in the closing line. The commit-msg hook
holds the subject to 50 characters and the body to 72, imperative and ASCII.
The pre-push hook
replays the same message rules over commits a rebase or an amend rewrote after
the fact, and CI runs them over a pull request's own commits.

## Releases and the embedded web tree

The [prerelease](install.md) is published by the `build` and `release`
jobs in `.github/workflows/check.yml`, which run only on a push to `main`. No
repository secrets are involved: the macOS binary ships with the linker's ad-hoc
signature and nothing else, so a fork builds the same artifacts this repository
does. Developer ID signing and notarization would need a paid Apple Developer
Program membership, and the download instructions clear quarantine instead.

The desktop app is released separately, from `.github/workflows/desktop.yml`
when a `desktop-vX.Y.Z` tag is pushed, and never touches the CLI's `latest`;
[desktop.md](desktop.md) has the procedure and what that workflow checks.

The embed falls back per file rather than wholesale, so a partially populated
web tree is filled in from the built-in copy rather than 404ing. Assets on disk
win where they exist. That root is `web/` relative to the working directory
unless `CODETRIAL_WEB_DIR` names another, so running the binary from a checkout
picks up edits with no environment variable set.

## Generated files

`web/problems/`, `web/judges/`, and the problem cards in `web/index.html` are
generated from `problem-bank/`. Regenerate them after editing that source, or
the gate fails on the drift:

```bash
python3 scripts/gen-problems.py
python3 scripts/gen-problem-cards.py
```

The generated Rust tables include scenario metadata, topics, private guides,
and the private rubric in `src/agent/problem_rubrics.rs`. Do not edit a
generated table directly. [Adding a problem](adding-a-problem.md) describes
the source files, required checks, and the difference between an imported and
an original exercise.

`scripts/top-interview-150.json` records which problems the study plan asks
for. Refresh it from LeetCode with:

```bash
python3 scripts/gen-problems.py --sync-study-plan
```

The sync refuses to write when the plan and `problem-bank/` disagree, naming
the imported problems each side is missing. Port those first. Original
exercises are deliberately outside the plan, so sync and drift checks leave
them alone. `--check` holds the committed manifest to the same imported set,
so drift fails the gate offline.

Two commands cover the porting. `--plan-drift` asks LeetCode what changed
without writing anything, and `--scaffold SLUG` prints the `problems.json` and
`judges.json` entries for one problem, filled in as far as LeetCode's metadata
reaches:

```bash
python3 scripts/gen-problems.py --plan-drift
python3 scripts/gen-problems.py --scaffold reverse-linked-list-ii
```

It stops where it has to. The statement, examples and constraints stay empty
because the fetcher never asks LeetCode for problem prose, and each judge case
carries its input without an expected value, because `exampleTestcases` is
inputs only. Both are written by someone who has read the problem.

A ported problem also needs its entry in `problem-bank/variants.json`, and the
generator refuses to run without one. The page shows a scenario written around
the same contract instead of the published problem, and the interviewer holds
the rest. Each variant has a `title` and a `brief`, which the page shows; a
private `contract` the interviewer judges against; one or two `examples`, each
naming a judge case by index and not all of them published ones;
`clarifications` answered only when asked, with the constraints and edge-case
policies the brief leaves out; `followUps` for after a tested solution; and
three `hints`, a nudge, a direction and the key step.

A function problem also declares a new `entry` and a class problem a new
`className`, since the published name is as recognisable as the title. Either
may declare `parameters` where a parameter name gives it away, and `terms` for
any other word a starter snippet carries, such as a comment naming the
published node type. The bank keeps LeetCode's names; the generator puts the
variant's in place in the judge and starter code it writes, and refuses a
variant whose title, brief, contract, examples, constraints, starter code or
judge cases still name the published problem.

The browser knows a problem only by its page name, made from the scenario
title: the interview URL, the page and judge files, the token request and saved
history all use it, and the server resolves it to the problem. Each page names
its published title once, as `source`, which the interview shows in small print
beside the scenario so the problem can be found again afterwards. The published
id is in one file, `web/problem-pages.json`, which the browser fetches only to
open a link that still carries a published id, to read history saved before
pages had names, or when the candidate asks the lobby to show the titles on the
cards. The title reaches the live interview page only: saved history, the
downloaded report and the replay keep the scenario's.

None of this is secrecy. The page map, each page's `source` and every judge
are served to anyone who asks, and a candidate with developer tools can map a
scenario back to its published problem and read its test cases. The disguise
exists so an honest candidate meets the problem the way an interview poses it;
nothing that must hold against a determined one, integrity checks included,
may rely on it.
Keep a variant's title stable once it ships: a retired page name opens the
default exercise, with a note saying so.

`problem-bank/guides.json` holds optional solution notes for the report
reviewer, with the license they are used under. The generator refuses a note
for a problem the bank does not have, or one still carrying page furniture from
the import, and writes `src/agent/problem_guides.rs`. Only the report prompt
reads them.

## Checks outside the gate

Whether the interviewer actually follows the live prompt, rather than whether
the prompt says the right things, needs a Gemini key. The check scripts a
candidate through three problems against a text model given the same
instructions, greeting and tools, and fails on a named source, a volunteered
limit, an unanswered size question, or a hint that goes past the rung it was
served:

```bash
scripts/interview-behavior-check.sh
BEHAVIOR_PROBLEMS=3sum,lru-cache scripts/interview-behavior-check.sh
```

A free key allows fifteen requests a minute, so the check waits out rate
limits; three problems take about two minutes.

`BEHAVIOR_CHECK_MODE=injection` runs only the instruction-injection probe. It
takes an explicit call cap and a token ceiling that can overshoot by one call,
and needs an isolated config and a journal path outside the repository;
`tests/fixtures/report-eval/README.md` describes it.

The end-to-end browser check additionally needs Playwright and Chromium:

```bash
npm ci && npx playwright install chromium
scripts/browser-check.sh
```

`BROWSER_CHECK_FLOW=soak` is the long-conversation acceptance, and the only
check that covers an interview outliving one Gemini socket. It needs real
credentials and runs for eleven minutes:

```bash
BROWSER_CHECK_FLOW=soak BROWSER_CHECK_AGENT=dispatch scripts/browser-check.sh
```

The duration defaults to 660 seconds, past the Live API's ten-minute connection
cap, because a soak that stops short of the cap crosses no handover and proves
only that the room stayed up. `BROWSER_CHECK_SOAK_SECONDS` lengthens it and
belongs to this flow alone; naming it on another one fails rather than being
ignored, and so does a value under the cap.

Presence is the weakest of the four things it judges, because the interviewer
stays in the room through every way this can go wrong. So the flow also
requires, from the runtime log and the transcript panel:

- Gemini asked for the restart, which is the proactive path. Reaching a new
  socket by way of the close resumes too, so a check that asked only whether it
  resumed would pass with that path dead.
- The replacement resumed rather than restarting cold. A cold one keeps the
  interviewer in the room having lost the conversation.
- No degraded restart at any point, reported where it happened.
- The transcript grew after the handover. Jim nudges an idle candidate, so
  turns keep arriving without the browser having to speak.

Checks that need credentials or a running service stay out of the gate and are
run on their own: `scripts/server-check.sh`, `gemini-check.sh`,
`parity-check.sh`, `report-parity-check.sh`, `visual-parity-check.sh`,
`recording-integration.sh`, and `recording-provision-check.sh`.
