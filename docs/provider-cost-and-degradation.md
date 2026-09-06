# Provider cost and degradation controls

What one interview costs upstream, what bounds it when a provider misbehaves,
and what the candidate sees when it does.

## What an interview spends

Each admitted interview spends one LiveKit and Gemini live-session open, plus
one open per Gemini socket close it survives. Gemini caps a single connection at
around ten minutes, so a long interview spends several on its normal path: a
close is resumed onto the same conversation when the server issued a handle, and
started cold when it did not.

`GEMINI_RESTART_LIMIT` bounds a failing endpoint rather than a long interview.
It allows 8 opens in a row, and any socket that lived past a minute clears the
run.

Final reporting has its own hard budget of six Gemini HTTP calls: initial
generation plus one semantic repair, each generation allowing its first call and
at most two transient retries. The counter is consumed immediately before the
network request, so no future loop change can exceed the budget by accident.
Authentication failures, bad models, malformed responses, and other permanent
failures get no transport retry.

## What bounds concurrency

The server admits at most `CODETRIAL_MAX_CONCURRENT_INTERVIEWS` live local
agents, 16 by default. A reload of an already-live room reuses its slot;
completion and panic release it. Provider projects are rotated and their LiveKit
connection-minute quota is refreshed in the background, and known-exhausted
projects are skipped.

The token endpoint allows 30 starts per 60-second bucket. Signed-in buckets are
keyed by account and anonymous buckets by client address, so anonymous traffic
cannot spend an authenticated candidate's allowance.

Candidate-facing failures use bounded machine-owned categories such as
`at_capacity`, `livekit_quota_exhausted`, rate limit with retry-after, report
schema failure, and report timeout. Logs never include provider bodies, API
keys, prompts, transcripts, code, or grounding text.

## What may be cached

Checked-in and static problem-bank data, schemas, and vendored runtime metadata,
and nothing else. Never candidate prompts, transcript, code, profile, job
description or resume grounding, provider output, repair output, framework
evidence, or personalized feedback. Report requests are built from the current
session and each retry uses that same session's immutable prompt, so there is no
cross-session response cache.

## What the candidate sees

The browser exposes distinct accessible states for connecting, live,
reconnecting, offline practice, report generation, incomplete report, and
retry-ready. Reconnecting preserves the live session and resends current code.
Offline practice keeps the editor and local tests usable while explicitly
promising no personalized evaluation. An invalid or exhausted report says that
no scores or verdict were created. Browser fallback may summarize local test
progress only for a session that never reached an interviewer, and never
presents canned feedback as an agent evaluation.

## Operating it

Watch `codetrial dispatch_refused ... reason=at_capacity`, `livekit quota:`
transitions, token HTTP 429 with `Retry-After`, `gemini report
transport_failed call=... retry=...`, and the bounded incomplete-report
categories.

Raise concurrency only after checking provider minutes, Gemini limits, CPU and
audio capacity, and the token burst policy. The deterministic dispatcher,
rate-isolation, resume-limit, report-budget, request-isolation, and browser-state
tests all run under `./scripts/test.sh`.
