# Reply latency observability

The Rust agent logs one `codetrial latency {JSON}` line at each point a reply
passes on the server. `python3 scripts/analyze-latency.py server.log` joins
them into a per-reply waterfall, a chronological timeline and a closing
`key=value` summary. `--summary` prints only the summary, `--json` prints
everything, and omitting the filename reads stdin. Log prefixes from journald
or containers are accepted. The run fails when no record is found; `--strict`
also fails on any rejected record or when no reply with a known input origin
reached native capture. Provider-only output remains visible but cannot establish
a healthy input-to-capture latency verdict. No
transcript, tool arguments, responses, credentials or candidate media enter
these records.

## One trace per reply

Each reply is a trace keyed by `(pid, room, session, turn)`. `socket` is where
the record was written and `origin_socket` where its input arrived, so a reply
that waits through a socket replacement stays one turn and its total includes
the stall. Recovery briefings preserve that origin; they do not restart the
wait. The origin is the input the reply answers:

| Origin | Meaning |
| --- | --- |
| `last_voiced_pcm` | The latest voiced 10 ms input frame the room loop consumed. An energy threshold, not an acoustic speech-end detector. |
| `prompt` | A platform prompt sent to the model. |
| `transcript` | A candidate transcript with no voiced frame before it. |
| `provider_voice_end` | Gemini's end of speech with no voiced frame before it. |
| `provider_event` | Output nothing local asked for. Reads near zero by construction and is kept out of percentiles. |

Until the reply's first output, the latest input wins: speech after an idle
prompt, or a prompt after a stray noise frame, moves the origin. Voiced frames
after Gemini's end of speech do not, until Gemini reports a new start. Input
that arrives once output has begun, whether barge-in speech or a prompt queued
behind the generation, waits for the next reply instead of moving or ending
this one, and survives either a normal completion or an interruption. Trailing
transcripts alone do not queue another reply, and the runtime
late-transcript grace also applies to tracing.

Generations a thinking hold has cut off locally are closed then, as `held`,
discarding queued input from before that local cut. Their late provider events
are not attributed to the reply that follows. An `interrupted` before any output
keeps the clock, because the barge-in that caused it is the input still waiting
for an answer.

## Stages

| Stage | Written when |
| --- | --- |
| `tool_response` | A tool batch's responses were sent, with its execution, board resend and send times and the declared names of the tools it answered. A batch that failed to send is recorded but does not move `last_tool_response_ms`. |
| `audio_accepted` | The first chunk that completes a frame was accepted for framing; a shorter chunk before it waits in the frame buffer and is not this stage. Logged after the handler; `enqueued` says whether a complete frame entered the worker queue. |
| `native_audio_capture` | The worker finished LiveKit `capture_frame` for that chunk's first frame; `captured` says whether it succeeded. |
| `native_audio_cancelled` | That frame was cancelled before capture, including a chunk still waiting for enough PCM to form a frame. |
| `audio_enqueue_failed` | The first frame could not enter a full or closed worker queue. It is not a native-capture attempt or a successful latency sample. |
| `turn_complete`, `interrupted` | The provider ended the generation. |
| `held` | A local cut ended it. |
| `paused` | A local cut by the Pause button ended it. Only logs from before that button was removed carry it; `scripts/analyze-latency.py` still reads it. |
| `socket_replaced` | The socket was replaced. A reply without accepted audio keeps waiting (`continuing=true`), including tool-only generations. |

## Waterfall

With `CODETRIAL_GEMINI_TOOL_ACK` set, a tool round that holds back the reply a
candidate is waiting for, and has heard nothing of, queues a short
acknowledgement generated once per interview in the session's voice and accent
(`gemini-3.1-flash-tts-preview`, on the report keys). It plays at most once per
thing the candidate said, never while they are speaking again, and never
before a goodbye. It is not the reply: the watchdog and this record still wait
for the reply's own audio. Its first accepted chunk cancels the remaining clip
and clears buffered filler before the reply plays. Audio already transmitted
cannot be recalled. Cancelled queue entries are skipped without capture;
`output_queue_frames` can include them until the worker drains them. The record
stamps it as `tool_ack_ms`, and the summary's `tool_ack_lead` is the reply's native
capture less that stamp: how much sooner the candidate heard something. An
interview whose clip is not ready yet, or failed to generate, plays nothing.

Offsets share the origin and use the agent's monotonic clock. A milestone seen
before an origin that later moved is negative, and one never observed is
`null`. `observed_ms` may be negative when provider receipt precedes the first
local input. It uses a fixed session clock to order events even when a
later input moves the reply origin. The analyzer partitions origin to first native capture into
consecutive segments, so they sum to `server_total_ms`; `unattributed` absorbs
per-field millisecond rounding.

| Segment | Span |
| --- | --- |
| `candidate_yield` | Origin to the candidate choosing "Your turn is done", when that came inside the silence window. The candidate's own time; it replaces `silence_window`. |
| `silence_window` | A baseline allowance for a `last_voiced_pcm` origin: configured `GEMINI_SILENCE_MS`, capped by observed END or output. This is not measured provider endpointing time. |
| `endpointing` | The rest of the wait for Gemini's end of speech: uplink, provider voice detection, and whatever Gemini does before it reports END. |
| `response_start` | End of speech, or origin without one, to the first provider output. |
| `tool_decision` | The same span when that output was a tool call. |
| `tool_rounds` | First tool call to the last tool response sent, including any further model rounds between batches. |
| `post_tool_response` | Last tool response to first audio accepted. |
| `output_to_audio` | First output (an output transcript) to first audio accepted. |
| `output_queue` | Audio accepted to the worker dequeuing its first frame. |
| `native_capture` | LiveKit `capture_frame` of that frame. |

Overlays are independent spans, not additional waterfall segments.
`tool_handler_ms` is the local share of `tool_rounds` (execution, board resend
and send);
`provider_event_queue_ms` is from receipt of the provider message, before
parsing, to the start of the audio handler. It can begin before the reply origin when the
room loop consumes later microphone frames before handling that message.
`context_tokens` is the prompt size of the last single-generation turn before
the reply, and a cold socket clears it until its own first report. The summary
bands `beyond_window` by it (`context_0k_8k` and up) and fits
`context_slope_ms_per_1k`, a least-squares slope in milliseconds per thousand
tokens fitted within each kind of reply, so a long interview shows whether
replies slow as the conversation grows rather than that tool replies, which cost
more, come later. `stale_drop_ms`, `enqueue_ms` and `handler_tail_ms` split the audio handler
around the enqueue, the last being what the handler does after it, the
agent-state update included; `output_queue_frames` and `playout_ahead_ms` say how much
audio was already queued ahead of the reply's first frame, which separates a
first frame waiting behind earlier audio from one waiting on the worker. Adding overlays to the waterfall double counts time.

Gemini can report END in the same message as its first output, so END alone
does not separate waiting from answering; `end_with_output` counts the replies
whose first output came within 10 ms of END, where the segment named
endpointing also holds the model's own start. The configured silence window is a
comparison allowance, not proof of provider time spent waiting: explicit
yielding can end input sooner, and inference can overlap voice detection.
A yield ends the audio stream, so Gemini answers without waiting out the
window, and the trace stamps it as `candidate_yield_ms`. `beyond_window` is
total time outside the allowance or the yield, not measured inference. `transcript_before_end`
reports how long before END the first input transcript of the utterance
arrived, in whole milliseconds; zero means the words came with END and
cannot inform an earlier end-of-turn decision, and a negative value means they
came after it. The
dominant segment excludes both, and a reply is slow when its time
beyond the allowance reaches `--slow-ms` (1,500 by default). A reply at or past
`--stall-ms` (5,000 by default) is a stall. The threshold is a flag, not a
diagnosis: it sits in the gap between ordinary slow starts and sockets that
stopped answering, and a reply past it is reported as a rate, `stalls`, and
left out of every distribution, where a few would set the mean and the tail;
`beyond_window_with_stalls` keeps them in for the tail they set, `worst` still
lists them, and a sample of nothing but stalls is `SLOW`. Statistics separate origins, count only captured
replies, and report turns without capture by their last audio stage and by
whether a tool round came first (`unmeasured_by_tool_tool`,
`unmeasured_by_tool_no_tool`), since uncaptured replies are mostly the slower
kind. `kind_<origin>_tool` and `kind_<origin>_no_tool` split `beyond_window` by
whether a tool round came before the reply's first audio; evidence recorded
after it began does not make a tool reply. Percentiles
interpolate between ranks, to a tenth of a millisecond, and a statistic over
fewer than 20 replies is marked `small_n`: its p95 then lies between its two
largest values. Compare matched yield settings and input recordings; this
budget comparison cannot substitute for browser-perceived E2E measurement.

## Comparing two runs

`python3 scripts/analyze-latency.py --compare before.log after.log` compares
two logs kind of reply by kind; several sessions are pooled by concatenating
their logs. Two runs' overall medians mislead on their own: provider speed
moves a run's median by a few hundred milliseconds with nothing else changed,
and a reply preceded by a tool round waits for a second model pass, so a run
whose replies take fewer tool rounds is faster without any reply getting
faster. The comparison therefore reports, per origin, the share of replies a
tool round came first (`tool_share` in the single-log summary), the median of
each kind with a 95% bootstrap interval for its shift, and the change in the
mean split exactly into mix, from a different share of tool replies, and
speed, from each kind getting faster or slower. Stalls are compared as rates.
The intervals resample replies one by one and so ignore what replies in one
session share; interleave the two arms' sessions so that provider drift falls
on both alike.

For a pre-change soak baseline, build and run a checkout pinned to its recorded
revision. Use an isolated copy of the selected provider config when comparing
one project: the config directory's sibling provider files otherwise join the
pool. Keep credentials private and record only a project pseudonym and the
effective model, silence, timeout and compression settings with the synthetic
input's digest. A looping microphone clip needs quiet longer than the
configured silence window between utterances; it can interrupt replies when
the next iteration starts. Retain interrupted outcomes without attributing
their cause to overlap, provider behavior or VAD noise from these records alone.

The [checks outside the gate](development.md#checks-outside-the-gate) describe
the browser prerequisites. From the pinned checkout, select the private config
with `CODETRIAL_CONFIG_ENV`, the synthetic WAV with
`BROWSER_CHECK_BARGE_AUDIO_FILE`, and a durable output path with
`BROWSER_CHECK_CAPTURE`. The runner does not retain its server log: for
analysis, an external wrapper must follow the newly created temporary
`web.log` into a durable log before cleanup, keeping its read descriptor open
through deletion. Run the underlying invocation through that wrapper:
`BROWSER_CHECK_FLOW=soak BROWSER_CHECK_AGENT=rust scripts/browser-check.sh`.
`BROWSER_CHECK_SOAK_SECONDS` controls the observation window, not the harness's
URL interview duration. Analyze the archived log with
`python3 scripts/analyze-latency.py --json retained.log` and
`python3 scripts/analyze-gemini-usage.py retained.log`.

By default, the soak observes 660 seconds inside a 20-minute interview. It does
not execute a scripted Thinking or editor sequence, measure browser audibility,
or verify final report delivery. Keep its exit status, including inconclusive
77, and archive diagnostics outside the shell runner's temporary directory:
cleanup deletes its server log, and failed captures do not retain every latency
record. A run without the final Live usage summary has incomplete usage even
when per-turn observations survived. Repeated GoAway notices on one pending
socket can precede one replacement. The agent prints the first notice and each
doubling (`(notice N)` after the time), and the raw count is the `advisories`
field of the episode's start line, so count actual replacements by episode and
not by notice messages. This baseline is a smoke observation, not a long-interview
qualification or a performance comparison by itself.

## Paired per-session screen

`python3 scripts/analyze-latency.py --paired before.log after.log` pairs the
two logs by session (room and session number; the process id differs between
runs) and reports p95 for each predeclared metric: time beyond the silence
window, origin to native capture, and origin to first provider output. It
cannot be combined with `--compare`, and `--strict` holds both logs to what it
holds one to.

- Unit of evidence. A reply is not an independent sample: replies in one
  session share a provider connection, a context and a candidate. Each session
  contributes its own p95 per arm and a per-session delta, and the interval
  below resamples sessions, never replies. Pooling replies as independent
  samples understates the interval's width; a fixture of three sessions whose
  replies repeat one value each fails if the tool does that.
- Accounting. Per arm and metric the report gives sessions, samples as
  `valid/of`, `failed` (a reply that never reached native capture) and
  `missing` (a captured reply that did not record that metric), and the
  failure rate. `failed` is any reply without a native capture, so it includes
  replies a barge-in, pause or hold cut short; an arm with more of those reads
  a higher failure rate for reasons other than provider health. Only the
  first-output metric can be `missing` in a valid log, because a captured
  record must carry its total. Failed and missing replies stay in the
  denominator. The per-arm p95 pools every session of that arm, while the
  shift and interval use usable pairs only, so with unmatched sessions the two
  arms' p95 need not differ by the shift. Sessions are keyed by room and
  session number; a log that reuses both for different runs merges them. Replies
  owed to a provider event are left out, as elsewhere; stalls are kept, since
  p95 is about the tail. A session absent from one arm is counted as
  `only_before` or `only_after`, and a matched session with no valid sample in
  either arm as `no_valid_sample`; neither is paired. `usable` pairs are the
  ones that remain.
- Interval method. With two or more usable pairs, 2000 resamples of whole
  sessions with replacement, both arms' replies of a drawn session taken
  together; the statistic is the pooled p95 of the after arm less the pooled
  p95 of the before arm; the interval is the 2.5th and 97.5th percentile of
  those shifts. The generator is the standard library's `random.Random` seeded
  with 20260101, so the same logs give the same report. With fewer than two
  usable pairs there is no interval.
- Limitations. The arms are matched by session identity, not randomised. A
  percentile bootstrap of a p95 is biased and undercovers with few sessions or
  samples, and an interval over fewer than about ten pairs is not a coverage
  claim. Pooling gives larger sessions more weight in the pooled p95. The
  metrics are server-side origin to capture, not browser-perceived end to end.
- Reading it. The output always states that it is a descriptive regression
  screen and inconclusive for this budget, and prints no adoption verdict. In
  particular three pairs remain descriptive even when the resampled interval is
  narrow, and the 30% latency target is never declared met by this tool: that
  needs registered arms and a pre-declared decision rule, which do not exist
  yet.

## Socket replacement lines

A Gemini Live connection is closed by the server after about ten minutes, and
the room loop replaces the socket then or earlier. Two plain log lines, not
latency records, say how long that took. Both carry `room=`.

`replacement:` is printed once per replacement, when the new socket has been
set up. `rotation_gap_ms` is the time from handling the close to the new
socket finishing setup, so it includes the budget checks, a resume attempt
that was refused, and a cold open that followed it. It ends before the
briefing goes out. It is named apart from the `gap_ms` of the floor lines,
which measures something else. `resumed` says whether the
conversation continued from a resumption handle or was rebuilt from the local
transcript, editor and interview state. Three more fields say why. `trigger`
is `goaway` when an advisory was acted on, `close` when the server hung up
first, and `watchdog` when the reply watchdog gave up on a socket that was not
answering. `handover` is `resumed`, `cold_handoff` or `degraded`. `fallback`
is `none` when the conversation resumed, and otherwise `no_handle` (the replaced
socket had offered none), `resume_suppressed` (the replaced socket was itself
a resumed one replaced young, so resumption is withheld), `resume_refused` (a
handle was offered and the resume did not take) or `checkpoint_before_handoff`
(see below).

`replacement first audio:` is printed once, when the replacement's first reply
audio is queued, and only for a replacement whose briefing asked for a
reply: a speech handoff or silent context asks for none, and the audio that
follows answers the candidate. `since_start_ms` counts from the same start as
`rotation_gap_ms`, so it includes the briefing, any board resend and the
model's reply; `since_connected_ms` counts from setup. `candidate_spoke_ms` is
when the candidate was first heard after the replacement began, measured from
that start, or `none`. A value there means the audio may have answered the
candidate and not the briefing, and speech buffered during the gap is
timed from when it was processed, just after setup.

Next to those, one notice says how the replacement was brought up. `Gemini
session resumed` means the conversation continued from a resumption handle.
`Gemini session restart rebuilt locally` means the candidate was speaking and
no checkpoint post-dated their speech, so the socket was rebuilt from the local
transcript, editor and interview state on purpose, since resuming from an older
checkpoint can replay a reply that was cancelled. `Gemini session restart
degraded` means it was rebuilt without that reason: no handle, a refused
handle, or resumption withheld because the replaced socket was itself a young
resumed one. Each carries ` room=` after the text, so two rooms rotating at
once stay apart. These are for the operator; the browser-check soak reads the
fields of the `replacement:` line, not the notice.

The soak judges the fields by `scripts/soak-judge.cjs`. A `goaway` replacement
that resumed or took the designed cold handoff is the handover it looks for. A
rebuild from local state on any other trigger or reason fails it, naming the
trigger and reason. One rebuild after a reply timeout is allowed and reported
when the replaced socket had offered no handle or was itself a young resumed
one, since a provider that does not answer an utterance costs a replacement
that the rotation under test did not cause; in the default `strict` mode a
second one fails, and in `pilot` mode (`BROWSER_CHECK_SOAK_VERDICT_MODE`, see
docs/development.md) that cap alone is left out and the result says
`qualification=not_evaluated`. One whose handle was offered and refused fails
in both. A run in which
Gemini never sends a `GoAway` tested nothing about rotation and exits 77
instead of passing or failing.

The line is left out when the wait outlived the longest reply timeout. Audio
that late may have been drawn by something other than the briefing, and the
cutoff is conservative: a briefing reply held up by long tool rounds is left
out as well. It is dropped too when a turn ends without audio, a thinking hold
takes the floor, another prompt goes out, or a newer replacement takes over.
A replacement that never reaches the line therefore says nothing about its
latency, and the absence is not a measurement.

## Lifecycle identity

The lines below are appended to what is described above; no legacy field moved
or changed meaning, and the parsers keep reading logs without them. Every id is
a whole number that only grows within its scope, and a log joined from two
processes or sessions is told apart by `pid` and `session` (and `room`, which
is always the last `key=value` field of a plain line because a room name may
hold spaces). Nothing here is a handle, credential or content.

### Obligation fields on `reply_latency` records

An owed reply is a reply whose origin is a local input (`origin` is anything
but `provider_event`). It is owed until audio is first accepted for capture; an
episode that begins after that is not attached to it. It is numbered by its trace, so `obligation` equals
`turn` and is scoped by `(pid, room, session)`. A replacement does not mint a
new one: the reply keeps its `obligation`, `origin`, `origin_socket` and start,
and its `socket` moves with the swap. Every record of the reply carries:

| Field | Meaning |
| --- | --- |
| `obligation` | The reply's number, or `null` for output nothing asked for. |
| `obligation_outcome` | `open` before the terminal record; the terminal one is `answered` (audio was accepted for capture and its turn ended), `superseded` (a local cut, hold or barge-in ended it before any audio) or `ended` (its turn completed with no audio). `null` without an obligation. A reply with no terminal record is `missing` to the analyzer: it is inferred, never logged. |
| `owed_start_interview_ms` | The origin's time on the assessment clock. |
| `interview_ms` | The record's time on the same clock. |
| `obligation_episodes` | The replacement episodes that began while the reply was owed, oldest first, at most 8. |
| `obligation_episode_count` | How many there were, unbounded. |
| `first_accept_ms`, `first_accept_socket` | When the handler first queued audio for native capture, from the origin, and on which socket. Whether native capture then succeeded is the `captured` flag of the `native_audio_capture` record, as before. |

`python3 scripts/analyze-latency.py --json` adds `obligation` to each turn and
`obligations` to the report (`count`, `outcomes`, `carried`,
`carried_unanswered`). The key=value summary adds `obligations=`,
`obligation_<outcome>=` and the two carried counts only when a log has
obligation records. Duplicate copies of a record do not add an obligation: the
turn id is the key.

### Replacement episodes

An episode is one attempt to bring up a new socket, whatever its result.
`episode` counts attempts per `(pid, room, session)` from 1, and `replacement`
counts only the ones that installed a socket, so a failed episode consumes an
episode number and no replacement number. Three lines carry them:

`replacement start: pid= session= episode= trigger= old_socket= obligation=
obligation_origin= advisories= interview_ms= room=` is printed when the close
is handled. `obligation` is the reply owed at that moment (`none` when there is
none) and `advisories` the raw number of `GoAway` notices received on the
replaced socket, however many were printed.

`replacement end: pid= session= episode= result= trigger= old_socket=
new_socket= replacement= attempt= resume_tried= obligation= notice_ms=
setup_ms= connected_ms= opener_ms= install_ms= interview_ms= room=` is printed
exactly once per episode. `result` is `installed`, `failed` (a setup was tried
and refused), `no_setup` (the restart budget or billing refused before any
connection) or `cancelled` (a terminal intent took over). `new_socket` and
`replacement` are `none` unless installed. `attempt` is the consecutive restart
count the episode reached, and `resume_tried` whether a resumption handle was
offered. An episode with a start and no end is incomplete, for example because
the log was cut; the soak reports it and does not treat it as a pass.

The installed replacement's legacy `replacement:` line gains `pid= session=
episode= replacement= old_socket= new_socket= obligation= interview_ms=` just
before `room=`, so two swaps logged at the same displayed `at=` clock are two
replacements by id.

The milestones are separate on purpose. `notice_ms`, `connected_ms`,
`opener_ms` and `install_ms` count from the same start as `rotation_gap_ms` and
only grow: the reconnect notice returned, setup completed and was counted
(`connected_ms` equals `rotation_gap_ms`), the briefing, board resend and
queued work were written and the opener returned, and the socket was
installed. `setup_ms` is a duration inside that span: from opening the
replacement connection to its setup completing, excluding the notice and the old
socket's close. `rotation_gap_ms` therefore still includes the notice, and its
delay must not be attributed to provider setup; read `notice_ms` and `setup_ms`
for that. Each milestone is `none` when the episode never reached it.

### Socket pressure

The reader task decodes provider frames and hands their events to the room
loop through a channel that holds 256. When the loop is slow the reader waits
for a slot and the socket goes unread, which from outside looks like a provider
that stopped answering. One `codetrial latency` record with `"event":
"socket_pressure"` says which it was. It is written once per socket, when the
socket is let go: `end` is `replacement` (it was replaced), `terminal` (the
interview ended with it installed) or `discarded` (a socket set up for a
replacement was dropped before it was installed). A socket that is let go
twice, for example a replaced socket and then the exit path, is reported only
the first time. There is nothing per frame, and the record holds counts and
durations only: no content, handle or credential.

| Field | Meaning |
| --- | --- |
| `pid`, `session`, `socket`, `room` | The scope of the lifecycle ids above. `socket` is the number the socket had while installed. |
| `end` | Why it was written, as above. |
| `reader_end` | Why the reader stopped: `read_idle` (nothing was read for 45 seconds), `closed` (a close frame or end of stream), `failed` (a socket error), `credential` (the provider reported a key failure), `receiver_closed` (the room loop dropped its end) or `stopped` (stopped from outside, which is how a replacement or exit ends a healthy reader). |
| `interview_ms` | The record's time on the assessment clock. |
| `capacity` | Slots in the channel. |
| `occupancy_high_water` | The most events the channel held at once. A peak, not what it holds when logged; at `capacity` the reader had to wait. |
| `blocked_sends` | Sends that found every slot taken and waited. |
| `blocked_ms_total`, `blocked_ms_max` | Time spent in those waits, summed and the longest. A wait still open when the socket was let go counts up to that moment. |
| `received` | Events decoded from the socket, including one still waiting for a slot. |
| `consumed` | Events the room loop took from the channel. `received - consumed` is the backlog, the waiting event included. |

Time blocked on the channel and the 45 second read limit are separate
measurements. The limit only covers waiting for the next frame from the peer,
so a reader parked on a full channel is never idle, however long the wait, and
`read_idle` is never blocked time. A socket with `blocked_ms_total` near the
duration of a silence and `reader_end` of `stopped` was held up locally; one
with `read_idle` and no blocked time was waiting on the provider.

Blocked time is delay, not loss, while the socket stays installed. A loopback
test with a consumer taking one event per 100 ms for 60 logical seconds
(606 events, the reader blocked for about 35 of those seconds) receives the
exact decoded sequence, transcripts, tool calls, turn ends and usage
observations included, with the channel never above its 256 slots; the reader
only waits, and an event waits at most 256 consumer periods, a bound that is
not measured. Letting a saturated socket go drops what it had not yet
delivered: queued events and the one being sent are never read, the usage
already decoded is drained from the ledger, and frames still unread in the
socket are gone with it. While the reader waits it does not read the socket, so
a provider ping goes unanswered locally until it resumes. Whether a real
provider closes a connection for that was not observed, so the channel stays
bounded and no event is dropped or coalesced for it.

`python3 scripts/analyze-latency.py --json` adds `socket_pressure` to the
report (`records`, `sockets`, `duplicates`, `conflicts`, `partial`, the
blocked and high-water totals and one row per socket). Copies of one socket's
record, keyed by `(pid, room, session, socket)`, count once, a copy that lacks
a field is completed from another that has it, and copies that disagree are
counted in `conflicts`, once per copy, with the first kept; so is a copy whose
fields would make the merged socket impossible, and it adds nothing. A field a record leaves out, or
sends as `null`, is unknown: it is never summed as zero, a total nobody
reported is `null`, and the key=value summary leaves that line out. A record
that cannot be true (high water over capacity, more consumed than received,
blocked time without a blocked send) is rejected. The summary adds
`socket_pressure=` and its companion lines only when a log has these records.

### Interview clock

`interview clock: pid= session= start_unix_ms= elapsed_ms= sample= room=` is
printed once at the interview's start (`sample=0`, `elapsed_ms=0`) and then at
most once per whole minute of the assessment clock, from the room loop's watch
tick; a tick delayed past several minutes yields one sample. `start_unix_ms` is
the wall clock at the start, repeated on every sample so a cut or joined log
still has the anchor. `elapsed_ms` and every `interview_ms` read the same clock,
which a socket replacement does not reset, so a long harness can verify the
full duration from the last sample.

### Reading the lines

`scripts/soak-judge.cjs` exports `parseEpisodes(log, room)`, which returns the
episodes (one per `(pid, session, episode)`, with a missing end as `result:
"incomplete"`) and the end lines that match no start; `judgeEpisodes` reports
unresolved starts, unmatched ends, ids that do not grow and milestones out of
order; `parseClock` and `clockCoverage(samples, seconds)` read the clock
samples. `parseReplacements` deduplicates a line that carries all four ids by
those ids and a legacy line, which has none, by its displayed clock as before.

`parseObligations(log, room)` reads the `reply_latency` records of one room
into one entry per `(pid, session, obligation)`, and `judgeEpisodeReplies`
gives each episode `pass`, `fail`, `uncovered` or (before the end) `pending`
with a class and the reason: `answered`, `idle`, `held` and `retried` pass;
`not_replaced`, `no_reply_record`, `no_terminal`, `pre_rotation_audio`,
`wrong_reply`, `no_audio`, `no_accept_socket` and `conflict` fail; `truncated`
and `cancelled` are uncovered and fail the soak. Ordering uses the ids the
lines carry (episode, socket) and never the time a poller read them.

### Owed gaps and interview-hour exposure

`python3 scripts/analyze-latency.py` joins the reply records, the replacement
lines, the clock samples and the pressure records by identity
(`pid`, `room`, `session`, then the turn, episode or socket). A reused pid with
another `session` is another scope, and so is another room. Exact copies of a
record count once (`duplicate_records`), a copy of an episode or clock line
that disagrees is counted in `lifecycle_conflicts`, an episode keeping its
first copy and a clock sample leaving its scope without verified exposure, and
line order does not matter: the ids and the assessment clock order everything.
A last line with no newline may be cut anywhere, so the file reader does not
trust a lifecycle line there (`truncated_final`), nor one that stops inside
the start of a record's own prefix; a cut JSON record is rejected as before,
and `--strict` fails either.

`--json` adds `owed` to the report, and each turn with an obligation carries
`owed`:

| Field | Meaning |
| --- | --- |
| `owed_gap_s` | From the original obligation start (the smallest `owed_start_interview_ms`) to the first successful native capture, on the assessment clock. A failed capture does not end it. |
| `state` | `captured`; `superseded` or `ended`, which stop at their terminal record; `uncaptured` (answered, but no capture succeeded) and `missing` (no terminal record), which run to the observed end of the run. |
| `censored` | The last two states: the gap is a lower bound. The endpoint is the latest assessment-clock reading seen in the scope (any record, episode line, pressure record or clock sample). |
| `unknown` | `unknown_start`, `unknown_end` or `clock_backwards`. The reply keeps its state and counts everywhere, and has no gap and no part in a sum. |

A reply whose first native record failed and a later one succeeded is
`captured` here but not in the existing `captured` count, which reads the first
native record; `captured_after_failed_first` counts those so the two are
reconciled.

`owed` totals (`count`, `states`, `censored`, `unknown`, `known`,
`gap_s_total`, `gap_s_max`) count every owed reply, so a reply with no audio,
a censored one and one with an unknown endpoint all stay in the denominator.
`no_reply_owed` counts provider-initiated replies, which owe nothing, and
`missing_identity` counts local-input replies that carry no obligation (a
legacy log).

`owed_s_per_interview_hour` sums the union of the owed intervals of each scope,
so time owed through several overlapping replies, or through several
replacement episodes of one carried reply, is counted once, and divides by the
scope's exposure. Exposure is the latest verified `interview clock` sample,
extended to the latest record on the same clock, and a scope's clock is
verified only when sample 0 anchors the start at zero, copies of a sample agree,
the `start_unix_ms` anchor never changes and elapsed never decreases.
Otherwise the scope has `exposure_unverified` (`no_clock_samples`,
`no_start_sample`, `conflicting_samples`, `conflicting_anchor` or
`clock_went_backwards`), stays in `owed.scopes` and the counts, and is left out
of the rate. With no verified scope the rate is `null` and the key=value
summary prints `owed_s_per_interview_hour=unknown`, never 0. A reply with an
unknown endpoint adds nothing and a censored one adds only what was observed,
so `rate_is_lower_bound` is true whenever either exists.

Each replacement episode has a `class` in `owed.episodes`: `satisfied`,
`prior_socket`, `carried`, `unresolved` (the reply has no terminal record, so the
log may simply have ended), `unanswered`, `superseded`, `not_installed`, `no_reply_owed`,
`missing_evidence` (the episode names a reply the log has no records of),
`incomplete` (no end line), `missing_start` or `conflict`. Only audio accepted
on the socket the episode installed satisfies it. Audio from a socket at or
before the replaced one, or between it and the installed one, is
`prior_socket` whatever the reply's final outcome, and audio on a later socket
means the episode only `carried` the debt on to the next. A reply counts as
answered here only if some native capture of it succeeded, as for its gap. `owed.tracks` keeps two measurements apart:
`origin_to_native_capture_ms` is the existing server total from the reply's
origin, and `setup_to_first_output_ms` runs from the installation of the new
socket (episode start plus `install_ms`) to the reply's first provider output,
and is unknown where either endpoint is, or the output came before the install.
Only a satisfied episode has it: `setup_to_first_output_unknown` counts those
that lack an endpoint and `setup_to_first_output_not_applicable` the others.
Provider-initiated replies, which read near zero by construction, and stalls are
left out of `origin_to_native_capture_ms`, as of the existing distributions. An episode
whose copies disagree is a `conflict` whichever copy comes first.

`resources` totals what the lines before carry: sockets observed per scope
(from reply records, pressure records and episodes), `recovery_ingress`
(`dropped_bytes`, `unconfirmed_bytes`, `unsent_bytes`, `omitted_contexts`,
`omitted_evictions`) and `audio_quarantine` (`bytes`) per room, and the
existing `socket_pressure`. The `recovery ingress:` and `candidate audio
quarantine:` lines carry no pid, session or episode, so they cannot be joined to
an episode or told from a copy of themselves; `ingress_lines_exceed_episode_ends`
is true when there are more of them than episode ends (a driver prints at most
one, and a billing exit none, so fewer is normal), which a copied log causes,
and `room_counts_exact` is false then and whenever the log held a copied record
or a conflict, because the room totals may be doubled.

`task` summarizes the runtime's own count of live tasks. The interviewer prints
`runtime tasks: pid= session= elapsed_ms= sample= alive_tasks= room=` once per
30 seconds of the assessment clock, from the same watch tick as the clock
sample (a tick late by several periods yields one line, not a burst).
`alive_tasks` is Tokio's `num_alive_tasks` for the runtime the tick runs on,
which is stable in the pinned 1.54.1; on a multithreaded runtime it is an
approximate instant reading and it does not replace joining the reader task.
`alive_tasks=none` says no runtime answered. `task` has `samples`, `known`,
`unknown`, `alive_max`, `alive_last` and `scopes`. It is `null` (shown as
`task=unknown`) when the log carries no such line, as every log from before the
line existed does, and `alive_max` and `alive_last` are `null` (shown as
`unknown`) when no sample carried a number. Absence is unknown and is never
read as zero; a malformed count (`-1`, `x`, an empty value) is a rejected
record, not a zero, and a copy of a line that disagrees with its first copy is
a lifecycle conflict.

The key=value summary adds these lines only when a log has obligations or
episodes, so a legacy log's summary is unchanged.

### Control adapter

`--compare BEFORE_LOG --control-adapter SHA` reads BEFORE_LOG as a legacy control
produced by revision `SHA` and compares it with the log under test on the
metrics a legacy log shows: replies and captures, the stall rate, time beyond
the silence window by origin and tool round, and origin to native capture. The
revision must be one the adapter lists (it is a full 40 character revision,
checked against `CONTROL_REVISIONS`); another is refused. The log under test
must carry the lifecycle ids: every record whose origin is not
`provider_event` needs `obligation`, `obligation_outcome` and the keys
`obligation_episode_count`, `owed_start_interview_ms` and `interview_ms`
(a null clock reading is an unknown endpoint, a missing key is a missing id).
A log with no such record at all, that lacks them in even one record, or that
has any rejected record (a record with an obligation and no outcome is rejected
by the parser and would otherwise vanish) fails with a message on stderr and
exit status 1, and prints no comparison; there is no fallback that reads it
as legacy. A control log that carries them is not a legacy log and is refused
too. The output lists what the control cannot show and so is not
compared: owed gaps and censoring, owed seconds per interview-hour, obligation
outcomes, carried debt, replacement episodes, setup-to-first-output, socket
pressure, ingress, quarantine and omission counts, sockets observed and the
runtime task counts.

## What is not measured

Gemini Live is one combined audio and model pipeline. These records cannot
separate its speech recognition, endpointing, inference and synthesis.
Native capture also does not prove the candidate heard anything: LiveKit
transport, browser jitter buffering, autoplay and the audio device on the way
out, and input transport before the agent on the way in, are outside this
record. A browser-perceived figure needs a matched audio measurement in the
browser.

## Browser audio smoke evidence

The credentialed `voice` and `barge` browser-check flows also capture
`jimAudio`: received byte/packet deltas, peak decoded RMS, playback-blocked
state and a bounded result per observed audio subscription. The harness uses
LiveKit's public track-specific `getRTCStatsReport()` and an analyser on the
same native track attached as Jim. It records no device labels or audio
samples.

Sampling waits up to 120 seconds for Speaking, then samples at 100 ms
intervals for at most 10 seconds, stopping early once both counters grow and
RMS exceeds 0.001. Counter resets and receiver changes start a new baseline;
different track generations never share deltas. At most 64 subscriptions are
sampled; exceeding that limit is an unavailable observation, not a healthy
verdict.

`unavailable` means there were insufficient counter pairs or decoded samples,
no attached Jim track, no Speaking interval, cancellation, a sampling error or
a subscription limit. `transport-silent` means bytes or packets, either one,
did not grow across the observed counter pairs, and `decoded-silent` means both
grew but decoded RMS stayed at or below the floor. These are separate failures.
The capture retains failed observations, and each resubscription that keeps its
track through its sampling window must pass independently. A generation whose
track is replaced, unsubscribed or disconnected before Speaking or within its
first second of it, before both counters grew and RMS exceeded the floor, is
marked `superseded`, since losing the track cut its window short and a fresh
analyser first reads near zero; it cannot fail an otherwise healthy
replacement, and the last generation must pass. A generation that sampled
longer is judged on its evidence even if its track then ends. Unmeasured
counters and RMS are `null`, not observed zeroes. This measures transport and
decoded signal during Speaking, not output-device audibility, physical level or
per-word delivery. The RMS floor is a smoke-check threshold, not a calibrated
speech detector. Local loopback tests do not replace the isolated provider run.

## Validation

The credential-free gate covers the trace lifecycle, the waterfall and the
parser. For a live comparison, run the Rust voice and barge-in flows in
`scripts/browser-check.sh` against the same provider, region, input recording
and silence setting. `BROWSER_CHECK_CAPTURE` includes a `latencyLog` field with
the latency lines the harness saw; pipe it into the analyzer. The harness keeps
a bounded tail of agent output, so this is a smoke test, not an archive of a
long session.

Input audio is sent to Gemini one complete 10 ms LiveKit frame at a time. A
two-frame batch held its first frame for another frame period, including the
last frame of an utterance that ended on an odd frame, and the endpointing
clock does not start until Gemini has it. This removes local batching wait, not
provider inference or network time. It doubles websocket messages from 50 to
100 per second while sending the same PCM bytes. Partial frames still buffer
and flush when the track ends.

`silence_window` is usually the largest segment, and the one most directly
traded against interrupting a candidate who paused to think. The default is 1.5
seconds, down from 3 seconds, so a reply to unyielded speech starts about 1.5
seconds sooner. `END_SENSITIVITY_HIGH` is sent beside it; it is the API's
documented default, named so the window is measured against a known value, and
it earns no latency of its own. A candidate who pauses longer mid-sentence is
answered sooner too, which the thinking hold, `GEMINI_SILENCE_MS` and
`GEMINI_END_SENSITIVITY=LOW` exist to absorb; compare
interruptions as well as latency before moving either default again.

## Design lineage

The breakdown follows the per-turn analysis in livekit-traces-analyzer, which
splits each reply into speech recognition, end-of-turn detection, the
tool-decision model call, tool execution and the model call after it, with the
remainder shown as overhead, and which surfaced a silent loss of spans when an
export file was named differently. CodeTrial keeps the tool split and the
residual, joins asynchronous records by identity rather than by neighboring
lines, and refuses an empty analysis. It uses milestones measured on its own
clock instead of estimating provider stages it cannot see.

## Synthetic LiveKit audio suite

`python3 scripts/measure-audio-latency.py --output /tmp/audio-local` builds the
`audio-latency` example against this checkout's locked dependencies and repeats
three trials of the 100 ms and 20 ms native source queues, alternating their
order. Each trial contains one labelled warmup pulse and twenty measured
pulses. `--queues`, `--iterations`, and `--pulses` select the experiment. Output
must be a new directory, created only after the build succeeds; an earlier
experiment is never overwritten.

The default `--mode local` connects a native source to a local track sink and
needs no service. `--mode room` publishes to a fresh, uniquely named LiveKit
room and measures the remote decoded PCM sink. Load `LIVEKIT_URL`,
`LIVEKIT_API_KEY`, and `LIVEKIT_API_SECRET` into the environment first, using
the selected private config. No Gemini requests, candidate media, or interview
dispatch are involved. The room mode creates ordinary synthetic participants
and consumes LiveKit connection minutes. On Linux x86_64 the runner uses
`scripts/fetch-clang.sh` unless `CXX` already selects a compiler.

The probe sends 24 kHz mono PCM in ten-millisecond frames, delivered in 100 ms
bursts. A 100 ms, 1 kHz tone follows 300 ms of silence in each one-second trial
window, and a final silent second follows the last pulse. RMS hysteresis detects
an onset after decode. Both participants share one process and one monotonic
clock, so host clock synchronization is not an assumption.

An onset counts for a pulse only within the one-second delivery window after
its send, which ends no later than the next pulse's send. An onset in a gap
between the window's deadline and the next send is recorded as late. Without an
embedded pulse identity, a delivery slower than the whole period can land in
the next pulse's window and look fast there; the pulse it belongs to is then
missing, and under a constant overrun the warmup is always one of them.
Variable delays and losses can still alias individual samples; this suite does
not prove pulse identity or measure latency beyond its matching window. Zero,
multiple or late onsets make a sample unsuccessful, the warmup included. Raw
records include the send offset, first tone-frame capture completion, detected
delivery latency, and on-time and late onset counts. The probe writes them
before leaving the room, so a capture timeout or a failed unpublish or close
cannot discard what was measured; the file retains measurement errors and
teardown errors separately, and any of them exits nonzero. A pending teardown
marker is saved first, then updated after every unpublish and close has been
attempted. Each cleanup operation has a five-second deadline; a failed save
also cannot skip cleanup. The raw result uses schema version 3; earlier files
without teardown status are rejected. This is an acoustic onset estimate with
ten-millisecond frame resolution; codec ringing, loss concealment, scheduling
and jitter buffering can affect detection. It is not sample-accurate waveform
correlation.

`local` measures source buffering and sink delivery; `room` additionally
includes encoding, the network and SFU, and decode/jitter buffering. Neither
measures a browser's speaker output, microphone hardware, Gemini inference,
or production worker backlog. Capture completion is reported separately: it
can precede delivery. In the pinned native implementation the configured
source queue is a backpressure threshold; capacity is twice that threshold.
Changing it can move waiting into the application worker instead of making
the listener hear a reply sooner. Repeated trials are essential: an arbitrary
phase relative to the native ten-millisecond timer alone can change onset
latency by nearly a frame.

Each experiment saves `manifest.json` with revision, whether tracked files
differ from it, one digest over the manifest, lockfile, build script and every
file under `src/` and `examples/`, the executable hash, the Rust and C++
compiler versions, and host identity. Each trial saves raw JSON.
`summary.text`, `summary.json`, and `summary.csv` pool the measured samples by
mode and queue size, retaining attempted, delivered, missing and late counts
alongside mean, p50, p95, p99 and maximum for capture and delivery. Warmup is
excluded from the statistics, and a missing warmup is counted separately.
Percentiles interpolate ranks; fewer than twenty deliveries is labelled
`small_n`. These are descriptive sample statistics, not confidence intervals or
evidence that the settings caused a difference. Keep endpoint, host, workload,
binary and network conditions matched when comparing experiments, and inspect
individual trial files before pooling.

The report tool also serves as an acceptance check:

```sh
python3 scripts/audio-latency-report.py /tmp/audio-local/iter-*.json
python3 scripts/audio-latency-report.py /tmp/audio-local/iter-*.json \
    --format json --max-p95-ms 50 --max-missing-rate 0
```

Thresholds are experiment-specific and compare unrounded statistics; only the
printed values are rounded. The default refuses any unsuccessful sample. A
report with no delivered samples, or with a missing warmup, always fails,
including when a nonzero missing-rate budget is allowed. Invalid data, an empty
or interrupted run, a trial that left no result, failed or incomplete teardown,
invalid timing windows, or the same run given twice fails rather than producing
a healthy verdict; the runner then writes the refusal reason in place of
statistics, as a JSON error object and a one-column CSV in the machine-readable
summaries. The ordinary gate checks the report's validation, statistics, export
formats and budget exit statuses; real-room experiments are separate from the
credential-free gate.

### Lessons from lk-latency

The suite adapts the methodology of
[lk-latency](https://github.com/haffi96/lk-latency), a LiveKit video latency
harness: fresh rooms per iteration, raw timing archives, build identity and
pooled statistics. It keeps CodeTrial's released SDK rather than the
separately pinned revision that harness needs for per-stage hooks, so it does
not invent encoder, network or decode timestamps it cannot observe. Several
rules carry over. A run that never delivers media cannot pass. Tracks are
unpublished before the room closes, so shutdown negotiation does not look like
a media failure. Credentials reach the probe through the environment, never
argv. Every timing belongs to an explicit pulse, and an ambiguous match is
rejected rather than resolved in favor of the fastest. The endpoint is named
for what it is, decoded PCM at the sink, with no claim about audible output.
