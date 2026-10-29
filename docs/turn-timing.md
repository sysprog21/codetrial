# Reading reply timing

The agent emits `codetrial turn_timing` when the first reply audio chunk is
accepted into its output queue. Later chunks do not emit another summary.
A pending measurement that ends without that chunk emits `outcome=no_audio`,
`interrupted`, `replaced`, `held`, or `ended` instead. Logs from before the
Pause button was removed can also carry `outcome=paused`, which nothing emits
now. Replacements finish the old measurement before the socket number changes;
the new socket starts with no inherited timestamps or tool counters.

For example, this synthetic measurement has one tool call before audio:

```text
codetrial turn_timing room=interview-example session=42 socket=1 turn=7 at=0:03.000 start_source=pcm_level wait_ms=3000 elapsed_ms=3000 transcript_wait_ms=2900 turn_close_ms=1500 turn_close_source=voice_activity_received turn_close_reason=observed vad_audio_offset_ms=99000 tool_calls=1 pre_tool_ms=2000 tool_phase_ms=1000 tool_exec_ms=400 tool_response_ms=100 tool_response_errors=0 tool_wait_ms=500 tool_after_close_ms=500 other_ms=1000 outcome=audio
```

`room`, `session`, `socket` and `turn` identify a measurement. `turn` is a local
sequence, not a provider turn ID or the `usage_event` counter. `at` is elapsed
interview time at emission. The line contains no transcript, code, tool
arguments, credentials or tokens.

| Field | Boundary or meaning |
|---|---|
| `start_source=pcm_level` | Last candidate PCM frame above the local voice-level threshold before the measurement freezes |
| `start_source=last_transcript` | Last input transcription receipt that armed a candidate reply wait, when no PCM proxy is available |
| `start_source=unknown` | No usable start proxy was observed, as in a greeting, platform prompt reply, or activity-end signal without a recent PCM frame or transcript |
| `wait_ms` | From that start proxy to the first accepted reply audio chunk; unknown when no audio arrived or no start was observed |
| `elapsed_ms` | From the same proxy to emission, including outcomes with no audio |
| `transcript_wait_ms` | From the last input transcript that armed a candidate reply wait to accepted reply audio; a separate receipt-clock cross-check |
| `turn_close_ms` | From the start proxy to receipt of Gemini `voiceActivity.ACTIVITY_END`; unknown if absent or ordered before the start |
| `turn_close_source` | `voice_activity_received` when that interval is available, otherwise `unknown` |
| `turn_close_reason` | `observed`, `no_start`, `not_observed`, `before_start`, or `after_audio`; missing and unusably ordered observations remain unknown |
| `vad_audio_offset_ms` | Provider audio-stream offset accompanying that signal, if valid; never mixed with the local monotonic clock |
| `tool_calls` | Calls locally executed before the first accepted audio chunk |
| `pre_tool_ms` | From the start proxy to entry into the first tool handler, if both are observed in that order |
| `tool_phase_ms` | From that first tool handler to accepted reply audio, including sends, board preparation, subsequent calls and model continuation; unknown without tools or reply audio |
| `tool_exec_ms` | Sum of local tool execution durations, excluding network sends |
| `tool_response_ms` | Sum of tool-response WebSocket send durations, including failed attempts |
| `tool_response_errors` | Failed tool-response sends in that window |
| `tool_wait_ms` | Union of local execution and response-send intervals clipped to the speech-proxy-to-audio window; overlapping work is counted once |
| `tool_after_close_ms` | The same union clipped to the valid turn-close-to-audio window; unknown without a usable close or reply audio |
| `other_ms` | Wait remaining after turn-close and tool work after close; unknown when either component is unavailable or inconsistent |

A transcription or provider activity observation establishes candidate speech;
microphone levels alone cannot assign a candidate wait to a greeting. A PCM
frame is usable when it precedes the speech observation by less than two
seconds; an older frame cannot stand in for a quiet answer's speech ending.
Before endpointing or model output, PCM may advance the speech-end proxy as the
candidate continues. The first activity-end signal or model output (including
a tool request) freezes the primary start. Later PCM and late transcripts
cannot shorten that wait or erase its endpoint. Accepted transcripts still
update the separate transcription cross-check.

A provider `ACTIVITY_START` can reopen speech before generation starts. During
generation, an activity-start observation is retained for the next measurement
until the old model turn ends. The original voice-activity event order and
interruption policy are preserved. A raw level crossing is never enough to
reopen a frozen measurement. Output transcripts received after turn completion
cannot create another measurement for the completed reply.

The same transcripts do not make the session generating again when nothing is
owed: within the late-transcript window after a completion, with no reply,
prompt or tool continuation owed and no candidate speech heard since the
completion, an output transcript does not set `generating`. No completion
follows it, so a replacement would otherwise ask for a reply that had already
finished. With a reply owed, or after candidate speech, the transcript still
counts as output, since it may be the answer. Text, audio and tool calls after
a completion are a new generation and are owed until it completes.

Durations are rounded down to whole milliseconds. A fast tool may report zero
milliseconds while still incrementing `tool_calls`. Tools handled during a
thinking hold or a discarded generation are excluded, as are calls after
the first reply audio. Board-image preparation, provider inference and event
queue delays are not local tool execution.

`tool_phase_ms` includes `tool_exec_ms` and `tool_response_ms`; these fields
are not additive. A long tool phase with fast local execution can indicate
waiting for the model's continuation. It does not prove which provider or
network operation caused the wait. Without a PCM observation, a late transcript
fallback can put the start proxy after a tool request. Then `pre_tool_ms` is
unknown, and only the portion of tool work inside the wait is attributed to it.

When all observations are usable, the decomposition is:

```text
wait = turn_close + tool_after_close + other
```

The example above reads as 3000 = 1500 + 500 + 1000 milliseconds. Work before
close is already covered by `turn_close_ms` and must not be added again. The
unclipped execution/send sums and `tool_phase_ms` describe operations, while
the clipped unions describe their contribution to waiting. `other_ms` includes
model continuation, transport, board preparation and event-processing delays;
it is not a measurement of provider inference alone. Missing close observations
do not become zero or a residual labeled as VAD. Independently rounded fields
can differ from the total by up to two milliseconds.

The PCM threshold is a speech proxy: noise or echo can cross it, and quiet
speech can fall below it. Transcription receipt can lag the final spoken word.
The end signal's receipt includes transport and room-event processing delay;
`turn_close_ms` is an observed endpointing interval, not pure provider VAD
processing time. Missing signals remain unknown. Subtracting tool durations
from `wait_ms` does not measure VAD latency. The audio queue's acceptance is
also earlier than browser playout, so this is not an end-to-end audible-delay
measurement.

Gemini `turnComplete` closes the model's turn, not the candidate's speech.
The automatic detection setting and its silence window are unchanged by this
instrumentation. Timing observations never feed model prompts or recovery
deadlines. See the [Live API reference](https://ai.google.dev/api/live#VoiceActivity)
for the optional activity signal and its separate audio clock.

For a manual check, use a rebuilt binary, speak a complete answer, and compare
the next summary with one where the interviewer reads the editor. Check that
the latter has tool activity, that later audio chunks add no summaries, and
that a cut, thinking hold or replacement leaves the next measurement with fresh
timestamps. A naturally slow reply is useful evidence, not a prerequisite for
testing the arithmetic with controlled timestamps in `tests/unit/livekit/`.

## Provider floor observations

`codetrial floor` records provider-observed pauses and overlaps. They use local
handler `Instant`s, not provider audio offsets or PCM levels. They never affect
prompts, recovery deadlines, endpointing, scoring or interruption policy.
Neither an overlap nor a suspect flag establishes what anyone physically heard.

A pause opens on an eligible candidate `ACTIVITY_END`. The first new
generation's tool call or accepted audio resolves it as `respond`, with
`gap_ms` from END to that output; another START first resolves it as
`continue`. Chunks of a reply already underway do not count as a new response.
A tool-only response has a gap but no first-audio timestamp. A completion
without a new response to that pause closes it as `unresolved`. Output
transcript text and local tool-ack filler do not resolve these records. Only
accepted reply audio, including a queued padded tail, establishes the
first-audio timestamp.

An overlap opens on eligible START while `generating` is true or the estimated
playout deadline is still ahead. The latter includes queued filler and audio
that survives a socket replacement. Provider `Interrupted` or a local stale
playout drop resolves the outcome as `interrupted`; this records the provider
boundary even when a protected reply is kept playing. A local stale drop can
also occur ahead of a new reply's first chunk; it resolves only an already open
provider-observed overlap, never invents candidate speech. Completion plus an
empty playout estimate gives `continued`. A later interruption cannot relabel
an already settled continuation. The loop checks drainage independently of the
existing floor enum, since other paths may already have marked it listening. No
physical speaker or receiver measurement is implied.

`overlap_ms` runs from START to END, not to interruption. For a continuation
the agent stops at drain, so `missed_barge_in` measures START to whichever of
END or drain comes first; speech that runs on after playout drained is not
talked over. An interrupted or continued overlap waits for END before emission.
A responded pause waits for START, another END, or reset so its
first-audio-to-START interval can be tested. No extra timer is scheduled. At
most one pause and one overlap are retained; they may coexist when candidate
END closes an overlap. Duplicate activity signals do not restart clocks, with
one exception: END after a responded pause cannot be told apart from an
utterance whose START went unobserved, so it emits that pause and opens a new
one. When activity expires locally or its track is lost without END, the active
overlap is flushed unresolved with unknown duration. It cannot stretch across
the next utterance. A new START also displaces any ended but unsettled overlap
as unresolved. Unobserved loss before expiry cannot be distinguished from
continuous provider activity. A thinking hold, socket replacement and
session teardown flush outstanding records; an unfinished overlap is
`unresolved`, and a responded pause keeps its known outcome. Replacement
flushes before the socket id changes. Events consumed by handoff or discarded
by existing acceptance guards are not reconstructed. New-socket records have
fresh observation state, even if old audio is queued.

These guessed thresholds are **not calibrated** for Gemini Live:

| Flag | Required observation |
|---|---|
| `cut_off` | `respond`, then START within <= 1,500 ms of first accepted audio |
| `false_stop` | `interrupted` with START-to-END overlap < 600 ms |
| `missed_barge_in` | `continued` with START to the earlier of END or drain >= 1,500 ms |

Durations are truncated to whole milliseconds. Missing or reversed intervals
remain `unknown` and never trigger a flag. `seq` is assigned when a record
opens, is local to the room session and survives socket replacement; delayed
emission can reorder sequence numbers. `at` is interview elapsed time at
emission. The fields are `room`, `session`, `socket`, `seq`, `kind`, `outcome`,
`at`, `gap_ms`, `overlap_ms`, `playout_ahead_ms` and `suspect`. Inapplicable
fields and an absent suspect print `unknown`. No text, word counts or audio are
logged. Activity is eligible only with a candidate audio track, outside a
thinking hold and discarded output. PCM levels and transcripts cannot open
records.

### Retaining browser-check evidence

For Rust and dispatch modes, `scripts/browser-check.sh` creates a private
`codetrial-floor.*` file outside its temporary server directory and prints its
path during cleanup. `BROWSER_CHECK_FLOOR_LOG` is an internal handoff to the
harness, not a caller-selected destination. The Node harness captures each
complete floor line before trimming its stderr buffer. A separate follower
reads dispatched-agent diagnostics from server byte zero, and uses a completion
marker to drain the server log after server teardown before the temporary
directory is deleted. Each line is appended with one synchronous write and is
not fsynced, so it survives a killed process but not power loss. On group
signals the follower waits for the completion marker, with a five-second
fallback; if its parent disappears it drains once and exits instead of polling
forever. Lines split across reads are joined; oversized lines and unrelated
output are excluded. Review and remove retained artifacts when no longer
needed. These diagnostics contain identifiers but no interview content.

`BROWSER_CHECK_CAPTURE`, when provided, includes `floorLogPath`, a snapshot of
record fields (including socket and sequence ids), and barge replay
state/timestamp metadata. Refused injections record an exclusion category and
observed state, so a misaligned trial stays distinct from a provider failure.
Files survive failed and interrupted runs. Abrupt process termination cannot
synthesize terminal records for still-open provider observations; missing END
or terminal records remain incomplete evidence, never successful benchmark
trials.

The barge harness replaces fake microphone audio with a silent Web Audio track
until it observes Speaking, decodes the supplied browser WAV and rechecks the
state before injection. A misaligned injection fails rather than becoming a
provider error. It then waits for a measured `kind=overlap outcome=interrupted`
for its room in the durable file; transcript order alone cannot pass the smoke.
This proves neither hint/tool-state alignment nor physical audibility. H-3/TT-2
still need separately digested WAV and headerless 16 kHz mono s16le PCM
fixtures, a replay pilot and explicit exclusion counts for misaligned trials.

## Continuity gaps

This section specifies how a gap in the candidate's audio would be entered,
merged, repaired and settled. It is a specification, not a description of the
product: the room loop does not implement it, because no run to date has
produced an attributable loss. `scripts/check-gap-state.py check` proves
the table is total and unambiguous, and `tests/test_check_gap_state.py` replays
the cases this section names. A change to the table, or to what it says the
interviewer does, changes both together.

### What counts as a gap

Three audio stores can lose candidate speech: the SDK's receive queue, the
room loop's own ingress buffer, and the audio quarantined for an unconfirmed
send. Only the last two are counted by the room loop, and today only as byte
totals with no interval and no record of whether the bytes were voiced. The
SDK's queue drops its oldest frame when full, counts the drops where a caller
cannot read them, and carries no capture time, so an SDK drop is **unknown**:
it never opens a gap, never settles one and is never inferred from a transcript.

| Evidence | Meaning |
|---|---|
| `loss_silent` | A local drop or quarantine of audio the voice classifier called silent. Trimming. No gap, no request |
| `loss_voiced` | A local drop or quarantine overlapping voiced audio, with a stable interval id derived from the interval itself. A gap |
| `sdk_unknown` | A suspected or possible SDK drop, or a transcript that lacks words. Recorded as unknown; no gap, no request |

A transcript that lacks words, or an answer-key turn that is missing, is
evidence of nothing local by itself. A model-visible repeat request therefore
needs a `loss_voiced`.

### Attributable loss

A turn's loss is attributable to this process only if all of these hold, and
the verdict names each one that does not:

1. The answer-key turn is present in the unstalled control.
2. The same turn is absent or repeated in the stall arm. A late turn is a
   delay, not a loss.
3. The stall arm's audio for that turn is verified as transmitted.
4. A drop or quarantine interval recorded in the same run exists.
5. That interval overlaps the turn's transmitted audio.

Speech-to-text mismatch alone is inconclusive. A control that also lacks the
turn makes the arm's miss inconclusive. Byte totals without an interval cannot
satisfy 4 and 5. One clip per arm is descriptive; it neither estimates a rate
nor establishes cause. `scripts/check-gap-state.py classify` applies the rule
to a report of arms and marks its result descriptive only: an attributable turn
from one clip names a case to repeat, not a rate. It reports a stall arm whose
every turn is missing or repeated, with no transcript, no local counter and no local
interval, as unexplained silence, and never as a loss. It reports `no-change`
only when every turn of every arm is intact.

### Episode rules

- One gap episode begins at the first `loss_voiced` with an unseen id and ends
  when it is settled, written off, or the session ends. Interval ids are kept
  for the session, so a repeated notice for an id that opened or merged into an
  earlier episode changes nothing, including after that episode ended.
- Another unseen `loss_voiced` while an episode is open merges into it. After a
  request was committed it reopens the episode: the request did not cover that
  audio, and settlement needs evidence after the newest loss. A loss that
  arrives while a send is in flight is remembered, and the committed send then
  reopens the episode instead of awaiting evidence.
- The debt is the interviewer's owed reply to the candidate. A gap preserves it
  and the clarification is that reply, not an addition to it. Candidate speech,
  a hold, a socket replacement and a failed send leave it owed. Only
  settlement, a write-off or the end of the session ends the gap's claim on it.
  A write-off does not cancel the owed reply: it returns to the ordinary reply
  path, with no repeat request and no assumption about what was missed.
- The request is sent when the candidate releases the floor and the
  interviewer is idle. While the interviewer is speaking, Thinking, holding,
  running a tool or republishing (`busy`), it is deferred and never interrupts.
  Candidate speech before the send supersedes it and the episode waits for the
  next release; so does a send that fails, or a request that goes undelivered,
  while the candidate is speaking. Speech that begins after the send has
  started does not recall it: the committed request stands, a candidate turn
  that began before the commit does not settle it, and only a failure or an
  undelivered request returns the gap to the candidate. A loss noticed after
  the floor was already released waits for the candidate's next release, and
  nothing is sent on a timer.
- Nothing follows `session_end`: a late send result is dropped by the caller,
  not replayed against the table.
- One request per attempt; two attempts per episode; three committed requests
  per session. A failed send retries only once the interviewer is idle again,
  at the next end of `busy` or the next floor release, never in a loop. A
  reopened episode starts a fresh attempt allowance, bounded by the session
  cap. A request whose audio was cancelled before delivery does not settle: it
  returns to due and stays spent against the session cap. The
  delivery-uncertain rule in the
  [interviewer delivery evidence policy](interviewer-delivery-evidence.md)
  applies. When a cap is reached the gap is written off and no request is sent.
- Evidence counts as `post_send` only when the candidate turn's speech began
  after the later of the last committed request and the last unseen voiced
  loss. Settlement means the candidate took the floor after a committed
  request. It does not mean the missing words were recovered. No transcript, note or report
  may supply, credit or penalize the missing content, and the request is a
  session condition, not a candidate signal (see the
  [observable delivery policy](observable-delivery-policy.md)).

### State table

States: `CLEAR` no gap; `OPEN` gap waiting for the candidate's next release; `DUE`
released but not sendable (busy, or waiting to retry); `SENDING` one send in
flight; `REQUESTED` committed, awaiting evidence. `=` keeps the state; `*`
means every state. The interpreter sets `busy` from `busy_start` and
`busy_end`, and `candidate_active` from `candidate_start` until the next
`floor_released`, before it reads the guards. Actions are labels the
implementation must realize; the checker enforces the counters, the debt and
the ordering around them. `invalid` marks an event that cannot
occur there: replaying it fails. Three rows, a release or retry with both
attempts already spent, cannot be reached because spending the last attempt
ends the gap; they stay so the table is total, and the check treats any other
unreachable row as a defect.

```gap-table
# state | event | guards | next | actions
@cap session_requests 3
@cap episode_attempts 2
* | loss_silent | - | = | note_trim
* | sdk_unknown | - | = | note_unknown
* | busy_start | - | = | noop
CLEAR | loss_voiced | dup | = | note_dup
CLEAR | loss_voiced | !dup | OPEN | open_gap,preserve_debt
OPEN | loss_voiced | dup | = | note_dup
OPEN | loss_voiced | !dup | = | merge_gap
DUE | loss_voiced | dup | = | note_dup
DUE | loss_voiced | !dup | = | merge_gap
SENDING | loss_voiced | dup | = | note_dup
SENDING | loss_voiced | !dup | = | merge_gap,mark_late
REQUESTED | loss_voiced | dup | = | note_dup
REQUESTED | loss_voiced | !dup | OPEN | reopen_gap
CLEAR | candidate_start | - | = | noop
OPEN | candidate_start | - | = | noop
DUE | candidate_start | - | OPEN | supersede
SENDING | candidate_start | - | = | noop
REQUESTED | candidate_start | - | = | noop
CLEAR | floor_released | - | = | noop
SENDING | floor_released | - | = | noop
REQUESTED | floor_released | - | = | noop
OPEN | floor_released | busy | DUE | defer_no_interrupt
OPEN | floor_released | !busy,budget_left,attempts_left | SENDING | send_request
OPEN | floor_released | !busy,!budget_left | CLEAR | unrepaired_budget
OPEN | floor_released | !busy,budget_left,!attempts_left | CLEAR | unrepaired_attempts
DUE | floor_released | busy | = | defer_no_interrupt
DUE | floor_released | !busy,budget_left,attempts_left | SENDING | send_request
DUE | floor_released | !busy,!budget_left | CLEAR | unrepaired_budget
DUE | floor_released | !busy,budget_left,!attempts_left | CLEAR | unrepaired_attempts
CLEAR | busy_end | - | = | noop
OPEN | busy_end | - | = | noop
SENDING | busy_end | - | = | noop
REQUESTED | busy_end | - | = | noop
DUE | busy_end | budget_left,attempts_left | SENDING | send_request
DUE | busy_end | !budget_left | CLEAR | unrepaired_budget
DUE | busy_end | budget_left,!attempts_left | CLEAR | unrepaired_attempts
CLEAR | send_ok | - | = | invalid
OPEN | send_ok | - | = | invalid
DUE | send_ok | - | = | invalid
SENDING | send_ok | !late_loss | REQUESTED | commit_request
SENDING | send_ok | late_loss | OPEN | commit_request,reopen_gap
REQUESTED | send_ok | - | = | invalid
CLEAR | send_failed | - | = | invalid
OPEN | send_failed | - | = | invalid
DUE | send_failed | - | = | invalid
SENDING | send_failed | attempts_left,!candidate_active | DUE | keep_debt
SENDING | send_failed | attempts_left,candidate_active | OPEN | supersede
SENDING | send_failed | !attempts_left | CLEAR | unrepaired_attempts
REQUESTED | send_failed | - | = | invalid
CLEAR | request_undelivered | - | = | invalid
OPEN | request_undelivered | - | = | invalid
DUE | request_undelivered | - | = | invalid
SENDING | request_undelivered | - | = | invalid
REQUESTED | request_undelivered | attempts_left,!candidate_active | DUE | keep_budget_spent
REQUESTED | request_undelivered | attempts_left,candidate_active | OPEN | keep_budget_spent,supersede
REQUESTED | request_undelivered | !attempts_left | CLEAR | unrepaired_attempts
CLEAR | evidence_turn | - | = | noop
OPEN | evidence_turn | - | = | noop
DUE | evidence_turn | - | = | noop
SENDING | evidence_turn | - | = | noop
REQUESTED | evidence_turn | post_send | CLEAR | settle
REQUESTED | evidence_turn | !post_send | = | noop
CLEAR | session_end | - | = | noop
OPEN | session_end | - | CLEAR | end_unsettled
DUE | session_end | - | CLEAR | end_unsettled
SENDING | session_end | - | CLEAR | end_unsettled
REQUESTED | session_end | - | CLEAR | end_unsettled
```

A written-off gap (`unrepaired_*`, `end_unsettled`) is reported as a session
condition: the interview had an unrepaired audio gap there. It is not clear or
unclear communication and cannot decide a verdict.

### Decision and contract impact

No run has produced an attributable loss, so the decision is not to implement:
no product behavior, live prompt, report prompt, rubric or report schema
changes, and the interview contract bundle is unchanged. This is an
inconclusive result, not a clearance. A whole utterance with no transcript, no
local counter and no readable SDK counter after a long microphone-consumer stall is the
unexplained case: it stays an open risk with named measurements that would
attribute or refute it (repeat arms for variance, a local drop interval per
frame, the overflow log line below, and the bytes a recording peer receives
instead of Gemini's transcript). Implementation becomes warranted when a run
produces an attributable loss the existing recovery handoff does not repair; it
then needs a live prompt and bundle bump under the
[contract versioning](interview-contract-versions.md) rules, and the local
counters must first record an interval and a voiced flag per dropped frame.

The SDK's queue does log `native audio stream queue overflow; dropped N queued
frames` through the `log` crate on the first drop and every hundredth. The
product installs no `log` backend, so nothing reads it. A test process that
installs one can observe whether an overflow happened, and N to within 99,
without a fork. The line carries no frame identity or time of its own; the
ignored continuity fixture installs such a logger and stamps each line when it
is logged, on the arm's clock, so the report gives a count within 99 and a time
and nothing about which frames were lost.
