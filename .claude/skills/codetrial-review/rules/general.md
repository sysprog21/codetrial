# Rules for every change

Three rules every area breaks the same way, so every reviewer loads them
whatever the files. Each rule here and in the files beside it was distilled from
this repository's own review threads: a defect class maintainers raised on at
least three PRs, worded as what breaks, with the near-miss that was pushed back
on. `Seen` is how many PRs raised it, which is how often to expect it, not how
bad it is.

## One source of truth for anything both sides spell

Applies to: `web/lib.js`, `src/protocol.rs`, `src/runtime.rs`, `src/agent.rs`,
`web/interview.html`, `web/*.js`, `tests/fixtures/`,
`scripts/browser-check.cjs`

Data-channel payloads are produced once in `web/lib.js` and consumed against
the interview `TOPIC_*` constants in `src/protocol.rs` and the
`TOPIC_TASK_*` ones in `src/runtime.rs`. `tests/fixtures/README.md` lists
each producer and consumer, and `node scripts/gen-wire-fixtures.mjs`
regenerates them. A payload built inline in `interview.js` and spelled as a
byte literal in Rust tests crosses nothing. Flag new hand-kept copies: voice
and accent lists in Rust, `<option>`s and the test literals (three copies, no
comparison); `harnessGap` reasons re-prefixed in `interview.js`;
`BRACKET_PAIRS` vs the inline map in `indentNewline`; line-comment markers
duplicated from `tokenizer.js`; judge case counts as literals in
`scripts/browser-check.cjs` and goldens; C starter structs vs the harness
struct; the lobby palette copied into `setup.rs`; `downloadMarkdown` copied
instead of exported; a placeholder string matched by literal in three files.
Ask for one export, or a test that compares the copies.

Seen: 14 PRs. An accent added on the server but missing from the browser list
drops `interviewerAccent` from every saved report.

Not a finding: a deliberate split already recorded in `.cargo/mutants.toml` or
a README that explains why two sides differ.

## Use the helper, rule or test the module already has

Applies to: `web/*.js`, `web/*.css`, `tests/browser/*.js`, `src/**`

A change that writes a second copy of something the file or its neighbors
already do. Flag it when the copies can drift: `clamp(value, min, max)` is
exported by `web/lib.js`, yet a new module spells `Math.min(Math.max(...))`; a
copy button re-spells the pending guard, `aria-disabled` and reset of
`mountFinalCodeCopy` in `web/render.js`; the container unwrap in
`candidateTypeMatches` in `web/runners.js` repeats the one in `compilerType`
and `nativeLiteral` in `web/compiler-explorer.js`; a rule repeats the
`display`, `align-items` and `gap` of a shared selector in `web/styles.css`; a
test declares the same locators in every case, or adds an assertion that an
earlier one in the loop already proves. Ask for the existing export, a helper
beside the code, or one fewer assertion, and name the copy a later edit would
miss.

Seen: over 20 PRs. A predicate added in a second file that disagrees with the
exported one about a disabled track.

Not a finding: a second use that needs a branch the helper cannot take without
a flag, when the diff says so.

## Change docs/ and nearby comments with the behavior

Applies to: `docs/*.md`, `README.md`, comments in changed files

`docs/` holds contracts (`docs/avatar-contract.md`,
`docs/provider-cost-and-degradation.md`, `docs/development.md`,
`docs/interview-contract-versions.md`, the troubleshooting walkthroughs). Flag
a change that makes a doc sentence false: an outcome missing from a list, a log
line a walkthrough still tells readers to look for, "the title reaches the live
page only" when it now reaches history and replay, "leaving aborts the load"
dropped while it is still true. Do the same for comments next to the change
that describe the old mechanism (pause/unpause comments after the pause is
gone, "sent rather than queued", a doc on `tail` that says "at the moment it
was cut"). Also flag a leftover diff marker (`+`) in a doc.

Seen: 12 PRs. A contract line dropped although `leave()` still aborts the
avatar load.

Not a finding: a doc sentence that already covers the case when read in full
(the CLI `--data-dir` working-directory wording).
