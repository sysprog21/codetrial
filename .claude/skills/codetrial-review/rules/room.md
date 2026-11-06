# The room loop, Gemini and the provider bill

`src/livekit*`, `src/gemini*` and `src/dispatch.rs` run the interview itself:
one `select!` that has to keep pumping audio, a provider that bills by the
turn, and operator log lines the docs tell people to count. Where the code
lives and how LiveKit behaves is
[codetrial-livekit](../../codetrial-livekit/SKILL.md); these are the defects
reviews keep finding there.

## Never block the room loop, and never swallow events while waiting

Applies to: `src/livekit.rs`, `src/livekit/*.rs`

`run_room` runs everything that has to keep happening during an interview from
one `select!`: candidate PCM pumped to Gemini, Gemini output, room events, the
watch tick, the keep-alive. Look for new awaits inside it, or ahead of it, that
can take seconds: a startup probe that waits `gemini_reply_timeout_s`, a board
handover that waits `BOARD_FINAL_WAIT`, notices published one after another
around a sleep. While such an await runs, no microphone audio is pumped (the 40
s `queue_size_frames` overflows), no keep-alive is sent (`READ_IDLE_LIMIT`
quietly caps the configured timeout), and nothing handles departures. Look just
as hard at helper loops that read the room's shared event receiver and drop
whatever they are not looking for (a loop waiting for one reply, an arm that
discards events while a publish is pending). A `Disconnected` or a
`report_received` consumed there never reaches `deliver_report` or
`recover_report`. A packet with `participant: None` that arrives right after
the candidate leaves has to be handled the same way `is_report_receipt` already
does it (`sender.is_none() && candidate_gone`), not rejected. A candidate's
`ParticipantDisconnected` is not final: a full LiveKit rejoin under the same
identity has to go through `presence.left`'s grace, not end the interview.

Seen: 6 PRs. An opening-probe wait that blocks mic pumping and turns a
reconnect during the greeting into the end of the interview. A skip-watcher
that consumes `Disconnected`, so delivery publishes to an empty room.

Not a finding: an await bounded by a sub-second constant that already sits on
the escape-wait budget (`REPORT_ESCAPE_WAIT_MS`), or a shared deadline across
delivery windows that a test pins. Those are deliberate.

## Every exit emits its operator log line exactly once, with `room=`

Applies to: `src/livekit*.rs`, `docs/provider-cost-and-degradation.md`,
`docs/livekit-connection-troubleshooting.md`

Operators diagnose from `codetrial report_delivery room=...
outcome=acknowledged|unconfirmed|failed` and from lines such as `turn_timing`
and the cut line. `docs/provider-cost-and-degradation.md` lists the outcomes
and tells readers to count them. Flag a new early `return` or `?` in
`deliver_report`, `publish_with_recovery`, `run_recovery`, or the room-loop
exits (candidate gave up, billing exits, "LiveKit event stream ended") that
skips the shared tail which prints the line. Also flag a line that now prints
twice (once in an early-delivery helper, once in its caller), a line deferred
to a later `close_turns` that some exits never call, and a new failure line
without `room=` (`dispatch` runs several interviews in one process). When a
line is renamed or dropped, the troubleshooting doc that quotes it has to
change in the same PR.

Seen: 5 PRs. A skip during recovery returns `Ok(())` with no outcome line. A
lost-report line without `room=`, so two concurrent failures cannot be told
apart.

Not a finding: a teardown helper that already prints for both of its callers.
Suggest extracting it, and do not ask for a second print.

## Every billed provider call leaves a usage record, and new billed context is stated

Applies to: `src/livekit*.rs`, `src/gemini.rs`, `src/web/setup.rs`

The `gemini ... usage` line is written only after a response is decoded, and
the cost doc says a call whose usage was never observed is a gap. Flag paths
that drop a future whose HTTP request may already be at the provider (a skip or
disconnect arm that cancels `generate_report_bounded`), and failure paths after
a billed greeting that return without draining the observed usage. Flag changes
that add realtime images or text to the live context more than once (resending
a board JPEG the model already has: every later turn is billed on it). Flag
candidate-facing cost text that leaves out what actually reaches Gemini (board
images in whiteboard and example-drawing modes, code, grounding) or that does
not say who pays.

Seen: 5 PRs. A skip arm that abandons an in-flight report call with no usage
line. A board resent on every handover even when `send_if_due` just sent it.

Not a finding: `model_input_bytes` leaving out images. It counts server-built
text by design, and image bytes need their own counter.

## Gemini failures: mark the socket dead, keep the cause, keep failover

Applies to: `src/gemini.rs`, `src/gemini/credentials.rs`, `src/livekit.rs`,
`src/livekit/open.rs`, `src/main.rs`, `src/config*`, `src/dispatch.rs`

A socket write that errors, as opposed to timing out, has to set `dead` and end
the reader. An idle break has to do the same. Otherwise the next flush pays the
full `WRITE_TIMEOUT` and callers that log and continue wait on a close that
never comes. Restart budgets must not be reset with `&mut 0`: that hands the
first open the whole `GEMINI_RESTART_LIMIT` (minutes with 15 s connect and
setup timeouts), or forgets earlier failed attempts. Key rotation
(`GeminiKeys`): check that `from_response` and `failure_from_reason` classify
the same Google reason codes (the `API_KEY_*_BLOCKED` restriction family,
code-form `RATE_LIMIT_EXCEEDED`), that `invalid` marks expire, that a sole key
is not cooled down for every interview in the process, and that rewrapping
errors keeps `retry_after` and the billing/quota cause, so the candidate is not
told "unavailable". Replacing `open_cold_session`/`retry_cold_open` with a bare
open drops failover to backup keys. `GOOGLE_API_KEY` vs `GOOGLE_API_KEYS`
precedence has to live in one place: blank values, per-provider overrides, and
redaction with an empty `google_api_key` have each regressed.

Seen: 6 PRs. Backoff indexed on calls made, so semantic repairs inflate the
first 503 wait to 4-8 s. `check-gemini` calls the single-key opener and fails
even though a backup key works.

Not a finding: a doc or design comment that states a deliberate trade-off, such
as the quota floor or the hosted-default wait when `/runtime-config.js` fails
to load.

## Do not repurpose a timeout, and keep stated durations true

Applies to: `src/livekit*.rs`, `src/config*`, `src/web/token.rs`

A constant sized for one job changes meaning when it is reused.
`RECONNECT_NOTICE_LIMIT` (1 s, for a droppable notice) made fatal for the ready
notice. `CODETRIAL_GEMINI_FIRST_OUTPUT_S` (recovery eagerness) deciding when
the whole interview ends. A browser `TOKEN_REQUEST_TIMEOUT_MS` equal to the
server's `TOKEN_BODY_READ_LIMIT`, so the browser gives up before the server's
408 arrives. Also flag durations shown to the candidate that no longer match
the wait: "did not respond within 20 seconds" after two 20 s attempts, "about 2
seconds" after sequential notices, `REPORT_TIMEOUT` reported as "Gemini did not
answer" when it actually covered repairs and sleeps.

Seen: 6 PRs. A 5 s first-output knob, tuned for fast recovery, that now ends
interviews whose greeting takes 6 s.

Not a finding: one deadline shared across delivery attempts that a test ties to
the browser escape wait.
