import { test } from "node:test";
import assert from "node:assert/strict";
import {
  createAudioResponseTiming,
  startAudioActivityMonitor,
} from "../../web/audio-response-timing.js";

function fixture() {
  let time = 0;
  const measurements = [];

  const timing = createAudioResponseTiming({
    now: () => time,
    report: (elapsed) => measurements.push(elapsed),
  });

  return {
    timing,
    measurements,
    advance: (ms) => {
      time += ms;
    },
  };
}

test("measures speaking-to-first-audio gap", () => {
  const f = fixture();

  f.timing.speakingChanged(true);
  f.advance(180);
  f.timing.audioActivity();

  assert.deepEqual(f.measurements, [180]);
});

test("ignores repeated speaking updates", () => {
  const f = fixture();

  f.timing.speakingChanged(true);
  f.advance(100);

  f.timing.speakingChanged(true);
  f.advance(80);
  f.timing.audioActivity();

  assert.deepEqual(f.measurements, [180]);
});

test("reports only the first audio activity per speaking turn", () => {
  const f = fixture();

  f.timing.speakingChanged(true);
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
  const callbacks = [];
  let tick;

  const analyser = {
    fftSize: 1024,
    getByteTimeDomainData(samples) {
      samples.fill(128);
      samples[0] = 192;
    },
  };

  const source = {
    connect() {},
    disconnect() {},
  };

  const context = {
    state: "running",

    createMediaStreamSource() {
      return source;
    },
    createAnalyser() {
      return analyser;
    },
    close: async () => {},
  };

  const stop = startAudioActivityMonitor(
    { kind: "audio", readyState: "live" },
    () => callbacks.push("audio"),
    {
      createContext: () => context,
      createStream: (track) => ({ getTracks: () => [track] }),
      schedule: (callback) => {
        tick = callback;
        return 1;
      },
      cancel: () => {},
    },
  );

  tick();

  assert.deepEqual(callbacks, ["audio"]);

  stop();
});
test("audio monitor ignores an ended track", () => {
  const callbacks = [];
  let tick;

  const track = {
    kind: "audio",
    readyState: "ended",
  };

  const analyser = {
    fftSize: 1024,
    getByteTimeDomainData(samples) {
      samples.fill(128);
      samples[0] = 192;
    },
  };

  const source = {
    connect() {},
    disconnect() {},
  };

  const context = {
    state: "running",
    createMediaStreamSource: () => source,
    createAnalyser: () => analyser,
    close: async () => {},
  };

  const stop = startAudioActivityMonitor(track, () => callbacks.push("audio"), {
    createContext: () => context,
    createStream: () => ({}),
    schedule: (callback) => {
      tick = callback;
      return 1;
    },
    cancel: () => {},
  });

  tick();

  assert.deepEqual(callbacks, []);

  stop();
});

test("audio monitor reports an unavailable audio context", () => {
  let tick;
  let unavailable = 0;
  let activity = 0;

  const context = {
    state: "suspended",
    createMediaStreamSource: () => ({
      connect() {},
      disconnect() {},
    }),
    createAnalyser: () => ({
      fftSize: 1024,
      getByteTimeDomainData() {
        throw new Error("Suspended context must not be sampled");
      },
    }),
    close: async () => {},
  };

  const stop = startAudioActivityMonitor(
    { kind: "audio", readyState: "live" },
    () => activity++,
    {
      createContext: () => context,
      createStream: () => ({}),
      onUnavailable: () => unavailable++,
      schedule: (callback) => {
        tick = callback;
        return 1;
      },
      cancel: () => {},
    },
  );

  tick();

  assert.equal(unavailable, 1);
  assert.equal(activity, 0);

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
  f.timing.speakingChanged(true);

  f.advance(80);
  f.timing.audioActivity();

  assert.deepEqual(f.measurements, [80]);
});
test("audio monitor notifies unavailability only on state changes", () => {
  let tick;
  let unavailable = 0;

  const context = {
    state: "suspended",
    createMediaStreamSource: () => ({
      connect() {},
      disconnect() {},
    }),
    createAnalyser: () => ({
      fftSize: 1024,
      getByteTimeDomainData(samples) {
        samples.fill(128);
      },
    }),
    close: async () => {},
  };

  const stop = startAudioActivityMonitor(
    { kind: "audio", readyState: "live" },
    () => {},
    {
      createContext: () => context,
      createStream: () => ({}),
      onUnavailable: () => unavailable++,
      schedule: (callback) => {
        tick = callback;
        return 1;
      },
      cancel: () => {},
    },
  );

  tick();
  tick();

  assert.equal(unavailable, 1);

  context.state = "running";
  tick();

  context.state = "suspended";
  tick();

  assert.equal(unavailable, 2);

  stop();
});
test("audio monitor attempts to resume a suspended context", () => {
  let resumes = 0;

  const context = {
    state: "suspended",

    resume: async () => {
      resumes++;
    },

    createMediaStreamSource: () => ({
      connect() {},
      disconnect() {},
    }),

    createAnalyser: () => ({
      fftSize: 1024,
      getByteTimeDomainData() {},
    }),

    close: async () => {},
  };

  const stop = startAudioActivityMonitor(
    { kind: "audio", readyState: "live" },
    () => {},
    {
      createContext: () => context,
      createStream: () => ({}),
      schedule: () => 1,
      cancel: () => {},
    },
  );

  assert.equal(resumes, 1);

  stop();
});
test("audio monitor closes its context when setup fails", async () => {
  let closes = 0;

  const context = {
    state: "running",

    createMediaStreamSource() {
      throw new Error("Cannot create media source");
    },

    close: async () => {
      closes++;
    },
  };

  assert.throws(
    () =>
      startAudioActivityMonitor(
        { kind: "audio", readyState: "live" },
        () => {},
        {
          createContext: () => context,
          createStream: () => ({}),
          schedule: () => 1,
          cancel: () => {},
        },
      ),
    /Cannot create media source/,
  );

  // close() is asynchronous, so allow its promise to settle.
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(closes, 1);
});
test("audio monitor resumes on a gesture and removes listeners on stop", async () => {
  const gestures = new EventTarget();
  let resumes = 0;
  let closes = 0;

  const context = {
    state: "suspended",

    resume: async () => {
      resumes++;
      context.state = "running";
    },

    createMediaStreamSource: () => ({
      connect() {},
      disconnect() {},
    }),

    createAnalyser: () => ({
      fftSize: 1024,
      getByteTimeDomainData(samples) {
        samples.fill(128);
      },
    }),

    close: async () => {
      closes++;
    },
  };

  const stop = startAudioActivityMonitor(
    { kind: "audio", readyState: "live" },
    () => {},
    {
      createContext: () => context,
      createStream: () => ({}),
      gestureTarget: gestures,
      schedule: () => 1,
      cancel: () => {},
    },
  );

  // The initial resume is attempted during setup.
  assert.equal(resumes, 1);

  // Simulate the browser suspending the context again.
  context.state = "suspended";

  gestures.dispatchEvent(new Event("pointerdown"));

  assert.equal(resumes, 2);

  stop();

  // Stopped monitors must no longer react to gestures.
  context.state = "suspended";
  gestures.dispatchEvent(new Event("keydown"));

  assert.equal(resumes, 2);

  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(closes, 1);
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
  f.timing.speakingChanged(true);

  f.advance(120);
  f.timing.audioActivity();

  assert.deepEqual(f.measurements, [120]);
});

test("hidden pages invalidate audio timing and cleanup visibility listeners", () => {
  const visibilityTarget = new EventTarget();
  visibilityTarget.hidden = false;

  let tick;
  let unavailable = 0;
  let available = 0;
  let activity = 0;

  const context = {
    state: "running",
    createMediaStreamSource: () => ({
      connect() {},
      disconnect() {},
    }),
    createAnalyser: () => ({
      fftSize: 1024,
      getByteTimeDomainData(samples) {
        samples.fill(128);
        samples[0] = 192;
      },
    }),
    close: async () => {},
  };

  const stop = startAudioActivityMonitor(
    { kind: "audio", readyState: "live" },
    () => activity++,
    {
      createContext: () => context,
      createStream: () => ({}),
      visibilityTarget,
      onAvailable: () => available++,
      onUnavailable: () => unavailable++,
      schedule: (callback) => {
        tick = callback;
        return 1;
      },
      cancel: () => {},
    },
  );

  tick();

  assert.equal(available, 1);
  assert.equal(activity, 1);

  visibilityTarget.hidden = true;
  visibilityTarget.dispatchEvent(new Event("visibilitychange"));

  assert.equal(unavailable, 1);

  // Even if the frame loop runs, hidden audio must not be counted.
  tick();

  assert.equal(activity, 1);
  assert.equal(unavailable, 1);

  visibilityTarget.hidden = false;
  visibilityTarget.dispatchEvent(new Event("visibilitychange"));
  tick();

  assert.equal(available, 2);
  assert.equal(activity, 2);

  stop();

  // Stopped monitors must not react to later visibility changes.
  visibilityTarget.hidden = true;
  visibilityTarget.dispatchEvent(new Event("visibilitychange"));

  assert.equal(unavailable, 1);
});
