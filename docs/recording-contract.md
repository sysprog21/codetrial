# Recording contract

## Provisioning status

Not provisioned. Before `CODETRIAL_RECORDING_ENABLED` can be introduced, an
operator records the following values outside the repository and runs
`./scripts/recording-provision-check.sh` with them in its environment:

| Environment variable | Required value |
|---|---|
| `CODETRIAL_RECORDING_GCS_BUCKET` | Private staging bucket name |
| `CODETRIAL_RECORDING_DRIVE_ID` | Platform-owned Shared Drive ID |
| `CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON` | Service-account key JSON, supplied from a secret store |
| `CODETRIAL_RECORDING_TEMPLATE_BASE_URL` | Public HTTPS origin that will serve `/recording/index.html` |

Record the LiveKit Cloud project ID, service-account principal, its exact roles,
the bucket lifecycle-policy date, Shared Drive creation date, and all role-grant
dates in the private operations record. Do not put IDs or key material in this
repository.

Grant the workload service account exactly these data-plane roles, scoped only
to the named resources: `roles/storage.objectAdmin` on the staging bucket and
the `organizer` member role on the dedicated Shared Drive. The former permits
object list/read/write/delete but not bucket administration; the latter is
needed to create reader permissions and delete the delivered artifact. Do not
grant it a project-wide Storage role, Workspace administrator privilege, or any
other Shared Drive membership.

The checker performs only a basic HTTPS-origin check (including no loopback or
bracketed IPv6 host) and does not establish public reachability or fetch the
path before task 8a creates it. The operator records that evidence separately;
task 7a verifies the template URL in a real Egress run. The checker writes the
supplied key and short-lived access-token header only to a private temporary
directory, removes the key from its child-process environment, and does not
replace a developer's active `gcloud` account.

### Naming, without the identifiers

The identifiers stay in the private operations record, but their shape does not,
because a later task has to recognize a wrong one:

| Thing | Shape |
|---|---|
| Google Cloud project | `codetrial-recording-<environment>` |
| Workload service account | `codetrial-recording@<project-id>.iam.gserviceaccount.com` |
| Staging bucket | `<project-id>-recording-staging` |
| LiveKit Cloud project | Its own project, sharing nothing with the interview projects in `config/codetrial.env.*` |

## Fixed paths

These values have exactly one owner in code, `src/recording.rs`, and
`tests/recording_contract.rs` fails if this document stops naming them. Do not
respell them in a later task.

- Webhook route: `POST /api/recording/webhook`
- Template path: `/recording/index.html`
- GCS object path: `{prefix}/{recording_id}.mp4`
- Twirp service: `livekit.Egress`
- Twirp methods: `StartRoomCompositeEgress`, `StopEgress`

`recording_id` is CodeTrial's own identifier, 22 characters of `[A-Za-z0-9_-]`
from `accounts::random_token(16)`. It is not the LiveKit `egress_id` and not the
Drive file id: it is the one value that survives all three and keys the
duplicate search in Drive.

## The provider API

LiveKit Cloud speaks Twirp over HTTPS at the project host, reached the same way
`src/livekit.rs` already reaches `livekit.RoomService`: the configured `wss://`
URL with its scheme swapped for `https://`, then
`/twirp/{service}/{method}`.

```text
POST {https base}/twirp/livekit.Egress/StartRoomCompositeEgress
POST {https base}/twirp/livekit.Egress/StopEgress
Authorization: Bearer {JWT}
Content-Type: application/json
```

The JWT is the same HS256 shape `src/token.rs` already mints, with a video
grant of `{"room": "<room name>", "roomRecord": true}`. Room-scoped: a token
that can record every room in the project is not a credential a per-interview
request needs.

### Two casings, one provider

This is the part that costs a day if it is discovered at integration time.

| Direction | Field naming | Why |
|---|---|---|
| Twirp request | snake_case (`room_name`) | twirp's generated JSON serializer sets protobuf field names |
| Twirp response | snake_case (`egress_id`) | same serializer |
| Webhook body | lowerCamelCase (`egressInfo`, `egressId`) | `webhook/url_notifier.go` calls `protojson.Marshal` directly, whose default does not set `UseProtoNames` |

Requests are forgiving, which is what hides the mistake: protojson accepts
either casing on the way in. Only the reply and the webhook show the
difference, and a field read under the wrong name is absent rather than an
error. `tests/fixtures/recording/` carries both casings as bytes so this cannot
be re-litigated from memory.

Every protobuf `int64` crosses protojson as a JSON *string*. `createdAt`,
`startedAt`, `endedAt`, `duration` and `size` are all quoted. A consumer typing
them as numbers gets null.

### Encoding

`advanced` rather than `preset`, because the product ceiling is 2 Mbps and
`H264_720P_30` is LiveKit's 3 Mbps profile.

| Field | Value |
|---|---|
| `width` / `height` | 1280 / 720 |
| `framerate` | 30 |
| `video_codec` | `H264_MAIN` |
| `video_bitrate` | 2000 (kbps) |
| `audio_codec` | `AAC` |
| `audio_bitrate` | 128 (kbps) |

`AAC` and not the `OPUS` default: an MP4 `EncodedFileOutput` needs it.

The file output writes straight to the staging bucket through
`file_outputs[].gcp`, with `disable_manifest` set. CodeTrial never holds the
media: there is no upload route, and the bytes go from Egress to GCS without
passing through this process.

### The custom template

`custom_base_url` is `{CODETRIAL_RECORDING_TEMPLATE_BASE_URL}/recording/index.html`
and carries no query string of its own. Egress appends `url`, `token` and
`layout`, and the template reads its room credentials from those parameters.
The join token is minted by Egress, not by CodeTrial, so its lifetime is the
provider's to set.

The page is `web/recording/index.html` with `web/recording/recording.js` and
`web/recording/recording.css` beside it, served by the same static fallback as
the rest of `web/`. It is not the candidate's page and it is not signed in.

The candidate is found by elimination: not the interviewer, which is `isAgent`
in `web/lib.js` and nothing restated here; not the recorder, which is a hidden
participant or one whose identity begins `EG_`; and publishing a track whose
source is `Camera`, because a screen share is video too and is not a face. Every
subscription goes through that one selector, so "the candidate" cannot come to
mean "whoever published video first".

An observer cannot be mistaken for the candidate: observer tokens carry
`canPublish: false`, so an observer publishes no camera for the rule to pick.

A track that goes away is detached, which clears its element rather than leaving
a candidate who left recorded as a frozen last frame for the rest of the
interview. The elements the SDK minted go with it; the camera element in the
layout does not, because a rejoining candidate needs something to attach to.

Egress records what the page plays, so every audio track in the room except the
recorder's is attached to an element. The camera element is muted, so the
candidate's voice arrives once rather than twice. Audio that is subscribed and
never attached is audio the file does not have, which reads back as an interview
where the candidate answers questions nobody can hear.

`#recording-ready` carries `data-ready`, `false` until the room is joined and
the candidate's camera is attached, and it moves once. A frame recorded before
that is a frame of an empty layout, and nothing downstream can tell the
difference. At the same moment the page logs
`START_RECORDING` to the console, and it logs `END_RECORDING` when the room
disconnects, so a finished interview does not pay for a black tail until the
provider's timeout. Those two exact strings on the console are the signal:
Egress watches Chrome's console output, there is no injected callback, and a
page that waited for one would never start the recording it was loaded for.

The policy the template loads under names the recording project's LiveKit
origin, both schemes, for the same reason the pool's projects are named: the SDK
opens the signaling socket at `wss://` and then calls the same host over HTTPS.
A blocked socket records a page that never joined a room, which looks exactly
like an interview nobody spoke in.

## Webhook authentication

LiveKit signs the request body, not the request line. The full rule, taken from
`livekit/protocol`'s `webhook/verifier.go`:

1. The `Authorization` header is a bare HS256 JWT. There is no `Bearer` prefix.
2. `iss` is the LiveKit API key. It selects the secret, so it is read before
   anything is verified, and nothing else in the token may be trusted until the
   signature checks out.
3. The signature is HMAC-SHA256 over `base64url(header).base64url(claims)` with
   the project API secret, base64url without padding.
4. `exp` is five minutes after issue. A token with no `exp` is refused.
5. `sha256` is `base64(SHA-256(body))` in **standard** base64, with padding. A
   URL-safe decoder rejects most real digests.
6. `Content-Type` is `application/webhook+json`, which upstream chose so that a
   receiver checks the signature before parsing.

Failure response: `401` with `{"error":"webhook_signature_invalid"}` and no
detail about which step failed. The reason is logged, never returned: telling a
caller apart "wrong secret" from "wrong digest" hands them a probe.

`verify_livekit_webhook` in `src/token.rs` is the only implementation.

There is no separate webhook secret, and there must not be one. LiveKit signs
with the project's own API key and secret, so verification uses
`CODETRIAL_RECORDING_LIVEKIT_API_KEY`/`_SECRET` when the override is set and
the room's provider credentials otherwise. A `CODETRIAL_RECORDING_WEBHOOK_SECRET`
key would be a value an operator believed was checked while nothing checked it.

### Delivery is at-least-once

LiveKit retries any webhook it did not get a 2xx for. A retry is a *fresh,
validly signed message*: new `createdAt`, new signature, same `id`. Dedup keys
on the event `id`. Not on the signature, not on a hash of the body, and not on
`egressId`, which repeats across `egress_started`, `egress_updated` and
`egress_ended` for the same recording.

Events this pipeline reacts to: `egress_started`, `egress_updated`,
`egress_ended`, `room_finished`. Everything else is acknowledged and dropped.

## The lifecycle

Eight states. `RecordingState::may_become` in `src/recording.rs` is the only
implementation of this table, and any transition absent from both is a bug.

| From | May become | Because |
|---|---|---|
| `starting` | `recording`, `finalizing`, `failed` | the row is written before the provider is called, so a process that dies during that call leaves evidence rather than nothing; a stop can arrive before the start is confirmed |
| `recording` | `finalizing`, `transferring`, `failed` | the provider can finish without being asked, at a duration limit or when the room ends, and refusing that transition threw the file away |
| `finalizing` | `transferring`, `failed` | a recording that ended with no file is finished and has nothing to move |
| `transferring` | `ready`, `failed` | |
| `ready` | `deleted`, `cleanup_failed` | |
| `failed` | `deleted`, `cleanup_failed` | a failed recording can still have left bytes in the staging bucket |
| `cleanup_failed` | `deleted` | terminal for the pipeline and an alert for a person |
| `deleted` | nothing | |

Every state may become itself. A duplicate webhook and a stop asked for twice
both request a transition that has already happened, and the answer to that is a
no-op rather than a refusal.

`starting` is written **before** the provider request and the egress id is
written after it. That order is the whole point: the other one loses a recording
that exists whenever the process dies between the call and the write.

Consent is re-read inside the same statement that decides whether to start. A
candidate who withdrew between the token and this moment has a room they may
join and a recording that must not begin, and this is the only place that window
closes.

### Duplicate delivery

`recording_events` is the ledger, keyed on the LiveKit event id, with an
`applied_at` that separates "seen" from "dealt with". An event that was claimed
and not applied comes back on redelivery: deleting the claim on failure was the
first answer and it had a hole, because the delete could itself fail. The ledger
is pruned after seven days, long after LiveKit has given up retrying.

An event naming an egress id this server has not written down yet is answered
`503` rather than acknowledged, because it is the only notice that job will ever
get.

### Stops that did not take

`failed` is terminal, so once a recording is there no state will ever say that
its Egress job is still running. `stopped_at` is what says it: a row with an
egress id, no `stopped_at`, and a state the provider should not be running under
is a job that has not agreed to end, and every sweep asks again until it does.
It is written when a stop succeeds and when an `egress_ended` webhook arrives,
because the job is over either way. An `egress_updated` carrying
`EGRESS_ACTIVE` writes nothing: marking a running job stopped would tell the
sweep to stop watching it.

"Should the provider be running" is not the same question as "does the pipeline
still owe this recording work", and `finalizing` is where they differ. Two cases
made the distinction necessary:

- A candidate withdraws consent, the row moves to `failed`, and the stop request
  fails. Without `stopped_at` the recording keeps running with nothing left to
  notice.
- A stop arrives before the start response does, so the row reaches `finalizing`
  with no egress id to name. When the id lands, the job attached to it has
  already been asked to stop, and the pipeline being "active" is not a reason to
  leave it running.

### Ending

Normal end is `POST /api/interviews/{id}/end` or the LiveKit `room_finished`
webhook, whichever arrives first. Neither is optional: a browser can be closed
and a webhook can be lost. Both move the recording to `finalizing`, and
`finalizing` may become itself, so the second one to arrive changes nothing.

Consent withdrawal is not a normal end. The recording is not finished, it is
abandoned: it goes to `failed` with reason `consent_withdrawn` and the file it
produced is scheduled for deletion. The withdrawal is written down before the
provider is told, because the other order loses the withdrawal when the stop
fails.

### Retries and the sweeper

Three retries after the first attempt: one minute, then five, then fifteen. The
first attempt is not a retry, and the route that makes it does not count one
when the provider refuses; counting it made the first retry five minutes out.
Running out of retries is its own ending, `provider_start_failed`, rather than
waiting for the stale rule: a provider that refused every time is a different
thing from nobody having said anything, and the reason has to say which.

Every retry asks the provider what it already has for the room before starting
anything. A start that succeeded and whose id this side never wrote down leaves
a job nothing knows about, and starting again would make a second: two bills,
two files, and neither stoppable by a candidate withdrawing consent. Not knowing
is not permission to start a second one, so a failed reconciliation spends a
retry rather than starting.

The sweeper runs at startup and every minute after. At startup because a process
that died mid-interview left rows no request will ever touch again; repeatedly
because a schedule checked less often than its own first step is a different
schedule. A row in an active state whose `updated_at` has not moved for its own
bound is stopped and failed with reason `abandoned`.

The bound is fifteen minutes for every state except `recording`, which gets
`CODETRIAL_RECORDING_MAX_MINUTES` plus five. `recording` is the one state with
no periodic signal: LiveKit sends `egress_updated` on a status change and not as
a heartbeat, so a healthy forty-minute interview touches its row once at the
start and not again. Untouched is not unfinished, and a recording that has been
running for forty minutes is a recording. What it cannot legitimately do is
outlive the maximum this server configured.

A webhook proves which project sent it and not which project owns the room it
names, so an event is applied only when the key that signed it is the key
`ProviderPool::for_room` routes that room to. Without that check any configured
project could end another project's recording by naming its room.

`CODETRIAL_RECORDING_KILL_SWITCH` refuses new starts inside the same statement
that would have created them, and the next sweep stops everything already
running with reason `kill_switch` and a structured line on stderr. A switch that
can be raced is not a kill switch.

## Failures, and what a person does about them

Every failure is a code in `recordings.error` and a recovery in
`Failure::recovery`. "Failed" is not an instruction: a candidate whose recording
never produced anything can start another, and a candidate whose file exists and
could not be delivered cannot, because telling them to retry would be telling
them to be recorded twice.

| Code | State | Cause | Recovery |
|---|---|---|---|
| `provider_start_failed` | `failed` | the provider refused every attempt | `start_again`, after retries at 1, 5 and 15 minutes |
| `abandoned` | `failed` | nothing said what happened and the row went stale | `start_again` |
| `partial_output` | `failed` | `EGRESS_COMPLETE` with no file, or a file of no bytes | `start_again` |
| `egress_failed` | `failed` | the provider gave up | `start_again` |
| `egress_aborted` | `failed` | the provider abandoned the job | `start_again` |
| `egress_limit_reached` | `failed` | the project ran out of its allowance | `wait_for_operator` |
| `tombstone_failed` | `cleanup_failed` | the media was deleted and the row could not be updated | `clear_the_row_by_hand` |
| `drive_failed` | `transferring` | the media exists and could not be delivered | `retry_delivery` |
| `consent_withdrawn` | `failed` | consent was taken back | `none` |
| `kill_switch` | `failed` | an operator turned recording off | `wait_for_operator` |
| `cleanup_failed` | `cleanup_failed` | the media outlived the attempt to delete it | `delete_by_hand` |

A completion with nothing in it is not a completion. `EGRESS_COMPLETE` whose
file result is missing, unnamed, or zero bytes is how a truncated recording
arrives, and sending it through the transfer shared a zero-byte video with a
candidate. The size is parsed from a string, because `int64` crosses protojson
that way and a numeric one is a shape this pipeline never receives.

A delivery that loses its row cleans up after itself only when the winning state
is final. The stale sweep moves a `transferring` row to `failed` while keeping
`drive_failed` precisely so the queue can try again, and deleting the staged
object out from under that would leave a recording whose recovery is "retry the
delivery" with nothing left to deliver. Not knowing the winner is not permission
to delete either.

A `transferring` row that goes stale is failed with its own reason kept:
overwriting `drive_failed` with `abandoned` would change the recovery from
"retry the delivery" to "record again", for a recording whose media may still
exist.

`retry_delivery` is therefore a recovery nothing performs yet. The transfer
queue is what claims `transferring` rows and `failed` rows carrying
`drive_failed`, and until it exists the word describes what should happen rather
than what does. `drive_failed` is also unreachable in the shipped path, because
nothing calls `deliver_recording` outside its tests: the pipeline stops at
`transferring` and the sweep fails it as `abandoned`.

A delivery failure is not terminal, and that is the point. The bytes are still
in the staging bucket, so the recording stays `transferring` with `drive_failed`
recorded and `retries` counted. Moving it to `failed` advertised a recovery no
transition could reach.

The staged object path is written to the row before anything remote happens.
It is derivable from the recording id, but the deletion path reads it from the
row, and a delivery that ended anywhere except success used to leave that column
empty: the bytes stayed in the bucket with nothing naming them. A delivery that
cannot write it down makes no remote call at all.

A retry reuses the `drive_file_id` the last attempt recorded rather than
uploading again. It cannot make the same promise about a permission: a crash
between Drive accepting the share and the write that records it leaves one
nothing names. Deleting the file removes its permissions, so retention still
reaches it, and the transfer queue's claim is what closes the window.

Each Drive handle is written on its own and only while the row is still
`transferring`. A withdrawal that wins the race leaves a file this delivery
created and nothing naming it, so that file is deleted rather than left in the
Shared Drive.

`revoke_and_delete` is one call covering three remote operations and records no
progress between them, so a revoke that succeeded and a delete that failed
cannot be resumed step by step. The transfer and retention tasks own that;
until then a retry repeats all three, which is why each has to be idempotent.

### Status

`GET /api/interviews/{id}/recording` answers the owner with the recording id,
the state, the error code, the recovery, and the retry count. `404` for an
interview with no recording, for somebody else's, and for a server that records
nothing; those are one answer because telling them apart enumerates other
people's interviews.

A server that records nothing answers a status request exactly as one with no
such recording does, which is exactly how it answers somebody else's. Three
states, one reply: telling them apart is a way to learn about other people's
interviews and about this deployment.

It returns no Drive file id and no permission id: those are handles to media,
and a status route that returned them would be a way to reach a recording
without the permission that governs it. The `recordingId` it does return derives
the staged object name, which is not a capability, because the bucket is private
and reaching it needs the service account.

The error code goes through `Failure::parse` on the way out, so only an
enumerated value can reach a client. The column is written by this crate today,
and a route that echoed whatever was in it is one refactor away from handing
back a provider's error body.

### Audit

One structured line on stderr per event, through `recording::audit`. JSON
because it is read by whatever collects logs rather than by a person scrolling.
Every value is a string, counts included, so the schema has one shape, and every
value is bounded.

Nothing here writes a candidate's name, address, or room. A deletion record that
named an address would outlive the deletion it recorded, so `recording_deleted`
carries an id and who asked. The one field this cannot promise about is `error`,
which is a provider's own message on a failure line: it is bounded and it is not
parsed, and these lines belong wherever the provider's own logs belong.

| Event | Fields beyond `recording_id` |
|---|---|
| `recording_failed` | `reason`, `recovery` |
| `recording_delivery_failed` | `step`, `error`, `reason`, `recovery` |
| `recording_delivery_lost` | `step` |
| `recording_ready` | |
| `recording_deleted` | `by` |
| `recording_cleanup_failed` | `step`, `error`, `reason`, `action`, and `media` when the bytes were already gone |
| `recording_killed` | `reason`, `recovery` |
| `recording_abandoned` | `state`, `reason`, `recovery`, `age_seconds` |
| `recording_still_running` | `error`, `action` |
| `recording_stop_refused` | `error`, and `step` where a route asked |
| `recording_stop_not_recorded` | `error`; the transition itself failed, so no provider call was made |

Every line that reports a state change is written after that change and only by
the caller that made it. Two sweepers can read the same row, and one of them
moved nothing.

`recording_cleanup_failed` is the one an operator has to act on: it names
`action: delete_by_hand`, because nothing automatic is going to fix a file that
would not delete.

### Where the pipeline currently stops

`transferring` has no worker yet. A recording whose `egress_ended` said
`EGRESS_COMPLETE` reaches that state and stays there, holding the account's one
active slot, until the sweeper fails it as `abandoned` fifteen minutes later.
The transfer step is the next task; until it lands, recording produces a staged
GCS object and no delivery. That is why the switch stays off.

## The replay

A recording is an MP4 and a transcript; a replay is what the candidate was
looking at while it happened. The two are delivered together and stored apart,
because one is media a provider owns and the other is small enough to keep and
cheap enough to serve.

One envelope, version 1:

```json
{ "v": 1, "kind": "transcript", "at": 1770000000000, "payload": { } }
```

`at` is the browser's clock in milliseconds. It is what the replay is played
back against, and it is never what orders anything: `seq`, allocated by the
server, is the ordering.

| `kind` | Producer | Replaces the last one |
|---|---|---|
| `transcript` | what was said | no |
| `editor` | the code and its language | yes |
| `tests` | a run's results | no |
| `stage` | the clock and the interview phase | yes |
| `avatar` | what Jim is doing | yes |
| `lifecycle` | what the recording is doing | yes |

"Replaces the last one" is what makes a late join one snapshot plus the events
after it, rather than every keystroke since the interview began.

### Limits

| Limit | Value | Why |
|---|---|---|
| One payload | 64 KiB | a full screen of code is a few KiB; this is room for a pathological one and a refusal for anything that is not an event |
| One string inside a payload | 16 KiB | a shape limit, not a size one: anything arriving as one enormous string is not what the producer is for |
| Events per interview | 5000 | one every half second for forty minutes |
| Bytes per interview | 8 MiB | more replay than an interview produces |

Every byte limit above is UTF-8 bytes. The payload limit measures the serialized
payload; the string limit measures the value itself, before JSON escaping.
Sixteen thousand four-byte characters are the entire event budget spent on one
field, so counting characters was counting the wrong thing, and escaping can
still double a string of quotes on the way into JSON, which is what the payload
limit is for.

An event is processed in three steps, and the order is the rule:

1. Overlong strings are cut, on a character boundary, and media values are
   replaced. Both are transformations of a value the producer is allowed to
   send, so a code snapshot at the limit becomes a prefix and the event is kept.
   That alters the value, which is worth saying plainly.
2. The payload is measured. This is the size of what a producer may send.
3. Secret keys are stripped and the payload is measured again. Removal is last
   and does not buy room: a megabyte arriving under a key that happens to be
   redacted is a megabyte, and nothing unredacted is ever retained.

The two per-interview limits are whichever comes first, and both exist so that a
stuck producer costs a bounded amount rather than the disk. Over either,
`POST /api/interviews/{id}/events` answers `413 replay_quota_exceeded` and sets
`recordings.quota_exceeded`; the recording itself is not failed, because a
replay that stopped growing is still a recording worth keeping. An interview
whose recording never started has no row to flag, and the refusal stands
anyway: the producer is told the ceiling was reached, and there is no replay to
review without a recording to review it beside.

### Ingest

`POST /api/interviews/{id}/events`, signed in, with `{"events": [...]}`.

| Answer | When |
|---|---|
| `200 {"firstSeq", "lastSeq"}` | stored, and those are the numbers they were given |
| `400 replay_envelope_invalid` | not the envelope, not this version of it, or a batch with nothing in it |
| `400 replay_kind_unknown` | a kind nothing renders |
| `413 replay_event_too_large` | one payload over 64 KiB |
| `413 replay_batch_too_large` | over 32 events, or over 256 KiB of body |
| `413 replay_quota_exceeded` | the interview is at one of its two ceilings |
| `404 recording_not_found` | no such interview, not this account's, or consent withdrawn |
| `404 recording_not_enabled` | this server records nothing |

A batch is all of its events or none. A batch that stored its first half and
refused its second would be a replay with a hole in it, and the producer has no
way to learn which half survived, so validation of every event happens before
the first insert and the inserts share one `IMMEDIATE` transaction.

Withdrawn consent closes ingest for good, and closes it in the same shape as an
id that was never this account's. A browser that buffered events before the
withdrawal will try to flush them afterwards, and consent that stops the video
while the replay keeps growing is not withdrawal. What is already stored is the
deletion path's to remove, not ingest's.

Cross-account is `404` rather than `403`, the same as everywhere else an
interview id arrives from a browser: an account that does not own an interview
should not learn that it exists.

Sequences are allocated inside the insert itself:

```sql
INSERT INTO replay_events (...)
SELECT ?1, COALESCE((SELECT MAX(seq) FROM replay_events WHERE interview_id = ?1), -1) + 1, ...
RETURNING seq
```

A `SELECT MAX(seq)` followed by an `INSERT` has a gap two writers land in, and
reading `MAX(seq)` back after the insert can return the other writer's row
rather than this one's. `RETURNING` is what makes the number the server
allocated the number the caller is told.

### The snapshot

`GET /api/interviews/{id}/snapshot`, signed in, answering
`{"seq", "quotaExceeded", "events": [...]}`.

Nothing is stored to produce it and there is no cadence to configure. The four
replaceable kinds already are the snapshot: the newest event of each is the
whole state of that kind, so a snapshot is the replay with the superseded frames
dropped, computed at the read. A periodic snapshot written to a second table
would be a copy that can disagree with the events it was made from.

A late join is therefore the snapshot plus everything after `seq`, concatenated.
The ceiling and the rows under it are read in one transaction, so a writer
landing between two statements cannot put an event in the snapshot that the
caller is about to ask for again.

| Answer | When |
|---|---|
| `200` | the snapshot, and the `seq` to carry on from |
| `404 recording_not_found` | no such interview, not this account's, consent withdrawn, or this server records nothing |
| `410 replay_expired` | past the retention deadline, whether or not the sweeper has run |
| `410 recording_deleted` | the recording is a tombstone |

`410` rather than `404` for the last two: the account owns the interview and is
owed the difference between "never yours" and "not any more".

Authorization is the account that owns the interview, and nothing else. There is
no separate verified-identity gate here, because a replay is scoped by account
row rather than by handle: a self-declared account is its own row and sees its
own interviews. The verified-identity gate lives where identity decides who
receives a file, which is the start of a recording, not the read of a replay.

An interview whose recording never started still has a replay and it is readable.
Expiry and deletion are read from the recording when there is one.

### The template's replay read

The recording template holds no session cookie. It holds the join token Egress
minted for it, which is signed with the LiveKit project's own secret, and that
secret is one this server has. So the template authorizes with the token:

    GET /api/recording/replay[?after=<seq>]
    Authorization: <the join token, bare, no Bearer prefix>

The token is verified rather than parsed: signature against the project's
secret, `exp` and `nbf`, `roomJoin` true, and the room it names. The key that
signed it has to belong to the project that owns that room, the same rule the
webhook route applies. The room then names the recording, which names the
interview, which is whose replay is returned.

The rule this enforces, stated plainly, is that a participant of a room may read
that room's replay. It is not narrower than that, and the ceiling is worth
writing down: a candidate's own join token and an observer token would pass it
too. Today that grants nothing, because every join token for an interview room
belongs either to the account that owns the interview or to this server's own
agent, and the observer route only issues one to the interview's owner. A
credential handed to anyone else, a reviewer's viewing token among them, would
have to narrow this check first. Task 7a's credentialed run is where to learn
whether the Egress token carries a distinguishing grant, `hidden` or `recorder`,
that this check could require.

`after` chooses the shape. Absent or negative is the snapshot; a sequence number
is everything after it, superseded frames included, because a reader carrying on
from a snapshot is replaying and a frame it never saw is not one to skip. Both
answer with the same body, so the template's loop parses one thing.

The template awaits its first read before starting the loop, which is what makes
"one complete snapshot, then ordered events" true rather than hoped for, and
then schedules each next read 500 ms after the last one landed rather than on an
interval, so a slow answer cannot stack up behind a request still out.

A refused or expired replay does not stop the recording: the camera and the
interviewer's voice are still worth the file. It does stop the polling, on
`401`, `403`, `404` and `410`, because an expired replay does not un-expire and
a credential this route refused stays refused for the life of the job.

Each read is abandoned after four seconds. A request that hangs answers neither
way, and the first read is what the recording waits on, so one stalled
connection would otherwise hold the interview at an empty layout for good.

A `500`, a dropped connection or an abandoned read is a bad moment rather than
an answer. It is
retried, and it does not count as a snapshot: a recording that started on the
first dropped request would open with the empty layout the snapshot exists to
prevent. Ten seconds after the page opened it records anyway, because a replay
that never answers must not hold the interview either. Ten seconds on the clock
rather than a count of attempts: with a four second abort behind each one, a
count would be a minute and a half of an interview nobody is recording.

The Jim panel carries fallback markup rather than an empty mount. The VRM model
is not published in this repo, so without it every recording would show a blank
box where the interviewer should be.

### What the producers send

The payload shapes the template renders, and therefore what the producers in
`web/interview.js` have to emit. Unknown kinds are ignored rather than refused,
so a producer from a later deploy does not stop a recording.

| Kind | Payload | Rendered as |
|---|---|---|
| `stage` | `{title, meta, remainingSeconds}` | the problem heading and the clock |
| `editor` | `{code, language}` | the code panel, as text |
| `tests` | `{passed, failed, total}` | one line, red if anything failed |
| `avatar` | `{state}`, one of `speaking`, `thinking`, `listening` | Jim's expression and label |
| `transcript` | `{speaker, text}` | nothing here; the replay page renders it |
| `lifecycle` | `{state}` | nothing here |

The producers live in `web/interview.js`, one call site per kind, all of them
through `recordReplay`: one answer to "is this server recording" and one place
the replay stops when the server says it has heard enough. Events are batched to
the server's own limit of thirty two and flushed every second, and a batch that
fails is dropped rather than retried, because a queue that grew through an
outage would deliver a burst of stale state on top of the newer state that had
already arrived. A `413` or a `404` stops the producers for the rest of the
interview: over quota and withdrawn consent both mean everything after this is
refused.

Cadence is where the per-interview budget goes. The editor rides the debounce
the agent's `code_update` already uses; the transcript is one event per spoken
turn rather than per chunk; the clock is restated every fifteen seconds, because
every second would be twenty-seven hundred events for a number the viewer can
read off the video; and the interviewer's state is sent on the change rather
than on the participant event that happened to carry it.

`CODETRIAL_REPLAY_VERSION` travels through `/runtime-config.js` for the same
reason the consent version does: a second spelling of it in the page would have
every event refused.

The code panel is written with `textContent`. This is the candidate's own code
rendered into a page a recorder screenshots sixty times a second, and markup in
it would be markup in the recording.

Jim is rendered locally rather than subscribed: the interviewer publishes audio
and no video, so the face in the file is the template's own VRM render, its
mouth driven by the amplitude of that audio and its expression by the replay's
`avatar` events.

### Redaction

Removed, not refused. A producer that accidentally carried a token should still
deliver the transcript line beside it, and refusing the whole event would lose
the interview in order to protect it.

- Any key containing `token`, `secret`, `password`, `credential`,
  `authorization`, `apikey`, `privatekey`, `bearer`, `jwt` or `cookie`, at any
  depth. Matched on a lowercased key with `-` and `_` removed, because the
  browser writes `apiKey`, `x-api-key` and `private_key` and a list of exact
  names would let every one of them through.

  `session` is deliberately not on that list: it would take `sessionId` and
  `sessionName` with it, and the session cookie is `HttpOnly` and unreachable
  from any producer.
- Any string that begins, after leading whitespace, with `data:` or `blob:`,
  replaced with `[media removed]`. Whitespace is skipped because a browser that
  wrote `" data:image/png..."` wrote a data URL.
  This is the one thing a replay must never carry: the video is the provider's,
  delivered under a permission that expires, and a frame smuggled into an event
  outlives it.

This is a key check and a media check, and it does not read values. A bearer
token in the middle of a transcript line, or a signed URL inside a test result,
survives it. The producers are first-party and none of them handle credentials,
and a value-scanning rule would cost false positives on ordinary code and prose
for a case none of them can reach.

Redaction runs where the value stops being the candidate's and starts being this
server's, before anything is stored. A payload that reaches storage unredacted
is one nothing later can un-store.

### `replay_events`

| Column | Type | Null | Meaning |
|---|---|---|---|
| `interview_id` | TEXT | no | `interviews(id)`, `ON DELETE CASCADE` |
| `seq` | INTEGER | no | server-allocated, monotonic per interview |
| `kind` | TEXT | no | one of the six above |
| `at` | INTEGER | no | the browser's clock |
| `payload` | TEXT | no | redacted JSON |
| `bytes` | INTEGER | no | the payload's size, so the quota is a sum rather than a scan |
| `received_at` | INTEGER | no | this server's clock |

`(interview_id, seq)` is the primary key, so no two events can claim one
position. It does not rule out a gap: only the server allocating the number does
that, and the key is what makes the allocation's answer durable.

`CHECK`s carry the rest of the shape: `seq`, `at` and `bytes` are non-negative
and `kind` is one of the six. A row that fails one has no meaning for this
table, whoever wrote it.

`ON DELETE CASCADE`, unlike `recordings`. Nothing here is a handle to media
somewhere else, so losing these rows loses only what they say.

## The delivery queue

Between a recording that has a file and a candidate who can read it. One row per
recording that still owes a delivery, and none for one that does not: success
deletes the row and giving up deletes it too, because `recordings.state` and
`recordings.error` are where an outcome lives and a queue that kept its own
history would be a second answer that can disagree.

| Column | Type | Null | Meaning |
|---|---|---|---|
| `recording_id` | TEXT | no | primary key, `recordings(id)` `ON DELETE CASCADE` |
| `attempts` | INTEGER | no | incremented by the claim, not by the outcome |
| `run_after` | INTEGER | no | when this row is next due |
| `claimed_at` | INTEGER | yes | null while nobody holds it |
| `claim` | TEXT | yes | the token that claim was taken under |
| `error` | TEXT | yes | the last failure, for an operator reading the queue |
| `created_at`, `updated_at` | INTEGER | no | epoch seconds |

The claim is the whole concurrency story. A worker claims a row before it
touches Drive, so two workers cannot both see no `drive_file_id`, both upload,
and leave the loser's file in the Shared Drive with nothing naming it and
nothing able to delete it. The claim and the attempt count move in the same
statement as the selection, inside an `IMMEDIATE` transaction.

A recording somebody is actively delivering is not stale, whatever its
`updated_at` says: the sweeper skips rows with a live claim. An upload runs for
up to half an hour and touches nothing while it does, so without that the
sweeper fails it mid-transfer and the worker then deletes the file it had just
uploaded. The kill switch still takes it, because a kill switch that waited for
an upload is not one.

A claim older than an hour is reclaimable, because the worker holding it may
have died with the process. An hour rather than a few minutes: a claim that
expires under a running upload is how two workers end up uploading at once,
which is the thing the claim exists to prevent, and a forty-five minute
interview is a few hundred megabytes that finish inside it on any link worth
recording over.

Every later write names the claim it was made under, and a write that finds the
row is no longer its own does nothing rather than treating it as a failure. A
worker whose claim expired mid-upload would otherwise delete or reschedule the
row its replacement is holding, or fail a recording somebody else is halfway
through delivering and take the file with it.

The transition to `transferring` and the queue insert are two writes, so a
process that dies between them leaves a recording that owes a delivery and a
queue that has never heard of it. The sweeper repairs that: any `transferring`
recording the queue does not know about is queued, before the same sweep reads
its stale rows rather than after, because a restart longer than the stale window
leaves exactly such a row and the other order fails the recording it had just
queued a delivery for. That is cheaper than a wider
transaction across two modules, and it is the sweep that already exists to find
work nothing is moving.

Three attempts, at zero, one minute and five. The first is the delivery itself.
After the third the recording is `failed` with `drive_failed`, in that order and
then the queue row is deleted: the other order leaves a window where the
recording is still `transferring` with an empty queue, which is exactly what the
sweeper's repair looks for, and the retry an operator is supposed to authorize
would happen by itself. The row is gone because a queue row nobody will attempt
again is work that never happens and the sweeper would eventually abandon the
recording with a less useful reason.

The remaining window is the other way round and is bounded: a process that dies
after the failure and before the delete leaves a queue row against a
`drive_failed` recording, and the next claim reopens it. That is one more
attempt than the policy says and no other difference, because the attempt reuses
the file the earlier one uploaded.

A `308` is read for the bytes the session says it holds, and the header has to
be exactly `bytes=0-n` within the object's size. Anything else is a header this
code cannot act on, and guessing at it moves the offset past bytes that were
never stored, which finishes a file with a hole in it.

`retry_delivery`, the recovery `drive_failed` names, is an operator inserting a
queue row by hand. The worker reopens a `drive_failed` recording when it claims
one, guarded in the statement rather than in the transition table: "failed for
this one reason" is not a state the table has, and giving it one would make every
other failure re-openable too. The bytes never left the staging bucket, which is
why this is a delivery to retry rather than an interview to record again.

Enqueueing happens where a recording reaches `transferring`, and only when that
transition moved the row. A webhook LiveKit sent twice must not become two
deliveries; the queue's `ON CONFLICT DO NOTHING` is the second half of that
rather than the first.

## Google Drive delivery

All calls carry `supportsAllDrives=true`. Without it the API pretends a Shared
Drive file does not exist, which surfaces as a 404 on a file that was just
created.

Resumable upload, session initiation:

```text
POST https://www.googleapis.com/upload/drive/v3/files?uploadType=resumable&supportsAllDrives=true
{
  "name": "<filename>",
  "parents": ["<shared drive id>"],
  "mimeType": "video/mp4",
  "appProperties": {"codetrial_recording_id": "<recording_id>"}
}
```

The session URI comes back in the `Location` header, and the bytes are `PUT` to
it with `Content-Range`. `appProperties.codetrial_recording_id` is the duplicate
search key, so a transfer resumed after a restart finds its own half-finished
file instead of creating a second one:

```text
GET https://www.googleapis.com/drive/v3/files
  ?q=appProperties has { key='codetrial_recording_id' and value='<recording_id>' }
  &corpora=drive&driveId=<shared drive id>
  &includeItemsFromAllDrives=true&supportsAllDrives=true
```

Chunks are `UPLOAD_CHUNK_BYTES`, eight megabytes, which is a multiple of the
256 KiB the protocol requires and small enough that one chunk is the memory a
delivery costs. A retryable answer, `429`, `500`, `502`, `503` or `504`, and a dropped
connection are both handled by asking the session what it actually holds and
carrying on from there, out of a budget of eight for the whole upload. A budget
for the transfer rather than a count per chunk: per chunk is the shape that
looks right and is unbounded, because a session that keeps reporting the same
offset hands back a chunk to re-send and a counter that starts again with each
one never runs out. Everything else is an answer rather than a hiccup, and
attempts beyond the budget belong to the delivery queue, because a retry a
worker cannot see is a retry nobody can bound.

One transfer runs for at most half an hour, which is bounded below the queue's
claim window on purpose: a transfer that outlived its claim would still be
running while another worker started a second one, and two resumable sessions
against one recording is exactly the duplicate the claim exists to prevent.

Each range has to come back `206` with the length it asked for, checked before
the body is read and again after. A proxy that ignores `Range` answers `200`
with the whole object, and buffering that is however many gigabytes the
recording is rather than the one chunk this delivery is supposed to cost; a
chunked answer with no length is refused for the same reason.

A `401` clears the cached token. It is the one answer that says the credential
is wrong whatever the local clock thinks, and without clearing it every attempt
in the queue's schedule would reuse the same rejected token.

The permission's expiry is read off the clock at the share rather than when the
delivery was queued. Twenty-four hours means twenty-four hours of readable file,
and an upload that took forty minutes would otherwise hand the candidate
twenty-three hours and twenty minutes of it.

A resumable session that is never finished creates no file. That is why an
attempt that dies mid-upload leaves nothing for the duplicate search to find and
nothing in the Drive for retention to chase.

Reader permission, with expiry:

```text
POST https://www.googleapis.com/drive/v3/files/{fileId}/permissions
  ?supportsAllDrives=true&sendNotificationEmail=false
{
  "role": "reader",
  "type": "user",
  "emailAddress": "<verified GitHub email>",
  "expirationTime": "<RFC 3339, 24 hours out>"
}
```

`expirationTime` is accepted on this permission because it is a `user`
permission with the `reader` role on a file, less than a year out, which is the
whole restriction list Drive documents. It is defence in depth rather than the
retention mechanism: the deletion at expiry is what actually removes the media,
and the permission expiring is what covers the gap if a deletion is late.

A cleanup that deletes a file this delivery uploaded also clears the row's
`drive_file_id` and `drive_permission_id`, when the whole cleanup succeeded. A
cleanup whose file deletion worked and whose staging deletion did not leaves the
row naming a file that is gone; that row is one whose recording has already
moved on, which is the only reason this cleanup runs, so nothing will read the
handle again. A handle naming a file that is gone
is worse than no handle: a later attempt reads it, skips the upload, and tries
to share something that is not there.

A permission is always named with the file it is on. Drive cannot revoke one
without that, and the file a delivery is revoking on is not always a file it may
delete: an attempt that reused an earlier upload and then granted its own
permission has to take back the grant and leave the file.

Revocation and deletion, in that order:

```text
DELETE https://www.googleapis.com/drive/v3/files/{fileId}/permissions/{permissionId}?supportsAllDrives=true
DELETE https://www.googleapis.com/drive/v3/files/{fileId}?supportsAllDrives=true
```

## The database

One database, the one at `CODETRIAL_DB_PATH`, default `codetrial.db`.
`interviewlab.db` at the repo root is a pre-rename leftover and is not migrated.

Schema changes are appended to `ACCOUNT_MIGRATIONS` in `src/accounts.rs` and
`ACCOUNT_SCHEMA_VERSION` is bumped. `migrate` reads `PRAGMA user_version`, runs
only the entries after it inside one `IMMEDIATE` transaction, and stamps the new
version, so a migration runs once or not at all. There is no down migration and
no rollback test, because there is nothing to roll back to.

A database stamped with a version above the one the binary knows is refused
rather than opened. Running today's queries against tomorrow's tables is the
failure that has no symptom until it has a bad one.

### `recordings`

| Column | Type | Null | Meaning |
|---|---|---|---|
| `id` | TEXT | no | `recording_id`, primary key, 22 chars from `random_token(16)` |
| `account_id` | INTEGER | no | with `interview_id`, a composite key into `interviews(account_id, id)`, `ON DELETE RESTRICT` |
| `interview_id` | TEXT | no | UNIQUE; one interview is one recording |
| `room_name` | TEXT | yes | the LiveKit room this recorded; cleared when tombstoned |
| `idempotency_key` | TEXT | no | UNIQUE; a retried start must not become a second Egress job |
| `egress_id` | TEXT | yes | null until the provider answers; unique among non-null values |
| `gcs_object` | TEXT | yes | `{prefix}/{recording_id}.mp4`, once staged |
| `drive_file_id` | TEXT | yes | set by the transfer |
| `drive_permission_id` | TEXT | yes | set when the reader permission is granted |
| `recipient_email` | TEXT | yes | the verified address the file was shared with, copied at start; cleared when tombstoned |
| `state` | TEXT | no | the lifecycle state; the transition table is task 4's |
| `error` | TEXT | yes | a short machine code, never a provider message |
| `retries` | INTEGER | no | default 0 |
| `duration_seconds` | INTEGER | yes | from the provider, once the recording ends |
| `created_at`, `updated_at` | INTEGER | no | epoch seconds |
| `started_at`, `ended_at`, `ready_at` | INTEGER | yes | each null until it happens |
| `expires_at` | INTEGER | yes | the retention deadline |
| `stopped_at` | INTEGER | yes | when the provider agreed to stop; an egress id with no `stopped_at` is a job still running |
| `deleted_at` | INTEGER | yes | when the media was deleted |
| `delete_error` | TEXT | yes | a short machine code for a partial failure |
| `quota_exceeded` | INTEGER | no | default 0; set when the replay hit a ceiling and stopped growing |
| `deleted_by` | TEXT | yes | `expiry`, `consent_withdrawn`, or `operator` |

Two `CHECK`s state one rule in both directions:

```sql
CHECK (deleted_at IS NOT NULL
       OR (room_name IS NOT NULL AND recipient_email IS NOT NULL))
CHECK (deleted_at IS NULL
       OR (room_name IS NULL AND recipient_email IS NULL
           AND gcs_object IS NULL AND drive_file_id IS NULL
           AND drive_permission_id IS NULL))
```

A recording that exists must know which room it recorded and who it is for. A
recording that has been deleted must have given up both, and every locator for
bytes that are gone. `NOT NULL` states the first half and cannot state the
second, and a tombstone that quietly kept a candidate's address would be a
deletion that deleted the wrong thing. `egress_id` and the tombstone fields
stay: they are what a later audit is answered from.

The foreign key is composite rather than two independent references. Two
references can disagree: each would be satisfied, and the row would attach one
account's recording to another account's interview. There is no separate
reference to `users`, because it would add nothing; an account cannot be deleted
without cascading through `interviews`, and that is where the refusal happens.
`interviews` therefore carries a unique index on `(account_id, id)`, which is
what a composite foreign key needs on the parent side.

Indexes: `recordings_by_account`, a partial unique index
`recordings_by_egress` over non-null `egress_id`, and a partial
`recordings_by_expiry` over rows that have a deadline and are not yet deleted.

The foreign key is `RESTRICT`, not `CASCADE`. Nothing here holds media; what it
holds is the handles, and deleting the row that says a file exists is how the
file becomes unreachable and undeletable. Deleting an account cascades into
`interviews`, that cascade reaches this `RESTRICT`, and the whole delete fails.

Removing the row stays an explicit step: `RESTRICT` does not soften once
`deleted_at` is set, so whatever deletes accounts has to delete their recordings
first. `sweep_expired_sessions` already excludes accounts that own one, in one
transaction with its session delete, because otherwise it would fail the
statement rather than skip the row, after the sessions had already gone.

`recipient_email` is a copy rather than a lookup: the account's verified address
can change afterwards, and the permission that was granted did not.

`id` is declared `TEXT PRIMARY KEY NOT NULL`, saying the same thing twice on
purpose. SQLite permits NULL in any primary key that is not an INTEGER rowid
alias, and permits several of them, so `PRIMARY KEY` alone is not the constraint
it reads as. `interviews.id` and `reports.id` carry the same hole and keep it:
nothing inserts a NULL id, and rebuilding a table another table's foreign key
already points at costs more than a hole no code path reaches.

### `interviews`

| Column | Type | Null | Meaning |
|---|---|---|---|
| `id` | TEXT | no | primary key |
| `account_id` | INTEGER | no | `users(id)`, `ON DELETE CASCADE` |
| `consent_version` | TEXT | no | the wording that was shown |
| `consent_at` | INTEGER | no | when it was agreed to |
| `consent_withdrawn_at` | INTEGER | yes | set once, by `DELETE /api/interviews/{id}/consent` |
| `room_name` | TEXT | yes | claimed once by `/api/token`; one consent is one room |

A partial unique index on `(account_id, consent_version)` over rows with no room
and no withdrawal keeps one pending interview per account and wording, so a
reload reuses it rather than inserting another.

## Fixtures

`tests/fixtures/recording/` is generated by
`scripts/gen-recording-fixtures.mjs`, and `scripts/test.sh` runs it with
`--check`. Do not hand-edit: the webhook bodies are compact and newline-free
because the signature covers exactly the bytes on disk, and an editor that adds
a trailing newline invalidates every signature in `webhook-cases.json`.

`webhook-cases.json` carries one case per distinguishable rejection, including
the one a signature alone cannot catch: a body swapped underneath a valid
signature.

## Cost

Not yet checked. Before `CODETRIAL_RECORDING_ENABLED` is turned on in
production, record here the date checked, the pricing page used, the
per-interview estimate for LiveKit Cloud transcoding/egress plus GCS plus
Drive, and who approved it. LiveKit Cloud's included 60 transcode-minutes are a
test budget, not a production allowance.
