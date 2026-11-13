import { test } from "node:test";
import assert from "node:assert/strict";
import { loadInterview } from "./source.js";

function node() {
  const classes = new Set();
  return {
    hidden: false,
    disabled: false,
    textContent: "",
    attributes: {},
    classList: {
      toggle(name, enabled) {
        if (enabled) classes.add(name);
        else classes.delete(name);
      },
      contains: (name) => classes.has(name),
    },
    set innerHTML(html) {
      this.textContent = html.replace(/<[^>]*>/g, "");
    },
    setAttribute(name, value) {
      this.attributes[name] = value;
    },
    closest() {
      return this;
    },
  };
}

function microphonePage(overrides = {}, scope = {}) {
  const tracks = [{ enabled: true, readyState: "live" }];
  const nodes = {
    mic: node(),
    micMutedStatus: node(),
    micControlStatus: node(),
    mutedSpeechConsent: { checked: true },
    mutedSpeechReminder: node(),
    agentState: node(),
  };
  nodes.mutedSpeechReminder.hidden = true;
  nodes.agentState.textContent = "Listening";
  const state = {
    phase: "live",
    micEnabled: true,
    micChanging: false,
    localUserStream: { getAudioTracks: () => tracks },
    room: {},
    ...overrides,
  };
  const events = [];
  const meters = [];
  const switchCalls = [];
  const watched = [];
  const api = loadInterview(
    [
      "toggleMicrophone",
      "paintMicrophoneState",
      "setLocalAudioEnabled",
      "setAgentStateLabel",
      "watchMutedSpeech",
      "stopLocalMedia",
    ],
    {
      state,
      nodes,
      localRecording: { finish() {} },
      stopIntegrityWorker() {},
      stopPreflight() {},
      forgetAudioInputs() {},
      publishIntegrityEvent: (event) => events.push(event),
      startTurnRing: (stream) => meters.push(stream),
      mutedSpeech: {
        watch: (track) => watched.push(track),
        stop: () => watched.push("stop"),
      },
      publishedMicrophone: () => null,
      microphoneSwitch: () => ({
        setEnabled: (enabled) => switchCalls.push(enabled),
      }),
      MediaStream: class {
        constructor(tracks) {
          this.tracks = tracks;
        }
      },
      ...scope,
    },
  );
  api.paintMicrophoneState();
  return {
    ...api,
    nodes,
    state,
    tracks,
    events,
    meters,
    switchCalls,
    watched,
  };
}

test("mute updates the track, visible text and accessible action together", async () => {
  const page = microphonePage();
  assert.equal(page.nodes.mic.textContent, "Mic on");
  assert.equal(page.nodes.micMutedStatus.hidden, true);
  await page.toggleMicrophone();
  assert.equal(page.tracks[0].enabled, false);
  assert.equal(page.state.micEnabled, false);
  assert.equal(page.nodes.mic.textContent, "Muted");
  assert.equal(page.nodes.micMutedStatus.hidden, false);
  assert.equal(
    page.nodes.mic.attributes["aria-label"],
    "Muted: unmute microphone",
  );
  assert.equal(page.nodes.mic.classList.contains("is-muted"), true);
  assert.equal(page.nodes.mic.disabled, false);
  assert.equal(page.nodes.mic.attributes["aria-disabled"], "false");
  assert.equal(page.nodes.agentState.textContent, "Listening");
  assert.deepEqual(
    page.events.map((event) => event.type),
    ["MICROPHONE_STOPPED"],
  );
  await page.toggleMicrophone();
  assert.equal(page.tracks[0].enabled, true);
  assert.equal(page.nodes.mic.textContent, "Mic on");
  assert.equal(page.nodes.micMutedStatus.hidden, true);
  assert.equal(
    page.nodes.mic.attributes["aria-label"],
    "Mic on: mute microphone",
  );
  assert.equal(page.nodes.mic.classList.contains("is-muted"), false);
  assert.deepEqual(page.switchCalls, [false, true]);
});

test("mute accepts ended or missing tracks and still reaches a pending input switch", async () => {
  for (const tracks of [[{ enabled: true, readyState: "ended" }], []]) {
    const page = microphonePage({
      localUserStream: { getAudioTracks: () => tracks },
    });
    await page.toggleMicrophone();
    assert.equal(page.state.micEnabled, false);
    assert.equal(page.nodes.mic.textContent, "Muted");
    assert.equal(page.nodes.micMutedStatus.hidden, false);
    assert.equal(page.nodes.micControlStatus.hidden, true);
    assert.deepEqual(page.switchCalls, [false]);
    assert.ok(tracks.every((track) => !track.enabled));

    // An ended or absent input can be disabled, but cannot be made live again.
    await page.toggleMicrophone();
    assert.equal(page.state.micEnabled, false);
    assert.equal(page.nodes.micMutedStatus.hidden, false);
    assert.equal(page.nodes.micControlStatus.hidden, false);
    assert.deepEqual(page.switchCalls, [false]);
  }
});

test("interviewer status changes cannot clear a confirmed mute indicator", async () => {
  const page = microphonePage();
  await page.toggleMicrophone();
  for (const label of ["Thinking...", "Speaking", "Listening", "Waiting"]) {
    page.setAgentStateLabel(label, label === "Listening");
    assert.equal(page.nodes.agentState.textContent, label);
    assert.equal(page.nodes.mic.textContent, "Muted");
    assert.equal(page.nodes.micMutedStatus.hidden, false);
    assert.equal(
      page.nodes.mic.attributes["aria-label"],
      "Muted: unmute microphone",
    );
  }
});

test("a failed SDK unmute preserves both mute indicators and permits retry", async () => {
  let refused = true;
  const track = { enabled: false, readyState: "live" };
  const page = microphonePage({
    micEnabled: false,
    localUserStream: null,
    room: {
      localParticipant: {
        async setMicrophoneEnabled(enabled) {
          if (refused) throw new Error("permission refused");
          track.enabled = enabled;
          return { track: { mediaStreamTrack: track } };
        },
      },
    },
  });
  await page.toggleMicrophone();
  assert.equal(page.state.micEnabled, false);
  assert.equal(page.nodes.mic.textContent, "Muted");
  assert.equal(page.nodes.micMutedStatus.hidden, false);
  assert.equal(
    page.nodes.mic.attributes["aria-label"],
    "Muted: unmute microphone",
  );
  assert.equal(page.nodes.micControlStatus.hidden, false);
  assert.match(page.nodes.micControlStatus.textContent, /still muted/);
  assert.equal(page.nodes.mic.attributes["aria-disabled"], "false");
  refused = false;
  await page.toggleMicrophone();
  assert.equal(page.state.micEnabled, true);
  assert.equal(page.nodes.micMutedStatus.hidden, true);
  assert.equal(page.nodes.micControlStatus.hidden, true);
  assert.equal(page.meters.length, 1);
});

test("a pending unmute keeps the confirmed state and rejects duplicate activation", async () => {
  let resolve;
  let calls = 0;
  const page = microphonePage({
    micEnabled: false,
    localUserStream: null,
    room: {
      localParticipant: {
        setMicrophoneEnabled() {
          calls++;
          return new Promise((done) => {
            resolve = done;
          });
        },
      },
    },
  });
  const pending = page.toggleMicrophone();
  await page.toggleMicrophone();
  assert.equal(calls, 1);
  assert.equal(page.state.micEnabled, false);
  assert.equal(page.nodes.micMutedStatus.hidden, false);
  assert.equal(page.nodes.mic.attributes["aria-disabled"], "true");
  assert.equal(page.nodes.mic.disabled, false, "native focus is retained");
  resolve({
    track: { mediaStreamTrack: { readyState: "live", enabled: true } },
  });
  await pending;
  assert.equal(page.state.micEnabled, true);
  assert.equal(page.nodes.micMutedStatus.hidden, true);
});

for (const initiallyEnabled of [false, true]) {
  test(`a late ${initiallyEnabled ? "mute" : "unmute"} after ending reflects the actual published track`, async () => {
    let finish;
    const track = { enabled: initiallyEnabled, readyState: "live" };
    const ending = microphonePage({
      micEnabled: initiallyEnabled,
      localUserStream: null,
      room: {
        localParticipant: {
          setMicrophoneEnabled: (enabled) =>
            new Promise((done) => {
              finish = () => {
                track.enabled = enabled;
                done({ track: { mediaStreamTrack: track } });
              };
            }),
        },
      },
    });
    const pending = ending.toggleMicrophone();
    ending.state.phase = "ending";
    finish();
    await pending;
    assert.equal(track.enabled, !initiallyEnabled);
    assert.equal(ending.state.micEnabled, track.enabled);
    assert.equal(ending.nodes.micMutedStatus.hidden, track.enabled);
    assert.equal(
      ending.nodes.mic.textContent,
      track.enabled ? "Mic on" : "Muted",
    );
    assert.equal(
      ending.nodes.mic.attributes["aria-label"],
      track.enabled ? "Mic on: mute microphone" : "Muted: unmute microphone",
    );
    assert.equal(ending.state.micChanging, false);
    assert.equal(ending.nodes.mic.attributes["aria-disabled"], "false");
  });
}

test("mute watches the muted input for speech, and only a successful unmute clears the reminder", async () => {
  const page = microphonePage();
  await page.toggleMicrophone();
  assert.deepEqual(page.watched, [page.tracks[0]]);

  page.nodes.mutedSpeechReminder.hidden = false;
  const enabled = page.tracks[0];
  page.tracks[0] = {
    readyState: "live",
    get enabled() {
      return false;
    },
    set enabled(value) {
      if (value) throw new Error("device unavailable");
    },
  };
  await page.toggleMicrophone();
  assert.equal(page.state.micEnabled, false);
  assert.equal(page.nodes.mutedSpeechReminder.hidden, false);

  page.tracks[0] = enabled;
  await page.toggleMicrophone();
  assert.equal(page.state.micEnabled, true);
  assert.equal(page.nodes.mutedSpeechReminder.hidden, true);
  assert.equal(page.watched.at(-1), "stop");
});

test("without consent a mute starts no watch", async () => {
  const page = microphonePage();
  page.nodes.mutedSpeechConsent.checked = false;
  await page.toggleMicrophone();
  assert.deepEqual(page.watched, ["stop"]);
});

test("the end of the interview releases the watch", () => {
  const calls = [];
  const { stopLocalMedia } = loadInterview(
    ["stopLocalMedia"],
    {
      state: { localUserStream: {}, integrityTrackStates: new Map() },
      mutedSpeech: { stop: () => calls.push("stop") },
    },
    { stubMissing: true },
  );
  stopLocalMedia();
  assert.deepEqual(calls, ["stop"]);
});

// The switched-in input is a new track: a watch left on the old one would read
// a device the switch has already stopped.
test("a completed microphone switch moves the watch to the new input", () => {
  let options;
  const watched = [];
  const { microphoneSwitch } = loadInterview(
    ["microphoneSwitch"],
    {
      microphoneSwitchInstance: null,
      createMicrophoneSwitch: (given) => {
        options = given;
        return {};
      },
      watchMutedSpeech: () => watched.push(true),
    },
    { stubMissing: true },
  );
  microphoneSwitch();
  options.onSwitched({ kind: "audio" }, "chosen");
  assert.deepEqual(watched, [true]);
});

// The interview can end while a mute is still on its way through LiveKit. Its
// late success must not start a watch that `stopLocalMedia` will never see.
test("a mute that resolves after the interview ended starts no watch", async () => {
  let finish;
  const published = { enabled: true, readyState: "live" };
  const page = microphonePage(
    {
      localUserStream: null,
      integrityTrackStates: new Map(),
      room: {
        localParticipant: {
          setMicrophoneEnabled: (enabled) =>
            new Promise((done) => {
              finish = () => {
                published.enabled = enabled;
                done({ track: { mediaStreamTrack: published } });
              };
            }),
        },
      },
    },
    { publishedMicrophone: () => ({ mediaStreamTrack: published }) },
  );
  const pending = page.toggleMicrophone();
  page.state.phase = "ending";
  page.stopLocalMedia();
  finish();
  await pending;
  assert.equal(page.state.micEnabled, false);
  assert.deepEqual(
    page.watched.filter((entry) => entry !== "stop"),
    [],
  );
});

// A dropped room and offline practice both leave the phase live with the
// microphone still running, and there is no Jim to be muted to.
test("mute starts no watch without a room", async () => {
  const page = microphonePage({ room: null });
  await page.toggleMicrophone();
  assert.equal(page.state.micEnabled, false);
  assert.deepEqual(page.watched, ["stop"]);
});

// Called again while still muted (a switch, a repaint), it starts nothing once
// the reminder is showing: the reminder stands for the whole muted stretch.
test("no fresh watch once it has reminded", async () => {
  const page = microphonePage();
  await page.toggleMicrophone();
  assert.deepEqual(page.watched, [page.tracks[0]]);
  page.nodes.mutedSpeechReminder.hidden = false;
  page.watchMutedSpeech();
  assert.deepEqual(page.watched, [page.tracks[0]]);
});
