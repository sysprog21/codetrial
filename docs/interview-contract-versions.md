# Interview contract versioning

Every agent-produced report carries one `interviewContract` bundle with five
positive integer versions: the bundle, live prompt, report prompt, scoring rubric,
and public report schema. The server owns this value and stamps it after model
generation; model or candidate output cannot select it.

The active bundle is version 2: live prompt 1, report prompt 2, rubric 1, and
report schema 1. Bundle 2 introduced provider-enforced structured report output
and strict validation without changing rubric semantics or the public schema. A change to prompt behavior,
score anchors, or report shape must update the relevant component and create a new
bundle version in the same change. Update Rust and browser constants, prompt/report
goldens, migration fixtures, and replay fixtures together. Never reuse a released
bundle number for different behavior.

Compatibility rules:

- Reports without `interviewContract` predate this contract. They remain readable
  and are labeled `legacy/unversioned`; they are never assigned the current rubric.
- The browser renders the active report schema normally. Additive fields may be
  ignored by an older renderer only after the bundle/schema migration explicitly
  permits them.
- A malformed, unknown, or future bundle becomes an incomplete but renderable
  report. Scores are not coerced or displayed under a rubric the renderer does not
  understand.
- Breaking field semantics, required-field changes, rubric-anchor changes, or
  prompt-policy changes require a new bundle and the corresponding component bump.
- Migration belongs at the browser report-sanitization boundary. It must be pure,
  deterministic, fixture-backed, and preserve the original rubric provenance.

Release checklist: update the active server bundle; add the browser migration;
refresh prompt and report goldens; cover successful, incomplete, legacy, malformed,
and future reports; verify HTML, Markdown, history/progress, and replay provenance;
then run the complete local test suite.
