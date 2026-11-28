# Writing JavaScript in CodeTrial

The browser half is plain ES modules under `web/`, with no build step and no
bundler. This page is what changes about the method in
[SKILL.md](SKILL.md) when the code is JavaScript. How a file reaches the
browser, the vendored bytes and the wire contract belong to
[codetrial-web](../codetrial-web/SKILL.md); this page links them and does not
restate them. Before writing a helper for something the browser may already
do, or reaching for a newer API, read
[platform.md](../codetrial-web/platform.md).

## What the build checks

Each exits 0 on a clean tree, so run them before handing back:

| Command | Catches |
|---|---|
| `./node_modules/.bin/eslint .` | An undefined name (`no-undef`), the bug a rename leaves behind, and the other statements that cannot mean what they say (`eslint.config.mjs` lists them, each with its reason: a `finally` that drops an error, a `catch` that only rethrows, a `map` callback that returns nothing), `no-empty` (an empty `catch {}` takes a comment inside it; `promise.catch(() => {})`, this tree's fire-and-forget, is no block), `no-debugger`, `no-console` in `web/`, and `.only`, `.skip` or `{ skip: true }` in a test file, each kept only with a disable comment naming its reason. Run `npm ci` first if `node_modules` is absent. |
| `python3 scripts/check-added-lines.py` | In the comments you added: one narrating the change, a TODO marker, an emoji, an `eslint-disable` that names no rule or has no `-- reason` (and, outside JavaScript, a Python test skipped with no reason) |
| `python3 scripts/check-json.py` | A JSON file the browser reads differently from Python or Rust: a duplicate key, a `NaN`, an integer past 2**53, a number past the double's range, an unpaired surrogate escape |
| `./scripts/indent.sh --check` | Prettier formatting, with the Rust, Python and shell formatters |
| `node scripts/gen-wire-fixtures.mjs --check` | A producer change that moves a wire fixture |
| `./scripts/test.sh browser` | The node and browser tests under `tests/browser/` |
| `cargo test --locked --test web embedded_web_assets` | That the embedded tree serves with no `web/` directory beside the binary |

`scripts/browser-check.sh` runs a real browser against a real page and is
outside the gate; codetrial-verify has what it needs. A passing lane is evidence
for what it covers. A node test covers a module it imports; a file the real-page
tests drive is covered only where Chromium runs them.

Read the lane's "not checked by this run" lines. Without a usable Chromium the
browser lane still exits 0 and skips the real-page tests, including the ones for
`task.js`. Even when they run they are blind to some of it: changing the
calibration line or the face-files message in `web/task-calibration.js` left
the tests in `tests/browser/task-workspace.test.js` green, so a change to code
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
`TOPIC_` constants in `src/protocol.rs` (interview) and `src/runtime.rs`
(`TOPIC_TASK_*`). A refactor of either side changes
neither name nor payload; see
[the data channel](../codetrial-web/SKILL.md#the-data-channel-is-two-implementations).

## Where the weight is

`web/interview.js`, `web/app.js` and `web/lib.js` are the largest and
most-edited files, and the cost is function size more than file size;
`bindEvents`, `mountBoardText`, `sanitizeReport` in `lib.js` and `connect` in
`interview.js` are the worst offenders. `web/task.js` is an entry module nothing
imports; its calibration checks, case text and result wording live in
`web/task-calibration.js`, `web/task-cases.js` and `web/task-review.js`.

## Where to start

`web/task.js` was the safest first split: few commits, no importers, two
functions over 100 lines. It was also the least covered, so each cut pins its
behavior with a node test first and moves one cohesive block of pure code:
the three modules named above are done.
The page's mutable variables live in one sealed object in `web/task-state.js`,
and `web/task-room.js` (the room connection) and `web/task-start.js` (what
pressing Start does) reach what `task.js` still owns through `host`, lent by
name, so they are imported without the page and tested in node with fakes. Each
move shortens the `lend` list in `task.js`; what is left in it is the next thing
to move.
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
