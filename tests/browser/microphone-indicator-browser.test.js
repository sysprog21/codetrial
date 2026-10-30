import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  FAKE_SPEECH_CAPTURE,
  fakeLiveKit,
  launchChromium,
  startStaticServer,
} from "./source.js";

let browser;
let server;
let base;

before(async () => {
  browser = await launchChromium({
    args: [
      "--use-fake-device-for-media-stream",
      "--use-fake-ui-for-media-stream",
      FAKE_SPEECH_CAPTURE,
    ],
  });
  if (browser) ({ server, base } = await startStaticServer());
});

after(async () => {
  await browser?.close();
  await new Promise((done) => (server ? server.close(done) : done()));
});

// A started interview needs the shared ready-room contract. Keep the real
// captured track so the native setter still exercises failed unmute and retry.
const ROOM = `${fakeLiveKit({ startupReady: true })}
window.LivekitClient.Room = class extends window.LivekitClient.Room {
  constructor() {
    super();
    window.__room = this;
    this.localParticipant.publishTrack = async (track) => {
      if (track.kind === "audio") window.__microphone = track;
    };
  }
};`;

async function connectedPage(t) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(ROOM);
  await page.route("**/vendor/livekit-client.js", (route) =>
    route.fulfill({ contentType: "text/javascript", body: "" }),
  );
  await page.route("**/api/token", (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        serverUrl: "wss://fake.invalid",
        token: "token",
        durationMin: 10,
      }),
    }),
  );
  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`);
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-input").placeholder !== "",
  );
  await page.getByRole("button", { name: "Play test tone" }).click();
  await page.getByRole("button", { name: "I heard it" }).click();
  await page.waitForFunction(
    () => !document.querySelector("#audio-check-join").disabled,
  );
  await page.click("#audio-check-join");
  await page.waitForFunction(
    () =>
      window.__microphone &&
      document.querySelector("#timer").textContent !== "00:00",
  );
  return { page, errors };
}

test("Space and Enter toggle the microphone without losing focus, including a failed unmute", async (t) => {
  const context = await connectedPage(t);
  if (!context) return;
  const { page, errors } = context;
  const mic = page.locator("#mic");
  await mic.focus();
  await page.keyboard.press("Space");
  await page.locator("#mic-muted-status").waitFor({ state: "visible" });
  assert.equal(
    await mic.getAttribute("aria-label"),
    "Muted: unmute microphone",
  );
  assert.equal(await page.evaluate(() => window.__microphone.enabled), false);

  // Model the device operation rejecting an unmute, rather than changing the
  // displayed state in the test. A retry goes through the same native setter.
  await page.evaluate(() => {
    const track = window.__microphone;
    const enabled = Object.getOwnPropertyDescriptor(
      MediaStreamTrack.prototype,
      "enabled",
    );
    window.__rejectUnmute = true;
    Object.defineProperty(track, "enabled", {
      get() {
        return enabled.get.call(this);
      },
      set(value) {
        if (value && window.__rejectUnmute)
          throw new Error("device unavailable");
        enabled.set.call(this, value);
      },
    });
  });
  await page.keyboard.press("Enter");
  await page.locator("#mic-control-status").waitFor({ state: "visible" });
  assert.match(
    await page.locator("#mic-control-status").innerText(),
    /still muted/,
  );
  assert.equal(await page.locator("#mic-muted-status").isVisible(), true);
  assert.equal(
    await mic.getAttribute("aria-label"),
    "Muted: unmute microphone",
  );
  assert.equal(await page.evaluate(() => document.activeElement.id), "mic");

  await page.evaluate(() => {
    window.__rejectUnmute = false;
  });
  await page.keyboard.press("Enter");
  await page.locator("#mic-muted-status").waitFor({ state: "hidden" });
  assert.equal(await mic.getAttribute("aria-label"), "Mic on: mute microphone");
  assert.equal(await page.evaluate(() => window.__microphone.enabled), true);
  assert.equal(await page.evaluate(() => document.activeElement.id), "mic");
  assert.equal(await page.locator("#mic-control-status").isVisible(), false);
  assert.deepEqual(errors, []);
});

test("mute remains visible beside changing interviewer states at narrow widths", async (t) => {
  const context = await connectedPage(t);
  if (!context) return;
  const { page, errors } = context;
  await page.locator("#mic").click();
  for (const [state, label] of [
    ["listening", "Listening"],
    ["thinking", "Thinking..."],
    ["speaking", "Speaking"],
  ]) {
    await page.evaluate((state) => {
      const agent = window.__room.remoteParticipants.get("agent-fake-room");
      agent.attributes["lk.agent.state"] = state;
      window.__handlers.ParticipantAttributesChanged({}, agent);
    }, state);
    assert.equal(await page.locator("#agent-state").innerText(), label);
    for (const width of [390, 320]) {
      await page.setViewportSize({ width, height: 844 });
      for (const selector of ["#mic-muted-status", "#mic"]) {
        assert.equal(await page.locator(selector).isVisible(), true);
        const box = await page.locator(selector).boundingBox();
        assert.ok(box && box.x >= 0 && box.x + box.width <= width);
      }
    }
  }
  // The normal first-interviewer hint closes itself; it must not prevent the
  // mute control from being reached by keyboard once the workspace is usable.
  await page.locator("#framework-hint").waitFor({ state: "hidden" });
  await page.locator("#mic").focus();
  await page.keyboard.press("Tab");
  await page.keyboard.press("Shift+Tab");
  assert.equal(await page.evaluate(() => document.activeElement.id), "mic");
  await page.keyboard.press("Enter");
  await page.locator("#mic-muted-status").waitFor({ state: "hidden" });
  assert.deepEqual(errors, []);
});
