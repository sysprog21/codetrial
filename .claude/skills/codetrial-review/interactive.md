# Work through a review interactively

The report in [SKILL.md](SKILL.md#5-report) is read once. An interactive
review is worked through across turns, pushes and a moving head. One plain
Markdown file, the ledger, carries it and is the only state.

The ledger is a working doc in `$SCRATCH`
([where that is](../codetrial-conventions/SKILL.md#working-docs-at-the-repo-root)):
`$SCRATCH/review-$pr.md` for a PR, `$SCRATCH/review-local.md` for a local diff.
What goes on GitHub is still the payload of step 6, in the text of
[comments.md](comments.md).

## Read in this order

A reviewer who opens on the code reads the most lines for the least signal.
Open on what the diff says the change does, then on what could contradict it:

1. **The tests the diff touches, first**, removed, skipped or loosened ones
   before added ones ([tests lens](lenses.md)). A weakened test is a P2, or a
   P1 when what it pinned is a contract or an interview flow, until the
   description explains it.
2. **The behavior each remaining test pins.** Name it as a sentence. A changed
   behavior with no test that would fail without the change is the first
   candidate for the tests lens.
3. **The changed code no test reaches.** Grep the tests for the changed
   function and wire names in one alternation (`grep -rnwE 'a|b|c' tests web`);
   no lane emits coverage, so the grep is the check. This is the short list
   where reading code still pays. Say "not executed by any test" only after it.
4. **Then the rest**, by the area grouping of step 1.

## The ledger

```markdown
# Pending
- [P1] src/web/token.rs on line 42-44: a refused admission still mints a token - "if !allowed {" (at 3f2a9c1)
- [P2] tests/runtime.rs on line 17: the assertion reads the value the function computed - "assert_eq!(got, got_again);" (at 3f2a9c1)

# Completed
- [P2] web/lib.js on line 88: the handler is added twice - "addEventListener('topic'," (at 3f2a9c1) - fixed in 9d41e20, `./scripts/test.sh` fails without it

# Questions
- Does the room close when the last candidate leaves? Needs a live interview (`src/livekit.rs`).

# Notes
- `src/dispatch.rs` is the only caller of the management API.
```

- **Pending** holds confirmed findings only, the ones that survived step 4,
  most severe first. A bullet is the level, the path, `on line N` or `N-M`,
  one sentence of the defect, the quote, and `(at <short head>)`: the head the
  line numbers count in. The quote is everything after the first ` - "`,
  copied verbatim from the first line of the range, and the whole of that
  line, indentation aside: `review-anchors.sh` places a posted comment by a
  quote that equals a whole line within the diff's hunks, while the ledger
  accepts any substring of a line anywhere in the file, so a partial quote
  is placed here and then fails to post. It is code, so never a credential:
  quote a neighbouring line, or none, when the line holds one. A finding about
  a whole file or directory leaves out `on line` and the quote.
- **Completed** is what was fixed, or accepted by a ruling with its link. It
  keeps the commit and the command or test that shows it, so "addressed" is
  never the whole entry.
- **Questions** are the unproven findings step 4 keeps for the user, each with
  what would settle it. **Notes** are observations that are not tasks: what a
  module is for, what to watch for, a decision made while reviewing. Neither
  is posted.

Anything else in the file, prose or a heading of the user's own, stays as it is
when the ledger is rewritten, in the order it was read.

## When the head moves

Before using a ledger from an earlier turn, place every Pending line at the
new head. When every `(at ...)` already names the head there is nothing to do.

```sh
ledger=$SCRATCH/review-$pr.md
short_head=$(jq -r '.head[:7]' "$bundle/manifest.json")
.claude/skills/codetrial-review/review-ledger.py "$ledger" "$bundle/head" --write --head "$short_head"
```

Without `--write` it only reports. A dirty-tree ledger has no commit that holds
its lines: run it against the checkout and leave `--head` out. The script's
header defines the states and what `--write` touches; what it cannot do
is yours:

- **moved**: a quote found on several lines goes to the nearest, the lower
  number on a tie. Check a duplicate against `git diff -U0 <at> <head> -- <path>`
  before trusting it.
- **unchecked**: it names a line but no quote, so nothing was looked for. Read
  the line and add the quote.
- **unparsed**: a Pending item the script could not read (no `[P0]`..`[P3]`
  level, a space in the path, indented or numbered, or inside a code fence
  that is never closed, which is reported too). It was not placed. Rewrite it in the bullet form
  above, or move it to Notes if it is prose.
- **stale**: read the head around where the line was. If the defect went with
  the code, move it to Completed as "gone at <head>" with the commit that removed
  it; if the code only changed shape, write a new finding and quote the new
  line. Never close one without reading, since "the line is gone" also
  describes a file that was moved.

Say in the report how many findings were exact, moved, stale, unchecked and
unparsed.
Placement establishes only where the quote is: verify every finding at the current head
under [step 4](SKILL.md#4-verify), guards and callers included, even when it
is exact, and keep an unproven one as a Question.

## Presets

Each is a request the user can make at any point; what the agent does is fixed
so that two sessions answer the same way.

- **Review into the ledger.** Steps 1 to 5 end with the findings written to
  Pending instead of only the report. Questions and Notes fill the same way.
- **Address one finding.** Fix only it, per
  [codetrial-change](../codetrial-change/SKILL.md) and the narrowest lane of
  [codetrial-verify](../codetrial-verify/SKILL.md). Move it to Completed with
  the commit and the command that shows it, or report it as partly fixed or
  unverified. If the fix outgrows the finding, stop and ask.
- **Address every pending finding.** The same, in ledger order, running the
  targeted test after each and the full lane once at the end. A finding that
  does not hold goes to Questions with the counter-evidence, not silently to
  Completed.
- **Ask about these lines.** Answer from the head and the contracts. An answer
  is not a finding: it goes in the reply, and into Notes only when the user
  wants it kept.
- **Post.** Only the Pending findings the user names, through step 6. The
  payload is built from the ledger's quotes, so a moved finding is placed by
  its quote and a stale one cannot be posted.

The ledger never grants a permission; the harness asks, and the user answers.
