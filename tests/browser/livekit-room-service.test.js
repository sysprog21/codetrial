// The offline half of the LiveKit RoomService client the browser check uses.
//
// Everything here runs against a local HTTP server and a fixed clock, because
// the only other exercise this code gets is a credentialed dispatch run that
// spends a whole Gemini interview before it reaches the first call. The two
// bugs this file pins were both found by reading rather than by running: a
// bare 404 read as "the room is not there yet", and a `fetch` that refuses a
// URL carrying userinfo while quoting the password into the log.

import assert from "node:assert/strict";
import crypto from "node:crypto";
import http from "node:http";
import { createRequire } from "node:module";
import test from "node:test";

const require = createRequire(import.meta.url);
const roomService = require("../../scripts/livekit-room-service.cjs");

const KEY = "browser-check-key";
const SECRET = "browser-check-secret";

/// Each case owns its credentials: the module reads `process.env` at call
/// time, which is what lets `scripts/browser-check.sh` run one lane with
/// `LIVEKIT_URL` unset.
function withEnv(url, run) {
  const before = { ...process.env };
  process.env.LIVEKIT_URL = url;
  process.env.LIVEKIT_API_KEY = KEY;
  process.env.LIVEKIT_API_SECRET = SECRET;
  return (async () => {
    try {
      return await run();
    } finally {
      process.env = before;
    }
  })();
}

/// A stand-in for LiveKit that records what it was asked and answers what the
/// case wants. Returns the base URL and the requests seen.
async function twirpServer(reply) {
  const requests = [];
  const server = http.createServer((request, response) => {
    let body = "";
    request.setEncoding("utf8");
    request.on("data", (chunk) => (body += chunk));
    request.on("end", () => {
      requests.push({ method: request.method, url: request.url, headers: request.headers, body });
      const { status = 200, text = "{}", type = "application/json" } = reply(request.url) ?? {};
      response.writeHead(status, { "Content-Type": type }).end(text);
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return {
    requests,
    url: `http://127.0.0.1:${server.address().port}`,
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

test("the base URL is derived the way the server derives it", () => {
  const cases = [
    ["wss://host.livekit.cloud", "https://host.livekit.cloud"],
    ["wss://host.livekit.cloud/", "https://host.livekit.cloud"],
    ["  wss://host.livekit.cloud//  ", "https://host.livekit.cloud"],
    // validate_livekit_url accepts every spelling of the scheme, so a pasted
    // WSS:// must not reach fetch as a websocket URL.
    ["WSS://host.livekit.cloud", "https://host.livekit.cloud"],
    ["ws://127.0.0.1:7880", "http://127.0.0.1:7880"],
    ["https://host.livekit.cloud", "https://host.livekit.cloud"],
    // The query would otherwise take the method name with it.
    ["wss://host.livekit.cloud?region=eu", "https://host.livekit.cloud"],
    ["wss://host.livekit.cloud/#frag", "https://host.livekit.cloud"],
    // A LiveKit behind a reverse proxy serves RoomService under the prefix.
    ["wss://host.example/livekit/", "https://host.example/livekit"],
    // fetch refuses a URL with credentials in it, and says so with the
    // password in the message.
    ["wss://user:pa55@host.livekit.cloud", "https://host.livekit.cloud"],
  ];
  for (const [input, expected] of cases) {
    assert.equal(roomService.livekitHttpBase(input), expected, input);
  }
});

test("a missing LIVEKIT_URL is named, not dereferenced", () => {
  assert.throws(() => roomService.livekitHttpBase(undefined), /LIVEKIT_URL is not set/);
  assert.throws(() => roomService.livekitHttpBase("  "), /LIVEKIT_URL is not set/);
});

test("a 200 that is not JSON says which call and what came back", async () => {
  // A proxy answering instead of LiveKit. "Unexpected end of JSON input" on
  // its own names neither.
  const server = await twirpServer(() => ({ text: "<html>hello</html>", type: "text/html" }));
  try {
    const error = await withEnv(server.url, () =>
      roomService.listRoomParticipants("interview-abc").then(() => null, (thrown) => thrown));
    assert.match(error.message, /ListParticipants returned invalid JSON/);
    assert.match(error.message, /<html>hello<\/html>/);
  } finally {
    await server.close();
  }
});

test("the admin token carries the grant src/token.rs signs", () => {
  return withEnv("wss://host.livekit.cloud", () => {
    const token = roomService.roomAdminToken("interview-abc", 1_700_000_000);
    const [header, payload, signature] = token.split(".");
    assert.equal(token.split(".").length, 3);
    assert.deepEqual(JSON.parse(Buffer.from(header, "base64url").toString()), {
      alg: "HS256",
      typ: "JWT",
    });
    assert.deepEqual(JSON.parse(Buffer.from(payload, "base64url").toString()), {
      iss: KEY,
      sub: "room-admin",
      nbf: 1_700_000_000,
      exp: 1_700_000_060,
      video: { room: "interview-abc", roomAdmin: true },
    });

    // Signed over the two encoded halves with the secret, not the key: a
    // signature checked against anything this function itself computes would
    // agree with a wrong algorithm too.
    assert.equal(
      signature,
      crypto.createHmac("sha256", SECRET).update(`${header}.${payload}`).digest("base64url"),
    );
  });
});

test("listing a room posts a signed Twirp request and returns its participants", async () => {
  const server = await twirpServer(() => ({
    text: JSON.stringify({ participants: [{ identity: "interviewer-x", permission: { agent: true } }] }),
  }));
  try {
    const participants = await withEnv(server.url, () =>
      roomService.listRoomParticipants("interview-abc"));
    assert.deepEqual(participants, [{ identity: "interviewer-x", permission: { agent: true } }]);
    assert.equal(server.requests.length, 1);
    const [request] = server.requests;
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/twirp/livekit.RoomService/ListParticipants");
    assert.deepEqual(JSON.parse(request.body), { room: "interview-abc" });
    assert.match(request.headers.authorization, /^Bearer [\w-]+\.[\w-]+\.[\w-]+$/);
    assert.equal(request.headers["content-type"], "application/json");
  } finally {
    await server.close();
  }
});

test("an empty room comes back as no participants, not as undefined", async () => {
  // Protobuf JSON omits an empty repeated field.
  const server = await twirpServer(() => ({ text: "{}" }));
  try {
    assert.deepEqual(
      await withEnv(server.url, () => roomService.listRoomParticipants("interview-abc")),
      [],
    );
  } finally {
    await server.close();
  }
});

test("a Twirp refusal carries its status and its code", async () => {
  const server = await twirpServer(() => ({
    status: 404,
    text: JSON.stringify({ code: "not_found", msg: "requested room does not exist" }),
  }));
  try {
    const error = await withEnv(server.url, () =>
      roomService.listRoomParticipants("interview-abc").then(() => null, (thrown) => thrown));
    assert.ok(error, "listing a missing room must reject");
    assert.equal(error.status, 404);
    assert.equal(error.code, "not_found");
    assert.ok(roomService.isGone(error));
  } finally {
    await server.close();
  }
});

test("a 404 that is not a Twirp answer is not treated as a missing room", async () => {
  // A wrong base URL or proxy prefix answers 404 with an HTML error page. Read
  // as "not there yet" it makes the caller retry for its whole timeout and
  // then report something else.
  const server = await twirpServer(() => ({ status: 404, text: "<html>no route</html>", type: "text/html" }));
  try {
    const error = await withEnv(server.url, () =>
      roomService.listRoomParticipants("interview-abc").then(() => null, (thrown) => thrown));
    assert.equal(error.status, 404);
    assert.equal(error.code, undefined);
    assert.equal(roomService.isGone(error), false);
  } finally {
    await server.close();
  }
});

test("removing a participant that already left is not a failure", async () => {
  const server = await twirpServer(() => ({
    status: 404,
    text: JSON.stringify({ code: "not_found", msg: "participant does not exist" }),
  }));
  try {
    await withEnv(server.url, () => roomService.removeParticipant("interview-abc", "stray"));
    assert.equal(server.requests[0].url, "/twirp/livekit.RoomService/RemoveParticipant");
    assert.deepEqual(JSON.parse(server.requests[0].body), { room: "interview-abc", identity: "stray" });
  } finally {
    await server.close();
  }
});

test("removing a participant still fails on anything else", async () => {
  const server = await twirpServer(() => ({ status: 500, text: JSON.stringify({ code: "internal" }) }));
  try {
    const error = await withEnv(server.url, () =>
      roomService.removeParticipant("interview-abc", "stray").then(() => null, (thrown) => thrown));
    assert.ok(error, "a 500 must reach the caller");
    assert.equal(error.status, 500);
    assert.match(error.message, /RemoveParticipant failed: 500/);
  } finally {
    await server.close();
  }
});
