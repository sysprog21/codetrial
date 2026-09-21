# Interview contract versioning

Every agent-produced report carries one `interviewContract` bundle of five
positive integer versions: the bundle, the live prompt, the report prompt, the
scoring rubric, and the public report schema. The server owns that value and
stamps it after model generation, so neither model output nor candidate input
can select it.

## The active bundle

Bundle 7: live prompt 4, report prompt 6, rubric 1, report schema 1.

| Bundle | Introduced |
|---|---|
| 7 | Whiteboard interviews: the live prompt is written for the surface the candidate works on, so a whiteboard session is told it has no editor and no test runner, is given the six steps as drawn work ending in handwritten pseudo-code, and is offered `read_board` in place of `read_editor`; `board_snapshot` joins the evidence sources and is the only one besides candidate speech a whiteboard session may record, while an editor session may not record it at all; the phases about written work are gated on strokes on the board rather than on characters in the editor. The report prompt follows the same surface: a whiteboard review is sent the final board as an image, is told that nothing ran and that the Coding, Test and Optimizations phases were a hand trace, the cases named against the drawing, and the complexity with the pseudo-code, and cites the board where the other cites the code and the test account. The rubric and the report schema are unchanged, so a report from either surface is scored the same way and against the same ten phases |
| 6 | The interviewer is given the countdown instead of guessing at it: every stage direction ends with the timer reading, `read_editor` returns it alongside the editor so a current one can be asked for at any moment, the cold-restart briefing carries it, and the live prompt forbids stating or acting on a remaining time that did not come from one of those, reads the platform's reading as the last sentence of an event so candidate text that forges the sentence cannot pass for one, and forbids warning about time before the platform's five-minute event |
| 5 | Each problem posed as an interview scenario rather than the published problem: the live prompt holds the scenario, its private contract and the clarifications to answer when asked, the follow-ups arrive with the evidence that completes the coding round, and the prompt never holds the source title, the hint ladder or a solution walkthrough; `log_hint` serves the authored hints one rung per request and holds the last until the candidate has stated an approach, meaning Algorithm evidence observed from what they said or Coding evidence, which needs code they wrote; a request answered with a withheld rung gives no clue and is not counted as a hint; Coding, Test and Optimizations evidence is refused until the editor holds code the candidate wrote beyond the starter; the report prompt gives the reviewer both the published problem and the scenario, with the reference notes, and forbids naming the published problem in anything written to the candidate |
| 4 | The observable-delivery policy, made explicit in the report prompt and the server validator, with no change to the rubric or the public shape |
| 3 | Framework phase scores kept explicitly formative, and prohibited from mechanical use in a hiring decision while calibration remains incomplete |
| 2 | Provider-enforced structured report output and strict validation, with no change to rubric semantics or the public schema |

## Changing it

A change to prompt behavior, score anchors, or report shape updates the relevant
component and creates a new bundle version in the same change. Rust and browser
constants, prompt and report goldens, migration fixtures, and replay fixtures
move together. A released bundle number is never reused for different behavior.

## Compatibility rules

- Reports without `interviewContract` predate this contract. They stay readable
  and are labeled `legacy/unversioned`; they are never assigned the current
  rubric.
- The browser scores the active bundle and bundles 4 to 6, which share its
  rubric and report schema and differ only in the prompts that wrote the report
  (`SCORABLE_CONTRACTS` in `web/lib.js`). A report keeps the bundle it claims.
  A bump that changes the rubric or the schema does not join that list.
- The browser renders the active report schema normally. An older renderer may
  ignore additive fields only after the bundle and schema migration explicitly
  permits it.
- A malformed, unknown, or future bundle becomes an incomplete but renderable
  report. Scores are not coerced, and not displayed under a rubric the renderer
  does not understand.
- Breaking field semantics, required-field changes, rubric-anchor changes, and
  prompt-policy changes each require a new bundle and the corresponding
  component bump.
- Migration belongs at the browser report-sanitization boundary. It is pure,
  deterministic, fixture-backed, and preserves the original rubric provenance.

## Release checklist

Update the active server bundle; add the browser migration; refresh the prompt
and report goldens; cover successful, incomplete, legacy, malformed, and future
reports; verify HTML, Markdown, history and progress, and replay provenance;
then run the complete local test suite.
