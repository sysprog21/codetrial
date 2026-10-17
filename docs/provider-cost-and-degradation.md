# Provider cost and degradation controls

What one interview costs upstream, what bounds it when a provider misbehaves,
and what the candidate sees when it does.

## Token optimization evidence

The measured reduction depends on which part of the workload is counted.
Repair-input savings do not imply the same reduction over an interview, and a
lower-cost session that loses candidate evidence is not a passing optimization.
The detailed experiments below retain both successful measurements and rejected
variants.

| Change or comparison | Measured result | Decision and scope |
| --- | --- | --- |
| Remove JSON whitespace outside strings in repair contents | 16,364 to 13,649 tokens over seven actual tokenizer calls; 16.59% fewer | Meaning-preserving repair-content reduction; not whole-request or session savings. Malformed and oversized inputs retain the previous fallback. |
| Compact tool and phase-enum setup | 39 plus 18 fewer tokens in matched setup cohorts | Small startup reduction; not a material full-session estimate. |
| Completed-coding silence backoff | After the first check, successful nudges in one quiet episode are spaced 60, 120, 240, then 480 seconds | Scheduler change; no measured Gemini token saving. Actual savings require a passing matched native comparison. |
| Longer first report attempt | One request versus two in a matched local fixture with a 22-second initial response | Isolated prototype; same canonical fixture delivered. No measured Gemini-token savings; retry checks and deadline-aware alternative pending. |
| Configured 12,000/6,000 window with editor recovery | 237,229 versus 256,846 observed tokens; 7.64% fewer in one full eleven-stage pair | The default control failed currentness and recall. This is not equivalent-passing-pair evidence or a general recommendation. |
| Supported whiteboard recall/report repair with that window | Candidate 310,036 observed tokens; control at least 455,460 plus unknown failed-call usage | Both failed recall; the control also failed report delivery. Rejected as adoption evidence; no savings estimate. |
| Historical-image prototype with 12,000/6,000 window | At least 82,978 tokens through four stages, plus four failed phase requests with unknown usage | Stopped for a missing required current-board read. No full-session comparison or savings estimate. |
| Report evidence indices instead of canonical evidence objects | 51,427 versus 33,055 observed tokens in the bounded HTTP comparison | Rejected: extra repairs increased observed usage by 55.58% and grounding still failed. |

## What an interview spends

Each admitted interview spends one LiveKit and Gemini live-session open, plus
one open per Gemini socket close it survives. Gemini caps a single connection at
around ten minutes, so a long interview spends several on its normal path. A
`GoAway` moves to a replacement before that close, at the first moment nothing
is lost by it: no reply generating, no reply awaited, and nothing left in the
LiveKit playout queue. A candidate talking over the queued reply is such a
moment, so the advisory is spent then rather than held for a turn boundary that
Gemini will not send. The replacement resumes the same conversation when the
server issued a handle; if the handle is unavailable or refused, the restart is
logged as degraded and is grounded from the bounded local transcript tail,
editor, round and evidence state instead. A resumed socket replaced
within a minute of opening is rebuilt locally on the next attempt, even if it
answered once before failing; one that stayed up longer did not fail at its
checkpoint, whatever it owed when it was replaced. Resumption stays disabled
through that failure run until completed output or a socket that stays up past
a minute proves recovery; these cold opens consume the same restart budget. The
reply to a recovery briefing is not that proof, whether the briefing went out
at once or was held for an unpause or the end of a thinking hold: every
replacement asks for one, so a run of sockets that each answer their briefing
and die still ends the interview.

Before the interview timer starts, one bounded Live generation verifies that
the configured model can answer, not merely accept setup. The opening greeting
is that request on the interview's own socket; there is no separate paid probe
turn. The initial socket open retains its bounded cold-open retry and key
failover policy; those attempts send setup, not a readiness generation. Once
setup succeeds, the greeting is sent once and a failed or silent generation
is not retried. Connect and setup retain their existing limits, and
`CODETRIAL_GEMINI_REPLY_TIMEOUT_S` bounds the greeting send and wait for the
first model output. Keepalive pings continue during that wait, without sending
generation input, so the reader's idle watchdog cannot shorten a longer reply
timeout on a responsive socket. Nonempty audio,
model text, output transcription, or a nonempty tool-call batch counts; input
transcription, voice activity, usage, empty output and turn completion alone
do not. The private clock travels as non-generating context before the opening
turn; it does not add another generation request.

Events observed by the probe are preserved, in order, for the normal room loop,
with the event queue's bound also applied to the startup buffer. Only observed
usage is logged on failure; no observation does not mean the request was free.
The server clock and hard deadline begin at successful output, and the browser
starts its countdown from receipt of readiness. It retains an early readiness
notice while publishing tracks or starting recording. Recording is still
requested after joining the room, not delayed until the greeting is heard.

Startup outcomes use the existing `interviewer_state` control topic with
`startup: "ready"` or `startup: "failed"`, and `reconnecting: false`. A failure
can carry a classified `reason`: `quota`, `billing`, `credentials`, or `refused`.
Only a provider response identifies those causes; timeout, 1011, 503, and an
unexplained close use `unavailable`. The browser uses fixed wording through the
existing degraded-start banner, not raw provider text or a second notification
channel. A missing notice or a disconnected room also ends its finite startup
wait without starting the timer. Replacement notices contain no startup field
and cannot restart the clock.

A deployment without a live session still offers timed offline practice as
before. That local clock is not granted after a joined session fails readiness.

After startup, a candidate turn, required prompt, or tool
continuation that produces no output for 45 seconds replaces the socket even
without a `GoAway`. `CODETRIAL_GEMINI_REPLY_TIMEOUT_S` sets that interval
within 20 to 120 seconds. The `codetrial turn_timing` summaries record observed
reply waits; [reply timing](turn-timing.md) defines their measurement boundaries
and unavailable observations. A reply the watchdog replaces is paid for twice,
so an endpoint whose slow replies do arrive warrants a longer timeout. For
recovery, a candidate turn is timed from the last transcript fragment of it,
and a fragment arriving within two seconds of a turn that said something is
taken as the lagging tail of speech that turn already answered, so it owes
nothing. The replacement, resumed or cold, carries
the unanswered reply across and publishes the same reconnecting state. A reply
cut off mid-generation is owed too, and the replacement is told not to repeat
what was already said. Optional editor reviews accept silence; queued audio and
a paused interview do not trigger this watchdog. A generation that stops
producing output for the same interval also recovers. Periodic nudges wait
while a reply is owed rather than replacing its debt. Until a socket has
transcribed the candidate, the first one and every replacement alike, the idle
timers also count the candidate's microphone level as speech, so a socket that
is not hearing them does not take their talking for silence and nudge them.

A close the interviewer asked for is not recovered either: it waits on the
tool acknowledgement, and if that never comes, or starts and then stops, the
interview closes once the acknowledgement has been silent for 20 seconds. A
close is held across a pause and acted on after the resume. A generation or
tool continuation silent for 20 seconds when the candidate pauses is not
waited on after the resume, so the reply to the resume is heard rather than
discarded, and if that generation does end later its ending is not taken as
the answer to the resume.

A turn Gemini completes with no output settles what it owed. The socket has
answered, and the model may choose silence; the idle nudges, not the watchdog,
respond to a silence that goes on. It is logged as a deliberate silence, so a
report of the interviewer going quiet can be told apart from a stalled socket.

`GEMINI_RESTART_LIMIT` bounds a failing endpoint rather than a long interview.
It allows 8 opens in a row. A completed turn with output clears the run, unless
it answered a recovery briefing, and so
does replacing a socket that lived past a minute and owed nothing. A socket
replaced while it owed a reply never counts as healthy, however long it stayed
connected and whether the watchdog, a `GoAway` or the server closed it, so
repeated unanswered recovery briefings exhaust the budget. Exhausting it, or
failing to open a replacement socket for any reason but billing, ends the
interview with reason `interviewer_unavailable`: no goodbye is asked for, the
reconnecting notice is withdrawn, and the report is still written from the
session held so far. A first socket that cannot be opened at all still ends the
session before it starts, with no report. A project that cannot pay is not an
endpoint that may recover: a 402, or a close reason saying the prepaid credit is
depleted or billing is not enabled, takes that key off both the Live and the
report surface and is retried only on another configured key. With none left the
interview ends at once instead of spending the remaining opens, and its summary
line says `outcome=billing`.

Every Live turn is billed on the whole context it runs in, retained audio and
images included, so what stays in the context costs again on every later turn.
Candidate video, off by default, therefore sends one frame in five seconds and
asks for the low media resolution; the camera is there for presence, and the
code reaches the model as text. On `gemini-3.1-flash-live-preview` the default
turn coverage is `TURN_INCLUDES_AUDIO_ACTIVITY_AND_ALL_VIDEO`, so every frame
also counts toward the turn. Outside candidate speech, judged from the
microphone's own voice level on every frame or from provider speech detection,
each Live socket therefore receives one presence frame and no more; during speech frames keep the
five-second interval. Board images take their own path and are unaffected.

Final reporting has a shared hard budget of five Gemini HTTP calls across
initial generation, up to two semantic repairs, and transient retries. The
counter is consumed immediately before the network request. Authentication
failures, bad models, malformed responses, and other permanent failures get no
transport retry.

After transient transport exhaustion, the overall report deadline, or a report
still refused when its repairs ran out, the connected candidate may request one
regeneration after a 30-second cooldown, or for quota exhaustion until the first
configured key leaves its cooldown, at most 60 seconds; a sole key waits the
whole 60, and a usable backup only the 30. A repair call that fails after a
refusal is judged by its own failure like any other call, so a rejected
credential still offers nothing. A missed deadline and a refused report ask the
key rotation the same question,
and so does an accepted request, so keys another interview sent back to quota during the wait
answer `early` rather than spend the regeneration on a call that cannot start. It
reuses the frozen assessment, has its own five-call pool and 125-second
deadline, and therefore bounds final reporting at ten HTTP calls per interview.
Report calls are seeded, so with the first seed the frozen prompt answers a
regeneration exactly as it answered the first time. When the first generation
had an answer refused, whether it then ran out of repairs, lost its repair call
or missed the deadline, the regeneration samples with a second fixed seed; one
that never had an answer refused keeps the first. The offer names its cause,
`schema` when an answer was refused and `unavailable` otherwise, so the page
does not call a refusal a passing outage.
Successful or salvaged reports cannot be regenerated. A rejected credential
never qualifies on its own; an available or quota-limited backup may still
offer recovery, regardless of which credential failed last. An already
exhausted quota rotation also qualifies, including when another interview
exhausted it. The offer expires after five minutes and the agent leaving ends
it. A candidate who leaves has 30 seconds to rejoin under the same identity,
which is how a full LiveKit reconnect looks, and a regeneration under way keeps
running through it and is published once they are back. One who rejoins
before acknowledging the provisional report gets it again, since the drop may
have lost it; a candidate already gone when the interview ends gets the failure
at once with no window. The
Live session is closed while the report, or the provisional report when a
window opens, is being delivered. While the offer is open the agent
answers each request on the control topic, `report_retry` with status
`accepted` or `early` (with the seconds still to wait, which does not spend the
regeneration), and announces `closed` at expiry or when no key can ever answer;
a duplicate request during
regeneration gets no answer. The browser waits at most 145 seconds from its
request or the acceptance, whichever came last, so neither a reconnect nor a
lost answer cuts off a regeneration still in progress. The terminal failure
determines eligibility, so a transient burst followed by a schema failure
regenerates as a schema failure. Reloads and process restarts cannot recover
the inputs.

Every report packet, provisional, regenerated or final, is delivered the same
way. The agent publishes those same bytes at most three times, each with a
five-second publication and receipt window, without another Gemini call. The
Live session closes beside the first delivery, so retries spend no Live time.
Each delivery keeps the room open for at most fifteen extra seconds and stops
waiting when the candidate leaves or the room disconnects. A candidate receipt
names the SHA-256 digest of the packet bytes. Missing receipts mean delivery is
unconfirmed, not that the candidate received no report; only publication
attempts that all fail or time out produce `report_delivery_failed`.

Quiet-pause interim reviews use that same report model and quota. A review is
eligible after 8 seconds of candidate quiet and 150 seconds from interview start,
no more often than every 150 seconds, and only after six new candidate turns;
none starts once the interviewer has asked to close or the planned time would
run out before the call returned, and a review whose editor has not changed
since the last one is told so instead of being sent it again.
Before the cap, a 90-minute
interview can make at most 36 such calls; shorter interviews cannot exceed that
rate. `CODETRIAL_MAX_INTERIM_REVIEWS` defaults to 6, accepts `0` to disable
the reviews, and is capped at 72. This is a quota guard, not a completeness
limit: the final report still receives the complete transcript and editor state.

The phase judge, which records the conversational REACTO steps from the
candidate's own words, uses the same report model and keys. Every judgment
needs a candidate turn or editor it has not read, or the window of a call that
failed, and a step it records still open: Repeat, Example and Algorithm until
Coding is recorded, Coding and Optimizations while there is code. On the watch
tick one starts once the candidate has been quiet and not typing for 2 seconds,
no more often than every 6 seconds, or every 30 seconds when only the editor
changed; typing pauses every few seconds while Coding is open, so editor-only
judgments also take at most 12 of the calls. The end and a held round
transition may start one sooner: the end waits at most 5 seconds in all for its
judgments, and a round transition is held at most 5 seconds while the room
keeps running, or until the resume when the interview is paused. Both share the
ceiling of 48 calls per interview. Each reads at most 6 KiB of transcript tail
and 6 KiB of editor. A rate limit on a judgment does not cool the key, so the
judge cannot leave the final report without one.

## What bounds concurrency

The server admits at most `CODETRIAL_MAX_CONCURRENT_INTERVIEWS` live local
agents, 16 by default. A reload of an already-live room reuses its slot;
completion and panic release it. A report recovery window keeps its slot and
LiveKit connection until expiry or 30 seconds after candidate departure, at most
five minutes plus a 125-second regeneration and a 30-second rejoin. Provider outages can therefore fill the slots
with completed interviews awaiting recovery; admission still refuses excess
starts rather than exceeding this bound. A fixed local room refuses a new
start while its existing agent is finalizing or awaiting report recovery, as
`dispatch_refused ... reason=finalizing` rather than `at_capacity`; it cannot
reuse that agent as an interviewer for another candidate. Provider projects are
rotated and their LiveKit connection-minute quota is refreshed in the
background, and known-exhausted projects are skipped.

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
cross-session response cache. A failed report may keep its frozen prompt and
assessment in the same agent's process memory for the five-minute recovery
window and one bounded regeneration. They are discarded on final outcome,
expiry, disconnection or process exit, never persisted or reused by another
session. The provisional failure report itself is not an input: the browser
saves it on arrival like any report, and the final outcome replaces it under
the same report id, so a tab lost during the window keeps the failure rather
than nothing.

## What the candidate sees

The browser exposes distinct accessible states for connecting, live,
reconnecting, offline practice, report generation, incomplete report, and
retry-ready. A reply owed for four seconds with nothing started on it, or one
that stops producing after its audio runs out, shows the interviewer as thinking
rather than listening. Reconnecting preserves the live session and resends
current code. While a replacement waits to retry, the notice distinguishes rate
limiting from an unreachable interviewer and estimates the next retry in seconds,
then drops the estimate once that retry starts. Each notice is best effort: one
that cannot be published within a second is skipped rather than delaying the
retry, so an estimate can outlive its wait. The estimate is a retry delay, not a
promise of recovery; startup keeps its connecting state. Offline practice keeps the editor and local tests usable while
explicitly promising no personalized evaluation. An invalid or exhausted report
says that no scores or verdict were created. Browser fallback may summarize
local test progress only for a session that never reached an interviewer, and
never presents canned feedback as an agent evaluation.

## Operating it

Watch `codetrial dispatch_refused ... reason=at_capacity` and
`reason=finalizing`, `livekit quota:` transitions, token HTTP 429 with
`Retry-After`, `gemini report transport_failed call=... retry=...`, `gemini
report retry_unavailable` (a key rotation lost during backoff, without another
HTTP call), `codetrial report_recovery_notice_failed`, `codetrial
report_publish_failed`, `codetrial report_receipt_missing`, `codetrial
report_delivery ... outcome=acknowledged|unconfirmed|failed`, `codetrial
live_usage ... outcome=billing`, and the bounded incomplete-report categories.
Each recovery window also keeps the agent and the candidate connected to
LiveKit for up to about seven minutes after the interview, which counts against
connection-minute quota like interview time.

Raise concurrency only after checking provider minutes, Gemini limits, CPU and
audio capacity, and the token burst policy. The deterministic dispatcher,
rate-isolation, resume-limit, report-budget, request-isolation, and browser-state
tests all run under `./scripts/test.sh`.

## Measuring token consumption

Google's [Live API billing guidance](https://ai.google.dev/gemini-api/docs/live-api/best-practices#pricing-and-billing)
bills each turn on the entire active context, retained raw audio included, and
adds a text-output charge for enabled audio transcription. The logs below are
what the provider reported to this server, not an invoice. Reconcile them with
an isolated project's billing before quoting a monetary figure, and do not
infer an account balance from them.

- `codetrial live_turn_usage` is one usage observation: room, `session` (the
  epoch millisecond the room loop started, so two runs sharing a room stay
  apart),
  interview clock, socket, event index, `cause`, and the provider's counters.
- `cause` names the last platform input that asked for a reply before the
  observation: `watch`, `turn` (greeting, test reaction, wrap-up and similar
  stage directions), `tool` or `recovery`, or `candidate` when the platform
  asked for nothing since the previous completed turn. Silent context, such as
  a compression checkpoint, asks for nothing and is billed inside whichever
  turn follows; `codetrial context_refresh` lines count it. The label says where
  turns come from, not a causal record: speech overlapping a platform input is
  credited to the input.
- `codetrial live_usage` sums a session's observations across its sockets, with
  the model, elapsed seconds, socket count and an `outcome` of `ok`, `error`,
  `gemini_unreachable` or `billing`. It is written on every exit of the room
  loop, and once with `phase=startup` and no socket count when the interview
  failed before its first turn, a first open that was refused included.
- `gemini report`, `gemini interim` and `gemini phase` lines carry the room,
  call number and retry count of each HTTP call. Failures are counted from
  `gemini report transport_failed` (with `final=true` when it was not retried),
  `interim review skipped` and `phase judge skipped`. A failure before usage is
  decoded has unknown cost, never an assumed zero. A successful HTTP response
  with usage but no usable text can log both usage and a failure for the same
  call; do not count that failure as a second request or extra unknown cost.
  Call numbers reset between operations, so room and call number alone do not
  uniquely correlate every attempt across a complete interview.

The counters keep prompt, response, cached, thought, tool-use prompt and total
counts, plus modality details for prompt, response, tool-use and cached tokens.
Response aliases are alternatives, not additive counters. A scalar the provider
omits is logged as zero. Each `*_detail_samples` counts valid detail arrays, so
zero samples means the breakdown is unknown and fewer than `usage_samples`
means it is partial; details may also sum to less than their scalar. The
analyzer reports a direction with a zero total and no breakdown, such as tool
use in a session without tools, as `none_reported`, never as complete: an
unused direction and one the provider left out look the same. Never add
cached tokens to the prompt total. With no `usageMetadata`, an HTTP record has
`usage_samples=0` and the analyzer increments `records_without_known_usage`;
its displayed numeric zeros are observed counters, not evidence of free work.
Check these counts, HTTP failures and incomplete-session flags before comparing
totals. Missing individual scalar fields also log as zero and cannot be
reconstructed from these logs alone.

`turn_complete_samples` counts observations that shared a frame with
`turnComplete`. Usage is recorded when its frame is decoded, before that frame's
content is queued, so a room that stops reading does not lose what it had
received. If a session's `usage_samples` exceeds its `turn_complete_samples`,
some observations arrived on frames of their own, and a sum may include
periodic snapshots of the same turn; treat it as an upper bound until the
provider's event semantics are confirmed. A process kill or task cancellation
can still leave a session without its summary.

`codetrial model_input_bytes` measures what this server constructed, by kind.
It is not billed tokens: it cannot see the retained context a turn is billed on.

## Comparing captured logs

`python3 scripts/analyze-gemini-usage.py interview.log` prints the counters above
as JSON, grouped by room and by Live session, and ignores everything else in
the log, including prefixes a log collector adds. `--room ROOM` selects one
room; stdin is read when no file is given, and the exit status is 2 when no
usage record matches.

A summarized session is reported from its summary; a session with events but
no summary is reported from them and marked incomplete. For each session with
events it also reports the prompt count of each completed turn in order, its
growth per turn, and counts by `cause`. The output makes no price or quality
claim.

Compare one change at a time, on the same synthetic interview, model, speech,
editor events and planned turns, with repeated runs. Record prompt and modality
counts, usage coverage, completed turns, reconnects, first-audio latency,
completion, and whether the interview still recalled the evidence it needed. A
projection is not an observed saving, and a smaller context must preserve the
interview's evidence before it becomes a default.

## Configuring a compression experiment

`GEMINI_CONTEXT_TRIGGER_TOKENS` and `GEMINI_CONTEXT_TARGET_TOKENS` are an
optional pair. Both must be positive integers, target strictly below trigger,
or config loading fails. Unset, setup still sends `slidingWindow: {}` and
leaves the thresholds to the provider. The pair is sent on every socket,
resumed ones included. Only the provider knows the model's context limit, so
run `codetrial check-gemini`, which opens a session with the same setup, before
an interview does.

With a pair configured, the room watches for a cut context and then sends a
silent checkpoint rebuilt from local state. A turn is taken to have run on a cut
context when its prompt count, judged at completion against the largest count
since the previous completed turn, fell by half the trigger-target gap (limited
to 1 to 2048 tokens), or fell at all from a context that had reached the
trigger. Both are heuristics, not an API compression event.

The checkpoint waits until the candidate is not mid-turn by transcript or by
microphone level, no reply, tool continuation or queued audio is outstanding,
and the interview is not paused; the room retries after Gemini events, at the
playout boundary and on the watch tick. It carries the platform timer, the
chosen language, the current round, the evidenced phases, a 2,500-byte
transcript budget (a long behavioral round keeps up to 750 bytes of its opening
beside the recent dialogue), the test report up to 1,000 bytes, and in the
coding round up to 1,800 bytes of the editor's opening and ending. Omitted
lines are named as omitted and remain available through `read_editor`; omission
is not evidence that a follow-up was unused or that no refusal occurred. A
replacement socket drops a pending checkpoint, because its recovery already
carries local state, and socket recovery keeps its own larger budgets.
Whiteboard checkpoints omit editor and test data and direct visual claims to a
fresh `read_board` snapshot. During the behavioral round, they omit the drawing
and prohibit returning to it merely to recover earlier work. They also omit
coding-run progress cues. Coding checkpoint output is unchanged by these mode
guards; no token reduction is claimed for this correctness fix.

Only while a pair is configured do `read_editor`, `log_hint` and
`record_framework_evidence` answers also carry the latest unanswered candidate
utterance, as quoted data, so a reply owed across a cut is not lost, and only
while a cut is plausible (as below) and the floor is not held. Without a
pair these application-level additions are omitted. Provider-default sliding
window compression still applies; an unset pair does not establish that the
provider retains every earlier tool call or utterance.

Configured compression also appends bounded continuity to platform text and
context inputs after the interviewer has first spoken, while a cut is
plausible: before any usage has been measured, after a cut was detected, or
while the last comparable prompt count is in the upper half of the band between
the target and the trigger. Just after a cut the context sits near the target,
so the reminder, which stays in the context billed on every later turn, is not
carried again until the context has grown back toward the trigger. This includes the
latest unanswered local candidate utterance, quoted as data, and preserves the
current round and evidenced steps without replaying the opening or full problem
restatement. In coding, a prior candidate opening is also quoted within 750
bytes once a later candidate turn exists; it is omitted during the behavioral
round. A new timer reading remains last. The first response on a cut context precedes its
usage-triggered checkpoint; the reminder covers that gap. Paused, thinking-held
and ending interviews receive no reminder. The reminder is not added under
provider defaults. Recovery briefings already carry the recent transcript and
their own no-restart rules, so they receive only the owed-question rule and any
opening or latest candidate line their transcript tail no longer holds whole.
The added text is part of measured model input, so comparisons must include its
cost rather than assume it is free.

A lower threshold reduces the history retained for later turns, and may remove
information a follow-up needs or add compression latency. Final reporting still
uses the full local transcript and editor. The credentialed probes that compare
arms and check recall are the ignored tests in `tests/unit/livekit/cost.rs`,
outside the credential-free gate; their file header lists the environment they
need.

Recall from a supplied checkpoint does not establish continuity across a cut.
The `live_compression_continuity_quality` probe starts with the real greeting
under the original interview instructions, accumulates synthetic editor context,
and requires the runtime's compression detector to fire. It checks responses
before sending the resulting checkpoint, so a renewed greeting in the first
response after a cut fails rather than being hidden by later recovery. Its
output retains synthetic replies, usage and failures for inspection. Like the
other direct-socket probes, it does not validate LiveKit playout, and the
detector remains a heuristic rather than a provider compression event.

## Retrieving omitted conversation

`read_conversation` is available on both interview surfaces. It reads only
recorded candidate and interviewer turns from the room's local transcript.
Platform events and private review notes are excluded; instructions inside
recorded speech remain untrusted data. The read adds no framework evidence,
hint or phase change. Runtime `live_tool_response` logs identify the tool, serialized response-body bytes and acceptance, without arguments or returned speech. Unknown model-supplied tool names are logged as `unknown`. These byte counts exclude the function-response envelope and do not estimate provider tokens.

`query` optionally selects a case-insensitive substring. `fromTurn` starts at a
one-based spoken turn, and `fromByte` continues within a long turn. Follow the
returned `nextTurn` and `nextByte` with the same query; null cursors mean no
matching speech remains for that query and cursor. If the full query occurs nowhere in the spoken transcript, the tool tries the longest query word of 3 to 100 bytes that occurs there, returns the original matching speech, and identifies that fallback as `matchedQuery`. The notice distinguishes a word match from an exact phrase match; continue paging with `matchedQuery`. Exact phrase matches are never broadened. A page has at most four turns and stays below 6,000
serialized JSON bytes. UTF-8 boundaries and JSON escaping are included in the
budget, so even one long utterance remains retrievable across pages.

Recovery briefs direct the model to retrieve missing earlier details before
answering from memory. This avoids resending the entire conversation in every
checkpoint. Its declaration, tool continuations and actual response bodies
still cost tokens and are included in comparisons; availability alone does not
prove that the model uses retrieval correctly. Final reports continue to read
the full local transcript.

## What a whiteboard interview adds

Measured at 1947b11 on `gemini-3.1-flash-live-preview`, problem `two-sum`, a
20-minute coding + behavioral loop, three interviews per surface with the arms
alternating, each about five minutes. Headless Chromium drove every interview
through LiveKit with a microphone that fell silent after the preflight; the
editor arm typed code and the whiteboard arm drew eight figures, 30 seconds
apart. The counts are the server's own `live_turn_usage` lines, read with
`scripts/analyze-gemini-usage.py`.

| Opening | Prompt tokens, first generation | Runs |
| ---------- | ------------------------------- | ---------------- |
| Editor | 5,431 | 3 of 3 identical |
| Whiteboard | 5,408 | 3 of 3 identical |

Per text, counted with `countTokens` on `gemini-3.1-flash-lite`. The parts sum
to the live difference exactly (47 - 56 - 14 = -23):

| Text | Editor | Whiteboard | Difference | Billed |
| ------------------- | ------ | ---------- | ---------- | ---------------- |
| System instructions | 4,408 | 4,455 | +47 | every generation |
| Tool declarations | 429 | 373 | -56 | every generation |
| Greeting | 170 | 156 | -14 | every generation |

The fixed overhead is therefore 23 tokens below the editor's: the whiteboard
instructions are longer, and `read_board` is declared in fewer tokens than
`read_editor`.

What grows is the board. Each board reaches the Live socket as a realtime image
and is counted at 60 image tokens whatever is drawn on it; JPEGs from 17 to
37 KB at the 1600x1000 export all counted 60. Boards stay in the context:
`prompt_image_tokens` rises by 60 for every board sent, and by the end of each
whiteboard session 8 to 10 boards were held, 480 to 600 tokens on every later
generation until the sliding window trims them. Across a session that was 2.9
to 4.2% of the prompt tokens. The page publishes a board a second after the
last stroke, or at the end of a stroke once one has waited four seconds, and
the server forwards at most one a second.

`countTokens` prices the same JPEG as an inline image at 1,093 tokens, so it
does not predict what a board costs on the Live socket. It does price the report
request, which attaches one board per completed phase and the final board when
it changed afterwards, each once.

Two cautions when reading these logs. An observation whose turn called a tool
carries the prompts of both of its generations, the call and the reply after
the answer, so those rows read about double, image tokens included, and the
analyzer's `mean_growth_per_observation` inherits that. Whole-session totals are
not comparable between the arms either, because a silent synthetic candidate
triggers an editor review on every code change in one arm and silence nudges in
the other.

### Compact default editor reads

When `read_editor` starts at line 1 (including an omitted `fromLine`), at least
80 leading line comments or blank lines may be compacted by representing each
run of identical consecutive lines once, with its original line range. Every
distinct note remains visible. This applies only if the compacted preface fits
4 KiB and saves at least 4,000 bytes. Unknown languages, block comments, unique
or oversized prefaces and buffers without implementation retain the original
paged read. Only the language's unambiguous line-comment prefix is recognized.

Preface and implementation share the existing 32,000-byte numbered-source
budget and per-line character cap. The answer preserves original line numbers;
explicit reads after line 1 expand the original buffer, and continuation
pointers expose later code. Only a read that shows the whole buffer marks it
shown: a default read that stops at its page limit neither carries the
proactive review nor moves the baseline, and neither does a later-line page or a
requested hint whose editor page stopped at its limit, so a change they did not
show is still reviewed. Requested hints use the same compact view. The
latest test record, evidence state and timer remain available through the same
tool; reports retain the original editor.

A tool-assisted Live usage observation may sum prompt tokens across several
model invocations. It remains fully included in spend counters, but is not a
single context-size reading. It never seeds the next comparable peak, including
when usage arrives before the tool call reveals that the turn used tools.
A tool turn's largest observed total is an upper bound on each invocation's
context: if even that total is sufficiently below the previous completed
single-generation count, it can establish a decrease. The watcher waits for the
turn's completion before making that comparison and clears the old baseline
after detecting such a cut. Other tool turns retain the last comparable
reading; cuts they cannot prove await a comparable observation. The
pre-generation and tool continuity reminders remain in force.

`live_spoken_compression_continuity_quality` requires
`CODETRIAL_COST_SPEECH_DIR` containing synthetic `restate.pcm` and
`question.pcm` fixtures (16 kHz mono signed 16-bit little-endian). The former
states the full two-sum contract and the latter asks about Rust iterator
`position`. It requires newly transcribed speech and received audio, preserves
failed results, and checks continuity across heuristic context cuts under the
original interview instructions. `CODETRIAL_COST_SPEECH_STREAM_END=1` also
flushes each fixture as a microphone mute would; that run does not validate
continuous-microphone endpointing. A missing or incomplete transcription fails
the restatement precondition, not just the continuity assertion.

The speech harness consumes events and answers tools while streaming its PCM,
as the room does. An unmuted run continues sending silent PCM while awaiting
the reply; ending the fixture is not treated as ending the microphone stream. `CODETRIAL_COST_SPEECH_INITIAL_TEXT=1` establishes the full
restatement through authored synthetic text, then tests the spoken follow-up
fixture across cuts. That mode validates native speech after a text opening,
not recognition of the initial spoken restatement. Its audio timing starts
when input streaming starts and includes the utterance itself. Follow-up PCM
fixtures are bounded to 30 seconds; the full restatement is bounded to 75.

Recovery quotes the stage direction whose reply was lost. Its final platform
clock is superseded by the recovery's fresh final clock and is removed;
timer-like text inside candidate code stays. Extra anti-script instructions
were withdrawn after all three live trials still read the private timer and
the added wording increased prompt usage.
Configured continuity similarly moves the trusted event clock to the end rather
than retaining two readings. This removes redundant input bytes, not provider
turns, and is no proof that the model will never read private metadata aloud.

When a configured window adds a timed tool-continuity block, editor, board and
hint results omit their own final canonical platform timer. The continuity
block retains the current clock; candidate source containing timer-like text
stays unchanged. Without the continuity block, the tool result keeps its timer.
This removes one redundant clock sentence from qualifying replies, rather than
claiming a whole-session saving or guaranteed silence about private metadata.
Evidence-recording replies carry the configured continuity note without a clock.
Recording evidence is not a time-reading operation; platform events and read
tools retain their clock sources.
A tool batch sends its continuity note once, in the final eligible response.
The final eligible checklist snapshot also appears once per batch, including
without compression. All results and per-tool state changes remain in order in
the same response message; standalone tool execution retains its note and
checklist. Input accounting counts the response bodies after these omissions. A coding evidence response can also carry a changed, parsed editor excerpt once, in the final eligible response, while the interview is active and the floor is free. This supplies the current edit when speech has deferred the editor watch. Unchanged buffers, incomplete parses, held floors and whiteboard or behavioral rounds add no editor excerpt. The excerpt uses the watch limits and preserves original-source access through `read_editor`. The excerpt includes whether a recorded run covers the current code or execution is disabled, so an edit does not imply passing tests. A successful response carrying changed code asks for the same proactive review within the reply already owed, with the candidate's latest question first. This includes a default editor read; explicit reads after line 1 keep their paging behavior and do not consume the review. The review is added once using the final batch state, only for a parsed substantive revision the watcher has not reviewed. Input accounting counts the finished bodies after this addition. Only a successful socket write consumes that revision; failed writes and later edits remain eligible for a watcher review. This avoids requesting another generation solely to review code just delivered. Its bytes are counted as part of the actual response; its token effect still requires live comparison.

A timed stage direction requesting a reply
sends its final platform clock as a separate non-generating clientContent update,
then sends the action with turnComplete true on that same ordered channel. It
keeps the clock outside the utterance the model is asked to produce. A completed
client turn interrupts the reply the provider owes or is producing, even before
its first output. The socket reader therefore counts the socket busy from
detected candidate speech, its input transcription, an interruption or model
output, and the socket itself from any send that requests a reply, until a turn
completes. Provider evidence lapses after 10 seconds and a requested reply
stays owed through the longest configurable reply deadline, 120 seconds, which
the reply watchdog enforces first; either way a lost turn end cannot hold every
later request on the fallback. While busy, the action goes as
realtime text instead and is answered after the reply in flight. The two parts are counted as
one input, before either write. Untimed input and context requesting no
reply retain their existing sends. This is a
privacy/continuity measure, not a standalone token saving: requested-time probes
used additional read_editor calls to obtain a fresh clock. The native browser
and compression comparisons must include that overhead.

Provider voiceActivity START/END observations hold silence nudges, editor watch
prompts, context refreshes, and idle assessment calls while Gemini detects
candidate speech. END starts the silence interval afresh. These observations
are not transcripts or assessment evidence. Socket replacement, pause, and
microphone teardown and explicit Thinking/yield stream finalization clear the active flag.
A START with no microphone or provider evidence for 10 seconds is treated as if
its END had arrived then, so one lost END cannot hold these back for the rest of
the socket. Raw microphone energy retains its
existing noise safeguard after the socket first transcribes the candidate.
This avoids treating a delayed transcript as silence; measured savings still
require a completed comparison including any additional tool calls.

An advisory transport restart also waits during the quiet transcription tail:
provider speech is still active but the microphone has been quiet for at least
500 ms, or a provider END arrived less than two seconds ago. This gives the
transcript and reply a chance to settle before a stale checkpoint is resumed.
The periodic room watch checks a held advisory during provider-detected speech,
without waiting for another provider event or a reply stall. The handoff is
judged before the speech guard that holds an ordinary restart back, since that
guard refuses exactly the voiced speech the handoff finalizes. For ongoing
microphone speech, an advisory restart first ends the old audio stream and waits up to two seconds for its partial transcription. A silent
context update cancels the reply to that prefix. The replacement receives the
full finalized prefix as JSON-quoted untrusted speech and silent interview
context before queued microphone audio continues; it is asked to answer only
after the remaining speech finishes. This silent update carries no clock;
requested-time answers can obtain a fresh reading from the editor or board
tool. Only a checkpoint issued after cancellation
can resume this handoff, and a resumed socket receives the listening note
alone and keeps its context baseline, since the provider still holds that
context; otherwise the replacement gets the full cold briefing in front of it. A
platform request the replaced socket still owed is named in the note and stays
owed. Provider events the replaced socket emits while the prefix is finalized
are discarded, and their count is logged. A failed
briefing leaves the local cold briefing due rather than ending the interview.
Microphone ingress buffers up to 40 seconds while the room loop reconnects,
about 1.3 MB of PCM plus queue overhead at 16 kHz mono. The oldest frames still
drop on overflow; ordinary consumption does not wait for this buffer to fill.
No transcription within the handoff budget falls back to normal recovery, as
does an actual transport close. This does not promise lossless recovery from an
unavailable provider. Microphone energy is measured during provider activity
even after the first transcription, without letting that energy suppress
silence nudges indefinitely.
