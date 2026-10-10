import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { launchChromium, read, startStaticServer } from "./source.js";

let browser, server, base;
before(async () => {
  browser = await launchChromium();
  ({ server, base } = await startStaticServer());
});
after(async () => {
  await browser?.close();
  await new Promise((resolve) => server.close(resolve));
});

const SITE = "https://teacher.github.io/course";
const ASSIGNMENT = `/t/classroom/delimiter-closer?site=${encodeURIComponent(SITE)}&version=7`;

const task = {
  setId: "classroom",
  version: 7,
  taskId: "delimiter-closer",
  title: "Delimiter exercise",
  brief: ["Explain and complete this supplied loop."],
  contract: "Keep the surrounding contract.",
  languages: ["python"],
  starterCode: { python: "# TASK_COMPLETE_CLOSER\npass\n" },
  completionTargets: [
    {
      id: "closer",
      language: "python",
      marker: "TASK_COMPLETE_CLOSER",
      goal: "Complete the closer branch",
    },
  ],
  understandingChecks: [{ id: "trace", question: "Trace an input" }],
  publicCases: {},
  rules: {
    durationMin: 15,
    lookAwaySeconds: 6,
    closesAt: null,
    rulesNote: "Bring paper.",
  },
  setupTimeoutSeconds: 120,
  reviewDeliverySeconds: 300,
};

const result = (outcome, cause, taskId = "delimiter-closer") => ({
  assessmentMode: "task",
  taskContract: { package: 1, prompt: 1, wire: 1, reportSchema: 1, rubric: 1 },
  taskAssessment: {
    setId: "classroom",
    taskId,
    outcome: { outcome, cause, at: 75, durationMs: 0 },
    finalRevisionId: "rev-final",
    finalCode: "print('frozen work')\n",
  },
});

// The LiveKit room, answered by a fake task agent that keeps a state snapshot
// and replies the way the runtime does.
const sdk = `
export const RoomEvent = {TrackSubscribed: "track", DataReceived: "data", Disconnected:"disconnected", ParticipantDisconnected: "participantDisconnected"};
export async function createLocalAudioTrack() {
 const track = {stops: 0, stop(){this.stops++;}};
 globalThis.publishedMicrophone = track;
 return track;
}
export class Room {
 constructor() {
  this.handlers = {};
  const owner = this;
  this.localParticipant = {async publishTrack(){}, async publishData(payload, options) {
   const message = JSON.parse(new TextDecoder().decode(payload));
   const h = globalThis.taskHarness;
   h.messages.push({topic: options.topic, message});
   if (options.topic === "task_revision" && h.failNextRevisionSend) { h.failNextRevisionSend = false; h.messages.pop(); throw new Error("data channel closed"); }
   if (options.topic === "task_revision" && h.holdCapture) { h.holdCapture = false; h.release = () => { h.state.acknowledgedCaptureRequestId = message.requestId; h.publish(); }; return; }
   if (options.topic === "task_revision" && h.rejectNextCapture) { h.rejectNextCapture = false; h.publish("task_error", {version: 1, code: "action_rejected", requestId: message.requestId, retryable: true, message: "Wait for the acknowledged task state before retrying."}); return; }
   if (options.topic === "task_start" && h.holdStartPublish) { h.holdStartPublish = false; return new Promise(resolve => { h.releaseStartPublish = resolve; }); }
   if (options.topic === "task_start" && h.rejectStartSend) { throw new Error("Start publish failed"); }
   if (options.topic === "task_connected" && globalThis.loseTaskAfterReady) { h.publish(); h.disconnect(); return; }
   if (options.topic === "task_end" && h.failNextEnd) { h.failNextEnd = false; h.messages.pop(); throw new Error("data channel closed"); }
   if (options.topic === "task_start" && h.hangStart) return new Promise(() => {});
   if (options.topic === "task_start" && h.holdStart) { h.holdStart = false; h.releaseStart = () => { h.state.phase = "work"; h.state.deadlineAt = new Date(Date.now() + 900000).toISOString(); h.publish(); }; return; }
   if (options.topic === "task_start") { h.state.phase = "work"; h.state.deadlineAt = new Date(Date.now() + 900000).toISOString(); }
   if (options.topic === "task_end") { h.state.phase = "feedback"; h.state.outcome = {outcome: message.outcome, cause: message.cause, at: 75, durationMs: message.durationMs}; }
   if (options.topic === "task_revision") h.state.acknowledgedCaptureRequestId = message.requestId;
   if (options.topic === "task_action" && message.action === "finish") { h.state.phase = "wrap-up"; h.state.finalRevisionId = message.revisionId; }
   h.publish();
  }};
  globalThis.oldTaskHarness = globalThis.taskHarness;
  globalThis.taskHarness = {messages: [], disconnect() { owner.handlers.disconnected?.(); }, agentLeaves() { owner.handlers.participantDisconnected?.({identity: "task-agent-room-1"}); }, state: ${JSON.stringify(JSON.parse(read("tests/fixtures/task-state.json")).cases[0].payload)}, publish(topic = "task_state", value = null) { this.state.seq++; owner.handlers.data?.(new TextEncoder().encode(JSON.stringify(value ?? this.state)), {identity: "task-agent-room-1"}, null, topic); }};
 }
 on(topic, callback) {this.handlers[topic] = callback;}
 async connect() { if (globalThis.snapshotOnConnect) globalThis.taskHarness.publish(); }
 async disconnect() { this.handlers.disconnected?.(); }
}
`;

// The face detector, answering whatever sample the test sets; a face
// looking at the screen until told otherwise.
const detector = `
export const FACE_SAMPLE_GAP_MS = 5000;
export const FACE_SAMPLE_UNAVAILABLE = {available: false, count: 0, confidence: 0};
export async function createFacePresenceDetector() {
  return {available: true, async detect() { return globalThis.faceHang ? new Promise(() => {}) : globalThis.faceSample; }, close() {}};
}
`;

async function pageFor(
  t,
  {
    signedIn = true,
    deviceLogin = true,
    path = ASSIGNMENT,
    denied = false,
    completedReview = null,
    reviewResponses = null,
    reviewMissing = false,
    runnerOutage = false,
    learnerError = false,
    taskOverrides = {},
  } = {},
) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await browser.newPage();
  await page.clock.install();
  await page.clock.resume();
  let unlocked = false;
  let session = signedIn;
  const admissions = [];
  const reviewRequests = [];
  const unlocks = [];
  await page.addInitScript(
    ({ denied }) => {
      globalThis.faceSample = {
        available: true,
        count: 1,
        confidence: 1,
        face: {
          box: [0.5, 0.5, 0.2, 0.3],
          landmarks: [
            [0.45, 0.4],
            [0.55, 0.4],
            [0.5, 0.45],
            [0.5, 0.5],
          ],
        },
      };
      let fullscreen = false;
      Object.defineProperty(document, "fullscreenElement", {
        get: () => (fullscreen ? document.getElementById("workspace") : null),
        configurable: true,
      });
      Object.defineProperty(document, "fullscreenEnabled", {
        get: () => true,
        configurable: true,
      });
      Element.prototype.requestFullscreen = function () {
        if (denied) return Promise.reject(new Error("denied"));
        fullscreen = true;
        document.dispatchEvent(new Event("fullscreenchange"));
        return Promise.resolve();
      };
      document.exitFullscreen = async () => {
        fullscreen = false;
        document.dispatchEvent(new Event("fullscreenchange"));
      };
      const canvas = document.createElement("canvas");
      globalThis.cameraStream = canvas.captureStream();
      navigator.mediaDevices.getUserMedia = async () => globalThis.cameraStream;
      HTMLMediaElement.prototype.play = async () => {};
      Object.defineProperty(HTMLMediaElement.prototype, "readyState", {
        get: () => 4,
        configurable: true,
      });
      globalThis.createImageBitmap = async () => ({ close() {} });
    },
    { denied },
  );
  await page.route(`**/t/classroom/*`, (route) =>
    route.fulfill({ contentType: "text/html", body: read("web/task.html") }),
  );
  await page.route("**/vendor/livekit-client.js", (route) =>
    route.fulfill({ contentType: "text/javascript", body: sdk }),
  );
  await page.route("**/face-presence.js", (route) =>
    route.fulfill({ contentType: "text/javascript", body: detector }),
  );
  await page.route("**/runners.js", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body: runnerOutage
        ? "export async function runBrowserTests(){return {passed:0,total:2,cases:[],runnerUnavailable:true,setupError:'Python runtime unavailable'};}"
        : learnerError
          ? "export async function runBrowserTests(){return {passed:0,total:2,cases:[],setupError:'SyntaxError: invalid syntax'};}"
          : "export async function runBrowserTests(){return {passed:1,total:2,cases:[{pass:true,label:'ordinary'},{pass:false,label:'boundary'}]};}",
    }),
  );
  let polls = 0;
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    let status = 200,
      body;
    if (url.pathname.startsWith("/api/task-reviews/")) {
      reviewRequests.push(url.pathname);
      assert.equal(url.pathname, "/api/task-reviews/room-1");
      status = reviewMissing ? 404 : 200;
      body = reviewMissing
        ? { code: "result_unavailable", message: "No retained result" }
        : (reviewResponses?.shift() ?? completedReview ?? { pending: true });
    } else if (url.pathname === "/api/session")
      body = { signedIn: session, deviceLogin };
    else if (url.pathname === "/api/github/device")
      body = {
        userCode: "WDJB-MJHT",
        verificationUri: "https://github.com/login/device",
        interval: 5,
        expiresIn: 900,
      };
    else if (url.pathname === "/api/github/device/poll") {
      polls += 1;
      session = polls > 1;
      status = session ? 200 : 202;
      body = session ? { signedIn: true } : { pending: true };
    } else if (url.pathname.endsWith("/attempts")) {
      // The task is named by the route, not the body.
      admissions.push({
        ...route.request().postDataJSON(),
        taskId: decodeURIComponent(url.pathname.split("/").at(-2)),
      });
      body = {
        token: "test",
        serverUrl: "wss://test.invalid",
        roomName: "room-1",
      };
    } else if (url.pathname.endsWith("/unlock")) {
      const sent = route.request().postDataJSON();
      unlocks.push(sent);
      if (sent.pin !== "123456") {
        status = 403;
        body = { code: "invalid_pin", message: "The PIN is incorrect." };
      } else {
        unlocked = true;
        body = {
          setId: "classroom",
          version: 7,
          tasks: [
            { id: task.taskId, title: task.title },
            { id: "second-task", title: "Second task" },
          ],
        };
      }
    } else if (!unlocked) {
      status = 403;
      body = { code: "unlock_required", message: "Enter the PIN." };
    } else
      body = {
        ...task,
        ...taskOverrides,
        taskId: url.pathname.split("/").at(-1),
      };
    await route.fulfill({
      status,
      contentType: "application/json",
      body: JSON.stringify(body),
    });
  });
  await page.goto(`${base}${path}`);
  return { page, admissions, unlocks, reviewRequests };
}

async function unlock(page) {
  await page.locator("#pin-form").waitFor({ state: "visible" });
  await page.fill("#pin", "123456");
  await page.locator("#pin-form button").click();
  await page.locator("#prepare").waitFor({ state: "visible" });
  await page.waitForFunction(() => {
    const title = document.getElementById("prepare-title").textContent;
    return title && title !== "Loading assignment…";
  });
}

/// The two calibration phases: reading, then typing the given line.
async function calibrationPhases(page) {
  await page.clock.runFor(10500);
  await page.locator("#calibration-line").waitFor({ state: "visible" });
  await page.fill("#calibration-line", "for item in items: total += item");
  await page.clock.runFor(10500);
}

/// Rules, devices, calibration and connection: everything before Start.
async function prepare(page, alreadyUnlocked = false) {
  if (!alreadyUnlocked) await unlock(page);
  await page.check("#acknowledge");
  await page.click("#devices");
  await page.waitForFunction(
    () => !document.getElementById("calibrate").disabled,
  );
  await page.click("#calibrate");
  await calibrationPhases(page);
  await page.waitForFunction(() => !document.getElementById("start").disabled);
}

async function start(page) {
  await prepare(page);
  await page.click("#start");
  await page.waitForFunction(() => !document.getElementById("code").disabled);
}

const ended = (page) =>
  page.evaluate(() =>
    globalThis.taskHarness.messages
      .filter((row) => row.topic === "task_end")
      .map((row) => [row.message.outcome, row.message.cause]),
  );

test("device sign-in, then the PIN unlocks the set at the linked site and version", async (t) => {
  const ctx = await pageFor(t, { signedIn: false });
  if (!ctx) return;
  const { page, unlocks } = ctx;
  try {
    await page.locator("#signin").waitFor({ state: "visible" });
    await page.click("#device-start");
    await page.locator("#device-code").waitFor({ state: "visible" });
    assert.equal(
      await page.locator("#device-user-code").textContent(),
      "WDJB-MJHT",
    );
    await page.clock.runFor(11000);
    await page.locator("#pin-form").waitFor({ state: "visible" });
    await page.fill("#pin", "000000");
    await page.locator("#pin-form button").click();
    await page.locator("#message").waitFor({ state: "visible" });
    assert.match(await page.locator("#message-text").textContent(), /PIN/);
    await page.fill("#pin", "123456");
    await page.locator("#pin-form button").click();
    await page.locator("#prepare").waitFor({ state: "visible" });
    assert.deepEqual(unlocks.at(-1), { site: SITE, version: 7, pin: "123456" });
  } finally {
    await page.close();
  }
});

test("a task opened without its assignment link says how to open it", async (t) => {
  const ctx = await pageFor(t, { path: "/t/classroom/delimiter-closer" });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await page.locator("#message").waitFor({ state: "visible" });
    assert.match(
      await page.locator("#message-text").textContent(),
      /assignment link/,
    );
    assert.equal(await page.locator("#pin-form").isVisible(), false);
  } finally {
    await page.close();
  }
});

test("a task's languages are chosen before the room joins, and the editor follows the choice", async (t) => {
  const starters = {
    javascript: "function closer(s) {\n  // TASK_COMPLETE_CLOSER\n}\n",
    cpp: "bool closer(string s) {\n  // TASK_COMPLETE_CLOSER\n}\n",
    python: task.starterCode.python,
  };
  const ctx = await pageFor(t, {
    taskOverrides: {
      languages: ["javascript", "cpp", "python"],
      starterCode: starters,
      completionTargets: [
        ...task.completionTargets,
        {
          id: "closer-js",
          language: "javascript",
          marker: "TASK_COMPLETE_CLOSER",
          goal: "Complete the JavaScript closer",
        },
      ],
    },
  });
  if (!ctx) return;
  const { page, admissions } = ctx;
  const editor = () =>
    page.evaluate(() => ({
      code: document.getElementById("code").value,
      label: document.getElementById("code-label").textContent,
      targets: [...document.getElementById("target").options].map(
        (option) => option.textContent,
      ),
      compiled: !document.getElementById("compiled-note").hidden,
    }));
  try {
    await unlock(page);
    assert.equal(await page.locator("#language-choice").isVisible(), true);
    // C++ is offered, so the page says where its runs go before any choice.
    assert.equal(await page.locator("#language-note").isVisible(), true);
    // The instructor's first language is the one offered first.
    assert.deepEqual(await editor(), {
      code: starters.javascript,
      label: "JavaScript implementation",
      targets: ["Whole task", "Complete the JavaScript closer"],
      compiled: false,
    });
    await page.selectOption("#language", "python");
    assert.deepEqual(await editor(), {
      code: starters.python,
      label: "Python implementation",
      targets: ["Whole task", "Complete the closer branch"],
      compiled: false,
    });
    await page.selectOption("#language", "cpp");
    assert.deepEqual(await editor(), {
      code: starters.cpp,
      label: "C++ implementation",
      targets: ["Whole task"],
      compiled: true,
    });
    await prepare(page, true);
    assert.equal(admissions.at(-1).language, "cpp");
    // The room was admitted for C++; the choice holds from here.
    assert.equal(await page.locator("#language").isDisabled(), true);
    await page.click("#start");
    await page.waitForFunction(() => !document.getElementById("code").disabled);
    assert.equal((await editor()).label, "C++ implementation");
  } finally {
    await page.close();
  }
});

test("rules, devices and calibration come before Start, and Start alone starts the clock", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page, admissions } = ctx;
  try {
    await unlock(page);
    const rules = await page.locator("#rules").textContent();
    for (const words of [
      "fullscreen",
      "invalid",
      "6 seconds",
      "calculator",
      "Bring paper.",
    ])
      assert.match(rules, new RegExp(words));
    assert.match(
      await page.locator("#not-detected").textContent(),
      /not detected/,
    );
    assert.equal(await page.locator("#devices").isDisabled(), true);
    assert.equal(await page.locator("#start").isDisabled(), true);
    await page.check("#acknowledge");
    await page.click("#devices");
    await page.waitForFunction(
      () => !document.getElementById("calibrate").disabled,
    );
    await page.click("#calibrate");
    await calibrationPhases(page);
    await page.waitForFunction(
      () => !document.getElementById("start").disabled,
    );
    assert.equal(admissions.length, 1);
    // One language is no choice to offer.
    assert.equal(await page.locator("#language-choice").isHidden(), true);
    assert.equal(admissions[0].language, "python");
    assert.ok(Date.parse(admissions[0].rulesAcknowledgedAt));
    assert.equal(admissions[0].calibration.ok, true);
    assert.equal(admissions[0].calibration.metric, "keypoints");
    assert.equal(admissions[0].version, 7);
    assert.equal(
      await page.evaluate(() =>
        globalThis.taskHarness.messages.some(
          (row) => row.topic === "task_start",
        ),
      ),
      false,
    );
    await page.click("#start");
    await page.waitForFunction(() => !document.getElementById("code").disabled);
    assert.equal(
      await page.evaluate(
        () =>
          globalThis.taskHarness.messages.filter(
            (row) => row.topic === "task_start",
          ).length,
      ),
      1,
    );
  } finally {
    await page.close();
  }
});

test("a calibration where nothing was typed is refused", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page, admissions } = ctx;
  try {
    await unlock(page);
    await page.check("#acknowledge");
    await page.click("#devices");
    await page.waitForFunction(
      () => !document.getElementById("calibrate").disabled,
    );
    await page.click("#calibrate");
    for (let i = 0; i < 2; i += 1) await page.clock.runFor(10500);
    await page.waitForFunction(() =>
      /Type the line/.test(
        document.getElementById("calibration-result").textContent,
      ),
    );
    assert.equal(await page.locator("#start").isDisabled(), true);
    assert.equal(admissions.length, 0);
  } finally {
    await page.close();
  }
});

test("a calibration without a steady face is refused and can be retried", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page, admissions } = ctx;
  try {
    await unlock(page);
    await page.check("#acknowledge");
    await page.click("#devices");
    await page.waitForFunction(
      () => !document.getElementById("calibrate").disabled,
    );
    await page.evaluate(() => {
      globalThis.faceSample = {
        available: true,
        count: 0,
        confidence: 0,
        face: null,
      };
    });
    await page.click("#calibrate");
    await calibrationPhases(page);
    await page.waitForFunction(() =>
      /not steadily/.test(
        document.getElementById("calibration-result").textContent,
      ),
    );
    assert.equal(admissions.length, 0);
    assert.equal(await page.locator("#start").isDisabled(), true);
    assert.equal(await page.locator("#calibrate").isDisabled(), false);
  } finally {
    await page.close();
  }
});

for (const [name, leave, cause] of [
  ["leaving fullscreen", () => document.exitFullscreen(), "fullscreen"],
  [
    "hiding the page",
    () => {
      Object.defineProperty(document, "hidden", {
        get: () => true,
        configurable: true,
      });
      document.dispatchEvent(new Event("visibilitychange"));
    },
    "visibility",
  ],
]) {
  test(`${name} ends the attempt as invalid at once`, async (t) => {
    const ctx = await pageFor(t, { completedReview: result("invalid", cause) });
    if (!ctx) return;
    const { page } = ctx;
    try {
      await start(page);
      await page.evaluate(leave);
      await page.waitForFunction(
        () => document.getElementById("code").disabled,
      );
      assert.deepEqual(await ended(page), [["invalid", cause]]);
      await page.clock.runFor(5500);
      await page.locator("#review").waitFor({ state: "visible" });
      assert.match(await page.locator("#outcome").textContent(), /invalid/);
      assert.doesNotMatch(
        await page.locator("#outcome").textContent(),
        /cheat/i,
      );
      const download = page.waitForEvent("download");
      await page.click("#export-code");
      assert.equal(
        await readFile(await (await download).path(), "utf8"),
        "print('frozen work')\n",
      );
    } finally {
      await page.close();
    }
  });
}

test("looking down past the calibration warns, counts down and then ends", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.evaluate(() => {
      globalThis.faceSample = {
        ...globalThis.faceSample,
        face: {
          box: [0.5, 0.6, 0.2, 0.3],
          landmarks: [
            [0.45, 0.4],
            [0.55, 0.4],
            [0.5, 0.5],
            [0.5, 0.5],
          ],
        },
      };
    });
    await page.clock.runFor(4000);
    assert.equal(await page.locator("#countdown").isVisible(), true);
    assert.match(
      await page.locator("#countdown").textContent(),
      /ends in \d seconds/,
    );
    assert.deepEqual(await ended(page), []);
    await page.clock.runFor(3000);
    assert.deepEqual(await ended(page), [["invalid", "look_away"]]);
  } finally {
    await page.close();
  }
});

test("a keyboard glance shorter than the threshold never ends the attempt", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    const looking = await page.evaluate(() => globalThis.faceSample);
    await page.evaluate(() => {
      globalThis.faceSample = {
        available: true,
        count: 0,
        confidence: 0,
        face: null,
      };
    });
    await page.clock.runFor(2000);
    await page.evaluate((sample) => {
      globalThis.faceSample = sample;
    }, looking);
    await page.clock.runFor(8000);
    assert.deepEqual(await ended(page), []);
    assert.equal(await page.locator("#countdown").isVisible(), false);
  } finally {
    await page.close();
  }
});

test("a detector that stops answering interrupts the attempt instead", async (t) => {
  const ctx = await pageFor(t, {
    completedReview: result("interrupted", "detector"),
  });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.evaluate(() => {
      globalThis.faceSample = { available: false, count: 0, confidence: 0 };
    });
    await page.clock.runFor(6000);
    assert.deepEqual(await ended(page), [["interrupted", "detector"]]);
    await page.clock.runFor(5500);
    await page.locator("#review").waitFor({ state: "visible" });
    assert.match(await page.locator("#outcome").textContent(), /interrupted/);
  } finally {
    await page.close();
  }
});

test("a detector that never answers interrupts the attempt instead of stalling it", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.evaluate(() => {
      globalThis.faceHang = true;
    });
    for (let i = 0; i < 12; i += 1) await page.clock.runFor(1000);
    assert.deepEqual(await ended(page), [["interrupted", "detector"]]);
  } finally {
    await page.close();
  }
});

test("a muted camera interrupts the attempt", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.evaluate(() =>
      globalThis.cameraStream
        .getVideoTracks()[0]
        .dispatchEvent(new Event("mute")),
    );
    assert.deepEqual(await ended(page), [["interrupted", "camera"]]);
  } finally {
    await page.close();
  }
});

test("leaving fullscreen while Start is acknowledged still ends the attempt", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await prepare(page);
    await page.evaluate(() => {
      globalThis.taskHarness.holdStart = true;
    });
    await page.click("#start");
    await page.waitForFunction(() =>
      globalThis.taskHarness.messages.some((row) => row.topic === "task_start"),
    );
    await page.evaluate(() => document.exitFullscreen());
    await page.evaluate(() => globalThis.taskHarness.releaseStart());
    await page.clock.runFor(500);
    await page.waitForFunction(() =>
      globalThis.taskHarness.messages.some((row) => row.topic === "task_end"),
    );
    assert.deepEqual(await ended(page), [["invalid", "fullscreen"]]);
  } finally {
    await page.close();
  }
});

test("once connected the task cannot change, and a departed interviewer frees the page to reconnect", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page, admissions } = ctx;
  try {
    await page.addInitScript(() => {
      globalThis.snapshotOnConnect = true;
    });
    await page.evaluate(() => {
      globalThis.snapshotOnConnect = true;
    });
    await prepare(page);
    // A snapshot that arrived before the handshake did not skip it, and the
    // handshake is sent again once the interviewer is known to be there, in
    // case the first went out before it joined.
    assert.ok(
      (await page.evaluate(
        () =>
          globalThis.taskHarness.messages.filter(
            (row) => row.topic === "task_connected",
          ).length,
      )) >= 2,
    );
    assert.equal(await page.locator("#task-picker").isDisabled(), true);
    await page.evaluate(() => globalThis.taskHarness.agentLeaves());
    await page.waitForFunction(() => document.getElementById("start").disabled);
    assert.equal(await page.locator("#task-picker").isDisabled(), false);
    assert.equal(await page.locator("#calibrate").isDisabled(), false);
    assert.match(
      await page.locator("#message-text").textContent(),
      /Calibrate again/,
    );
    await page.click("#calibrate");
    await calibrationPhases(page);
    await page.waitForFunction(
      () => !document.getElementById("start").disabled,
    );
    assert.equal(admissions.length, 2);
    assert.notEqual(admissions[0].requestKey, admissions[1].requestKey);
  } finally {
    await page.close();
  }
});

test("an ending that cannot be sent leaves the room instead of running unwatched", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.evaluate(() => {
      globalThis.taskHarness.failNextEnd = true;
      document.exitFullscreen();
    });
    // Leaving the room is what tells the server to end the attempt.
    await page.waitForFunction(() =>
      /Waiting for your result/.test(
        document.getElementById("message-text").textContent,
      ),
    );
  } finally {
    await page.close();
  }
});

test("the interviewer leaving mid-attempt turns the page to collecting the result", async (t) => {
  const ctx = await pageFor(t, {
    completedReview: result("interrupted", "room_loss"),
  });
  if (!ctx) return;
  const { page, reviewRequests } = ctx;
  try {
    await start(page);
    await page.evaluate(() => globalThis.taskHarness.agentLeaves());
    assert.match(
      await page.locator("#message-text").textContent(),
      /Waiting for your result/,
    );
    assert.equal(await page.locator("#code").isDisabled(), true);
    // Nothing is watched any more: looking down ends nothing, and the camera
    // is let go.
    await page.evaluate(() => {
      globalThis.faceSample = {
        ...globalThis.faceSample,
        face: {
          ...globalThis.faceSample.face,
          landmarks: [
            [0.45, 0.4],
            [0.55, 0.4],
            [0.5, 0.5],
            [0.5, 0.5],
          ],
        },
      };
    });
    for (let i = 0; i < 4; i += 1) await page.clock.runFor(1000);
    assert.deepEqual(await ended(page), []);
    assert.equal(await page.locator("#countdown").isHidden(), true);
    assert.equal(
      await page.evaluate(
        () => globalThis.cameraStream.getVideoTracks()[0].readyState,
      ),
      "ended",
    );
    await page.clock.runFor(5500);
    await page.locator("#review").waitFor({ state: "visible" });
    assert.ok(reviewRequests.length > 0);
    const download = page.waitForEvent("download");
    await page.click("#export-review");
    const exported = JSON.parse(
      await readFile(await (await download).path(), "utf8"),
    );
    assert.match(exported.notice, /not an authenticated submission/);
    assert.equal(exported.taskAssessment.outcome.cause, "room_loss");
  } finally {
    await page.close();
  }
});

test("a camera that stopped after calibration is caught before Start", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await prepare(page);
    await page.evaluate(() =>
      Object.defineProperty(
        globalThis.cameraStream.getVideoTracks()[0],
        "muted",
        { get: () => true },
      ),
    );
    await page.click("#start");
    assert.match(
      await page.locator("#message-text").textContent(),
      /camera stopped/,
    );
    assert.equal(
      await page.evaluate(() =>
        globalThis.taskHarness.messages.some(
          (row) => row.topic === "task_start",
        ),
      ),
      false,
    );
    assert.equal(await page.locator("#devices").isDisabled(), false);
  } finally {
    await page.close();
  }
});

test("when the server ends the attempt, leaving fullscreen is not reported", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.evaluate(() => {
      const h = globalThis.taskHarness;
      h.state.phase = "feedback";
      h.state.outcome = {
        outcome: "completed",
        cause: "timeout",
        at: 900,
        durationMs: 0,
      };
      h.publish();
    });
    await page.waitForFunction(() => !document.fullscreenElement);
    assert.deepEqual(await ended(page), []);
  } finally {
    await page.close();
  }
});

test("Start refuses without fullscreen and never starts the clock", async (t) => {
  const ctx = await pageFor(t, { denied: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await prepare(page);
    await page.click("#start");
    await page.locator("#message").waitFor({ state: "visible" });
    assert.match(
      await page.locator("#message-text").textContent(),
      /Fullscreen/,
    );
    assert.equal(
      await page.evaluate(() =>
        globalThis.taskHarness.messages.some(
          (row) => row.topic === "task_start",
        ),
      ),
      false,
    );
  } finally {
    await page.close();
  }
});

test("the calculator works inside the workspace", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.fill("#calculator-input", "(17 + 4) * 2 ^ 3");
    await page.locator("#calculator button").click();
    assert.equal(await page.locator("#calculator-result").textContent(), "168");
    await page.fill("#calculator-input", "alert(1)");
    await page.locator("#calculator button").click();
    assert.match(
      await page.locator("#calculator-result").textContent(),
      /Unknown name/,
    );
    assert.deepEqual(await ended(page), []);
  } finally {
    await page.close();
  }
});

test("runs report practice evidence and outages as telemetry gaps", async (t) => {
  for (const [options, expected] of [
    [{}, /1\/2 public cases passed/],
    [{ runnerOutage: true }, /telemetry gap/],
    [{ learnerError: true }, /SyntaxError/],
  ]) {
    const ctx = await pageFor(t, options);
    if (!ctx) return;
    const { page } = ctx;
    try {
      await start(page);
      await page.click("#run");
      await page.waitForFunction(() =>
        /\S/.test(document.getElementById("results").textContent),
      );
      assert.match(await page.locator("#results").textContent(), expected);
    } finally {
      await page.close();
    }
  }
});

test("a rejected capture or one that never left frees the next Run", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    for (const flag of ["rejectNextCapture", "failNextRevisionSend"]) {
      await page.evaluate((flag) => {
        globalThis.taskHarness[flag] = true;
      }, flag);
      await page.click("#run");
      await page.locator("#message").waitFor({ state: "visible" });
      await page.waitForFunction(
        () => !document.getElementById("run").disabled,
      );
      await page.click("#recovery");
    }
    await page.click("#run");
    await page.waitForFunction(() =>
      document.getElementById("results").textContent.includes("1/2"),
    );
  } finally {
    await page.close();
  }
});

test("code over the server's byte limit is refused on the page, never sent", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    // Multi-byte characters: under the character count, over the byte limit.
    await page.fill("#code", "# " + "é".repeat(16100));
    await page.click("#run");
    await page.locator("#message").waitFor({ state: "visible" });
    assert.match(
      await page.locator("#message-text").textContent(),
      /32000 bytes/,
    );
    assert.equal(
      await page.evaluate(() =>
        globalThis.taskHarness.messages.some(
          (row) =>
            row.topic === "task_revision" && row.message.code.length > 16000,
        ),
      ),
      false,
    );
  } finally {
    await page.close();
  }
});

test("Finish clears thinking quietly, which the locked page could no longer clear", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.check("#thinking");
    await page.click("#finish");
    await page.click("#confirm-finish");
    await page.waitForFunction(() =>
      globalThis.taskHarness.messages.some(
        (row) => row.topic === "task_action" && row.message.action === "finish",
      ),
    );
    await page.waitForFunction(
      () => !document.getElementById("thinking").checked,
    );
    assert.equal(await page.locator("#thinking").isDisabled(), true);
  } finally {
    await page.close();
  }
});

test("the editor stays locked from a Finish capture until its action is sent", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.fill("#code", "print('final answer')\n");
    await page.evaluate(() => {
      globalThis.taskHarness.holdCapture = true;
    });
    await page.click("#finish");
    await page.click("#confirm-finish");
    await page.waitForFunction(() => globalThis.taskHarness.release);
    assert.equal(await page.locator("#code").isDisabled(), true);
    await page.evaluate(() => globalThis.taskHarness.release());
    await page.waitForFunction(() =>
      globalThis.taskHarness.messages.some(
        (row) => row.topic === "task_action" && row.message.action === "finish",
      ),
    );
    const [finish, capture] = await page.evaluate(() => [
      globalThis.taskHarness.messages.find(
        (row) => row.topic === "task_action" && row.message.action === "finish",
      ),
      globalThis.taskHarness.messages.find(
        (row) =>
          row.topic === "task_revision" && row.message.trigger === "finish",
      ),
    ]);
    assert.equal(finish.message.revisionId, capture.message.revisionId);
    assert.deepEqual(await ended(page), []);
  } finally {
    await page.close();
  }
});

test("leaving warns only after the attempt ends, until its result arrives", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  const leaving = () =>
    page.evaluate(() => {
      const event = new Event("beforeunload", { cancelable: true });
      window.dispatchEvent(event);
      return event.defaultPrevented;
    });
  try {
    assert.equal(await leaving(), false);
    await start(page);
    // While the rules are armed the dialog would itself leave fullscreen, so
    // there is none: leaving is an interruption, not a broken rule.
    assert.equal(await leaving(), false);
    await page.evaluate(() => {
      const h = globalThis.taskHarness;
      h.state.phase = "feedback";
      h.publish();
    });
    assert.equal(await leaving(), true);
    await page.evaluate(
      (value) => globalThis.taskHarness.publish("task_review", value),
      result("completed", "finish"),
    );
    await page.locator("#review").waitFor({ state: "visible" });
    assert.equal(await leaving(), false);
  } finally {
    await page.close();
  }
});

test("OAuth sign-in preserves the complete assignment URL", async (t) => {
  const ctx = await pageFor(t, { signedIn: false, deviceLogin: false });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await page.locator("#web-login").waitFor({ state: "visible" });
    const url = new URL(
      await page.locator("#web-login").getAttribute("href"),
      page.url(),
    );
    assert.equal(url.searchParams.get("returnTo"), ASSIGNMENT);
  } finally {
    await page.close();
  }
});

test("reload retrieves the retained result and final code without another admission", async (t) => {
  const ctx = await pageFor(t, {
    completedReview: result("interrupted", "reload"),
  });
  if (!ctx) return;
  const { page, admissions } = ctx;
  try {
    await start(page);
    assert.equal(admissions.length, 1);
    await page.reload();
    await page.locator("#review").waitFor({ state: "visible" });
    assert.equal(admissions.length, 1);
    assert.match(await page.locator("#outcome").textContent(), /interrupted/i);
    const pending = page.waitForEvent("download");
    await page.click("#export-code");
    const download = await pending;
    assert.equal(
      await readFile(await download.path(), "utf8"),
      "print('frozen work')\n",
    );
  } finally {
    await page.close();
  }
});

test("assignment Setup requires the Google key before saving", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    const html = read("src/web/setup.rs")
      .split('const SETUP_PAGE: &str = r#"')[1]
      .split('"#;')[0];
    await page.route("**/t/classroom/delimiter-closer?**", (route) =>
      route.fulfill({ contentType: "text/html", body: html }),
    );
    await page.goto(base + ASSIGNMENT);
    assert.equal(
      await page.locator("#googleApiKey").evaluate((input) => input.required),
      true,
    );
    assert.match(
      await page.locator("body").textContent(),
      /Your assignment needs all four values/,
    );
  } finally {
    await page.close();
  }
});

test("changed task and expired polling budget still recover a retained result", async (t) => {
  const ctx = await pageFor(t, {
    completedReview: result("interrupted", "reload", "second-task"),
  });
  if (!ctx) return;
  const { page, admissions, reviewRequests } = ctx;
  try {
    await unlock(page);
    await page.selectOption("#task-picker", "second-task");
    await prepare(page, true);
    await page.click("#start");
    await page.waitForFunction(() => !document.getElementById("code").disabled);
    // Expired as the reloaded page starts, after the old page's `pagehide`
    // has written the record on its way out.
    await page.addInitScript(() => {
      const key = Object.keys(sessionStorage).find((key) =>
        key.startsWith("codetrial-task-result:"),
      );
      if (!key) return;
      const saved = JSON.parse(sessionStorage.getItem(key));
      saved.until = Date.now() - 1000;
      sessionStorage.setItem(key, JSON.stringify(saved));
    });
    await page.reload();
    await page.locator("#review").waitFor({ state: "visible" });
    assert.equal(admissions.length, 1);
    assert.equal(admissions[0].taskId, "second-task");
    assert.deepEqual(reviewRequests, ["/api/task-reviews/room-1"]);
    // The task picked before Start is the one recovered after the reload.
    const download = page.waitForEvent("download");
    await page.click("#export-review");
    const exported = JSON.parse(
      await readFile(await (await download).path(), "utf8"),
    );
    assert.equal(exported.taskAssessment.taskId, "second-task");
  } finally {
    await page.close();
  }
});

test("restored attempt waits for pending finalization without a leave prompt", async (t) => {
  const ctx = await pageFor(t, {
    reviewResponses: [{ pending: true }, result("interrupted", "reload")],
  });
  if (!ctx) return;
  const { page, admissions, reviewRequests } = ctx;
  try {
    await start(page);
    await page.reload();
    await page.waitForFunction(() =>
      document.getElementById("clock").textContent.includes("Retrieving"),
    );
    assert.equal(
      await page.evaluate(() => {
        const event = new Event("beforeunload", { cancelable: true });
        dispatchEvent(event);
        return event.defaultPrevented;
      }),
      false,
    );
    await page.locator("#review").waitFor({ state: "visible" });
    assert.equal(admissions.length, 1);
    assert.equal(reviewRequests.length, 2);
  } finally {
    await page.close();
  }
});

test("a restored telemetry gap does not offer an empty code download", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    // Expired as the reloaded page starts: the old page's `pagehide` writes
    // the record again on its way out, as a real reload would.
    await page.addInitScript(() => {
      const key = Object.keys(sessionStorage).find((key) =>
        key.startsWith("codetrial-task-result:"),
      );
      if (!key) return;
      const saved = JSON.parse(sessionStorage.getItem(key));
      saved.until = Date.now() - 1000;
      sessionStorage.setItem(key, JSON.stringify(saved));
    });
    await page.reload();
    await page.locator("#review").waitFor({ state: "visible" });
    assert.equal(await page.locator("#export-code").isDisabled(), true);
    assert.match(
      await page.locator("#review-content").textContent(),
      /cannot be retrieved/,
    );
    assert.equal(await page.locator("#new-attempt").isHidden(), true);
    assert.equal(
      await page.evaluate(() =>
        Object.keys(sessionStorage).some((key) =>
          key.startsWith("codetrial-task-result:"),
        ),
      ),
      true,
    );
  } finally {
    await page.close();
  }
});

test("a recovered result allows an explicit new attempt in the same tab", async (t) => {
  const ctx = await pageFor(t, {
    completedReview: result("interrupted", "reload"),
  });
  if (!ctx) return;
  const { page, admissions } = ctx;
  try {
    await start(page);
    await page.reload();
    await page.locator("#review").waitFor({ state: "visible" });
    await page.click("#new-attempt");
    await page.locator("#pin-form").waitFor({ state: "visible" });
    assert.equal(
      await page.evaluate(() =>
        Object.keys(sessionStorage).some((key) =>
          key.startsWith("codetrial-task-result:"),
        ),
      ),
      false,
    );
    assert.equal(admissions.length, 1);
    await start(page);
    assert.equal(admissions.length, 2);
  } finally {
    await page.close();
  }
});

test("an expired retained result offers a new attempt instead of waiting for missing code", async (t) => {
  const ctx = await pageFor(t, { reviewMissing: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.reload();
    await page.locator("#review").waitFor({ state: "visible" });
    assert.equal(await page.locator("#export-code").isDisabled(), true);
    assert.match(
      await page.locator("#review-content").textContent(),
      /No retained code/,
    );
    assert.doesNotMatch(
      await page.locator("#review-content").textContent(),
      /yet|reloading later/,
    );
    assert.equal(
      await page.evaluate(() =>
        Object.keys(sessionStorage).some((key) =>
          key.startsWith("codetrial-task-result:"),
        ),
      ),
      false,
    );
  } finally {
    await page.close();
  }
});

test("a restored interrupted result without final code disables code export", async (t) => {
  const missingCode = result("interrupted", "reload");
  missingCode.taskAssessment.finalCode = null;
  const ctx = await pageFor(t, { completedReview: missingCode });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.reload();
    await page.locator("#review").waitFor({ state: "visible" });
    assert.equal(await page.locator("#export-code").isDisabled(), true);
    assert.equal(await page.locator("#new-attempt").isVisible(), true);
  } finally {
    await page.close();
  }
});

test("a disconnected live page with pending feedback explains recovery and retains code export", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.fill("#code", "print('retained draft')\n");
    await page.evaluate(() => globalThis.taskHarness.disconnect());
    await page.clock.runFor((task.reviewDeliverySeconds + 1) * 1000);
    await page.locator("#review").waitFor({ state: "visible" });
    assert.match(
      await page.locator("#review-content").textContent(),
      /still being finalized.*reload later/,
    );
    assert.equal(await page.locator("#new-attempt").isHidden(), true);
    assert.equal(await page.locator("#export-code").isDisabled(), false);
  } finally {
    await page.close();
  }
});

test("disconnect during Start acknowledgement requires fresh calibration instead of entering work", async (t) => {
  for (const loss of ["disconnect", "agentLeaves"]) {
    await t.test(loss, async (t) => {
      const ctx = await pageFor(t);
      if (!ctx) return;
      const { page, admissions } = ctx;
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      try {
        await prepare(page);
        await page.evaluate(() => {
          globalThis.taskHarness.holdStart = true;
        });
        await page.click("#start");
        await page.waitForFunction(() => globalThis.taskHarness.releaseStart);
        await page.evaluate((loss) => globalThis.taskHarness[loss](), loss);
        await page.clock.runFor(500);
        assert.equal(await page.locator("#start").isDisabled(), true);
        assert.equal(await page.locator("#calibrate").isDisabled(), false);
        assert.match(
          await page.locator("#message-text").textContent(),
          /Calibrate again/,
        );
        assert.deepEqual(errors, []);
        assert.equal(
          await page.evaluate(() => {
            const event = new Event("beforeunload", { cancelable: true });
            dispatchEvent(event);
            return event.defaultPrevented;
          }),
          false,
        );
        await page.click("#calibrate");
        await calibrationPhases(page);
        await page.waitForFunction(
          () => !document.getElementById("start").disabled,
        );
        assert.equal(admissions.length, 2);
        assert.notEqual(admissions[0].requestKey, admissions[1].requestKey);
        await page.evaluate(() => {
          globalThis.oldTaskHarness.releaseStart();
          globalThis.oldTaskHarness.disconnect();
          globalThis.oldTaskHarness.agentLeaves();
        });
        assert.equal(await page.locator("#start").isDisabled(), false);
        assert.match(await page.locator("#phase").textContent(), /ready/);
        await page.click("#start");
        await page.waitForFunction(
          () => !document.getElementById("code").disabled,
        );
        assert.deepEqual(errors, []);
      } finally {
        await page.close();
      }
    });
  }
});

test("a ready snapshot followed by setup loss cannot enable Start", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await unlock(page);
    await page.check("#acknowledge");
    await page.click("#devices");
    await page.waitForFunction(
      () => !document.getElementById("calibrate").disabled,
    );
    await page.evaluate(() => {
      globalThis.loseTaskAfterReady = true;
    });
    await page.click("#calibrate");
    await calibrationPhases(page);
    await page.locator("#message").waitFor({ state: "visible" });
    assert.equal(await page.locator("#start").isDisabled(), true);
    assert.equal(await page.locator("#calibrate").isDisabled(), false);
    assert.match(
      await page.locator("#message-text").textContent(),
      /Calibrate again/,
    );
  } finally {
    await page.close();
  }
});

test("failed Start publication leaves no phantom recovery record", async (t) => {
  for (const failure of [
    "fullscreen-wait",
    "fullscreen-denied-after-loss",
    "publish",
  ]) {
    await t.test(failure, async (t) => {
      const ctx = await pageFor(t);
      if (!ctx) return;
      const { page } = ctx;
      try {
        await prepare(page);
        await page.evaluate((failure) => {
          if (failure === "publish") {
            globalThis.taskHarness.rejectStartSend = true;
            return;
          }
          const request = Element.prototype.requestFullscreen;
          Element.prototype.requestFullscreen = function () {
            request.call(this);
            return new Promise((resolve, reject) => {
              globalThis.releaseFullscreen =
                failure === "fullscreen-denied-after-loss"
                  ? () => reject(new Error("Fullscreen denied"))
                  : resolve;
            });
          };
        }, failure);
        await page.click("#start");
        if (failure !== "publish") {
          await page.evaluate(() => {
            globalThis.taskHarness.disconnect();
            globalThis.releaseFullscreen();
          });
        }
        await page.clock.runFor(500);
        assert.equal(
          await page.evaluate(() =>
            Object.keys(sessionStorage).some((key) =>
              key.startsWith("codetrial-task-result:"),
            ),
          ),
          false,
        );
        assert.equal(await page.locator("#start").isDisabled(), true);
        assert.equal(await page.locator("#calibrate").isDisabled(), false);
        assert.match(
          await page.locator("#message-text").textContent(),
          /Calibrate again/,
        );
        assert.equal(
          await page.evaluate(() => Boolean(document.fullscreenElement)),
          false,
        );
      } finally {
        await page.close();
      }
    });
  }
});

test("an old fullscreen request completing after replacement Start cannot disturb the active room", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page, admissions } = ctx;
  try {
    await prepare(page);
    await page.evaluate(() => {
      const request = Element.prototype.requestFullscreen;
      Element.prototype.requestFullscreen = function () {
        Element.prototype.requestFullscreen = request;
        request.call(this);
        return new Promise((resolve) => {
          globalThis.releaseOldFullscreen = resolve;
        });
      };
    });
    await page.click("#start");
    await page.evaluate(() => globalThis.taskHarness.disconnect());
    await page.click("#calibrate");
    await calibrationPhases(page);
    await page.waitForFunction(
      () => !document.getElementById("start").disabled,
    );
    await page.click("#start");
    await page.waitForFunction(() => !document.getElementById("code").disabled);
    await page.evaluate(() => globalThis.releaseOldFullscreen());
    await page.clock.runFor(500);
    assert.equal(admissions.length, 2);
    assert.equal(await page.locator("#code").isDisabled(), false);
    assert.equal(
      await page.evaluate(() => Boolean(document.fullscreenElement)),
      true,
    );
    assert.deepEqual(await ended(page), []);
    assert.equal(await page.locator("#message").isHidden(), true);
  } finally {
    await page.close();
  }
});

test("an old Start publication finishing after replacement cannot overwrite recovery metadata", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await prepare(page);
    await page.evaluate(() => {
      globalThis.taskHarness.holdStartPublish = true;
    });
    await page.click("#start");
    await page.waitForFunction(
      () => globalThis.taskHarness.releaseStartPublish,
    );
    await page.evaluate(() => globalThis.taskHarness.disconnect());
    await page.click("#calibrate");
    await calibrationPhases(page);
    await page.waitForFunction(
      () => !document.getElementById("start").disabled,
    );
    await page.click("#start");
    await page.waitForFunction(() => !document.getElementById("code").disabled);
    const saved = await page.evaluate(() => JSON.stringify(sessionStorage));
    await page.evaluate(() => globalThis.oldTaskHarness.releaseStartPublish());
    await page.clock.runFor(500);
    assert.equal(
      await page.evaluate(() => JSON.stringify(sessionStorage)),
      saved,
    );
    assert.equal(await page.locator("#code").isDisabled(), false);
    assert.equal(await page.locator("#message").isHidden(), true);
    assert.deepEqual(await ended(page), []);
  } finally {
    await page.close();
  }
});

test("task loading blocks calibration and a failed load cannot admit the previous task", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page, admissions } = ctx;
  let release;
  try {
    await unlock(page);
    await page.check("#acknowledge");
    await page.click("#devices");
    await page.waitForFunction(
      () => !document.getElementById("calibrate").disabled,
    );
    await page.route("**/tasks/second-task?**", async (route) => {
      await new Promise((resolve) => {
        release = resolve;
      });
      await route.fulfill({
        status: 500,
        contentType: "application/json",
        body: JSON.stringify({ message: "Load failed" }),
      });
    });
    await page.selectOption("#task-picker", "second-task");
    assert.equal(await page.locator("#calibrate").isDisabled(), true);
    assert.equal(await page.locator("#start").isDisabled(), true);
    await page.evaluate(() => document.getElementById("calibrate").onclick());
    assert.equal(admissions.length, 0);
    release();
    await page.locator("#message").waitFor({ state: "visible" });
    assert.equal(await page.locator("#calibrate").isDisabled(), true);
    await page.selectOption("#task-picker", "delimiter-closer");
    await page.waitForFunction(
      () => !document.getElementById("calibrate").disabled,
    );
    await page.click("#calibrate");
    await calibrationPhases(page);
    await page.waitForFunction(
      () => !document.getElementById("start").disabled,
    );
    assert.equal(admissions.length, 1);
    assert.equal(admissions[0].taskId, "delimiter-closer");
  } finally {
    release?.();
    await page.close();
  }
});

test("a stale task load failure cannot replace the current assignment's state", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  let release;
  try {
    await unlock(page);
    const pending = new Promise((resolve) => {
      page.route("**/tasks/second-task?**", async (route) => {
        resolve();
        await new Promise((done) => {
          release = done;
        });
        await route.fulfill({
          status: 403,
          contentType: "application/json",
          body: JSON.stringify({
            code: "unlock_required",
            message: "Stale failure",
          }),
        });
      });
    });
    await page.evaluate(() => {
      const picker = document.getElementById("task-picker");
      picker.value = "second-task";
      globalThis.selectionFinished = picker.onchange();
    });
    await pending;
    await page.selectOption("#task-picker", "delimiter-closer");
    await page.waitForFunction(
      () =>
        document.getElementById("prepare-title").textContent ===
        "Delimiter exercise",
    );
    const response = page.waitForResponse((response) =>
      response.url().includes("/tasks/second-task?"),
    );
    release();
    await response;
    await page.evaluate(() => globalThis.selectionFinished);
    assert.equal(await page.locator("#message").isHidden(), true);
    assert.equal(
      await page.locator("#prepare-title").textContent(),
      "Delimiter exercise",
    );
  } finally {
    release?.();
    await page.close();
  }
});

test("recovery retries the same failed assignment load", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  let requests = 0;
  try {
    await unlock(page);
    await page.check("#acknowledge");
    await page.click("#devices");
    await page.route("**/tasks/second-task?**", (route) => {
      requests++;
      return route.fulfill({
        status: requests === 1 ? 500 : 200,
        contentType: "application/json",
        body: JSON.stringify(
          requests === 1
            ? { message: "Load failed" }
            : { ...task, taskId: "second-task", title: "Second task" },
        ),
      });
    });
    await page.selectOption("#task-picker", "second-task");
    await page.locator("#message").waitFor({ state: "visible" });
    await page.click("#recovery");
    await page.waitForFunction(
      () =>
        document.getElementById("prepare-title").textContent === "Second task",
    );
    assert.equal(requests, 2);
    assert.equal(await page.locator("#calibrate").isDisabled(), false);
    assert.equal(await page.locator("#prepare").isVisible(), true);
    assert.equal(await page.locator("#message").isHidden(), true);
  } finally {
    await page.close();
  }
});

test("clearing rules acknowledgment blocks calibration even with granted devices", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page, admissions } = ctx;
  try {
    await unlock(page);
    await page.check("#acknowledge");
    await page.click("#devices");
    await page.waitForFunction(
      () => !document.getElementById("calibrate").disabled,
    );
    await page.uncheck("#acknowledge");
    assert.equal(await page.locator("#calibrate").isDisabled(), true);
    await page.evaluate(() => document.getElementById("calibrate").onclick());
    assert.equal(admissions.length, 0);
    await page.selectOption("#task-picker", "second-task");
    await page.waitForFunction(
      () =>
        document.getElementById("prepare-title").textContent !==
        "Loading assignment…",
    );
    assert.equal(await page.locator("#calibrate").isDisabled(), true);
    await page.check("#acknowledge");
    assert.equal(await page.locator("#calibrate").isDisabled(), false);
  } finally {
    await page.close();
  }
});

test("rules changes during calibration cannot start a second admission", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page, admissions } = ctx;
  try {
    await unlock(page);
    await page.check("#acknowledge");
    await page.click("#devices");
    await page.waitForFunction(
      () => !document.getElementById("calibrate").disabled,
    );
    await page.click("#calibrate");
    await page.uncheck("#acknowledge");
    await page.check("#acknowledge");
    assert.equal(await page.locator("#calibrate").isDisabled(), true);
    await page.evaluate(() => {
      document.getElementById("calibrate").onclick();
    });
    await calibrationPhases(page);
    await page.waitForFunction(
      () => !document.getElementById("start").disabled,
    );
    assert.equal(admissions.length, 1);
  } finally {
    await page.close();
  }
});

test("terminal cleanup stops the separately published microphone once", async (t) => {
  for (const ending of ["feedback", "disconnect", "rule"]) {
    await t.test(ending, async (t) => {
      const ctx = await pageFor(t);
      if (!ctx) return;
      const { page } = ctx;
      try {
        await start(page);
        assert.equal(
          await page.evaluate(() => globalThis.publishedMicrophone.stops),
          0,
        );
        await page.evaluate((ending) => {
          if (ending === "disconnect") globalThis.taskHarness.disconnect();
          else if (ending === "feedback") {
            globalThis.taskHarness.state.phase = "feedback";
            globalThis.taskHarness.publish();
          } else {
            document.dispatchEvent(new Event("fullscreenchange"));
            Object.defineProperty(document, "fullscreenElement", {
              configurable: true,
              value: null,
            });
            document.dispatchEvent(new Event("fullscreenchange"));
          }
        }, ending);
        assert.equal(
          await page.evaluate(() => globalThis.publishedMicrophone.stops),
          1,
        );
        await page.evaluate(() => globalThis.taskHarness.disconnect());
        assert.equal(
          await page.evaluate(() => globalThis.publishedMicrophone.stops),
          1,
        );
      } finally {
        await page.close();
      }
    });
  }
});

test("the camera recovery button asks for the devices again", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await unlock(page);
    await page.check("#acknowledge");
    await page.evaluate(() => {
      globalThis.grant = navigator.mediaDevices.getUserMedia;
      navigator.mediaDevices.getUserMedia = async () => {
        throw new Error("NotAllowedError");
      };
    });
    await page.click("#devices");
    await page.locator("#recovery").waitFor({ state: "visible" });
    assert.match(await page.locator("#recovery").textContent(), /microphone/i);
    await page.evaluate(() => {
      navigator.mediaDevices.getUserMedia = globalThis.grant;
    });
    await page.click("#recovery");
    await page.waitForFunction(
      () => !document.getElementById("calibrate").disabled,
    );
  } finally {
    await page.close();
  }
});

test("an attempt that ends early is collected within the delivery window", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    const saved = () =>
      page.evaluate(() => {
        const key = Object.keys(sessionStorage).find((key) =>
          key.startsWith("codetrial-task-result:"),
        );
        return JSON.parse(sessionStorage.getItem(key)).until - Date.now();
      });
    // At Start the window covers the whole task and then the delivery.
    assert.ok((await saved()) > 900000);
    await page.evaluate(() => document.exitFullscreen());
    // Ended early, it is the delivery window alone.
    assert.ok((await saved()) <= 300000, `${await saved()}`);
  } finally {
    await page.close();
  }
});

test("a reload while Start is on its way keeps the way back to the result", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await prepare(page);
    await page.evaluate(() => {
      globalThis.taskHarness.hangStart = true;
    });
    await page.click("#start");
    await page.waitForFunction(() =>
      globalThis.taskHarness.messages.some((row) => row.topic === "task_start"),
    );
    const recorded = () =>
      page.evaluate(() =>
        Object.keys(sessionStorage).some((key) =>
          key.startsWith("codetrial-task-result:"),
        ),
      );
    assert.equal(await recorded(), false);
    await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
    assert.equal(await recorded(), true);
  } finally {
    await page.close();
  }
});
