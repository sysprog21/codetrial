# Writing JavaScript in CodeTrial

The browser half is plain ES modules under `web/`, with no build step and no
bundler. This page is what changes about the method in
[SKILL.md](SKILL.md) when the code is JavaScript. How a file reaches the
browser, the vendored bytes and the wire contract belong to
[codetrial-web](../codetrial-web/SKILL.md); this page links them and does not
restate them.

## What the build checks

Each exits 0 on a clean tree, so run them before handing back:

| Command | Catches |
|---|---|
| `./node_modules/.bin/eslint .` | An undefined name (`no-undef`), the bug a rename leaves behind. Run `npm ci` first if `node_modules` is absent. |
| `./scripts/indent.sh --check` | Prettier formatting, with the Rust, Python and shell formatters |
| `node scripts/gen-wire-fixtures.mjs --check` | A producer change that moves a wire fixture |
| `./scripts/test.sh browser` | The node and browser tests under `tests/browser/` |
| `cargo test --locked --test web embedded_web_assets` | That the embedded tree serves with no `web/` directory beside the binary |

`scripts/browser-check.sh` runs a real browser against a real page and is
outside the gate; codetrial-verify has what it needs. A passing lane is evidence
for what it covers, and a file no test imports is covered by none of them.

Read the lane's "not checked by this run" lines. Without a usable Chromium the
browser lane still exits 0 and skips the real-page tests (973 of them on a
machine whose Playwright build is not installed), including the ones for
`task.js`. Even when they run they are blind to some of it: changing the
calibration line or the face-files message in `web/task-calibration.js` left all
120 tests in `tests/browser/task-workspace.test.js` green, so a change to code
the page tests do not pin needs its own node test, as that file has.

## Splitting a file

A new `web/*.js` file is embedded without being listed anywhere: `rust-embed`
takes the `web/` folder, and only a page's entry module has a `<script>` tag in
its HTML, by an absolute path such as `/task.js`. Every other file is imported by
a relative path that keeps the `.js` extension, or started as a Worker by an
absolute URL (`new Worker("/face-worker.js")`).

A missing file is caught in one place: the test "relative imports resolve to
files that exist" in `tests/browser/dom-contract.test.js`, which runs in the
browser lane. It reads literal relative specifiers only, so an import built in a
template literal (`web/document-grounding.js`, `web/face-presence.js`) and a
Worker URL are not checked, and `no-undef` catches a missing binding, not a
missing file. When a move touches either of those, the page itself is the test.
When behavior moves, add a test that imports the new module, in the commit that
moves it.

The wire topics are a contract implemented twice, in `web/lib.js` and in the
`TOPIC_` constants in `src/runtime.rs`. A refactor of either side changes
neither name nor payload; see
[the data channel](../codetrial-web/SKILL.md#the-data-channel-is-two-implementations).

## What the tree looks like (2026-11-06)

| File | Lines | Commits in 90 days | Note |
|---|---|---|---|
| `web/interview.js` | 6226 | 161 | the most edited file in the browser half |
| `web/app.js` | 2685 | 80 | |
| `web/lib.js` | 2287 | 87 | carries the wire topics, 12 importers |
| `web/task.js` | 1836 | 6 | an entry module, nothing imports it; the calibration checks, the case text and the result wording moved to `web/task-calibration.js`, `web/task-cases.js` and `web/task-review.js` |

`eslint .` passes. With `complexity` over 20, `max-lines-per-function` over 100,
`max-depth` over 4 and `max-params` over 7 switched on for a probe, there are 34,
40, 5 and 1 hits, so the cost here is function size more than file size: the
worst are `bindEvents` (496 lines), `mountBoardText` (444), `sanitizeReport` in
`lib.js` (complexity 65) and `connect` in `interview.js` (63).

## Where to start

`web/task.js` was the safest first split: few commits, no importers, two
functions over 100 lines. It was also the least covered, so each cut pins its
behavior with a node test first and moves one cohesive block of pure code:
`web/task-calibration.js`, `web/task-cases.js` and `web/task-review.js` are done.
The page's 45 mutable variables now live in one sealed object in
`web/task-state.js`, so `connect` and the start handler can move once the
helpers they call do.
Not `web/interview.js` first: it is the file most changes go through and the
most likely to conflict with an open pull request; and not `web/lib.js`, whose
topics are the contract.

## No size ceiling yet

The Rust ceilings only shrink, and the baseline check refuses a file that gains
an entry, so four entries for these files would fail it by design. A `web/`
ceiling needs a deliberate one-time decision about how those four enter the
file. Until then no size limit applies to `web/*.js`: `check-rust-style.py` reads
`web/` for CJK text only, so keep new files under 1500 lines by convention and
do not grow a file already over it to land a change.
