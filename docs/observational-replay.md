# Observational audio replay

A named fixture identifies synthetic audio and the scenario it is meant to
exercise. It is an observation input, never an instruction to change Gemini's
endpointing, interruption, hint, recovery or scoring policy. The
[provider floor contract](turn-timing.md#provider-floor-observations) defines
what a provider observation means. Neither a PCM level nor transcript text
establishes provider activity or physical audibility.

The offline loader is implemented. The browser and socket observational
adapters and trial judge described below are implementation requirements, not
claims of working live replay. The existing `barge` browser smoke is separate:
it still requires Speaking followed by a measured interrupted overlap.

## Existing entry points and missing observations

| Entry point | Observable today | Replay work required |
| --- | --- | --- |
| `scripts/barge-replay.cjs` | Page `#agent-state` text, microphone acquisition, decoded sample rate, duration and monotonic injection time; state rechecked after decoding/resume | Preserve this smoke; a separate named replay must support outcomes other than interruption and bound the gate wait. |
| `scripts/browser-check.cjs` | Room identity, Rust agent streams before tail trimming, capture JSON | Retain full-run latency alongside floor records and correlate each trial to the provider clock/record opening, not emission order. |
| `scripts/floor-capture.cjs` and its shell follower | Complete floor lines, including session/socket/sequence, retained beyond server cleanup | Reuse this writer. A highest *emitted* sequence is not an opening watermark. |
| `src/livekit/session.rs` tool response diagnostics | Allowlisted tool name, accepted response and separately the latency record's send outcome | Hint/tool gates need fresh accepted-and-sent evidence and invalidation at the reply/socket boundary. A caption, old hint count or accepted-but-unsent response cannot open a gate. |
| `tests/unit/livekit/cost.rs` spoken probes | Fixed restatement/question PCM, provider events and tool responses, streamed concurrently in 40 ms chunks | Add named input and a gated driver; these probes do not currently feed the production floor recorder or retain full-run latency. |
| `src/livekit/session.rs` / `src/livekit/floor.rs` | Provider activity mapped through room eligibility checks to floor observations | A direct socket driver must preserve this eligibility/mapping or label its evidence as harness-mapped, never as room/LiveKit acceptance. |

## Fixture version 1

One manifest describes one clip and one attempted injection. It is UTF-8 JSON
of at most 16,384 bytes. Every listed field is required; duplicate keys, unknown fields, invalid
types and unsupported versions are refused. Audio is a regular file beside the
manifest, named by a single filename (1–128 ASCII letters, digits, `.`, `_` or
`-`, starting with a letter or digit). No paths or symlinks. The manifest must
also be a regular file, not a symlink.

```json
{
  "version": 1,
  "id": "short-backchannel",
  "origin": "synthetic-speech",
  "audio": {
    "file": "short-backchannel.wav",
    "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
    "format": "wav_pcm_s16le",
    "sample_rate_hz": 48000,
    "channels": 1,
    "pause": null
  },
  "gate": {
    "state": "speaking",
    "tool": null,
    "timeout_ms": 30000,
    "max_age_ms": 250
  },
  "observation_ms": 10000,
  "expected": { "kind": "overlap", "outcome": "continued" }
}
```

Replace the placeholder digest with the lowercase SHA-256 of the complete audio
file's exact bytes, including a WAV header. `id` is 1–64 lowercase ASCII letters,
digits or hyphens, starting with a letter. `origin` is `synthetic-speech` or
`synthetic-tone`: consenting team speech/TTS or deterministic tone fixtures,
never candidate recordings. A tone can test transport mechanics locally; it
cannot establish a provider's speech, backchannel or interruption behavior.
Do not include transcript, credentials or candidate identity in a manifest.

`audio.format` selects the transport and exact format:

| Transport | Format | Sample rate | Limits |
| --- | --- | --- | --- |
| Browser | `wav_pcm_s16le` | 48,000 Hz, mono, signed 16-bit little-endian PCM in RIFF/WAVE | Positive duration up to 30 s; at most 2,945,536 file bytes (30 s plus 64 KiB for headers/chunks). |
| Direct socket | `pcm_s16le` | 16,000 Hz, mono, headerless signed 16-bit little-endian PCM | Positive duration up to 30 s; at most 960,000 bytes; whole 2-byte frames. |

WAV must contain exactly one `fmt ` and one `data` chunk, consistent file/chunk
lengths, PCM tag 1, 16 bits, block alignment 2 and byte rate 96,000. Ancillary
RIFF chunks are permitted within the file cap. Known RIFF/RIFX/RF64/WAVE,
FORM (AIFF), OggS, fLaC and ID3 prefixes are refused
on the raw path. This is not a universal container detector: headerless PCM has
no self-describing rate/channel/endianness metadata, so its declared provenance
and any unrecognized encoding must be checked by the fixture author.
The loader verifies the digest and byte structure before any provider access.
The browser adapter must also retain the decoded buffer's actual rate/duration.
Socket streaming uses 1,280-byte / 40 ms chunks; a final partial chunk retains
all its whole frames without padding or dropping samples. Any transport-added
silence and its duration must be explicit in the trial artifact.

## Gate and observation windows

`gate.state` is `speaking`, `listening`, `hint` or `tool`.
Speaking/listening use `tool: null`; hint requires `tool: "log_hint"`; tool
requires a lowercase ASCII tool identifier, at most 64 characters, starting
with a letter. The identifier names the required event; spelling a name in a
manifest does not establish that it exists or was observed.

A gate describes where to attempt injection, not a guarantee of the expected
provider outcome. For example, a listening gate does not establish an overlap;
the judge must require an actual aligned overlap record and cannot pass on the
gate alone. Disagreements between required scenario and observed injection
state are excluded; missing provider activity is unknown.

`timeout_ms` bounds waiting for the gate, 1–120,000 ms. `max_age_ms` is
1–5,000 ms and no greater than `timeout_ms`; it bounds the age of the last
qualifying state observation at injection. Decode audio before the final gate
check. Recheck after every asynchronous step before injection, including audio
resume and budget admission. Expire hint/tool eligibility at any boundary or
replacement that invalidates it. Retain the observed state and its source,
clock, age and session/socket identity. Browser text alone supports only its
actual speaking/listening indication; an adapter without a trustworthy hint or
tool observable must refuse with `unsupported_gate`, not treat it as Speaking.
The full replay implementation must supply these observables for both paths.

`observation_ms` is 1–120,000 ms of observation **after the final audio frame is
injected**. It is independent of the preceding gate timeout. Its end bounds
which provider events can determine an outcome, not when a delayed floor line
arrives. Include the injection and any internal pause in the evidence window.
After the window, finalize and drain retained evidence; never inject a follow-up
that could turn an unresolved observation into a successful trial. A teardown
may flush a previously resolved pause. A response first occurring after the
window cannot pass merely because its record arrived during finalization.

Allowed expected pairs are:

| Kind | Outcome | Scenario |
| --- | --- | --- |
| `overlap` | `continued` | Candidate backchannel during output; generation completes and playout estimate drains without interruption. |
| `overlap` | `interrupted` | Provider interruption or an eligible local stale-playout drop resolves an observed overlap. |
| `pause` | `respond` | New generation's tool call or accepted audio resolves the candidate pause. |
| `pause` | `continue` | A new candidate START resumes after the clip's internal pause before a new interviewer response. |

For `pause/continue`, `audio.pause` is an object with integer `start_ms` and
`end_ms`: a nonempty interval strictly inside the clip. Integer milliseconds at the
allowed rates are always frame-aligned. Samples
in that interval must have absolute amplitude at most 500, with signal above
500 both before and after it. All other scenarios use `pause: null`. This
annotation makes a resumed-pause input possible; it is not evidence Gemini
emitted either activity signal. Human labels belong to the separately frozen
corpus. These four pairs are expected outcomes; an observed `unresolved` is
incomplete evidence, never a fifth passing outcome.

## Trial identity, evidence and verdict requirements

An emitted floor line cannot be joined to an injection by file position, max
emitted `seq`, or the floor line's `at` (coarse emission clock). Records open
before they emit, and can emit out of sequence. The adapters must establish an
opening watermark or explicit trial-to-provider-event join before injection,
with room/session/socket identity. Until that join and an event-window clock
mapping exist, a record is ambiguous and cannot pass. A socket replacement
invalidates trial alignment. Start with one fixture attempt per fresh session;
multiple trials require proof no pending observation can leak between them.

Retain, before trimming any diagnostic tail:

- Manifest and clip digests, fixture ID, executable commit/digest, model/config
  identity and contract tuple; no secrets or transcript.
- Gate wait start, selected/observed state, evidence source, observation time,
  injection start/end and their monotonic clock identities; actual audio format
  and transport-added silence.
- Full-run floor and latency artifacts with room/session/socket identities,
  record opening/settlement window evidence and finalization/drain status. A
  rolling `latencyLog` tail is insufficient. Missing logs stay missing.
- Verdict and reason; aggregate `trials_started`, `injected`, `scored`,
  `excluded`, `unknown`, `passed` and `failed` counts, with exclusion reasons.
  Misaligned/failed trials still consume the configured run budget.

Verdict precedence: an invalid fixture refuses before connecting. A gate
failure (`unsupported_gate`, `gate-timeout`, `misaligned`, `missing-microphone`,
`decode-or-resume-failed`, `unavailable`) excludes the trial. Missing, ambiguous,
truncated, unresolved or out-of-window evidence is `unknown`. Only complete,
aligned evidence can fail on a wrong outcome or pass on the expected outcome.
Every attempted trial counts; no retries erase a failure. Counts and artifacts
must survive failure, not just the successful exit path.

Live adapters must refuse absent explicit isolated configuration and run
budgets before joining a room or connecting a socket. Their action/time/exposure
caps use the existing [run-budget controller](development.md#run-budget-for-live-validation)
where its protocol applies. Report provider usage as observed or unknown, not
as an enforced invoice ceiling. One clip/manifest is not a trial-count budget.
No live acceptance is implied by the offline validator or local doubles.

## Offline validation

```sh
node scripts/replay-fixture.cjs /absolute/path/manifest.json browser
node scripts/replay-fixture.cjs /absolute/path/manifest.json socket
node --test tests/browser/replay-fixture.test.js
```

Each validation command reads one manifest and its audio without connecting,
prints metadata and exits 0, or prints the input error and exits 2. Output
includes the manifest-byte digest, decoded frame count/duration and byte count,
not audio bytes. The tests generate deterministic WAV/PCM fixtures in temporary
directories and remove them afterward; their outcomes prove validation mechanics.
