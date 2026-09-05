# Development

The formatters, the hooks, the generated files, and the checks that stay out of
the gate. `make` targets and the one-line summary of each are in
[README.md](../README.md#development); everything here is the detail behind
them.

## Formatting

`scripts/indent.sh` holds the formatter chain: comment reflow with
`commentflow`, then `cargo fmt`, `ruff format` and `shfmt`, in that order
because the formatters have to have the last word over the reflow. `make
indent` runs it with `--write` and the gate runs it with `--check`, against a
copy of the tree so a check never rewrites what it is judging. `shfmt` takes
its style from `.editorconfig` and is passed no style flags anywhere.

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

`scripts/top-interview-150.json` records which problems the study plan asks
for. Refresh it from LeetCode with:

```bash
python3 scripts/gen-problems.py --sync-study-plan
```

The sync refuses to write when the plan and `problem-bank/` disagree, naming
the problems each side is missing. Port those first. `--check` holds the
committed manifest to the same rule, so drift fails the gate offline.

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

## Checks outside the gate

The end-to-end browser check additionally needs Playwright and Chromium:

```bash
npm ci && npx playwright install chromium
scripts/browser-check.sh
```

Checks that need credentials or a running service stay out of the gate and are
run on their own: `scripts/server-check.sh`, `gemini-check.sh`,
`parity-check.sh`, `report-parity-check.sh`, `visual-parity-check.sh`,
`recording-integration.sh`, and `recording-provision-check.sh`.
