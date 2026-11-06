import test from "node:test";
import assert from "node:assert/strict";
import { lend, st } from "../../web/task-state.js";
import { startAttempt } from "../../web/task-start.js";

// task-start.js reaches the page through `host`, so a node test imports it and
// lends it fakes. The DOM is a map of plain objects.
const elements = {};
const el = (id) =>
  (elements[id] ??= { disabled: false, textContent: "", prepend() {} });
globalThis.document = {
  getElementById: el,
  querySelectorAll: () => [],
  fullscreenElement: {},
  hidden: false,
};

const liveCamera = { getVideoTracks: () => [{ readyState: "live" }] };
const FULLSCREEN_REQUIRED = {
  code: "fullscreen_required",
  message: "Fullscreen.",
};
let log, published;

// Everything start touches, as it stood before the click.
function setUp({
  phase = "work",
  sendFails = false,
  fullscreen = true,
  duringSend = () => {},
} = {}) {
  log = [];
  published = [];
  const spies = {};
  for (const name of [
    "releaseMedia",
    "rememberAttempt",
    "startAttemptTimers",
    "show",
    "clearError",
    "armRules",
    "endAttempt",
    "leaveFullscreen",
    "error",
    "lock",
    "stopMonitoring",
    "lockChoices",
    "pollReview",
  ])
    spies[name] =
      () =>
      (...args) =>
        log.push([name, ...args]);
  lend({
    ...spies,
    localRecording: () => ({ settingUp: false }),
    recoveryKey: () => () => "key-1",
    startReady: () => () => true,
    FULLSCREEN_REQUIRED: () => FULLSCREEN_REQUIRED,
  });
  Object.assign(st, {
    room: {
      disconnect: () => Promise.resolve(),
      localParticipant: {
        publishData: async (...args) => {
          duringSend();
          if (sendFails) throw new Error("no");
          published.push(args);
        },
      },
    },
    calibration: { ok: true },
    attempt: "preparing",
    media: liveCamera,
    acknowledgedAt: 99,
    activeRoomName: "room-a",
    task: { rules: { durationMin: 15 }, reviewDeliverySeconds: 60 },
    state: { phase },
    startViolation: null,
    startInFlight: null,
    currentStart: null,
    enteringFullscreen: false,
    reviewPollUntil: undefined,
  });
  document.fullscreenElement = fullscreen ? {} : undefined;
  document.hidden = false;
  el("workspace").requestFullscreen = () => Promise.resolve();
  el("start").disabled = false;
}
const settle = () => new Promise((resolve) => setTimeout(resolve, 20));
const names = () => log.map((entry) => entry[0]);

test("Start does nothing without a room, a calibration, or while started", async () => {
  for (const change of [
    { room: null },
    { calibration: null },
    { attempt: "running" },
  ]) {
    setUp();
    Object.assign(st, change);
    startAttempt();
    await settle();
    assert.equal(st.currentStart, null, JSON.stringify(change));
    assert.equal(elements.start.disabled, false);
    assert.deepEqual(log, []);
  }
});

test("Start waits while a local recording is being set up", async () => {
  setUp();
  lend({ localRecording: () => ({ settingUp: true }) });
  startAttempt();
  await settle();
  assert.equal(st.currentStart, null);
});

test("a camera that stopped since calibration sends the learner back", () => {
  setUp();
  st.media = { getVideoTracks: () => [{ readyState: "ended" }] };
  lend({ lockChoices: () => () => {} });
  startAttempt();
  assert.deepEqual(names(), ["releaseMedia", "error"]);
  assert.equal(log[1][1].code, "devices_required");
  assert.equal(st.room, null, "the room is let go");
  assert.equal(elements.devices.disabled, false, "rules were accepted");
  assert.equal(elements.calibrate.disabled, true);
});

test("Start sends once, waits for the acknowledgement, then arms the rules", async () => {
  setUp();
  startAttempt();
  assert.equal(elements.start.disabled, true, "the button locks at the click");
  assert.equal(
    st.enteringFullscreen,
    true,
    "fullscreen is asked inside the click",
  );
  await settle();
  assert.equal(st.attempt, "running");
  assert.equal(st.enteringFullscreen, false);
  assert.equal(st.startInFlight, null);
  assert.equal(new TextDecoder().decode(published[0][0]), '{"version":1}');
  assert.equal(published[0][1].topic, "task_start");
  assert.deepEqual(names(), [
    "rememberAttempt",
    "rememberAttempt",
    "startAttemptTimers",
    "show",
    "clearError",
    "armRules",
  ]);
  assert.deepEqual(
    [log[0][1], log[0][3], log[0][4]],
    ["room-a", "key-1", true],
    "the first record says Start is on its way",
  );
  assert.equal(
    log[1][4],
    false,
    "the second record says Start was acknowledged",
  );
  assert.deepEqual(log[3], ["show", "task"]);
  assert.ok(st.reviewPollUntil > Date.now() + (15 * 60 + 59) * 1000);
});

test("Start keeps waiting while the server reports ready, then arms the rules", async () => {
  setUp({ phase: "ready" });
  startAttempt();
  await settle();
  assert.equal(st.attempt, "starting", "waiting, not running");
  assert.ok(!names().includes("armRules"));
  st.state = { phase: "work" };
  // Outwaits the 200 ms poll in startAttempt that notices the new phase.
  await new Promise((resolve) => setTimeout(resolve, 400));
  assert.equal(st.attempt, "running");
  assert.ok(names().includes("armRules"));
});

test("Start without fullscreen is refused and the page stays preparing", async () => {
  setUp({ fullscreen: false });
  startAttempt();
  await settle();
  assert.equal(st.attempt, "preparing");
  assert.equal(published.length, 0, "nothing is sent");
  assert.deepEqual(names(), ["leaveFullscreen", "error"]);
  assert.equal(log[1][1], FULLSCREEN_REQUIRED);
  assert.equal(elements.start.disabled, false, "ready again");
});

test("a Start that cannot be sent drops the room and asks to calibrate again", async () => {
  setUp({ sendFails: true });
  lend({ lockChoices: () => () => {} });
  startAttempt();
  await settle();
  assert.equal(st.attempt, "preparing");
  assert.equal(st.startInFlight, null);
  assert.equal(st.room, null);
  const error = log.find((entry) => entry[0] === "error")[1];
  assert.equal(error.code, "setup_failed");
  // The room was just dropped, so the page reports the connection and not the
  // thrown "Start could not be sent" text.
  assert.match(error.message, /The connection ended before Start/);
});

test("a Start the server did not acknowledge leaves the room to collect the result", async () => {
  setUp({ phase: "feedback" });
  lend({ lockChoices: () => () => {} });
  startAttempt();
  await settle();
  assert.equal(st.attempt, "lost");
  assert.equal(st.startUnconfirmed, true);
  assert.equal(st.room, null, "the room is dropped after collecting");
  assert.ok(!names().includes("armRules"));
});

test("a rule broken while Start was on its way ends the attempt at once", async () => {
  // The page clears the violation just before sending, so it has to happen
  // during the send, which is when the page is waiting for the server.
  setUp({ duringSend: () => (st.startViolation = "visibility") });
  startAttempt();
  await settle();
  assert.equal(st.attempt, "running");
  assert.deepEqual(log.at(-1), ["endAttempt", "invalid", "visibility", 0]);
  assert.equal(st.startViolation, null);
});

test("a newer Start that took over during the send is left alone", async () => {
  setUp({ duringSend: () => (st.currentStart = {}) });
  startAttempt();
  await settle();
  assert.equal(st.attempt, "starting", "this Start no longer owns the page");
  assert.deepEqual(log, [], "it records and arms nothing");
});

test("a loss that moved the page to collecting the result is left alone", async () => {
  setUp({ duringSend: () => (st.attempt = "lost") });
  startAttempt();
  await settle();
  assert.equal(st.attempt, "lost");
  assert.ok(!names().includes("armRules"));
  assert.ok(!names().includes("rememberAttempt"));
});

test("a different room before fullscreen resolves is left to its own Start", async () => {
  setUp();
  el("workspace").requestFullscreen = () => {
    st.room = { other: true };
    return Promise.resolve();
  };
  startAttempt();
  await settle();
  assert.equal(published.length, 0, "nothing is sent");
  assert.equal(st.enteringFullscreen, false);
  assert.ok(!names().includes("error"), "no message for a room that is gone");
});

test("a calibration cleared before fullscreen resolves asks to calibrate again", async () => {
  setUp();
  el("workspace").requestFullscreen = () => {
    st.calibration = null;
    return Promise.resolve();
  };
  startAttempt();
  await settle();
  assert.equal(published.length, 0);
  assert.deepEqual(names(), ["leaveFullscreen", "error"]);
  assert.equal(log[1][1].code, "setup_failed");
  assert.match(log[1][1].message, /The connection ended before Start/);
});
