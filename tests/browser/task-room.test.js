import test from "node:test";
import assert from "node:assert/strict";
import { lend, st } from "../../web/task-state.js";
import {
  collectResult,
  connect,
  dropConnection,
  send,
  startNeverArrived,
} from "../../web/task-room.js";

// task-room.js reaches the page through `host`, so a node test imports it and
// lends it fakes. The DOM is a map of plain objects.
const elements = {};
globalThis.document = {
  getElementById: (id) =>
    (elements[id] ??= { disabled: false, textContent: "x" }),
  querySelectorAll: () => audio,
};
let audio = [];

test("the module's functions are importable without the page", () => {
  for (const fn of [
    collectResult,
    connect,
    dropConnection,
    send,
    startNeverArrived,
  ])
    assert.equal(typeof fn, "function");
});

test("a message is published as JSON on its topic, reliably", async () => {
  const published = [];
  st.room = {
    localParticipant: { publishData: async (...args) => published.push(args) },
  };
  try {
    await send("hint", { rung: 2 });
    const [bytes, options] = published[0];
    assert.equal(new TextDecoder().decode(bytes), '{"rung":2}');
    assert.deepEqual(options, { reliable: true, topic: "hint" });
  } finally {
    st.room = undefined;
  }
});

test("a message with no room throws and publishes nothing", async () => {
  st.room = undefined;
  await assert.rejects(send("hint", {}), /Task is not connected/);
});

test("letting go of a room forgets what belonged to it", () => {
  const locked = [];
  lend({ lockChoices: () => (value) => locked.push(value) });
  const disconnected = [];
  const removed = [];
  audio = [
    { remove: () => removed.push(1) },
    { remove: () => removed.push(2) },
  ];
  Object.assign(st, {
    room: {
      disconnect: (arg) => (
        disconnected.push(arg),
        Promise.reject(new Error("gone"))
      ),
    },
    admissionKey: "old-key",
    state: { phase: "work" },
    serverClockOffset: 5,
    calibration: { ok: true },
    media: {},
    acknowledgedAt: 123,
  });
  try {
    dropConnection();
    assert.equal(st.room, null);
    assert.deepEqual(
      disconnected,
      [false],
      "disconnects without stopping tracks",
    );
    assert.deepEqual(removed, [1, 2], "the room's audio elements go");
    assert.notEqual(st.admissionKey, "old-key", "a new room needs a new key");
    assert.equal(st.state, null);
    assert.equal(st.serverClockOffset, null);
    assert.equal(st.calibration, null);
    assert.equal(elements.start.disabled, true);
    assert.deepEqual(locked, [false], "the choices unlock");
    assert.equal(elements.calibrate.disabled, false, "media and rules are in");
    assert.equal(elements["calibration-result"].textContent, "");
  } finally {
    Object.assign(st, {
      room: undefined,
      state: undefined,
      media: null,
      acknowledgedAt: null,
    });
  }
});

test("calibration waits for the rules and the microphone and camera", () => {
  lend({ lockChoices: () => () => {} });
  audio = [];
  st.room = null;
  for (const [media, acknowledgedAt, disabled] of [
    [null, 123, true],
    [{}, null, true],
    [{}, 123, false],
  ]) {
    Object.assign(st, { media, acknowledgedAt });
    dropConnection();
    assert.equal(elements.calibrate.disabled, disabled);
  }
  Object.assign(st, { media: null, acknowledgedAt: null });
});

// A host whose every function records its name, in order, and returns what the
// caller may need.
function spyHost(log, extra = {}) {
  const spies = {};
  for (const name of [
    "show",
    "lock",
    "stopMonitoring",
    "leaveFullscreen",
    "rememberAttempt",
    "startAttemptTimers",
    "pollReview",
    "forgetAttempt",
    "stopAttemptTimers",
    "lockChoices",
    "showUnlock",
    "error",
  ])
    spies[name] =
      () =>
      (...args) =>
        log.push([name, ...args]);
  lend({
    ...spies,
    localRecordingHome: () => ({ append: (el) => log.push(["append", el]) }),
    ...extra,
  });
}

test("a room lost mid-attempt leaves the learner collecting the result", () => {
  const log = [];
  spyHost(log);
  Object.assign(st, {
    attempt: "running",
    task: { reviewDeliverySeconds: 90 },
  });
  const before = Date.now();
  collectResult();
  assert.equal(st.attempt, "lost");
  assert.equal(st.startUnconfirmed, false);
  assert.equal(st.startViolation, null);
  assert.deepEqual(
    log.map((entry) => entry[0]),
    [
      "show",
      "lock",
      "stopMonitoring",
      "leaveFullscreen",
      "rememberAttempt",
      "startAttemptTimers",
      "pollReview",
    ],
  );
  assert.deepEqual(log[0], ["show", "task"]);
  assert.ok(
    st.reviewPollUntil >= before + 90_000 &&
      st.reviewPollUntil <= Date.now() + 90_000,
  );
  assert.equal(
    elements["message-text"].textContent,
    "The room disconnected. Waiting for your result.",
  );
  assert.equal(elements.message.hidden, false);
  assert.equal(elements.recovery.hidden, true);
});

test("a room lost while Start was unconfirmed says it is checking", () => {
  spyHost([]);
  Object.assign(st, {
    attempt: "starting",
    task: { reviewDeliverySeconds: 1 },
  });
  collectResult();
  assert.equal(st.startUnconfirmed, true);
  assert.match(
    elements["message-text"].textContent,
    /Checking whether the attempt began/,
  );
});

test("an attempt that is already over is not collected again", () => {
  for (const attempt of ["done", "lost"]) {
    const log = [];
    spyHost(log);
    Object.assign(st, { attempt, startViolation: "kept" });
    collectResult();
    assert.deepEqual(log, [], attempt);
    assert.equal(st.attempt, attempt);
    assert.equal(st.startViolation, "kept", "nothing is reset");
  }
});

test("a Start that never arrived sends the learner back to prepare", () => {
  const log = [];
  spyHost(log);
  Object.assign(st, {
    task: { id: 1 },
    restored: false,
    acknowledgedAt: 7,
    media: null,
    activeRoomName: "r",
    startUnconfirmed: true,
    attempt: "lost",
    room: null,
  });
  startNeverArrived();
  assert.equal(st.attempt, "preparing");
  assert.equal(st.activeRoomName, null);
  assert.equal(st.startUnconfirmed, false);
  assert.deepEqual(
    log.map((entry) => entry[0]),
    [
      "forgetAttempt",
      "stopAttemptTimers",
      "lockChoices",
      "append",
      "show",
      "lock",
      "error",
    ],
  );
  assert.deepEqual(
    log.find((e) => e[0] === "show"),
    ["show", "prepare"],
  );
  assert.equal(
    elements.devices.disabled,
    false,
    "rules accepted, devices not held",
  );
  assert.equal(log.at(-1)[1].code, "devices_required");
});

test("a restored page that lost its Start asks for the PIN again", () => {
  const log = [];
  spyHost(log);
  Object.assign(st, { task: { id: 1 }, restored: true, room: null });
  startNeverArrived();
  assert.equal(st.restored, false);
  assert.deepEqual(
    log.slice(-2).map((entry) => entry[0]),
    ["showUnlock", "error"],
  );
  assert.equal(log.at(-1)[1].code, "unlock_required");
  assert.equal(st.attempt, "preparing");
});
