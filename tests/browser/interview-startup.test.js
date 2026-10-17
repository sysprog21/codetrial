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
  class Room {
    constructor() {
      this.handlers = new Map();
      this.remoteParticipants = new Map([[agent.identity, agent]]);
      this.disconnectCalls = 0;
      this.published = [];
      this.localParticipant = {
        identity: "candidate-startup",
        publishTrack: async () => {},
        publishData: async (data) => {
          this.published.push(data);
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
    async connect() {}
    async disconnect() {
      this.disconnectCalls += 1;
    }
    emit(event, ...args) {
      return Promise.all(
        (this.handlers.get(event) || []).map((callback) => callback(...args)),
      );
    }
    startup(message, topic) {
      return this.emit(
        "DataReceived",
        new TextEncoder().encode(JSON.stringify(message)),
        agent,
        undefined,
        topic,
      );
    }
  }
  window.LivekitClient = {
    Room,
    RoomEvent: new Proxy({}, { get: (_target, name) => name }),
    Track: { Source: { Microphone: "microphone", Camera: "camera" } },
  };
}

async function joinedPage(t) {
  const page = await browser.newPage();
  t.after(() => page.close());
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  t.after(() => assert.deepEqual(errors, []));
  await page.route("**/vendor/livekit-client.js", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body: `(${sdkFixture.toString()})();`,
    }),
  );
  await page.route("**/api/token", (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        serverUrl: "wss://example.invalid",
        token: "fixture-token",
        durationMin: 45,
      }),
    }),
  );
  await page.goto(
    `${base}/interview.html?problem=chargeback-pair-match&duration=45`,
  );
  await page.locator("#camera-skip").click();
  await page.locator("#audio-heard").click();
  await page.waitForFunction(
    () => !document.querySelector("#audio-check-join").disabled,
  );
  await page.locator("#audio-check-join").click();
  await page.waitForFunction(() => window.testRoom?.published.length > 0);
  return page;
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
      () => document.querySelector("#timer").textContent !== "45:00",
    );
    await page.evaluate(() => window.testRoom.emit("Disconnected"));
    assert.match(
      await page.locator("#presence-banner").textContent(),
      /provider could not be reached/,
    );
  },
);
