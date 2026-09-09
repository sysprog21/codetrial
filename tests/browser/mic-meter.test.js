// Run with: node --test tests/browser/mic-meter.test.js
//
// The media gate has no bypass, so what it reads has to be about the devices
// that are still there. These drive `createMicMeter` and `preflightReadiness`
// against a fake meter and a fake pool, so what is asserted is behaviour --
// which peaks are latched, which are forgotten, and whether the gate opens --
// rather than the text of the functions.

import { test } from "node:test";
import assert from "node:assert/strict";

import { MIC_CONFIRM_FRAMES, preflightReadiness } from "../../web/audio-check.js";
import { createMicMeter } from "../../web/mic-meter.js";

/// A pool with one audio track, whose readiness a test can change.
function fakePool(audioState = "live") {
  const errors = {};
  return {
    stream: {},
    trackOf: (kind) => (kind === "audio" ? { readyState: audioState } : null),
    setError(kind, error) {
      errors[kind] = error;
    },
    errorOf: (kind) => errors[kind] ?? null,
    dropped: [],
    dropTrack(track) {
      this.dropped.push(track);
    },
    retry() {
      this.retried = true;
    },
  };
}

/// A meter that hands its callbacks back instead of opening an AudioContext.
function fakeMeter() {
  const started = [];
  const start = (stream, onPeak, onError, shouldStop) => {
    started.push({ onPeak, onError, shouldStop });
  };
  return { start, started, latest: () => started.at(-1) };
}

const loudEnough = (meter, level = 0.5) => {
  for (let i = 0; i < MIC_CONFIRM_FRAMES; i += 1) meter.latest().onPeak(level);
};

test("a proven peak is latched, so a candidate need not keep talking", () => {
  const started = fakeMeter();
  const meter = createMicMeter({
    pool: fakePool(),
    isFinished: () => false,
    onLevel: () => {},
    onFailure: () => {},
    startMeter: started.start,
  });

  meter.start();
  loudEnough(started, 0.5);
  assert.ok(meter.peak() > 0, "six loud frames prove a microphone");

  for (let i = 0; i < MIC_CONFIRM_FRAMES; i += 1) started.latest().onPeak(0);
  assert.ok(meter.peak() > 0, "going quiet must not un-prove it");
});

test("forgetting a microphone drops what it proved and silences its meter", () => {
  const started = fakeMeter();
  const meter = createMicMeter({
    pool: fakePool(),
    isFinished: () => false,
    onLevel: () => {},
    onFailure: () => {},
    startMeter: started.start,
  });

  meter.start();
  loudEnough(started);
  const abandoned = started.latest();
  assert.ok(meter.peak() > 0);

  meter.forget();

  assert.equal(meter.peak(), 0, "nothing a departed microphone proved carries over");
  // The generation bump, observed rather than counted: the meter still running
  // over the old track must not report another level.
  assert.equal(abandoned.shouldStop(), true, "the meter over the old track must stop");
});

test("a replacement microphone is proven on its own frames", () => {
  const started = fakeMeter();
  const meter = createMicMeter({
    pool: fakePool(),
    isFinished: () => false,
    onLevel: () => {},
    onFailure: () => {},
    startMeter: started.start,
  });

  meter.start();
  loudEnough(started);
  meter.forget();
  meter.start();

  assert.equal(meter.peak(), 0, "the new device has proven nothing yet");
  loudEnough(started);
  assert.ok(meter.peak() > 0, "and proves itself on its own frames");
  assert.equal(started.latest().shouldStop(), false, "the current meter keeps running");
});

/// Timers a test can run by hand.
///
/// The retry is the whole point of these two, and a real `setTimeout` would
/// make them wait to find out whether one was even scheduled.
function fakeTimers(t) {
  const pending = [];
  const savedSet = globalThis.setTimeout;
  const savedClear = globalThis.clearTimeout;
  globalThis.setTimeout = (fn) => pending.push(fn);
  globalThis.clearTimeout = (handle) => {
    if (handle) pending[handle - 1] = null;
  };
  t.after(() => {
    globalThis.setTimeout = savedSet;
    globalThis.clearTimeout = savedClear;
  });
  return {
    scheduled: () => pending.filter(Boolean).length,
    run: () => {
      for (const fn of pending) if (fn) fn();
    },
  };
}

test("a meter that fails forgets the peak and asks again", (t) => {
  const timers = fakeTimers(t);
  const started = fakeMeter();
  const failures = [];
  const meter = createMicMeter({
    pool: fakePool(),
    isFinished: () => false,
    onLevel: () => {},
    onFailure: (error) => failures.push(error),
    startMeter: started.start,
  });

  meter.start();
  loudEnough(started);
  started.latest().onError("device lost");

  assert.equal(meter.peak(), 0, "a device that went away has proven nothing about its replacement");
  assert.deepEqual(failures, ["device lost"]);
  assert.equal(timers.scheduled(), 1, "the gate has no bypass, so the meter must ask again");
  timers.run();
  assert.equal(started.started.length, 2, "and asking again starts a meter");
});

/// The preflight's own `onFailure` repaints, which drops the ended track,
/// which forgets this meter -- all before the error callback returns. A retry
/// armed after that runs over a stream whose track the pool has already let go.
test("a meter forgotten while it was failing does not restart itself", (t) => {
  const timers = fakeTimers(t);
  const started = fakeMeter();
  let meter;
  meter = createMicMeter({
    pool: fakePool(),
    isFinished: () => false,
    onLevel: () => {},
    onFailure: () => meter.forget(),
    startMeter: started.start,
  });

  meter.start();
  started.latest().onError("device lost");
  timers.run();

  assert.equal(started.started.length, 1, "a forgotten meter must not restart itself");
});

/// The bug this exists for: a candidate proves their microphone, unplugs it,
/// and clicks Start. The drop is what invalidates the peak, so a sample that
/// read the peak first would open the gate on a device that is gone.
test("an ended microphone closes the gate even when a peak was proven", () => {
  const pool = fakePool("ended");
  const started = fakeMeter();
  const meter = createMicMeter({
    pool,
    isFinished: () => false,
    onLevel: () => {},
    onFailure: () => {},
    startMeter: started.start,
  });
  meter.start();
  loudEnough(started);
  assert.ok(meter.peak() > 0, "the microphone was proven before it went away");

  // `dropTrack` is what fires `onLost` in the real pool, which is what calls
  // `forget`. Wiring that here is the whole point: the sample has to drop the
  // dead track before it reads the peak, or it reads a peak the drop clears.
  pool.dropTrack = (track) => {
    pool.dropped.push(track);
    meter.forget();
  };

  const state = preflightReadiness({
    pool,
    browserSupported: true,
    outputConfirmed: true,
    micPeak: meter.peak,
    faceCheck: { ready: true, error: null },
  });

  assert.equal(state.steps.mic, false, "an unplugged microphone is not a proven one");
  assert.equal(state.ready, false, "and the gate does not open on it");
  assert.ok(pool.retried, "the pool is asked for a replacement");
});

/// Both kinds, one rule. An ended track never revives, and while it sits in
/// the stream the pool's retry sees a device of that kind and asks for
/// nothing, so the gate -- which has no bypass -- would stay shut for the rest
/// of the preflight.
test("a dead device of either kind reopens the request the pool makes", () => {
  for (const dead of ["audio", "video"]) {
    const pool = fakePool();
    pool.trackOf = (kind) => ({ readyState: kind === dead ? "ended" : "live" });
    preflightReadiness({
      pool,
      browserSupported: true,
      outputConfirmed: true,
      micPeak: () => 1,
      faceCheck: { ready: true, error: null },
    });
    assert.equal(pool.dropped.length, 1, `an ended ${dead} track must be dropped`);
    assert.ok(pool.retried, `and the pool asked for a replacement ${dead} device`);
  }
});
