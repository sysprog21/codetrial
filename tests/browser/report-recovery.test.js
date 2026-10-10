import { test } from "node:test";
import assert from "node:assert/strict";
import {
  createReportRecovery,
  reportRecoveryLimits,
} from "../../web/report-recovery.js";
import { retryReportPayload, sanitizeReport, topics } from "../../web/lib.js";

// Counted from the agent's answer, or from the click when no answer comes.
const retryWaitMs = reportRecoveryLimits.retryWaitSeconds * 1000;

function setup() {
  let now = 0;
  let next = 0;
  const scheduled = new Map();
  const events = [];
  const recovery = createReportRecovery({
    timers: {
      setTimeout(fn, ms) {
        scheduled.set(++next, { fn, at: now + ms });
        return next;
      },
      clearTimeout(id) {
        scheduled.delete(id);
      },
    },
    send: () => events.push(retryReportPayload()),
    offer: (ready) => events.push(ready),
    waiting: () => events.push("waiting"),
    finalize: (report) => events.push(report),
    keep: (report) => events.push({ kept: report }),
  });
  function advance(ms) {
    now += ms;
    for (const [id, timer] of [...scheduled]) {
      if (timer.at <= now && scheduled.has(id)) {
        scheduled.delete(id);
        timer.fn();
      }
    }
  }
  return { recovery, advance, events, scheduled };
}
const raw = {
  incomplete: true,
  summary: "503",
  reportRecovery: { expiresInSeconds: 300, retryAfterSeconds: 30 },
};

const accepted = { type: "report_retry", status: "accepted" };
const closed = { type: "report_retry", status: "closed" };
const finalized = (events) => events.filter((e) => e?.summary);

test("report recovery keeps the provisional failure the moment it arrives", () => {
  const { recovery, events } = setup();
  assert.equal(recovery.start(raw), true);
  assert.deepEqual(events[0], { kept: { incomplete: true, summary: "503" } });
  assert.equal(finalized(events).length, 0);
});

test("report recovery spends one retry after cooldown and times it from acceptance", () => {
  const { recovery, events, advance } = setup();
  recovery.start(raw);
  assert.equal(recovery.retry(), false);
  advance(30_000);
  assert.equal(recovery.retry(), true);
  assert.equal(recovery.retry(), false);
  // Queued behind a reconnect: the agent's answer restarts the wait.
  advance(100_000);
  assert.equal(finalized(events).length, 0);
  assert.equal(recovery.notice(accepted), true);
  advance(retryWaitMs - 1000);
  assert.equal(finalized(events).length, 0);
  advance(1000);
  assert.equal(events.filter((e) => e?.type === "retry_report").length, 1);
  assert.deepEqual(events.at(-1), { incomplete: true, summary: "503" });
  recovery.finish();
  assert.equal(finalized(events).length, 1);
});

test("the server can lengthen a retry's wait but not shorten it", () => {
  // A local model's regeneration runs under a longer deadline, which only the
  // server knows; a smaller or missing value leaves the floor in place.
  for (const [said, waited] of [
    [270, 270_000],
    [60, 145_000],
    [undefined, 145_000],
  ]) {
    globalThis.CODETRIAL_REPORT_RETRY_WAIT_SECONDS = said;
    try {
      const { recovery, events, advance } = setup();
      recovery.start(raw);
      advance(30_000);
      recovery.retry();
      recovery.notice(accepted);
      advance(waited - 1000);
      assert.equal(events.filter((e) => e?.summary).length, 0, `${said}`);
      advance(1000);
      recovery.finish();
      assert.equal(events.filter((e) => e?.summary).length, 1, `${said}`);
    } finally {
      delete globalThis.CODETRIAL_REPORT_RETRY_WAIT_SECONDS;
    }
  }
});

test("an accepted retry outlives the offer's own expiry", () => {
  const { recovery, events, advance } = setup();
  recovery.start(raw);
  advance(290_000);
  recovery.retry();
  recovery.notice(accepted);
  // A closing notice cannot cut short a generation the agent already began.
  assert.equal(recovery.notice(closed), false);
  advance(retryWaitMs - 1000);
  assert.equal(finalized(events).length, 0);
  advance(1000);
  assert.equal(finalized(events).length, 1);
});

test("an early answer arriving after the offer ran out finalizes instead", () => {
  const { recovery, advance, events } = setup();
  recovery.start(raw);
  advance(290_000);
  recovery.retry();
  // The offer's own expiry passes while the request is in flight.
  advance(30_000);
  assert.equal(finalized(events).length, 0);
  assert.equal(
    recovery.notice({
      type: "report_retry",
      status: "early",
      retryAfterSeconds: 2,
    }),
    true,
  );
  assert.equal(finalized(events).length, 1);
  advance(2000);
  assert.equal(recovery.retry(), false);
  assert.notEqual(events.at(-1), true);
});

test("an early answer re-offers the retry after the agent's wait", () => {
  const { recovery, events, advance } = setup();
  recovery.start(raw);
  advance(30_000);
  recovery.retry();
  assert.equal(
    recovery.notice({
      type: "report_retry",
      status: "early",
      retryAfterSeconds: 2,
    }),
    true,
  );
  assert.equal(events.at(-1), false);
  assert.equal(recovery.retry(), false);
  advance(2000);
  assert.equal(events.at(-1), true);
  assert.equal(recovery.retry(), true);
  assert.equal(events.filter((e) => e?.type === "retry_report").length, 2);
});

test("answers to a request never sent, or malformed ones, change nothing", () => {
  const { recovery, events, advance, scheduled } = setup();
  recovery.start(raw);
  const timers = scheduled.size;
  for (const message of [
    accepted,
    { type: "report_retry", status: "early", retryAfterSeconds: 2 },
    { type: "report_retry", status: "nonsense" },
    { type: "thinking_state", thinking: true },
  ])
    assert.equal(recovery.notice(message), false);
  advance(30_000);
  recovery.retry();
  assert.equal(
    recovery.notice({ type: "report_retry", status: "early" }),
    false,
  );
  // The offer's expiry and the retry's own wait; no early offer re-armed.
  assert.equal(scheduled.size, timers);
  assert.equal(finalized(events).length, 0);
});

test("the agent's closing notice, or its absence, finalizes exactly once", () => {
  for (const close of ["notice", "fallback", "departure"]) {
    const { recovery, advance, events } = setup();
    recovery.start(raw);
    advance(30_000);
    // A retry sent too late is answered by the close, not left spinning.
    if (close === "notice") {
      recovery.retry();
      assert.equal(recovery.notice(closed), true);
    }
    if (close === "departure") recovery.finish();
    advance(284_000);
    assert.equal(finalized(events).length, close === "fallback" ? 0 : 1);
    advance(1000);
    recovery.finish();
    recovery.notice(closed);
    assert.equal(finalized(events).length, 1);
  }
});

test("a retry whose answer is lost is bounded by its own wait, not the offer", () => {
  const { recovery, advance, events } = setup();
  recovery.start(raw);
  advance(290_000);
  assert.equal(recovery.retry(), true);
  // The offer would have run out here; the agent may still be generating.
  advance(30_000);
  assert.equal(finalized(events).length, 0);
  advance(retryWaitMs - 31_000);
  assert.equal(finalized(events).length, 0);
  advance(1000);
  assert.equal(finalized(events).length, 1);
});
test("a delivered final report cancels recovery without saving provisional failure", () => {
  const { recovery, advance, events, scheduled } = setup();
  recovery.start(raw);
  advance(30_000);
  recovery.retry();
  recovery.notice(accepted);
  recovery.stop();
  advance(500_000);
  assert.equal(events.filter((e) => e?.summary).length, 0);
  assert.equal(scheduled.size, 0);
});

test("duplicate provisional reports cannot extend the window or allow rerolls", () => {
  const { recovery, advance, events } = setup();
  recovery.start(raw);
  advance(20_000);
  assert.equal(recovery.start(raw), false);
  advance(10_000);
  recovery.retry();
  recovery.stop();
  assert.equal(recovery.start(raw), false);
  assert.equal(events.filter((e) => e?.type).length, 1);
});

test("untrusted recovery metadata cannot offer unbounded retention", () => {
  for (const data of [
    { ...raw, incomplete: false },
    {
      ...raw,
      reportRecovery: { expiresInSeconds: 301, retryAfterSeconds: 30 },
    },
    { ...raw, reportRecovery: { expiresInSeconds: 300, retryAfterSeconds: 0 } },
  ]) {
    const { recovery, scheduled } = setup();
    assert.equal(recovery.start(data), false);
    assert.equal(scheduled.size, 0);
  }
});

// Exercise the real page functions with a small DOM and LiveKit boundary.
// The recovery controller uses fake time; rendering and saving are observed.
import { functionBody, read } from "./source.js";

function pageHarness(phase = "live") {
  const source = read("web/interview.js");
  let now = 0;
  let next = 0;
  const scheduled = new Map();
  const events = [];
  const saves = [];
  const timers = {
    setTimeout(fn, ms) {
      scheduled.set(++next, { fn, at: now + ms });
      return next;
    },
    clearTimeout(id) {
      scheduled.delete(id);
    },
  };
  const room = { disconnect: async () => events.push("disconnect") };
  const nodes = Object.fromEntries(
    [
      "endingTitle",
      "frameworkHint",
      "retryReport",
      "editor",
      "ending",
      "forceReport",
      "leaveRoom",
      "endingDetail",
    ].map((key) => [key, {}]),
  );
  const spinner = {};
  nodes.ending.querySelector = () => spinner;
  nodes.endingTitle.textContent = "Jim is writing up your evaluation...";
  nodes.leaveRoom.textContent = "Taking too long? Leave the room";
  const scope = {
    state: { phase, room, connected: true },
    nodes,
    topics,
    sanitizeReport,
    createReportRecovery: (args) => createReportRecovery({ ...args, timers }),
    globalThis: { ...timers },
    setTimeout: timers.setTimeout,
    clearTimeout: timers.clearTimeout,
    frameworkHintTimer: 0,
    codePublishTimer: null,
    pendingLanguagePublish: null,
    endingEscape: [],
    REPORT_DELIVERY_GRACE_MS: 3000,
    deliveryGrace: 0,
    stopEndingEscape() {
      for (const id of scope.endingEscape) timers.clearTimeout(id);
      scope.endingEscape = [];
      timers.clearTimeout(scope.deliveryGrace);
      scope.deliveryGrace = 0;
    },
    stopAvatar: () => events.push("stop-avatar"),
    stopLocalMedia: () => events.push("stop-media"),
    startEndingClock: () => {},
    stopEndingClock: () => {},
    publish: (topic, payload) => {
      events.push(payload);
      return Promise.resolve();
    },
    retryReportPayload,
    recordReplay: (_, row) => events.push(row),
    flushReplay: async () => {},
    setBanner: () => {},
    providerUiState: () => ({ message: "failure" }),
    setLocalAudioEnabled: () => {},
    saveHistory: async (report = scope.state.report) => {
      events.push("save");
      saves.push(report);
    },
    renderReport: () => events.push("render"),
    renderReportSaveStatus: () => {},
    interviewLoop: "coding_only",
    roomParticipants: () => [],
    roomInterviewer: () => null,
    setAgentStateLabel: () => {},
    window: { location: {}, addEventListener() {} },
  };
  const recovery = source.slice(
    source.indexOf("const reportRecovery ="),
    source.indexOf("async function receiveReport"),
  );
  const names = [
    "receiveReport",
    "leaveRoom",
    "updateAgentState",
    "receiveControl",
  ];
  const bodies = names
    .map((name) => `${functionBody(source, name)}\n}`)
    .join("\n");
  const page = new Function(
    "scope",
    `with(scope) { ${recovery}\n${bodies}\nreturn {reportRecovery, finalizeRecoveryOnPageHide, ${names.join(",")}}; }`,
  )(new Proxy(scope, { has: (target, key) => key in target }));
  function advance(ms) {
    now += ms;
    for (const [id, timer] of [...scheduled]) {
      if (timer.at <= now && scheduled.has(id)) {
        scheduled.delete(id);
        timer.fn();
      }
    }
  }
  return { ...page, room, scope, events, saves, advance };
}
const packet = (value) => new TextEncoder().encode(JSON.stringify(value));
const settle = () => new Promise((resolve) => setImmediate(resolve));

const saveCount = (page) =>
  page.events.filter((event) => event === "save").length;
const control = (value) => packet(value);

test("page saves the transient failure on arrival, then once more on leaving", async () => {
  for (const phase of ["live", "ending"]) {
    const page = pageHarness(phase);
    await page.receiveReport(page.room, packet(raw));
    assert.equal(page.scope.state.phase, "report_recovery");
    assert.ok(page.events.includes("stop-media"));
    assert.equal(saveCount(page), 1);
    assert.equal(page.saves[0].summary, "503");
    assert.equal(page.saves[0].incomplete, true);
    assert.ok(!page.events.includes("disconnect"));
    page.leaveRoom();
    await settle();
    assert.equal(page.scope.state.phase, "report");
    assert.equal(saveCount(page), 2);
    assert.equal(
      page.events.filter((event) => event === "disconnect").length,
      1,
    );
  }
});

test("page finalizes a regeneration once, cancelling provisional timers", async () => {
  const page = pageHarness();
  await page.receiveReport(page.room, packet(raw));
  page.advance(30_000);
  assert.equal(page.reportRecovery.retry(), true);
  page.receiveControl(control({ type: "report_retry", status: "accepted" }));
  const final = { incomplete: true, summary: "final outcome" };
  await page.receiveReport(page.room, packet(final));
  await page.receiveReport(page.room, packet(final));
  page.advance(500_000);
  await settle();
  assert.equal(saveCount(page), 2);
  assert.equal(page.saves.at(-1).summary, "final outcome");
  assert.equal(
    page.events.filter((event) => event?.state === "ended").length,
    1,
  );
  assert.equal(
    page.events.filter((event) => event?.state === "rounds_final").length,
    1,
  );
});

test("the agent's closing notice finalizes the page through its control topic", async () => {
  const page = pageHarness();
  await page.receiveReport(page.room, packet(raw));
  page.advance(30_000);
  page.reportRecovery.retry();
  page.receiveControl(control({ type: "report_retry", status: "closed" }));
  await settle();
  assert.equal(page.scope.state.phase, "report");
  assert.equal(saveCount(page), 2);
});

test("a report packet that parses to null is still delivered", async () => {
  const page = pageHarness();
  await page.receiveReport(page.room, packet(null));
  assert.equal(page.scope.state.phase, "report");
  assert.ok(page.events.includes("render"));
  assert.equal(saveCount(page), 1);
});

test("a regenerated report that fails to render does not keep the recovery overlay", async () => {
  const page = pageHarness();
  await page.receiveReport(page.room, packet(raw));
  page.advance(30_000);
  page.reportRecovery.retry();
  assert.equal(
    page.scope.nodes.endingTitle.textContent,
    "Retrying your evaluation",
  );
  page.scope.renderReport = () => {
    throw new Error("undrawable");
  };
  page.scope.reportRenderFailed = () => page.events.push("render-failed");
  page.scope.console = { warn() {} };
  await page.receiveReport(
    page.room,
    packet({ incomplete: true, summary: "final outcome" }),
  );
  assert.ok(page.events.includes("render-failed"));
  const { nodes } = page.scope;
  assert.equal(
    nodes.endingTitle.textContent,
    "Jim is writing up your evaluation...",
  );
  assert.equal(nodes.leaveRoom.textContent, "Taking too long? Leave the room");
  assert.equal(nodes.ending.querySelector(".spinner").hidden, false);
  assert.equal(nodes.retryReport.hidden, true);
});

test("a refused offer in a live interview writes its end frame once", async () => {
  const page = pageHarness();
  await page.receiveReport(
    page.room,
    packet({
      ...raw,
      endReason: "time_up",
      reportRecovery: { expiresInSeconds: 999, retryAfterSeconds: 30 },
    }),
  );
  const ended = page.events.filter((event) => event?.state === "ended");
  assert.equal(page.scope.state.phase, "report");
  assert.equal(ended.length, 1);
  assert.equal(ended[0].reason, "time_up");
});

test("agent departure at cooldown boundary still finalizes the original failure", async () => {
  const page = pageHarness();
  await page.receiveReport(page.room, packet(raw));
  page.advance(28_000);
  page.updateAgentState();
  page.advance(2000);
  assert.equal(page.scope.nodes.retryReport.disabled, false);
  page.advance(1000);
  await settle();
  assert.equal(page.scope.state.phase, "report");
  assert.equal(saveCount(page), 2);
});

test("terminal room disconnect finalizes recovery before clearing the room", async () => {
  const page = pageHarness();
  await page.receiveReport(page.room, packet(raw));
  const source = read("web/interview.js");
  const start =
    source.indexOf("room.on(livekit.RoomEvent.Disconnected, () => {") +
    "room.on(livekit.RoomEvent.Disconnected, () => {".length;
  const body = source.slice(start, source.indexOf("\n  });", start));
  page.scope.reportRecovery = page.reportRecovery;
  page.scope.console = { warn() {} };
  const disconnect = new Function(
    "scope",
    `with(scope) { return () => { ${body} }; }`,
  )(new Proxy(page.scope, { has: (target, key) => key in target }));
  disconnect();
  await settle();
  assert.equal(page.scope.state.room, null);
  assert.equal(page.scope.state.connected, false);
  assert.equal(page.scope.state.phase, "report");
  assert.equal(saveCount(page), 2);
  page.advance(500_000);
  assert.equal(saveCount(page), 2);
});

test("a finalization with no room left still renders and saves", async () => {
  const page = pageHarness();
  await page.receiveReport(page.room, packet(raw));
  page.scope.state.room = null;
  page.reportRecovery.finish();
  await settle();
  assert.equal(page.scope.state.phase, "report");
  assert.ok(page.events.includes("render"));
  assert.equal(saveCount(page), 2);
});

test("page exit finalizes once, after the arrival save already landed", async () => {
  const page = pageHarness();
  await page.receiveReport(page.room, packet(raw));
  assert.equal(saveCount(page), 1);
  page.finalizeRecoveryOnPageHide();
  assert.equal(saveCount(page), 2);
  page.finalizeRecoveryOnPageHide();
  page.advance(500_000);
  await settle();
  assert.equal(saveCount(page), 2);
});

test("a returning agent cancels failure finalization during delivery grace", async () => {
  const page = pageHarness();
  await page.receiveReport(page.room, packet(raw));
  page.advance(28_000);
  page.updateAgentState();
  page.updateAgentState();
  page.scope.roomInterviewer = () => ({});
  page.advance(3000);
  await settle();
  assert.equal(page.scope.state.phase, "report_recovery");
  assert.equal(saveCount(page), 1);
});

test("quota recovery waits for the provider cooldown and hides framework hints", async () => {
  const page = pageHarness();
  await page.receiveReport(
    page.room,
    packet({
      ...raw,
      reportRecovery: { expiresInSeconds: 300, retryAfterSeconds: 60 },
    }),
  );
  assert.equal(page.scope.nodes.frameworkHint.hidden, true);
  assert.match(page.scope.nodes.endingDetail.textContent, /60 seconds/);
  page.advance(30_000);
  assert.equal(page.reportRecovery.retry(), false);
  page.advance(30_000);
  assert.equal(page.reportRecovery.retry(), true);
});

test("a refused report is offered under its own cause, and anything else is an outage", () => {
  const causes = (reportRecovery) => {
    const seen = [];
    const recovery = createReportRecovery({
      timers: { setTimeout: () => 1, clearTimeout() {} },
      send() {},
      offer: (ready, seconds, cause) => seen.push(cause),
      waiting() {},
      finalize() {},
    });
    recovery.start({
      ...raw,
      reportRecovery: { ...raw.reportRecovery, ...reportRecovery },
    });
    return seen;
  };
  assert.deepEqual(causes({ cause: "schema" }), ["schema"]);
  assert.deepEqual(causes({ cause: "unavailable" }), ["unavailable"]);
  // Untrusted metadata: a missing or unknown cause gets the ordinary wording.
  assert.deepEqual(causes({}), ["unavailable"]);
  assert.deepEqual(causes({ cause: "<b>forged</b>" }), ["unavailable"]);
});
