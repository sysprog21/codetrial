import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import {
  launchChromium,
  pauseClock,
  read,
  startStaticServer,
} from "./source.js";
import {
  apply,
  parseAlgorithm,
  positionCode,
  solved,
  stateFromCode,
} from "../../web/cube.js";

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
// The default task, which the session names where task mode is on.
const BUILT_IN = {
  setId: "default",
  taskId: "rubik",
  site: "https://built-in.invalid/",
  version: 1,
};
const DEFAULT_TASK = `/t/default/rubik?site=${encodeURIComponent(BUILT_IN.site)}&version=1`;

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
// and replies the way the runtime does. A classic script setting the global,
// as the vendored build is.
const sdk = `
{
const RoomEvent = {TrackSubscribed: "track", DataReceived: "data", Disconnected:"disconnected", ParticipantDisconnected: "participantDisconnected"};
class Room {
 constructor(options) {
  this.handlers = {};
  this.options = options;
  const owner = this;
  this.localParticipant = {async publishTrack(track, options) { (globalThis.publishedTracks ??= []).push({track, options, roomOptions: owner.options}); }, async streamBytes(options) { const stream = {options, bytes: 0, closed: false}; globalThis.taskHarness.boards.push(stream); return {async write(chunk) { stream.bytes += chunk.byteLength; }, async close() { stream.closed = true; }}; }, async publishData(payload, options) {
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
  globalThis.taskHarness = {messages: [], boards: [], disconnect() { owner.handlers.disconnected?.(); }, agentLeaves() { owner.handlers.participantDisconnected?.({identity: "task-agent-room-1"}); }, state: ${JSON.stringify(JSON.parse(read("tests/fixtures/task-state.json")).cases[0].payload)}, publish(topic = "task_state", value = null) { this.state.seq++; if (topic === "task_state" && !value) this.state.serverNow = new Date(Date.now()).toISOString(); owner.handlers.data?.(new TextEncoder().encode(JSON.stringify(value ?? this.state)), {identity: "task-agent-room-1"}, null, topic); }};
 }
 on(topic, callback) {this.handlers[topic] = callback;}
 async connect() { if (globalThis.snapshotOnConnect) globalThis.taskHarness.publish(); }
 async disconnect(stopTracks = true) { (globalThis.roomDisconnects ??= []).push(stopTracks); this.handlers.disconnected?.(); }
}
globalThis.LivekitClient = {RoomEvent, Room, Track: {Source: {Microphone: "microphone"}}};
}
`;

// The face detector, answering whatever sample the test sets; a face
// looking at the screen until told otherwise.
const detector = `
export const FACE_SAMPLE_GAP_MS = 5000;
export const FACE_SAMPLE_UNAVAILABLE = {available: false, count: 0, confidence: 0};
export async function loadFaceAssets({onProgress}) { onProgress({loaded: 5, total: 10}); return globalThis.faceFilesResult ?? {missing: [], unreachable: []}; }
export async function createFacePresenceDetector() {
  if (globalThis.detectorMissing) return {available: false, reason: "detector_load_failed"};
  return {available: true, async detect() { const wait = globalThis.faceFirstDelay ?? 0; globalThis.faceFirstDelay = 0; if (wait) await new Promise((resolve) => setTimeout(resolve, wait)); return globalThis.faceHang ? new Promise(() => {}) : globalThis.faceSample; }, close() { globalThis.detectorCloses = (globalThis.detectorCloses ?? 0) + 1; }};
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
    faceFiles = null,
    runnerBody = null,
    githubSignedIn = null,
    taskOverrides = {},
    realLivekit = false,
    builtIn = null,
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
    ({ denied, faceFiles }) => {
      if (faceFiles) globalThis.faceFilesResult = faceFiles;
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
      // The microphone the Devices step grants, which is the one published.
      for (const track of new AudioContext()
        .createMediaStreamDestination()
        .stream.getAudioTracks())
        globalThis.cameraStream.addTrack(track);
      navigator.mediaDevices.getUserMedia = async () => globalThis.cameraStream;
      HTMLMediaElement.prototype.play = async () => {};
      Object.defineProperty(HTMLMediaElement.prototype, "readyState", {
        get: () => 4,
        configurable: true,
      });
      globalThis.createImageBitmap = async () => ({ close() {} });
    },
    { denied, faceFiles },
  );
  await page.route(/\/t\/[^/]+\/[^/?]+/, (route) =>
    route.fulfill({ contentType: "text/html", body: read("web/task.html") }),
  );
  if (!realLivekit)
    await page.route("**/vendor/livekit-client.js", (route) =>
      route.fulfill({ contentType: "text/javascript", body: sdk }),
    );
  await page.route("**/face-presence.js", (route) =>
    route.fulfill({ contentType: "text/javascript", body: detector }),
  );
  await page.route("**/runners.js", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body:
        runnerBody ??
        (runnerOutage
          ? "export async function runBrowserTests(){return {passed:0,total:2,cases:[],runnerUnavailable:true,setupError:'Python runtime unavailable'};}"
          : learnerError
            ? "export async function runBrowserTests(){return {passed:0,total:2,cases:[],setupError:'SyntaxError: invalid syntax'};}"
            : "export async function runBrowserTests(){return {passed:1,total:2,cases:[{pass:true,label:'ordinary'},{pass:false,label:'boundary'}]};}"),
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
      body = {
        signedIn: session,
        githubSignedIn: githubSignedIn ?? session,
        deviceLogin,
        builtInTask: builtIn,
      };
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
        agentIdentity: "task-agent-room-1",
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
    } else if (!unlocked && !(builtIn && url.pathname.includes("/default/"))) {
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

/// Prepares and sends Start, which the harness holds unacknowledged until
/// `releaseStart`.
async function startHeld(page) {
  await prepare(page);
  await page.evaluate(() => {
    globalThis.taskHarness.holdStart = true;
  });
  await page.click("#start");
  await page.waitForFunction(() => globalThis.taskHarness.releaseStart);
}

/// A new camera for the page to be granted after it let the last one go.
const freshCamera = (page) =>
  page.evaluate(() => {
    globalThis.cameraStream = document.createElement("canvas").captureStream();
    for (const track of new AudioContext()
      .createMediaStreamDestination()
      .stream.getAudioTracks())
      globalThis.cameraStream.addTrack(track);
  });

async function start(page) {
  await prepare(page);
  await page.click("#start");
  await page.waitForFunction(() => !document.getElementById("code").disabled);
}

/// One stroke across the open whiteboard.
async function drawStroke(page) {
  await page.locator("#task-board canvas").scrollIntoViewIfNeeded();
  const canvas = await page.locator("#task-board canvas").boundingBox();
  await page.mouse.move(canvas.x + 40, canvas.y + 40);
  await page.mouse.down();
  await page.mouse.move(canvas.x + 120, canvas.y + 90, { steps: 4 });
  await page.mouse.up();
}

const ended = (page) =>
  page.evaluate(() =>
    globalThis.taskHarness.messages
      .filter((row) => row.topic === "task_end")
      .map((row) => [row.message.outcome, row.message.cause]),
  );

test("task setup and work keep their reading order when the columns stack", async (t) => {
  const ctx = await pageFor(t, {
    taskOverrides: {
      title:
        "A longer assignment title with a supplied implementation to revise",
      publicCases: {
        cases: [
          { label: "nested delimiters", input: ["([])"], expected: true },
        ],
      },
    },
  });
  if (!ctx) return;
  const { page } = ctx;
  const fits = () =>
    page.evaluate(() => {
      const workspace = document.getElementById("workspace");
      return (
        document.documentElement.scrollWidth <= innerWidth &&
        workspace.scrollWidth <= workspace.clientWidth
      );
    });
  // The first column ends where the second begins when they sit side by side,
  // and above it when they stack.
  const layout = async (first, second, direction) => {
    const [one, two] = await Promise.all(
      [first, second].map((selector) => page.locator(selector).boundingBox()),
    );
    const at = `${first} and ${second} at ${page.viewportSize().width}px`;
    if (direction === "row")
      assert.ok(one.x + one.width <= two.x, `${at} do not sit side by side`);
    else assert.ok(one.y + one.height <= two.y, `${at} do not stack`);
    assert.equal(await fits(), true, `${at} overflow the page`);
  };
  const across = async (first, second, ready = async () => {}) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    await layout(first, second, "row");
    await page.setViewportSize({ width: 901, height: 900 });
    await layout(first, second, "row");
    for (const width of [900, 640, 480, 375]) {
      await page.setViewportSize({ width, height: 900 });
      await layout(first, second, "column");
      await ready();
    }
  };
  try {
    await page.setViewportSize({ width: 1280, height: 900 });
    await unlock(page);
    await across(".rules-panel", ".device-steps");
    await page.setViewportSize({ width: 1280, height: 900 });
    await prepare(page, true);
    await page.click("#start");
    await page.waitForFunction(() => !document.getElementById("code").disabled);
    assert.match(
      await page.locator("#public-cases").textContent(),
      /nested delimiters/,
    );
    await across(".task-reference", ".task-editor", async () =>
      assert.equal(await page.locator("#run").isEnabled(), true),
    );
  } finally {
    await page.close();
  }
});

test("the editor can fill the page and give it back to the brief", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await page.setViewportSize({ width: 1280, height: 900 });
    await start(page);
    const toggle = page.locator("#maximize-editor");
    const width = () =>
      page.locator("#code").evaluate((box) => box.clientWidth);
    const split = await width();
    assert.equal(await toggle.getAttribute("aria-pressed"), "false");
    assert.equal(await page.locator(".task-reference").isVisible(), true);
    await toggle.click();
    assert.equal(await toggle.getAttribute("aria-pressed"), "true");
    assert.equal(await page.locator(".task-reference").isVisible(), false);
    assert.ok((await width()) > split * 1.5, "the editor widened");
    assert.equal(await page.locator("#code").isVisible(), true);
    assert.equal(
      await page.evaluate(() => document.activeElement.id),
      "maximize-editor",
    );
    await page.fill("#code", "pass\n");
    await toggle.click();
    assert.equal(await toggle.getAttribute("aria-pressed"), "false");
    assert.equal(await page.locator(".task-reference").isVisible(), true);
    assert.equal(await page.locator("#code").inputValue(), "pass\n");
    assert.equal(await page.locator("#run").isEnabled(), true);
  } finally {
    await page.close();
  }
});

test("a whiteboard drawing reaches the room once it settles", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    const boards = () => page.evaluate(() => globalThis.taskHarness.boards);
    assert.deepEqual(await boards(), []);
    await page.locator("#task-board summary").click();
    await drawStroke(page);
    // The drawing is sent after it has been still for a moment.
    await page.clock.runFor(1100);
    await page.waitForFunction(() => globalThis.taskHarness.boards.length > 0);
    const [sent] = await boards();
    assert.equal(sent.options.topic, "board_image");
    assert.equal(sent.options.mimeType, "image/jpeg");
    assert.equal(sent.options.totalSize, sent.bytes);
    assert.ok(Number(sent.options.attributes.strokes) >= 1);
    assert.equal(sent.closed, true);
    assert.match(
      await page
        .locator("#task-board [role=status]:not(.board-text-error)")
        .textContent(),
      /ready for Jim/,
    );
    // Nothing the drawing does is a message about the task.
    assert.deepEqual(await ended(page), []);
  } finally {
    await page.close();
  }
});

test("the whiteboard stops taking ink when the attempt is frozen", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.locator("#task-board summary").click();
    await drawStroke(page);
    const undo = page.locator("#task-board [data-action=undo]");
    const clear = page.locator("#task-board [data-action=clear]");
    assert.equal(await undo.isEnabled(), true);
    assert.equal(await clear.isEnabled(), true);
    await page.click("#finish");
    await page.click("#confirm-finish");
    await page.waitForFunction(() => document.getElementById("code").disabled);
    assert.equal(await undo.isEnabled(), false);
    assert.equal(await clear.isEnabled(), false);
  } finally {
    await page.close();
  }
});

test("the task editor paints syntax colours and line numbers behind the textarea", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    const layer = page.locator(".editor-highlight code");
    // A trailing newline is painted once more, so the last line keeps its row.
    const painted = async () => {
      const value = await page.inputValue("#code");
      return value.endsWith("\n") ? `${value}\n` : value;
    };
    const gutter = page.locator(".editor-lines");
    // The starter: a comment and a keyword, three lines for a buffer ending in
    // a newline.
    await page.waitForFunction(() =>
      document.querySelector(".editor-highlight code .tok-comment"),
    );
    assert.equal(await gutter.textContent(), "1\n2\n3");
    const colour = (selector) =>
      page
        .locator(selector)
        .first()
        .evaluate((node) => getComputedStyle(node).color);
    assert.equal(
      await colour(".editor-highlight .tok-comment"),
      "rgb(106, 153, 85)",
    );
    // The glyphs the learner sees are the layer's; the textarea's are clear.
    assert.equal(await colour("#code"), "rgba(0, 0, 0, 0)");
    // The text is not painted twice: the layer holds exactly the buffer.
    assert.equal(await layer.textContent(), await painted());

    await page.fill("#code", "def f(x):\n    return 'a' + str(1)\n\n\n");
    await page.waitForFunction(() =>
      document.querySelector(".editor-highlight code .tok-string"),
    );
    assert.equal(await gutter.textContent(), "1\n2\n3\n4\n5");
    assert.match(await layer.innerHTML(), /tok-keyword">def</);
    assert.equal(await layer.textContent(), await painted());

    // A long buffer: the layers follow the textarea's scroll.
    await page.fill(
      "#code",
      Array.from({ length: 120 }, (_, i) => `x${i} = ${i}`).join("\n"),
    );
    await page.evaluate(() => {
      document.getElementById("code").scrollTop = 900;
    });
    await page.waitForFunction(
      () => document.querySelector(".editor-highlight").scrollTop > 0,
    );
    assert.deepEqual(
      await page.evaluate(() => [
        document.getElementById("code").scrollTop,
        document.querySelector(".editor-highlight").scrollTop,
        document.querySelector(".editor-lines").scrollTop,
      ]),
      [900, 900, 900],
    );
  } finally {
    await page.close();
  }
});

test("a task that names the cube explainer offers a cube to turn", async (t) => {
  const plain = await pageFor(t);
  if (!plain) return;
  try {
    await start(plain.page);
    // A task that names no explainer has no cube on its page.
    assert.equal(await plain.page.locator("#cube-explorer").isVisible(), false);
    assert.equal(await plain.page.locator(".cube-scene").count(), 0);
  } finally {
    await plain.page.close();
  }

  const ctx = await pageFor(t, { taskOverrides: { explainers: ["cube"] } });
  const { page } = ctx;
  const expected = (text) =>
    positionCode(apply(solved(), parseAlgorithm(text)));
  const code = () => page.locator(".cube-code-value").textContent();
  const places = () =>
    page
      .locator(".cube-places tr")
      .evaluateAll((rows) =>
        rows.map((row) =>
          [...row.children].map((cell) => cell.textContent).join(" "),
        ),
      );
  try {
    await page.setViewportSize({ width: 1400, height: 1000 });
    await start(page);
    await page.locator("#cube-explorer").waitFor({ state: "visible" });
    assert.equal(await code(), "12345671111111");
    assert.deepEqual(await places(), [
      "Place URF DRF DFL URB DRB DBL UBL",
      "Cubie 1 2 3 4 5 6 7",
      "Twist + 1 1 1 1 1 1 1 1",
    ]);
    // 8 cubies, each a black body of six squares and three stickers.
    assert.equal(await page.locator(".cube-cubie").count(), 8);
    assert.equal(await page.locator(".cube-sticker").count(), 24);

    // A turn swings its layer and lands on the code the solver's tables give.
    await page
      .getByRole("button", { name: "R: right face, clockwise" })
      .click();
    await page.locator(".cube-layer").waitFor({ state: "attached" });
    await page.waitForFunction(
      () =>
        document.querySelector(".cube-code-value").textContent !==
        "12345671111111",
    );
    assert.equal(await code(), "25314672313211");
    assert.equal(await code(), expected("R"));
    assert.match(
      await page.locator(".cube-history").textContent(),
      /Moves so far: R$/,
    );
    assert.equal(await page.locator(".cube-layer").count(), 0);

    // Notation, in the order asked.
    await page.fill(".cube-algorithm input", "[R, D] B2");
    await page.press(".cube-algorithm input", "Enter");
    await page.waitForFunction(
      (text) =>
        document.querySelector(".cube-history").textContent.endsWith(text),
      "R D R' D' B2",
    );
    assert.equal(await code(), expected("R [R, D] B2"));
    // An algorithm it cannot read is said so, and changes nothing.
    const before = await code();
    await page.fill(".cube-algorithm input", "R X");
    await page.click(".cube-algorithm button[type=submit]");
    assert.match(
      await page.locator(".cube-status").textContent(),
      /Cannot read "X"/,
    );
    assert.equal(await code(), before);

    // A code from the public cases shows its position; a bad one says why.
    await page.fill(".cube-load input", "21653471111312");
    await page.click(".cube-load button");
    assert.equal(await code(), "21653471111312");
    assert.equal(await page.locator(".cube-status").textContent(), "");
    await page.fill(".cube-load input", "12345671111112");
    await page.click(".cube-load button");
    assert.match(
      await page.locator(".cube-status").textContent(),
      /multiple of three/,
    );
    assert.equal(await code(), "21653471111312");

    // The same cube unfolded, and each cubie's number on it.
    const views = page.locator(".cube-views button");
    assert.equal(await views.nth(0).getAttribute("aria-pressed"), "true");
    await views.nth(1).click();
    assert.equal(await views.nth(1).getAttribute("aria-pressed"), "true");
    assert.equal(await page.locator(".cube-scene").isVisible(), false);
    assert.equal(await page.locator(".cube-net rect").count(), 24);
    // One label for each face.
    assert.equal(await page.locator(".cube-net text").count(), 6);
    await page.locator(".cube-option input").check();
    // And a digit on the one up or down sticker of each of the eight cubies.
    assert.equal(await page.locator(".cube-net text").count(), 6 + 8);
    await views.nth(0).click();
    assert.equal(await page.locator(".cube-scene").isVisible(), true);

    // Reset is the solved cube, and looking round does not turn it.
    await page.getByRole("button", { name: "Back to the solved cube" }).click();
    assert.equal(await code(), "12345671111111");
    const tilt = () => page.locator(".cube-pivot").getAttribute("style");
    const first = await tilt();
    await page.locator(".cube-scene").focus();
    await page.keyboard.press("ArrowRight");
    assert.notEqual(await tilt(), first);
    assert.equal(await code(), "12345671111111");
  } finally {
    await page.close();
  }
});

test("Reset and Show are not undone by turns still swinging", async (t) => {
  const ctx = await pageFor(t, { taskOverrides: { explainers: ["cube"] } });
  if (!ctx) return;
  const { page } = ctx;
  const history = () => page.locator(".cube-history").textContent();
  const code = () => page.locator(".cube-code-value").textContent();
  // Starts R U R' U' swinging, queues behind it nine turns, which are past what
  // the cube animates and so land at once, interrupts both and asks for U, then
  // moves the clock past every turn that could still land.
  const interrupt = async (replace) => {
    await page.fill(".cube-algorithm input", "R U R' U'");
    await page.press(".cube-algorithm input", "Enter");
    await page.locator(".cube-layer").waitFor({ state: "attached" });
    await page.fill(".cube-algorithm input", "(R U)4 F");
    await page.press(".cube-algorithm input", "Enter");
    await replace();
    await page.getByRole("button", { name: "U: up face, clockwise" }).click();
    await page.clock.runFor(2000);
  };
  try {
    await page.setViewportSize({ width: 1400, height: 1000 });
    await start(page);
    await page.locator("#cube-explorer").waitFor({ state: "visible" });
    // A swing ends only when runFor moves the stopped clock, so Reset and Show
    // land while the first turn is still swinging however slowly the calls
    // before them run.
    await pauseClock(page);
    await interrupt(() =>
      page.getByRole("button", { name: "Back to the solved cube" }).click(),
    );
    assert.equal(await history(), "Moves so far: U");
    assert.equal(
      await code(),
      positionCode(apply(solved(), parseAlgorithm("U"))),
    );
    await interrupt(async () => {
      await page.fill(".cube-load input", "21653471111312");
      await page.click(".cube-load button");
    });
    assert.equal(await history(), "Moves so far: U");
    assert.equal(
      await code(),
      positionCode(apply(stateFromCode("21653471111312"), parseAlgorithm("U"))),
    );
  } finally {
    await page.close();
  }
});

test("the cube turns at once for a reader who asks for less motion", async (t) => {
  const ctx = await pageFor(t, { taskOverrides: { explainers: ["cube"] } });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await start(page);
    await page
      .getByRole("button", { name: "R: right face, clockwise" })
      .click();
    // Nothing swings: the state is there by the next frame.
    await page.waitForFunction(
      () =>
        document.querySelector(".cube-code-value").textContent ===
        "25314672313211",
      null,
      { timeout: 1000 },
    );
    assert.equal(await page.locator(".cube-layer").count(), 0);
  } finally {
    await page.close();
  }
});

test("the default task opens straight into preparation with no sign-in or PIN", async (t) => {
  const ctx = await pageFor(t, {
    signedIn: false,
    builtIn: BUILT_IN,
    path: DEFAULT_TASK,
    taskOverrides: { title: "Cube Solve Planner" },
  });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await page.locator("#prepare").waitFor({ state: "visible" });
    await page.waitForFunction(
      () =>
        document.getElementById("prepare-title").textContent ===
        "Cube Solve Planner",
    );
    for (const hidden of ["#signin", "#unlock", "#server", "#task"])
      assert.equal(await page.locator(hidden).isVisible(), false, hidden);
    // One task, so nothing to pick; and a way to the instructor's tasks.
    assert.equal(await page.locator("#task-picker").isVisible(), false);
    assert.equal(await page.locator("#instructor-offer").isVisible(), true);
    // Rules, devices and calibration come first, as for any task.
    assert.equal((await page.locator("#rules li").count()) > 0, true);
    assert.equal(await page.locator("#start").isDisabled(), true);
    await prepare(page, true);
    assert.equal(await page.locator("#start").isEnabled(), true);
  } finally {
    await page.close();
  }
});

test("the page does not offer the default task to a link that names another", async (t) => {
  const ctx = await pageFor(t, { builtIn: BUILT_IN });
  if (!ctx) return;
  const { page } = ctx;
  try {
    // An instructor's link, signed in: the PIN, as always, and no offer.
    await page.locator("#pin-form").waitFor({ state: "visible" });
    await unlock(page);
    assert.equal(await page.locator("#instructor-offer").isVisible(), false);
  } finally {
    await page.close();
  }
});

test("a learner can leave the default task for their instructor's, through GitHub, a server and a PIN", async (t) => {
  const ctx = await pageFor(t, {
    signedIn: false,
    builtIn: BUILT_IN,
    path: DEFAULT_TASK,
    taskOverrides: { title: "Cube Solve Planner" },
  });
  if (!ctx) return;
  const { page, unlocks } = ctx;
  try {
    await page.locator("#instructor-offer").waitFor({ state: "visible" });
    // Not signed in: GitHub first, then where the tasks are.
    await page.click("#instructor-tasks");
    await page.locator("#signin").waitFor({ state: "visible" });
    await page.click("#device-start");
    await page.locator("#device-code").waitFor({ state: "visible" });
    await page.clock.runFor(11000);
    await page.locator("#server").waitFor({ state: "visible" });
    assert.equal(await page.locator("#signin").isVisible(), false);

    // What cannot be an assignment is said before the PIN is spent on it.
    const message = page.locator("#server-message");
    await page.fill("#server-site", "http://teacher.example");
    await page.fill("#server-set", "classroom");
    await page.click("#server-form button");
    assert.match(await message.textContent(), /https address/);
    await page.fill("#server-site", SITE);
    await page.fill("#server-set", "Class Room");
    await page.click("#server-form button");
    assert.match(await message.textContent(), /lower case/);
    await page.fill("#server-set", "classroom");
    await page.fill("#server-version", "0");
    await page.evaluate(() => {
      document.getElementById("server-version").removeAttribute("min");
    });
    await page.click("#server-form button");
    assert.match(await message.textContent(), /whole number/);
    assert.deepEqual(unlocks, []);

    // Then the PIN for that set, and a way back to the server.
    await page.fill("#server-version", "7");
    await page.click("#server-form button");
    await page.locator("#unlock").waitFor({ state: "visible" });
    assert.match(
      await page.locator("#assignment").textContent(),
      /classroom, version 7/,
    );
    await page.click("#unlock-back");
    await page.locator("#server").waitFor({ state: "visible" });
    await page.click("#server-form button");
    await page.fill("#pin", "123456");
    await page.locator("#pin-form button").click();
    await page.locator("#prepare").waitFor({ state: "visible" });
    assert.deepEqual(unlocks.at(-1), { site: SITE, version: 7, pin: "123456" });

    // The set's own first task, in the set's own address, and no offer now.
    await page.waitForFunction(
      () =>
        document.getElementById("prepare-title").textContent !==
        "Loading assignment…",
    );
    assert.equal(
      new URL(page.url()).pathname + new URL(page.url()).search,
      ASSIGNMENT,
    );
    assert.equal(await page.locator("#task-picker").isVisible(), true);
    assert.equal(await page.locator("#instructor-offer").isVisible(), false);
  } finally {
    await page.close();
  }
});

test("a learner already signed in goes straight to the server, and can come back to the default task", async (t) => {
  const ctx = await pageFor(t, {
    builtIn: BUILT_IN,
    path: DEFAULT_TASK,
    taskOverrides: { title: "Cube Solve Planner" },
  });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await page.locator("#instructor-offer").waitFor({ state: "visible" });
    await page.click("#instructor-tasks");
    await page.locator("#server").waitFor({ state: "visible" });
    assert.equal(await page.locator("#signin").isVisible(), false);
    await page.click("#server-back");
    await page.locator("#prepare").waitFor({ state: "visible" });
    await page.waitForFunction(
      () =>
        document.getElementById("prepare-title").textContent ===
        "Cube Solve Planner",
    );
    assert.equal(new URL(page.url()).pathname, "/t/default/rubik");
    assert.equal(await page.locator("#instructor-offer").isVisible(), true);
  } finally {
    await page.close();
  }
});

test("the learning review groups feedback under readable headings", async (t) => {
  const feedback = JSON.parse(
    read("tests/fixtures/task-review.json"),
  ).cases.find((row) => row.name === "validated review").payload;
  // A verdict a review should never carry is not drawn even if one arrives.
  feedback.hireRecommendation = "hire";
  feedback.taskAssessment.recommendation = "strong hire";
  const ctx = await pageFor(t, { completedReview: feedback });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.click("#finish");
    await page.click("#confirm-finish");
    await page.evaluate(
      (feedback) => globalThis.taskHarness.publish("task_review", feedback),
      feedback,
    );
    await page
      .locator("#review-content h3")
      .first()
      .waitFor({ state: "visible" });
    const labels = {
      reasoningParticipation: "Reasoning participation",
      implementationOwnership: "Implementation ownership",
      testingAndDiagnosis: "Testing and diagnosis",
      revisionAndImprovement: "Revision and improvement",
    };
    const { dimensions, gaps, nextActions } = feedback.taskAssessment;
    const expected = [
      ...Object.entries(dimensions).map(([name, row]) => [
        labels[name],
        `${row.rating ?? "Insufficient evidence"}. ${row.reason}`,
      ]),
      ["Gaps", gaps.join(" ")],
      ["Next actions", nextActions.join(" ")],
    ];
    assert.deepEqual(
      await page
        .locator("#review-content .review-item")
        .evaluateAll((rows) =>
          rows.map((row) => [
            row.querySelector("h3").textContent,
            row.querySelector("p").textContent,
          ]),
        ),
      expected,
    );
    assert.doesNotMatch(
      await page.locator("#review").innerText(),
      /hire|hiring|recommend/i,
    );
    assert.equal(await page.locator("#export-review").isEnabled(), true);
    assert.equal(await page.locator("#export-code").isEnabled(), true);
  } finally {
    await page.close();
  }
});

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

test("a task that lowers the optimization says how its code is built", async (t) => {
  const note = (overrides) =>
    pageFor(t, {
      taskOverrides: {
        languages: ["c"],
        starterCode: { c: "int f(void);\n" },
        ...overrides,
      },
    });
  const plain = await note({});
  if (!plain) return;
  try {
    await unlock(plain.page);
    for (const id of ["#language-note", "#compiled-note"]) {
      const base = await plain.page.locator(id).textContent();
      assert.match(base, /Compiler Explorer/, id);
      assert.doesNotMatch(base, /-O1/, id);
    }
  } finally {
    await plain.page.close();
  }
  const lowered = await note({ optimization: "O1" });
  try {
    await unlock(lowered.page);
    for (const id of ["#language-note", "#compiled-note"]) {
      const text = await lowered.page.locator(id).textContent();
      assert.match(text, /Compiler Explorer/, id);
      assert.match(text, /built with -O1, not the default/, id);
    }
  } finally {
    await lowered.page.close();
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

test("a server without the face detector files says which are missing", async (t) => {
  const ctx = await pageFor(t, {
    faceFiles: {
      missing: [
        { file: "face_detection_short_range.tflite", status: 404 },
        { file: "face_detection_solution_wasm_bin.wasm", status: 404 },
      ],
      unreachable: [],
    },
  });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await unlock(page);
    await page.check("#acknowledge");
    await page.click("#devices");
    for (const place of ["#message-text", "#detector-status"]) {
      const text = await page.locator(place).textContent();
      assert.match(text, /face_detection_short_range\.tflite/);
      assert.match(text, /face_detection_solution_wasm_bin\.wasm/);
      assert.match(text, /fetch-vendor\.sh/);
    }
  } finally {
    await page.close();
  }
});

test("the face model's download and start are shown, and calibration waits for them", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    assert.match(
      await page.locator("#detector-status").textContent(),
      /model downloaded|Downloading the face model/,
    );
    await unlock(page);
    await page.check("#acknowledge");
    await page.evaluate(() => (globalThis.faceFirstDelay = 9000));
    await page.click("#devices");
    await page.waitForFunction(() =>
      /Starting the face detector/.test(
        document.getElementById("detector-status").textContent,
      ),
    );
    assert.equal(await page.locator("#calibrate").isDisabled(), true);
    await page.clock.runFor(9500);
    await page.waitForFunction(() =>
      /Face detector ready/.test(
        document.getElementById("detector-status").textContent,
      ),
    );
    await page.waitForFunction(
      () => !document.getElementById("calibrate").disabled,
    );
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
    await startHeld(page);
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
    await page.fill("#calculator-input", "sin(30)");
    await page.locator("#calculator button").click();
    assert.equal(
      await page.locator("#calculator-result").textContent(),
      "-0.988031624092862",
    );
    await page.selectOption("#calculator-angle", "deg");
    assert.equal(await page.locator("#calculator-result").textContent(), "");
    await page.locator("#calculator button").click();
    assert.equal(await page.locator("#calculator-result").textContent(), "0.5");
    await page.fill("#calculator-input", "exp(1) - e");
    await page.locator("#calculator button").click();
    assert.equal(await page.locator("#calculator-result").textContent(), "0");
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

// Without a client ID neither sign-in can work: the browser one needs the
// same ID and a secret too. A link to it led only to a refusal.
test("a build without a client ID says what sign-in needs", async (t) => {
  const ctx = await pageFor(t, { signedIn: false, deviceLogin: false });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await page.locator("#no-device-login").waitFor({ state: "visible" });
    assert.match(
      await page.locator("#no-device-login").textContent(),
      /GITHUB_CLIENT_ID/,
    );
    assert.equal(await page.locator("#device-start").isHidden(), true);
    assert.equal(
      await page.locator("#signin a[href^='/api/login']").count(),
      0,
    );
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

test("a connection lost while Start is acknowledged asks the server whether the attempt began", async (t) => {
  for (const loss of ["disconnect", "agentLeaves"]) {
    await t.test(`${loss}, and the server never started it`, async (t) => {
      const ctx = await pageFor(t, { reviewMissing: true });
      if (!ctx) return;
      const { page, admissions, reviewRequests } = ctx;
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      try {
        await startHeld(page);
        await page.evaluate((loss) => globalThis.taskHarness[loss](), loss);
        // Nothing retained for the room: the Start never arrived, so the
        // learner prepares again, with the devices let go of meanwhile.
        await page.waitForFunction(() =>
          /did not start/.test(
            document.getElementById("message-text").textContent,
          ),
        );
        assert.ok(reviewRequests.length > 0);
        assert.equal(await page.locator("#prepare").isVisible(), true);
        assert.equal(await page.locator("#start").isDisabled(), true);
        assert.equal(await page.locator("#devices").isDisabled(), false);
        assert.equal(
          await page.evaluate(() =>
            Object.keys(sessionStorage).some(
              (key) =>
                key.startsWith("codetrial-task-result:") &&
                sessionStorage.getItem(key),
            ),
          ),
          false,
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
        // The camera the page let go of is asked for anew.
        await freshCamera(page);
        await page.click("#recovery");
        await page.waitForFunction(
          () => !document.getElementById("calibrate").disabled,
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
    await t.test(`${loss}, after the server started it`, async (t) => {
      const ctx = await pageFor(t, {
        completedReview: result("interrupted", "room_loss"),
      });
      if (!ctx) return;
      const { page } = ctx;
      try {
        await startHeld(page);
        await page.evaluate((loss) => globalThis.taskHarness[loss](), loss);
        await page.locator("#review").waitFor({ state: "visible" });
        assert.match(
          await page.locator("#outcome").textContent(),
          /interrupted/,
        );
      } finally {
        await page.close();
      }
    });
  }
});

test("a Start never acknowledged is collected rather than refused", async (t) => {
  const ctx = await pageFor(t, {
    completedReview: result("interrupted", "reload"),
  });
  if (!ctx) return;
  const { page, reviewRequests } = ctx;
  try {
    await startHeld(page);
    await page.clock.runFor(10500);
    await page.locator("#review").waitFor({ state: "visible" });
    assert.ok(reviewRequests.length > 0);
    assert.match(await page.locator("#outcome").textContent(), /interrupted/);
  } finally {
    await page.close();
  }
});

test("a reload after an unacknowledged Start the server never ran prepares again", async (t) => {
  const ctx = await pageFor(t, { reviewMissing: true });
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
    await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
    await page.reload();
    await page.locator("#pin-form").waitFor({ state: "visible" });
    assert.match(
      await page.locator("#message-text").textContent(),
      /did not start/,
    );
    assert.equal(await page.locator("#review").isHidden(), true);
  } finally {
    await page.close();
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
  const ctx = await pageFor(t, { reviewMissing: true });
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
    await page.locator("#devices").waitFor({ state: "visible" });
    await page.waitForFunction(
      () => !document.getElementById("devices").disabled,
    );
    await freshCamera(page);
    await page.click("#devices");
    await page.waitForFunction(
      () => !document.getElementById("calibrate").disabled,
    );
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
    // The acknowledgement the admission records cannot be withdrawn mid-way.
    assert.equal(await page.locator("#acknowledge").isDisabled(), true);
    assert.equal(await page.locator("#calibrate").isDisabled(), true);
    await page.evaluate(() => {
      document.getElementById("calibrate").onclick();
    });
    await calibrationPhases(page);
    await page.waitForFunction(
      () => !document.getElementById("start").disabled,
    );
    assert.equal(admissions.length, 1);
    assert.equal(await page.locator("#acknowledge").isDisabled(), true);
  } finally {
    await page.close();
  }
});

// Every other test stands in for LiveKit, so this one loads the vendored build
// the page really gets. It is a classic script with no exports: importing it
// as a module failed to link and left the page dead before its first line.
test("the page boots on the vendored LiveKit build", async (t) => {
  const errors = [];
  const ctx = await pageFor(t, { realLivekit: true });
  if (!ctx) return;
  const { page } = ctx;
  page.on("pageerror", (value) => errors.push(String(value)));
  try {
    await unlock(page);
    assert.equal(
      await page.evaluate(() => typeof globalThis.LivekitClient?.Room),
      "function",
    );
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

// The room gets the microphone the Devices step granted, with its echo
// cancellation, not a second capture with LiveKit's defaults; and leaving a
// room is not the end of it, only the attempt's end is.
test("the granted microphone is the one published, and only the end stops it", async (t) => {
  for (const ending of ["feedback", "disconnect", "rule"]) {
    await t.test(ending, async (t) => {
      const ctx = await pageFor(t);
      if (!ctx) return;
      const { page } = ctx;
      try {
        await start(page);
        const published = await page.evaluate(() => {
          const [first, ...rest] = globalThis.publishedTracks;
          return {
            count: rest.length + 1,
            granted:
              first.track === globalThis.cameraStream.getAudioTracks()[0],
            source: first.options?.source,
            keepsTracks: first.roomOptions?.stopLocalTrackOnUnpublish === false,
            live: first.track.readyState,
          };
        });
        assert.deepEqual(published, {
          count: 1,
          granted: true,
          source: "microphone",
          keepsTracks: true,
          live: "live",
        });
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
          await page.evaluate(
            () => globalThis.cameraStream.getAudioTracks()[0].readyState,
          ),
          "ended",
        );
      } finally {
        await page.close();
      }
    });
  }
});

// A Compiler Explorer run can outlast the attempt. Once the result is shown the
// room is gone, so the run's report fails on it; that is no error to show over
// the result, and no gap is sent after it.
test("a run that finishes after the result stays quiet", async (t) => {
  const ctx = await pageFor(t, {
    runnerBody:
      "export async function runBrowserTests(){await new Promise((resolve) => { globalThis.finishRun = resolve; }); return {passed:1,total:1,cases:[{pass:true,label:'ordinary'}]};}",
  });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.click("#run");
    await page.waitForFunction(() => globalThis.finishRun);
    await page.evaluate(
      (value) => globalThis.taskHarness.publish("task_review", value),
      result("completed", "timeout"),
    );
    await page.locator("#review").waitFor({ state: "visible" });
    const sent = await page.evaluate(
      () => globalThis.taskHarness.messages.length,
    );
    await page.evaluate(() => globalThis.finishRun());
    // The run's report and its gap each fail at once on the missing room.
    await page.waitForTimeout(300);
    assert.equal(await page.locator("#message").isHidden(), true);
    assert.equal(
      await page.evaluate(() => globalThis.taskHarness.messages.length),
      sent,
    );
  } finally {
    await page.close();
  }
});

// A room lost before Start is left without stopping the devices, and the next
// one gets the same, still live microphone.
test("a room dropped before Start keeps the microphone for the next one", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await prepare(page);
    await page.evaluate(() => globalThis.taskHarness.disconnect());
    await page.waitForFunction(
      () => !document.getElementById("calibrate").disabled,
    );
    await page.click("#calibrate");
    await calibrationPhases(page);
    await page.waitForFunction(
      () => !document.getElementById("start").disabled,
    );
    const state = await page.evaluate(() => {
      const granted = globalThis.cameraStream.getAudioTracks()[0];
      return {
        published: globalThis.publishedTracks.map(
          ({ track }) => track === granted,
        ),
        live: granted.readyState,
      };
    });
    assert.deepEqual(state, { published: [true, true], live: "live" });
  } finally {
    await page.close();
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

test("leaving the page or fullscreen while Start is acknowledged counts even after coming back", async (t) => {
  for (const rule of ["visibility", "fullscreen"]) {
    await t.test(rule, async (t) => {
      const ctx = await pageFor(t);
      if (!ctx) return;
      const { page } = ctx;
      try {
        await page.evaluate(() => {
          globalThis.hiddenNow = false;
          Object.defineProperty(document, "hidden", {
            get: () => globalThis.hiddenNow,
            configurable: true,
          });
        });
        await startHeld(page);
        await page.evaluate(async (rule) => {
          if (rule === "visibility") {
            globalThis.hiddenNow = true;
            document.dispatchEvent(new Event("visibilitychange"));
            globalThis.hiddenNow = false;
            document.dispatchEvent(new Event("visibilitychange"));
          } else {
            await document.exitFullscreen();
            await document.getElementById("workspace").requestFullscreen();
          }
          globalThis.taskHarness.releaseStart();
        }, rule);
        await page.clock.runFor(500);
        await page.waitForFunction(() =>
          globalThis.taskHarness.messages.some(
            (row) => row.topic === "task_end",
          ),
        );
        assert.deepEqual(await ended(page), [["invalid", rule]]);
      } finally {
        await page.close();
      }
    });
  }
});

test("the workspace lists the public cases, and a failing run says what went wrong", async (t) => {
  const ctx = await pageFor(t, {
    taskOverrides: {
      publicCases: {
        entry: "isValid",
        checker: "exact",
        cases: [
          { label: "ordinary", input: ["()"], expected: true },
          { label: "boundary", input: [")("], expected: false },
          { label: "nested", input: ["([])"], expected: true },
          // A rename can make a shown label equal a required case's.
          { label: "boundary", input: ["(("], expected: false },
        ],
      },
      requiredCases: [1],
    },
    runnerBody:
      "export async function runBrowserTests(){return {passed:1,total:3,cases:[{pass:true,label:'ordinary'},{pass:false,label:'boundary',input:'[\")(\"]',expected:'false',got:'true'},{pass:false,label:'nested',input:'[\"([])\"]',expected:'true',got:'-',error:'IndexError: pop from empty list'}]};}",
  });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    assert.deepEqual(await page.locator("#public-cases li").allTextContents(), [
      'ordinary: input ["()"], expected true',
      'boundary (required): input [")("], expected false',
      'nested: input ["([])"], expected true',
      'boundary: input ["(("], expected false',
    ]);
    await page.click("#run");
    await page.waitForFunction(() =>
      /1\/3/.test(document.getElementById("results").textContent),
    );
    const results = await page.locator("#results").textContent();
    assert.match(results, /OK: ordinary/);
    assert.ok(
      results.includes(
        'FAIL: boundary (required) (input [")("]; expected false; got true)',
      ),
      results,
    );
    assert.match(
      results,
      /FAIL: nested .*error IndexError: pop from empty list/,
    );
  } finally {
    await page.close();
  }
});

test("the editor leaves input-method keys and chords alone, and its indents can be undone", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    const prevented = (init, type = "keydown") =>
      page.evaluate(
        ({ init, type }) => {
          const editor = document.getElementById("code");
          const event =
            type === "keydown"
              ? new KeyboardEvent("keydown", {
                  ...init,
                  bubbles: true,
                  cancelable: true,
                })
              : new InputEvent("beforeinput", {
                  ...init,
                  bubbles: true,
                  cancelable: true,
                });
          editor.dispatchEvent(event);
          return event.defaultPrevented;
        },
        { init, type },
      );
    assert.equal(await prevented({ key: "Enter", isComposing: true }), false);
    assert.equal(await prevented({ key: "Tab", isComposing: true }), false);
    assert.equal(await prevented({ key: "Tab", keyCode: 229 }), false);
    assert.equal(await prevented({ key: "Tab", ctrlKey: true }), false);
    assert.equal(await prevented({ key: "Enter", metaKey: true }), false);
    assert.equal(
      await prevented(
        { inputType: "insertLineBreak", isComposing: true },
        "beforeinput",
      ),
      false,
    );
    assert.equal(await prevented({ key: "Tab" }), true);
    await page.fill("#code", "if x:");
    await page.focus("#code");
    await page.keyboard.press("End");
    await page.keyboard.press("Enter");
    assert.equal(await page.inputValue("#code"), "if x:\n    ");
    await page.keyboard.press("ControlOrMeta+Z");
    assert.equal(await page.inputValue("#code"), "if x:");
    // F2, not Escape, lets Tab leave the editor: Escape would also take the
    // browser out of fullscreen and end the attempt.
    assert.equal(await prevented({ key: "Escape" }), false);
    assert.equal(await prevented({ key: "Tab" }), true);
    await page.keyboard.press("F2");
    assert.equal(await prevented({ key: "Tab" }), false);
  } finally {
    await page.close();
  }
});

test("each recovery button does what its label says", async (t) => {
  await t.test("Reload reloads", async (t) => {
    const ctx = await pageFor(t);
    if (!ctx) return;
    const { page } = ctx;
    try {
      await page.locator("#pin-form").waitFor({ state: "visible" });
      await page.route("**/unlock", (route) =>
        route.fulfill({
          status: 403,
          contentType: "application/json",
          body: JSON.stringify({ code: "csrf_rejected", message: "No." }),
        }),
      );
      await page.fill("#pin", "123456");
      await page.locator("#pin-form button").click();
      await page.locator("#message").waitFor({ state: "visible" });
      assert.equal(await page.locator("#recovery").textContent(), "Reload");
      const reloaded = page.waitForEvent("load");
      await page.click("#recovery");
      await reloaded;
    } finally {
      await page.close();
    }
  });
  await t.test("Return to task list opens another task", async (t) => {
    const ctx = await pageFor(t);
    if (!ctx) return;
    const { page } = ctx;
    try {
      await page.route("**/tasks/delimiter-closer?**", (route) =>
        route.fulfill({
          status: 404,
          contentType: "application/json",
          body: JSON.stringify({ code: "task_not_found", message: "Gone." }),
        }),
      );
      await page.locator("#pin-form").waitFor({ state: "visible" });
      await page.fill("#pin", "123456");
      await page.locator("#pin-form button").click();
      await page.locator("#message").waitFor({ state: "visible" });
      assert.equal(
        await page.locator("#recovery").textContent(),
        "Return to task list",
      );
      await page.click("#recovery");
      await page.waitForFunction(
        () =>
          document.getElementById("prepare-title").textContent ===
          "Delimiter exercise",
      );
      assert.equal(await page.inputValue("#task-picker"), "second-task");
      assert.match(page.url(), /\/t\/classroom\/second-task\?/);
    } finally {
      await page.close();
    }
  });
  await t.test("Retry Start retries Start", async (t) => {
    const ctx = await pageFor(t);
    if (!ctx) return;
    const { page } = ctx;
    try {
      await prepare(page);
      await page.evaluate(() => {
        const request = Element.prototype.requestFullscreen;
        Element.prototype.requestFullscreen = function () {
          Element.prototype.requestFullscreen = request;
          return Promise.reject(new Error("denied"));
        };
      });
      await page.click("#start");
      await page.locator("#message").waitFor({ state: "visible" });
      assert.equal(
        await page.locator("#recovery").textContent(),
        "Retry Start",
      );
      await page.click("#recovery");
      await page.waitForFunction(
        () => !document.getElementById("code").disabled,
      );
    } finally {
      await page.close();
    }
  });
  await t.test(
    "an uncoded failure mid-attempt is only dismissed",
    async (t) => {
      const ctx = await pageFor(t);
      if (!ctx) return;
      const { page } = ctx;
      try {
        await start(page);
        await page.evaluate(() => {
          globalThis.taskHarness.failNextRevisionSend = true;
        });
        await page.click("#run");
        await page.locator("#message").waitFor({ state: "visible" });
        assert.equal(await page.locator("#recovery").textContent(), "Dismiss");
        await page.click("#recovery");
        assert.equal(await page.locator("#message").isHidden(), true);
        assert.equal(await page.locator("#code").isDisabled(), false);
      } finally {
        await page.close();
      }
    },
  );
  await t.test(
    "a failure answered by a reload mid-attempt is only dismissed",
    async (t) => {
      const ctx = await pageFor(t);
      if (!ctx) return;
      const { page } = ctx;
      try {
        await start(page);
        await page.evaluate(() =>
          globalThis.taskHarness.publish("task_error", {
            version: 1,
            code: "csrf_rejected",
            retryable: false,
            message: "The request was refused.",
          }),
        );
        await page.locator("#message").waitFor({ state: "visible" });
        assert.equal(await page.locator("#recovery").textContent(), "Dismiss");
        const navigations = [];
        page.on("framenavigated", (frame) => navigations.push(frame.url()));
        await page.click("#recovery");
        assert.equal(await page.locator("#message").isHidden(), true);
        assert.deepEqual(navigations, []);
      } finally {
        await page.close();
      }
    },
  );
});

test("a Finish dialog open when the attempt ends closes and finishes nothing", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.click("#finish");
    assert.equal(await page.locator("#confirm").isVisible(), true);
    await page.evaluate(() => {
      const h = globalThis.taskHarness;
      h.state.phase = "feedback";
      h.publish();
    });
    await page.waitForFunction(() => !document.getElementById("confirm").open);
    await page.evaluate(() =>
      document.getElementById("confirm-finish").onclick(),
    );
    assert.equal(
      await page.evaluate(() =>
        globalThis.taskHarness.messages.some(
          (row) => row.topic === "task_action",
        ),
      ),
      false,
    );
  } finally {
    await page.close();
  }
});

test("a result poll still on its way neither repeats nor replaces the delivered result", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  let held = 0;
  const releases = [];
  try {
    await start(page);
    await page.route("**/api/task-reviews/**", async (route) => {
      held += 1;
      await new Promise((resolve) => releases.push(resolve));
      await route.fulfill({
        status: 202,
        contentType: "application/json",
        body: JSON.stringify({ pending: true }),
      });
    });
    await page.evaluate(() => document.exitFullscreen());
    // Past the delivery window, with the first poll still unanswered.
    await page.clock.runFor(301000);
    assert.equal(held, 1);
    await page.evaluate(
      (value) => globalThis.taskHarness.publish("task_review", value),
      result("invalid", "fullscreen"),
    );
    await page.locator("#review").waitFor({ state: "visible" });
    for (const release of releases) release();
    await page.clock.runFor(1000);
    assert.match(await page.locator("#outcome").textContent(), /invalid/);
    assert.doesNotMatch(
      await page.locator("#review-content").textContent(),
      /still being finalized|unavailable/i,
    );
  } finally {
    for (const release of releases) release();
    await page.close();
  }
});

test("an ended attempt lets go of a wake lock granted late and of its detector", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await page.evaluate(() => {
      globalThis.wakeReleases = 0;
      Object.defineProperty(navigator, "wakeLock", {
        configurable: true,
        value: {
          request: () =>
            new Promise((resolve) => {
              globalThis.grantWakeLock = () =>
                resolve({
                  async release() {
                    globalThis.wakeReleases += 1;
                  },
                });
            }),
        },
      });
    });
    await start(page);
    await page.evaluate(() => document.exitFullscreen());
    await page.evaluate(() => globalThis.grantWakeLock());
    await page.clock.runFor(100);
    assert.equal(await page.evaluate(() => globalThis.wakeReleases), 1);
    assert.equal(await page.evaluate(() => globalThis.detectorCloses), 1);
  } finally {
    await page.close();
  }
});

test("asking for the devices again closes the detector it replaces", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await prepare(page);
    await page.evaluate(() =>
      globalThis.cameraStream.getVideoTracks()[0].stop(),
    );
    await page.click("#start");
    await page.locator("#message").waitFor({ state: "visible" });
    await freshCamera(page);
    await page.click("#recovery");
    await page.waitForFunction(() => globalThis.detectorCloses === 1);
  } finally {
    await page.close();
  }
});

test("a request key refused once is not reused for another language", async (t) => {
  const ctx = await pageFor(t, {
    taskOverrides: {
      languages: ["python", "javascript"],
      starterCode: {
        python: task.starterCode.python,
        javascript: "// TASK_COMPLETE_CLOSER\n",
      },
    },
  });
  if (!ctx) return;
  const { page, admissions } = ctx;
  let refused = 0;
  try {
    await page.route("**/attempts", async (route) => {
      if (refused++ === 0) {
        admissions.push(route.request().postDataJSON());
        return route.fulfill({
          status: 409,
          headers: { "Retry-After": "1" },
          contentType: "application/json",
          body: JSON.stringify({ code: "setup_pending", message: "Wait." }),
        });
      }
      return route.fallback();
    });
    await prepare(page).catch(() => {});
    await page.locator("#message").waitFor({ state: "visible" });
    await page.selectOption("#language", "javascript");
    await page.click("#calibrate");
    await calibrationPhases(page);
    await page.waitForFunction(
      () => !document.getElementById("start").disabled,
    );
    assert.equal(admissions.length, 2);
    assert.equal(admissions[1].language, "javascript");
    assert.notEqual(admissions[0].requestKey, admissions[1].requestKey);
  } finally {
    await page.close();
  }
});

test("a revision that never left reports no run gap for it", async (t) => {
  const ctx = await pageFor(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await start(page);
    await page.evaluate(() => {
      globalThis.taskHarness.failNextRevisionSend = true;
    });
    await page.click("#run");
    await page.locator("#message").waitFor({ state: "visible" });
    await page.waitForFunction(() => !document.getElementById("run").disabled);
    assert.equal(
      await page.evaluate(() =>
        globalThis.taskHarness.messages.some((row) => row.topic === "task_run"),
      ),
      false,
    );
  } finally {
    await page.close();
  }
});

test("an account signed in without GitHub signs in again before the PIN", async (t) => {
  const ctx = await pageFor(t, { githubSignedIn: false });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await page.locator("#signin").waitFor({ state: "visible" });
    assert.equal(await page.locator("#unlock").isHidden(), true);
  } finally {
    await page.close();
  }
});

function beforeUnloadPrevented(page) {
  return page.evaluate(() => {
    const event = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(event);
    return event.defaultPrevented;
  });
}

// Fresh devices, calibration, a new recording, then Start.
async function recordAgainAndStart(page) {
  await freshCamera(page);
  await page.click("#devices");
  await page.waitForFunction(
    () => !document.getElementById("calibrate").disabled,
  );
  await page.click("#calibrate");
  await calibrationPhases(page);
  await page.waitForFunction(() => !document.getElementById("start").disabled);
  await page.click("[data-local-start]");
  await page.waitForFunction(
    () => !document.querySelector("[data-local-stop]").hidden,
  );
  await page.click("#start");
  await page.waitForFunction(() => !document.getElementById("code").disabled);
}

async function prepareLocalRecording(page) {
  await page.addInitScript(() => {
    navigator.mediaDevices.setCaptureHandleConfig = ({ handle }) => {
      window.taskCaptureHandle = handle;
    };
  });
  await page.reload();
  await prepare(page);
  await page.evaluate(() => {
    // Codec coverage lives in local-recorder.test.js. This fixture controls
    // final data delivery while exercising the task's real lifecycle wiring.
    globalThis.MediaRecorder = class {
      static isTypeSupported() {
        return true;
      }
      constructor() {
        globalThis.taskRecorder = this;
        this.mimeType = "video/webm";
        this.state = "inactive";
      }
      start() {
        this.state = "recording";
      }
      stop() {
        this.state = "inactive";
        queueMicrotask(() => {
          this.ondataavailable({
            data: new Blob(["task recording"], { type: this.mimeType }),
          });
          this.onstop();
        });
      }
    };
    navigator.mediaDevices.getDisplayMedia = async () => {
      const canvas = document.createElement("canvas");
      const stream = canvas.captureStream(10);
      canvas.getContext("2d").fillRect(0, 0, 100, 100);
      const video = stream.getVideoTracks()[0];
      video.getSettings = () => ({ displaySurface: "browser" });
      video.getCaptureHandle = () => ({ handle: window.taskCaptureHandle });
      stream.addTrack(
        new AudioContext()
          .createMediaStreamDestination()
          .stream.getAudioTracks()[0],
      );
      return stream;
    };
  });
  await page.click("[data-local-start]");
  await page.waitForFunction(() =>
    document.querySelector("[data-local-stop]").textContent.includes("00:"),
  );
}

for (const ending of [
  "finish",
  "disconnect",
  "fullscreen",
  "visibility",
  "look_down",
  "no_face",
  "camera",
  "detector",
  "stop_then_look_down",
]) {
  test(`local task recording downloads after ${ending}`, async (t) => {
    const fixture = await pageFor(t);
    if (!fixture) return;
    const { page } = fixture;
    t.after(() => page.close());
    await prepareLocalRecording(page);
    await page.click("#start");
    await page.waitForFunction(() => !document.getElementById("code").disabled);
    const download = page.waitForEvent("download");
    if (ending === "stop_then_look_down") {
      await page.click("[data-local-stop]");
      await download;
      assert.equal(await page.locator("#code").isEnabled(), true);
      assert.equal(
        await page.evaluate(
          () => globalThis.cameraStream.getVideoTracks()[0].readyState,
        ),
        "live",
      );
    }
    await page.evaluate((ending) => {
      if (ending === "disconnect") globalThis.taskHarness.disconnect();
      else if (ending === "fullscreen") document.exitFullscreen();
      else if (ending === "visibility") {
        Object.defineProperty(document, "hidden", {
          get: () => true,
          configurable: true,
        });
        document.dispatchEvent(new Event("visibilitychange"));
      } else if (["look_down", "stop_then_look_down"].includes(ending)) {
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
      } else if (ending === "no_face") {
        globalThis.faceSample = {
          available: true,
          count: 0,
          confidence: 0,
          face: null,
        };
      } else if (ending === "camera") {
        globalThis.cameraStream
          .getVideoTracks()[0]
          .dispatchEvent(new Event("mute"));
      } else if (ending === "detector") {
        globalThis.faceSample = { available: false, count: 0, confidence: 0 };
      } else {
        globalThis.taskHarness.state.phase = "feedback";
        globalThis.taskHarness.publish();
      }
    }, ending);
    if (["look_down", "no_face", "stop_then_look_down"].includes(ending)) {
      await page.clock.runFor(4000);
      assert.equal(await page.locator("#countdown").isVisible(), true);
      assert.deepEqual(await ended(page), []);
      await page.clock.runFor(3000);
    } else if (ending === "detector") await page.clock.runFor(6000);
    const expectedCause = {
      fullscreen: "fullscreen",
      visibility: "visibility",
      look_down: "look_away",
      no_face: "look_away",
      stop_then_look_down: "look_away",
      camera: "camera",
      detector: "detector",
    }[ending];
    if (expectedCause) {
      assert.deepEqual(await ended(page), [
        [
          ["camera", "detector"].includes(ending) ? "interrupted" : "invalid",
          expectedCause,
        ],
      ]);
      assert.equal(await page.locator("#code").isDisabled(), true);
    }
    assert.match((await download).suggestedFilename(), /\.webm$/);
    assert.equal(
      await page.locator("#task [data-local-download]").isVisible(),
      true,
    );
    assert.equal(
      await page
        .locator("#task [data-local-download]")
        .getAttribute("href")
        .then(Boolean),
      true,
    );
    await page.evaluate(
      (review) => globalThis.taskHarness.publish("task_review", review),
      result("completed", "finish"),
    );
    await page
      .locator("#review [data-local-download]")
      .waitFor({ state: "visible" });
  });
}

test("task recording finalizes before stopped camera devices are replaced", async (t) => {
  const fixture = await pageFor(t);
  if (!fixture) return;
  const { page } = fixture;
  t.after(() => page.close());
  await prepareLocalRecording(page);
  await page.evaluate(() => globalThis.cameraStream.getVideoTracks()[0].stop());
  await page.click("#start");
  await page.waitForFunction(() =>
    document
      .getElementById("message-text")
      .textContent.includes("camera stopped"),
  );
  assert.equal(await page.locator("[data-local-stop]").isHidden(), true);
  assert.equal(
    await page.locator("#prepare [data-local-download]").isVisible(),
    true,
  );
  await recordAgainAndStart(page);
  assert.equal(
    await page.evaluate(
      () => globalThis.cameraStream.getAudioTracks()[0].readyState,
    ),
    "live",
  );
});

test("task recording can restart after an unconfirmed Start never arrived", async (t) => {
  const fixture = await pageFor(t, { reviewMissing: true });
  if (!fixture) return;
  const { page } = fixture;
  t.after(() => page.close());
  await prepareLocalRecording(page);
  await page.evaluate(() => {
    globalThis.taskHarness.holdStart = true;
  });
  await page.click("#start");
  await page.waitForFunction(() => globalThis.taskHarness.releaseStart);
  await page.evaluate(() => globalThis.taskHarness.disconnect());
  await page.waitForFunction(() =>
    document
      .getElementById("message-text")
      .textContent.includes("did not start"),
  );
  assert.equal(
    await page.locator("#prepare [data-local-start]").isVisible(),
    true,
  );
  assert.equal(
    await page.locator("#prepare [data-local-download]").isVisible(),
    true,
  );
  await recordAgainAndStart(page);
  const download = page.waitForEvent("download");
  await page.evaluate(() => document.exitFullscreen());
  assert.match((await download).suggestedFilename(), /\.webm$/);
  assert.deepEqual(await ended(page), [["invalid", "fullscreen"]]);
});

test("task recording can restart after a rule broken during an unconfirmed Start that never arrived", async (t) => {
  const fixture = await pageFor(t, { reviewMissing: true });
  if (!fixture) return;
  const { page } = fixture;
  t.after(() => page.close());
  await prepareLocalRecording(page);
  await page.evaluate(() => {
    globalThis.taskHarness.holdStart = true;
  });
  await page.click("#start");
  await page.waitForFunction(() => globalThis.taskHarness.releaseStart);
  await page.evaluate(() => document.exitFullscreen());
  await page.evaluate(() => globalThis.taskHarness.disconnect());
  await page.waitForFunction(() =>
    document
      .getElementById("message-text")
      .textContent.includes("did not start"),
  );
  assert.equal(
    await page.locator("#prepare [data-local-start]").isVisible(),
    true,
  );
  assert.equal(
    await page.locator("#prepare [data-local-start]").isDisabled(),
    false,
  );
});

test("leaving while a task recording is unsaved asks first, and a kept page keeps recording usable", async (t) => {
  const fixture = await pageFor(t);
  if (!fixture) return;
  const { page } = fixture;
  t.after(() => page.close());
  const unloadPrevented = () => beforeUnloadPrevented(page);
  assert.equal(await unloadPrevented(), false);
  await prepareLocalRecording(page);
  assert.equal(await unloadPrevented(), true);
  // A page restored from the back/forward cache is not torn down.
  await page.evaluate(() => {
    const event = new Event("pagehide");
    event.persisted = true;
    window.dispatchEvent(event);
  });
  assert.equal(await page.locator("[data-local-stop]").isVisible(), true);
});

test("a take that was not downloaded still asks before leaving after the result", async (t) => {
  const fixture = await pageFor(t);
  if (!fixture) return;
  const { page } = fixture;
  t.after(() => page.close());
  const unloadPrevented = () => beforeUnloadPrevented(page);
  await prepareLocalRecording(page);
  // A lost microphone saves the partial take without downloading it.
  await page.evaluate(() =>
    globalThis.cameraStream
      .getAudioTracks()[0]
      .dispatchEvent(new Event("ended")),
  );
  await page
    .locator("#prepare [data-local-download]")
    .waitFor({ state: "visible" });
  assert.equal(await unloadPrevented(), true);
  await page.click("#start");
  await page.waitForFunction(() => !document.getElementById("code").disabled);
  await page.evaluate(
    (review) => globalThis.taskHarness.publish("task_review", review),
    result("completed", "finish"),
  );
  await page.locator("#review").waitFor({ state: "visible" });
  assert.equal(await unloadPrevented(), true);
  const download = page.waitForEvent("download");
  await page.locator("#review [data-local-download]").click();
  await download;
  assert.equal(await unloadPrevented(), false);
});

test("an empty task recording returns focus to a visible workspace control", async (t) => {
  const fixture = await pageFor(t);
  if (!fixture) return;
  const { page } = fixture;
  t.after(() => page.close());
  await prepareLocalRecording(page);
  await page.click("#start");
  await page.waitForFunction(() => !document.getElementById("code").disabled);
  await page.evaluate(() => {
    globalThis.taskRecorder.stop = function () {
      this.state = "inactive";
      queueMicrotask(() => this.onstop());
    };
  });
  await page.click("[data-local-stop]");
  await page.waitForFunction(
    () => document.querySelector("[data-local-stop]").hidden,
  );
  assert.equal(
    await page.evaluate(() => document.activeElement.id),
    "maximize-editor",
  );
  assert.equal(await page.locator("#code").isEnabled(), true);
});

test("fullscreen entry blocks a new task recording picker until Start settles", async (t) => {
  const fixture = await pageFor(t);
  if (!fixture) return;
  const { page } = fixture;
  t.after(() => page.close());
  await prepareLocalRecording(page);
  const download = page.waitForEvent("download");
  await page.click("[data-local-stop]");
  await download;
  await page.evaluate(() => {
    const workspace = document.getElementById("workspace");
    const fullscreen = workspace.requestFullscreen.bind(workspace);
    workspace.requestFullscreen = () =>
      new Promise((resolve) => {
        globalThis.releaseRecordingFullscreen = async () => {
          await fullscreen();
          resolve();
        };
      });
    const capture = navigator.mediaDevices.getDisplayMedia;
    globalThis.taskCaptureRequests = 0;
    navigator.mediaDevices.getDisplayMedia = (...args) => {
      globalThis.taskCaptureRequests += 1;
      return capture(...args);
    };
  });
  await page.click("#start");
  await page.waitForFunction(() => globalThis.releaseRecordingFullscreen);
  await page.click("[data-local-start]");
  assert.equal(await page.evaluate(() => globalThis.taskCaptureRequests), 0);
  await page.evaluate(() => globalThis.releaseRecordingFullscreen());
  await page.waitForFunction(() => !document.getElementById("code").disabled);
});

for (const browserOffset of [-600000, 0, 600000]) {
  test(`task countdown corrects browser clock offset ${browserOffset} and keeps advancing`, async (t) => {
    const fixture = await pageFor(t);
    if (!fixture) return;
    const { page } = fixture;
    t.after(() => page.close());
    await start(page);
    const serverNow = "2030-01-01T00:10:00Z";
    await page.clock.setSystemTime(
      new Date(Date.parse(serverNow) + browserOffset),
    );
    await page.evaluate((serverNow) => {
      const harness = globalThis.taskHarness;
      const snapshot = {
        ...harness.state,
        seq: harness.state.seq + 1,
        serverNow,
        deadlineAt: "2030-01-01T00:15:00Z",
      };
      globalThis.clockSnapshot = snapshot;
      harness.publish("task_state", snapshot);
    }, serverNow);
    await page.clock.runFor(1000);
    assert.equal(await page.locator("#clock").textContent(), "04:59 remaining");
    // Old timestamps and deadlines must not move the accepted clock offset.
    await page.evaluate(() => {
      const { seq, ...snapshot } = globalThis.clockSnapshot;
      for (const staleSeq of [seq - 1, seq]) {
        globalThis.taskHarness.publish("task_state", {
          ...snapshot,
          seq: staleSeq,
          serverNow: "2030-01-01T02:10:00Z",
        });
      }
    });
    await page.clock.runFor(2000);
    assert.equal(await page.locator("#clock").textContent(), "04:57 remaining");
    // Three seconds in, a snapshot that reads five seconds ahead moves the
    // clock forward; one that reads behind it, as a delayed one does, cannot
    // pull it back.
    for (const serverNow of ["2030-01-01T00:10:08Z", "2030-01-01T00:10:00Z"]) {
      await page.evaluate((serverNow) => {
        const snapshot = {
          ...globalThis.clockSnapshot,
          seq: globalThis.taskHarness.state.seq + 1,
          serverNow,
        };
        globalThis.taskHarness.publish("task_state", snapshot);
      }, serverNow);
    }
    await page.clock.runFor(2000);
    assert.equal(await page.locator("#clock").textContent(), "04:50 remaining");
  });
}
