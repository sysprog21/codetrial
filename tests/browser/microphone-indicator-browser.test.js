import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  FAKE_SPEECH_CAPTURE,
  launchChromium,
  read,
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

// Exercise the real preflight and page handlers without a server or agent.
// The fake room exposes only its published track and participant events.
const ROOM = `
window.LivekitClient = {
  RoomEvent: new Proxy({}, { get: (_, name) => String(name) }),
  Track: { Source: { Microphone: "microphone", Camera: "camera" } },
  Room: class {
    constructor() {
      window.__room = this;
      this.handlers = {};
      this.remoteParticipants = new Map();
      this.localParticipant = {
        identity: "candidate",
        publishTrack: async (track) => {
          if (track.kind === "audio") window.__microphone = track;
        },
        publishData: async () => {},
        streamBytes: async () => ({ write: async () => {}, close: async () => {} }),
      };
    }
    on(name, handler) {
      (this.handlers[name] ||= []).push(handler);
      return this;
    }
    off() {
      return this;
    }
    registerTextStreamHandler() {}
    async connect() {}
    async disconnect() {}
  },
};`;

// Traces every clone the page takes, and three ways a track reaches Jim, the
// room or the recorder: the streams `getUserMedia` returns (the interview's,
// which the recorder reads), `addTrack`, and the room's publication. The
// `MediaStream` constructor is not traced: the watch itself wraps the clone in
// one to hand it to its level source, which is how the clone is read at all.
const TRACE_TRACKS = `
window.__clones = [];
window.__streams = [];
window.__added = [];
const clone = MediaStreamTrack.prototype.clone;
MediaStreamTrack.prototype.clone = function () {
  const copy = clone.call(this);
  window.__clones.push(copy);
  return copy;
};
const getUserMedia = MediaDevices.prototype.getUserMedia;
MediaDevices.prototype.getUserMedia = async function (...args) {
  const stream = await getUserMedia.apply(this, args);
  window.__streams.push(stream);
  return stream;
};
const addTrack = MediaStream.prototype.addTrack;
MediaStream.prototype.addTrack = function (track) {
  window.__added.push(track);
  return addTrack.call(this, track);
};`;

// Records the audio time of every level the page's worklets post, as the
// page's own handler receives it.
const TRACE_LEVELS = `
window.__heard = [];
const Node = window.AudioWorkletNode;
window.AudioWorkletNode = class extends Node {
  constructor(...args) {
    super(...args);
    this.port.addEventListener("message", ({ data }) => window.__heard.push(data.at));
  }
};`;

async function connectedPage(
  t,
  { remindWhileMuted = false, traceTracks = false, realPage = false } = {},
) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(ROOM);
  if (traceTracks) await page.addInitScript(TRACE_TRACKS);
  await page.route("**/vendor/livekit-client.js", (route) =>
    route.fulfill({ contentType: "text/javascript", body: "" }),
  );
  // Face detection and the avatar stall headless Chromium's main thread for
  // seconds after joining, which every test here waits through and none is
  // about, so by default the page runs neither: the detector reports itself
  // unavailable and the avatar model never arrives. `realPage` keeps both.
  if (!realPage) {
    await page.route("**/face-presence.js", (route) =>
      route.fulfill({
        contentType: "text/javascript",
        body:
          read("web/face-presence.js").replace(
            "export async function createFacePresenceDetector(",
            "async function unusedFacePresenceDetector(",
          ) +
          "\nexport async function createFacePresenceDetector() {\n  return { available: false, close() {} };\n}\n",
      }),
    );
    await page.route("**/*.vrm", (route) => route.abort());
  }
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
  if (remindWhileMuted) await page.check("#muted-speech-consent");
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
      const agent = {
        identity: "interviewer-test",
        attributes: { "lk.agent.state": state },
      };
      window.__room.remoteParticipants.set(agent.identity, agent);
      for (const handler of window.__room.handlers.ParticipantAttributesChanged)
        handler({}, agent);
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

// The fake capture plays speech on a loop, so a muted microphone is one a
// candidate is talking into.
test("speaking while muted reminds once consented, and unmuting clears it", async (t) => {
  const context = await connectedPage(t, { remindWhileMuted: true });
  if (!context) return;
  const { page, errors } = context;
  const reminder = page.locator("#muted-speech-reminder");
  await page.locator("#mic").click();
  await reminder.waitFor({ state: "visible", timeout: 10000 });
  // What the room was given is still the muted original, not the clone.
  assert.equal(await page.evaluate(() => window.__microphone.enabled), false);

  await page.locator("#mic").click();
  await reminder.waitFor({ state: "hidden" });
  assert.equal(await page.evaluate(() => window.__microphone.enabled), true);
  assert.deepEqual(errors, []);
});

// A status region is read whole whenever its text changes, so the reminder and
// the Mic status each need their own, or a change to one re-reads the other.
test("the reminder and the Mic status are announced from separate regions", async (t) => {
  const context = await connectedPage(t);
  if (!context) return;
  const { page } = context;
  assert.deepEqual(
    await page.evaluate(() => {
      const status = document.querySelector("#mic-control-status");
      const reminder = document.querySelector("#muted-speech-reminder");
      const regions = [status, reminder].map((it) =>
        it.closest("[role=status]"),
      );
      return [
        regions.every(Boolean),
        regions[0] === regions[1],
        regions[0].contains(reminder),
        regions[1].contains(status),
      ];
    }),
    [true, false, false, false],
  );
});

// Face detection and the avatar run here as on a real interview, sharing the
// main thread with everything else on the page. The level is measured on the
// audio thread, so they can only delay its messages, which carry their audio
// time with them. The fake capture's first loop holds enough speech within
// about 1.2 s of audio; the 30 s allow for a page that starts up the face model
// and the avatar on a loaded runner and handles the messages in one late burst.
test("speaking while muted reminds with face detection and the avatar running", async (t) => {
  const context = await connectedPage(t, {
    remindWhileMuted: true,
    realPage: true,
  });
  if (!context) return;
  const { page, errors } = context;
  await page.locator("#mic").click();
  await page
    .locator("#muted-speech-reminder")
    .waitFor({ state: "visible", timeout: 30000 });
  assert.equal(await page.evaluate(() => window.__microphone.enabled), false);
  assert.deepEqual(errors, []);
});

test("without consent a muted microphone is not listened to", async (t) => {
  const context = await connectedPage(t, { traceTracks: true });
  if (!context) return;
  const { page, errors } = context;
  await page.locator("#mic").click();
  await page.locator("#mic-muted-status").waitFor({ state: "visible" });
  // The watch clones in the task that paints Muted, so a clone taken without
  // consent would already be here.
  assert.equal(await page.evaluate(() => window.__clones.length), 0);
  assert.equal(await page.locator("#muted-speech-reminder").isVisible(), false);
  assert.deepEqual(errors, []);
});

/// Jim speaking keeps the detector from firing, so the watch stays running
/// for as long as a test needs it to.
async function jimSpeaks(page) {
  await page.evaluate(() => {
    const agent = {
      identity: "interviewer-test",
      attributes: { "lk.agent.state": "speaking" },
    };
    window.__room.remoteParticipants.set(agent.identity, agent);
    for (const handler of window.__room.handlers.ParticipantAttributesChanged)
      handler({}, agent);
  });
}

async function mutedAndWatched(page) {
  await jimSpeaks(page);
  await page.locator("#mic").click();
  await page.locator("#mic-muted-status").waitFor({ state: "visible" });
  await page.waitForFunction(() =>
    window.__clones.some((clone) => clone.readyState === "live"),
  );
}

test("the clone stays out of the stream, the room and the recorder, and pagehide releases it", async (t) => {
  const context = await connectedPage(t, {
    remindWhileMuted: true,
    traceTracks: true,
  });
  if (!context) return;
  const { page, errors } = context;
  await mutedAndWatched(page);
  const reached = await page.evaluate(() =>
    window.__clones.filter(
      (clone) =>
        clone === window.__microphone ||
        window.__added.includes(clone) ||
        window.__streams.some((stream) => stream.getTracks().includes(clone)),
    ),
  );
  assert.equal(reached.length, 0);
  assert.equal(await page.evaluate(() => window.__microphone.enabled), false);

  // Read in the same task as the event, so the watch's own check of the input
  // at the next level cannot be what releases the clone.
  assert.deepEqual(
    await page.evaluate(() => {
      window.dispatchEvent(new PageTransitionEvent("pagehide"));
      return window.__clones.map((c) => c.readyState);
    }),
    ["ended"],
  );
  assert.deepEqual(errors, []);
});

// A switch that strands the input stops it with `stop()`, which fires no
// `ended` and leaves the room in place, so only the watch can notice.
test("an input stopped without `ended` releases its clone", async (t) => {
  const context = await connectedPage(t, {
    remindWhileMuted: true,
    traceTracks: true,
  });
  if (!context) return;
  const { page, errors } = context;
  await mutedAndWatched(page);
  await page.evaluate(() => window.__microphone.stop());
  await page.waitForFunction(() =>
    window.__clones.every((clone) => clone.readyState === "ended"),
  );
  assert.deepEqual(errors, []);
});

// The fake capture loops 3.6 s of speech, so a watch that heard it through a
// whole loop without reminding was held by Jim speaking, and one that reminds
// once he stops was listening all along. Waited out in the audio time the
// watch has been handed, not in wall time: a busy page handles the levels late.
test("no reminder while Jim speaks, and one once he stops", async (t) => {
  const context = await connectedPage(t, {
    remindWhileMuted: true,
    traceTracks: true,
  });
  if (!context) return;
  const { page, errors } = context;
  await page.evaluate(TRACE_LEVELS);
  await mutedAndWatched(page);
  // The 3.6 s loop and a second more.
  await page.waitForFunction(
    () =>
      window.__heard.length &&
      window.__heard.at(-1) - window.__heard[0] >= 4600,
    null,
    { timeout: 30000 },
  );
  const reminder = page.locator("#muted-speech-reminder");
  assert.equal(await reminder.isVisible(), false);
  await page.evaluate(() => {
    const agent = window.__room.remoteParticipants.get("interviewer-test");
    agent.attributes = { "lk.agent.state": "listening" };
    for (const handler of window.__room.handlers.ParticipantAttributesChanged)
      handler({}, agent);
  });
  await reminder.waitFor({ state: "visible", timeout: 10000 });
  assert.deepEqual(errors, []);
});

// With an interviewer in the room, End waits for the report, and the watch
// would go on reading the microphone for as long as that takes.
test("ending the interview releases the watch", async (t) => {
  const context = await connectedPage(t, {
    remindWhileMuted: true,
    traceTracks: true,
  });
  if (!context) return;
  const { page, errors } = context;
  await mutedAndWatched(page);
  await page.locator("#end").click();
  await page.waitForFunction(() =>
    window.__clones.every((clone) => clone.readyState === "ended"),
  );
  assert.deepEqual(errors, []);
});

test("ending the interview hides the reminder", async (t) => {
  const context = await connectedPage(t, { remindWhileMuted: true });
  if (!context) return;
  const { page, errors } = context;
  const reminder = page.locator("#muted-speech-reminder");
  await page.locator("#mic").click();
  await reminder.waitFor({ state: "visible", timeout: 10000 });
  await page.locator("#end").click();
  await reminder.waitFor({ state: "hidden" });
  assert.deepEqual(errors, []);
});

// Local recording can keep the microphone live after the room drops, and a
// reminder already showing would go on telling the candidate that a Jim who
// is no longer there cannot hear them.
test("a dropped room hides the reminder even when the microphone stays live", async (t) => {
  const context = await connectedPage(t, {
    remindWhileMuted: true,
    traceTracks: true,
  });
  if (!context) return;
  const { page, errors } = context;
  const reminder = page.locator("#muted-speech-reminder");
  await page.locator("#mic").click();
  await reminder.waitFor({ state: "visible", timeout: 10000 });
  await page.evaluate(() => {
    window.__microphone.stop = () => {};
    for (const handler of window.__room.handlers.Disconnected) handler();
  });
  await reminder.waitFor({ state: "hidden" });
  assert.equal(
    await page.evaluate(() => window.__microphone.readyState),
    "live",
  );
  assert.deepEqual(errors, []);
});

// The phase stays live after a drop, so a later mute toggle would otherwise
// clone the live microphone again and remind about a room not there.
test("no watch starts again once the room has dropped", async (t) => {
  const context = await connectedPage(t, {
    remindWhileMuted: true,
    traceTracks: true,
  });
  if (!context) return;
  const { page, errors } = context;
  await mutedAndWatched(page);
  await page.evaluate(() => {
    window.__microphone.stop = () => {};
    for (const handler of window.__room.handlers.Disconnected) handler();
  });
  await page.waitForFunction(() =>
    window.__clones.every((clone) => clone.readyState === "ended"),
  );
  const clones = await page.evaluate(() => window.__clones.length);
  await page.locator("#mic").click();
  await page.locator("#mic-muted-status").waitFor({ state: "hidden" });
  await page.locator("#mic").click();
  await page.locator("#mic-muted-status").waitFor({ state: "visible" });
  assert.equal(
    await page.evaluate(() =>
      window.__clones.some((clone) => clone.readyState === "live"),
    ),
    false,
  );
  assert.equal(await page.evaluate(() => window.__clones.length), clones);
  assert.equal(await page.locator("#muted-speech-reminder").isVisible(), false);
  assert.deepEqual(errors, []);
});

// Browsers restore a checkbox across a reload or a history visit unless told
// not to, which would carry the consent into the next interview.
test("the consent is not carried into a revisited page", async (t) => {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.addInitScript(ROOM);
  await page.route("**/vendor/livekit-client.js", (route) =>
    route.fulfill({ contentType: "text/javascript", body: "" }),
  );
  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`);
  await page.check("#muted-speech-consent");
  await page.goto(`${base}/`);
  await page.goBack();
  await page.locator("#muted-speech-consent").waitFor({ state: "attached" });
  await page.waitForLoadState("load");
  assert.equal(await page.isChecked("#muted-speech-consent"), false);
});
