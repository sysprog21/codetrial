# Interview selection

Practice is the default. Its problem picker, published-id and scenario URLs,
repeat attempts, refresh behavior, offline fallback and local candidate cases
are unchanged. Reloading a Practice URL runs media preflight and starts the same
problem again; it does not recover the previous attempt.

Strict Interview is an opt-in selection policy, separate from the coding or
whiteboard surface and the coding-only or behavioral loop. Difficulty remains
a multi-select choice in both modes. Strict hides the random and specific-problem
pickers; the server draws from the selected levels, without using Practice's
history recommendations or topic filter. The lobby sends `selection=strict`
and one `difficulty` query parameter per checked level, without a problem. A
Strict URL's `problem` parameter is ignored; reopening it cannot request that
problem. Switching to Practice is allowed. This is a self-assessment mode, not
an administrator-enforced exam.

## Admission and selection

The recording-consent notice appears during media preflight; the camera
and notice are required on recording deployments. Consent is recorded and a
room requested only after preflight succeeds; failure or leaving for the lobby creates no attempt.

`POST /api/token` accepts `selectionMode: "strict"`, optional `difficulties`
and an optional `previousAssignment` receipt. The browser passes the URL's
`difficulty` values as the `difficulties` array without dropping invalid values.
When present, it must contain one to three distinct strings drawn from exactly
`"Easy"`, `"Medium"` and `"Hard"`. Empty arrays, duplicates, unknown values,
wrong casing and non-string values are refused with HTTP 400 and
`strict_difficulty_invalid`. Missing difficulties keep the all-level draw for
existing Strict URLs and API clients; this is not a fallback for invalid input.

The endpoint rejects any `problemId` field in a Strict request, including null,
even with valid difficulties. Missing selection mode and explicit `"practice"`
keep the existing API. Unknown modes are refused.

After the existing account and consent checks, the server randomly selects
from the checked-in problems matching any selected difficulty, excluding only
the preceding assignment. The response adds `problemPage`, the public scenario
page named in the signed LiveKit metadata, and a new `previousAssignment`
receipt. It uses a fresh room
and candidate identity even when `INTERVIEW_ROOM_NAME` pins Practice to a
development room. The normal provider quota, capacity, consent claim and
dispatcher gates still apply. The hosting dispatcher starts an independent
agent; a web-only deployment must staff these dynamically named rooms through
its external agent host, just as for unpinned Practice rooms. An agent launched
manually for the fixed room does not serve Strict attempts.

If no eligible problem remains after difficulty filtering and exclusion,
admission is refused with HTTP 409 and `strict_no_alternative`, before a room
or agent is allocated. The browser shows the reason and keeps its previous
receipt. There is no fallback to another level or to the previous problem.
A pool with one matching problem allows it once, then refuses a request
carrying that assignment. A later return to an older problem is allowed (A, B, A).

The selected difficulties stay in the interview URL across a normal reload.
The receipt format is unchanged: it names the previous problem regardless of
difficulty. If a caller changes the difficulty filter, that page is still
excluded whenever it belongs to the new pool.

## The previous-assignment receipt

Only one receipt is kept in this tab's `sessionStorage`. It contains a purpose
and format version, account id and public scenario page, authenticated with the
server's session signing secret. It conveys neither room access nor recording
consent and contains no interview content. The server verifies the signature,
account and current scenario before excluding the page. Invalid receipts are
refused; another account or a rotated signing secret requires a new sequence.

The browser checks storage before allocation and saves the new receipt before
loading or revealing the assigned problem or joining the room. A token refusal
keeps the old receipt; a subsequent problem-load or join failure keeps the new
one. Storage failure, an invalid assignment or a missing problem stops Strict
admission without starting offline practice or falling back to the default
problem. Retry returns to preflight. Strict candidate cases stay in memory.

Reloading retains this tab's receipt and returns to preflight. A browser history
restore from the back/forward cache reloads Strict through the same gate. Opening an old
URL in a new tab has no prior assignment unless the browser copied the opener's
session storage. Independent tabs have independent sequences; a duplicated tab
initially excludes the copied assignment, then progresses independently. Two
tabs may receive the same new problem. Explicitly starting Strict from the lobby
clears this tab's sequence; returning to the lobby alone does not.

## Limits

Strict is an opt-in self-assessment mode. Excluding only the previous problem
does not prevent repeated refreshes from eventually drawing a familiar one.
It keeps the existing agent cleanup policy, so an old agent may temporarily
occupy capacity while a new attempt starts. This change adds neither
multi-problem history nor Strict-specific cleanup.

The receipt is not an account-wide attempt ledger or a replay-proof credential.
Clearing storage, omitting the receipt or replaying an older valid receipt can
reset the exclusion. The server still selects the next problem and
never accepts a caller-selected Strict problem. Enforcing the latest attempt
across tabs or devices would need server-side history and a separate product
decision. There is no cheating signal, score penalty or permanent exclusion.

## Lifecycle and contracts

A browser reload starts a new attempt. It restores no editor, transcript,
timer, framework progress, evidence, report-recovery inputs or Gemini session.
The previous room follows its existing participant-absence cleanup and recording
lifecycle; it may briefly overlap the new attempt and consume a capacity slot.

LiveKit reconnect within a loaded page keeps its current room and problem.
Gemini reconnect and same-agent report recovery remain unchanged. The existing
beforeunload warning still protects an active, unsaved interview in both modes;
it does not classify refresh or trigger allocation.

Recording consent is still claimed once per room, and starting again requires
fresh consent. Report shape, assessment prompts, rubric and integrity evidence
semantics do not change, so no interview contract bundle or component version
is bumped. The receipt's format tag versions only selection metadata.

Regression coverage lives in `tests/unit/web/selection.rs`,
`tests/web/token.rs` and `tests/browser/strict-selection.test.js`. The
credential-free browser tests use the real preflight and page reload with a
fake media grant and room transport; provider reconnect and recording still
need the existing credentialed integration checks for end-to-end assurance.
