import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { topics } from "../../web/lib.js";
import { DEFAULT_RUNTIME_CONFIG, startStaticServer } from "./source.js";

let browser;
let server;
let base;
before(async () => {
  try {
    const { chromium } = await import("playwright");
    browser = await chromium.launch({
      args: [
        "--use-fake-device-for-media-stream",
        "--use-fake-ui-for-media-stream",
        `--use-file-for-fake-audio-capture=${fileURLToPath(new URL("../../web/audio/test-voice.wav", import.meta.url))}`,
      ],
    });
  } catch (error) {
    if (process.env.CI) throw error;
    return;
  }
  ({ server, base } = await startStaticServer({
    runtimeConfig: DEFAULT_RUNTIME_CONFIG,
  }));
});
after(async () => {
  await browser?.close();
  await new Promise((closed) => (server ? server.close(closed) : closed()));
});

// Delivery and teardown are released by the test, not by a timer. The page
// still registers its real callbacks and crosses the real media preflight.
function sdkFixture() {
  const agent = { identity: "agent-startup", kind: "AGENT", attributes: {} };
  const other = { identity: "agent-other", kind: "AGENT", attributes: {} };
  class Room {
    constructor() {
      this.handlers = new Map();
      this.remoteParticipants = new Map([
        [agent.identity, agent],
        [other.identity, other],
      ]);
      this.disconnectCalls = 0;
      this.published = [];
      this.receipts = [];
      this.localParticipant = {
        identity: "candidate-startup",
        publishTrack: async () => {},
        publishData: async (data, options) => {
          this.published.push(data);
          const message = JSON.parse(new TextDecoder().decode(data));
          if (message.type === "startup_received") {
            this.receipts.push({ message, options });
            if (this.holdReceipt)
              await new Promise((resolve) => {
                this.releaseReceipt = resolve;
              });
          }
        },
      };
      window.testRoom = this;
    }
    on(event, callback) {
      const callbacks = this.handlers.get(event) || [];
      callbacks.push(callback);
      this.handlers.set(event, callbacks);
      return this;
    }
    off(event, callback) {
      this.handlers.set(
        event,
        (this.handlers.get(event) || []).filter(
          (handler) => handler !== callback,
        ),
      );
      return this;
    }
    async connect() {
      if (window.testDepartDuringConnect) await this.depart();
    }
    async disconnect() {
      this.disconnectCalls += 1;
    }
    emit(event, ...args) {
      return Promise.all(
        (this.handlers.get(event) || []).map((callback) => callback(...args)),
      );
    }
    startup(message, topic, identity = agent.identity) {
      return this.emit(
        "DataReceived",
        new TextEncoder().encode(JSON.stringify(message)),
        this.remoteParticipants.get(identity),
        undefined,
        topic,
      );
    }
    depart(identity = agent.identity) {
      const participant = this.remoteParticipants.get(identity);
      this.remoteParticipants.delete(identity);
      return this.emit("ParticipantDisconnected", participant);
    }
  }
  window.LivekitClient = {
    Room,
    RoomEvent: new Proxy({}, { get: (_target, name) => name }),
    Track: { Source: { Microphone: "microphone", Camera: "camera" } },
  };
}

async function joinedPage(
  t,
  {
    clock = false,
    departDuringConnect = false,
    recording = false,
    strict = false,
    setup = async () => {},
  } = {},
) {
  let page;
  let phase = "opening page";
  const errors = [];
  const events = [];
  t.after(async () => {
    try {
      if (t.passed === false || t.signal?.aborted || errors.length) {
        let deadline;
        try {
          const snapshot = page
            ? page
                .evaluate(() => ({
                  controls: Object.fromEntries(
                    [
                      "audio-check",
                      "audio-check-status",
                      "audio-check-join",
                      "audio-step-output",
                      "audio-step-mic",
                      "audio-step-camera",
                      "editor",
                      "end",
                      "pause",
                      "timer",
                      "ending-overlay",
                      "presence-banner",
                    ].map((id) => {
                      const node = document.getElementById(id);
                      return [
                        id,
                        node && {
                          hidden: node.hidden,
                          disabled: node.disabled,
                          classes: node.className,
                          text: node.matches("textarea")
                            ? undefined
                            : node.textContent.slice(0, 250),
                        },
                      ];
                    }),
                  ),
                  captures: window.testCaptures,
                  tracks: window.testMediaTracks?.map((track) => ({
                    kind: track.kind,
                    state: track.readyState,
                    enabled: track.enabled,
                    muted: track.muted,
                  })),
                  room: window.testRoom && {
                    disconnects: window.testRoom.disconnectCalls,
                    published: window.testRoom.published.length,
                    receipts: window.testRoom.receipts.length,
                  },
                }))
                .catch((error) => ({ unavailable: error.message }))
            : Promise.resolve(null);
          const dom = await Promise.race([
            snapshot,
            new Promise((resolve) => {
              deadline = setTimeout(
                () => resolve({ unavailable: "snapshot exceeded 2 seconds" }),
                2000,
              );
            }),
          ]);
          t.diagnostic(JSON.stringify({ phase, errors, events, dom }));
        } finally {
          clearTimeout(deadline);
        }
      }
      assert.deepEqual(errors, []);
    } finally {
      await page?.close();
    }
  });
  page = await browser.newPage();
  page.on("pageerror", (error) => errors.push(error.message));
  const note = (event) => {
    events.push(event);
    if (events.length > 10) events.shift();
  };
  page.on("console", (message) => {
    if (["error", "warning"].includes(message.type()))
      note({ console: message.text().slice(0, 250) });
  });
  page.on("requestfailed", (request) =>
    note({
      path: new URL(request.url()).pathname,
      failure: request.failure()?.errorText,
    }),
  );
  const clockStart = new Date("2030-01-01T00:00:00Z");
  if (clock) await page.clock.install({ time: clockStart });
  await page.addInitScript(
    ({ departDuringConnect }) => {
      window.testDepartDuringConnect = departDuringConnect;
      // Keep native capture and preflight, without depending on downloaded
      // vision models while exercising startup event ordering.
      window.FaceDetector = class {
        async detect() {
          return [{}];
        }
      };
      window.testMediaTracks = [];
      window.testCaptures = [];
      const capture = navigator.mediaDevices.getUserMedia.bind(
        navigator.mediaDevices,
      );
      navigator.mediaDevices.getUserMedia = async (...args) => {
        const entry = {
          audio: Boolean(args[0]?.audio),
          video: Boolean(args[0]?.video),
          state: "pending",
        };
        window.testCaptures.push(entry);
        try {
          const stream = await capture(...args);
          entry.state = "resolved";
          window.testMediaTracks.push(...stream.getTracks());
          return stream;
        } catch (error) {
          entry.state = error.name;
          throw error;
        }
      };
    },
    { departDuringConnect },
  );
  if (recording) {
    await page.route("**/runtime-config.js", (route) =>
      route.fulfill({
        contentType: "text/javascript",
        body: `${DEFAULT_RUNTIME_CONFIG}\nglobalThis.CODETRIAL_RECORDING_ENABLED = true;\nglobalThis.CODETRIAL_CONSENT_VERSION = "fixture";`,
      }),
    );
    await page.route("**/api/interviews", (route) =>
      route.fulfill({
        json: { interviewId: "startup-recording" },
      }),
    );
    await page.route("**/api/interviews/startup-recording/replay", (route) =>
      route.fulfill({ json: {} }),
    );
  }
  await page.route("**/vendor/livekit-client.js", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body: `(${sdkFixture.toString()})();`,
    }),
  );
  // Readiness must also work in voice-only mode. Loading a VRM and software
  // WebGL here made Join clicks compete with rendering unrelated to this suite.
  await page.route("**/avatar/model.js", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body: `export async function loadAvatarModel() {
      throw new Error("Startup fixture uses voice-only mode");
    }`,
    }),
  );
  await page.route("**/api/token", (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        serverUrl: "wss://example.invalid",
        token: "fixture-token",
        durationMin: 45,
        ...(strict
          ? {
              problemPage: "chargeback-pair-match",
              previousAssignment: "fixture-assignment",
            }
          : {}),
      }),
    }),
  );
  await setup(page);
  phase = "loading interview";
  await page.goto(
    `${base}/interview.html?${strict ? "selection=strict" : "problem=chargeback-pair-match"}&duration=45`,
  );
  phase = "media preflight";
  if (!recording) await page.locator("#camera-skip").click();
  await page.locator("#audio-heard").click();
  if (recording) await page.locator("#recording-consent").check();
  phase = "waiting for Join readiness";
  await page.waitForFunction(
    () => !document.querySelector("#audio-check-join").disabled,
  );
  phase = "clicking Join";
  await page.locator("#audio-check-join").click();
  phase = "waiting for room publication";
  await page.waitForFunction(() => window.testRoom?.published.length > 0);
  phase = "startup scenario after room publication";
  return page;
}

async function warnsOnLeave(page) {
  return page.evaluate(() => {
    const event = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(event);
    return event.defaultPrevented;
  });
}

async function assertPendingControls(page) {
  assert.equal(await page.locator("#end").isDisabled(), true);
  assert.equal(await page.locator("#pause").isDisabled(), true);
  const result = await page.evaluate(() => {
    document.querySelector("#end").click();
    document.querySelector("#pause").click();
    return {
      ending: !document.querySelector("#ending-overlay").hidden,
      paused: document.querySelector("#pause").textContent.trim() !== "Pause",
      endRequests: window.testRoom.published
        .map((bytes) => JSON.parse(new TextDecoder().decode(bytes)))
        .filter((message) => message.type === "end_interview").length,
    };
  });
  assert.deepEqual(result, { ending: false, paused: false, endRequests: 0 });
}

test(
  "Strict startup failure does not promise offline editing or start the clock",
  { timeout: 15_000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await joinedPage(t, { strict: true });
    await assertPendingControls(page);
    const timer = await page.locator("#timer").textContent();
    await page.locator("#editor").fill("// Work while waiting for readiness");
    assert.equal(await warnsOnLeave(page), true);
    await page.evaluate(
      (topic) =>
        window.testRoom.startup(
          {
            type: "interviewer_state",
            reconnecting: false,
            startup: "failed",
            reason: "billing",
          },
          topic,
        ),
      topics.control,
    );
    await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
    await page.waitForFunction(
      () => !document.querySelector("#audio-check").hidden,
    );
    const message = await page.locator("#audio-check-status").textContent();
    assert.match(message, /billing/);
    assert.match(message, /lobby or retry media preflight/);
    assert.doesNotMatch(message, /work the problem offline/);
    assert.equal(await page.locator("#editor").isDisabled(), true);
    assert.equal(await page.locator("#run-tests").isDisabled(), true);
    assert.equal(await page.locator("#end").isDisabled(), true);
    assert.equal(await page.locator("#pause").isDisabled(), true);
    assert.equal(await page.locator("#timer").textContent(), timer);
    assert.equal(await warnsOnLeave(page), true);
  },
);

test(
  "editable startup work and untimed offline work both retain leave protection",
  { timeout: 15_000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await joinedPage(t);
    await assertPendingControls(page);
    await page.locator("#editor").fill("// Work written before Gemini answers");
    assert.equal(await warnsOnLeave(page), true);
    const timer = await page.locator("#timer").textContent();
    await page.evaluate(
      (topic) =>
        window.testRoom.startup(
          {
            type: "interviewer_state",
            reconnecting: false,
            startup: "failed",
            reason: "billing",
          },
          topic,
        ),
      topics.control,
    );
    await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
    await page.locator("#editor").fill("// Still working offline");
    assert.equal(await warnsOnLeave(page), true);
    assert.equal(await page.locator("#timer").textContent(), timer);
    assert.equal(await page.locator("#end").isDisabled(), false);
    assert.equal(await page.locator("#pause").isDisabled(), false);
    assert.equal(await page.locator("#ending-overlay").isVisible(), false);
  },
);

test(
  "departure before the room is assigned ends readiness without waiting for its timeout",
  { timeout: 15_000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await joinedPage(t, { departDuringConnect: true });
    await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
    assert.match(
      await page.locator("#presence-banner").textContent(),
      /timer has not started/,
    );
    assert.equal(await warnsOnLeave(page), true);
    const media = await page.evaluate(() =>
      window.testMediaTracks.map((track) => track.readyState),
    );
    assert.ok(media.length > 0);
    assert.ok(media.every((ready) => ready === "ended"));
  },
);

test(
  "startup failure releases local media while the recording response is still pending",
  { timeout: 15_000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    let recordingRoute;
    let requested;
    const recordingRequested = new Promise((resolve) => {
      requested = resolve;
    });
    const page = await joinedPage(t, {
      recording: true,
      setup: async (page) => {
        await page.route(
          "**/api/interviews/startup-recording/recording",
          (route) => {
            recordingRoute = route;
            requested();
          },
        );
      },
    });
    await recordingRequested;
    await page.evaluate(
      (topic) =>
        window.testRoom.startup(
          {
            type: "interviewer_state",
            reconnecting: false,
            startup: "failed",
            reason: "quota",
          },
          topic,
        ),
      topics.control,
    );
    await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
    assert.match(await page.locator("#presence-banner").textContent(), /quota/);
    const media = await page.evaluate(() =>
      window.testMediaTracks.map((track) => track.readyState),
    );
    assert.ok(media.length > 0);
    assert.ok(media.every((ready) => ready === "ended"));
    assert.equal(await warnsOnLeave(page), true);
    await recordingRoute.fulfill({ json: { state: "ready" } });
    await page.waitForFunction(() =>
      document.querySelector("#recording-state").textContent.includes("saved"),
    );
    assert.match(await page.locator("#presence-banner").textContent(), /quota/);
    assert.equal(await page.evaluate(() => window.testRoom.disconnectCalls), 1);
  },
);

test("the vendored SDK delivers a post-departure packet without its sender", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(
    `${base}/interview.html?problem=chargeback-pair-match&duration=45`,
  );
  const events = await page.evaluate(async () => {
    const sdk = window.LivekitClient;
    const room = new sdk.Room();
    const agent = {
      identity: "agent-ordering",
      trackPublications: new Map(),
      emit() {},
      setDisconnected() {},
    };
    room.remoteParticipants.set(agent.identity, agent);
    const seen = [];
    room.on(sdk.RoomEvent.DataReceived, (_payload, sender) =>
      seen.push({ event: "data", sender: sender?.identity ?? null }),
    );
    room.on(sdk.RoomEvent.ParticipantDisconnected, (sender) =>
      seen.push({ event: "departure", sender: sender.identity }),
    );
    const packet = {
      participantIdentity: agent.identity,
      kind: 0,
      value: {
        case: "user",
        value: { payload: new Uint8Array(), topic: "control" },
      },
    };
    room.handleDataPacket(packet);
    room.handleParticipantDisconnected(agent.identity, agent);
    room.handleDataPacket(packet);
    await room.disconnect();
    return seen;
  });
  assert.deepEqual(events, [
    { event: "data", sender: "agent-ordering" },
    { event: "departure", sender: "agent-ordering" },
    { event: "data", sender: null },
  ]);
});

for (const reason of ["billing", "quota", "unavailable"]) {
  test(
    `offline practice can pause and resume after startup ${reason}`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t, { clock: true });
      const waitingTimer = await page.locator("#timer").textContent();
      await page.evaluate(
        ({ reason, topic }) =>
          window.testRoom.startup(
            {
              type: "interviewer_state",
              reconnecting: false,
              startup: "failed",
              reason,
            },
            topic,
          ),
        { reason, topic: topics.control },
      );
      await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
      const banner = await page.locator("#presence-banner").textContent();
      assert.match(banner, /timer has not started/);

      await page.locator("#pause").click();
      await page.waitForFunction(
        () => document.querySelector("#editor").disabled,
      );
      assert.equal(await page.locator("#ending-overlay").isVisible(), false);
      await page.clock.fastForward(46 * 60 * 1000);
      await page.locator("#pause").click();
      await page.waitForFunction(
        () => !document.querySelector("#editor").disabled,
      );
      await page.locator("#editor").fill("// Offline practice after resume");
      assert.equal(await page.locator("#pause").textContent(), "Pause");
      assert.equal(await page.locator("#ending-overlay").isVisible(), false);
      assert.equal(await page.locator("#timer").textContent(), waitingTimer);
      assert.equal(
        await page.locator("#presence-banner").textContent(),
        banner,
      );
      assert.equal(
        await page.evaluate(() => window.testRoom.disconnectCalls),
        1,
      );
    },
  );
}

test(
  "a ready interview counts down and expires even while paused",
  { timeout: 15_000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await joinedPage(t, { clock: true });
    // Let media preflight run before freezing wall-clock drift. Readiness and
    // interview time then advance only when the test moves the clock.
    await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
    await assertPendingControls(page);
    await page.evaluate(
      (topic) =>
        window.testRoom.startup(
          {
            type: "interviewer_state",
            reconnecting: false,
            startup: "ready",
          },
          topic,
        ),
      topics.control,
    );
    await page.waitForFunction(
      () => document.querySelector("#timer").textContent === "45:00",
    );
    assert.equal(await page.locator("#end").isDisabled(), false);
    assert.equal(await page.locator("#pause").isDisabled(), false);
    await page.clock.fastForward(60_000);
    assert.equal(await page.locator("#timer").textContent(), "44:00");
    await page.locator("#pause").click();
    await page.evaluate(
      (topic) =>
        window.testRoom.startup({ type: "pause_state", paused: true }, topic),
      topics.control,
    );
    await page.waitForFunction(
      () => document.querySelector("#editor").disabled,
    );
    await page.clock.fastForward(60_000);
    assert.equal(await page.locator("#timer").textContent(), "43:00");
    assert.equal(await page.locator("#ending-overlay").isVisible(), false);
    await page.clock.fastForward(43 * 60 * 1000);
    await page.waitForFunction(
      () => !document.querySelector("#ending-overlay").hidden,
    );
    const endings = await page.evaluate(() =>
      window.testRoom.published
        .map((bytes) => JSON.parse(new TextDecoder().decode(bytes)))
        .filter((message) => message.type === "end_interview"),
    );
    assert.equal(endings.length, 1);
    assert.equal(endings[0].reason, "time_up");
    assert.equal(await page.locator("#timer").textContent(), "00:00");
  },
);

for (const reason of ["billing", "quota"]) {
  test(
    `a received ${reason} failure survives departure while its receipt is flushing`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t);
      const waitingTimer = await page.locator("#timer").textContent();
      await page.evaluate(
        ({ reason, topic }) => {
          window.testRoom.holdReceipt = true;
          return window.testRoom.startup(
            {
              type: "interviewer_state",
              reconnecting: false,
              startup: "failed",
              reason,
            },
            topic,
          );
        },
        { reason, topic: topics.control },
      );
      await page.waitForFunction(() => !!window.testRoom.releaseReceipt);
      assert.equal(
        await page.evaluate(() => window.testRoom.disconnectCalls),
        0,
        "cleanup must give the receipt its bounded chance",
      );
      const receipt = await page.evaluate(() => window.testRoom.receipts[0]);
      assert.equal(receipt.options.topic, topics.control);
      assert.equal(receipt.options.reliable, true);
      assert.deepEqual(receipt.options.destinationIdentities, [
        "agent-startup",
      ]);
      await page.evaluate(() => window.testRoom.depart());
      await page.evaluate(() => window.testRoom.releaseReceipt());
      await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
      const banner = await page.locator("#presence-banner").textContent();
      assert.match(banner, new RegExp(reason));
      assert.match(banner, /timer has not started/);
      assert.equal(await page.locator("#timer").textContent(), waitingTimer);
      await page.evaluate(() => window.testRoom.emit("Disconnected"));
      assert.equal(
        await page.locator("#presence-banner").textContent(),
        banner,
      );
      assert.equal(
        await page.evaluate(() => window.testRoom.disconnectCalls),
        1,
      );
    },
  );
}

test(
  "a permanently stalled startup receipt cannot hold cleanup",
  { timeout: 15_000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await joinedPage(t, { clock: true });
    const waitingTimer = await page.locator("#timer").textContent();
    await page.evaluate((topic) => {
      window.testRoom.holdReceipt = true;
      return window.testRoom.startup(
        {
          type: "interviewer_state",
          reconnecting: false,
          startup: "failed",
          reason: "billing",
        },
        topic,
      );
    }, topics.control);
    await page.waitForFunction(() => !!window.testRoom.releaseReceipt);
    assert.equal(await page.evaluate(() => window.testRoom.disconnectCalls), 0);
    await page.clock.fastForward(1001);
    await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
    assert.match(
      await page.locator("#presence-banner").textContent(),
      /billing/,
    );
    assert.equal(await page.locator("#timer").textContent(), waitingTimer);
    await page.evaluate(() => window.testRoom.releaseReceipt());
    await page.clock.fastForward(1000);
    assert.equal(await page.evaluate(() => window.testRoom.disconnectCalls), 1);
  },
);

for (const late of [false, true]) {
  test(
    `departure with ${late ? "an unattributed late" : "no"} failure notice ends startup once`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t);
      const waitingTimer = await page.locator("#timer").textContent();
      await page.evaluate(() => window.testRoom.depart());
      if (late)
        await page.evaluate(
          (topic) =>
            window.testRoom.startup(
              {
                type: "interviewer_state",
                reconnecting: false,
                startup: "failed",
                reason: "billing",
              },
              topic,
            ),
          topics.control,
        );
      await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
      const banner = await page.locator("#presence-banner").textContent();
      assert.match(banner, /timer has not started/);
      assert.doesNotMatch(banner, /billing|quota/);
      assert.equal(await page.locator("#timer").textContent(), waitingTimer);
      assert.equal(
        await page.evaluate(() => window.testRoom.receipts.length),
        0,
      );
      await page.evaluate(() => window.testRoom.emit("Disconnected"));
      assert.equal(
        await page.locator("#presence-banner").textContent(),
        banner,
      );
      assert.equal(
        await page.evaluate(() => window.testRoom.disconnectCalls),
        1,
      );
    },
  );
}

test(
  "a permanently missing startup notice expires without a countdown",
  { timeout: 15_000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await joinedPage(t, { clock: true });
    const waitingTimer = await page.locator("#timer").textContent();
    await page.clock.fastForward(180_000);
    await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
    assert.match(
      await page.locator("#presence-banner").textContent(),
      /timer has not started/,
    );
    assert.equal(await page.locator("#timer").textContent(), waitingTimer);
    await page.clock.fastForward(180_000);
    assert.equal(await page.evaluate(() => window.testRoom.disconnectCalls), 1);
  },
);

for (const startup of ["ready", "failed"]) {
  test(
    `the other agent's startup ${startup} cannot settle the pinned interviewer's gate`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t);
      const waitingTimer = await page.locator("#timer").textContent();
      await page.evaluate(
        ({ startup, topic }) =>
          window.testRoom.startup(
            {
              type: "interviewer_state",
              reconnecting: false,
              startup,
              reason: "billing",
            },
            topic,
            "agent-other",
          ),
        { startup, topic: topics.control },
      );
      assert.equal(
        await page.evaluate(() => window.testRoom.disconnectCalls),
        0,
      );
      assert.equal(await page.locator("#timer").textContent(), waitingTimer);
      await page.evaluate(
        ({ startup, topic }) =>
          window.testRoom.startup(
            {
              type: "interviewer_state",
              reconnecting: false,
              startup: startup === "ready" ? "failed" : "ready",
              reason: "quota",
            },
            topic,
          ),
        { startup, topic: topics.control },
      );
      if (startup === "ready") {
        await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
        assert.match(
          await page.locator("#presence-banner").textContent(),
          /quota/,
        );
        assert.equal(await page.locator("#timer").textContent(), waitingTimer);
      } else {
        await page.waitForFunction(
          (waiting) => document.querySelector("#timer").textContent !== waiting,
          waitingTimer,
        );
        assert.equal(
          await page.evaluate(() => window.testRoom.disconnectCalls),
          0,
        );
      }
    },
  );
}

for (const reason of ["billing", "quota"]) {
  test(
    `a delayed startup disconnect preserves the ${reason} banner`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t);
      await page.evaluate(
        ({ reason, topic }) =>
          window.testRoom.startup(
            {
              type: "interviewer_state",
              reconnecting: false,
              startup: "failed",
              reason,
            },
            topic,
          ),
        { reason, topic: topics.control },
      );
      await page.waitForFunction(
        (reason) =>
          document
            .querySelector("#presence-banner")
            .textContent.includes(reason),
        reason,
      );
      const before = await page.locator("#presence-banner").textContent();
      assert.equal(
        await page.evaluate(() => window.testRoom.disconnectCalls),
        1,
      );
      await page.evaluate(() => window.testRoom.emit("Disconnected"));
      assert.equal(
        await page.locator("#presence-banner").textContent(),
        before,
      );
      assert.match(before, /timer has not started/);
    },
  );
}

test(
  "a real disconnect after readiness keeps the connection-drop banner",
  { timeout: 15_000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await joinedPage(t);
    const waitingTimer = await page.locator("#timer").textContent();
    await page.evaluate(
      (topic) =>
        window.testRoom.startup(
          {
            type: "interviewer_state",
            reconnecting: false,
            startup: "ready",
          },
          topic,
        ),
      topics.control,
    );
    await page.waitForFunction(
      (waiting) => document.querySelector("#timer").textContent !== waiting,
      waitingTimer,
    );
    await page.evaluate(() => window.testRoom.emit("Disconnected"));
    assert.match(
      await page.locator("#presence-banner").textContent(),
      /provider could not be reached/,
    );
  },
);
