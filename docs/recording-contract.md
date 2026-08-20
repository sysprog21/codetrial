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
  ?q=appProperties has {key='codetrial_recording_id' and value='<recording_id>'}
  &corpora=drive&driveId=<shared drive id>
  &includeItemsFromAllDrives=true&supportsAllDrives=true
```

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

Revocation and deletion, in that order:

```text
DELETE https://www.googleapis.com/drive/v3/files/{fileId}/permissions/{permissionId}?supportsAllDrives=true
DELETE https://www.googleapis.com/drive/v3/files/{fileId}?supportsAllDrives=true
```

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
