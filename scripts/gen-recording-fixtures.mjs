#!/usr/bin/env node
// Generates the LiveKit provider-contract fixtures in tests/fixtures/recording/.
//
// These pin the other boundary in this repo: not browser-to-Rust, which
// scripts/gen-wire-fixtures.mjs covers, but LiveKit-to-Rust. Nothing here is a
// producer we own, so the fixtures are transcriptions of the provider's own
// wire format, taken from livekit/protocol rather than from a guess:
//
//   - Twirp request and response JSON uses protobuf field names (snake_case),
//     because twirp's generated JSON serializer sets OrigName.
//   - The webhook body uses lowerCamelCase, because livekit/protocol's
//     webhook/url_notifier.go marshals it with protojson.Marshal and its
//     default options, which do not set UseProtoNames.
//
// One provider, two casings, and no error from either side when you send the
// wrong one: the request is accepted (protojson unmarshal takes both) and the
// webhook field you read is simply absent. That asymmetry is the reason these
// fixtures exist as bytes rather than as prose in the contract.
//
// The webhook signature is over the exact request body bytes, so the body
// files are written compact and without a trailing newline: what is on disk is
// what LiveKit would have put on the wire, and re-serializing it in a test
// would sign different bytes than the fixture claims.
//
// Deterministic, like the wire generator: fixed ids and a fixed clock, so
// `--check` can tell a stale fixture from a fresh one.

import { createHash, createHmac } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const FIXTURES = join(ROOT, "tests", "fixtures", "recording");

// Test credentials, and nothing else. A LiveKit API key is a public
// identifier; the secret here is a fixture string that has never been a real
// credential, and the contract test needs both to be checked in or it cannot
// verify a signature at all.
const API_KEY = "APIfixturekey";
const API_SECRET = "fixture-secret-not-a-real-livekit-credential";

// Fixed clock. The webhook JWT carries `exp`, so a test verifying these
// fixtures has to be told the epoch they were minted at rather than reading
// the real one.
const ISSUED_AT = 1_770_000_000;
const WEBHOOK_TTL_SECONDS = 300;

const ROOM_NAME = "interview-3f9a2c17";
const EGRESS_ID = "EG_fixtureEgress01";
const ROOM_ID = "RM_fixtureRoom0001";
const RECORDING_ID = "Rf2Kq7XmZ0pB4nT8sVdLwA";
const GCS_PREFIX = "codetrial";
const FILEPATH = `${GCS_PREFIX}/${RECORDING_ID}.mp4`;
const TEMPLATE_BASE_URL = "https://recording.codetrial.example";

const base64url = (value) => Buffer.from(value).toString("base64url");

// The same shape src/token.rs signs: base64url(header).base64url(claims),
// HMAC-SHA256, base64url signature, no padding anywhere.
function jwt(secret, claims) {
  const signingInput = `${base64url(JSON.stringify({ alg: "HS256", typ: "JWT" }))}.${base64url(
    JSON.stringify(claims),
  )}`;
  const signature = createHmac("sha256", secret).update(signingInput).digest("base64url");
  return `${signingInput}.${signature}`;
}

// Standard base64 with padding, not base64url: webhook/verifier.go compares
// against base64.StdEncoding, and a URL-safe digest would fail every message
// whose hash happens to contain a 62nd or 63rd character.
const bodyDigest = (body) => createHash("sha256").update(body).digest("base64");

function webhookAuthorization(secret, body, { issuedAt = ISSUED_AT, key = API_KEY } = {}) {
  return jwt(secret, {
    iss: key,
    nbf: issuedAt,
    exp: issuedAt + WEBHOOK_TTL_SECONDS,
    sha256: bodyDigest(body),
  });
}

// int64 protobuf fields cross protojson as JSON strings, which is why every
// timestamp below is quoted. A consumer that reads `createdAt` as a number
// gets null from serde and a recording that never finalizes.
const egressInfo = (status, extra) => ({
  egressId: EGRESS_ID,
  roomId: ROOM_ID,
  roomName: ROOM_NAME,
  status,
  startedAt: "1770000000000000000",
  ...extra,
});

const endedEvent = (id, createdAt) => ({
  event: "egress_ended",
  id,
  createdAt,
  egressInfo: egressInfo("EGRESS_COMPLETE", {
    endedAt: "1770000123000000000",
    fileResults: [
      {
        filename: FILEPATH,
        startedAt: "1770000000000000000",
        endedAt: "1770000123000000000",
        duration: "123000000000",
        size: "30750000",
        location: `https://storage.googleapis.com/codetrial-recording-staging/${FILEPATH}`,
      },
    ],
  }),
});

// Compact and newline-free: these bytes are what the signature covers.
const wire = (value) => JSON.stringify(value);

const ENDED_BODY = wire(endedEvent("EV_fixtureEvent01", "1770000123"));

// Same event id, later delivery. LiveKit retries a webhook it did not get a
// 2xx for, and the retry is a fresh, validly signed message: dedup has to key
// on `id`, never on the signature or the body bytes.
const DUPLICATE_BODY = wire(endedEvent("EV_fixtureEvent01", "1770000188"));

// A different room's event, so the wrong-secret case below signs something
// that is otherwise a perfectly good message.
const ROOM_FINISHED_BODY = wire({
  event: "room_finished",
  id: "EV_fixtureEvent02",
  createdAt: "1770000124",
  room: {
    sid: ROOM_ID,
    name: ROOM_NAME,
    creationTime: "1769999880",
    numParticipants: 0,
  },
});

const bodies = {
  "webhook-egress-ended.json": ENDED_BODY,
  "webhook-egress-ended-duplicate.json": DUPLICATE_BODY,
  "webhook-room-finished.json": ROOM_FINISHED_BODY,
};

const cases = [
  {
    name: "egress_ended",
    body: "webhook-egress-ended.json",
    authorization: webhookAuthorization(API_SECRET, ENDED_BODY),
    verdict: "accepted",
  },
  {
    name: "egress_ended retry",
    body: "webhook-egress-ended-duplicate.json",
    authorization: webhookAuthorization(API_SECRET, DUPLICATE_BODY),
    verdict: "accepted",
  },
  {
    name: "room_finished",
    body: "webhook-room-finished.json",
    authorization: webhookAuthorization(API_SECRET, ROOM_FINISHED_BODY),
    verdict: "accepted",
  },
  {
    // Signed by someone who does not hold the project secret.
    name: "wrong secret",
    body: "webhook-egress-ended.json",
    authorization: webhookAuthorization("attacker-secret", ENDED_BODY),
    verdict: "bad_signature",
  },
  {
    // Correctly signed, for a different body. This is the case a checksum
    // catches and a signature alone does not: the JWT verifies, and the bytes
    // it vouches for are not the bytes that arrived.
    name: "body swapped under a valid signature",
    body: "webhook-egress-ended.json",
    authorization: webhookAuthorization(API_SECRET, ROOM_FINISHED_BODY),
    verdict: "body_mismatch",
  },
  {
    name: "expired",
    body: "webhook-egress-ended.json",
    authorization: webhookAuthorization(API_SECRET, ENDED_BODY, {
      issuedAt: ISSUED_AT - WEBHOOK_TTL_SECONDS - 1,
    }),
    verdict: "expired",
  },
  {
    name: "another project's key",
    body: "webhook-egress-ended.json",
    authorization: webhookAuthorization(API_SECRET, ENDED_BODY, { key: "APIotherproject" }),
    verdict: "unknown_key",
  },
  {
    name: "not a jwt",
    body: "webhook-egress-ended.json",
    authorization: "Bearer nope",
    verdict: "malformed",
  },
];

const files = {
  // Twirp: snake_case in, snake_case out.
  //
  // `advanced` rather than a preset, because the contract fixes 720p/30 at a
  // 2 Mbps ceiling and no EncodingOptionsPreset carries that bitrate. `aac`
  // rather than the OPUS default, because an MP4 EncodedFileOutput needs it.
  "start-egress-request.json": {
    room_name: ROOM_NAME,
    custom_base_url: `${TEMPLATE_BASE_URL}/recording/index.html`,
    audio_only: false,
    video_only: false,
    advanced: {
      width: 1280,
      height: 720,
      framerate: 30,
      video_codec: "H264_MAIN",
      video_bitrate: 2000,
      audio_codec: "AAC",
      audio_bitrate: 128,
    },
    file_outputs: [
      {
        file_type: "MP4",
        filepath: FILEPATH,
        disable_manifest: true,
        gcp: {
          credentials: "<service account key JSON>",
          bucket: "codetrial-recording-staging",
        },
      },
    ],
  },
  "start-egress-response.json": {
    egress_id: EGRESS_ID,
    room_id: ROOM_ID,
    room_name: ROOM_NAME,
    status: "EGRESS_STARTING",
    started_at: "0",
  },
  "stop-egress-request.json": { egress_id: EGRESS_ID },
  "stop-egress-response.json": {
    egress_id: EGRESS_ID,
    room_id: ROOM_ID,
    room_name: ROOM_NAME,
    status: "EGRESS_ENDING",
    started_at: "1770000000000000000",
  },
  "webhook-cases.json": {
    apiKey: API_KEY,
    apiSecret: API_SECRET,
    now: ISSUED_AT + 1,
    cases,
  },
};

const check = process.argv.includes("--check");
const stale = [];

mkdirSync(FIXTURES, { recursive: true });

function emit(name, wanted) {
  const path = join(FIXTURES, name);
  if (!check) {
    writeFileSync(path, wanted);
    return;
  }
  let found = null;
  try {
    found = readFileSync(path, "utf8");
  } catch {
    found = null;
  }
  if (found !== wanted) stale.push(name);
}

for (const [name, body] of Object.entries(bodies)) emit(name, body);
for (const [name, value] of Object.entries(files)) {
  emit(name, `${JSON.stringify(value, null, 2)}\n`);
}

if (check && stale.length) {
  console.error(
    `recording fixtures are stale: ${stale.join(", ")}\n` +
      "Run scripts/gen-recording-fixtures.mjs and re-run\n" +
      "cargo test --test recording_contract. A hand-edited webhook body is the\n" +
      "one edit these fixtures cannot survive: the signature covers the bytes.",
  );
  process.exit(1);
}
