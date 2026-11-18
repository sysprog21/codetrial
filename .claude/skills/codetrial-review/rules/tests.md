# Tests

What a test has to prove, and what makes one hang or pass in the wrong lane.
How to write one is in
[codetrial-verify](../../codetrial-verify/SKILL.md#writing-a-test).

## A new test must fail on the regression it names

Applies to: `tests/**`, `scripts/browser-check.cjs`

This is the most common maintainer finding. For each new assertion, ask which
one-line revert would make it fail. Recurring vacuous shapes: the setup already
produces the asserted state (a one-element `due` pool, a default `MEDIUM`
level, a phase that is already `ending`), the test adds the class or state
itself, a fixture whose input never reaches the branch (`# ` before the
block-comment marker), `-1 < attach` order checks with no `-1` guard, all-zero
layout boxes that satisfy every inequality, `deepEqual` on a Set that ignores
order, an outer timer that fires the flush the code should have scheduled,
`.is_ok()` true for `Ok(Err(_))`, a test that reads the same env var as the
binary under test. Tests that match source text with regexes are rejected: they
pass when the line is unreachable and fail on reformatting. Drive the real
handler through `FAKE_LIVEKIT` or a mocked `nodes` instead. Mutation shards
(`.cargo/mutants.toml`) report MISSED mutants, and a default trait body that no
test reaches counts as missed.

Seen: 30+ PRs. A reload test that passes even when the removal handler's
`writeStored` is deleted.

Not a finding: a regex test that follows an accepted precedent, where the
reviewer notes it and moves on.

## Bound every wait, and do not sleep for a fixed time

Applies to: `tests/browser/**`, `tests/unit/**`, `tests/*.rs`

`scripts/test.sh` runs `node --test` without `--test-timeout`, and
`page.evaluate` has no timeout. A `while` loop inside `evaluate`, a promise
that never rejects (`arrived` waiting on two downloads), a compiled candidate
binary run without `timeout`, or a send loop that never fails will hang the
gate instead of failing it. Fixed waits (`1200 ms` for a debounced flush, a 2 s
`waitForRequest` cap) race on loaded runners. Use `waitForFunction`,
`page.clock`, or the file's existing `settles` helper. `domcontentloaded` plus
a static selector can resolve before `bindEvents` runs (top-level `await
loadProblem` comes first).

Seen: 8 PRs. A `page.evaluate` spin that turns a regression into a CI kill with
no error.

Not a finding: `waitForFunction` called from the Playwright library. It already
has a 30 s default without Playwright Test.

## Tests must run, and pass, in the gate lanes as they exist

Applies to: `tests/**`, `scripts/test.sh`, `scripts/browser-check.cjs`,
`tests/golden/**`

CI installs only headless Chromium (`test_every_browser_launch_is_headless`
rejects `channel:`). Linux Chromium reports a collapsed caret direction as
`forward`, unlike macOS. `Instant::now() - 15 min` panics on a young CI
container (use `past_the_coding_round`). Java tests must probe both `javac` and
`java`. Compile-and-run cases belong in `describe(..., { concurrency: true })`.
Interview pages need `DEFAULT_RUNTIME_CONFIG` (the lobby server does not serve
`/runtime-config.js`). Assertions placed only in a browser lane
`.github/workflows/check.yml` does not run (read which `BROWSER_CHECK_AGENT` values it starts)
never run in CI. When behavior changes,
update the dependent browser tests, goldens and parity scripts in the same PR:
renamed selectors, the case count in `tests/golden/report-python.json`,
`report-parity-check.sh` shape checks, the `indent` gate (prettier,
`printWidth` 80).

Seen: 12 PRs. A language-selector change that leaves about 30 browser tests
clicking `[data-language=...]` buttons that no longer exist.

Not a finding: a relative golden path. `cargo test` runs from the package root,
as the other goldens assume.
