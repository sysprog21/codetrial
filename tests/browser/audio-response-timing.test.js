import { test } from "node:test";
import assert from "node:assert/strict";
import {
  createAudioResponseTiming,
  createAudioTimingController,
  startAudioActivityMonitor,
} from "../../web/audio-response-timing.js";

function fixture() {
  let time = 0;
  const measurements = [];
  const diagnostics = [];

  const timing = createAudioResponseTiming({
    now: () => time,
    report: (elapsed) => measurements.push(elapsed),
    diagnose: (reason) => diagnostics.push(reason),
  });

  return {
    timing,
    measurements,
    diagnostics,
    advance: (ms) => {
      time += ms;
    },
  };
}

test("measures speaking-to-first-audio gap", () => {
  const f = fixture();

  f.timing.speakingChanged(true);
  f.timing.audioQuiet();
  f.advance(180);
  f.timing.audioActivity();

  assert.deepEqual(f.measurements, [180]);
});

test("ignores repeated speaking updates", () => {
  const f = fixture();

  f.timing.speakingChanged(true);
  f.timing.audioQuiet();
  f.advance(100);

  f.timing.speakingChanged(true);
  f.advance(80);
  f.timing.audioActivity();

  assert.deepEqual(f.measurements, [180]);
});

test("reports only the first audio activity per speaking turn", () => {
  const f = fixture();

  f.timing.speakingChanged(true);
  f.timing.audioQuiet();
  f.advance(120);
  f.timing.audioActivity();

  f.advance(50);
  f.timing.audioActivity();

  assert.deepEqual(f.measurements, [120]);
});

test("does not measure audio without speaking", () => {
  const f = fixture();

  f.timing.audioActivity();

  assert.deepEqual(f.measurements, []);
});

test("cancels pending measurement when speaking stops", () => {
  const f = fixture();

  f.timing.speakingChanged(true);
  f.advance(100);
  f.timing.speakingChanged(false);

  f.advance(50);
  f.timing.audioActivity();

  assert.deepEqual(f.measurements, []);
});

test("reset cancels an incomplete measurement", () => {
  const f = fixture();

  f.timing.speakingChanged(true);
  f.advance(100);
  f.timing.reset();

  f.advance(50);
  f.timing.audioActivity();

  assert.deepEqual(f.measurements, []);
});

test("audio monitor detects a frame above threshold", () => {
  const m = monitorFixture();
  const callbacks = [];
  const stop = startAudioActivityMonitor(
    m.track,
    () => callbacks.push("audio"),
    m.options,
  );
  assert.deepEqual(
    callbacks,
    [],
    "remote timing waits for the first animation frame",
  );
  m.tick();
  assert.deepEqual(callbacks, ["audio"]);
  assert.deepEqual(m.sampleLengths, [1024]);
  stop();
});
test("audio monitor ignores an ended track", () => {
  const m = monitorFixture({ readyState: "ended" });
  const callbacks = [];
  const stop = startAudioActivityMonitor(
    m.track,
    () => callbacks.push("audio"),
    m.options,
  );
  m.tick();
  assert.deepEqual(callbacks, []);
  assert.deepEqual(m.sampleLengths, []);
  stop();
});

test("audio monitor reports an unavailable audio context", () => {
  const m = monitorFixture({ state: "suspended" });
  let unavailable = 0;
  let activity = 0;
  const stop = startAudioActivityMonitor(m.track, () => activity++, {
    ...m.options,
    onUnavailable: () => unavailable++,
  });
  m.tick();
  assert.equal(unavailable, 1);
  assert.equal(activity, 0);
  assert.deepEqual(m.sampleLengths, []);
  stop();
});
test("cancelPending preserves speaking state without reporting stale audio", () => {
  const f = fixture();

  f.timing.speakingChanged(true);
  f.advance(100);

  f.timing.cancelPending();
  f.advance(50);
  f.timing.audioActivity();

  assert.deepEqual(f.measurements, []);

  // A repeated speaking update must not restart the cancelled measurement.
  f.timing.speakingChanged(true);
  f.advance(50);
  f.timing.audioActivity();

  assert.deepEqual(f.measurements, []);

  // Only a new speaking transition can start another measurement.
  f.timing.speakingChanged(false);
  f.timing.audioQuiet();
  f.timing.speakingChanged(true);
  f.timing.audioQuiet();

  f.advance(80);
  f.timing.audioActivity();

  assert.deepEqual(f.measurements, [80]);
});
test("audio monitor notifies unavailability only on state changes", () => {
  const m = monitorFixture({ state: "suspended", peak: 0 });
  let unavailable = 0;
  const stop = startAudioActivityMonitor(m.track, () => {}, {
    ...m.options,
    onUnavailable: () => unavailable++,
  });
  m.tick();
  m.tick();
  assert.equal(unavailable, 1);
  m.context.state = "running";
  m.tick();
  m.context.state = "suspended";
  m.tick();
  assert.equal(unavailable, 2);
  stop();
});
test("audio monitor attempts to resume a suspended context", () => {
  const m = monitorFixture({ state: "suspended" });
  const stop = startAudioActivityMonitor(m.track, () => {}, m.options);
  assert.equal(m.resumes(), 1);
  stop();
});
test("audio monitor closes its context when setup fails", async () => {
  const m = monitorFixture({ setupError: "Cannot create media source" });
  assert.throws(
    () => startAudioActivityMonitor(m.track, () => {}, m.options),
    /Cannot create media source/,
  );
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(m.closes(), 1);
});
test("audio monitor resumes on a gesture and removes listeners on stop", async () => {
  const m = monitorFixture({
    state: "suspended",
    resumeState: "running",
    peak: 0,
  });
  const stop = startAudioActivityMonitor(m.track, () => {}, m.options);
  assert.equal(m.resumes(), 1);
  m.context.state = "suspended";
  m.gestures.dispatchEvent(new Event("pointerdown"));
  assert.equal(m.resumes(), 2);
  stop();
  m.context.state = "suspended";
  m.gestures.dispatchEvent(new Event("keydown"));
  assert.equal(m.resumes(), 2);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(m.closes(), 1);
  assert.equal(m.disconnections(), 1);
  assert.equal(m.cancellations(), 1);
});
test("monitor interruption does not restart the same speaking turn", () => {
  const f = fixture();

  // Jim begins speaking while the monitor is available.
  f.timing.speakingChanged(true);
  f.advance(100);

  // The audio monitor becomes unavailable.
  f.timing.cancelPending();

  // It later recovers, but Jim is still in the same speaking turn.
  f.advance(100);
  f.timing.speakingChanged(true);

  // Audio is detected after recovery.
  f.advance(50);
  f.timing.audioActivity();

  // The interrupted turn must not produce a misleading measurement.
  assert.deepEqual(f.measurements, []);

  // A genuine new speaking transition may be measured.
  f.timing.speakingChanged(false);
  f.timing.audioQuiet();
  f.timing.speakingChanged(true);
  f.timing.audioQuiet();

  f.advance(120);
  f.timing.audioActivity();

  assert.deepEqual(f.measurements, [120]);
});

test("hidden pages invalidate audio timing and cleanup visibility listeners", () => {
  const m = monitorFixture();
  let unavailable = 0;
  let available = 0;
  let activity = 0;
  const stop = startAudioActivityMonitor(m.track, () => activity++, {
    ...m.options,
    onAvailable: () => available++,
    onUnavailable: () => unavailable++,
  });
  m.tick();
  assert.equal(available, 1);
  assert.equal(activity, 1);
  m.visibility.hidden = true;
  m.visibility.dispatchEvent(new Event("visibilitychange"));
  assert.equal(unavailable, 1);
  m.tick();
  assert.equal(activity, 1);
  assert.equal(unavailable, 1);
  m.visibility.hidden = false;
  m.visibility.dispatchEvent(new Event("visibilitychange"));
  m.tick();
  assert.equal(available, 2);
  assert.equal(activity, 2);
  stop();
  m.visibility.hidden = true;
  m.visibility.dispatchEvent(new Event("visibilitychange"));
  assert.equal(unavailable, 1);
});

function monitorFixture({
  state = "running",
  readyState = "live",
  peak = 0.5,
  resumeState = state,
  setupError = null,
} = {}) {
  let frame;
  let scheduled = 0;
  let closes = 0;
  let resumes = 0;
  let disconnections = 0;
  let cancellations = 0;
  const sampleLengths = [];
  const streams = [];
  const track = { kind: "audio", readyState, muted: false, enabled: true };
  const visibility = new EventTarget();
  visibility.hidden = false;
  const gestures = new EventTarget();
  const analyser = {
    fftSize: 1024,
    get frequencyBinCount() {
      return this.fftSize / 2;
    },
    getByteTimeDomainData(samples) {
      assert.equal(
        context.state,
        "running",
        "unavailable contexts must not be sampled",
      );
      sampleLengths.push(samples.length);
      samples.fill(128);
      samples[0] = 128 + Math.round(peak * 128);
    },
  };
  const context = {
    state,
    createMediaStreamSource(stream) {
      streams.push(stream);
      if (setupError) throw new Error(setupError);
      return {
        connect(target) {
          assert.equal(target, analyser);
        },
        disconnect() {
          disconnections++;
        },
      };
    },
    createAnalyser: () => analyser,
    resume: async () => {
      resumes++;
      context.state = resumeState;
    },
    close: async () => {
      closes++;
    },
  };
  const options = {
    createContext: () => context,
    createStream: (mediaTrack) => ({ getTracks: () => [mediaTrack] }),
    schedule: (callback) => {
      frame = callback;
      return ++scheduled;
    },
    cancel: () => {
      cancellations++;
    },
    visibilityTarget: visibility,
    gestureTarget: gestures,
  };
  return {
    track,
    context,
    visibility,
    gestures,
    options,
    sampleLengths,
    streams,
    tick: () => frame(),
    setPeak: (value) => {
      peak = value;
    },
    closes: () => closes,
    resumes: () => resumes,
    disconnections: () => disconnections,
    cancellations: () => cancellations,
  };
}

test("audio already active before speaking never becomes a positive gap", () => {
  const f = fixture();
  f.timing.audioActivity();
  f.timing.speakingChanged(true);
  f.advance(16);
  f.timing.audioActivity();
  assert.deepEqual(f.measurements, []);
  assert.deepEqual(f.diagnostics, ["audio already active"]);
});

test("the first active frame cannot substitute for a post-transition quiet baseline", () => {
  const f = fixture();
  f.timing.speakingChanged(true);
  f.advance(16);
  f.timing.audioActivity();
  assert.deepEqual(f.measurements, []);
  assert.deepEqual(f.diagnostics, [
    "audio already active before a quiet frame",
  ]);
});

test("only a quiet frame after speaking arms a positive measurement", () => {
  const f = fixture();
  assert.equal(typeof f.timing.audioQuiet, "function");
  f.timing.audioQuiet();
  f.timing.speakingChanged(true);
  f.advance(16);
  f.timing.audioActivity();
  assert.deepEqual(f.measurements, []);
  f.timing.audioQuiet();
  f.timing.audioActivity();
  assert.deepEqual(f.measurements, [], "a dropped turn stays dropped");
  f.timing.speakingChanged(false);
  f.timing.audioQuiet();
  f.timing.speakingChanged(true);
  f.advance(20);
  f.timing.audioQuiet();
  f.advance(80);
  f.timing.audioActivity();
  assert.deepEqual(
    f.measurements,
    [100],
    "elapsed time still starts at the state transition",
  );
});

for (const [property, value, reason] of [
  ["muted", true, "track muted"],
  ["enabled", false, "track disabled"],
]) {
  test(`a ${property} interruption cancels the turn instead of timing recovery`, () => {
    const f = fixture();
    const m = monitorFixture({ peak: 0 });
    const unavailable = [];
    const stop = startAudioActivityMonitor(m.track, f.timing.audioActivity, {
      ...m.options,
      onQuiet: f.timing.audioQuiet,
      onUnavailable: (reason) => {
        unavailable.push(reason);
        f.timing.cancelPending(reason);
      },
    });
    m.tick();
    f.timing.speakingChanged(true);
    m.tick();
    m.track[property] = value;
    m.tick();
    m.tick();
    assert.deepEqual(unavailable, [reason]);
    assert.deepEqual(f.diagnostics, [reason]);
    m.track[property] = !value;
    m.setPeak(0.5);
    f.advance(200);
    m.tick();
    assert.deepEqual(f.measurements, []);
    m.setPeak(0);
    m.tick();
    f.timing.speakingChanged(false);
    f.timing.speakingChanged(true);
    m.tick();
    f.advance(80);
    m.setPeak(0.5);
    m.tick();
    assert.deepEqual(f.measurements, [80]);
    stop();
  });
}

test("the monitor supplies quiet frames without reporting activity", () => {
  const m = monitorFixture({ peak: 0 });
  let quiet = 0;
  let activity = 0;
  const stop = startAudioActivityMonitor(m.track, () => activity++, {
    ...m.options,
    onQuiet: () => quiet++,
  });
  m.tick();
  m.setPeak(0.5);
  m.tick();
  m.setPeak(0);
  m.tick();
  assert.equal(quiet, 2);
  assert.equal(activity, 1);
  stop();
});

test("skipped transitions and canceled turns explain themselves once", () => {
  const f = fixture();
  f.timing.speakingChanged(true, "monitor not ready");
  f.timing.speakingChanged(true, "monitor not ready");
  f.timing.cancelPending("audio context suspended");
  assert.deepEqual(f.diagnostics, ["monitor not ready"]);
  f.timing.speakingChanged(false);
  f.timing.speakingChanged(true);
  f.timing.cancelPending("page hidden");
  f.timing.cancelPending("page hidden");
  f.timing.audioActivity();
  assert.deepEqual(f.measurements, []);
  assert.deepEqual(f.diagnostics, ["monitor not ready", "page hidden"]);
});

test("stopping speech and resetting explain pending measurements", () => {
  const f = fixture();
  f.timing.speakingChanged(true);
  f.timing.speakingChanged(false);
  f.timing.speakingChanged(true);
  f.timing.reset("room disconnected");
  assert.deepEqual(f.diagnostics, [
    "speaking stopped before audio",
    "room disconnected",
  ]);
});

for (const [name, peak, expected] of [
  ["silent", 0, []],
  ["below threshold", 7 / 128, []],
  ["at threshold", 8 / 128, []],
  ["above threshold", 9 / 128, [80]],
]) {
  test(`${name} frames obey the strict activity threshold`, () => {
    const f = fixture();
    const m = monitorFixture({ peak: 0 });
    const stop = startAudioActivityMonitor(m.track, f.timing.audioActivity, {
      ...m.options,
      threshold: 8 / 128,
      onQuiet: f.timing.audioQuiet,
    });
    m.tick();
    f.timing.speakingChanged(true);
    m.tick();
    m.setPeak(peak);
    f.advance(80);
    m.tick();
    m.tick();
    assert.deepEqual(f.measurements, expected);
    m.setPeak(0);
    m.tick();
    assert.deepEqual(
      f.measurements,
      expected,
      "quiet frames never add a result",
    );
    stop();
  });
}

test("failed monitor setup leaves no gesture or visibility listeners", () => {
  const m = monitorFixture({
    state: "suspended",
    setupError: "Cannot create media source",
  });
  let unavailable = 0;
  assert.throws(
    () =>
      startAudioActivityMonitor(m.track, () => {}, {
        ...m.options,
        onUnavailable: () => unavailable++,
      }),
    /Cannot create media source/,
  );
  const resumes = m.resumes();
  m.gestures.dispatchEvent(new Event("pointerdown"));
  m.gestures.dispatchEvent(new Event("keydown"));
  m.visibility.hidden = true;
  m.visibility.dispatchEvent(new Event("visibilitychange"));
  assert.equal(m.resumes(), resumes);
  assert.equal(unavailable, 0);
  assert.equal(m.closes(), 1);
});

test("a synchronous resume failure releases the remote monitor before throwing", () => {
  const m = monitorFixture({ state: "suspended" });
  m.context.resume = () => {
    throw new Error("Resume failed");
  };
  assert.throws(
    () => startAudioActivityMonitor(m.track, () => {}, m.options),
    /Resume failed/,
  );
  assert.equal(m.closes(), 1);
  assert.equal(m.disconnections(), 1);
  assert.equal(m.cancellations(), 1);
});

test("callbacks from a released monitor cannot affect a reattached track", () => {
  const monitors = [];
  const measurements = [];
  const diagnostics = [];
  let time = 0;
  const controller = createAudioTimingController({
    now: () => time,
    report: (elapsed) => measurements.push(elapsed),
    diagnose: (reason) => diagnostics.push(reason),
    startMonitor(track, activity, options) {
      const monitor = { activity, ...options, stopped: false };
      monitors.push(monitor);
      return () => {
        monitor.stopped = true;
      };
    },
  });
  const room = {};
  const track = { mediaStreamTrack: {} };
  controller.attach(track, room);
  const old = monitors[0];
  controller.release(null, "room reconnecting");
  assert.equal(old.stopped, true);
  controller.attach(track, room);
  controller.attach(track, room);
  assert.equal(monitors.length, 2);
  const current = monitors[1];
  current.onAvailable();
  controller.speakingChanged(true, true);
  current.onQuiet();
  old.onUnavailable("track ended");
  old.onAvailable();
  old.onQuiet();
  time = 80;
  old.activity();
  assert.deepEqual(measurements, []);
  current.activity();
  assert.deepEqual(measurements, [80]);
  assert.deepEqual(diagnostics, []);
});

test("a stale disconnect preserves the newer monitor even after the page clears its room", () => {
  const f = fixture();
  const first = monitorFixture({ peak: 0 });
  const second = monitorFixture({ peak: 0 });
  const controller = createAudioTimingController({
    diagnose: (reason) => f.diagnostics.push(reason),
    startMonitor(track, activity, options) {
      const m = track === first.track ? first : second;
      return startAudioActivityMonitor(track, activity, {
        ...m.options,
        ...options,
      });
    },
  });
  const oldRoom = {};
  const newRoom = {};
  const oldTrack = { mediaStreamTrack: first.track };
  const newTrack = { mediaStreamTrack: second.track };
  controller.attach(oldTrack, oldRoom);
  controller.attach(newTrack, newRoom);
  assert.equal(first.closes(), 1);
  controller.release(oldTrack);
  controller.disconnect(oldRoom, newRoom);
  controller.disconnect(oldRoom, null);
  assert.equal(second.closes(), 0);

  second.tick();
  controller.speakingChanged(true, true);
  second.tick();
  controller.disconnect(newRoom, null);
  assert.equal(second.closes(), 1);
  assert.deepEqual(f.diagnostics, ["room disconnected"]);
});

test("unavailable audio cancels a pending turn and recovery cannot measure it", () => {
  const m = monitorFixture({ peak: 0 });
  const diagnostics = [];
  const measurements = [];
  const controller = createAudioTimingController({
    diagnose: (reason) => diagnostics.push(reason),
    report: (elapsed) => measurements.push(elapsed),
    startMonitor: (track, activity, options) =>
      startAudioActivityMonitor(track, activity, { ...m.options, ...options }),
  });
  controller.attach({ mediaStreamTrack: m.track }, {});
  m.tick();
  controller.speakingChanged(true, true);
  m.tick();
  m.track.muted = true;
  m.tick();
  m.track.muted = false;
  m.tick();
  m.setPeak(0.5);
  m.tick();
  assert.deepEqual(diagnostics, ["track muted"]);
  assert.deepEqual(measurements, []);
  controller.release();
});
