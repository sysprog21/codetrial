import { test } from "node:test";
import assert from "node:assert/strict";
import { runInNewContext } from "node:vm";
import { createAudioResponseTiming } from "../../web/audio-response-timing.js";
import { functionBody, loadInterview, read } from "./source.js";

async function fixture({
  enabled = true,
  failFirstMonitor = false,
  deferPublication = false,
} = {}) {
  const page = read("web/interview.js");
  const monitors = [];
  const measurements = [];
  const diagnostics = [];
  const playbackReleases = [];
  const forgotten = [];
  const disconnects = [];
  const integrityStops = [];
  const clearedTimers = [];
  const publication = Promise.withResolvers();
  const publishing = Promise.withResolvers();
  let cancellations = 0;
  let time = 0;
  let monitorAttempts = 0;
  let interviewerPresent = true;
  const timing = createAudioResponseTiming({
    now: () => time,
    report: (elapsed) => measurements.push(elapsed),
    diagnose: (reason) => diagnostics.push(reason),
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
      forget: (element) => forgotten.push(element),
      release: (room) => playbackReleases.push(room),
    },
    audioTiming: enabled
      ? {
          ...timing,
          cancelPending(reason) {
            cancellations++;
            timing.cancelPending(reason);
          },
        }
      : null,
    startAudioActivityMonitor(track, onActivity, options) {
      monitorAttempts++;
      if (failFirstMonitor && monitorAttempts === 1)
        throw new Error("monitor setup failed");
      const monitor = { track, onActivity, options, stopped: false };
      monitors.push(monitor);
      return () => {
        monitor.stopped = true;
      };
    },
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
    isAvatarAnalyserTrack: () => false,
    releaseStart() {},
    stopLocalMedia() {},
    playRemoteAudio(track) {
      context.timingApi.setRemoteTrack(track);
    },
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
    let audioTimingTrack = null;
    let audioTimingRoom = null;
    let stopAudioTimingMonitor = null;
    let audioTimingReady = false;
    let jimTrack = null;
    let jimAudio = null;
    let detachSelfView = () => {};
    let recordingMicrophoneDeviceId = null;
    ${[
      "releaseAudioTiming",
      "attachAudioTiming",
      "dropRemoteAudio",
      "stopPreflight",
      "stopIntegrityMonitoring",
      "abandonStart",
      "connectLiveKit",
    ]
      .map((name) => functionBody(page, name) + "\n}")
      .join("\n")}
    globalThis.timingApi = {
      attachAudioTiming, releaseAudioTiming, connectLiveKit, abandonStart,
      setRemoteTrack: (track) => { jimTrack = track; attachAudioTiming(track); },
      currentTrack: () => audioTimingTrack,
      owner: () => audioTimingRoom,
      ready: () => audioTimingReady,
    };
  `;
  runInNewContext(code, context);
  const scope = {
    Boolean,
    state,
    audioTiming: context.audioTiming,
    get audioTimingReady() {
      return context.timingApi.ready();
    },
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
    timing,
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
    updateAgentState,
    api: context.timingApi,
    monitors,
    cancellations: () => cancellations,
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
    mediaStreamTrack: {
      kind: "audio",
      readyState: "live",
    },
  };
}

test("replacing a remote track stops the old monitor", async () => {
  const f = await fixture();

  const trackA = remoteTrack("A");
  const trackB = remoteTrack("B");

  f.api.attachAudioTiming(trackA);

  assert.equal(f.monitors.length, 1);
  assert.equal(f.monitors[0].stopped, false);

  f.api.attachAudioTiming(trackB);

  assert.equal(f.monitors.length, 2);
  assert.equal(f.monitors[0].stopped, true);
  assert.equal(f.monitors[1].stopped, false);

  assert.equal(f.api.currentTrack(), trackB);

  f.api.releaseAudioTiming();
});

test("late unsubscribe from an old track preserves the new monitor", async () => {
  const f = await fixture();

  const trackA = remoteTrack("A");
  const trackB = remoteTrack("B");

  f.api.attachAudioTiming(trackA);
  f.api.attachAudioTiming(trackB);

  // The old unsubscribe arrives after the replacement is active.
  f.api.releaseAudioTiming(trackA);

  assert.equal(f.monitors.length, 2);
  assert.equal(f.monitors[1].stopped, false);
  assert.equal(f.api.currentTrack(), trackB);

  // Releasing the current track must still work.
  f.api.releaseAudioTiming(trackB);

  assert.equal(f.monitors[1].stopped, true);
  assert.equal(f.api.currentTrack(), null);
});

for (const readyState of ["live", "ended"]) {
  test(`registered reconnect handlers respect a retained ${readyState} track`, async () => {
    const f = await fixture();
    const track = remoteTrack("A");
    f.api.setRemoteTrack(track);
    const old = f.monitors[0];
    old.options.onAvailable();
    f.updateAgentState();
    old.options.onQuiet();
    track.mediaStreamTrack.readyState = readyState;
    const cancellations = f.cancellations();
    f.room.emit("Reconnecting");
    assert.equal(f.state.connected, false);
    assert.equal(old.stopped, true);
    assert.equal(f.api.ready(), false);
    assert.equal(f.api.owner(), null);
    assert.equal(f.cancellations(), cancellations + 1);
    f.room.emit("Reconnected");
    assert.equal(f.state.connected, true);
    assert.equal(f.monitors.length, readyState === "live" ? 2 : 1);
    assert.equal(f.api.currentTrack(), readyState === "live" ? track : null);
    assert.equal(f.api.owner(), readyState === "live" ? f.room : null);
    old.options.onAvailable();
    old.options.onQuiet();
    f.advance(200);
    old.onActivity();
    if (readyState === "live") {
      const current = f.monitors[1];
      current.options.onAvailable();
      current.options.onQuiet();
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
    assert.equal(f.api.currentTrack(), null);
    assert.equal(f.api.owner(), null);
    assert.equal(f.api.ready(), false);
    assert.ok(f.monitors.every((monitor) => monitor.stopped));
    assert.deepEqual(f.playbackReleases, [f.room]);
  });
}

test("disabled audio timing creates no monitor", async () => {
  const f = await fixture({ enabled: false });

  const track = remoteTrack("A");

  f.api.attachAudioTiming(track);

  assert.equal(f.monitors.length, 0);
  assert.equal(f.api.currentTrack(), null);

  f.api.releaseAudioTiming();

  assert.equal(f.monitors.length, 0);
});

test("report disconnect releases the owning monitor after state.room is cleared", async () => {
  const f = await fixture();
  f.api.attachAudioTiming(remoteTrack("A"));
  f.monitors[0].options.onAvailable();
  f.timing.speakingChanged(true);
  f.state.room = null;
  f.disconnect();
  assert.equal(f.monitors[0].stopped, true);
  assert.equal(f.api.currentTrack(), null);
  assert.equal(f.api.ready(), false);
  assert.deepEqual(f.diagnostics, ["room disconnected"]);
});

for (const cleared of [false, true]) {
  test(`a stale disconnect preserves the newer room's monitor (cleared=${cleared})`, async () => {
    const f = await fixture();
    f.api.attachAudioTiming(remoteTrack("A"));
    f.state.room = {};
    const newerTrack = remoteTrack("B");
    f.api.attachAudioTiming(newerTrack);
    f.monitors[1].options.onAvailable();
    if (cleared) f.state.room = null;
    f.disconnect();
    assert.equal(f.monitors[1].stopped, false);
    assert.equal(f.api.currentTrack(), newerTrack);
    assert.equal(f.api.ready(), true);
  });
}

test("speaking updates explain an unready monitor once and never time its recovery", async () => {
  const f = await fixture();
  f.api.attachAudioTiming(remoteTrack("A"));
  f.updateAgentState();
  f.updateAgentState();
  assert.deepEqual(f.diagnostics, ["monitor not ready"]);
  f.monitors[0].options.onAvailable();
  f.advance(200);
  f.monitors[0].onActivity();
  assert.deepEqual(f.measurements, []);
});

test("an unavailable monitor cancels a pending measurement with its reason", async () => {
  const f = await fixture();
  f.api.attachAudioTiming(remoteTrack("A"));
  const monitor = f.monitors[0];
  monitor.options.onAvailable();
  f.updateAgentState();
  monitor.options.onQuiet();
  const cancellations = f.cancellations();
  monitor.options.onUnavailable("audio context suspended");
  assert.equal(f.cancellations(), cancellations + 1);
  monitor.options.onUnavailable("audio context suspended");
  assert.equal(f.api.ready(), false);
  assert.deepEqual(f.diagnostics, ["audio context suspended"]);
  monitor.options.onAvailable();
  f.advance(200);
  monitor.onActivity();
  assert.deepEqual(f.measurements, []);
});

test("quiet frames reach the timer through the attached monitor", async () => {
  const f = await fixture();
  f.api.attachAudioTiming(remoteTrack("A"));
  const monitor = f.monitors[0];
  monitor.options.onAvailable();
  f.updateAgentState();
  assert.equal(typeof monitor.options.onQuiet, "function");
  monitor.options.onQuiet();
  f.advance(80);
  monitor.onActivity();
  assert.deepEqual(f.measurements, [80]);
});

test("console diagnostics are installed only with audioTiming=1", () => {
  const source = read("web/interview.js");
  const setup = source.slice(
    source.indexOf("const params = new URLSearchParams"),
    source.indexOf("let audioTimingTrack"),
  );
  for (const search of ["", "?audioTiming=0", "?audioTiming=1"]) {
    const logs = [];
    let created = 0;
    const context = {
      URLSearchParams,
      window: { location: { search } },
      console: { debug: (message) => logs.push(message) },
      createAudioResponseTiming(options) {
        created++;
        return createAudioResponseTiming(options);
      },
    };
    runInNewContext(
      setup + '; audioTiming?.speakingChanged(true, "monitor not ready");',
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
  f.api.setRemoteTrack(remoteTrack("A"));
  const newerRoom = new f.room.constructor();
  f.state.room = newerRoom;
  f.api.attachAudioTiming(remoteTrack("B"));
  f.monitors[1].options.onAvailable();
  const cancellations = f.cancellations();
  f.room.emit("Reconnecting");
  f.room.emit("Reconnected");
  f.room.emit("Disconnected");
  assert.equal(f.state.room, newerRoom);
  assert.equal(f.state.connected, true);
  assert.equal(f.api.owner(), newerRoom);
  assert.equal(f.api.ready(), true);
  assert.equal(f.monitors.length, 2);
  assert.equal(f.monitors[1].stopped, false);
  assert.equal(f.cancellations(), cancellations);
});

test("monitor release clears readiness and late callbacks cannot complete a pending turn", async () => {
  const f = await fixture();
  f.api.attachAudioTiming(remoteTrack("A"));
  const monitor = f.monitors[0];
  monitor.options.onAvailable();
  f.updateAgentState();
  monitor.options.onQuiet();
  const cancellations = f.cancellations();
  f.api.releaseAudioTiming();
  assert.equal(f.cancellations(), cancellations + 1);
  assert.equal(f.api.ready(), false);
  assert.equal(f.api.owner(), null);
  assert.equal(monitor.stopped, true);
  monitor.options.onAvailable();
  monitor.options.onQuiet();
  f.advance(200);
  monitor.onActivity();
  assert.equal(f.api.ready(), false);
  assert.deepEqual(f.measurements, []);
  assert.deepEqual(f.diagnostics, ["audio monitor released"]);
});

test("callbacks from a replaced track cannot cancel or measure the current track", async () => {
  const f = await fixture();
  f.api.attachAudioTiming(remoteTrack("A"));
  const old = f.monitors[0];
  f.api.attachAudioTiming(remoteTrack("B"));
  const current = f.monitors[1];
  current.options.onAvailable();
  f.updateAgentState();
  current.options.onQuiet();
  const cancellations = f.cancellations();
  old.options.onUnavailable("track ended");
  old.options.onAvailable();
  old.options.onQuiet();
  f.advance(80);
  old.onActivity();
  assert.equal(f.cancellations(), cancellations);
  assert.equal(f.api.ready(), true);
  assert.equal(f.api.owner(), f.room);
  assert.deepEqual(f.measurements, []);
  current.onActivity();
  assert.deepEqual(f.measurements, [80]);
});

test("monitor setup failure clears the track and reconnect retries the diagnostic", async () => {
  const f = await fixture({ failFirstMonitor: true });
  const track = remoteTrack("A");
  f.agent.attributes["lk.agent.state"] = "listening";
  f.room.emit("TrackSubscribed", track, {}, f.agent);
  assert.equal(f.monitorAttempts(), 1);
  assert.equal(f.monitors.length, 0);
  assert.equal(f.api.currentTrack(), null);
  assert.equal(f.api.owner(), null);
  assert.equal(f.api.ready(), false);

  f.room.emit("Reconnected");
  assert.equal(f.monitorAttempts(), 2);
  assert.equal(f.monitors.length, 1);
  assert.equal(f.api.currentTrack(), track);
  assert.equal(f.api.owner(), f.room);
  const monitor = f.monitors[0];
  monitor.options.onAvailable();
  assert.equal(f.api.ready(), true);
  f.agent.attributes["lk.agent.state"] = "speaking";
  f.room.emit("ParticipantAttributesChanged");
  monitor.options.onQuiet();
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
  monitor.options.onAvailable();
  f.agent.attributes["lk.agent.state"] = "speaking";
  f.room.emit("ParticipantAttributesChanged");
  monitor.options.onQuiet();
  f.advance(40);

  f.setInterviewerPresent(false);
  f.room.emit("ParticipantDisconnected", f.agent);
  await Promise.resolve();
  assert.equal(f.state.interviewerPresent, false);
  assert.deepEqual(f.diagnostics, ["interviewer unavailable"]);
  f.advance(80);
  monitor.onActivity();
  assert.deepEqual(f.measurements, []);

  monitor.options.onQuiet();
  f.setInterviewerPresent(true);
  f.room.emit("ParticipantConnected", f.agent);
  assert.equal(f.state.interviewerPresent, true);
  monitor.options.onQuiet();
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
  current.options.onAvailable();
  f.agent.attributes["lk.agent.state"] = "speaking";
  f.room.emit("ParticipantAttributesChanged");
  current.options.onQuiet();
  const cancellations = f.cancellations();

  f.room.emit("TrackUnsubscribed", oldTrack);
  assert.equal(f.monitors[0].stopped, true);
  assert.equal(current.stopped, false);
  assert.equal(f.api.currentTrack(), currentTrack);
  assert.equal(f.api.owner(), f.room);
  assert.equal(f.api.ready(), true);
  assert.equal(f.cancellations(), cancellations);
  f.room.emit("TrackUnsubscribed", currentTrack);
  assert.equal(current.stopped, true);
  assert.equal(f.api.currentTrack(), null);
  assert.equal(f.api.owner(), null);
  assert.equal(f.api.ready(), false);
  assert.deepEqual(f.forgotten, [element]);
  assert.deepEqual(removed, [element]);
  current.options.onAvailable();
  current.options.onQuiet();
  f.advance(80);
  current.onActivity();
  assert.equal(f.api.ready(), false);
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
  monitor.options.onAvailable();
  assert.equal(f.api.ready(), true);
  f.agent.attributes["lk.agent.state"] = "speaking";
  f.room.emit("ParticipantAttributesChanged");
  assert.deepEqual(f.diagnostics, ["room disconnected"]);
  monitor.options.onQuiet();
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
    monitor.options.onAvailable();
    f.agent.attributes["lk.agent.state"] = "speaking";
    f.room.emit("ParticipantAttributesChanged");
    monitor.options.onQuiet();
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
    assert.equal(f.api.currentTrack(), null);
    assert.equal(f.api.owner(), null);
    assert.equal(f.api.ready(), false);
    assert.equal(f.state.room, null);
    assert.equal(f.state.connected, false);
    monitor.options.onAvailable();
    monitor.options.onQuiet();
    f.advance(80);
    monitor.onActivity();
    assert.equal(f.api.ready(), false);
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
  f.monitors[0].options.onAvailable();
  f.room.emit("Reconnecting");
  const track = remoteTrack("B");
  f.room.emit("TrackSubscribed", track, {}, f.agent);
  const current = f.monitors[1];
  current.options.onAvailable();
  current.options.onQuiet();
  const cancellations = f.cancellations();
  f.room.emit("Reconnected");
  assert.equal(f.monitors.length, 2);
  assert.equal(f.monitorAttempts(), 2);
  assert.equal(current.stopped, false);
  assert.equal(f.api.currentTrack(), track);
  assert.equal(f.api.owner(), f.room);
  assert.equal(f.api.ready(), true);
  assert.equal(f.cancellations(), cancellations);
  f.agent.attributes["lk.agent.state"] = "speaking";
  f.room.emit("ParticipantAttributesChanged");
  current.options.onQuiet();
  f.advance(80);
  current.onActivity();
  assert.deepEqual(f.measurements, [80]);
  f.disconnect();
});
