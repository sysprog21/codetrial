# Judges, harnesses and the problem bank

Five languages run the same judge cases through harnesses in
`web/compiler-explorer.js` and `web/runners.js`, generated from
`problem-bank/`; [adding a problem](../../../../docs/adding-a-problem.md) is
the contract.

## All five judge harnesses refuse the same answers the same way

Applies to: `web/compiler-explorer.js` (C, C++, Java harnesses),
`web/runners.js` (JavaScript, Python)

A refusal (cycle, shared node, shallow copy, stray pointer, non-finite number)
has to come back as `error` in every language. C writing the message as
`actual` turns it into a wrong answer, and a custom case with no `expected`
then shows `pass: null`. When a guard is added to one serializer, check the
other four (next-tree walks, `nextTreeToJson`, `_next_tree_to_levels`,
`json_next_tree`). JS copies leave `undefined` where the original had `null`:
compare loosely. Python `json.dumps` writes bare `Infinity` by default. Java
rejects int literals above 2^31 even for a `double` parameter. C harness code
must check `malloc`, grow through a temporary pointer on `realloc`, skip NULL
neighbor slots, and keep fixed queues (`queue[4096]`) and value ranges
(`1..1023`) and the 2047-node cap consistent with the bank's stated limits.
Self-referential returns must not hang the run: add a timeout in tests.

Seen: 11 PRs. C reports "Returned list must be a deep copy." as the candidate's
output while the four other languages fail the case with a reason.

Not a finding: a duplicate-ID check that reports a different but still correct
refusal for an input that breaks two rules.

## Do not leak the problem's identity or technique to the candidate

Applies to: `scripts/gen-problem-cards.py`, `scripts/gen-problems.py`,
`problem-bank/*.json`, `web/problems/*.json`, `web/render.js`, `web/app.js`,
`src/agent/report.rs`

Cards carry no topic tags (a "Dynamic Programming" tag tells the candidate what
is coming), and `data-topics` in the page source leaks the same thing through
devtools. The published title and LeetCode link stay behind the `showSources`
opt-in. Saved history, downloaded reports and the replay keep the scenario's
title (`docs/development.md`). Reports must not name a practice site, even for
original problems. When page examples are removed or hidden, rewrite briefs
that point at them. `/problem-pages.json` is fetched only for the documented
reasons.

Seen: 5 PRs. A history report export that carries the published title and URL,
against `docs/development.md`.

Not a finding: judge files that keep published cases as tests, page examples
rendered from the judge's own cases, or a `source` title the maintainer asked
to show. These were deliberate and were pushed back on.

## Judge cases must satisfy the variant contract and exercise the method

Applies to: `problem-bank/judges.json`, `problem-bank/variants.json`,
`web/judges/*.json`, `scripts/problem_bank/*.py`

Each new case must fit the stated contract: a 9x9 board has nine cells in every
row, an empty tree only when the contract allows 0 nodes, exactly one valid
answer where the contract promises uniqueness. A class "boundary" case has to
call the method, not only the constructor. Expected outputs must be correct for
the domain (coin change). Counts that appear elsewhere (browser checks,
goldens) have to come from the judge and not from literals, and harness limits
must fit the bank's stated sizes.

Seen: 4 PRs. A "no stored words" case that never calls `search`, so a broken
`search` still passes.

Not a finding: a C starter that returns a string literal with no malloc note.
That note applies only to arrays returned through `returnSize`.

## A judge-gap fix must make each wrong answer fail by name, in every derived file

Applies to: `problem-bank/judges.json`, `web/judges/*.json`,
`tests/golden/task-posed.json`, `tests/web/contract.rs`,
`tests/browser/compiler-explorer.test.js`, `scripts/problem_bank/rules.py`

A pull request that closes a hole in a judge (a wrong solution the cases let
pass) is only as good as the wrong solutions it was checked against. Run every
plausible wrong variant of the technique against the cases, not only the one the
report names: the full hop, the farthest nonzero, the largest range with the
earlier or the later pick on a tie, the first match, the greedy order. A case
that rejects one of them and lets another through is half a fix, so each wrong
variant needs a case that rejects it and the other cases still pass a correct
solution. The label of each new case must not match `REVEALING_LABEL` (run
`rules.check_labels`, which rejects a match): it describes the situation and
never names the trick.
The files the judge feeds move together: `web/judges/<page>.json`
regenerated from `problem-bank/judges.json`, the digest in
`tests/golden/task-posed.json`, and the tricky-case labels `tests/web/contract.rs`
pins. The browser test runs the wrong implementations by name and looks each
pinned case up by label, asserting the case exists and the wrong answer differs
from `expected`; a `.some(...)` over all cases passes when one of three cases is
removed and names none.

Seen: 5 PRs. A new case for one wrong chooser while the same chooser with a tie
rule flipped still passes all eight cases.

Not a finding: a wrong implementation the problem's contract allows, such as an
unspecified tie order or another valid path to the same answer. It needs no
rejecting case.
