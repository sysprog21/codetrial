import assert from "node:assert/strict";
import test from "node:test";
import { loadInterview } from "./source.js";

function setup({
  isGuest = false,
  savedToAccount = false,
  possiblySavedToAccount = false,
  savedReportOwnerId = savedToAccount ? 7 : null,
  savedReportOwnerConflict = false,
  deleteReport,
}) {
  const calls = [];
  const alerts = [];
  const guardChanges = [];
  const beforeUnloadGuard = {
    setEnabled(enabled) {
      guardChanges.push(enabled);
    },
  };
  const state = {
    isGuest,
    savedToAccount,
    possiblySavedToAccount,
    savedReportOwnerId,
    savedReportOwnerConflict,
    reportId: "report-123",
    skipCleanupPending: false,
  };
  const nodes = {
    ending: { hidden: true },
    skipReport: { hidden: true, textContent: "" },
    endingDetail: { textContent: "" },
    leaveAnyway: { hidden: true, textContent: "Leave anyway" },
  };
  const window = {
    location: { href: "/interview" },
    alert(message) {
      alerts.push(message);
    },
  };
  const saveHistory = { pending: new Set() };

  const { finishSkippedReportCleanup } = loadInterview(
    ["finishSkippedReportCleanup"],
    {
      state,
      nodes,
      window,
      saveHistory,
      beforeUnloadGuard,
      deleteReport: async (...args) => {
        calls.push(args);
        return deleteReport(...args);
      },
      console: { warn() {} },
    },
  );

  return {
    finishSkippedReportCleanup,
    state,
    nodes,
    window,
    saveHistory,
    calls,
    alerts,
    guardChanges,
  };
}

test("render failure hides Skip without removing safe exits", () => {
  for (const offlineSummary of [false, true]) {
    const events = [];
    const state = {
      phase: "report",
      reportReceiving: true,
      reportUnreadable: false,
    };
    const nodes = {
      report: { hidden: false },
      ending: { hidden: true },
      skipReport: { hidden: false },
      endingDetail: { textContent: "" },
      leaveRoom: { hidden: true },
      forceReport: { hidden: true },
    };
    const { reportRenderFailed } = loadInterview(["reportRenderFailed"], {
      state,
      nodes,
      console: { warn: () => events.push("warn") },
      stopEndingClock: () => events.push("stop-clock"),
      stopEndingEscape: () => events.push("stop-escape"),
      providerUiState: (kind) => ({ message: kind }),
    });

    reportRenderFailed(new Error("render failed"), { offlineSummary });

    assert.equal(state.phase, "ending");
    assert.equal(state.reportUnreadable, true);
    assert.equal(state.reportReceiving, false);
    assert.equal(nodes.report.hidden, true);
    assert.equal(nodes.ending.hidden, false);
    assert.equal(nodes.skipReport.hidden, true);
    assert.equal(nodes.leaveRoom.hidden, false);
    assert.equal(nodes.forceReport.hidden, !offlineSummary);
    assert.equal(
      nodes.endingDetail.textContent,
      offlineSummary ? "report_unreadable" : "report_undrawable",
    );
    assert.deepEqual(events, ["warn", "stop-clock", "stop-escape"]);
  }
});

test("Skip requires an in-page confirmation and cancellation keeps waiting", async () => {
  const state = { phase: "ending" };
  const nodes = {
    endingDetail: { textContent: "Report generation continues." },
    skipReport: { hidden: false },
    confirmSkipReport: { hidden: true },
    cancelSkipReport: { hidden: true },
  };
  let skips = 0;
  const { requestSkipReport, cancelSkipReport, confirmSkipReport } =
    loadInterview(
      [
        "requestSkipReport",
        "cancelSkipReport",
        "confirmSkipReport",
        "dismissSkipConfirmation",
      ],
      {
        state,
        nodes,
        skipReportAndExit: async () => {
          skips++;
          state.phase = "skipped";
        },
      },
    );

  requestSkipReport();
  assert.equal(state.phase, "ending");
  assert.equal(skips, 0);
  assert.equal(nodes.skipReport.hidden, true);
  assert.equal(nodes.confirmSkipReport.hidden, false);
  assert.equal(nodes.cancelSkipReport.hidden, false);
  assert.match(nodes.endingDetail.textContent, /discarded/);

  cancelSkipReport();
  assert.equal(state.phase, "ending");
  assert.equal(skips, 0);
  assert.equal(nodes.skipReport.hidden, false);
  assert.equal(nodes.confirmSkipReport.hidden, true);
  assert.equal(nodes.cancelSkipReport.hidden, true);
  assert.equal(nodes.endingDetail.textContent, "Report generation continues.");

  requestSkipReport();
  await confirmSkipReport();
  assert.equal(skips, 1);
  assert.equal(state.phase, "skipped");
  assert.equal(nodes.confirmSkipReport.hidden, true);
  assert.equal(nodes.cancelSkipReport.hidden, true);
});

test("guest exits without attempting report deletion", async () => {
  const ctx = setup({
    isGuest: true,
    deleteReport: async () => {
      throw new Error("Guest must not call deleteReport");
    },
  });

  await ctx.finishSkippedReportCleanup();

  assert.deepEqual(ctx.calls, []);
  assert.equal(ctx.window.location.href, "/");
  assert.deepEqual(ctx.guardChanges, [false]);
  assert.equal(ctx.state.skipCleanupPending, false);
  assert.deepEqual(ctx.alerts, []);
});

test("failed account cleanup blocks exit and succeeds on retry", async () => {
  let attempts = 0;
  const ctx = setup({
    savedToAccount: true,
    deleteReport: async () => {
      attempts += 1;
      return attempts === 1 ? "failed" : "deleted";
    },
  });

  await ctx.finishSkippedReportCleanup();

  assert.equal(ctx.window.location.href, "/interview");
  assert.equal(ctx.nodes.skipReport.hidden, false);
  assert.equal(ctx.nodes.skipReport.textContent, "Retry cleanup and exit");
  assert.equal(ctx.state.skipCleanupPending, false);
  assert.equal(ctx.alerts.length, 1);
  assert.deepEqual(ctx.guardChanges, [true]);
  assert.match(ctx.nodes.endingDetail.textContent, /sign in again/);
  assert.deepEqual(ctx.calls, [
    [
      "report-123",
      { account: true, requireAccountDeletion: true, expectedOwnerId: 7 },
    ],
  ]);

  await ctx.finishSkippedReportCleanup();

  assert.equal(ctx.window.location.href, "/");
  assert.equal(ctx.state.skipCleanupPending, false);
  assert.deepEqual(ctx.calls, [
    [
      "report-123",
      { account: true, requireAccountDeletion: true, expectedOwnerId: 7 },
    ],
    [
      "report-123",
      { account: true, requireAccountDeletion: true, expectedOwnerId: 7 },
    ],
  ]);
  assert.deepEqual(ctx.guardChanges, [true, false]);
});

test("cleanup waits for an in-flight history save before deletion", async () => {
  let finishSave;
  const saving = new Promise((resolve) => {
    finishSave = resolve;
  });
  const ctx = setup({
    savedToAccount: true,
    deleteReport: async () => "deleted",
  });
  ctx.saveHistory.pending.add(saving);

  const cleanup = ctx.finishSkippedReportCleanup();

  await Promise.resolve();
  await Promise.resolve();

  assert.equal(ctx.calls.length, 0);
  assert.equal(ctx.window.location.href, "/interview");

  finishSave();
  await cleanup;

  assert.deepEqual(ctx.calls, [
    [
      "report-123",
      { account: true, requireAccountDeletion: true, expectedOwnerId: 7 },
    ],
  ]);
  assert.equal(ctx.window.location.href, "/");
});

test("uncertain account POST blocks signed-out Skip until deletion succeeds", async () => {
  let signedIn = false;
  const ctx = setup({
    // Account POST was sent, but its response was lost before ownerId arrived.
    // A different tab then signed out.
    isGuest: true,
    savedToAccount: false,
    possiblySavedToAccount: true,
    savedReportOwnerId: null,
    deleteReport: async (_id, options) => {
      assert.equal(options.requireAccountDeletion, true);
      return signedIn ? "deleted" : "failed";
    },
  });

  await ctx.finishSkippedReportCleanup();

  assert.equal(ctx.window.location.href, "/interview");
  assert.equal(ctx.state.possiblySavedToAccount, true);
  assert.equal(ctx.nodes.skipReport.textContent, "Retry cleanup and exit");
  assert.match(ctx.nodes.endingDetail.textContent, /sign in again/);
  assert.deepEqual(ctx.guardChanges, [true]);
  assert.deepEqual(ctx.calls, [
    ["report-123", { account: true, requireAccountDeletion: true }],
  ]);

  // The user signs in again and the account DELETE is confirmed.
  signedIn = true;
  ctx.state.isGuest = false;
  await ctx.finishSkippedReportCleanup();

  assert.equal(ctx.window.location.href, "/");
  assert.equal(ctx.calls.length, 2);
  assert.deepEqual(ctx.calls[1], [
    "report-123",
    { account: true, requireAccountDeletion: true },
  ]);
  assert.deepEqual(ctx.guardChanges, [true, false]);
});

test("former account session cannot abandon a saved account report", async () => {
  const ctx = setup({
    isGuest: true,
    savedToAccount: true,
    deleteReport: async () => "failed",
  });

  await ctx.finishSkippedReportCleanup();

  assert.deepEqual(ctx.calls, [
    [
      "report-123",
      { account: true, requireAccountDeletion: true, expectedOwnerId: 7 },
    ],
  ]);
  assert.equal(ctx.window.location.href, "/interview");
  assert.equal(ctx.nodes.skipReport.hidden, false);
  assert.equal(ctx.nodes.skipReport.textContent, "Retry cleanup and exit");
  assert.equal(ctx.alerts.length, 1);
  assert.deepEqual(ctx.guardChanges, [true]);
  assert.match(ctx.nodes.endingDetail.textContent, /sign in again/);
});

test("skip while board upload is pending prevents end publication", async () => {
  let finishUpload;
  const uploadPending = new Promise((resolve) => {
    finishUpload = resolve;
  });

  const published = [];
  const sent = [];
  const state = {
    phase: "live",
    reportReceiving: false,
    language: "en",
    connected: true,
    room: { disconnect: async () => {} },
    skipCleanupPending: false,
  };
  const nodes = {
    frameworkHint: { hidden: false },
    end: { disabled: false },
    ending: { hidden: true },
    forceReport: { hidden: true },
    leaveRoom: { hidden: false },
    retryReport: { hidden: false },
    skipReport: { hidden: false, textContent: "" },
  };

  const scope = {
    state,
    nodes,
    strict: false,
    exampleBoard: null,
    whiteboard: true,
    board: {
      settle: null,
      unsentSince: 0,
      model: { end() {} },
    },
    codePublishTimer: null,
    pendingLanguagePublish: null,
    frameworkHintTimer: null,
    roomParticipants: () => [],
    roomInterviewer: () => false,
    topics: { control: "control" },
    currentCode: () => "",
    endInterviewPayload: () => ({ type: "end_interview" }),
    skipReportPayload: () => ({ type: "skip_report" }),
    boardUploadsSettled: () => uploadPending,
    publish: async (topic, payload) => {
      published.push({ topic, payload });
    },
    sendData: async (topic, payload) => {
      sent.push({ topic, payload });
    },
    window: { confirm: () => true },
    reportRecovery: { stop() {} },
    beforeUnloadGuard: { setEnabled() {}, dispose() {} },
    recordReplay() {},
    flushReplay: async () => {},
    flushPendingBoardCheckpoints() {},
    recordBoardOps() {},
    queueBoardPublish() {},
    clearCandidateCases() {},
    startEndingClock() {},
    stopEndingEscape() {},
    stopEndingClock() {},
    stopAvatar() {},
    stopLocalMedia() {},
    finishSkippedReportCleanup: async () => {},
    showReport() {},
    setTimeout: () => 0,
  };

  const { endInterview, skipReportAndExit } = loadInterview(
    ["endInterview", "skipReportAndExit"],
    scope,
  );

  endInterview("candidate_ended");

  assert.equal(state.phase, "ending");
  assert.equal(published.length, 0);

  await skipReportAndExit();

  assert.equal(state.phase, "skipped");
  assert.equal(nodes.leaveRoom.hidden, true);
  assert.equal(nodes.retryReport.hidden, true);
  assert.equal(nodes.forceReport.hidden, true);
  assert.deepEqual(
    sent.map(({ payload }) => payload.type),
    ["skip_report"],
  );

  finishUpload();
  await Promise.resolve();
  await Promise.resolve();

  assert.deepEqual(published, []);
});

test("skip during reconnection publishes after reconnecting", async () => {
  const sent = [];
  const listeners = new Map();

  const room = {
    on(event, listener) {
      listeners.set(event, listener);
    },
    off(event, listener) {
      if (listeners.get(event) === listener) {
        listeners.delete(event);
      }
    },
    async disconnect() {},
  };

  const state = {
    phase: "ending",
    connected: false,
    room,
    skipCleanupPending: false,
  };

  const nodes = {
    leaveRoom: { hidden: false },
    retryReport: { hidden: false },
    forceReport: { hidden: false },
    skipReport: { hidden: false, textContent: "" },
  };

  const scope = {
    state,
    nodes,
    livekit: {
      RoomEvent: {
        Reconnected: "reconnected",
      },
    },
    topics: { control: "control" },
    skipReportPayload: () => ({ type: "skip_report" }),
    sendData: async (topic, payload) => {
      sent.push({ topic, payload });
    },
    window: {
      confirm: () => true,
      LivekitClient: {
        RoomEvent: { Reconnected: "reconnected" },
      },
    },
    reportRecovery: { stop() {} },
    stopEndingEscape() {},
    stopEndingClock() {},
    beforeUnloadGuard: { setEnabled() {}, dispose() {} },
    stopAvatar() {},
    stopLocalMedia() {},
    finishSkippedReportCleanup: async () => {},
    console: { warn() {} },
    setTimeout,
    clearTimeout,
  };

  const { skipReportAndExit } = loadInterview(["skipReportAndExit"], scope);

  const skipping = skipReportAndExit();

  assert.deepEqual(sent, []);
  assert.equal(nodes.leaveRoom.hidden, true);
  assert.equal(nodes.retryReport.hidden, true);
  assert.equal(nodes.forceReport.hidden, true);
  assert.equal(listeners.has("reconnected"), true);

  state.connected = true;
  listeners.get("reconnected")();

  await skipping;

  assert.deepEqual(sent, [
    { topic: "control", payload: { type: "skip_report" } },
  ]);
  assert.equal(listeners.has("reconnected"), false);
  assert.equal(state.room, null);
});

test("skip publication timeout preserves room and retries successfully", async () => {
  const listeners = new Map();
  const warnings = [];
  const alerts = [];
  const sent = [];
  const guard = [];
  let disconnected = 0;
  let cleanedUp = 0;
  let expirePublication;

  const room = {
    on(event, listener) {
      listeners.set(event, listener);
    },
    off(event, listener) {
      if (listeners.get(event) === listener) listeners.delete(event);
    },
    async disconnect() {
      disconnected++;
    },
  };

  const state = {
    phase: "ending",
    connected: false,
    room,
    skipCleanupPending: false,
  };

  const nodes = {
    ending: { hidden: true },
    skipReport: { hidden: true, textContent: "" },
    endingDetail: { textContent: "" },
    leaveRoom: { hidden: false },
    retryReport: { hidden: false },
    forceReport: { hidden: false },
  };

  const { skipReportAndExit } = loadInterview(["skipReportAndExit"], {
    state,
    nodes,
    topics: { control: "control" },
    skipReportPayload: () => ({ type: "skip_report" }),
    sendData: async (topic, payload) => {
      sent.push({ topic, payload });
    },
    window: {
      confirm: () => true,
      alert: (message) => alerts.push(message),
      LivekitClient: {
        RoomEvent: { Reconnected: "reconnected" },
      },
    },
    reportRecovery: { stop() {} },
    stopEndingEscape() {},
    stopEndingClock() {},
    beforeUnloadGuard: {
      setEnabled: (enabled) => guard.push(enabled),
      dispose() {},
    },
    stopAvatar() {},
    stopLocalMedia() {},
    finishSkippedReportCleanup: async () => {
      cleanedUp++;
    },
    console: {
      warn(...args) {
        warnings.push(args);
      },
    },
    setTimeout(callback, delay) {
      assert.equal(delay, 5000);
      expirePublication = callback;
      return 1;
    },
    clearTimeout() {},
  });

  const first = skipReportAndExit();
  assert.equal(listeners.has("reconnected"), true);
  expirePublication();
  await first;

  assert.equal(disconnected, 0);
  assert.equal(cleanedUp, 0);
  assert.equal(state.room, room);
  assert.equal(state.skipPublishPending, false);
  assert.equal(nodes.skipReport.textContent, "Retry Skip and exit");
  assert.equal(nodes.skipReport.hidden, false);
  assert.deepEqual(guard, [true, true]);
  assert.equal(nodes.leaveRoom.hidden, true);
  assert.equal(nodes.retryReport.hidden, true);
  assert.equal(nodes.forceReport.hidden, true);
  assert.equal(alerts.length, 1);
  assert.equal(warnings[0][0], "codetrial skip_report_publish_failed");
  assert.equal(listeners.has("reconnected"), false);

  state.connected = true;
  await skipReportAndExit();

  assert.deepEqual(sent, [
    { topic: "control", payload: { type: "skip_report" } },
  ]);
  assert.equal(disconnected, 1);
  assert.equal(cleanedUp, 1);
  assert.equal(state.room, null);
});

test("account deletion followed by local failure retries local cleanup", async () => {
  let attempts = 0;
  const ctx = setup({
    savedToAccount: true,
    deleteReport: async () => {
      attempts += 1;
      return attempts === 1 ? "account-deleted-local-failed" : "deleted";
    },
  });

  await ctx.finishSkippedReportCleanup();

  assert.equal(ctx.window.location.href, "/interview");
  assert.equal(ctx.state.savedToAccount, false);
  assert.equal(ctx.state.skipCleanupPending, false);
  assert.equal(ctx.nodes.skipReport.textContent, "Retry cleanup and exit");
  assert.deepEqual(ctx.calls, [
    [
      "report-123",
      {
        account: true,
        requireAccountDeletion: true,
        expectedOwnerId: 7,
      },
    ],
  ]);

  await ctx.finishSkippedReportCleanup();

  assert.equal(ctx.window.location.href, "/");
  assert.equal(ctx.state.skipCleanupPending, false);
  assert.deepEqual(ctx.calls, [
    [
      "report-123",
      {
        account: true,
        requireAccountDeletion: true,
        expectedOwnerId: 7,
      },
    ],
    [
      "report-123",
      {
        account: false,
        requireAccountDeletion: false,
        localOnly: true,
      },
    ],
  ]);
  assert.deepEqual(ctx.guardChanges, [true, false]);
});

test("signed-out retry finishes cleanup after account deletion", async () => {
  let attempts = 0;
  const ctx = setup({
    savedToAccount: true,
    deleteReport: async () => {
      attempts += 1;
      return attempts === 1 ? "account-deleted-local-failed" : "deleted";
    },
  });

  await ctx.finishSkippedReportCleanup();

  assert.equal(ctx.state.savedToAccount, false);
  assert.equal(ctx.state.skipAccountDeleted, true);
  assert.equal(ctx.window.location.href, "/interview");

  // Session changed after the account DELETE succeeded.
  ctx.state.isGuest = true;

  await ctx.finishSkippedReportCleanup();

  assert.equal(ctx.window.location.href, "/");
  assert.equal(ctx.calls.length, 2);
  assert.deepEqual(ctx.calls[1], [
    "report-123",
    {
      account: false,
      requireAccountDeletion: false,
      localOnly: true,
    },
  ]);
  assert.deepEqual(ctx.guardChanges, [true, false]);
});

test("Skip cleanup carries the saved owner to account deletion", async () => {
  const ctx = setup({
    savedToAccount: true,
    savedReportOwnerId: 7,
    deleteReport: async () => "deleted",
  });

  await ctx.finishSkippedReportCleanup();

  assert.equal(ctx.calls.length, 1);
  assert.equal(ctx.calls[0][0], "report-123");
  assert.equal(ctx.calls[0][1].expectedOwnerId, 7);
  assert.equal(ctx.window.location.href, "/");
});

test("Skip with unknown owner uses conservative account deletion", async () => {
  const ctx = setup({
    savedToAccount: true,
    savedReportOwnerId: null,
    deleteReport: async () => "deleted",
  });

  await ctx.finishSkippedReportCleanup();

  assert.equal(ctx.calls.length, 1);
  assert.equal(ctx.calls[0][0], "report-123");
  assert.equal(ctx.calls[0][1].account, true);
  assert.equal(ctx.calls[0][1].requireAccountDeletion, true);
  assert.equal(ctx.calls[0][1].expectedOwnerId, undefined);
  assert.equal(ctx.window.location.href, "/");
});

test("conflicting account saves expose escape without claiming deletion", async () => {
  const ctx = setup({
    savedToAccount: true,
    savedReportOwnerId: 7,
    savedReportOwnerConflict: true,
    deleteReport: async () => {
      throw new Error("Must not claim account deletion");
    },
  });

  ctx.window.confirm = () => {
    throw new Error("Blocking confirmation is forbidden");
  };

  await ctx.finishSkippedReportCleanup();

  assert.deepEqual(ctx.calls, []);
  assert.equal(ctx.window.location.href, "/interview");
  assert.equal(ctx.nodes.leaveAnyway.hidden, false);
  assert.equal(ctx.nodes.skipReport.textContent, "Retry cleanup and exit");
});

test("Leave anyway requires two clicks and never claims cleanup succeeded", async () => {
  const state = {
    phase: "skipped",
    room: null,
    skipCleanupPending: false,
    skipPublishPending: false,
  };
  const nodes = {
    leaveAnyway: { hidden: false, textContent: "Leave anyway" },
    endingDetail: { textContent: "Cleanup failed." },
  };
  const guard = [];
  const window = { location: { href: "/interview" } };
  const { leaveAnywayAfterSkip } = loadInterview(["leaveAnywayAfterSkip"], {
    state,
    nodes,
    window,
    beforeUnloadGuard: {
      setEnabled(value) {
        guard.push(value);
      },
    },
  });

  leaveAnywayAfterSkip();
  assert.equal(window.location.href, "/interview");
  assert.deepEqual(guard, []);
  assert.equal(nodes.leaveAnyway.textContent, "Confirm leave anyway");
  assert.match(nodes.endingDetail.textContent, /may remain saved/);

  leaveAnywayAfterSkip();
  assert.equal(window.location.href, "/");
  assert.deepEqual(guard, [false]);
});

test("rejected Skip publication keeps room until successful retry", async () => {
  const attempts = [];
  const warnings = [];
  const alerts = [];
  let disconnected = 0;
  let cleaned = 0;
  let confirms = 0;

  const room = {
    async disconnect() {
      disconnected++;
    },
  };
  const state = {
    phase: "ending",
    connected: true,
    room,
    skipCleanupPending: false,
  };
  const nodes = {
    ending: { hidden: true },
    skipReport: { hidden: true, textContent: "" },
    endingDetail: { textContent: "" },
    leaveRoom: { hidden: false },
    retryReport: { hidden: false },
    forceReport: { hidden: false },
  };
  const guard = [];

  const { skipReportAndExit } = loadInterview(["skipReportAndExit"], {
    state,
    nodes,
    topics: { control: "control" },
    skipReportPayload: () => ({ type: "skip_report" }),
    sendData: async (topic, payload) => {
      attempts.push({ topic, payload });
      if (attempts.length === 1) {
        throw new Error("Data publication rejected");
      }
    },
    window: {
      confirm: () => {
        confirms++;
        return true;
      },
      alert: (message) => alerts.push(message),
    },
    reportRecovery: { stop() {} },
    stopEndingEscape() {},
    stopEndingClock() {},
    beforeUnloadGuard: {
      setEnabled: (value) => guard.push(value),
      dispose() {},
    },
    stopAvatar() {},
    stopLocalMedia() {},
    finishSkippedReportCleanup: async () => {
      cleaned++;
    },
    console: {
      warn(...args) {
        warnings.push(args);
      },
    },
    setTimeout,
    clearTimeout,
  });

  await skipReportAndExit();

  assert.equal(attempts.length, 1);
  assert.equal(disconnected, 0);
  assert.equal(cleaned, 0);
  assert.equal(state.room, room);
  assert.equal(state.skipPublishPending, false);
  assert.equal(nodes.skipReport.textContent, "Retry Skip and exit");
  assert.equal(nodes.skipReport.hidden, false);
  assert.match(
    nodes.endingDetail.textContent,
    /report generation may continue/,
  );
  assert.equal(alerts.length, 1);
  assert.equal(warnings[0][0], "codetrial skip_report_publish_failed");
  assert.deepEqual(guard, [true, true]);
  assert.equal(nodes.leaveRoom.hidden, true);
  assert.equal(nodes.retryReport.hidden, true);
  assert.equal(nodes.forceReport.hidden, true);

  await skipReportAndExit();

  assert.deepEqual(attempts, [
    { topic: "control", payload: { type: "skip_report" } },
    { topic: "control", payload: { type: "skip_report" } },
  ]);
  assert.equal(
    confirms,
    0,
    "confirmed Skip and publication retry must not use blocking dialogs",
  );
  assert.equal(disconnected, 1);
  assert.equal(cleaned, 1);
  assert.equal(state.room, null);
  assert.equal(state.skipPublishPending, false);
});

test("late save cannot disable the unload guard during Skip", () => {
  const changes = [];
  const state = { phase: "skipped", reportPersisted: true };
  const beforeUnloadGuard = {
    setEnabled(value) {
      changes.push(value);
    },
  };

  const { updateBeforeUnloadGuard } = loadInterview(
    ["updateBeforeUnloadGuard"],
    {
      state,
      beforeUnloadGuard,
      shouldWarnBeforeUnload: () => false,
    },
  );

  beforeUnloadGuard.setEnabled(true);
  updateBeforeUnloadGuard();

  assert.deepEqual(changes, [true]);
});

test("Skip keeps unload protection until account cleanup finishes", async () => {
  let finishDeletion;
  const deletionPending = new Promise((resolve) => {
    finishDeletion = resolve;
  });

  const ctx = setup({
    savedToAccount: true,
    deleteReport: async () => {
      await deletionPending;
      return "deleted";
    },
  });

  // Skip has already confirmed and must protect the page during deletion.
  ctx.guardChanges.push(true);

  const cleanup = ctx.finishSkippedReportCleanup();
  await Promise.resolve();

  assert.equal(ctx.state.skipCleanupPending, true);
  assert.equal(ctx.window.location.href, "/interview");
  assert.deepEqual(ctx.guardChanges, [true]);

  finishDeletion();
  await cleanup;

  assert.equal(ctx.window.location.href, "/");
  assert.deepEqual(ctx.guardChanges, [true, false]);
});

test("Leave anyway disconnects after failed Skip publication", () => {
  let disconnected = 0;
  const room = {
    async disconnect() {
      disconnected++;
    },
  };
  const state = {
    phase: "skipped",
    room,
    connected: true,
    skipSendFailed: true,
    skipCleanupPending: false,
    skipPublishPending: false,
  };
  const nodes = {
    leaveAnyway: { hidden: false, textContent: "Leave anyway" },
    endingDetail: { textContent: "Could not send Skip." },
  };
  const guard = [];
  const window = { location: { href: "/interview" } };
  const { leaveAnywayAfterSkip } = loadInterview(["leaveAnywayAfterSkip"], {
    state,
    nodes,
    window,
    beforeUnloadGuard: {
      setEnabled(value) {
        guard.push(value);
      },
    },
    console,
  });

  leaveAnywayAfterSkip();
  assert.equal(disconnected, 0);
  assert.equal(window.location.href, "/interview");
  assert.equal(nodes.leaveAnyway.textContent, "Confirm leave anyway");
  assert.match(nodes.endingDetail.textContent, /may remain saved/);

  leaveAnywayAfterSkip();
  assert.equal(disconnected, 1);
  assert.equal(state.room, null);
  assert.equal(state.connected, false);
  assert.equal(window.location.href, "/");
  assert.deepEqual(guard, [false]);
});

test("Leave anyway cannot abandon an active room before Skip fails", () => {
  const state = {
    phase: "skipped",
    room: {
      async disconnect() {
        throw Error("must not disconnect");
      },
    },
    skipSendFailed: false,
    skipCleanupPending: false,
    skipPublishPending: false,
  };
  const window = { location: { href: "/interview" } };
  const { leaveAnywayAfterSkip } = loadInterview(["leaveAnywayAfterSkip"], {
    state,
    window,
    nodes: {
      leaveAnyway: { textContent: "Leave anyway" },
      endingDetail: { textContent: "" },
    },
    beforeUnloadGuard: {
      setEnabled() {
        throw Error("must not leave");
      },
    },
  });

  leaveAnywayAfterSkip();
  assert.equal(window.location.href, "/interview");
});
