---
name: codetrial-conventions
description: The CodeTrial conventions no gate enforces - the register a comment, a commit message and a PR reply are written in, the untracked working docs at the repo root, where multi-byte characters are allowed and where they are not, deleting a redundant surface instead of deprecating it, and the repository layout. Use when drafting a commit message or a PR description, adding a file or a public surface, removing a flag or a subcommand, or writing a comment longer than a line.
---

# CodeTrial conventions

The gate settles formatting and correctness. `cargo fmt`, ESLint, `ruff` and
`shellcheck` run in `scripts/test.sh`, so none of that is here. What is here is
what a reviewer would otherwise have to say out loud, plus the two rules that
are enforced by the git hooks rather than by CI: the commit message and the
staged-content checks. Install them with `make hooks`.

`README.md` is the tracked half and this file does not restate it. Where the
two speak to the same thing, `README.md` wins.

## Commit messages

The house style is Chris Beams' seven rules, and `scripts/git-commit-msg.sh`
enforces the mechanical ones: subject within 50 characters, capitalized,
imperative, no trailing period, no backticks, no conventional-commit prefix,
body wrapped at 72, printable ASCII throughout. Run
`git log --no-merges --format=%s` if you want the calibration set rather than
the claim; the recent log sits at 46 characters in the median.

The rule the hook cannot check is what the body says. This tree keeps its
detailed reasoning in the comment next to the code, often several paragraphs
per decision. A body that retells the mechanics duplicates that comment and
then goes stale on its own. Write the premise and the trade, once, usually a
single paragraph:

```
Bound a video frame by the memory it will take
Refuse a permission listing that may be truncated
Prove an empty test run is not a pass
```

## Prose register

Source comments and commit messages are ASCII: no em dash, no typographic
quote, no arrow character, no CJK. Markdown files are exempt and use ordinary
GitHub Markdown, so a backtick belongs in `docs/` and not in a commit subject.

Chinese, and CJK generally, stays out of `src/`, `web/` and `tests/` even when
the conversation that produced the change was in Chinese. The product's
surfaces are English. A test that genuinely needs multi-byte input to pin a
character-boundary rule should use characters that read as a fixture rather
than as prose in any language: Greek letters, accented Latin, an emoji. Say in
the comment that the byte width is the point.

## Comments

Brevity is part of correctness, but the bar here is rationale rather than
length: this tree carries long comments where the reasoning is long, and the
rule is that every line of one says something the code cannot. Delete anything
restating the statement below it.

- Bad: `let deadline = start + duration; // add the duration`
- Good: `let deadline = start + duration; // truncated to whole minutes above,
  so the page and the interviewer end at the same number`

Never point a comment, a docstring or a `docs/` page at `TODO.md`. It is
untracked and per-developer, so the reference dangles for everyone else. Code
may name the path where it genuinely reads the file, and a runtime diagnostic
may print it so the reader can navigate.

## Working docs at the repo root

`TODO.md`, `DONE.md` and `CLAUDE.md` are excluded through `.git/info/exclude`,
not through `.gitignore`, because the exclusion is one person's habit and
`.gitignore` would push it onto everybody. Never `git add`, stage, commit or
delete them, and never assume a clone has them. Reports and analyses stay out
of the tree the same way: a scratch directory, not a new tracked file.

## Deleting a surface

A subcommand, flag or helper that turns out to be redundant gets deleted,
along with its call sites in the Makefile, the scripts, the README and the
tests. It does not become an alias and it does not get a deprecation warning.
"Never break userspace" here means the compatibility contract the golden
fixtures in `tests/golden/` and the versions recorded in
`docs/interview-contract-versions.md` describe, not the convenience surface of
the CLI.

## Pull requests and review replies

The commit body carries what and why. A review thread carries a correction, a
measurement or nothing: no pasted agent walkthroughs, no severity tables, no
re-summarizing a diff git already shows. Close an addressed thread with
"Resolve conversation".

## Layout

```text
src/            Rust server, interviewer agent, recording, token minting
web/            Static browser application, served embedded or from disk
problem-bank/   Source of truth for web/problems and web/judges
config/         Env file templates and per-provider credentials
docs/           Contracts and operational notes
scripts/        Build, gate, hook and integration scripts
tests/          Rust, Python and browser tests, plus golden fixtures
```

Generated files live under `web/` but are owned by `problem-bank/` and by the
generators in `scripts/`. Editing one by hand is a drift the gate catches.
See codetrial-verify for what regenerates them and codetrial-web for how
`web/` reaches a browser.
