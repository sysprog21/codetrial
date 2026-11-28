# Adding a problem

`problem-bank/` is the source of truth. Add the same id, in bank order, to
`problems.json`, `judges.json`, and `variants.json`; then regenerate the
candidate pages, judges, private server data, page map, and cards:

```bash
python3 scripts/gen-problems.py
python3 scripts/gen-problem-cards.py
python3 scripts/gen-problems.py --check
python3 scripts/gen-problem-cards.py --check
```

Every problem needs difficulty, topics, constraints, starter code, and private
`summary`, `optimal`, and `pitfalls` fields. The generator keeps those fields
on the server; never copy them into the browser payload.
A judge has executable cases, and a variant supplies the candidate-facing
scenario, renamed entry point or class name, examples, clarifications,
follow-ups, and hints. Class judges omit C because its harness supports only
function exercises.

An optional `sequel` names the problem the lobby offers next after a pass:
the same technique exactly one level harder, so an Easy problem names a
Medium one and a Medium problem a Hard one. The card generator writes it onto
the card by page name, so no problem id reaches the page.

An optional `sibling` names another problem at the same level that uses the
same technique. After the candidate chooses a practice focus from a report on
this problem, the lobby offers the sibling as a new problem to try that focus
on. It is written onto the card by page name the same way, and a pair must
name each other.

Both offers count a pass the way the progress panel does: a finished interview
decided HIRE whose report this build can score, which includes one saved
before reports recorded a contract. An incomplete HIRE, or one recorded under
an older rubric, neither triggers the sequel nor counts as having passed a
sequel or sibling, so it does not hide that offer.

An imported LeetCode exercise keeps `title` and published `examples` in
`problems.json`, and carries its LeetCode number (filled by `fetch.py`). It
belongs to `scripts/top-interview-150.json`, and is checked against the
study plan. An original exercise sets `"origin": "original"`,
has no published `title` or `examples`, and stays outside that plan. Its
candidate page and `web/problem-pages.json` deliberately omit `source`.

Run the focused bank checks before the full gate:

```bash
python3 -m unittest -v -k outside_the_plan tests/test_gen_problems.py
cargo test --test agent problem_bank_matches_imported_golden
python3 scripts/gen-problems.py --check
./scripts/test.sh
```

When an original exercise intentionally changes the private-rubric golden,
refresh it explicitly and review the resulting fixture:

```bash
UPDATE_PROBLEM_GOLDEN=1 cargo test --test agent problem_bank_matches_imported_golden
```

The task-mode prompts rendered from every problem are hashed in
`tests/golden/task-prompts.json`, so editing or adding a problem changes
that fixture too. Refresh it the same way and review the diff:

```bash
UPDATE_PROMPT_GOLDEN=1 cargo test task_bank_prompts_have_only_allowlisted_material_and_match_golden
```

## Reference solutions

The shape checks cannot tell a wrong `expected` from a right one, and a wrong
one fails a correct candidate in the interview. A correct answer saved as
`problem-bank/references/<judge id>.py`, defining a function named after the
judge's `entry`, is run over every case by `gen-problems.py --check`. Each
input reaches it as it reaches a candidate, through JSON, so `2.0` arrives as
`2` and an integer past 2\*\*53 as the nearest double. Its result is compared as the page compares it: through JSON, so a tuple is an
array, then by `deepEqual` in `web/lib.js`, so `True` is not `1`. As in the
Python driver in `web/runners.js`, a judge with `outputParam` compares that
argument after the call, and one with `outputPrefixParam` expects the
reference to change that argument in place and return how many leading items
to keep, then compares that prefix. Nothing in that folder is served.

Only an `exact` function judge over plain values can have one so far; the
other checkers, linked lists, trees and class judges come later. Write
the plainest correct solution rather than a fast one: it is read to settle
whether a case is wrong. When the problem allows more than one right answer,
as `find-peak-element` and `gas-station` do, the reference raises when a case
has several, and otherwise returns what the judge expects: the one answer, or
the problem's own result for none, such as `-1`. A case with two answers then
fails the check with `case N raises` instead of failing a correct candidate.

## Validation messages

The generator stops at the first broken contract. These messages identify the
source file to fix; do not edit generated output to silence them.

- `missing or duplicate problem id`, `origin must be leetcode or original`,
  `an original problem has no published title or examples`, and `an imported
  problem needs its published title` come from `problems.json` identity and
  origin checks.
- `number must be a positive integer` ensures the LeetCode number is valid.
- `an imported problem needs its LeetCode number` means an imported catalog
  entry is missing `number`; add its official LeetCode number in `problems.json`.
- `duplicate number` means two catalog entries share a LeetCode number;
  correct the `number` field in `problems.json`.
- `original exercises cannot have a number` means an original exercise
  carries a LeetCode number; remove its `number` field.
- `missing rubric`, `unknown difficulty`, and `topics must contain 1..8
  values` (or `topics must be non-empty and unique`) name required problem
  metadata.
- `sequel must name a problem one level harder` comes from a `sequel` in
  `problems.json` that is not a string, names no problem in the bank, or
  names one that is not exactly one level harder.
- `sibling must name another problem at the same level` comes from a
  `sibling` that is not a string, names no problem in the bank, names the
  problem itself, or names one at a different level.
- `sibling must name this problem back` comes from a `sibling` whose problem
  does not name this one as its own `sibling`.
- `variants must list every problem once, in bank order`, `a variant has
  exactly`, `a function problem declares a new entry`, and `a class problem
  declares a new className` name the scenario record to repair.
- `the brief never names`, `starter does not define`, `case labels repeat`,
  `examples show published case`, and the source-title messages mean the
  candidate-facing scenario, starter, or selected judge case leaks an invalid
  name or does not match the executable contract.
- `variant titles must be unique, and unique as page names` and `page names
  must not equal a problem id` protect links and saved history.
- `plan diverges from problem-bank` means an imported id and the study plan
  differ. Mark a course-owned exercise `original` instead of adding it to the
  plan.
- `case N expects X, the reference returns Y` comes from a reference
  solution under `problem-bank/references/`. Work the case out by hand: either
  the judge's `expected` is wrong, or the reference is. `case N raises` means
  the reference failed on that case's input, or found more than one right
answer. `fails to load` means the file itself does not run: a syntax error, a
failed import or an exception at module level. `no judge has this id`, `only an
  exact judge`, `needs a codec` and `defines no function` mean the file's
  name, its judge's checker or values, or its function do not fit.
