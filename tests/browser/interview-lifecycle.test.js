import { test } from "node:test";
import assert from "node:assert/strict";
import {
  createBeforeUnloadGuard,
  shouldWarnBeforeUnload,
} from "../../web/before-unload.js";
import { reportIsPersisted } from "../../web/history.js";
import { MAX_REPORT_BODY_BYTES, withUnclearLines } from "../../web/lib.js";
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
    "localRecording",
    "releaseStart",
    `${body}\n}\nreturn finalizeRecoveryOnPageHide;`,
  )(state, reportRecovery, { dispose: () => calls.push("dispose") }, () =>
    calls.push("release"),
  );
  const guard = createBeforeUnloadGuard(target);

  guard.setEnabled(true);
  target.addEventListener("pagehide", pagehide);

  // A cancelled native prompt dispatches beforeunload, but no pagehide.
  assert.equal(beforeUnloadEvent(target).defaultPrevented, true);
  assert.deepEqual(calls, []);

  // Once navigation really proceeds, pagehide performs room cleanup.
  target.dispatchEvent(new Event("pagehide"));
  assert.deepEqual(calls, ["dispose", "disconnect", "release"]);

  // A page the back-forward cache keeps comes back to the same start.
  calls.length = 0;
  pagehide({ persisted: true });
  assert.deepEqual(calls, ["dispose", "disconnect"]);

  guard.dispose();
  target.removeEventListener("pagehide", pagehide);
});

test("the Leave button releases the warning before tearing down the session", async () => {
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
    "localRecording",
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
    { stop: async () => {} },
  );

  await leaveRoom();

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

for (const admittedOwner of ["account:A", "device", null]) {
  test(`leaving stores a selected focus only for admitted owner ${admittedOwner}`, async () => {
    const source = read("web/interview.js");
    const calls = [];
    const target = new CountingTarget();
    const guard = createBeforeUnloadGuard(target);
    guard.setEnabled(true);
    const location = {};
    Object.defineProperty(location, "href", {
      set(value) {
        calls.push(`navigate:${value}`);
      },
    });
    target.location = location;
    const beforeUnloadGuard = {
      dispose() {
        calls.push("guard-off");
        guard.dispose();
      },
    };
    const focus = {
      phase: "Test",
      weakness: "Test boundaries",
      drill: "Build a test table",
      successCriterion: "Predict each output",
    };
    const tabStorage = {};
    const storeSelectedPracticeFocus = (
      storage,
      owner,
      selected,
      interviewMode,
      sourceReportId,
    ) => {
      assert.equal(owner, admittedOwner);
      assert.equal(storage, tabStorage);
      assert.deepEqual(selected, focus);
      assert.equal(interviewMode, "coding");
      assert.equal(sourceReportId, "report-1");
      calls.push("store-focus");
      return owner !== null;
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
      "localRecording",
      "storeSelectedPracticeFocus",
      "tabStorage",
      "admittedPracticeFocusOwner",
      "sessionPromise",
      "mode",
      `${body}\n}\nreturn leaveRoom;`,
    )(
      {
        phase: "ending",
        reportReceiving: false,
        leaving: false,
        reportId: "report-1",
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
      { stop: async () => {} },
      storeSelectedPracticeFocus,
      tabStorage,
      admittedOwner,
      Promise.resolve({ signedIn: true, user: { id: "initial-account" } }),
      "coding",
    );
    await leaveRoom(focus);
    assert.deepEqual(
      calls,
      admittedOwner
        ? [
            "store-focus",
            "guard-off",
            "stop-clock",
            "disconnect",
            "stop-avatar",
            "stop-media",
            "navigate:/",
          ]
        : ["store-focus"],
    );
  });
}

test("a failed practice focus save keeps the report open", async () => {
  const source = read("web/interview.js");
  const calls = [];
  const focus = {
    phase: "Test",
    weakness: "Test boundaries",
    drill: "Build a test table",
    successCriterion: "Predict each output",
  };
  const status = {
    hidden: true,
    textContent: "",
  };
  const practiceButton = {
    parentElement: {
      querySelector(selector) {
        assert.equal(selector, "[data-practice-focus-status]");
        return status;
      },
    },
  };
  const target = {
    location: {
      set href(value) {
        calls.push(`navigate:${value}`);
      },
    },
  };
  const state = {
    phase: "report",
    reportReceiving: false,
    reportPersisted: true,
    downloadingReport: false,
    leaving: false,
    reportId: "report-1",
    room: null,
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
    "localRecording",
    "storeSelectedPracticeFocus",
    "tabStorage",
    "admittedPracticeFocusOwner",
    "sessionPromise",
    "mode",
    `${body}\n}\nreturn leaveRoom;`,
  )(
    state,
    {},
    {
      dispose() {
        calls.push("guard-off");
      },
    },
    target,
    () => calls.push("stop-clock"),
    () => calls.push("stop-avatar"),
    () => calls.push("stop-media"),
    {
      stop: async () => {
        calls.push("stop-recording");
      },
    },
    () => {
      calls.push("store-focus");
      return false;
    },
    {},
    "account:A",
    Promise.resolve(),
    "coding",
  );
  await leaveRoom(focus, practiceButton);
  assert.deepEqual(calls, ["store-focus"]);
  assert.equal(state.leaving, false);
  assert.equal(status.hidden, false);
  assert.equal(status.textContent, "Could not save this practice focus.");
});

test("a second leave is ignored while the first is still stopping the recording", async () => {
  const source = read("web/interview.js");
  const calls = [];
  let releaseRecording;
  const recordingStopped = new Promise((resolve) => {
    releaseRecording = resolve;
  });
  const focus = {
    phase: "Test",
    weakness: "Test boundaries",
    drill: "Build a test table",
    successCriterion: "Predict each output",
  };
  const target = {
    location: {
      set href(value) {
        calls.push(`navigate:${value}`);
      },
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
    "localRecording",
    "storeSelectedPracticeFocus",
    "tabStorage",
    "admittedPracticeFocusOwner",
    "sessionPromise",
    "mode",
    `${body}\n}\nreturn leaveRoom;`,
  )(
    {
      phase: "report",
      reportReceiving: false,
      reportPersisted: true,
      downloadingReport: false,
      leaving: false,
      reportId: "report-1",
      room: null,
    },
    {},
    {
      dispose() {
        calls.push("guard-off");
      },
    },
    target,
    () => calls.push("stop-clock"),
    () => calls.push("stop-avatar"),
    () => calls.push("stop-media"),
    {
      stop: async () => {
        calls.push("stop-recording");
        await recordingStopped;
      },
    },
    () => {
      calls.push("store-focus");
      return true;
    },
    {},
    "account:A",
    Promise.resolve(),
    "coding",
  );
  const first = leaveRoom(focus);
  // Let the first call reach localRecording.stop().
  await Promise.resolve();
  const second = leaveRoom(focus);
  await second;
  assert.deepEqual(calls, [
    "store-focus",
    "guard-off",
    "stop-clock",
    "stop-avatar",
    "stop-media",
    "stop-recording",
  ]);
  releaseRecording();
  await first;
  assert.deepEqual(calls, [
    "store-focus",
    "guard-off",
    "stop-clock",
    "stop-avatar",
    "stop-media",
    "stop-recording",
    "navigate:/",
  ]);
});

test("cancelling an unsafe leave does not store the selected practice focus", async () => {
  const source = read("web/interview.js");
  const calls = [];
  const focus = {
    phase: "Test",
    weakness: "Test boundaries",
    drill: "Build a test table",
    successCriterion: "Predict each output",
  };
  const target = {
    confirm(message) {
      calls.push(`confirm:${message}`);
      return false;
    },
    location: {},
  };
  const beforeUnloadGuard = {
    dispose() {
      calls.push("guard-off");
    },
  };
  const storeSelectedPracticeFocus = () => {
    calls.push("store-focus");
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
    "localRecording",
    "storeSelectedPracticeFocus",
    "tabStorage",
    "admittedPracticeFocusOwner",
    "sessionPromise",
    `${body}\n}\nreturn leaveRoom;`,
  )(
    {
      phase: "report",
      reportReceiving: false,
      reportPersisted: false,
      downloadingReport: false,
      room: null,
    },
    {
      stop() {
        calls.push("recovery-stop");
      },
      finish() {
        calls.push("recovery-finish");
      },
    },
    beforeUnloadGuard,
    target,
    () => calls.push("stop-clock"),
    () => calls.push("stop-avatar"),
    () => calls.push("stop-media"),
    {
      stop: async () => {
        calls.push("stop-recording");
      },
    },
    storeSelectedPracticeFocus,
    {},
  );
  await leaveRoom(focus);
  assert.deepEqual(calls, [
    "confirm:This report may not be saved. Leave this page anyway?",
  ]);
});

test("practice focus actions wait for saving, receipt and download independently", () => {
  const buttons = [{ disabled: false }, { disabled: false }];
  const state = {};
  const nodes = {
    report: { querySelectorAll: () => buttons },
  };
  const body = functionBody(
    read("web/interview.js"),
    "setPracticeFocusButtonsDisabled",
  );
  const setDisabled = new Function(
    "nodes",
    "state",
    `${body}\n}\nreturn setPracticeFocusButtonsDisabled;`,
  )(nodes, state);
  for (const receiving of [false, true]) {
    for (const downloading of [false, true]) {
      for (const saved of [false, true]) {
        Object.assign(state, {
          reportReceiving: receiving,
          downloadingReport: downloading,
          reportSaveComplete: saved,
        });
        setDisabled(false);
        assert.deepEqual(
          buttons.map((button) => button.disabled),
          [
            receiving || downloading || !saved,
            receiving || downloading || !saved,
          ],
        );
      }
    }
  }
  setDisabled(true);
  assert.ok(buttons.every((button) => button.disabled));
});

/// `saveHistory` from web/interview.js, compiled against the module state it
/// reads. `whiteboard` and `mode` are passed separately, as the page derives
/// them, so a test can set one without the other.
function loadSaveHistory({
  state,
  whiteboard = false,
  mode = "coding",
  code = "",
  id = "unused",
  unclear = [],
  saveReportHistory,
  persisted = reportIsPersisted,
  updateBeforeUnloadGuard = () => {},
}) {
  const body = functionBody(read("web/interview.js"), "saveHistory");
  return new Function(
    "state",
    "randomId",
    "problem",
    "whiteboard",
    "durationMin",
    "interviewLoop",
    "mode",
    "currentCode",
    "saveReportHistory",
    "reportIsPersisted",
    "updateBeforeUnloadGuard",
    "unclearLines",
    "withUnclearLines",
    `let saveGeneration = 0;\n${body}\n}\nreturn saveHistory;`,
  )(
    state,
    () => id,
    { page: "two-sum", title: "Two Sum", difficulty: "Easy" },
    whiteboard,
    45,
    "coding_only",
    mode,
    () => code,
    saveReportHistory,
    persisted,
    updateBeforeUnloadGuard,
    () => unclear,
    withUnclearLines,
  );
}

test("guest reports are not persisted across sessions", async () => {
  const guardStates = [];
  let saveCalls = 0;

  const state = {
    report: null,
    reportId: null,
    interviewId: "interview-guest",
    language: "javascript",
    reportPersisted: true,
    isGuest: true,
  };

  const saveReportHistory = async (_entry, options) => {
    saveCalls += 1;
    return options?.persistAccount === false
      ? { local: "skipped", account: "guest" }
      : { local: "saved", account: "saved" };
  };

  const updateBeforeUnloadGuard = () => {
    guardStates.push(state.reportPersisted);
  };

  const saveHistory = loadSaveHistory({
    state,
    mode: "whiteboard",
    saveReportHistory,
    updateBeforeUnloadGuard,
  });

  const result = await saveHistory({ summary: "guest report" });

  assert.deepEqual(result, { local: "skipped", account: "guest" });
  assert.equal(saveCalls, 1, "guest reports must skip persistent history");
  assert.equal(state.reportPersisted, false);
  assert.deepEqual(guardStates, [false]);
});

test("report is saved locally when session state is unavailable", async () => {
  const guardStates = [];
  let saveCalls = 0;

  const state = {
    report: null,
    reportId: null,
    interviewId: "interview-unknown-session",
    language: "javascript",
    reportPersisted: true,
  };

  const saveReportHistory = async () => {
    saveCalls += 1;
    return { local: "saved", account: "saved" };
  };

  const updateBeforeUnloadGuard = () => {
    guardStates.push(state.reportPersisted);
  };

  const saveHistory = loadSaveHistory({
    state,
    mode: "whiteboard",
    id: "attempt-unknown-session",
    saveReportHistory,
    persisted: () => true,
    updateBeforeUnloadGuard,
  });

  const result = await saveHistory({ summary: "temporary report" });

  assert.deepEqual(result, { local: "saved", account: "saved" });
  assert.equal(
    saveCalls,
    1,
    "local persistence must be attempted without session state",
  );
  assert.equal(state.reportPersisted, true);
  assert.deepEqual(guardStates, [false, true]);
});

test("only the newest overlapping report save can release the warning", async () => {
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
  const saveHistory = loadSaveHistory({
    state,
    mode: "whiteboard",
    saveReportHistory,
    updateBeforeUnloadGuard,
  });

  const provisional = saveHistory({ summary: "provisional" });
  const final = saveHistory({ summary: "final" });
  // saveHistory now awaits the session check before starting persistence.
  await new Promise((resolve) => setImmediate(resolve));
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

test("a saved attempt carries the final code, and a whiteboard none", async () => {
  const code = "function twoSum() {\n  return [0, 1];\n}\n";
  for (const [whiteboard, expected] of [
    [false, code],
    [true, null],
  ]) {
    const saved = [];
    const saveHistory = loadSaveHistory({
      state: { reportId: "attempt-1", language: "javascript" },
      whiteboard,
      code: whiteboard ? "" : code,
      saveReportHistory: async (entry) => {
        saved.push(entry);
        return { local: "saved", account: "skipped" };
      },
    });
    await saveHistory({ decision: "HIRE" });
    assert.equal(saved.length, 1);
    assert.equal(saved[0].code, expected);
  }
});

test("a long final code does not cost the attempt its unclear lines", async () => {
  // The unclear lines are cut to fit the account body, which never carries
  // the code, so code past that whole limit still leaves every line kept.
  const code = "x".repeat(MAX_REPORT_BODY_BYTES);
  const unclear = ["Line 1.", "Line 2."];
  const saved = [];
  const saveHistory = loadSaveHistory({
    state: { reportId: "attempt-1", language: "javascript" },
    code,
    unclear,
    saveReportHistory: async (entry) => {
      saved.push(entry);
      return { local: "saved", account: "skipped" };
    },
  });
  await saveHistory({ decision: "HIRE" });
  assert.equal(saved[0].code, code);
  assert.deepEqual(saved[0].unclearLines, unclear);
});

test("pagehide finalizes recovery before disconnecting the room", () => {
  const source = read("web/interview.js");
  const body = functionBody(source, "finalizeRecoveryOnPageHide");
  const calls = [];
  const handler = new Function(
    "state",
    "reportRecovery",
    "localRecording",
    "releaseStart",
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
    { dispose: () => calls.push("dispose") },
    () => calls.push("release"),
  );

  handler();
  assert.deepEqual(calls, ["dispose", "finish", "disconnect", "release"]);
});
