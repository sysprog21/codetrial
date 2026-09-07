# Response window manual check

`node --test` can prove that `responseWindows` pairs the right rows and that
`web/replay.js` draws only the words it is allowed. It cannot say whether the
panel a reviewer opens reads as a set of durations or as a set of findings about
a candidate, and no exit code ever will. This is the procedure that asks, and
the table at the bottom is the evidence.

The fixtures are the other reason this exists. A suite of hand-written events
hid a panel where every window read "duration not recorded" for three review
rounds, because nothing in the suite produced the timing a real interview
produces. One real replay catches that class of failure and a hundred fixtures
do not.

Re-run this when the wording in `#replay-window-note` in `web/replay.html`
changes, when `WINDOW_WORDS` in `web/replay.js` gains or loses a phrase, when
`responseWindows` in `web/lib.js` changes what closes a window, or before
showing the panel to anyone outside the team.

## Environment

Recording is configured but never provisioned. That is deliberate and it is why
this check does not wait on cloud accounts: the replay records its events
whether or not Egress ever starts.

- A local server: `make web`, with LiveKit and Gemini credentials good
  enough for the interviewer to speak. Nothing here needs a deployment.
- `CODETRIAL_RECORDING_ENABLED=true`, plus the four keys `load_recording` in
  `src/config.rs` requires. None of them is contacted at startup:
  - `CODETRIAL_RECORDING_GCS_BUCKET` and `CODETRIAL_RECORDING_DRIVE_ID` are
    compared against nothing, so any syntactically valid value starts the
    server.
  - `CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON` only has to parse and carry a
    non-empty `client_email` and `private_key` (`readable_service_account` in
    `src/delivery.rs`).
  - `CODETRIAL_RECORDING_TEMPLATE_BASE_URL` only has to be an `https://` origin
    and path with no query, fragment or userinfo.
- A signed-in account with a verified email. `start_recording_handler` answers
  `403 recording_requires_verified_identity` without one, and `POST /api/login`
  does not supply one: the GitHub handle it collects is self-declared, so
  `verified_email` is `None`. Two ways to get one, and the row below records
  which was used:
  - Complete the GitHub OAuth flow, which is the real path.
  - Sign in through `/api/login`, then set `email` and `email_verified = 1` on
    that row in the accounts database, which is what `recorded_server` in
    `tests/web.rs` does. This is a local-only shortcut. Say so in the row; the
    next person should not have to discover it by failing.
- Headphones, so the candidate microphone does not re-capture the interviewer
  and turn one turn into two.

Egress will fail to start against this configuration. That is expected:
`start_recording_handler` leaves the recording in `starting` rather than
failing, and the replay keeps recording events either way. Step 2 confirms that
by reading the state rather than assuming it.

## Procedure

1. Bring the server up with the environment above and sign in as an account
   with a verified email.
2. Start an interview, agreeing to the recording notice. Open `/replay.html` in
   a second tab and confirm the recording appears and reads `starting`. If it
   reads anything else, the environment is not the one this document describes;
   record what it read and stop.
3. Sit the interview for several turns, so there are `avatar` rows to pair.
   Answer at least one question after a noticeable pause, and answer at least
   one immediately, so the table has a long window and a short one to compare.
   Leave at least one interviewer turn unanswered, so there is a window holding
   no transcript.
4. Pause the interview once, for long enough to notice, and resume it. This is
   the only cause of a long window the page can name on the window itself.
5. End the interview.
6. Open `/replay.html`, select that recording, and read the timeline.

## Pass criteria

All six must hold. Any failure is a fail row, and the row names which one broke.

- The windows carry durations. A panel in which every window reads "duration
  not recorded" is the failure this check exists for, and it is not a wording
  problem.
- The windows appear interleaved with the editor and test moments, in the order
  they happened, rather than grouped into a list of their own.
- Clicking a window shows the code as it stood when that question ended.
- The paragraph above the list (`#replay-window-note`) is hidden on a replay
  that produced no windows, and shown on one that did.
- An ordinary answered question does not say it holds no transcript. This is the
  delivery race `DONE.md` records a fix for, and a real interview is the only
  thing that has ever exercised it.
- The window covering the pause from step 4 says "interview paused during this
  window", and no other window does.

## The judgement

The pass criteria above are mechanical and a careful reader could argue them
from the source. The deliverable of this check is the sentence they cannot: in
the reviewer's own words, whether the panel reads as evidence a person is meant
to go and look at, or as an accusation the page has already made.

If the wording fails, name the sentence and say why, in the same row. The copy
is an allowlist in `tests/browser/history.test.js` and a static paragraph in
`web/replay.html`, so changing it is a two-line edit; knowing which sentence
failed is what makes that edit possible, and a row reading only "reads badly"
fires nothing.

## Questions only a reviewer can answer

Four entries under Candidate Improvements are waiting on a person who has read a
real replay. "Nobody asked" and "somebody decided no" are different states and
only the second one closes an entry, so all four get a written answer here even
when the answer is that nothing should change.

1. **Is the coarse window enough?** It measures the interviewer finishing to the
   candidate finishing, so it cannot separate the silence before an answer from
   the answer itself, and CodeTrial's own model round trip is inside the number.
   Splitting it would mean stamping the first transcript chunk, or publishing a
   third avatar state.
2. **Should the turn that closed a window be rendered beside it, or dropped from
   the payload?** `responseWindows` returns the text and time of that turn and
   the page reads only whether one exists. Either render it or reduce it to a
   boolean; both are small, and this asks which a reviewer wants to see.
3. **How many consecutive windows holding no transcript is worth marking?** Give
   a number. Two in a row is a candidate gathering their thoughts and five is
   something else, and no number between them has been observed. This is the
   shape the interviewer's own silence nudge leaves behind, and the panel cannot
   currently point at it.
4. **Was the pause mark alone enough, or does the break want its own timeline
   entry?** The page receives `lifecycle` rows and draws none of them, so it
   knows the interview was paused for six minutes and shows only that one window
   overlapped it.

## Privacy

The interview is the reviewer's own. Egress never delivered anything in this
configuration, so there is no cloud artifact: what exists is local rows. Delete
the recording afterwards and record the interview id and the date, not the
artifact. Do not use a real candidate's interview as the test subject, and do
not paste transcript text into the row.

## Results

| Date | Interview id | Verified email obtained by | Result | Judgement, and the failing sentence if any |
|---|---|---|---|---|
| | | | | |

## Answers

Recorded once, by the reviewer whose row is above. All four, whatever the
answer.

| Question | Answer | Reviewer |
|---|---|---|
| 1. Coarse window enough? | | |
| 2. Closing turn rendered or dropped? | | |
| 3. Consecutive empty windows worth marking (a number) | | |
| 4. Pause mark enough, or its own entry? | | |
