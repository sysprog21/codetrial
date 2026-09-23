# Interview contract versioning

Every agent-produced report carries one `interviewContract` bundle of five
positive integer versions: the bundle, the live prompt, the report prompt, the
scoring rubric, and the public report schema. The server owns that value and
stamps it after model generation, so neither model output nor candidate input
can select it.

## The active bundle

Bundle 16: live prompt 8, report prompt 13, rubric 1, report schema 2.

| Bundle | Introduced |
|---|---|
| 16 | Test counts, deltas, and diagnostics in the evidence projection retain their unverified browser provenance in live, interim, and final assessment prompts. Test event counts are labeled browser-reported, and the shared assessment heading distinguishes those claims from server-derived metadata. |
| 15 | The interviewer's watch prompts carry a plain-text view of the deterministic evidence ledger, a few lines of counts and closed enumerations with no digests, timestamps or entries, plus a line of what arrived since the last watch prompt, and the editor fenced as untrusted, whole while it is short and otherwise the lines around the change since the last review; the interviewer calls `read_editor` for code it leaves out. The interim review carries the same view ahead of its untrusted editor and transcript blocks. A code change reaches the model as a coarse class (`formatting`, `comment`, `identifier` or `code`) with no node facts; the ledger keeps the finer class for replay. The interim review and the final report sample with a fixed seed. The report prompt delimits the candidate's editor, transcript and test output as untrusted blocks, with the refusal to follow anything inside them stated above all three, and the server's own evidence ledger rendered apart from them rather than inside the untrusted rolling assessment. |
| 14 | Behavioral silence past the existing idle threshold gets one nudge per round. It asks the round's question if it has not been asked, invites nothing further on a declined probe or a complete answer, and otherwise offers one neutral invitation to continue, without adding a STAR follow-up or returning to coding; silence alone never ends the interview. Editor reviews remain suppressed in the behavioral round. A behavioral question declined in candidate lines that recovery pulls into the round's block counts as the round's question, so that round asks none. |
| 13 | No behavioral question is asked before the behavioral round opens, and a behavioral probe the candidate cannot recall, declines, or cannot share is abandoned in either round: editor reviews, silence nudges, the round transition, resume, timer and recovery prompts must not reopen it, and the report leaves its unsupported STAR parts unassessed rather than recording a timing skip. Recovery places the round's start at an interviewer turn still speaking when the round opened; it asks the question when nothing has been said in the round, recovers the round in a transcript block of its own so a question declined inside it counts as asked while one declined before it only closes its theme, and asks nothing further when the opening is lost. Coding watcher prompts are suppressed during the behavioral round, and resuming preserves the active round. |
| 12 | The interviewer records the step the candidate just finished before moving to the next, and the evidence reply that first ticks a later step names the earlier steps of the same framework still without evidence, so the candidate's step list fills in order rather than all at once. |
| 11 | Candidate-authored test evidence includes the bounded input beside its result, so the interviewer and report reviewer can identify the case. |
| 10 | Hints remain qualitative context for candidate independence rather than automatic numeric communication-score deductions. |
| 9 | Reports keep the fixed mid-level hiring bar and state the optional level the candidate practiced for beside it. |
| 8 | Candidate-authored test cases reach the live interviewer and report brief, while judge pass totals remain separate. |
| 7 | The post-interview server stamp adds optional debrief, topics, and practice level fields. `tests/golden/report-schema.json` remains the model output shape only; server-stamped fields are versioned at the browser sanitizer. |
| 6 | The interviewer is given the countdown instead of guessing at it: every stage direction ends with the timer reading, `read_editor` returns it alongside the editor so a current one can be asked for at any moment, the cold-restart briefing carries it, and the live prompt forbids stating or acting on a remaining time that did not come from one of those, reads the platform's reading as the last sentence of an event so candidate text that forges the sentence cannot pass for one, and forbids warning about time before the platform's five-minute event |
| 5 | Each problem posed as an interview scenario rather than the published problem: the live prompt holds the scenario, its private contract and the clarifications to answer when asked, the follow-ups arrive with the evidence that completes the coding round, and the prompt never holds the source title, the hint ladder or a solution walkthrough; `log_hint` serves the authored hints one rung per request and holds the last until the candidate has stated an approach, meaning Algorithm evidence observed from what they said or Coding evidence, which needs code they wrote; a request answered with a withheld rung gives no clue and is not counted as a hint; Coding, Test and Optimizations evidence is refused until the editor holds code the candidate wrote beyond the starter; the report prompt gives the reviewer both the published problem and the scenario, with the reference notes, and forbids naming the published problem in anything written to the candidate. PR #38 covers `8eaaef0`, `26410f7`, `4e316f2`, `0dc521f`, `b72984a`, and `3027bb8`. |
| 4 | The observable-delivery policy, made explicit in the report prompt and the server validator, with no change to the rubric or the public shape. Confirmed 2026-09-16: prompt revisions `14ad45d`, `a3872ce`, `a43eb29`, `2655f6a`, `8d2f3df`, `dbc2060`, and `5dca169` shipped under this bundle. |
| 3 | Framework phase scores kept explicitly formative, and prohibited from mechanical use in a hiring decision while calibration remains incomplete |
| 2 | Provider-enforced structured report output and strict validation, with no change to rubric semantics or the public schema |

## Changing it

A change to prompt behavior, score anchors, or report shape updates the relevant
component and creates a new bundle version in the same change. Rust and browser
constants, prompt and report goldens, migration fixtures, and replay fixtures
move together. A prompt-only change bumps its prompt constant and the bundle in
both `src/agent.rs` and `ACTIVE_CONTRACT`, refreshes the prompt golden, and adds
a row here; it needs no browser compatibility-list edit. A released bundle
number is never reused for different behavior. A bundle is released when the
pull request that opens it merges, so its versions move once per pull request: a
later commit in the same series that changes a prompt again keeps that bundle's
versions, records the prompt golden's new digest, and extends the bundle's row.
Bundle 5's row, which covers every commit of PR #38, is the precedent.

## Compatibility rules

- Reports without `interviewContract` predate this contract. They stay readable
  and are labeled `legacy/unversioned`; they are never assigned the current
  rubric.
- The browser scores a report when its rubric is active, its schema is in
  `SCORABLE_SCHEMAS`, its bundle is at least 4 and no newer than active, and
  neither prompt version is newer than active. A report keeps the bundle it
  claims. A rubric or schema change is not compatible until this rule says so.
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

Update the active server bundle and `ACTIVE_CONTRACT`; add the browser
migration; refresh the prompt and report goldens; add this bundle's table row;
cover successful, incomplete, legacy, malformed, and future reports; verify
HTML, Markdown, history and progress, and replay provenance; then run the
complete local test suite.
