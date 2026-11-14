import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { topics } from "../../web/lib.js";
import { DEFAULT_RUNTIME_CONFIG, read, startStaticServer } from "./source.js";

let browser;
let server;
let base;
const replayEvidence = new WeakMap();
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
      this.state = "connected";
      this.remoteParticipants = new Map(
        window.testEmptyRoster
          ? []
          : [
              [agent.identity, agent],
              [other.identity, other],
            ],
      );
      this.disconnectCalls = 0;
      this.published = [];
      this.receipts = [];
      this.syncRequests = [];
      this.trackPublications = [];
      this.settledPublications = [];
      this.localParticipant = {
        identity: "candidate-startup",
        publishTrack: async (track) => {
          this.trackPublications.push(track.kind);
          try {
            if (
              (window.testHoldPublication && track.kind === "audio") ||
              (window.testHoldCamera && track.kind === "video")
            )
              await new Promise((resolve) => {
                this.releasePublication = resolve;
              });
            if (window.testRejectPublication)
              throw new Error("Microphone publication rejected");
          } finally {
            this.settledPublications.push(track.kind);
          }
        },
        publishData: async (data, options) => {
          this.published.push(data);
          const message = JSON.parse(new TextDecoder().decode(data));
          if (message.type === "end_interview" && this.holdEnd) {
            await new Promise((resolve) => {
              this.releaseEnd = resolve;
            });
            this.endSettled = true;
          }
          if (message.type === "startup_sync") {
            this.syncRequests.push({ message, options });
            if (window.testReadyState)
              await this.startup(window.testReadyState, options.topic);
          }
          if (
            window.testHoldIntegrity &&
            options.topic === window.testIntegrityTopic
          ) {
            await new Promise((resolve) => {
              this.releasePublication = resolve;
            });
            this.settledPublications.push("integrity");
            if (window.testRejectPublication)
              throw new Error("Integrity publication rejected");
          }
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
      if (window.testHoldConnect)
        await new Promise((resolve) => {
          this.releaseConnect = resolve;
        });
      if (window.testDepartDuringConnect) {
        await this.depart(other.identity);
        await this.depart();
      }
      this.connectSettled = true;
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

for (const strict of [false, true]) {
  for (const late of ["resolve", "reject"]) {
    test(
      `${strict ? "Strict" : "Practice"} early-ready disconnect during integrity cannot revive startup after ${late}`,
      { timeout: 15_000 },
      async (t) => {
        if (!browser) return t.skip("playwright chromium unavailable");
        let recordings = 0;
        const page = await joinedPage(t, {
          strict,
          recording: true,
          clock: true,
          beforePublication: true,
          setup: async (page) => {
            await page.addInitScript((topic) => {
              window.testHoldIntegrity = true;
              window.testIntegrityTopic = topic;
            }, topics.integrity);
            await page.route(
              "**/api/interviews/startup-recording/recording",
              (route) => {
                recordings++;
                return route.fulfill({ json: { status: "active" } });
              },
            );
          },
        });
        await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
        await page.evaluate(
          async ({ topic, late }) => {
            await window.testRoom.startup(
              {
                type: "interviewer_state",
                reconnecting: false,
                startup: "ready",
              },
              topic,
            );
            await window.testRoom.emit("Disconnected");
            window.testRejectPublication = late === "reject";
            window.testRoom.releasePublication();
          },
          { topic: topics.control, late },
        );
        await page.waitForFunction(() =>
          window.testRoom.settledPublications.includes("integrity"),
        );
        await page.clock.runFor(1_000);
        assert.equal(await page.evaluate(() => window.testInterviewClocks), 0);
        assert.equal(
          recordings,
          0,
          "a lost startup room cannot request recording",
        );
        assert.equal(await page.locator("#timer").textContent(), "00:00");
        assert.equal(await page.locator("#editor").isDisabled(), strict);
        assert.equal(
          await page.evaluate(() =>
            window.testMediaTracks.every(
              (track) => track.readyState === "ended",
            ),
          ),
          true,
        );
      },
    );
  }
}

async function joinedPage(
  t,
  {
    clock = false,
    departDuringConnect = false,
    recording = false,
    replayOutcome = 200,
    camera = false,
    strict = false,
    candidateLed = false,
    beforePublication = false,
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
      if (recording && page) await replayEvents(page);
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
      const interval = window.setInterval;
      window.testInterviewClocks = 0;
      window.setInterval = function (callback, ...args) {
        if (callback.name === "tickTimer") window.testInterviewClocks += 1;
        return interval.call(this, callback, ...args);
      };
      // Keep native capture and preflight, without depending on downloaded
      // vision models while exercising startup event ordering.
      window.FaceDetector = class {
        async detect() {
          return [{}];
        }
      };
      window.testMediaTracks = [];
      window.testCaptures = [];
      window.testUnhandledRejections = [];
      window.addEventListener("unhandledrejection", (event) => {
        window.testUnhandledRejections.push(String(event.reason));
      });
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
    const evidence = { requests: [], batches: [], outcome: replayOutcome };
    replayEvidence.set(page, evidence);
    page.on("request", (request) => {
      if (
        new URL(request.url()).pathname ===
        "/api/interviews/startup-recording/events"
      )
        evidence.requests.push(request);
    });
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
    await page.route("**/api/interviews/startup-recording/events", (route) => {
      const batch = route.request().postDataJSON().events;
      const firstSeq = evidence.batches.flat().length;
      evidence.batches.push(batch);
      return replayOutcome === "offline"
        ? route.abort("internetdisconnected")
        : route.fulfill({
            status: replayOutcome,
            json:
              replayOutcome === 200
                ? { firstSeq, lastSeq: firstSeq + batch.length - 1 }
                : { error: "Replay fixture deliberately refused the batch." },
          });
    });
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
    `${base}/interview.html?${strict ? "selection=strict" : "problem=chargeback-pair-match"}&duration=45${candidateLed ? "&candidateLed=1" : ""}`,
  );
  phase = "media preflight";
  if (!recording && !camera) await page.locator("#camera-skip").click();
  await page.locator("#audio-heard").click();
  if (recording) await page.locator("#recording-consent").check();
  phase = "waiting for Join readiness";
  await page.waitForFunction(
    () => !document.querySelector("#audio-check-join").disabled,
  );
  phase = "clicking Join";
  await page.locator("#audio-check-join").click();
  phase = "waiting for room publication";
  if (beforePublication)
    await page.waitForFunction(
      () =>
        !!(
          window.testRoom?.releaseConnect || window.testRoom?.releasePublication
        ),
    );
  else if (departDuringConnect)
    await page.waitForFunction(() => window.testRoom?.disconnectCalls === 1);
  else await page.waitForFunction(() => window.testRoom?.published.length > 0);
  phase = "startup scenario after room publication";
  return page;
}

async function replayEvents(page) {
  // Flush the real queue, including any batch already in flight. A fixture
  // that never receives the producer's request is a failure even if the page
  // deliberately tolerates replay outages or closes the feed on a 404.
  await page.evaluate(async () => {
    const { flushReplay } = await import("/replay-feed.js");
    await flushReplay();
  });
  const { requests, batches, outcome } = replayEvidence.get(page);
  assert.ok(requests.length > 0, "the page must POST replay evidence");
  assert.equal(
    batches.length,
    requests.length,
    "every replay POST must reach the fixture",
  );
  for (const [index, request] of requests.entries()) {
    assert.equal(request.method(), "POST");
    assert.match(request.headers()["content-type"], /application\/json/);
    assert.deepEqual(request.postDataJSON(), { events: batches[index] });
    assert.ok(
      batches[index].length > 0,
      "an empty batch is not recording evidence",
    );
    const response = await request.response();
    if (outcome === "offline") {
      assert.equal(response, null);
      assert.ok(request.failure(), "the deliberate network outage must occur");
    } else {
      assert.ok(response, "the replay POST must receive an HTTP response");
      assert.equal(
        response.status(),
        outcome,
        "unexpected replay publishing failure",
      );
    }
  }
  return batches.flat();
}

async function warnsOnLeave(page) {
  return page.evaluate(() => {
    const event = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(event);
    return event.defaultPrevented;
  });
}

// Use the pinned SDK's roster teardown, bridging its events into the real page
// handlers. This exercises event ordering without a signaling server.
async function fullSdkRestart(page) {
  await page.evaluate(() => {
    window.fixtureSdk = window.LivekitClient;
  });
  await page.addScriptTag({ content: read("web/vendor/livekit-client.js") });
  return page.evaluate(() => {
    const sdk = window.LivekitClient;
    window.LivekitClient = window.fixtureSdk;
    const appRoom = window.testRoom;
    appRoom.savedParticipants = [...appRoom.remoteParticipants];
    const sdkRoom = new sdk.Room();
    sdkRoom.state = sdk.ConnectionState.Connected;
    const seen = [];
    for (const participant of appRoom.remoteParticipants.values()) {
      sdkRoom.remoteParticipants.set(participant.identity, {
        ...participant,
        trackPublications: new Map(),
        setDisconnected() {},
      });
    }
    sdkRoom.on(sdk.RoomEvent.ParticipantDisconnected, (participant) => {
      seen.push({
        event: "departure",
        identity: participant.identity,
        state: sdkRoom.state,
      });
      appRoom.remoteParticipants.delete(participant.identity);
      void appRoom.emit("ParticipantDisconnected", participant);
    });
    sdkRoom.on(sdk.RoomEvent.Reconnecting, () => {
      seen.push({ event: "reconnecting", state: sdkRoom.state });
      appRoom.state = sdkRoom.state;
      void appRoom.emit("Reconnecting");
    });
    sdkRoom.handleRestarting();
    return seen;
  });
}

async function restoreSdkRoom(page) {
  await page.evaluate(async () => {
    const room = window.testRoom;
    room.remoteParticipants = new Map(room.savedParticipants);
    room.state = "connected";
    await room.emit("Reconnected");
  });
}

// Use the pinned SDK's roster mutations and callbacks while the page is still
// publishing media. No signaling server is needed to exercise this ordering.
async function duplicateStartupRoster(
  page,
  { audio = false, reconnect = false } = {},
) {
  await page.evaluate(() => {
    window.fixtureSdk = window.LivekitClient;
  });
  await page.addScriptTag({ content: read("web/vendor/livekit-client.js") });
  return page.evaluate(
    async ({ audio, reconnect }) => {
      const sdk = window.LivekitClient;
      window.LivekitClient = window.fixtureSdk;
      const appRoom = window.testRoom;
      const sdkRoom = new sdk.Room();
      sdkRoom.state = sdk.ConnectionState.Connected;
      appRoom.remoteParticipants = sdkRoom.remoteParticipants;
      window.testJoinParticipant = (identity, attributes = {}) =>
        sdkRoom.getOrCreateParticipant(identity, {
          sid: `PA_${identity}`,
          identity,
          tracks: [],
          dataTracks: [],
          attributes,
        });
      const seen = [];
      if (reconnect) {
        for (const event of [
          sdk.RoomEvent.Reconnecting,
          sdk.RoomEvent.Reconnected,
        ]) {
          sdkRoom.on(event, () => {
            appRoom.state = sdkRoom.state;
            void appRoom.emit(
              event === sdk.RoomEvent.Reconnecting
                ? "Reconnecting"
                : "Reconnected",
            );
            seen.push({
              event,
              state: sdkRoom.state,
              roster: [...sdkRoom.remoteParticipants.keys()],
              label: document.querySelector("#agent-state").textContent,
            });
          });
        }
        sdkRoom.handleRestarting();
      }
      for (const event of [
        sdk.RoomEvent.ParticipantConnected,
        sdk.RoomEvent.ParticipantDisconnected,
      ]) {
        sdkRoom.on(event, (participant) => {
          seen.push({
            event,
            identity: participant.identity,
            roster: [...sdkRoom.remoteParticipants.keys()],
          });
          void appRoom.emit(
            event === sdk.RoomEvent.ParticipantConnected
              ? "ParticipantConnected"
              : "ParticipantDisconnected",
            participant,
          );
        });
      }
      const duplicate = window.testJoinParticipant(
        "interviewer-duplicate",
        reconnect ? { "lk.agent.state": "thinking" } : {},
      );
      if (audio)
        await appRoom.emit(
          "TrackSubscribed",
          {
            kind: "audio",
            attach(element) {
              window.testAttachedAudio = true;
              return element;
            },
          },
          {},
          duplicate,
        );
      window.testJoinParticipant("interviewer-survivor");
      if (reconnect) {
        // Match handleSignalRestarted after its transport/publication awaits:
        // Reconnected sees the populated roster before buffered join callbacks.
        sdkRoom.setAndEmitConnectionState(sdk.ConnectionState.Connected);
        sdkRoom.emit(sdk.RoomEvent.Reconnected);
        sdkRoom.emitBufferedEvents();
      }
      sdkRoom.handleParticipantDisconnected(duplicate.identity, duplicate);
      return seen;
    },
    { audio, reconnect },
  );
}

for (const strict of [false, true]) {
  for (const { startup, reconnect = false } of [
    { startup: "ready" },
    { startup: "failed" },
    { startup: "ready", reconnect: true },
    { startup: "failed", reconnect: true },
  ]) {
    test(
      `${strict ? "Strict" : "Practice"} startup ${startup} survives a duplicate departing during media publication${reconnect ? " with a full SDK reconnect" : ""}`,
      { timeout: 15_000 },
      async (t) => {
        if (!browser) return t.skip("playwright chromium unavailable");
        const page = await joinedPage(t, {
          strict,
          clock: true,
          beforePublication: true,
          setup: (page) =>
            page.addInitScript(() => {
              window.testEmptyRoster = true;
              window.testHoldPublication = true;
            }),
        });
        await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
        const events = await duplicateStartupRoster(page, { reconnect });
        if (reconnect) {
          assert.deepEqual(events.slice(0, 2), [
            {
              event: "reconnecting",
              state: "reconnecting",
              roster: [],
              label: "Connecting",
            },
            {
              event: "reconnected",
              state: "connected",
              roster: ["interviewer-duplicate", "interviewer-survivor"],
              label: "Thinking...",
            },
          ]);
          assert.equal(
            await page.evaluate(() => window.testRoom.syncRequests.length),
            1,
          );
        }
        assert.deepEqual(events.slice(reconnect ? 2 : 0), [
          {
            event: "participantConnected",
            identity: "interviewer-duplicate",
            roster: reconnect
              ? ["interviewer-duplicate", "interviewer-survivor"]
              : ["interviewer-duplicate"],
          },
          {
            event: "participantConnected",
            identity: "interviewer-survivor",
            roster: ["interviewer-duplicate", "interviewer-survivor"],
          },
          {
            event: "participantDisconnected",
            identity: "interviewer-duplicate",
            roster: ["interviewer-survivor"],
          },
        ]);
        assert.deepEqual(
          await page.evaluate(() => window.testRoom.settledPublications),
          [],
          "roster callbacks must run before microphone publication settles",
        );
        // A normal peer cannot establish an interviewer while media is held.
        await page.evaluate(async (topic) => {
          window.testJoinParticipant("peer-untrusted");
          for (const startup of ["ready", "failed"])
            await window.testRoom.startup(
              {
                type: "interviewer_state",
                reconnecting: false,
                startup,
                reason: "billing",
              },
              topic,
              "peer-untrusted",
            );
          window.testRoom.releasePublication();
        }, topics.control);
        await page.waitForFunction(() =>
          window.testRoom.published.some(
            (data) =>
              JSON.parse(new TextDecoder().decode(data)).type ===
              "SESSION_START",
          ),
        );
        // Once publication has established the surviving interviewer, another
        // agent must still be unable to settle its startup gate.
        await page.evaluate(async (topic) => {
          window.testJoinParticipant("interviewer-other");
          for (const startup of ["ready", "failed"])
            await window.testRoom.startup(
              {
                type: "interviewer_state",
                reconnecting: false,
                startup,
                reason: "billing",
              },
              topic,
              "interviewer-other",
            );
        }, topics.control);
        assert.equal(await page.locator("#timer").textContent(), "00:00");
        assert.deepEqual(
          await page.evaluate(() => [
            window.testRoom.disconnectCalls,
            window.testRoom.receipts.length,
          ]),
          [0, 0],
        );
        await page.evaluate(
          ({ startup, topic }) =>
            window.testRoom.startup(
              {
                type: "interviewer_state",
                reconnecting: false,
                startup,
                reason: "quota",
                elapsedMs: 0,
              },
              topic,
              "interviewer-survivor",
            ),
          { startup, topic: topics.control },
        );
        // Receipt publication includes a bounded send grace on the page clock.
        if (startup === "failed") await page.clock.runFor(1100);
        const settled = await page
          .waitForFunction(
            (startup) =>
              startup === "ready"
                ? document.querySelector("#timer").textContent === "45:00"
                : window.testRoom.disconnectCalls === 1,
            startup,
            { timeout: 3000 },
          )
          .then(
            () => true,
            (error) => {
              if (error.name !== "TimeoutError") throw error;
              return false;
            },
          );
        assert.equal(
          settled,
          true,
          `the surviving interviewer's ${startup} notice must settle startup`,
        );
        if (startup === "ready") {
          assert.equal(
            await page.evaluate(() => window.testRoom.disconnectCalls),
            0,
          );
          assert.equal(
            await page.evaluate(() => window.testInterviewClocks),
            1,
          );
          await page.clock.fastForward(1000);
          await page.evaluate(
            (topic) =>
              window.testRoom.startup(
                {
                  type: "interviewer_state",
                  reconnecting: false,
                  startup: "ready",
                  elapsedMs: 0,
                },
                topic,
                "interviewer-survivor",
              ),
            topics.control,
          );
          assert.equal(await page.locator("#timer").textContent(), "44:59");
          assert.equal(
            await page.evaluate(() => window.testInterviewClocks),
            1,
          );
        } else {
          assert.match(
            await page.locator("#presence-banner").textContent(),
            /quota/,
          );
          assert.equal(await page.locator("#timer").textContent(), "00:00");
          assert.deepEqual(
            await page.evaluate(() =>
              window.testRoom.receipts.map(
                ({ options }) => options.destinationIdentities,
              ),
            ),
            [["interviewer-survivor"]],
          );
          assert.ok(
            await page.evaluate(() =>
              window.testMediaTracks.every(
                (track) => track.readyState === "ended",
              ),
            ),
          );
        }
      },
    );
  }
}

for (const reconnect of [false, true]) {
  test(
    `early audio keeps the established interviewer pinned after departure${reconnect ? " with a full SDK reconnect" : ""}`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t, {
        clock: true,
        beforePublication: true,
        setup: (page) =>
          page.addInitScript(() => {
            window.testEmptyRoster = true;
            window.testHoldPublication = true;
          }),
      });
      await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
      await duplicateStartupRoster(page, { audio: true, reconnect });
      assert.equal(await page.evaluate(() => window.testAttachedAudio), true);
      await page.evaluate(() => window.testRoom.releasePublication());
      await page.waitForFunction(() => window.testRoom.published.length > 0);
      await page.evaluate(async (topic) => {
        for (const startup of ["ready", "failed"])
          await window.testRoom.startup(
            {
              type: "interviewer_state",
              reconnecting: false,
              startup,
              reason: "quota",
            },
            topic,
            "interviewer-survivor",
          );
      }, topics.control);
      await page.clock.runFor(1100);
      assert.equal(await page.locator("#timer").textContent(), "00:00");
      assert.deepEqual(
        await page.evaluate(() => [
          window.testRoom.disconnectCalls,
          window.testRoom.receipts.length,
        ]),
        [0, 0],
      );
      await page.clock.fastForward(180_000);
      await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
      assert.doesNotMatch(
        await page.locator("#presence-banner").textContent(),
        /quota/,
      );
    },
  );
}

for (const strict of [false, true]) {
  test(
    `${strict ? "Strict" : "Practice"} a later full SDK reconnect does not reopen roster identity binding`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t, {
        strict,
        clock: true,
        setup: (page) =>
          page.addInitScript(() => {
            window.testEmptyRoster = true;
          }),
      });
      await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
      assert.deepEqual(
        await page.evaluate(() => window.testRoom.settledPublications),
        ["audio"],
      );
      await duplicateStartupRoster(page, { reconnect: true });
      await page.evaluate(async (topic) => {
        for (const startup of ["ready", "failed"])
          await window.testRoom.startup(
            {
              type: "interviewer_state",
              reconnecting: false,
              startup,
              reason: "quota",
            },
            topic,
            "interviewer-survivor",
          );
      }, topics.control);
      await page.clock.runFor(1100);
      assert.equal(await page.locator("#timer").textContent(), "00:00");
      assert.deepEqual(
        await page.evaluate(() => [
          window.testRoom.disconnectCalls,
          window.testRoom.receipts.length,
        ]),
        [0, 0],
      );
      await page.clock.fastForward(180_000);
      await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
      assert.doesNotMatch(
        await page.locator("#presence-banner").textContent(),
        /quota/,
      );
    },
  );
}

for (const strict of [false, true]) {
  for (const reconnect of [false, true]) {
    test(
      `${strict ? "Strict" : "Practice"} candidate-led startup preserves readiness with reconnect ${reconnect}`,
      { timeout: 15_000 },
      async (t) => {
        if (!browser) return t.skip("playwright chromium unavailable");
        const tokenRequests = [];
        const page = await joinedPage(t, {
          clock: true,
          strict,
          candidateLed: true,
          setup: async (page) => {
            page.on("request", (request) => {
              if (new URL(request.url()).pathname === "/api/token")
                tokenRequests.push(request.postDataJSON());
            });
          },
        });
        await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
        assert.equal(tokenRequests.length, 1);
        assert.equal(tokenRequests[0].candidateLed, true);
        assert.equal(tokenRequests[0].selectionMode === "strict", strict);
        assert.equal(await page.locator("#timer").textContent(), "00:00");
        assert.equal(await page.locator("#end").isDisabled(), strict);
        assert.equal(await page.evaluate(() => window.testInterviewClocks), 0);
        const work = "// Candidate-led work survives startup";
        await page.locator("#editor").fill(work);
        const ready = {
          type: "interviewer_state",
          reconnecting: false,
          startup: "ready",
          startedAt: (await page.evaluate(() => Date.now())) + 120_000,
          elapsedMs: reconnect ? 7_000 : 0,
        };
        if (reconnect) {
          await fullSdkRestart(page);
          await page.clock.fastForward(7_000);
          await page.evaluate((ready) => {
            window.testReadyState = ready;
          }, ready);
          await restoreSdkRoom(page);
        } else {
          await page.evaluate(
            ({ ready, topic }) => window.testRoom.startup(ready, topic),
            { ready, topic: topics.control },
          );
        }
        await page.waitForFunction(() => window.testInterviewClocks === 1);
        assert.equal(
          await page.locator("#timer").textContent(),
          reconnect ? "44:53" : "45:00",
        );
        assert.equal(await page.locator("#editor").inputValue(), work);
        assert.equal(
          await page.evaluate(() => window.testRoom.syncRequests.length),
          reconnect ? 1 : 0,
        );
        await page.evaluate(
          ({ ready, topic }) =>
            window.testRoom.startup({ ...ready, elapsedMs: 0 }, topic),
          { ready, topic: topics.control },
        );
        await page.clock.fastForward(1_000);
        assert.equal(
          await page.locator("#timer").textContent(),
          reconnect ? "44:52" : "44:59",
        );
        assert.equal(
          tokenRequests.length,
          1,
          "reconnect must not mint a new token",
        );
        assert.equal(await page.evaluate(() => window.testInterviewClocks), 1);
        assert.equal(
          await page.evaluate(() => window.testRoom.disconnectCalls),
          0,
        );
      },
    );
  }
}

for (const strict of [false, true]) {
  for (const skew of [-120_000, 120_000]) {
    for (const reconnect of [false, true]) {
      test(
        `${strict ? "Strict" : "Practice"} readiness ignores server clock skew ${skew} with reconnect ${reconnect}`,
        { timeout: 15_000 },
        async (t) => {
          if (!browser) return t.skip("playwright chromium unavailable");
          const page = await joinedPage(t, { clock: true, strict });
          await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
          const ready = {
            type: "interviewer_state",
            reconnecting: false,
            startup: "ready",
            startedAt: (await page.evaluate(() => Date.now())) + skew,
            elapsedMs: reconnect ? 10_000 : 0,
          };
          if (reconnect) {
            await fullSdkRestart(page);
            await page.clock.fastForward(10_000);
            await page.evaluate((ready) => {
              window.testReadyState = ready;
            }, ready);
            await restoreSdkRoom(page);
          } else {
            await page.evaluate(
              ({ ready, topic }) => window.testRoom.startup(ready, topic),
              { ready, topic: topics.control },
            );
          }
          await page.waitForFunction(() => window.testInterviewClocks === 1);
          assert.equal(
            await page.locator("#timer").textContent(),
            reconnect ? "44:50" : "45:00",
          );
          await page.clock.fastForward((45 * 60 - (reconnect ? 10 : 0)) * 1000);
          await page.waitForFunction(() =>
            window.testRoom.published.some(
              (bytes) =>
                JSON.parse(new TextDecoder().decode(bytes)).type ===
                "end_interview",
            ),
          );
          assert.equal(
            await page.evaluate(
              () =>
                window.testRoom.published.filter(
                  (bytes) =>
                    JSON.parse(new TextDecoder().decode(bytes)).type ===
                    "end_interview",
                ).length,
            ),
            1,
          );
          assert.equal(
            await page.evaluate(() => window.testInterviewClocks),
            1,
          );
        },
      );
    }
  }
}

for (const strict of [false, true]) {
  for (const order of ["absent", "before departure", "after return"]) {
    test(
      `${strict ? "Strict" : "Practice"} startup synchronizes readiness generated ${order}`,
      { timeout: 15_000 },
      async (t) => {
        if (!browser) return t.skip("playwright chromium unavailable");
        const page = await joinedPage(t, { clock: true, strict });
        await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
        const ready = {
          type: "interviewer_state",
          reconnecting: false,
          startup: "ready",
          startedAt: Date.parse("2030-01-01T00:01:00Z"),
          elapsedMs: 0,
        };
        if (order === "before departure") {
          await page.evaluate(
            ({ ready, topic }) => window.testRoom.startup(ready, topic),
            { ready, topic: topics.control },
          );
          await page.waitForFunction(() => window.testInterviewClocks === 1);
        }
        await fullSdkRestart(page);
        await page.locator("#editor").fill("// Work retained during reconnect");
        assert.equal(await warnsOnLeave(page), true);
        await page.clock.fastForward(5_000);
        if (order === "absent")
          ready.startedAt = await page.evaluate(() => Date.now());
        if (order !== "after return")
          await page.evaluate(
            (ready) => {
              window.testReadyState = ready;
            },
            { ...ready, elapsedMs: order === "absent" ? 10_000 : 15_000 },
          );
        await page.clock.fastForward(10_000);
        await restoreSdkRoom(page);
        assert.equal(
          await page.evaluate(() => window.testRoom.syncRequests.length),
          order === "before departure" ? 0 : 1,
          "a pending reconnect requests existing state; a completed startup does not",
        );
        if (order === "after return") {
          assert.equal(await page.locator("#timer").textContent(), "00:00");
          await page.clock.fastForward(5_000);
          ready.startedAt = await page.evaluate(() => Date.now());
          await page.evaluate(
            ({ ready, topic }) => window.testRoom.startup(ready, topic),
            { ready, topic: topics.control },
          );
        }
        await page.waitForFunction(() => window.testInterviewClocks === 1);
        const expected =
          order === "absent"
            ? "44:50"
            : order === "after return"
              ? "45:00"
              : "44:45";
        assert.equal(await page.locator("#timer").textContent(), expected);
        assert.equal(
          await page.locator("#editor").inputValue(),
          "// Work retained during reconnect",
        );
        // A return event and its query may both deliver the same authoritative
        // state. Even a conflicting late timestamp cannot establish a new clock.
        await page.evaluate(
          async ({ ready, topic }) => {
            await window.testRoom.startup(ready, topic);
            await window.testRoom.startup(
              { ...ready, startedAt: Date.now() },
              topic,
            );
          },
          { ready, topic: topics.control },
        );
        await page.clock.fastForward(1000);
        assert.equal(
          await page.locator("#timer").textContent(),
          order === "absent"
            ? "44:49"
            : order === "after return"
              ? "44:59"
              : "44:44",
        );
        assert.equal(await page.evaluate(() => window.testInterviewClocks), 1);
        assert.equal(
          await page.evaluate(() => window.testRoom.disconnectCalls),
          0,
        );
        await page.locator("#end").click();
        await page.waitForFunction(() =>
          window.testRoom.published.some(
            (bytes) =>
              JSON.parse(new TextDecoder().decode(bytes)).type ===
              "end_interview",
          ),
        );
        await page.evaluate(
          ({ ready, topic }) => window.testRoom.startup(ready, topic),
          { ready, topic: topics.control },
        );
        assert.equal(
          await page.evaluate(
            () =>
              window.testRoom.published.filter(
                (bytes) =>
                  JSON.parse(new TextDecoder().decode(bytes)).type ===
                  "end_interview",
              ).length,
          ),
          1,
        );
        assert.equal(await page.evaluate(() => window.testInterviewClocks), 1);
      },
    );
  }
}

for (const { strict, terminal } of [false, true].flatMap((strict) =>
  (strict ? ["failure", "deadline"] : ["failure", "deadline", "cancel"]).map(
    (terminal) => ({ strict, terminal }),
  ),
)) {
  test(
    `${strict ? "Strict" : "Practice"} startup synchronization cannot revive a ${terminal} after reconnect`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t, { clock: true, strict });
      await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
      await fullSdkRestart(page);
      if (terminal === "deadline") await page.clock.fastForward(150_000);
      else if (terminal === "cancel") await page.locator("#end").click();
      else {
        // Restore the sender roster, without completing the reconnect yet.
        await page.evaluate(async (topic) => {
          window.testRoom.remoteParticipants = new Map(
            window.testRoom.savedParticipants,
          );
          await window.testRoom.startup(
            {
              type: "interviewer_state",
              reconnecting: false,
              startup: "failed",
              reason: "quota",
            },
            topic,
          );
        }, topics.control);
      }
      await page.waitForFunction(() =>
        window.testMediaTracks.every((track) => track.readyState === "ended"),
      );
      await page.evaluate(
        (ready) => {
          window.testReadyState = ready;
        },
        {
          type: "interviewer_state",
          reconnecting: false,
          startup: "ready",
          startedAt: Date.parse("2030-01-01T00:01:00Z"),
        },
      );
      await restoreSdkRoom(page);
      await page.evaluate(async (topic) => {
        await window.testRoom.startup(window.testReadyState, topic);
        await window.testRoom.emit("Reconnected");
      }, topics.control);
      await page.clock.fastForward(1000);
      assert.equal(await page.locator("#timer").textContent(), "00:00");
      assert.equal(await page.evaluate(() => window.testInterviewClocks), 0);
      assert.equal(
        await page.evaluate(() => window.testRoom.syncRequests.length),
        0,
      );
      assert.equal(
        await page.evaluate(() =>
          window.testMediaTracks.every((track) => track.readyState === "ended"),
        ),
        true,
      );
      if (terminal === "failure")
        assert.match(
          await page.locator("#presence-banner").textContent(),
          /quota|capacity/,
        );
    },
  );
}

for (const strict of [false, true]) {
  test(
    `${strict ? "Strict" : "Practice"} startup survives the vendored SDK's full reconnect ordering`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t, { clock: true, strict });
      await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
      assert.deepEqual(await fullSdkRestart(page), [
        { event: "departure", identity: "agent-startup", state: "connected" },
        { event: "departure", identity: "agent-other", state: "connected" },
        { event: "reconnecting", state: "reconnecting" },
      ]);
      assert.equal(
        await page.evaluate(() => window.testRoom.disconnectCalls),
        0,
      );
      assert.equal(
        await page.evaluate(() =>
          window.testMediaTracks.some((track) => track.readyState === "live"),
        ),
        true,
      );
      assert.equal(await page.locator("#timer").textContent(), "00:00");
      assert.doesNotMatch(
        await page.locator("#presence-banner").textContent(),
        /startup check failed|did not answer/,
      );
      await page.evaluate(async () => {
        window.testRoom.remoteParticipants = new Map(
          window.testRoom.savedParticipants,
        );
        window.testRoom.state = "connected";
        await window.testRoom.emit("Reconnected");
      });
      const ready = {
        type: "interviewer_state",
        reconnecting: false,
        startup: "ready",
      };
      await page.evaluate(
        ({ ready, topic }) => window.testRoom.startup(ready, topic),
        { ready, topic: topics.control },
      );
      await page.waitForFunction(
        () => document.querySelector("#timer").textContent === "45:00",
      );
      await page.clock.fastForward(1000);
      assert.equal(await page.locator("#timer").textContent(), "44:59");
      await page.evaluate(
        ({ ready, topic }) => window.testRoom.startup(ready, topic),
        { ready, topic: topics.control },
      );
      await page.clock.fastForward(1000);
      assert.equal(await page.locator("#timer").textContent(), "44:58");
      assert.equal(
        await page.evaluate(() => window.testRoom.disconnectCalls),
        0,
      );
    },
  );
}

for (const terminal of ["deadline", "disconnect"]) {
  test(
    `startup full reconnect still fails on its original ${terminal}`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t, { clock: true });
      await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
      await fullSdkRestart(page);
      assert.equal(
        await page.evaluate(() => window.testRoom.disconnectCalls),
        0,
      );
      // At 60s, 150s crosses the original wait but not a fresh 180s wait. A
      // reconnect that incorrectly rearms readiness must fail this check.
      if (terminal === "deadline") await page.clock.fastForward(150_000);
      else await page.evaluate(() => window.testRoom.emit("Disconnected"));
      await page.waitForFunction(() =>
        window.testMediaTracks.every((track) => track.readyState === "ended"),
      );
      assert.equal(await page.locator("#timer").textContent(), "00:00");
      assert.equal(await page.locator("#ending-overlay").isVisible(), false);
      assert.match(
        await page.locator("#presence-banner").textContent(),
        /did not answer the startup check/,
      );
    },
  );
}

async function assertPendingControls(page, { expectEndDisabled }) {
  assert.equal(await page.locator("#end").isDisabled(), expectEndDisabled);
  assert.equal(await page.locator("#pause").count(), 0);

  const result = await page.evaluate((checkDisabledEnd) => {
    if (checkDisabledEnd) {
      document.querySelector("#end").click();
    }

    return {
      ending: !document.querySelector("#ending-overlay").hidden,
      endRequests: window.testRoom.published
        .map((bytes) => JSON.parse(new TextDecoder().decode(bytes)))
        .filter((message) => message.type === "end_interview").length,
    };
  }, expectEndDisabled);

  assert.deepEqual(result, {
    ending: false,
    endRequests: 0,
  });
}

test(
  "Strict startup failure does not promise offline editing or start the clock",
  { timeout: 15_000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await joinedPage(t, { strict: true });
    await assertPendingControls(page, {
      expectEndDisabled: true,
    });
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
    assert.equal(await page.locator("#pause").count(), 0);
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
    await assertPendingControls(page, {
      expectEndDisabled: false,
    });
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
    assert.equal(await page.locator("#pause").count(), 0);
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
    const starter = await page.locator("#editor").inputValue();
    const opening = await replayEvents(page);
    assert.deepEqual(
      opening
        .filter((event) => event.kind === "lifecycle")
        .map((event) => event.payload.state),
      ["started"],
      "the opening is recorded while the recording-start response is held",
    );
    assert.ok(
      opening.some((event) => event.kind === "stage" && event.payload.title),
    );
    assert.ok(
      opening.some(
        (event) => event.kind === "editor" && event.payload.code === starter,
      ),
    );
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
    const code = "// Work retained after failed startup";
    await page.locator("#editor").fill(code);
    await page.locator("#end").click();
    await page.waitForFunction(
      () => !document.querySelector("#report-modal").hidden,
    );
    const replay = await replayEvents(page);
    assert.deepEqual(
      replay
        .filter((event) => event.kind === "lifecycle")
        .map((event) => event.payload.state),
      ["started", "ended"],
    );
    const edited = replay.findIndex(
      (event) => event.kind === "editor" && event.payload.code === code,
    );
    const ended = replay.findIndex(
      (event) => event.kind === "lifecycle" && event.payload.state === "ended",
    );
    assert.ok(
      edited >= 0 && edited < ended,
      "the final editor snapshot precedes the end",
    );
    await recordingRoute.fulfill({ json: { state: "ready" } });
    await page.waitForFunction(() =>
      document.querySelector("#recording-state").textContent.includes("saved"),
    );
    assert.match(await page.locator("#presence-banner").textContent(), /quota/);
    assert.equal(await page.evaluate(() => window.testRoom.disconnectCalls), 1);
  },
);

test(
  "a microphone publication failure after joining cannot start an offline clock",
  { timeout: 15_000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await joinedPage(t, {
      clock: true,
      beforePublication: true,
      setup: (page) =>
        page.addInitScript(() => {
          window.testHoldPublication = true;
          window.testRejectPublication = true;
        }),
    });
    await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
    await page.evaluate(() => window.testRoom.releasePublication());
    await page.waitForFunction(() =>
      document
        .querySelector("#presence-banner")
        .textContent.includes("microphone could not be connected"),
    );
    assert.equal(await page.locator("#timer").textContent(), "00:00");
    await page.clock.fastForward(60_000);
    assert.equal(await page.locator("#timer").textContent(), "00:00");
    assert.match(
      await page.locator("#presence-banner").textContent(),
      /microphone could not be connected/,
    );
    assert.equal(await warnsOnLeave(page), true);
    assert.equal(await page.evaluate(() => window.testRoom.disconnectCalls), 1);
    assert.equal(
      await page.evaluate(() =>
        window.testMediaTracks.every((track) => track.readyState === "ended"),
      ),
      true,
    );
  },
);

for (const unavailable of [false, true]) {
  test(
    `recording startup keeps replay evidence when recording is ${unavailable ? "unavailable" : "saved"}`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t, {
        recording: true,
        setup: (page) =>
          page.route("**/api/interviews/startup-recording/recording", (route) =>
            route.fulfill({
              json: unavailable
                ? { state: "failed", recovery: "start_again" }
                : { state: "ready" },
            }),
          ),
      });
      await page.waitForFunction(
        (word) =>
          document.querySelector("#recording-state").textContent.includes(word),
        unavailable ? "not kept" : "saved",
      );
      const opening = await replayEvents(page);
      assert.ok(
        opening.some(
          (event) =>
            event.kind === "lifecycle" && event.payload.state === "started",
        ),
      );
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
      await page.waitForFunction(() => window.testInterviewClocks === 1);
      const code = "// Recording status does not gate editor evidence";
      await page.locator("#editor").fill(code);
      await page.locator("#end").click();
      const replay = await replayEvents(page);
      assert.deepEqual(
        replay
          .filter((event) => event.kind === "lifecycle")
          .map((event) => event.payload.state),
        ["started", "ended"],
      );
      const edited = replay.findIndex(
        (event) => event.kind === "editor" && event.payload.code === code,
      );
      const ended = replay.findIndex(
        (event) =>
          event.kind === "lifecycle" && event.payload.state === "ended",
      );
      assert.ok(
        edited >= 0 && edited < ended,
        "the final editor snapshot is captured before ending",
      );
      assert.match(
        await page.locator("#recording-state").textContent(),
        unavailable ? /not kept/ : /saved/,
      );
      assert.deepEqual(
        await page.evaluate(() => window.testUnhandledRejections),
        [],
      );
    },
  );
}

for (const replayOutcome of [404, 503, "offline"]) {
  test(
    `recording startup tolerates deliberate replay ${replayOutcome} without hiding evidence expectations`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t, {
        recording: true,
        replayOutcome,
        setup: (page) =>
          page.route("**/api/interviews/startup-recording/recording", (route) =>
            route.fulfill({ json: { state: "ready" } }),
          ),
      });
      const opening = await replayEvents(page);
      assert.ok(
        opening.some(
          (event) =>
            event.kind === "lifecycle" && event.payload.state === "started",
        ),
      );
      const attempts = replayEvidence.get(page).requests.length;
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
      await page.waitForFunction(() => window.testInterviewClocks === 1);
      const code = "// Replay failure must not stop the interview";
      await page.locator("#editor").fill(code);
      assert.equal(await warnsOnLeave(page), true);
      await page.locator("#end").click();
      const replay = await replayEvents(page);
      if (replayOutcome === 404) {
        assert.equal(
          replayEvidence.get(page).requests.length,
          attempts,
          "a 404 closes the feed permanently",
        );
        assert.equal(
          replay.some(
            (event) =>
              event.kind === "lifecycle" && event.payload.state === "ended",
          ),
          false,
        );
      } else {
        assert.ok(
          replayEvidence.get(page).requests.length > attempts,
          "an outage drops its batch but later evidence is still attempted",
        );
        assert.ok(
          replay.some(
            (event) => event.kind === "editor" && event.payload.code === code,
          ),
        );
        assert.ok(
          replay.some(
            (event) =>
              event.kind === "lifecycle" && event.payload.state === "ended",
          ),
        );
      }
      await page.waitForFunction(() =>
        document
          .querySelector("#recording-state")
          .textContent.includes("saved"),
      );
      assert.equal(await page.evaluate(() => window.testInterviewClocks), 1);
      assert.deepEqual(
        await page.evaluate(() => window.testUnhandledRejections),
        [],
      );
    },
  );
}

for (const strict of [false, true]) {
  for (const stage of ["microphone", "camera", "integrity"]) {
    for (const cause of stage === "microphone"
      ? ["billing", "timeout"]
      : ["billing"]) {
      for (const late of ["resolve", "reject"]) {
        test(
          `${strict ? "Strict" : "Practice"} startup ${cause} interrupts held ${stage} publication before late ${late}`,
          { timeout: 15_000 },
          async (t) => {
            if (!browser) return t.skip("playwright chromium unavailable");
            const workers = [];
            const page = await joinedPage(t, {
              strict,
              camera: true,
              clock: true,
              beforePublication: true,
              setup: async (page) => {
                page.on("worker", (worker) => workers.push(worker.url()));
                await page.addInitScript(
                  ({ stage, integrity }) => {
                    window.testHoldPublication = stage === "microphone";
                    window.testHoldCamera = stage === "camera";
                    window.testHoldIntegrity = stage === "integrity";
                    window.testIntegrityTopic = integrity;
                  },
                  { stage, integrity: topics.integrity },
                );
              },
            });
            await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
            const code = strict
              ? await page.locator("#editor").inputValue()
              : "// Work retained across failed startup";
            if (!strict) await page.locator("#editor").fill(code);
            else assert.equal(await page.locator("#editor").isDisabled(), true);
            if (cause === "timeout") await page.clock.fastForward(180_000);
            else
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
            // Capture stops before the receipt's bounded departure grace.
            await page.waitForFunction(
              () =>
                window.testMediaTracks.every(
                  (track) => track.readyState === "ended",
                ),
              undefined,
              { timeout: 3000 },
            );
            if (cause === "billing") {
              await page.waitForFunction(
                () => window.testRoom.receipts.length === 1,
              );
              assert.equal(
                await page.evaluate(() => window.testRoom.disconnectCalls),
                0,
              );
              await page.clock.fastForward(251);
            }
            // Failure must finish without releasing the unrelated operation.
            await page.waitForFunction(
              () => window.testRoom.disconnectCalls === 1,
              undefined,
              { timeout: 3000 },
            );
            const message = strict ? "#audio-check-status" : "#presence-banner";
            const expected = cause === "billing" ? /billing/ : /did not answer/;
            await page.waitForFunction(
              ({ message, word }) =>
                document.querySelector(message).textContent.includes(word),
              {
                message,
                word: cause === "billing" ? "billing" : "did not answer",
              },
            );
            const banner = await page.locator(message).textContent();
            assert.match(banner, expected);
            assert.match(banner, /timer has not started/);
            assert.equal(await page.locator("#timer").textContent(), "00:00");
            assert.equal(await page.locator("#editor").inputValue(), code);
            assert.equal(await page.locator("#editor").isDisabled(), strict);
            assert.equal(await page.locator("#end").isDisabled(), strict);
            assert.equal(await page.locator("#pause").count(), 0);
            // Strict has not exposed editable work before publication finishes.
            assert.equal(await warnsOnLeave(page), !strict);
            assert.equal(
              await page.evaluate(() =>
                window.testMediaTracks.every(
                  (track) => track.readyState === "ended",
                ),
              ),
              true,
            );
            if (strict) {
              assert.match(banner, /lobby or retry media preflight/);
              assert.doesNotMatch(banner, /work the problem offline/);
            } else assert.match(banner, /work the problem offline/);
            assert.equal(
              await page.evaluate(() => window.testRoom.receipts.length),
              cause === "billing" ? 1 : 0,
            );
            const published = await page.evaluate(
              () => window.testRoom.published.length,
            );
            await page.evaluate((late) => {
              window.testRejectPublication = late === "reject";
              window.testRoom.releasePublication();
            }, late);
            await page.waitForFunction(
              (stage) => window.testRoom.settledPublications.includes(stage),
              stage === "microphone"
                ? "audio"
                : stage === "camera"
                  ? "video"
                  : stage,
            );
            await page.clock.fastForward(60_000);
            // SDK callbacks may arrive after local disconnect as well.
            await page.evaluate(async () => {
              await window.testRoom.emit("Reconnected");
              await window.testRoom.emit(
                "TrackSubscribed",
                {
                  kind: "audio",
                  attach() {
                    throw new Error("A failed room attached late audio");
                  },
                },
                {},
                window.testRoom.remoteParticipants.get("agent-startup"),
              );
              await window.testRoom.emit("Disconnected");
            });
            assert.equal(await page.locator(message).textContent(), banner);
            assert.equal(await page.locator("#editor").inputValue(), code);
            assert.equal(await page.locator("#editor").isDisabled(), strict);
            assert.equal(await page.locator("#timer").textContent(), "00:00");
            assert.deepEqual(
              await page.evaluate(() => window.testRoom.trackPublications),
              stage === "microphone" ? ["audio"] : ["audio", "video"],
            );
            assert.equal(
              await page.evaluate(() => window.testRoom.published.length),
              published,
            );
            assert.equal(
              await page.evaluate(() => window.testRoom.disconnectCalls),
              1,
            );
            assert.deepEqual(
              await page.evaluate(() => window.testUnhandledRejections),
              [],
            );
            assert.equal(
              workers.some((url) => url.includes("integrity-worker")),
              false,
            );
          },
        );
      }
    }
  }
}

for (const strict of [false, true]) {
  test(
    `${strict ? "Strict" : "Practice"} billing diagnosis survives immediate publication rejection`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t, {
        strict,
        beforePublication: true,
        setup: (page) =>
          page.addInitScript(() => {
            window.testHoldPublication = true;
          }),
      });
      await page.evaluate((topic) => {
        void window.testRoom.startup(
          {
            type: "interviewer_state",
            reconnecting: false,
            startup: "failed",
            reason: "billing",
          },
          topic,
        );
        window.testRejectPublication = true;
        window.testRoom.releasePublication();
      }, topics.control);
      const selector = strict ? "#audio-check-status" : "#presence-banner";
      await page.waitForFunction(
        (selector) =>
          /billing|microphone could not be connected/.test(
            document.querySelector(selector).textContent,
          ),
        selector,
      );
      assert.match(await page.locator(selector).textContent(), /billing/);
      assert.equal(
        await page.evaluate(() => window.testRoom.disconnectCalls),
        1,
      );
      assert.equal(
        await page.evaluate(() => window.testRoom.receipts.length),
        1,
      );
      assert.deepEqual(
        await page.evaluate(() => window.testUnhandledRejections),
        [],
      );
    },
  );
}

test(
  "an early ready notice still waits for held microphone publication",
  { timeout: 15_000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await joinedPage(t, {
      camera: true,
      clock: true,
      beforePublication: true,
      setup: (page) =>
        page.addInitScript(() => {
          window.testHoldPublication = true;
        }),
    });
    await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
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
    await page.clock.fastForward(1000);
    assert.equal(await page.locator("#timer").textContent(), "00:00");
    assert.equal(await page.evaluate(() => window.testRoom.disconnectCalls), 0);
    await page.evaluate(() => window.testRoom.releasePublication());
    await page.waitForFunction(
      () => document.querySelector("#timer").textContent === "44:59",
    );
    assert.deepEqual(
      await page.evaluate(() => window.testRoom.trackPublications),
      ["audio", "video"],
    );
  },
);

for (const notice of ["pending", "ready", "failed"]) {
  test(
    `Practice End during microphone publication after ${notice} disconnects before camera publication`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t, {
        recording: true,
        beforePublication: true,
        setup: (page) =>
          page.addInitScript(() => {
            window.testHoldPublication = true;
          }),
      });
      assert.deepEqual(
        await page.evaluate(() => window.testRoom.trackPublications),
        ["audio"],
      );
      if (notice !== "pending")
        await page.evaluate(
          ({ notice, topic }) =>
            window.testRoom.startup(
              {
                type: "interviewer_state",
                reconnecting: false,
                startup: notice,
                reason: "billing",
              },
              topic,
            ),
          { notice, topic: topics.control },
        );
      await page.locator("#end").click();
      await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
      await page.waitForFunction(
        () => !document.querySelector("#report-modal").hidden,
      );
      assert.equal(
        await page.evaluate(() =>
          window.testMediaTracks.every((track) => track.readyState === "ended"),
        ),
        true,
      );
      await page.evaluate(() => window.testRoom.releasePublication());
      await page.waitForFunction(() =>
        window.testRoom.settledPublications.includes("audio"),
      );
      assert.deepEqual(
        await page.evaluate(() => window.testRoom.trackPublications),
        ["audio"],
      );
      assert.equal(
        await page.evaluate(() => window.testRoom.disconnectCalls),
        1,
      );
      assert.equal(
        await page.evaluate(
          () =>
            window.testRoom.published
              .map((bytes) => JSON.parse(new TextDecoder().decode(bytes)))
              .filter((message) => message.type !== "startup_received").length,
        ),
        0,
      );
      const replay = await replayEvents(page);
      assert.deepEqual(
        replay
          .filter((event) => event.kind === "lifecycle")
          .map((event) => event.payload.state),
        ["ended"],
        "cancelling held publication records the end without a late start",
      );
    },
  );
}

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

for (const reason of ["billing", "quota", "unavailable", "delivery"]) {
  test(
    `offline practice stays editable and untimed after startup ${reason}`,
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
      if (reason === "delivery") {
        assert.match(
          banner,
          /interviewer answered.*confirmation could not be delivered/,
        );
        assert.doesNotMatch(banner, /Gemini did not answer/);
      }

      assert.equal(await page.locator("#pause").count(), 0);
      assert.equal(await page.locator("#editor").isDisabled(), false);
      await page
        .locator("#editor")
        .fill("// Offline work after startup failure");
      assert.equal(await page.locator("#ending-overlay").isVisible(), false);
      await page.clock.fastForward(46 * 60 * 1000);
      assert.equal(await page.locator("#editor").isDisabled(), false);
      assert.equal(
        await page.locator("#editor").inputValue(),
        "// Offline work after startup failure",
      );
      assert.equal(await warnsOnLeave(page), true);
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
  "a ready interview counts down and expires without a Pause control",
  { timeout: 15_000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await joinedPage(t, { clock: true });
    // Let media preflight run before freezing wall-clock drift. Readiness and
    // interview time then advance only when the test moves the clock.
    await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
    await assertPendingControls(page, {
      expectEndDisabled: false,
    });
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
    assert.equal(await page.locator("#pause").count(), 0);
    await page.clock.fastForward(60_000);
    assert.equal(await page.locator("#timer").textContent(), "44:00");
    assert.equal(await page.locator("#editor").isDisabled(), false);
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
      await page.evaluate(async () => {
        await window.testRoom.depart("agent-other");
        await window.testRoom.depart();
      });
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

for (const late of ["ready", "failed", "departure", "missing"]) {
  test(
    `Practice End during startup exits without a report wait before ${late}`,
    { timeout: 15_000 },
    async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await joinedPage(t, { clock: true });
      await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
      const timer = await page.locator("#timer").textContent();
      const code = "// Work kept when ending before readiness";
      await page.locator("#editor").fill(code);
      await page.locator("#end").click();
      assert.equal(
        await page.locator("#skip-report").evaluate((node) => node.hidden),
        true,
      );
      await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
      if (late === "ready" || late === "failed") {
        await page.evaluate(
          ({ late, topic }) =>
            window.testRoom.startup(
              {
                type: "interviewer_state",
                reconnecting: false,
                startup: late,
                reason: "billing",
              },
              topic,
            ),
          { late, topic: topics.control },
        );
      } else if (late === "departure") {
        await page.evaluate(async () => {
          await window.testRoom.depart("agent-other");
          await window.testRoom.depart();
        });
      }
      await page.clock.fastForward(1000);
      await page.waitForFunction(
        () => !document.querySelector("#report-modal").hidden,
      );
      assert.match(
        await page.locator("#report-modal").textContent(),
        /did not produce an evaluation/,
      );
      assert.equal(await page.locator("#ending-overlay").isVisible(), false);
      assert.equal(await page.locator("#editor").inputValue(), code);
      assert.equal(await page.locator("#timer").textContent(), timer);
      await page.clock.fastForward(180_000);
      assert.equal(await page.locator("#timer").textContent(), timer);
      const result = await page.evaluate(() => ({
        ends: window.testRoom.published
          .map((b) => JSON.parse(new TextDecoder().decode(b)))
          .filter((m) => m.type === "end_interview").length,
        stopped: window.testMediaTracks.every(
          (track) => track.readyState === "ended",
        ),
        disconnects: window.testRoom.disconnectCalls,
      }));
      assert.deepEqual(result, { ends: 0, stopped: true, disconnects: 1 });
    },
  );
}

test(
  "Skip after readiness waits for End publication and then its acknowledgement",
  { timeout: 20_000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await joinedPage(t, { clock: true });
    await page.evaluate(
      (topic) =>
        window.testRoom.startup(
          { type: "interviewer_state", reconnecting: false, startup: "ready" },
          topic,
        ),
      topics.control,
    );
    await page.waitForFunction(() => window.testInterviewClocks === 1);
    await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
    await page.evaluate(() => {
      window.testRoom.holdEnd = true;
    });
    await page.locator("#end").click();
    await page.waitForFunction(() => Boolean(window.testRoom.releaseEnd));
    assert.equal(await page.locator("#skip-report").isVisible(), true);
    page.on("dialog", (dialog) => dialog.accept());
    await page.locator("#skip-report").click();
    await page.waitForFunction(
      () =>
        document.querySelector("#ending-title").textContent ===
        "Skipping report...",
    );
    const controls = () =>
      page.evaluate(() =>
        window.testRoom.published
          .map((bytes) => JSON.parse(new TextDecoder().decode(bytes)).type)
          .filter((type) => ["end_interview", "skip_report"].includes(type)),
      );
    assert.deepEqual(await controls(), ["end_interview"]);
    assert.equal(await page.evaluate(() => window.testRoom.disconnectCalls), 0);
    await page.evaluate(() => window.testRoom.releaseEnd());
    await page.waitForFunction(() =>
      window.testRoom.published.some(
        (bytes) =>
          JSON.parse(new TextDecoder().decode(bytes)).type === "skip_report",
      ),
    );
    assert.equal(await page.evaluate(() => window.testRoom.endSettled), true);
    assert.deepEqual(await controls(), ["end_interview", "skip_report"]);
    assert.equal(await page.evaluate(() => window.testRoom.disconnectCalls), 0);
    await Promise.all([
      page.waitForURL(`${base}/`),
      page.evaluate(
        (topic) => window.testRoom.startup({ type: "skip_report_ack" }, topic),
        topics.control,
      ),
    ]);
  },
);

test(
  "Practice End during room join stops media and a late join publishes nothing",
  { timeout: 15_000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await joinedPage(t, {
      beforePublication: true,
      setup: (page) =>
        page.addInitScript(() => {
          window.testHoldConnect = true;
        }),
    });
    await page.locator("#end").click();
    await page.waitForFunction(
      () => !document.querySelector("#report-modal").hidden,
    );
    assert.equal(
      await page.evaluate(() =>
        window.testMediaTracks.every((track) => track.readyState === "ended"),
      ),
      true,
    );
    await page.evaluate(() => window.testRoom.releaseConnect());
    await page.waitForFunction(() => window.testRoom.connectSettled);
    await page.waitForFunction(() => window.testRoom.disconnectCalls === 1);
    assert.equal(
      await page.evaluate(() => window.testRoom.published.length),
      0,
    );
    assert.equal(await page.locator("#timer").textContent(), "00:00");
    assert.equal(await page.locator("#ending-overlay").isVisible(), false);
  },
);
