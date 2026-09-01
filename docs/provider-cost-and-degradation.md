# Provider cost and degradation controls

Each admitted interview spends one LiveKit/Gemini live-session open and may spend
at most 16 resumption opens after resumable disconnects. `GEMINI_RESUME_LIMIT` is
consumed only with a server-issued handle. Final reporting has a separate hard
budget of six Gemini HTTP calls: initial generation plus one semantic repair,
with each generation allowing its first call and at most two transient retries.
The counter is consumed immediately before the network request, so no future loop
change can exceed the budget accidentally. Authentication, bad models, malformed
responses, and other permanent failures do not receive transport retries.

The server admits at most `CODETRIAL_MAX_CONCURRENT_INTERVIEWS` live local agents
(default 16). A reload of an already-live room reuses its slot; completion and
panic release it. Provider projects are rotated and their LiveKit connection-minute
quota is refreshed in the background; known-exhausted projects are skipped. The
token endpoint allows 30 starts per 60-second bucket. Signed-in buckets are keyed
by account, anonymous buckets by client address, so anonymous traffic cannot spend
an authenticated candidate's allowance. Candidate-facing failures use bounded
machine-owned categories such as `at_capacity`, `livekit_quota_exhausted`, rate
limit/retry-after, report schema failure, or report timeout; logs must never include
provider bodies, API keys, prompts, transcripts, code, or grounding text.

Only checked-in/static problem-bank, schema, and vendored runtime metadata may be
cached. Never cache candidate prompts, transcript, code, profile/JD/resume
grounding, provider output, repair output, framework evidence, or personalized
feedback. Report requests are constructed from the current session and each retry
uses that same session's immutable prompt; there is no cross-session response
cache.

The browser exposes distinct accessible states for connecting, live,
reconnecting, offline practice, report generation, incomplete report, and
retry-ready. Reconnecting preserves the live session and resends current code.
Offline practice keeps the editor and local tests usable but explicitly promises
no personalized evaluation. An invalid/exhausted report says no scores or verdict
were created. Browser fallback may summarize local test progress only for a
session that never reached an interviewer; it never masquerades canned feedback as
an agent evaluation.

Operational checks: watch `codetrial dispatch_refused ... reason=at_capacity`,
`livekit quota:` transitions, token HTTP 429 plus `Retry-After`,
`gemini report transport_failed call=... retry=...`, and bounded incomplete-report
categories. Raise concurrency only after checking provider minutes, Gemini limits,
CPU/audio capacity, and the token burst policy. The deterministic dispatcher,
rate-isolation, resume-limit, report-budget, request-isolation, and browser-state
tests run under `./scripts/test.sh`.
