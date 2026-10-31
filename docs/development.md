# Development

The gate, the formatters, the hooks, the generated files, and where a slow run's
time actually goes. `make` targets and the one-line summary of each are in
[README.md](../README.md#development); everything here is the detail behind
them.

Selection admission and refresh behavior are specified in
[interview selection](interview-selection.md). The Strict browser regression tests use
the existing real media preflight with a fake media grant and room transport;
they do not exercise paid LiveKit, Gemini or Egress services.

## Account database writes

Each server keeps one SQLite connection behind a mutex and runs account queries
on the blocking pool. The mutex serializes that server's queries; it does not
protect against another server using the same database.

Read-then-write account operations use `Accounts::transaction`, which reserves
SQLite's writer with `BEGIN IMMEDIATE` before reading. Report saves enforce the
`MAX_REPORTS_PER_USER` quota and ownership check in the same transaction as the upsert.
Interview creation reuses pending consent before applying the `MAX_INTERVIEWS_PER_USER`
quota, with no interleaving claim or insert from another connection. Session
creation, expired-account sweeps, delivery claims and retry scheduling, and
recording tombstones use the same commit-or-rollback helper. Single-statement
room claims, releases and deletes remain atomic without it.

Replay ingestion keeps its ownership precheck outside the writer transaction;
replay snapshots use a deferred read transaction so they do not reserve the
writer.

A competing writer waits up to the connection's five-second busy timeout, then
returns a database error. These transactions contain only local database work;
network calls must stay outside them. No schema migration is required.
Regression checks live in `tests/unit/accounts/schema.migration_tests.rs`.

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
- Pushes to main run a full audit. The rolling release waits for it (the builds
  run beside it), so a known vulnerability or scan error blocks publication. This can delay
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
cargo install cargo-deny          # dependency licenses and sources, deny.toml
sudo apt install libwebkit2gtk-4.1-dev   # Linux: clippy and tests of desktop/
```

CI runs `scripts/test.sh rust`, `scripts/test.sh portable` and
`scripts/test.sh browser` in parallel, with fail-fast disabled so a failure in
one lane does not hide the others. The browser lane runs as three jobs, each
with a `BROWSER_SHARD` slice (`1/3`, `2/3`, `3/3`) that splits the suites by
source size; set it locally to run one slice. Clippy and Rust tests share the
Rust lane's build cache; the desktop app's clippy and tests run in the same
lane against their own lockfile and cache. The Rust lane installs both crates'
system packages in one apt transaction. Formatting, Python and shell checks
run in the portable lane; Node browser tests have their own lane, except the
face-detector suite, which needs the Rust binary and runs after its build. All
lanes fetch and
verify vendor assets; each restores the vendored-asset cache, and only the
portable lane saves it, after its gates pass. Each lane has its own job
timeout: 45 minutes for Rust, 20 for the others. The Rust and browser lanes
clone only `HEAD`; the portable lane keeps full history for the commit-message
check. The Rust lane deletes the runner's unused toolchains only when less than
40 GB is free. Running `./scripts/test.sh` locally still runs every gate.

On a busy local host, `RUST_TEST_THREADS=4 ./scripts/test.sh rust` limits
concurrent server launches in the Rust suites. The browser compiler fixtures
also bound their concurrent jobs so candidate programs get CPU before their
execution deadlines. Editor-history fixtures set selections directly and use
the browser's platform modifier; native dragging includes a press-and-hold
before moving selected text.

`check` is the only status a branch rule for main should require. It needs every
lane and the independent security audit to succeed, and on a pull request the
mutation plan and every shard too; shards may be skipped only when the plan
found no mutants. A push to main or a merge queue entry plans no mutants, so
`check` reports as soon as the lanes finish. The workflow also runs on
`merge_group`, so once main has a merge queue, it tests each pull request on top
of the main it will land on; two pull requests that pass against their own bases
can still break main together. A merge group saves no caches, since its queue
ref is gone once it lands, and the cargo build caches are saved from main only:
a pull request's copy is readable by that pull request alone, and the
repository's 10 GB limit was full of them. Mutation planning checks the Rust diff before
installing its tools, so a PR with no Rust changes starts no mutation shards and
pays no mutation-tool setup cost. Rust diffs still use the complete
diff-selected mutant set and the existing exclusions.

Every skip the gate sees joins its `not checked by this run:` summary: a gate's
own skip, a Node browser test's skip or todo, a missing formatter in
`scripts/indent.sh --skip-missing`, and a skipped Python case. A Rust test that
skips itself when sqlite3 or Playwright is missing is not in it, because `cargo
test` hides
a passing test's output. With `CI=true` the summary fails the run, and those
Rust tests fail rather than skip. Optional tools still skip on a local run.
Browser files run two at a time with a ten-minute timeout per file (Node
also applies that limit to individual tests). The editor history suite bounds
each interaction at thirty seconds separately, so adding passing cases cannot
consume a single interaction's budget. Local Node versions without the timeout
or concurrency option still run the browser tests. `npm test` uses the same
browser lane as CI, including vendor verification and skip reporting; the
server-tier face detector suite stays with the Rust lane, which builds the
server it needs.
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
first lane that compiles. It runs the Python lanes with the first Python 3.11 or
newer it finds, because the Rust style checker uses the standard-library TOML
parser; set `PYTHON` to name one. An explicitly named older interpreter is
refused before any gate runs, except for the `browser` lane, which runs no
Python gate. The Rust lane requires Python for its workspace inheritance audit
but never installs the instructor tool's packaging environment. Recreate an older `target/task-tools` environment
with Python 3.11+ before using it for the gate; existing environments are never
cleared automatically. `ruff.toml` pins the rule set the gate
was written against, so a newer `ruff` with wider defaults does not fail an
unchanged tree.

Lint levels live in the workspace tables in `Cargo.toml` rather than only in
the gate's command line. The server and the Rust hook checker both inherit
them through `[lints] workspace = true`. The Rust lane's `workspace-lints`
gate obtains actual membership from offline Cargo metadata and refuses a
member that opts out, including implicit path dependencies. The portable
source checker needs no Cargo metadata. The Rust lane also builds public documentation with
`cargo doc --no-deps`; shared rustdoc lints reject broken links and links from
public documentation to private items.
`[workspace.lints.clippy] all = "deny"` is why an editor running a bare `cargo clippy`
agrees with `make check` instead of reporting warnings for what the gate calls
an error. Ignored must-use results, stale lint expectations, discarded futures,
locks held across await and `mem::forget` are explicit denials too.
`[workspace.lints.rust] unsafe_code = "forbid"` is what keeps this crate's
`unsafe` count at zero — `forbid` rather than `deny`, so a local `#[allow]`
cannot reintroduce it. `scripts/check-rust-style.py` holds what a lint cannot
ask: that this table stays strict, that no `allow` or bare `#[ignore]` appears,
that no marker or CJK text lands in Rust or `web/`, and that no file passes the
line ceiling in `scripts/rust-style-ceilings.json`, which `--ratchet` only
lowers. The rules and the lints left off on purpose are in the codetrial-rust
skill.

The compiler those lints run on is pinned in `rust-toolchain.toml`, so a new
Rust release cannot fail an unchanged tree: Dependabot proposes each bump as
its own pull request. The pin is the gate's compiler; `rust-version` remains the
oldest release the crate builds on, and the Linux release leg builds on it.

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

Changes to the hook scripts, installer, message checker or hook suite run
`scripts/test-git-hooks.sh` during pre-commit. Changes under `.claude/skills/`,
`scripts/`, `docs/`, `src/`, `web/`, `tests/`, `problem-bank/`, `.github/` or to
root files run the skill reference checker when the verification skill exists.
Both checks read the staged snapshot, including deletions. The hook suite
suppresses recursive suite invocation in its nested test commits. Checks run
before formatting and never stage their output.

When a push goes to `origin`, the pre-push hook first fetches the rolling
release tags `latest` and `desktop-latest` using forced exact refspecs.
Discovery and fetch share a one-minute deadline, with TERM at 55 seconds and
a forced kill five seconds later, and never prompt.
Synchronization needs `timeout` or `gtimeout`; without either tool (stock
macOS has neither) the hook says so and skips it. An unreachable `origin`
refuses that push; a push to another remote, or one that
itself updates those tags, does not fetch, and an `origin` without the tags (a
fork) is not an error. A repository without `origin` skips this. The installer
also adds the glob refspec `+refs/tags/*latest:refs/tags/*latest` to the clone's
`remote.origin.fetch` (git fails a fetch whose exact refspec names a tag the
remote lacks), so a plain `git pull` or `git fetch` tracking `origin` refreshes
the tags even when the branch is already up to date. A missing tag is not an
error there; the glob also takes any other tag ending in `latest` from `origin`. Explicit branch refspecs, as in
`git pull origin main`, do not request those extra tags. Pulling another remote
does not fetch `origin`. Git has no pre-pull hook. Installing replaces
the exact refspecs an earlier installer wrote. Uninstalling removes the refspec;
other fetch settings stay in place.

### Offline credential checks and message guidance

`scripts/check-secrets.py` checks the changed index blobs before pre-commit
materializes the snapshot. It also checks the outgoing commit range in
pre-push, including merge resolutions and an earlier commit whose secret was
removed at the tip. Commits the destination remote already holds are not
rescanned, and neither is history another remote already holds. Pull-request
CI repeats the history check. Git inspection errors refuse the operation rather
than reporting a clean scan.

The checker uses Python's standard library. It detects private-key headers,
Google API keys and GitHub token formats, and literal credential values from
`.env`, `.env.local` and `config/codetrial.env.local`, for variables whose
names contain KEY, TOKEN, PASSWORD or SECRET. Those local files are
read as data, never sourced or interpolated; symbolic-link sources are refused.
Values shorter than eight bytes, obvious placeholders and whole-value variable
or command references are excluded; a password that merely contains `$` is
kept. Paths under `tests/`, which carry throwaway private keys, are exempt
from the private-key rule only.
Changed live `.env` paths are refused; `.example`, `.sample` and `.template`
paths are allowed but their contents are still scanned. Binary blobs are not
exempt, and submodule commits are not read as files. Findings name the path,
rule and commit, with matching values redacted even from filenames. No snippet,
credential value or provider request is logged or transmitted. This is a
bounded set of checks, not a claim to detect every secret format or encoding.
The regression suite is `tests/test_check_secrets.py`.

The prepare-commit-msg hook preserves supplied messages and adds its guidance
only once. It shows at most forty staged filenames and a short change summary
above the scissors line, without generating a subject or sending a diff to an
AI service. The existing message rules remain authoritative.

### Build-time installation

Every `cargo build`, `cargo check`, `cargo run` and rust-analyzer pass in a
clone or worktree runs the install through `scripts/hook-check`, a
workspace member whose build script reruns on every build, so a deleted or
non-executable hook and a `core.hooksPath` set in a global config are all
caught on the next one. Anything the install cannot settle, a `FOREIGN` hook, a
`SHARED` hooks directory or no `sh` to run it with, fails the build with the
installer's own instructions. `CODETRIAL_SKIP_HOOKS=1` skips the check: CI sets
it, since it replays every commit message anyway, and so can a checkout that
cannot take hooks. It is also what keeps `make uninstall-hooks` in effect, since
the next build otherwise puts the hooks back. `cargo test` leaves the check
out, since it would link an empty test binary for it each time, and so does a
command that names its targets: `-p codetrial`, `--bin`, `--test` or
`cargo install --path .`.

The hooks run the `scripts/` of the checkout they fire in, as
`scripts/hook-check` does on a build, so review a branch before committing on it
as you would before building it. No script of ours runs while a merge is
concluded: by the time its commit-msg or prepare-commit-msg fires the incoming
tree is written, and running it would make `git pull` execute what it pulled, so
the wrapper skips it, and there is no pre-merge-commit hook. CI checks a pull
request's content.

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
holds the subject to 50 characters and the body to 72, imperative and ASCII,
with a link or file name longer than a line, or a trailer, allowed past the
limit. It refuses a subject that only names a file, an AI attribution and a
`Refs #N` on the last line, and says why (see CONTRIBUTING.md). The rules run
in `scripts/git-commit-msg.awk`, one process per message, which is also what
`scripts/check-commit-log.sh` and the PR sweep's hygiene check ask, with
`--stored` for a message a commit already holds. The pre-push hook replays the
same message rules over commits a rebase or an amend rewrote after
the fact, and CI runs them over a pull request's own commits. On a topic branch
it also prints one line when the head has no `/codetrial-review` on record (the
skill records a reviewed head under the git directory); that line is advice and
never refuses the push.

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

Static assets use ETags for revalidation. Ordinary files require revalidation
with `no-cache`; vendored assets may be cached for one day. Conditional GET and
HEAD accept `If-None-Match: *`, tag lists, repeated header fields, and weak or
strong spellings of the same tag. A matching asset returns a bodyless 304 with
its cache policy, ETag when available, and `Vary: accept-encoding`. Unknown or
stale tags return the current representation.

## Review memory and the review skill

`/codetrial-review` in Claude Code, and `$codetrial-review` in Codex (which
reads the same skill through `.agents/skills`), review a diff against the
project's own history instead of from a blank slate. That history lives in
`memory.db`, a SQLite file under the git common directory
(`<git-dir>/codetrial-review/memory.db`), or wherever `CODETRIAL_MEMORY` points.
It holds two kinds of data with different trust:

- **Public project memory**: every pull request and issue discussion of this
  repository, searchable, with author, date, trust level and source. Review
  preparation draws the entries relevant to the diff into `history.md` for the
  reviewers, and `memory.py search`, `why`, `fixes` and `show` open a decisive
  source.
- **Private review experience**: the findings a reviewer confirmed, the claims
  it refuted, the questions left unproven, and later fixes, recorded by this
  clone's own reviews. Preparation retrieves the cases that touch the same
  files into `experience.md`. It is a lead to recheck against the current code,
  never a maintainer ruling. It stays on the machine that wrote it: it is
  excluded from every published snapshot and the file is owner-only.

Each CodeTrial skill ends with a closeout step. Review closeout runs
`review-lifecycle.py finish`, which checks that the report still matches the
bundle it started from, records the report, and marks the head reviewed only
after the record succeeded. A failed write is kept and retried with
`review-lifecycle.py --retry`. Other skills record through
`memory.py experience record`. The confirmed and refuted cases of complete
runs are then retrieved for later reviews that touch the same files.

A new clone does not collect the history itself. `memory.py ensure` downloads
the latest verified `memory.db.gz` from a `review-memory-*` release, so the
first review costs one download and an incremental poll rather than a backfill.
The `Review memory` workflow (`.github/workflows/review-memory.yml`) builds and
publishes that release from `main` every six hours; the file is checked against
the signed manifest and the publisher's provenance before it is installed, and
installing it keeps any private experience and local decisions already present.
Until a release exists, or when the download cannot be verified, `ensure`
reports a degraded status and the review continues without history. A full
`backfill` from GitHub is slow and runs only when asked.

The snapshot contract is in
`.claude/skills/codetrial-review/snapshots.md`, and the experience contract in
`.claude/skills/codetrial-review/experience.md`.

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
pages had names, when the candidate asks the lobby to show the titles on the
cards, or to show or export a completed report. Completed reports, including
those opened from Past attempts or a replay, and downloaded Markdown keep the
scenario title and add an optional Original problem link with the published
title and LeetCode URL. Collapsed attempts do not request the map for this
link; opening or exporting their report does. Successful lookups are cached
for the page's lifetime. Missing or malformed mappings leave the link out,
and a failed lookup or a wait longer than ten seconds does not block a
Markdown download: it proceeds without the link.

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

Presence is the weakest of the things it judges, because the interviewer
stays in the room through every way this can go wrong. So the flow also
requires, from the runtime log and the transcript panel:

- Gemini asked for the restart, which is the proactive path. Reaching a new
  socket by way of the close resumes too, so a check that asked only whether it
  resumed would pass with that path dead.
- The replacement resumed rather than restarting cold. A cold one keeps the
  interviewer in the room having lost the conversation.
- No degraded restart at any point, reported where it happened.
- Every replacement episode is held to the reply it owed, matched by ids and
  not by the transcript panel. An episode that began with a reply owed passes
  only when that same reply (same pid, session and obligation) was answered
  with audio first accepted on a socket at or after the one the episode
  installed; audio taken on the replaced socket, an earlier recovery's answer
  and another room's or session's reply satisfy nothing. An episode that began
  with nothing owed is classified idle, and one whose reply a hold, cut or
  barge-in ended before any audio is classified held; neither is made to speak.
  A start with no end, a swap whose reply never finished and a cut log fail.
  Each episode is printed with its status and reason, and
  `BROWSER_CHECK_SOAK_RESULT` names a file that receives the same as JSON.
- Every swap that was answered with audio is followed by decoded audio on the
  track. The track is reused across Gemini sockets, so the sample taken when it
  was first subscribed is older than the socket under test and covers nothing.
  The page marks each installed swap, waits for the first Speaking transition
  after the mark (audio already playing when it was marked is the old socket's),
  and samples the track again from a fresh counter baseline, within a fixed
  number of samples. A swap passes with such a sample of its own that has
  received packets and a decoded signal, or when a later swap's sample covers
  it after the later mark replaced a sample that had not opened its window. A
  missing, stale, silent or unavailable sample fails, as does one whose window
  did not open after its mark. If the track is resubscribed before the sample
  is taken, it is taken again on the new track from the same mark. A swap that
  owed no audio, or whose reply was held, is not made to speak. The sample
  proves decoded audio on the receiving track after the mark, ordered by the
  page's Speaking transitions; it is not tied to the swap's own reply, which the
  reply check above settles, and a reply that began between the install and the
  mark (the log is read every quarter second) is missed, so the next
  transition is used and may not come within the grace. It is not evidence
  that anything was physically audible and it does not time when the audio
  began.

`BROWSER_CHECK_SOAK_VERDICT_MODE` is `strict` (the default) or `pilot`, and
belongs to the soak alone. Strict is the qualification and keeps the cap of one
rebuild from local state after a reply timeout. Only a degraded watchdog
rebuild whose fallback is `no_handle` or `resume_suppressed` counts toward the
cap; a natural GoAway swap never does, and a watchdog swap that resumed the
session is reported apart and counts for nothing. Pilot leaves that cap alone
out of its verdict, so its verdict is the structural one: a reply, audio,
GoAway or handover failure still exits 1, and a soak without a GoAway still
exits 77. A pilot prints `qualification=not_evaluated` and both outcomes, so a
pilot that completed is not a pass of the strict check.

`BROWSER_CHECK_SOAK_RESULT` receives `mode`, `verdict`, `structural_verdict`,
`strict_verdict`, `qualification`, `watchdog_rebuilds_total` (every degraded
watchdog rebuild), `strict_cap_eligible_total` (those that count toward the
cap), `watchdog_resumed_total`, `goaway_swaps_total`, `exposure_s`, the
per-episode reply and audio results, and the capture file holds the same under
`soak`. `exposure_s` is the largest assessment clock of the `interview clock:`
samples when they are anchored and in order; when they are missing or
inconsistent it is `null` (with `exposure_basis` saying why), never zero. A
rate per hour of any of these totals is meaningful only with a known positive
`exposure_s`.

Checks that need credentials or a running service stay out of the gate and are
run on their own: `scripts/server-check.sh`, `gemini-check.sh`,
`parity-check.sh`, `report-parity-check.sh`, `visual-parity-check.sh`,
`recording-integration.sh`, and `recording-provision-check.sh`.

## Instructor task packaging

The instructor tool and its acceptance tests use the pinned Python dependency in
`scripts/requirements-task.txt`. `scripts/task-package.py` finds its own
interpreter: it needs Python 3.9 or newer (the test gate needs 3.11, for its Rust style
checker), and reruns itself under a newer
`python3.x` on PATH when started on an older one; `build` also needs the
dependency, and reruns under `target/task-tools` (`CODETRIAL_TASK_TOOLS` names
another place), or else under a `python3.x` on PATH that already has it, and
only then sets up `target/task-tools` from that file. It creates that
directory and never clears one, so a broken one has to be removed by hand.
`./scripts/test.sh` uses the same environment and sets it up the same way, in a
full run or the portable lane, the only one with packaging tests. Given a
`PYTHON` without the dependency, it skips those tests instead. To set
it up by hand:

```sh
python3 -m venv target/task-tools
target/task-tools/bin/pip install -r scripts/requirements-task.txt
```

`scripts/task-package.py --help` lists references, new, validate,
render-prompt, preview, build and publish-scan;
[task-mode.md](task-mode.md#instructor-workflow) says what each does. The
tooling never uploads or pushes files.
