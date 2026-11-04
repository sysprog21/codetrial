import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  acceptsPreview,
  previewPayload,
  peerApi,
  peerRole,
} from "../../web/peer-lib.js";
const state = {
  you: { slot: 0 },
  members: [
    { slot: 0, identity: "a" },
    { slot: 1, identity: "b" },
  ],
  currentRound: {
    index: 0,
    status: "active",
    role: "interviewer",
  },
};
test("preview trusts participant identity and current round, never claimed sender", () => {
  const packet = previewPayload({
    round: 0,
    revision: 2,
    seq: 5,
    language: "python",
    code: "x",
  });
  assert.equal(acceptsPreview(state, { identity: "b" }, packet), true);
  assert.equal(
    acceptsPreview(
      state,
      { identity: "attacker" },
      { ...packet, identity: "b" },
    ),
    false,
  );
  assert.equal(
    acceptsPreview(state, { identity: "b" }, { ...packet, round: 1 }),
    false,
  );
  assert.equal(
    acceptsPreview(
      { ...state, currentRound: { ...state.currentRound, status: "closed" } },
      { identity: "b" },
      packet,
    ),
    false,
  );
  assert.equal(peerRole(state), "interviewer");
  assert.equal(peerRole({ ...state, currentRound: null }), null);
});
test("preview byte limit replaces code with revision notice", () => {
  const packet = previewPayload({
    round: 0,
    revision: 8,
    seq: 6,
    language: "python",
    code: "é".repeat(7000),
  });
  assert.equal(packet.kind, "revision");
  assert.equal(packet.code, undefined);
  assert.equal(packet.revision, 8);
  assert.equal(packet.seq, 6);
});
test("preview without an ordering number is refused", () => {
  const unordered = previewPayload({
    round: 0,
    revision: 2,
    seq: 1,
    language: "python",
    code: "x",
  });
  delete unordered.seq;
  assert.equal(acceptsPreview(state, { identity: "b" }, unordered), false);
});
test("candidate expects previews only from its partner slot", () => {
  const candidate = {
    ...state,
    you: { slot: 1 },
    currentRound: { ...state.currentRound, role: "candidate" },
  };
  const packet = { kind: "revision", round: 0, revision: 1, seq: 1 };
  assert.equal(acceptsPreview(candidate, { identity: "b" }, packet), true);
  assert.equal(acceptsPreview(candidate, { identity: "a" }, packet), false);
});

test("nickname trims and preserves case while requiring ASCII letters and optional digits", async () => {
  const { normalizePeerNickname } = await import("../../web/peer-lib.js");
  assert.equal(normalizePeerNickname(" Lucky7 "), "Lucky7");
  assert.equal(normalizePeerNickname("a"), "a");
  assert.equal(normalizePeerNickname("Z".repeat(24)), "Z".repeat(24));
  for (const invalid of [
    null,
    "",
    "123",
    "Lucky 7",
    "Lucky_7",
    "Lucky-7",
    "Luckyé",
    "🙂",
    "a".repeat(25),
  ])
    assert.equal(normalizePeerNickname(invalid), null, String(invalid));
});

for (const [code, expected] of [
  ["invalid_nickname", /letters, with digits allowed after them/],
  ["version_conflict", /partner just changed/],
  ["waiting_for_partner", /Waiting for your partner/],
  ["invite_unavailable", /Ask your partner for a new link/],
]) {
  test(`the server code ${code} reaches the person as a sentence`, async (t) => {
    const original = globalThis.fetch;
    t.after(() => {
      globalThis.fetch = original;
    });
    globalThis.fetch = async () => ({
      ok: false,
      status: 409,
      json: async () => ({ code }),
    });
    await assert.rejects(
      () => peerApi("/api/peer/sessions/s", {}),
      (error) => expected.test(error.message) && error.code === code,
    );
  });
}

test("a code the page does not know is shown as sent, never swallowed", async (t) => {
  const original = globalThis.fetch;
  t.after(() => {
    globalThis.fetch = original;
  });
  globalThis.fetch = async () => ({
    ok: false,
    status: 500,
    json: async () => ({ code: "brand_new_code" }),
  });
  await assert.rejects(
    () => peerApi("/api/peer/sessions/s", {}),
    /brand_new_code/,
  );
});

test("every consent surface states the 30-day retention of workspace data", () => {
  const sentence =
    /Workspace snapshots, your feedback and session details are kept for up to 30 days after the session is created/g;
  for (const [file, notices] of [
    ["peer-lobby.html", 1],
    ["peer-interview.html", 2],
    ["peer-presence.js", 1],
  ]) {
    const text = readFileSync(
      new URL(`../../web/${file}`, import.meta.url),
      "utf8",
    ).replaceAll(/\s+/g, " ");
    assert.equal(text.match(sentence)?.length, notices, file);
  }
});
