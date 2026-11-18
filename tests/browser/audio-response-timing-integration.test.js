import { test } from "node:test";
import assert from "node:assert/strict";
import { runInNewContext } from "node:vm";
import { createAudioTimingController } from "../../web/audio-response-timing.js";
import { functionBody, loadInterview, read } from "./source.js";

async function fixture({
  failFirstMonitor = false,
  deferPublication = false,
} = {}) {
  const page = read("web/interview.js");
  const monitors = [];
  const measurements = [];
  const diagnostics = [];
  const playbackReleases = [];
  const avatarTracks = [];
  const forgotten = [];
  const disconnects = [];
  const integrityStops = [];
  const clearedTimers = [];
  const publication = Promise.withResolvers();
  const publishing = Promise.withResolvers();
  let time = 0;
  let monitorAttempts = 0;
  let interviewerPresent = true;
  const audioTiming = createAudioTimingController({
    now: () => time,
    report: (elapsed) => measurements.push(elapsed),
    diagnose: (reason) => diagnostics.push(reason),
    startMonitor(track, onActivity, options) {
      monitorAttempts++;
      if (failFirstMonitor && monitorAttempts === 1)
        throw new Error("monitor setup failed");
      const monitor = { track, onActivity, ...options, stopped: false };
      monitors.push(monitor);
      return () => {
        monitor.stopped = true;
      };
    },
  });
  class Room {
    handlers = new Map();
    on(event, callback) {
      const handlers = this.handlers.get(event) || [];
      handlers.push(callback);
      this.handlers.set(event, handlers);
      return this;
    }
    emit(event, ...args) {
      for (const callback of this.handlers.get(event) || []) callback(...args);
    }
    async connect() {}
    async disconnect(stopTracks) {
      disconnects.push(stopTracks);
      this.emit("Disconnected");
    }
    registerTextStreamHandler() {}
  }
  const state = {
    phase: "live",
    connected: false,
    sawAgent: true,
    integrityTrackStates: new Map(),
  };
  const agent = {
    identity: "agent-test",
    attributes: { "lk.agent.state": "listening" },
  };
  const context = {
    state,
    isAgent: (participant) => participant === agent,
    roomParticipants: () => (interviewerPresent ? [agent] : []),
    queueMicrotask,
    window: {
      LivekitClient: {
        Room,
        RoomEvent: new Proxy({}, { get: (_, event) => event }),
      },
    },
    audioPlayback: {
      setRoom() {},
      play: async () => {},
      forget: (element) => forgotten.push(element),
      release: (room) => playbackReleases.push(room),
    },
    audioTiming,
    strict: false,
    whiteboard: false,
    topics: {},
    console: { warn() {} },
    nodes: { mutedSpeechReminder: {} },
    mutedSpeech: { stop() {} },
    localRecording: { needsMicrophone: false },
    stopTurnRing() {},
    stopIntegrityWorker() {
      integrityStops.push("stopped");
    },
    clearInterval(timer) {
      clearedTimers.push(timer);
    },
    isCurrentAgent: (participant) => participant === agent,
    attachAvatarAnalyser(track) {
      avatarTracks.push(track);
    },
    isAvatarAnalyserTrack: () => false,
    document: { createElement: () => ({ isConnected: true }) },
    jimVolume: { setRoutingHeld() {}, apply() {} },
    applyAudioOutput() {},
    audioOutputId() {},
    audioOutputSettled: async () => {},
    setTimeout() {},
    clearTimeout() {},
    releaseStart() {},
    stopLocalMedia() {},
    publishPreflightTracks: async () => {
      publishing.resolve();
      if (deferPublication) await publication.promise;
    },
    paintMicrophoneState() {},
    monitorIntegrityTracks() {},
    followMicrophoneLoss() {},
    refreshAudioInputs: async () => {},
    publishIntegrityEvent: async () => {},
    startIntegrityHeartbeat() {},
    setBanner() {},
    providerUiState: () => ({ message: "connection unavailable" }),
    reconnectCodingInterview: async () => {},
    exampleBoard: {},
    flushPendingPublishes() {},
    publishCode() {},
  };
  const code = `
    let jimTrack = null;
    let jimAudio = null;
    let detachSelfView = () => {};
    let recordingMicrophoneDeviceId = null;
    ${[
      "playRemoteAudio",
      "dropRemoteAudio",
      "stopPreflight",
      "stopIntegrityMonitoring",
      "abandonStart",
      "connectLiveKit",
    ]
      .map((name) => functionBody(page, name) + "\n}")
      .join("\n")}
    globalThis.timingApi = {
      connectLiveKit, abandonStart,
    };
  `;
  runInNewContext(code, context);
  const scope = {
    Boolean,
    state,
    audioTiming,
    isAgent: (participant) => participant === agent,
    roomParticipants: () => (interviewerPresent ? [agent] : []),
    roomInterviewer: () => (interviewerPresent ? agent : null),
  };
  const { updateAgentState } = loadInterview(["updateAgentState"], scope, {
    stubMissing: true,
  });
  context.updateAgentState = updateAgentState;
  const joining = context.timingApi.connectLiveKit({}, { cameraSkipped: true });
  await publishing.promise;
  if (!deferPublication) assert.equal(await joining, "joined");
  const room = state.room;
  for (const event of ["Reconnecting", "Reconnected", "Disconnected"]) {
    assert.equal(
      room.handlers.get(event)?.length,
      1,
      `${event} is registered by connectLiveKit`,
    );
  }
  agent.attributes["lk.agent.state"] = "speaking";
  return {
    state,
    room,
    agent,
    avatarTracks,
    diagnostics,
    measurements,
    playbackReleases,
    forgotten,
    disconnects,
    integrityStops,
    clearedTimers,
    joining,
    finishPublication: publication.resolve,
    advance: (ms) => {
      time += ms;
    },
    disconnect: () => room.emit("Disconnected"),
    api: context.timingApi,
    monitors,
    monitorAttempts: () => monitorAttempts,
    setInterviewerPresent: (present) => {
      interviewerPresent = present;
    },
  };
}

function remoteTrack(id) {
  return {
    id,
    kind: "audio",
    attach() {},
    mediaStreamTrack: {
      kind: "audio",
      readyState: "live",
    },
  };
}

for (const readyState of ["live", "ended"]) {
  test(`registered reconnect handlers respect a retained ${readyState} track`, async () => {
    const f = await fixture();
    const track = remoteTrack("A");
    f.agent.attributes["lk.agent.state"] = "listening";
    f.room.emit("TrackSubscribed", track, {}, f.agent);
    const old = f.monitors[0];
    old.onAvailable();
    f.agent.attributes["lk.agent.state"] = "speaking";
    f.room.emit("ParticipantAttributesChanged");
    old.onQuiet();
    track.mediaStreamTrack.readyState = readyState;
    f.room.emit("Reconnecting");
    assert.equal(f.state.connected, false);
    assert.equal(old.stopped, true);
    f.room.emit("Reconnected");
    assert.equal(f.state.connected, true);
    assert.equal(f.monitors.length, readyState === "live" ? 2 : 1);
    old.onAvailable();
    old.onQuiet();
    f.advance(200);
    old.onActivity();
    if (readyState === "live") {
      const current = f.monitors[1];
      current.onAvailable();
      current.onQuiet();
      current.onActivity();
      assert.equal(current.stopped, false);
    }
    assert.deepEqual(
      f.measurements,
      [],
      "recovery cannot finish the interrupted turn",
    );
    assert.deepEqual(f.diagnostics, ["room reconnecting"]);
    f.disconnect();
    assert.ok(f.monitors.every((monitor) => monitor.stopped));
    assert.deepEqual(f.playbackReleases, [f.room]);
  });
}

test("console diagnostics are installed only with audioTiming=1", () => {
  const source = read("web/interview.js");
  const setup = source.slice(
    source.indexOf("const params = new URLSearchParams"),
    source.indexOf("const editorOptions"),
  );
  for (const search of ["", "?audioTiming=0", "?audioTiming=1"]) {
    const logs = [];
    let created = 0;
    const context = {
      URLSearchParams,
      window: { location: { search } },
      console: { debug: (message) => logs.push(message) },
      createAudioTimingController(options) {
        created++;
        return createAudioTimingController(options);
      },
    };
    runInNewContext(
      setup + "; audioTiming?.speakingChanged(true, true);",
      context,
    );
    assert.equal(created, search === "?audioTiming=1" ? 1 : 0);
    assert.deepEqual(
      logs,
      search === "?audioTiming=1"
        ? ["[audio-timing] Turn not measured: monitor not ready"]
        : [],
    );
  }
});

test("registered stale reconnect callbacks preserve the newer room and monitor", async () => {
  const f = await fixture();
  f.room.emit("TrackSubscribed", remoteTrack("A"), {}, f.agent);
  await f.api.connectLiveKit({}, { cameraSkipped: true });
  const newerRoom = f.state.room;
  newerRoom.emit("TrackSubscribed", remoteTrack("B"), {}, f.agent);
  f.monitors[1].onAvailable();
  f.room.emit("Reconnecting");
  f.room.emit("Reconnected");
  f.room.emit("Disconnected");
  assert.equal(f.state.room, newerRoom);
  assert.equal(f.state.connected, true);
  assert.equal(f.monitors.length, 2);
  assert.equal(f.monitors[1].stopped, false);
});

test("monitor setup failure clears the track and reconnect retries the diagnostic", async () => {
  const f = await fixture({ failFirstMonitor: true });
  const track = remoteTrack("A");
  f.agent.attributes["lk.agent.state"] = "listening";
  f.room.emit("TrackSubscribed", track, {}, f.agent);
  assert.equal(f.monitorAttempts(), 1);
  assert.equal(f.monitors.length, 0);

  f.room.emit("Reconnected");
  assert.equal(f.monitorAttempts(), 2);
  assert.equal(f.monitors.length, 1);
  const monitor = f.monitors[0];
  monitor.onAvailable();
  f.agent.attributes["lk.agent.state"] = "speaking";
  f.room.emit("ParticipantAttributesChanged");
  monitor.onQuiet();
  f.advance(80);
  monitor.onActivity();
  assert.deepEqual(f.measurements, [80]);
  f.disconnect();
});

test("an unavailable interviewer cancels a pending turn and can speak again", async () => {
  const f = await fixture();
  f.agent.attributes["lk.agent.state"] = "listening";
  f.room.emit("TrackSubscribed", remoteTrack("A"), {}, f.agent);
  const monitor = f.monitors[0];
  monitor.onAvailable();
  f.agent.attributes["lk.agent.state"] = "speaking";
  f.room.emit("ParticipantAttributesChanged");
  monitor.onQuiet();
  f.advance(40);

  f.setInterviewerPresent(false);
  f.room.emit("ParticipantDisconnected", f.agent);
  await Promise.resolve();
  assert.equal(f.state.interviewerPresent, false);
  assert.deepEqual(f.diagnostics, ["interviewer unavailable"]);
  f.advance(80);
  monitor.onActivity();
  assert.deepEqual(f.measurements, []);

  monitor.onQuiet();
  f.setInterviewerPresent(true);
  f.room.emit("ParticipantConnected", f.agent);
  assert.equal(f.state.interviewerPresent, true);
  monitor.onQuiet();
  f.advance(60);
  monitor.onActivity();
  assert.deepEqual(f.measurements, [60]);
  assert.deepEqual(f.diagnostics, ["interviewer unavailable"]);
  f.disconnect();
});

test("registered unsubscribe releases only the current audio monitor", async () => {
  const f = await fixture();
  f.agent.attributes["lk.agent.state"] = "listening";
  const oldTrack = remoteTrack("A");
  const currentTrack = remoteTrack("B");
  const removed = [];
  const element = { remove: () => removed.push(element) };
  oldTrack.detach = () => [];
  currentTrack.detach = () => [element];
  f.room.emit("TrackSubscribed", oldTrack, {}, f.agent);
  f.room.emit("TrackSubscribed", currentTrack, {}, f.agent);
  const current = f.monitors[1];
  current.onAvailable();
  f.agent.attributes["lk.agent.state"] = "speaking";
  f.room.emit("ParticipantAttributesChanged");
  current.onQuiet();

  f.room.emit("TrackUnsubscribed", oldTrack);
  assert.equal(f.monitors[0].stopped, true);
  assert.equal(current.stopped, false);
  f.room.emit("TrackUnsubscribed", currentTrack);
  assert.equal(current.stopped, true);
  assert.deepEqual(f.forgotten, [element]);
  assert.deepEqual(removed, [element]);
  current.onAvailable();
  current.onQuiet();
  f.advance(80);
  current.onActivity();
  assert.deepEqual(f.measurements, []);
  assert.deepEqual(f.diagnostics, ["audio monitor released"]);
  f.disconnect();
});

test("speaking during initial publication reports room disconnected", async () => {
  const f = await fixture({ deferPublication: true });
  assert.equal(f.state.joinedRoom, true);
  assert.equal(f.state.connected, false);
  assert.equal(f.state.initialMediaPublished, false);
  f.agent.attributes["lk.agent.state"] = "listening";
  f.room.emit("TrackSubscribed", remoteTrack("A"), {}, f.agent);
  const monitor = f.monitors[0];
  monitor.onAvailable();
  f.agent.attributes["lk.agent.state"] = "speaking";
  f.room.emit("ParticipantAttributesChanged");
  assert.deepEqual(f.diagnostics, ["room disconnected"]);
  monitor.onQuiet();
  f.advance(80);
  monitor.onActivity();
  assert.deepEqual(f.measurements, []);
  f.finishPublication();
  assert.equal(await f.joining, "joined");
  assert.equal(f.state.connected, true);
  assert.deepEqual(f.measurements, []);
  f.disconnect();
});

for (const preserveMicrophone of [false, true]) {
  test(`abandonStart releases the monitor before deferred disconnect (preserveMicrophone=${preserveMicrophone})`, async () => {
    const f = await fixture();
    f.agent.attributes["lk.agent.state"] = "listening";
    f.room.emit("TrackSubscribed", remoteTrack("A"), {}, f.agent);
    const monitor = f.monitors[0];
    monitor.onAvailable();
    f.agent.attributes["lk.agent.state"] = "speaking";
    f.room.emit("ParticipantAttributesChanged");
    monitor.onQuiet();
    const audioTrack = {
      kind: "audio",
      readyState: "live",
      stopCount: 0,
      stop() {
        this.stopCount++;
        this.readyState = "ended";
      },
    };

    const videoTrack = {
      kind: "video",
      stopCount: 0,
      stop() {
        this.stopCount++;
      },
    };

    const userStream = {
      getTracks: () => [audioTrack, videoTrack],
    };

    const preflight = { userStream };

    f.state.integrityHeartbeatTimer = 101;
    f.state.integrityTrackStateTimer = 102;
    f.state.integrityTrackStates.set("audio", "active");
    const flushed = Promise.withResolvers();
    const leaving = f.api.abandonStart(preflight, {
      preserveMicrophone,
      flushed: flushed.promise,
    });
    assert.equal(videoTrack.stopCount, 1);

    if (preserveMicrophone) {
      assert.equal(audioTrack.stopCount, 0);
      assert.equal(audioTrack.readyState, "live");
      assert.equal(f.state.localUserStream, userStream);

      assert.deepEqual(f.integrityStops, ["stopped"]);
      assert.ok(f.clearedTimers.includes(101));
      assert.ok(f.clearedTimers.includes(102));

      assert.equal(f.state.integrityHeartbeatTimer, null);
      assert.equal(f.state.integrityTrackStateTimer, null);
      assert.equal(f.state.integrityTrackStates.size, 0);
    } else {
      assert.equal(audioTrack.stopCount, 1);
      assert.equal(audioTrack.readyState, "ended");
    }
    assert.deepEqual(f.disconnects, []);
    assert.equal(
      monitor.stopped,
      true,
      "startup failure stops the monitor before disconnect",
    );
    assert.equal(f.state.room, null);
    assert.equal(f.state.connected, false);
    monitor.onAvailable();
    monitor.onQuiet();
    f.advance(80);
    monitor.onActivity();
    assert.deepEqual(f.measurements, []);
    assert.deepEqual(f.diagnostics, ["audio monitor released"]);
    flushed.resolve();
    await leaving;
    assert.deepEqual(f.disconnects, [!preserveMicrophone]);
    assert.equal(f.state.startupCleanupRoom, null);
  });
}

test("reconnected preserves a monitor attached by TrackSubscribed", async () => {
  const f = await fixture();
  f.agent.attributes["lk.agent.state"] = "listening";
  f.room.emit("TrackSubscribed", remoteTrack("A"), {}, f.agent);
  f.monitors[0].onAvailable();
  f.room.emit("Reconnecting");
  const track = remoteTrack("B");
  f.room.emit("TrackSubscribed", track, {}, f.agent);
  const current = f.monitors[1];
  current.onAvailable();
  current.onQuiet();
  f.room.emit("Reconnected");
  assert.equal(f.monitors.length, 2);
  assert.equal(f.monitorAttempts(), 2);
  assert.equal(f.avatarTracks.at(-1), track);
  assert.equal(current.stopped, false);
  f.agent.attributes["lk.agent.state"] = "speaking";
  f.room.emit("ParticipantAttributesChanged");
  current.onQuiet();
  f.advance(80);
  current.onActivity();
  assert.deepEqual(f.measurements, [80]);
  f.disconnect();
});

test("room events do not restart audio timing during report recovery", async () => {
  const f = await fixture();
  const track = remoteTrack("A");
  f.agent.attributes["lk.agent.state"] = "listening";
  f.room.emit("TrackSubscribed", track, {}, f.agent);
  f.room.emit("Reconnecting");
  f.state.phase = "report_recovery";
  f.room.emit("Reconnected");
  f.room.emit("TrackSubscribed", remoteTrack("B"), {}, f.agent);
  f.agent.attributes["lk.agent.state"] = "speaking";
  f.room.emit("ParticipantAttributesChanged");
  assert.equal(f.monitorAttempts(), 1);
  assert.deepEqual(f.diagnostics, []);
  assert.equal(f.state.room, f.room);
  assert.equal(f.state.connected, true);
});
