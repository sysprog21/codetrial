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

An imported LeetCode exercise keeps `title` and published `examples` in
`problems.json`, belongs to `scripts/top-interview-150.json`, and is checked
against the study plan. An original exercise sets `"origin": "original"`,
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

## Validation messages

The generator stops at the first broken contract. These messages identify the
source file to fix; do not edit generated output to silence them.

- `missing or duplicate problem id`, `origin must be leetcode or original`,
  `an original problem has no published title or examples`, and `an imported
  problem needs its published title` come from `problems.json` identity and
  origin checks.
- `missing rubric`, `unknown difficulty`, and `topics must contain 1..8
  values` (or `topics must be non-empty and unique`) name required problem
  metadata.
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
