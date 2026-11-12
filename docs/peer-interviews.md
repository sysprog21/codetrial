# Reciprocal mock interviews

Two GitHub-authenticated users take turns as candidate and interviewer. A
subscribe-only AI participant listens silently, transcribes the conversation
and gives each member technical suggestions after the session. It publishes no
microphone, audio, data message or live hint. Human feedback remains separate
from AI suggestions and never becomes a CodeTrial score.

## Joining and pairing

1. Sign in with GitHub and open Reciprocal practice. Choose a nickname before
   opening the participant list. Nicknames contain 1 to 24 ASCII letters and
   digits, with at least one letter; `Lucky7` is valid. Case is preserved.
   Nicknames need not be unique: the GitHub handle remains visible beside them
   to distinguish people. Nicknames never confer identity or permissions.
2. Choose topic, difficulty, language, round length, first interviewer and the
   independent **Save transcript** and **Save audio** preferences. Both, either,
   or neither may be selected. The browser runner supports C, C++, Java,
   JavaScript and Python.
3. Open availability. The server creates a LiveKit waiting room before
   publishing the opted-in user's nickname and GitHub handle to authenticated
   peers. Waiting joins publish no microphone. Merely signing in does not
   advertise someone. Availability heartbeats expire after 45 seconds; close
   availability to leave the list and cancel pending requests.
4. Another signed-in user chooses a nickname and requests that participant.
   Both users see the same immutable proposal, including the persistence
   choices, and explicitly accept the AI observer notice. Changing a proposal
   means making a new one; an old confirmation cannot authorize changed choices.
   Only the two confirmed members may subsequently enter the session.
5. Enter, connect audio and mark ready. Both confirm Start. The silent observer
   must connect before the actual round clock starts; a failed observer join
   leaves the clock stopped. The two problems are locked at start, and the
   next round's problem stays undisclosed until the swap.
6. The candidate owns the editor and test runner; the interviewer sees the
   workspace read-only, a guide with three hint rungs, and private page-local
   notes. Finish seals the round, then give human feedback. Both confirm the
   swap; the other member becomes the candidate on a different problem of the
   same difficulty. Complete the second round, then collect AI suggestions.

Private invite links remain available for a prearranged partner. They use the
same fixed membership, consent, audio readiness and two-round controls.
An account may have 20 sessions nobody has entered yet, which are its
invitations, accepted or not, and any pairing that has not reached the lobby. A
further invitation is refused with `too_many_invitations` until one is entered,
cancelled or expires. A pairing both people confirmed is never refused for it.

## Configuration

Reciprocal practice is offered like any other feature once its prerequisites
exist: a LiveKit provider, the account session secret, database,
`GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`, and `GOOGLE_API_KEY` or
`GOOGLE_API_KEYS`. Without both OAuth app credentials or the silent observer's
Google keys it is simply not offered, and the session reply reports it as
unavailable.
OAuth identity is sufficient: a verified email address is not required.

| Variable | Default | Meaning |
| --- | --- | --- |
| `CODETRIAL_PEER_MAX_SESSIONS` | 8 | Shared cap for waiting rooms and bound sessions |
| `CODETRIAL_PEER_ROUND_MINUTES` | 25 | Default round length |
| `CODETRIAL_PEER_HARD_MAX_MINUTES` | 90 | Cutoff from round 1's actual start; must cover two rounds |
| `CODETRIAL_PEER_INVITE_TTL_DAYS` | 7 | Private invite lifetime, capped at 14 days |
| `CODETRIAL_PEER_RECONNECT_GRACE_SECONDS` | 120 | Heartbeat presence grace |
| `CODETRIAL_PEER_AUDIO_DIR` | unset | Opt in to private local audio artifacts |
| `GEMINI_REPORT_MODEL` | existing report default | HTTP transcription and post-session suggestions model |

Audio saving is offered only when the operator configured a private writable
`CODETRIAL_PEER_AUDIO_DIR`. On Unix its directory must have mode 0700; new
directories and WAV files are created with 0700 and 0600 respectively. It must
not be under `web/` or a publicly served directory. The silent observer captures
at most two hours, so every configuration limits the hard maximum to 120
minutes. Allow up to 460.8 MB per two-hour stereo
recording, plus transient files, and provision disk for the 24-hour retention
window. Normal 90-minute sessions need up to 345.6 MB each.

Waiting rooms and active sessions share the
[LiveKit connection-minute quota](provider-cost-and-degradation.md#what-bounds-concurrency).
The server reserves room capacity before publishing availability. Confirming an
availability proposal transfers that reservation to the pending session, so
both confirmed members can enter even at the session limit. Confirmation starts
no round countdown. A discovery pair still pending
ten minutes after confirmation is cancelled. An abandoned lobby returns to
pending after ten idle minutes; discovery pairs then expire, while private-invite
sessions retain their configured invitation expiry.
The waiting-room participant limit is three: the two humans and silent observer.
A background worker settles idle/expired sessions and deletes rooms with retries.

## Saving choices and AI processing

Both members must confirm the same choices before capture starts. No choice
allows capture before both consents or playback to a third account.

| Save transcript | Save audio | Persisted raw conversation |
| --- | --- | --- |
| No | No | Neither transcript nor audio |
| Yes | No | Text transcript only |
| No | Yes | Audio only |
| Yes | Yes | Both |

Regardless of saving choices, the observer processes bounded audio chunks in
memory through Google's transcription API and uses a bounded temporary
transcript to generate suggestions after the session. This is temporary AI
processing, not a promise that audio never reaches a provider. Unselected audio
is never written to a recording file. Unselected transcripts are never written
to the transcript table and are discarded after the same session's suggestions
finish. They are not recovered across a server restart or reused for another
session. Suggestions do not retain verbatim transcript excerpts when transcript
saving was not selected.

Saved transcripts, audio and AI suggestions expire 24 hours after completion.
Reads enforce expiry even before the cleanup worker deletes the artifact.
Only session members may export a saved transcript or audio; each member reads
their own AI suggestions. Either member can delete the shared saved transcript
or stop/delete shared audio. Deleting audio stops its writer before unlinking
files. This does not stop the previously agreed temporary AI processing and
suggestions; terminate the session to stop the interview. A downloaded copy
cannot be recalled by deleting the server copy. A withdrawal made before capture
starts is recorded, and the observer then declines to record that audio.

The observer's connection is not retried: if it drops mid-practice, capture and
the AI suggestions for that session are lost, and the members' own practice
continues. Advice is generated from partner speech, so a partner can influence
the wording of the suggestions you receive; they are shown as plain text only.

Choosing neither raw saving option does not delete the ordinary workspace
snapshots, submitted human feedback or session metadata. Those are kept for
30 days after a session was created, and only once it is complete or cancelled
and its room and any audio file are cleaned up; an hourly worker then deletes the session with its members, rounds,
snapshots and feedback. Finished lobby entries and pairing requests go on the
same schedule. The record of which problems a member has seen is kept so
that later practice does not repeat them. The consent notice states this
distinction. Interviewer notes
remain only in the page and are lost when it closes. Peer audio is separate
from AI interview recording and has no email or Google Drive delivery.

Only technical content may support suggestions. Accent, filler words, eye
contact, nervousness, confidence and personality are not assessed. Transcription
gaps or interrupted capture produce unavailable or incomplete suggestions,
not invented evidence. No live advice is sent to either participant.

## Workspace and control API

HTTP and SQLite own membership, roles, clocks, snapshots and feedback. LiveKit
carries human audio, disposable editor previews and the observer's subscription.
Previews are accepted only from the stored identity of the current candidate.
A writer lease and revision CAS prevent an older tab from saving after takeover.
After a lease expires, Recover edit access preserves the candidate's unsaved
code and renews access only if another tab has not replaced that lease.
Interviewers refresh authoritative snapshots and test results while polling
and when the round seals, including after missed disposable previews.
The candidate saves snapshots every five seconds and on finish. A 30-second
final-capture window precedes sealing if the candidate does not send a final save.

All peer APIs are under `/api/peer/`, require OAuth identity, and guard unsafe
requests by same-origin `Origin`. Profile routes are `GET/PUT profile`;
availability uses `POST/DELETE availability` and `POST availability/heartbeat`.
`GET lobby` publishes the roster; pages poll it every five seconds. Pairing
uses `POST pairing-requests` and `POST pairing-requests/{id}/confirm|decline`.
A pairing confirmation carries explicit `observerConsent: true`.

Session state includes `sessionId`, `status`, `version`, `serverNow`, `settings`,
`members` (slot, nickname, login, opaque identity, entered and ready),
`you.slot`, `you.observerConsented`, `currentRound` and `hardDeadline`.
Round state includes index, status, requesting member's role and deadline.
Times are Unix seconds. Start and swap confirmations carry the latest session
`expectedVersion`; a conflict requires refreshing state, not silently confirming
another proposal.

Round workspace paths are `sessions/{id}/rounds/{n}/writer|snapshot`, with
zero-based round indices. Recovery requests to `writer` carry `previousLease`
for an atomic comparison. Snapshot writes carry a lease, `expectedRevision`,
language, code, tests and final flag; the code limit is 48 KiB of UTF-8, and
the server keeps only the runner's known test fields, cutting long text. Body
ceilings (2 MiB for a snapshot, 256 KiB for feedback) sit above anything those
limits accept, counting JSON escapes. Writes have a separate 30/minute
per-account bucket. Feedback keeps only the rubric fields, each text field at
most 1,500 bytes. Draft human feedback remains private until both members
submit for that round or 24 hours after sealing. A member must have submitted
round 0 feedback before confirming the swap (`feedback_required`), because the
swap replaces that round's form.

Member-only `sessions/{id}/advice` and `transcript` expose observation results;
`DELETE transcript` withdraws the saved text. `audio/status` reports capture
availability/state, `GET audio` downloads a private WAV, and `DELETE audio`
stops/deletes it. These are separate from agent reports and recording history.

## Staging acceptance check

Use two real GitHub accounts in separate browser profiles, headphones and a
staging deployment. Record the deployed commit, exact browsers, operator and
result below. Use synthetic code and speech; never attach invite links, tokens,
actual transcripts or audio to a public issue.

1. Sign in as A, enter `Lucky7`, and confirm that A is absent from the roster
   until opening availability. Reject non-ASCII/punctuation and numeric-only
   nicknames; distinguish two identical nicknames by their GitHub handles.
2. Open availability with text-only saving. Sign in as B, choose a nickname,
   find A and send a request. Confirm the same preferences on both sides.
   One confirmation must not create a fixed pair or start a clock.
3. After both confirm, enter and connect audio on both profiles. Verify waiting
   clients published no microphone, each side now hears the other, and the
   AI never speaks. Mute/unmute, change microphones and exercise Enable sound
   when autoplay is blocked. Confirm no duplicate track attachments.
4. Confirm ready/start on both sides. Verify the countdown begins only once
   the observer is connected. Repeat against an unavailable observer and
   confirm the clock stays stopped with a clear error.
5. Edit and run tests as candidate. The interviewer watches read-only and sees
   the guide. A third account cannot read state, transcript, audio or advice.
   Close the candidate tab for 30 seconds; reconnect must recover role, timer,
   last snapshot and audio without duplicate tracks. Take the writer lease in
   another tab; the old tab must stop saving and offer its unsaved code.
6. Finish, save final code, exchange feedback and confirm the role swap. Verify
   a different problem of the same difficulty. Complete round 2 and confirm
   both see their own post-session AI suggestions only after completion.
7. Verify text-only exported text but produced no audio artifact. Repeat with
   audio-only, both and neither: check both API access and storage, not merely
   hidden buttons. For neither, verify the transcript table and audio folder
   contain no raw conversation, while suggestions remain available.
8. Stop/delete audio while capturing and confirm no more samples are saved;
   delete saved text and confirm later segments cannot repopulate it. After
   completion, confirm 24-hour expiry blocks reads and cleanup removes files.
9. Opt out of availability and expire a heartbeat; confirm the roster and
   pending requests disappear. Abandon a confirmed availability proposal and
   verify ten-minute cleanup. Terminate an active session and confirm LiveKit
   room deletion.
10. Allow a short round and hard deadline to expire with no polling; confirm
    final capture and settlement continue even while room deletion is stalled.

Any failed step is a failing result. A runnable procedure is not a staging pass.

| Date | Deployment commit | Browsers | Operator | Result | Evidence |
| --- | --- | --- | --- | --- | --- |
| Not run | | | | Pending | Requires staging and two real OAuth accounts |

## Rollout

Start with 8 to 12 users, then 30 to 40, then a 180-user class (90 sessions).
Raise the session cap deliberately after checking connection-minute quota,
transcription throughput, Gemini spend, audio disk usage and cleanup. The
observer's global HTTP concurrency and bounded queues deliberately degrade to
incomplete evidence instead of growing an unlimited backlog.

Validate control p95 below 500 ms, snapshot p95 below 1 s, and reconnect within
5 s at each stage. These are targets, not measured capacity claims. Record user
and session counts, commit, provider quota, sample counts, observation coverage,
latencies and cost before increasing capacity. Automated load validation is a
follow-up after the first stage.
