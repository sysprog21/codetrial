# Development

The gate, the formatters, the hooks, the generated files, and where a slow run's
time actually goes. `make` targets and the one-line summary of each are in
[README.md](../README.md#development); everything here is the detail behind
them.

## What the gate checks, and what it will not

`./scripts/test.sh` finishes by printing what it did not check, because several
lanes are optional and a run that skips them still exits 0. A checkout with none
of the tools below is a green gate over roughly a tenth of the browser suite and
no static analysis of the shell, the workflows, the Python, or the dependency
tree. The summary names each one; these are what they need.

```bash
npx playwright install chromium   # ~30 browser tests, the whole lobby flow
npm install                       # eslint
cargo install cargo-audit         # the dependency advisory scan
```

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
`commentflow`, then `cargo fmt`, `ruff format` and `shfmt`, in that order
because the formatters have to have the last word over the reflow. `make
indent` runs it with `--write` and the gate runs it with `--check`, against a
copy of the tree so a check never rewrites what it is judging. `shfmt` takes
its style from `.editorconfig` and is passed no style flags anywhere.

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

`make hooks` installs wrappers in `.git/hooks` that resolve the active
worktree's `scripts/git-*.sh`. The pre-commit hook
runs `rustfmt`, ESLint, `ruff`, `shellcheck`, `shfmt` and `commentflow` over a
checkout of the index, so an unstaged edit neither fails a commit nor passes
one; the commit-msg hook
holds the subject to 50 characters and the body to 72, imperative and ASCII.
Whatever is missing locally reports itself as skipped. The pre-push hook
replays the same message rules over commits a rebase or an amend rewrote after
the fact, and CI runs them over a pull request's own commits.

## Releases and the embedded web tree

The [prerelease](install.md) is published by the `build` and `release`
jobs in `.github/workflows/check.yml`, which run only on a push to `main`. No
repository secrets are involved: the macOS binary ships with the linker's ad-hoc
signature and nothing else, so a fork builds the same artifacts this repository
does. Developer ID signing and notarization would need a paid Apple Developer
Program membership, and the download instructions clear quarantine instead.

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
the prompt says the right things, needs a Gemini key or a local model. The
check scripts a candidate through three problems against a text model given
the same instructions, greeting and tools, and fails on a named source, a
volunteered limit, an unanswered size question, or a hint that goes past the
rung it was served:

```bash
scripts/interview-behavior-check.sh
BEHAVIOR_PROBLEMS=3sum,lru-cache scripts/interview-behavior-check.sh
```

A free key allows fifteen requests a minute, so the check waits out rate
limits; three problems take about two minutes.

Against a local model, point it at `scripts/gemini-shim.py` the way the report
is pointed. The key is still read but goes no further than the shim, which
ignores it, so any non-empty value does when the config file has none:

```bash
CODETRIAL_GEMINI_REST_BASE=http://127.0.0.1:8090 GOOGLE_API_KEY=local \
    scripts/interview-behavior-check.sh
```

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
