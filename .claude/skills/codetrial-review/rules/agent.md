# Prompts, reports and the interview contract

`src/agent*` builds the prompts and validates the report, and [interview
contract versions](../../../../docs/interview-contract-versions.md) versions
both. A golden in `tests/golden/` proves a string moved, not that the interview
still behaves.

## Prompt text must agree with the rest of the prompt and with what the code emits

Applies to: `src/agent/prompts.rs`, `src/agent/prompts/*.rs`,
`src/agent/events.rs`, `src/agent.rs`, `tests/golden/prompts.json`

Prompt builders are composed from many fragments (`Surface::CODING`,
`run_note`, `evidence_note`, `TRANSCRIPTION_REPORT_POLICY`, the decision rule,
flows 1-5, `silence_nudge`, `[SYSTEM EVENT]` stamps). When a fragment changes,
check its siblings for contradictions. Examples: a new instruction that
"confirms" an approach while flow 5 says never confirm it. A zero-turn brief
that nulls `communicationScore` but leaves a decision rule granting `HIRE` only
with "clear communication". Execution disabled while `run_note` still says test
runs arrive. A whiteboard instruction that says "assess from code and tests".
Also check claims about what the server sends: "every `[SYSTEM EVENT]` ends
with TIMER" when greeting, `wrap_up` and `cold_restart` bypass
`apply_data_event`. Paragraphs gated on a lobby option the server never
receives (`exampleDrawing` stays in the browser) go into every interview. A new
behavioral rule needs a probe in `tests/interview_behavior.rs`. A golden only
proves the string moved.

Seen: 9 PRs. A five-minute warning that says "click Run" when the runner is
reported unavailable and only a hand trace can record Test.

Not a finding: a constraint quoted verbatim from the published problem whose
character list already resolves the apparent contradiction.

## Report validators must not burn repairs on legitimate text

Applies to: `src/agent/report.rs`, `src/gemini.rs` (`MAX_REPORT_REPAIRS` = 2)

`validate_report*` failures spend one of two semantic repairs. After that,
`finish` returns the error and the candidate loses the report. Flag word scans
that match ordinary coding prose ("diagram", "sketch", "picture", "drawing on
prior experience", "the big picture"), scans of fields the round never released
(follow-ups before the coding gate), and requirements the new state cannot
satisfy (an integer `communicationScore` and two strengths from one transcribed
cough). Error paths must index the raw response the repair prompt shows, not a
normalized array with dropped entries. An overlong field must produce a length
error, not a silent `null`.

Seen: 5 PRs. Any coding report that mentions "diagram" refused once a drawing
was received, though the problem's own variant calls its input "the diagram".

Not a finding: declining to add a free-text filter for STAR content in the
summary. Reviewers agreed a text filter cannot decide that.

## Bump the interview contract everywhere at once

Applies to: `src/agent.rs` (`LIVE_PROMPT_VERSION`, report schema), `web/lib.js`
(`ACTIVE_CONTRACT`), `docs/interview-contract-versions.md`,
`tests/browser/replay-render.test.js`, `tests/browser/lib.test.js`,
`tests/browser/render.test.js`

A version bump has to change the Rust constant,
`ACTIVE_CONTRACT.livePromptVersion` (currently 30 on both sides), the new
bundle row (which names its live prompt version, like every earlier row), and
the browser tests that pin the active contract or allowlist card words.
`interview_contract_versions_are_one_closed_bundle` checks only that the row
exists, so a wrong number passes the gate. Adding a bundle to the scorable list
can quietly retire the tests that build an `active` fixture. Server-stamped
optional report fields are versioned at the browser sanitizer and may not need
a schema bump. Ask instead of assuming.

Seen: 4 PRs. A bundle row that says live prompt 24 while the code moves to 25.

Not a finding: optional server-stamped fields added without a schema bump,
which earlier bundles also did.
