import { test } from "node:test";
import assert from "node:assert/strict";
import { runInNewContext } from "node:vm";
import {
  LANGUAGE_KEY,
  MAX_REPORT_BODY_BYTES,
  createInterviewerReadiness,
  interviewerReconnectMessage,
  isAgent,
  roomInterviewer,
  providerUiState,
  withUnclearLines,
} from "../../web/lib.js";
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

test("untimed work is protected until its report is persisted", () => {
  const target = new CountingTarget();
  const guard = createBeforeUnloadGuard(target);
  const lifecycle = { endsAt: 0, workStarted: true, reportPersisted: false };
  guard.setEnabled(shouldWarnBeforeUnload(lifecycle));
  assert.equal(beforeUnloadEvent(target).defaultPrevented, true);
  lifecycle.reportPersisted = true;
  guard.setEnabled(shouldWarnBeforeUnload(lifecycle));
  assert.equal(beforeUnloadEvent(target).defaultPrevented, false);
});

test("interview wiring starts after preflight and only persistence releases it", () => {
  const page = read("web/interview.js");
  const init = functionBody(page, "init");
  const preflight = init.indexOf("const preflight = await runAudioCheck()");
  const connect = init.indexOf("await connect(preflight, presenting)");
  const deadline = init.indexOf("state.endsAt = startup.startedAt");
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

test("startup notices settle once and replacement notices cannot start a timer", async () => {
  let timeout;
  let cancelled = 0;
  let now = 10;
  const gate = createInterviewerReadiness({
    now: () => now,
    schedule: (fn) => {
      timeout = fn;
      return 1;
    },
    cancel: () => {
      cancelled += 1;
    },
  });
  let settled = false;
  gate.wait.then(() => {
    settled = true;
  });
  gate.receive({ type: "interviewer_state", reconnecting: false });
  gate.receive({ type: "other", startup: "ready" });
  await Promise.resolve();
  assert.equal(settled, false);
  now = 30;
  gate.receive({ type: "interviewer_state", startup: "ready" });
  now = 50;
  gate.receive({ type: "interviewer_state", startup: "ready" });
  timeout();
  assert.deepEqual(await gate.wait, { startup: "ready", startedAt: 30 });
  assert.equal(cancelled, 1);

  const missing = createInterviewerReadiness({
    schedule: (fn) => {
      timeout = fn;
      return 2;
    },
    cancel: () => {},
  });
  timeout();
  assert.equal((await missing.wait).startup, "failed");
});

test("the actual connection preserves refusal reasons and gates the init deadline on readiness", async () => {
  const page = read("web/interview.js");
  const init = functionBody(page, "init");
  const tail = init.slice(init.indexOf("const startup = await connect"));
  for (const outcome of [
    "ready",
    "failed",
    "offline",
    "refused",
    "strict-refused",
    "strict-timeout",
    "joined-error",
    "recording-failed",
    "ended-ready",
    "strict-failed",
  ]) {
    const strict = outcome.startsWith("strict-");
    const refused = outcome.includes("refused") || outcome === "strict-timeout";
    const serverReason = "The interview service is at capacity.";
    const expectedReason =
      outcome === "strict-timeout"
        ? "Strict Interview connection timed out. Return to the lobby or retry media preflight."
        : strict
          ? serverReason
          : "The live interview service is busy or rate limited.";
    const delivered = [];
    let gate;
    let ticks = 0;
    let intervals = 0;
    let banner = "";
    let mediaStopped = 0;
    let boardRefreshes = 0;
    const warnings = [];
    const context = {
      protected: false,
      state: { endsAt: 0, phase: "live", language: "python" },
      preferredLanguage: "python",
      languageChoiceCommitted: false,
      LANGUAGE_KEY,
      writeStored() {},
      board: { ready: false },
      exampleBoard: {
        refresh: () => {
          boardRefreshes += 1;
        },
      },
      preflight: {},
      presenting: false,
      createInterviewerReadiness: () => {
        gate = createInterviewerReadiness({
          now: () => 1000,
          schedule: () => 1,
          cancel: () => {},
        });
        return gate;
      },
      providerUiState,
      interviewerReconnectMessage,
      setAgentStateLabel() {},
      consentGiven: () => true,
      recordConsent: async () =>
        outcome === "recording-failed" ? "interview-one" : null,
      AbortSignal,
      TOKEN_REQUEST_TIMEOUT_MS: 30_000,
      fetch: async () => {
        if (outcome === "offline")
          throw new Error("Live session not configured");
        if (refused)
          return {
            ok: false,
            status: outcome === "strict-timeout" ? 504 : 503,
            json: async () => ({ error: serverReason, code: "capacity" }),
          };
        return {
          ok: true,
          json: async () => ({ durationMin: 45, problemPage: "two-sum" }),
        };
      },
      problem: { page: "two-sum" },
      strict,
      previousAssignment: () => null,
      rememberAssignment() {},
      loadProblem: async () => ({ page: "two-sum", starterCode: {} }),
      loadJudge: async () => null,
      renderProblem() {},
      initializeLanguages() {},
      initializeCandidateCases() {},
      updateRunAvailability() {},
      tabStorage: {},
      params: new URLSearchParams(),
      durationMin: 45,
      codingMinutes: 45,
      behavioralMinutes: 0,
      interviewLoop: "coding_only",
      mode: "coding",
      interviewProfile: {},
      interviewGrounding: null,
      editorOptions: { execution: true },
      nodes: {
        hideExamples: { checked: false },
        interviewerAccent: { value: "" },
        interviewerVoice: { value: "" },
        editor: { disabled: false },
        run: { disabled: false },
        audioStatus: { textContent: "" },
        audioCheck: { hidden: true, querySelectorAll: () => [] },
        audioLeave: {},
        audioJoin: { addEventListener() {} },
        end: { disabled: true },
        withdrawConsent: {},
        endingWithdrawConsent: {},
      },
      applyGrantedDuration() {},
      connectLiveKit: async () => {
        context.state.joinedRoom = true;
        if (outcome === "joined-error")
          throw new Error("Integrity publication failed");
      },
      recordingEnabled: outcome === "recording-failed",
      startRecording: () => new Promise(() => {}),
      recordReplay() {},
      recordStage() {},
      currentCode: () => "",
      exampleDrawingEnabled: true,
      candidateLedEnabled: false,
      whiteboard: false,
      stopPreflight() {},
      stopLocalMedia: () => {
        mediaStopped += 1;
      },
      setBanner: (_source, text) => {
        banner = text;
        delivered.push("banner");
      },
      addTranscript() {
        delivered.push("offline");
      },
      publishCode() {},
      updateBeforeUnloadGuard() {
        context.protected = shouldWarnBeforeUnload(context.state);
      },
      tickTimer: () => {
        ticks += 1;
      },
      setInterval: () => {
        intervals += 1;
      },
      console: { warn: (_message, error) => warnings.push(error?.message) },
    };
    const running = runInNewContext(
      `${functionBody(page, "abandonStart")}\n}\n${functionBody(page, "connect")}\n}\n(async () => {${tail}\n})()`,
      context,
    );
    await new Promise((resolve) => setImmediate(resolve));
    if (!refused && outcome !== "offline" && outcome !== "joined-error") {
      assert.equal(
        context.state.endsAt,
        0,
        `${outcome}: ${warnings.join(", ")}`,
      );
      assert.equal(ticks, 0);
      assert.equal(
        context.board.ready,
        false,
        `${outcome}: ${warnings.join(", ")}`,
      );
      assert.equal(boardRefreshes, 0);
      assert.equal(
        context.protected,
        true,
        `${outcome}: work is protected before readiness`,
      );
      if (outcome === "ended-ready") context.state.phase = "ending";
      gate.receive({
        type: "interviewer_state",
        startup: outcome.endsWith("ready") ? "ready" : "failed",
        reason: "billing",
      });
    }
    await running;
    if (outcome === "ended-ready") {
      assert.equal(context.state.endsAt, 0);
      assert.equal(intervals, 0);
      assert.equal(context.board.ready, false);
      assert.equal(banner, "");
      continue;
    }
    if (outcome === "strict-failed") {
      assert.match(banner, /billing/);
      assert.doesNotMatch(banner, /work the problem offline/);
      assert.equal(context.nodes.editor.disabled, true);
      assert.equal(context.nodes.run.disabled, true);
      assert.equal(context.state.endsAt, 0);
      continue;
    }
    assert.equal(
      context.protected,
      !strict,
      `${outcome}: editable work stays protected`,
    );
    if (refused) {
      assert.ok(banner.includes(expectedReason), `${outcome}: ${banner}`);
      assert.equal(mediaStopped, 1);
      assert.deepEqual(delivered, strict ? ["banner"] : ["banner", "offline"]);
      if (strict) {
        assert.equal(context.nodes.editor.disabled, true);
        assert.equal(context.nodes.run.disabled, true);
        assert.equal(context.nodes.audioStatus.textContent, expectedReason);
        assert.equal(context.state.endsAt, 0);
        assert.equal(ticks, 0);
        assert.equal(intervals, 0);
        assert.equal(context.board.ready, false);
        continue;
      }
    }
    assert.equal(context.board.ready, true);
    assert.equal(boardRefreshes, 1);
    if (outcome === "ready") {
      assert.equal(context.state.endsAt, 1000 + 45 * 60 * 1000);
      assert.equal(ticks, 1);
      assert.equal(intervals, 1);
    } else if (outcome === "offline" || outcome === "refused") {
      assert.ok(context.state.endsAt > 0);
      assert.equal(ticks, 1);
      assert.equal(intervals, 1);
    } else {
      assert.equal(context.state.endsAt, 0);
      assert.equal(ticks, 0);
      assert.equal(intervals, 0);
      assert.match(
        banner,
        outcome === "joined-error"
          ? /provider could not be reached/
          : /billing/,
      );
      assert.equal(mediaStopped, 1);
    }
  }
});

test("departure fails startup only after the last interviewer leaves", async () => {
  const source = functionBody(read("web/interview.js"), "connectLiveKit");
  const handlerSource =
    /room\.on\(livekit\.RoomEvent\.ParticipantDisconnected,\s*(\(participant\) => \{[\s\S]*?\n  \})\);/.exec(
      source,
    )?.[1];
  assert.ok(handlerSource, "the participant departure handler exists");
  const gate = createInterviewerReadiness({
    schedule: () => 1,
    cancel: () => {},
  });
  const state = {
    agentIdentity: "interviewer-primary",
    interviewerReadiness: gate,
  };
  let updates = 0;
  const primary = { identity: state.agentIdentity, kind: "AGENT" };
  const other = { identity: "interviewer-other", kind: "AGENT" };
  const room = {
    remoteParticipants: new Map([
      [primary.identity, primary],
      [other.identity, other],
    ]),
  };
  const handler = runInNewContext(`(${handlerSource})`, {
    state,
    isAgent,
    room,
    roomInterviewer,
    queueMicrotask,
    roomParticipants: () => [...room.remoteParticipants.values()],
    updateAgentState: () => {
      updates += 1;
    },
  });
  room.remoteParticipants.delete(primary.identity);
  handler(primary);
  handler({ identity: "candidate" });
  await Promise.resolve();
  assert.equal(gate.pending, true);
  assert.equal(
    roomInterviewer([...room.remoteParticipants.values()], state.agentIdentity),
    undefined,
    "waiting for readiness does not adopt another agent as the pinned report sender",
  );
  room.remoteParticipants.delete(other.identity);
  handler(other);
  assert.equal((await gate.wait).startup, "failed");
  assert.equal(updates, 3);
});

test("an agent departing before identity binding fails readiness, but a candidate does not", async () => {
  const source = functionBody(read("web/interview.js"), "connectLiveKit");
  const handlerSource =
    /room\.on\(livekit\.RoomEvent\.ParticipantDisconnected,\s*(\(participant\) => \{[\s\S]*?\n  \})\);/.exec(
      source,
    )?.[1];
  assert.ok(handlerSource);
  const gate = createInterviewerReadiness({ schedule: () => 1, cancel() {} });
  const agent = { identity: "agent", kind: "AGENT" };
  const duplicate = { identity: "duplicate", kind: "AGENT" };
  const room = {
    remoteParticipants: new Map([
      [agent.identity, agent],
      [duplicate.identity, duplicate],
    ]),
  };
  const handler = runInNewContext(`(${handlerSource})`, {
    state: { agentIdentity: null, interviewerReadiness: gate },
    isAgent,
    room,
    roomInterviewer,
    queueMicrotask,
    roomParticipants: () => [...room.remoteParticipants.values()],
    updateAgentState() {},
  });
  handler({ identity: "candidate", kind: "STANDARD" });
  await Promise.resolve();
  assert.equal(gate.pending, true);
  room.remoteParticipants.delete(duplicate.identity);
  handler(duplicate);
  await Promise.resolve();
  assert.equal(
    gate.pending,
    true,
    "removing a duplicate leaves the real interviewer waiting",
  );
  room.remoteParticipants.delete(agent.identity);
  handler(agent);
  assert.equal((await gate.wait).startup, "failed");
  assert.equal(gate.pending, false);
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
