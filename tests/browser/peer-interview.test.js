import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createMuteToggle, createPeerAudio } from "../../web/peer-audio.js";

class Button extends EventTarget {
  hidden = true;
}
class Room {
  static instances = [];
  handlers = new Map();
  remoteParticipants = new Map();
  localParticipant = {
    trackPublications: new Map(),
    setMicrophoneEnabled: async () => {},
  };
  constructor() {
    Room.instances.push(this);
  }
  on(event, handler) {
    this.handlers.set(event, handler);
  }
  emit(event, ...args) {
    return this.handlers.get(event)?.(...args);
  }
  async connect() {}
  async disconnect() {
    this.disconnected = true;
  }
}
const RoomEvent = {
  TrackSubscribed: "track",
  TrackUnsubscribed: "untrack",
  Disconnected: "disconnected",
  Reconnected: "reconnected",
};
test("peer audio attaches once, detaches on unsubscribe and reconnect, stops local tracks", async () => {
  Room.instances = [];
  let attaches = 0,
    detaches = 0,
    stops = 0,
    recoveries = 0;
  const element = { play: async () => {}, remove() {} };
  const track = {
    kind: "audio",
    attach() {
      attaches++;
      return element;
    },
    detach() {
      detaches++;
      return [element];
    },
  };
  const audio = createPeerAudio({
    sdk: { Room, RoomEvent },
    container: { append() {} },
    enableSound: new Button(),
    onReconnect: async () => {
      recoveries++;
    },
  });
  const room = await audio.connect("wss://example", "token");
  room.emit("track", track);
  room.emit("track", track);
  assert.equal(attaches, 1);
  room.emit("untrack", track);
  assert.equal(detaches, 1);
  room.emit("track", track);
  room.remoteParticipants.set("partner", {
    trackPublications: new Map([["p", { track }]]),
  });
  room.emit("reconnected");
  assert.equal(detaches, 2);
  assert.equal(attaches, 3);
  assert.equal(recoveries, 1);
  room.localParticipant.trackPublications.set("local", {
    track: {
      stop() {
        stops++;
      },
    },
  });
  await audio.disconnect();
  assert.equal(stops, 1);
  assert.equal(detaches, 3);
  assert.equal(room.disconnected, true);
});

import { launchChromium, startStaticServer } from "./source.js";
let browser, server, base;
before(async () => {
  browser = await launchChromium();
  if (browser) ({ server, base } = await startStaticServer());
});
after(async () => {
  await browser?.close();
  await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
});

test("peer interview renders partner code read-only and only the interviewer guide", async (t) => {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  const failures = [];
  page.on("pageerror", (failure) => failures.push(failure.message));
  const state = {
    sessionId: "s",
    status: "active",
    version: 3,
    serverNow: Math.floor(Date.now() / 1000),
    you: { slot: 0 },
    settings: {
      language: "javascript",
      saveTranscript: false,
      saveAudio: true,
    },
    members: [
      { slot: 0, identity: "a", login: "alice" },
      { slot: 1, identity: "b", login: "bob" },
    ],
    currentRound: {
      index: 0,
      status: "active",
      role: "interviewer",
      deadline: Math.floor(Date.now() / 1000) + 1500,
    },
  };
  await page.route("**/api/peer/sessions/s", (route) =>
    route.fulfill({ json: state }),
  );
  await page.route("**/api/peer/sessions/s/rounds/0/problem", (route) =>
    route.fulfill({
      json: { page: "chargeback-pair-match", title: "Chargeback Pair Match" },
    }),
  );
  await page.route("**/api/peer/sessions/s/rounds/0/guide", (route) =>
    route.fulfill({
      json: { contract: "Find the pair", hints: ["one", "two", "three"] },
    }),
  );
  await page.route("**/api/peer/sessions/s/rounds/0/snapshot", (route) =>
    route.fulfill({
      json: {
        revision: 2,
        language: "javascript",
        code: "const partner = 42;",
        tests: {
          passed: 0,
          total: 1,
          cases: [
            {
              label: "pair case",
              pass: false,
              input: "[1,2]",
              expected: "[0,1]",
              got: "[]",
              timeMs: 1,
            },
          ],
        },
      },
    }),
  );
  let captureStopped = false;
  await page.route("**/api/peer/sessions/s/audio/status", (route) =>
    route.fulfill({
      json: { status: captureStopped ? "deleted" : "recording" },
    }),
  );
  await page.route("**/api/peer/sessions/s/audio", (route) => {
    assert.equal(route.request().method(), "DELETE");
    captureStopped = true;
    return route.fulfill({ json: { status: "deleted" } });
  });
  await page.goto(`${base}/peer-interview.html?session=s`);
  await page.waitForFunction(
    () => document.getElementById("editor").value === "const partner = 42;",
  );
  assert.equal(await page.locator("#editor").getAttribute("readonly"), "");
  await page.locator("#editor").focus();
  await page.keyboard.type("x");
  assert.equal(
    await page.locator("#editor").inputValue(),
    "const partner = 42;",
  );
  assert.equal(await page.locator("#peer-run").isDisabled(), true);
  assert.match(
    await page.locator("#peer-guide").textContent(),
    /Find the pair/,
  );
  assert.match(await page.locator("#peer-results").textContent(), /pair case/);
  assert.match(
    await page.locator("#peer-results").textContent(),
    /expected \[0,1\]/,
  );
  assert.doesNotMatch(
    await page.locator("#peer-problem").textContent(),
    /Jim is listening/,
  );
  await page.locator("#peer-stop-saved-audio").waitFor({ state: "visible" });
  await page.locator("#peer-stop-saved-audio").click();
  await page.waitForFunction(() =>
    document
      .getElementById("peer-audio-capture-status")
      .textContent.includes("stopped and deleted"),
  );
  assert.equal(captureStopped, true);
  assert.equal(state.settings.saveAudio, true);
  await page.setViewportSize({ width: 390, height: 844 });
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    true,
    JSON.stringify(
      await page.evaluate(() =>
        [...document.querySelectorAll("*")]
          .filter((node) => node.getBoundingClientRect().right > innerWidth)
          .map((node) => ({
            tag: node.tagName,
            id: node.id,
            class: node.className,
            right: node.getBoundingClientRect().right,
          })),
      ),
    ),
  );
  await page.screenshot({
    path: "/tmp/codetrial-peer-interview-mobile.png",
    fullPage: true,
  });
  assert.deepEqual(failures, []);
});

test("prestart requires entering, connected audio, readiness and explicit observer consent", async (t) => {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  const failures = [];
  page.on("pageerror", (failure) => failures.push(failure.message));
  let starts = 0;
  const state = {
    sessionId: "pre",
    status: "pending",
    version: 7,
    serverNow: Math.floor(Date.now() / 1000),
    settings: {
      difficulty: "easy",
      language: "python",
      roundMinutes: 25,
      saveTranscript: true,
      saveAudio: false,
    },
    you: { slot: 0, observerConsented: false },
    members: [
      {
        slot: 0,
        login: "alice",
        nickname: "Lucky7",
        identity: "a",
        entered: false,
        ready: false,
      },
      {
        slot: 1,
        login: "bob",
        nickname: "Runner2",
        identity: "b",
        entered: true,
        ready: true,
      },
    ],
    currentRound: null,
  };
  await page.route("**/vendor/livekit-client.js", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body: `window.LivekitClient={RoomEvent:{TrackSubscribed:'track',TrackUnsubscribed:'untrack',Disconnected:'disconnected',Reconnected:'reconnected',DataReceived:'data'},Room:class{constructor(){window.peerTestRoom=this}handlers=new Map();remoteParticipants=new Map();localParticipant={trackPublications:new Map(),setMicrophoneEnabled:async()=>{}};on(k,f){this.handlers.set(k,f)}async connect(){}async disconnect(){}}};`,
    }),
  );
  await page.route("**/api/peer/sessions/pre", (route) =>
    route.fulfill({ json: state }),
  );
  await page.route("**/api/peer/sessions/pre/enter", (route) => {
    state.members[0].entered = true;
    state.status = "lobby";
    return route.fulfill({ json: state });
  });
  await page.route("**/api/peer/sessions/pre/token", (route) =>
    route.fulfill({ json: { url: "wss://example", token: "token" } }),
  );
  await page.route("**/api/peer/sessions/pre/ready", (route) => {
    const payload = route.request().postDataJSON();
    assert.equal(payload.expectedVersion, state.version);
    state.members[0].ready = payload.ready;
    state.version++;
    return route.fulfill({ json: state });
  });
  await page.route("**/api/peer/sessions/pre/start", (route) => {
    starts++;
    assert.deepEqual(route.request().postDataJSON(), {
      expectedVersion: 8,
      observerConsent: true,
    });
    return route.fulfill({ json: state });
  });
  await page.goto(`${base}/peer-interview.html?session=pre`);
  await page.locator("#peer-prestart").waitFor({ state: "visible" });
  assert.equal(await page.locator("#peer-session-ready").isDisabled(), true);
  assert.equal(await page.locator("#peer-session-start").isDisabled(), true);
  assert.match(
    await page.locator("#peer-prestart-settings").textContent(),
    /Save transcript: yes. Save audio: no/,
  );
  await page.locator("#peer-session-enter").click();
  await page.waitForFunction(
    () => document.getElementById("peer-session-enter").disabled,
  );
  await page.locator("#peer-connect").click();
  await page.waitForFunction(
    () => !document.getElementById("peer-session-ready").disabled,
  );
  await page.locator("#peer-session-ready").click();
  await page.waitForFunction(() =>
    document
      .getElementById("peer-prestart-members")
      .textContent.includes("Lucky7: ready"),
  );
  assert.equal(await page.locator("#peer-session-start").isDisabled(), true);
  assert.equal(starts, 0);
  await page.locator("#peer-session-consent").check();
  await page.locator("#peer-session-start").click();
  await page.waitForFunction(() =>
    document.getElementById("peer-error").textContent.includes("Waiting"),
  );
  assert.equal(starts, 1);
  const reset = page.waitForRequest(
    (request) =>
      request.url().endsWith("/ready") &&
      request.postDataJSON().ready === false,
  );
  await page.evaluate(() => window.peerTestRoom.handlers.get("disconnected")());
  assert.deepEqual((await reset).postDataJSON(), {
    ready: false,
    expectedVersion: 8,
  });
  assert.deepEqual(failures, []);
});

test("disconnect during microphone permission cleans late tracks and cancels connection", async () => {
  let resolveMicrophone;
  let microphoneStarted;
  const started = new Promise((resolve) => {
    microphoneStarted = resolve;
  });
  let stops = 0;
  class DeferredRoom extends Room {
    constructor() {
      super();
      this.localParticipant.setMicrophoneEnabled = () => {
        microphoneStarted();
        return new Promise((resolve) => {
          resolveMicrophone = () => {
            this.localParticipant.trackPublications.set("late", {
              track: {
                stop() {
                  stops++;
                },
              },
            });
            resolve();
          };
        });
      };
    }
  }
  const audio = createPeerAudio({
    sdk: { Room: DeferredRoom, RoomEvent },
    container: { append() {} },
    enableSound: new Button(),
  });
  const connecting = audio.connect("wss://example", "token");
  await started;
  await audio.disconnect();
  resolveMicrophone();
  assert.equal(await connecting, null);
  assert.equal(stops, 1);
});

test("a server-side disconnect releases the dead room so mute does nothing to it", async () => {
  const enabled = [];
  class DeadRoom extends Room {
    constructor() {
      super();
      this.localParticipant.setMicrophoneEnabled = async (value) => {
        enabled.push(value);
      };
    }
  }
  let dropped = 0;
  const audio = createPeerAudio({
    sdk: { Room: DeadRoom, RoomEvent },
    container: { append() {} },
    enableSound: new Button(),
    onDisconnect: () => dropped++,
  });
  const connected = await audio.connect("wss://example", "token");
  connected.emit(RoomEvent.Disconnected);
  assert.equal(dropped, 1);
  enabled.length = 0;
  await audio.mute(true);
  assert.deepEqual(enabled, []);
});

test("reconnecting peer audio preserves the requested muted microphone state", async () => {
  const enabled = [];
  class MutedRoom extends Room {
    constructor() {
      super();
      this.localParticipant.setMicrophoneEnabled = async (value) => {
        enabled.push(value);
      };
    }
  }
  const audio = createPeerAudio({
    sdk: { Room: MutedRoom, RoomEvent },
    container: { append() {} },
    enableSound: new Button(),
  });
  await audio.connect("wss://example", "token");
  await audio.mute(true);
  await audio.connect("wss://example", "fresh-token");
  assert.deepEqual(enabled, [true, false, false]);
  await audio.disconnect();
});
test("rapid mute clicks alternate and leave UI state matching the microphone", async () => {
  const calls = [];
  const pending = [];
  const shown = [];
  const toggle = createMuteToggle({
    mute: (value) => {
      calls.push(value);
      return new Promise((resolve) => pending.push(resolve));
    },
    onChange: (value) => shown.push(value),
    onError: () => {},
  });
  const first = toggle();
  const second = toggle();
  for (let turn = 0; turn < 2; turn++) {
    for (let spin = 0; pending.length < 1 && spin < 1000; spin++)
      await Promise.resolve();
    assert.ok(pending.length, `mute call ${turn + 1} never started`);
    pending.shift()();
  }
  await Promise.all([first, second]);
  assert.deepEqual(calls, [true, false]);
  assert.deepEqual(shown, [true, false]);
});
test("a failed mute leaves the next click requesting the applied state", async () => {
  const calls = [];
  let fail = true;
  const toggle = createMuteToggle({
    mute: async (value) => {
      calls.push(value);
      if (fail) {
        fail = false;
        throw new Error("denied");
      }
    },
    onChange: () => {},
    onError: () => {},
  });
  await toggle();
  await toggle();
  assert.deepEqual(calls, [true, true]);
});
test("a failed mute does not corrupt the intent of clicks queued behind it", async () => {
  const calls = [];
  let failFirst = true;
  const toggle = createMuteToggle({
    mute: async (value) => {
      calls.push(value);
      if (failFirst) {
        failFirst = false;
        throw new Error("denied");
      }
    },
    onChange: () => {},
    onError: () => {},
  });
  await Promise.all([toggle(), toggle(), toggle()]);
  await toggle();
  assert.deepEqual(calls, [true, false, true, false]);
});
