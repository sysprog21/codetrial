// The two LiveKit RoomService calls the browser check makes, and the token
// that signs them.
//
// Hand-signed rather than taken from `livekit-server-sdk`: listing and
// removing a participant is one POST each, and the package would be the first
// npm dependency `npm ci` installs for something other than linting or driving
// a browser. The product does the same thing in Rust, so the harness now
// speaks LiveKit the way the server it tests speaks it.
//
// Its own module rather than part of `browser-check.cjs`, because that file
// requires Playwright at load and runs the whole check on require, so nothing
// there can be exercised without a browser and a credentialed room. This is
// the part worth testing offline, and `tests/browser/livekit-room-service.test.js`
// drives it against a local HTTP server.

const crypto = require("node:crypto");

/// The RoomService origin, derived the way `livekit_http_base` in
/// src/livekit/rooms.rs:216 derives it, because the harness reads the same
/// `codetrial.env.local` the server does and a URL the server accepts has to
/// work here too. The query and fragment come off before the Twirp path is
/// appended, the scheme is rewritten in any case because
/// `validate_livekit_url` accepts every spelling, and the path stays: a
/// LiveKit behind a proxy at `https://host/livekit` serves RoomService under
/// that prefix.
///
/// Userinfo is the one place this diverges from the Rust, which keeps it and
/// lets reqwest send it. `validate_livekit_url` accepts `wss://user:pass@host`,
/// and `fetch` refuses such a URL outright with a TypeError that quotes the
/// password back into the log. Dropping it keeps the call working, because
/// what authenticates here is the bearer token below, not the URL.
function livekitHttpBase(url = process.env.LIVEKIT_URL) {
  // Named here rather than left to `fetch`. One lane runs with LIVEKIT_URL
  // deliberately unset, and "Cannot read properties of undefined" names
  // neither the variable nor the lane that forgot it.
  if (!String(url ?? "").trim()) throw new Error("LIVEKIT_URL is not set");
  const [scheme, rest] = splitScheme(String(url).trim().split(/[?#]/)[0].replace(/\/+$/, ""));
  const authorityEnd = rest.indexOf("/");
  const authority = authorityEnd === -1 ? rest : rest.slice(0, authorityEnd);
  const path = authorityEnd === -1 ? "" : rest.slice(authorityEnd);
  const at = authority.lastIndexOf("@");
  return `${scheme}${authority.slice(at + 1)}${path}`;
}

/// `wss:` and `ws:` become their HTTP spellings in any case, because
/// `validate_livekit_url` accepts `WSS://`. Anything else is handed back
/// untouched, so an `https://` URL and an unparseable one both survive to be
/// judged by `fetch`.
function splitScheme(url) {
  const match = url.match(/^(wss?|https?):\/\//i);
  if (!match) return ["", url];
  const scheme = match[1].toLowerCase() === "wss" || match[1].toLowerCase() === "https"
    ? "https://"
    : "http://";
  return [scheme, url.slice(match[0].length)];
}

/// The same grant as `livekit_room_admin_token` in src/token.rs:119, down to
/// the `room-admin` subject, so the two spellings of this credential in the
/// tree stay one spelling. A minute of life rather than that function's two
/// hours: nothing here holds a token across calls.
function roomAdminToken(room, now = Math.floor(Date.now() / 1000)) {
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const header = encode({ alg: "HS256", typ: "JWT" });
  const payload = encode({
    iss: process.env.LIVEKIT_API_KEY,
    sub: "room-admin",
    nbf: now,
    exp: now + 60,
    video: { room, roomAdmin: true },
  });
  const claims = `${header}.${payload}`;
  const signature = crypto
    .createHmac("sha256", process.env.LIVEKIT_API_SECRET)
    .update(claims)
    .digest("base64url");
  return `${claims}.${signature}`;
}

/// One Twirp POST.
///
/// What comes back is protobuf JSON, so the fields are spelled as the wire
/// spells them: `can_subscribe`, `joined_at`, and the `permission.agent` that
/// the caller reads. The SDK camel-cased them, and anything reading a new
/// field has to use the wire name or silently get `undefined`.
async function roomService(method, body) {
  const response = await fetch(`${livekitHttpBase()}/twirp/livekit.RoomService/${method}`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${roomAdminToken(body.room)}`,
      "content-type": "application/json",
    },
    body: JSON.stringify(body),

    // Bounded, because `fetch` is not. `isolateRustAgent` gives itself two
    // minutes to settle a room and `soakInterview` checks on a deadline of its
    // own; a call that never answers would hold either of them past its own
    // budget and report as a hang with no line saying which request stopped.
    signal: AbortSignal.timeout(30000),
  });
  const text = await response.text();
  if (!response.ok) {
    // Status and Twirp code both, the pair `is_participant_gone` in
    // src/livekit/rooms.rs:115 matches on: a bare 404 can also mean the route
    // is wrong, and `not_found` in the body is what says the thing asked for
    // is simply not there.
    const error = new Error(`${method} failed: ${response.status} ${text}`);
    error.status = response.status;
    try {
      error.code = JSON.parse(text).code;
    } catch {}
    throw error;
  }

  // Named the way `parse_room_service_response` in src/livekit/rooms.rs:227
  // names it. A 200 that is not JSON is a proxy answering instead of LiveKit,
  // and "Unexpected end of JSON input" says nothing about which call, which
  // status, or what came back instead.
  try {
    return JSON.parse(text);
  } catch (error) {
    throw new Error(`${method} returned invalid JSON: ${error.message}: ${text.slice(0, 200)}`);
  }
}

/// Protobuf JSON omits an empty repeated field, so a room with nobody in it
/// can come back as `{}`.
async function listRoomParticipants(roomName) {
  return (await roomService("ListParticipants", { room: roomName })).participants ?? [];
}

/// Already gone counts as removed, the same call `remove_room_participant` in
/// src/livekit/rooms.rs:99 makes: the caller listed the room a moment ago, and
/// a participant that disconnected in between is the outcome being asked for,
/// not a failure.
async function removeParticipant(roomName, identity) {
  try {
    await roomService("RemoveParticipant", { room: roomName, identity });
  } catch (error) {
    if (!isGone(error)) throw error;
  }
}

/// The pair, not either half. A 404 alone is also what a wrong Twirp route
/// answers, and treating that as "not there yet" is how a misconfigured base
/// URL turns into a caller retrying for two minutes and then reporting
/// something else.
function isGone(error) {
  return error.status === 404 && error.code === "not_found";
}

module.exports = {
  isGone,
  listRoomParticipants,
  livekitHttpBase,
  removeParticipant,
  roomAdminToken,
  roomService,
};
