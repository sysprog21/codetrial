# Interview contract versioning

Every agent-produced report carries one `interviewContract` bundle of five
positive integer versions: the bundle, the live prompt, the report prompt, the
scoring rubric, and the public report schema. The server owns that value and
stamps it after model generation, so neither model output nor candidate input
can select it.

## The active bundle

Bundle 4: live prompt 1, report prompt 4, rubric 1, report schema 1.

| Bundle | Introduced |
|---|---|
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
