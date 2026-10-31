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
