import test from "node:test";
import assert from "node:assert/strict";
import {
  calibrate,
  createLookAwayMonitor,
  downMetric,
} from "../../web/look-away.js";

// A face whose nose sits `ratio` of the way from the eyes to the mouth.
function face(ratio, yCenter = 0.5) {
  return {
    box: [0.5, yCenter, 0.2, 0.3],
    landmarks: [
      [0.45, 0.4],
      [0.55, 0.4],
      [0.5, 0.4 + 0.1 * ratio],
      [0.5, 0.5],
    ],
  };
}
const sample = (ratio) => ({ available: true, count: 1, face: face(ratio) });
const many = (n, ratio) => Array.from({ length: n }, () => sample(ratio));

test("the head-down measure reads keypoints, or the box without them", () => {
  assert.deepEqual(downMetric(face(0.5)), { kind: "keypoints", value: 0.5 });
  assert.deepEqual(downMetric({ box: [0.5, 0.62, 0.2, 0.3], landmarks: [] }), {
    kind: "box",
    value: 0.62,
  });
  assert.equal(downMetric(null), null);
});

test("calibration sets the limit beyond both reading and keyboard glances", () => {
  const result = calibrate(many(20, 0.5), [...many(15, 0.5), ...many(5, 0.7)]);
  assert.equal(result.ok, true);
  assert.equal(result.metric, "keypoints");
  assert.equal(result.baseline, 0.5);
  assert.equal(result.keyboard, 0.7);
  assert.ok(
    result.limit > 0.7,
    `limit ${result.limit} must clear the keyboard dip`,
  );
});

test("calibration fails without a steady single face or enough samples", () => {
  const absent = Array.from({ length: 20 }, (_, i) =>
    i % 2 ? sample(0.5) : { available: true, count: 0, face: null },
  );
  assert.deepEqual(calibrate(absent, many(20, 0.5)), {
    ok: false,
    reason: "face_not_steady",
  });
  assert.deepEqual(calibrate(many(5, 0.5), many(20, 0.5)), {
    ok: false,
    reason: "too_few_samples",
  });
});

// Samples arrive about every second, as the page takes them; a gap of
// several seconds would itself be an interruption.
function feed(monitor, from, to, value) {
  let last;
  for (let now = from; now <= to; now += 1000)
    last = monitor.update(value, now);
  return last;
}

test("a keyboard glance passes, sustained looking down warns and then ends", () => {
  const monitor = createLookAwayMonitor({
    limit: 0.8,
    metric: "keypoints",
    thresholdMs: 8000,
  });
  assert.equal(feed(monitor, 0, 2000, sample(0.5)).state, "ok");
  // A dip deeper than reading but within the calibrated keyboard range.
  assert.equal(feed(monitor, 3000, 20000, sample(0.75)).state, "ok");
  assert.equal(feed(monitor, 21000, 23000, sample(0.95)).state, "away");
  assert.equal(feed(monitor, 24000, 25000, sample(0.95)).state, "warning");
  assert.equal(monitor.update(sample(0.5), 26000).state, "ok");
  assert.equal(monitor.update(sample(0.95), 27000).state, "away");
  assert.deepEqual(feed(monitor, 28000, 35000, sample(0.95)), {
    state: "violation",
    awayMs: 8000,
  });
});

test("leaving the frame counts as looking away", () => {
  const monitor = createLookAwayMonitor({
    limit: 0.8,
    metric: "keypoints",
    thresholdMs: 8000,
  });
  const gone = { available: true, count: 0, face: null };
  monitor.update(sample(0.5), 0);
  assert.equal(monitor.update(gone, 1000).state, "away");
  assert.deepEqual(feed(monitor, 2000, 9000, gone), {
    state: "violation",
    awayMs: 8000,
  });
});

test("a detector that stops answering interrupts rather than accuses", () => {
  const monitor = createLookAwayMonitor({
    limit: 0.8,
    metric: "keypoints",
    thresholdMs: 8000,
  });
  const down = { available: false, count: 0, confidence: 0 };
  monitor.update(sample(0.5), 0);
  assert.equal(monitor.update(down, 2000).state, "ok");
  assert.equal(monitor.update(down, 5000).state, "interrupted");
  // A successful sample after an unwatched gap is an interruption too.
  const later = createLookAwayMonitor({
    limit: 0.8,
    metric: "keypoints",
    thresholdMs: 8000,
  });
  later.update(sample(0.5), 0);
  assert.equal(later.update(sample(0.5), 6000).state, "interrupted");
});

test("a detector that never answers after Start is unwatched time too", () => {
  const monitor = createLookAwayMonitor({
    limit: 0.8,
    metric: "keypoints",
    thresholdMs: 8000,
  });
  const down = { available: false, count: 0, confidence: 0 };
  assert.equal(monitor.update(down, 0).state, "ok");
  assert.equal(monitor.update(down, 4000).state, "ok");
  assert.equal(monitor.update(down, 5000).state, "interrupted");
});

test("time the detector missed is not counted as looking away", () => {
  const monitor = createLookAwayMonitor({
    limit: 0.75,
    metric: "keypoints",
    thresholdMs: 5000,
  });
  const down = sample(0.95);
  const missed = { available: false, count: 0, confidence: 0 };
  assert.equal(monitor.update(down, 0).state, "away");
  for (let at = 500; at < 5000; at += 500)
    assert.equal(monitor.update(missed, at).awayMs, 0);
  // One more look down after the gap: barely any of it was observed.
  const after = monitor.update(down, 4900);
  assert.notEqual(after.state, "violation");
  assert.ok(after.awayMs < 1000, `${after.awayMs}`);
});

test("calibration needs the typing phase measured, not only the reading one", () => {
  const reading = Array.from({ length: 20 }, () => sample(0.5));
  const unseen = Array.from({ length: 20 }, () => ({
    available: false,
    count: 0,
    confidence: 0,
  }));
  assert.deepEqual(calibrate(reading, unseen), {
    ok: false,
    reason: "face_not_steady",
  });
});

test("a look-away resumed after a long gap counts only what was seen", () => {
  const monitor = createLookAwayMonitor({
    limit: 0.75,
    metric: "keypoints",
    thresholdMs: 8000,
  });
  const down = sample(0.95);
  for (let at = 0; at <= 4500; at += 500) monitor.update(down, at);
  monitor.update({ available: false, count: 0, confidence: 0 }, 5000);
  // Back after four unobserved seconds: 4.5 s seen before, none since.
  const back = monitor.update(down, 9000);
  assert.equal(back.state, "warning");
  assert.ok(back.awayMs <= 5000, `${back.awayMs}`);
});

test("a face measured another way neither clears nor extends a look-away", () => {
  const monitor = createLookAwayMonitor({
    limit: 0.75,
    metric: "keypoints",
    thresholdMs: 8000,
  });
  const down = sample(0.95);
  // The same face with no keypoints: only the box is measured.
  const boxOnly = {
    available: true,
    count: 1,
    face: { box: [0.5, 0.5, 0.2, 0.3] },
  };
  monitor.update(down, 0);
  monitor.update(down, 1000);
  const mismatched = monitor.update(boxOnly, 1500);
  assert.notEqual(mismatched.state, "ok", "a box sample cleared the look-away");
  assert.ok(mismatched.awayMs <= 1000, `${mismatched.awayMs}`);
});

test("calibration rests on samples measured one way, most of the phase", () => {
  const box = {
    available: true,
    count: 1,
    face: { box: [0.5, 0.5, 0.2, 0.3] },
  };
  // Two keypoint samples among eighteen box ones: the baseline is the box's,
  // and it is measured over enough of the phase.
  const mixed = [
    sample(0.5),
    sample(0.5),
    ...Array.from({ length: 18 }, () => box),
  ];
  const typing = Array.from({ length: 20 }, () => box);
  const result = calibrate(mixed, typing);
  assert.equal(result.ok, true);
  assert.equal(result.metric, "box");
  // Split down the middle, neither way covers enough of the phase.
  const split = [
    ...Array.from({ length: 10 }, () => sample(0.5)),
    ...Array.from({ length: 10 }, () => box),
  ];
  assert.deepEqual(calibrate(split, typing), {
    ok: false,
    reason: "face_not_steady",
  });
});
