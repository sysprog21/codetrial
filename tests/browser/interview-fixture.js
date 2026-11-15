import assert from "node:assert/strict";

// Real MediaStream tracks with deterministic microphone samples exercise the
// existing readiness gate without depending on headless audio scheduling.
// The room transport is fake; reload still discards the whole page state.
export function mediaAndRoom() {
  const NativeAudioContext = window.AudioContext;
  let sink;
  window.AudioContext = class extends NativeAudioContext {
    createAnalyser() {
      const analyser = super.createAnalyser();
      analyser.getByteTimeDomainData = (samples) => {
        for (let index = 0; index < samples.length; index++)
          samples[index] = index % 2 ? 200 : 60;
      };
      return analyser;
    }
  };
  // Every track handed out, so a test can check the page released them.
  window.acquiredTracks = [];
  const handOut = (stream) => {
    window.acquiredTracks.push(...stream.getTracks());
    return stream;
  };
  navigator.mediaDevices.getUserMedia = async (constraints) => {
    if (constraints.video) {
      if (globalThis.CODETRIAL_RECORDING_ENABLED !== true)
        throw new DOMException("No camera", "NotFoundError");
      const canvas = document.createElement("canvas");
      canvas.getContext("2d").fillRect(0, 0, 1, 1);
      return handOut(canvas.captureStream());
    }
    sink ??= new NativeAudioContext();
    return handOut(sink.createMediaStreamDestination().stream);
  };
  window.roomJoins = [];
  window.roomEvents = {};
  window.LivekitClient = {
    RoomEvent: new Proxy({}, { get: (_, key) => key }),
    Room: class {
      agent = { identity: "agent-test", kind: "AGENT", attributes: {} };
      remoteParticipants = new Map([[this.agent.identity, this.agent]]);
      // Each publish's topic is recorded, and a test that sets
      // `window.roomPublishHeld` to a promise holds every publish there.
      // A test that sets `window.roomTrackHeld` to a promise holds each track
      // publish there, after the room is connected but before it is claimed,
      // and a promise that rejects fails the publish.
      localParticipant = {
        publishTrack: async () => {
          await window.roomTrackHeld;
        },
        publishData: async (_bytes, options) => {
          (window.roomPublishes ??= []).push(options?.topic);
          await window.roomPublishHeld;
          (window.roomPublicationSettled ??= []).push(options?.topic);
          if (options.topic !== "integrity" || this.ready) return;
          this.ready = true;
          const payload = new TextEncoder().encode(
            JSON.stringify({
              type: "interviewer_state",
              reconnecting: false,
              startup: "ready",
            }),
          );
          for (const handler of window.roomEvents.DataReceived ?? [])
            handler(payload, this.agent, undefined, "control");
        },
      };
      on(event, callback) {
        (window.roomEvents[event] ??= []).push(callback);
        return this;
      }
      off(event, callback) {
        window.roomEvents[event] = (window.roomEvents[event] ?? []).filter(
          (handler) => handler !== callback,
        );
        return this;
      }
      // A test that sets `window.roomJoinHeld` to a promise holds the join
      // there, so it can act while the page is still joining.
      async connect(url, token) {
        window.roomJoins.push({ url, token });
        await window.roomJoinHeld;
      }
      async disconnect() {
        window.roomLeaves = (window.roomLeaves ?? 0) + 1;
      }
    },
  };
}

export async function openInterview(
  { browser, base },
  t,
  query,
  handler,
  context = browser,
  session = { signedIn: true, loginRequired: true },
) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  if (context === browser) {
    context = await browser.newContext();
    t.after(() => context.close());
  }
  const page = await context.newPage();
  t.after(() => page.close());
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  t.after(() => assert.deepEqual(errors, []));
  page.on("dialog", (dialog) => dialog.accept());
  await page.route("**/vendor/livekit-client.js", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body: "",
    }),
  );
  await page.route("**/api/session", (route) =>
    route.fulfill({
      json: session,
    }),
  );
  await page.route("**/api/token", handler);
  await page.addInitScript(mediaAndRoom);
  await page.goto(`${base}/interview.html?${query}`);
  return page;
}

export async function strictFailure(page, message) {
  await page.waitForFunction(
    (text) =>
      document.querySelector("#audio-check-status").textContent === text,
    message,
  );
  assert.equal(await page.locator("#audio-check").isVisible(), true);
  assert.equal(await page.locator("#audio-check-leave").isEnabled(), true);
  assert.equal(await page.locator("#editor").isDisabled(), true);
  assert.equal(await page.locator("#run-tests").isDisabled(), true);
  assert.equal(
    await page.locator("#agent-state").textContent(),
    "Not connected",
  );
  assert.doesNotMatch(
    await page.locator("#presence-banner").textContent(),
    /offline|personalized evaluation|still work the problem/i,
  );
  assert.equal(await page.evaluate(() => window.roomJoins.length), 0);
  assert.doesNotMatch(
    await page.locator("#transcript-panel").textContent(),
    /Offline (?:mode|practice)/,
  );
}

export function grant(page, receipt, room = receipt) {
  return {
    problemPage: page,
    previousAssignment: receipt,
    roomName: room,
    serverUrl: "wss://example.test",
    token: room,
    durationMin: 30,
  };
}
