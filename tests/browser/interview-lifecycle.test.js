import { test } from "node:test";
import assert from "node:assert/strict";
import {
  createBeforeUnloadGuard,
  shouldWarnBeforeUnload,
} from "../../web/before-unload.js";
import { reportIsPersisted } from "../../web/history.js";
import { functionBody, read } from "./source.js";

function beforeUnloadEvent(target) {
  const event = new Event("beforeunload", { cancelable: true });
  Object.defineProperty(event, "returnValue", {
    configurable: true,
    writable: true,
    value: null,
  });
  target.dispatchEvent(event);
  return event;
}

class CountingTarget extends EventTarget {
  added = 0;
  removed = 0;

  addEventListener(type, ...args) {
    if (type === "beforeunload") this.added += 1;
    super.addEventListener(type, ...args);
  }

  removeEventListener(type, ...args) {
    if (type === "beforeunload") this.removed += 1;
    super.removeEventListener(type, ...args);
  }
}

test("the warning follows interview start through report persistence", () => {
  const target = new CountingTarget();
  const guard = createBeforeUnloadGuard(target);
  const lifecycle = { endsAt: 0, reportPersisted: false };
  const update = () => guard.setEnabled(shouldWarnBeforeUnload(lifecycle));

  // The media/device preflight has no deadline yet, so leaving stays quiet.
  update();
  assert.equal(beforeUnloadEvent(target).defaultPrevented, false);

  // init() sets endsAt only after connect() finishes; this is the timed start.
  lifecycle.endsAt = Date.now() + 45 * 60 * 1000;
  update();
  assert.equal(beforeUnloadEvent(target).defaultPrevented, true);
  assert.equal(beforeUnloadEvent(target).returnValue, true);

  // A failed save is not enough. Either actual Past attempts source can make
  // the report reachable; once one reports success, the listener is removed.
  lifecycle.reportPersisted = reportIsPersisted({
    local: "failed",
    account: "failed",
  });
  update();
  assert.equal(beforeUnloadEvent(target).defaultPrevented, true);

  lifecycle.reportPersisted = reportIsPersisted({
    local: "saved",
    account: "failed",
  });
  update();
  assert.equal(beforeUnloadEvent(target).defaultPrevented, false);
  assert.equal(target.added, 1, "the listener is not registered twice");
  assert.equal(target.removed, 1, "successful persistence removes it");
});

test("the unload listener can be disposed without leaving a stale handler", () => {
  const target = new CountingTarget();
  const guard = createBeforeUnloadGuard(target);

  guard.setEnabled(true);
  guard.setEnabled(true);
  assert.equal(target.added, 1);
  assert.equal(beforeUnloadEvent(target).defaultPrevented, true);

  guard.dispose();
  guard.dispose();
  assert.equal(target.removed, 1);
  assert.equal(beforeUnloadEvent(target).defaultPrevented, false);
});

test("interview wiring starts after preflight and only persistence releases it", () => {
  const page = read("web/interview.js");
  const init = functionBody(page, "init");
  const preflight = init.indexOf("const preflight = await runAudioCheck()");
  const connect = init.indexOf("await connect(preflight, presenting)");
  const deadline = init.indexOf("state.endsAt = Date.now()");
  const guard = init.indexOf("updateBeforeUnloadGuard()", deadline);

  assert.ok(preflight !== -1 && preflight < connect);
  assert.ok(connect !== -1 && connect < deadline);
  assert.ok(deadline !== -1 && deadline < guard);
  assert.match(page, /endsAt: 0,[\s\S]*reportPersisted: false/);
  assert.match(
    page,
    /shouldWarnBeforeUnload\([\s\S]*endsAt: state\.endsAt,[\s\S]*reportPersisted: state\.reportPersisted/,
  );

  const ending = functionBody(page, "endInterview");
  assert.doesNotMatch(ending, /beforeUnloadGuard\.setEnabled\(false\)/);
  const save = functionBody(page, "saveHistory");
  assert.match(save, /state\.reportPersisted = false/);
  assert.match(save, /const generation = \+\+saveGeneration/);
  assert.match(
    save,
    /generation === saveGeneration &&\s*reportIsPersisted\(result\)/,
  );
  assert.ok(
    save.indexOf("reportIsPersisted(result)") >
      save.indexOf("await saveReportHistory(entry)"),
  );
});

test("LiveKit stays connected while the native leave warning is pending", () => {
  const connect = functionBody(read("web/interview.js"), "connectLiveKit");
  const roomOptions = /new livekit\.Room\(\{([\s\S]*?)\}\)/.exec(connect)?.[1];

  assert.ok(roomOptions, "the interview creates its LiveKit room here");
  // LiveKit defaults this option to true and disconnects on beforeunload,
  // before the browser can honor the candidate choosing to stay on the page.
  assert.match(roomOptions, /disconnectOnPageLeave:\s*false/);
});

test("the room disconnects on actual pagehide, not when beforeunload is cancelled", () => {
  const source = read("web/interview.js");
  assert.match(
    source,
    /window\.addEventListener\("pagehide", finalizeRecoveryOnPageHide\)/,
  );

  const target = new EventTarget();
  const calls = [];
  const state = {
    phase: "live",
    room: {
      disconnect() {
        calls.push("disconnect");
        return Promise.resolve();
      },
    },
  };
  const reportRecovery = { finish: () => calls.push("finish") };
  const body = functionBody(source, "finalizeRecoveryOnPageHide");
  const pagehide = new Function(
    "state",
    "reportRecovery",
    `${body}\n}\nreturn finalizeRecoveryOnPageHide;`,
  )(state, reportRecovery);
  const guard = createBeforeUnloadGuard(target);

  guard.setEnabled(true);
  target.addEventListener("pagehide", pagehide);

  // A cancelled native prompt dispatches beforeunload, but no pagehide.
  assert.equal(beforeUnloadEvent(target).defaultPrevented, true);
  assert.deepEqual(calls, []);

  // Once navigation really proceeds, pagehide performs room cleanup.
  target.dispatchEvent(new Event("pagehide"));
  assert.deepEqual(calls, ["disconnect"]);

  guard.dispose();
  target.removeEventListener("pagehide", pagehide);
});

test("the Leave button releases the warning before tearing down the session", () => {
  const source = read("web/interview.js");
  const calls = [];
  const target = new CountingTarget();
  const guard = createBeforeUnloadGuard(target);
  guard.setEnabled(true);

  const location = {};
  Object.defineProperty(location, "href", {
    set(value) {
      calls.push(`navigate:${value}`);
      assert.equal(beforeUnloadEvent(target).defaultPrevented, false);
    },
  });
  target.location = location;

  const beforeUnloadGuard = {
    dispose() {
      calls.push("guard-off");
      guard.dispose();
    },
  };
  const body = functionBody(source, "leaveRoom");
  const leaveRoom = new Function(
    "state",
    "reportRecovery",
    "beforeUnloadGuard",
    "window",
    "stopEndingClock",
    "stopAvatar",
    "stopLocalMedia",
    `${body}\n}\nreturn leaveRoom;`,
  )(
    {
      phase: "ending",
      reportReceiving: false,
      room: {
        disconnect() {
          calls.push("disconnect");
          return Promise.resolve();
        },
      },
    },
    { finish: () => calls.push("finish") },
    beforeUnloadGuard,
    target,
    () => calls.push("stop-clock"),
    () => calls.push("stop-avatar"),
    () => calls.push("stop-media"),
  );

  leaveRoom();

  assert.deepEqual(calls, [
    "guard-off",
    "stop-clock",
    "disconnect",
    "stop-avatar",
    "stop-media",
    "navigate:/",
  ]);
  assert.equal(target.removed, 1);
});

test("only the newest overlapping report save can release the warning", async () => {
  const source = read("web/interview.js");
  const pending = [];
  const guardStates = [];
  const state = {
    report: null,
    reportId: "attempt-1",
    interviewId: "interview-1",
    language: "javascript",
    reportPersisted: true,
  };
  const saveReportHistory = (entry) =>
    new Promise((resolve) => pending.push({ entry, resolve }));
  const updateBeforeUnloadGuard = () => guardStates.push(state.reportPersisted);
  const body = functionBody(source, "saveHistory");
  const saveHistory = new Function(
    "state",
    "randomId",
    "problem",
    "whiteboard",
    "durationMin",
    "interviewLoop",
    "mode",
    "saveReportHistory",
    "reportIsPersisted",
    "updateBeforeUnloadGuard",
    `let saveGeneration = 0;\n${body}\n}\nreturn saveHistory;`,
  )(
    state,
    () => "unused",
    { page: "two-sum", title: "Two Sum", difficulty: "Easy" },
    false,
    45,
    "coding_only",
    "whiteboard",
    saveReportHistory,
    reportIsPersisted,
    updateBeforeUnloadGuard,
  );

  const provisional = saveHistory({ summary: "provisional" });
  const final = saveHistory({ summary: "final" });
  assert.equal(pending.length, 2);
  assert.equal(pending[0].entry.interviewMode, "whiteboard");
  assert.equal(pending[0].entry.interviewLoop, "coding_only");
  assert.equal(pending[0].entry.durationMin, 45);
  assert.equal(state.reportPersisted, false);

  // The older provisional save finishes successfully after the final save has
  // already started. Its success must not release the guard for the final copy.
  pending[0].resolve({ local: "saved", account: "failed" });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(state.reportPersisted, false);

  pending[1].resolve({ local: "failed", account: "failed" });
  await Promise.all([provisional, final]);
  assert.equal(state.reportPersisted, false);
  assert.deepEqual(guardStates, [false, false]);
});

test("pagehide finalizes recovery before disconnecting the room", () => {
  const source = read("web/interview.js");
  const body = functionBody(source, "finalizeRecoveryOnPageHide");
  const calls = [];
  const handler = new Function(
    "state",
    "reportRecovery",
    `${body}\n}\nreturn finalizeRecoveryOnPageHide;`,
  )(
    {
      phase: "report_recovery",
      room: {
        disconnect() {
          calls.push("disconnect");
          return Promise.resolve();
        },
      },
    },
    { finish: () => calls.push("finish") },
  );

  handler();
  assert.deepEqual(calls, ["finish", "disconnect"]);
});
