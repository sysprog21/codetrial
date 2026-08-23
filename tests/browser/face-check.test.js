import { test } from "node:test";
import assert from "node:assert/strict";

import { createFaceCheck } from "../../web/face-check.js";

globalThis.MediaStream = class {
  constructor(tracks) {
    this.tracks = tracks;
  }
};

function harness(overrides = {}) {
  const video = { srcObject: null, play: async () => {} };
  const seen = { verdicts: 0, closed: 0 };
  const check = createFaceCheck({
    video,
    trackOf: () => ({ kind: "video" }),
    isReady: () => true,
    isFinished: () => false,
    createDetector: async () => ({
      available: true,
      detect: async () => "raw",
      close: () => {
        seen.closed += 1;
      },
    }),
    verdictOf: () => ({ ready: true, error: null }),
    onVerdict: () => {
      seen.verdicts += 1;
    },
    checkMs: 5,
    ...overrides,
  });
  return { check, video, seen };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

test("a camera that is not ready is not watched", async () => {
  const { check, video } = harness({ isReady: () => false });

  await check.start();

  assert.equal(video.srcObject, null);
});

// No detector means no verdict to wait for, not a failed one. Reporting an
// error here would block a candidate whose browser simply lacks the API.
test("no available detector leaves the check ready rather than failed", async () => {
  const { check } = harness({ createDetector: async () => ({ available: false }) });

  await check.start();

  assert.equal(check.ready, true);
  assert.equal(check.error, null);
});

test("a verdict is reported through onVerdict", async () => {
  const { check, seen } = harness({
    verdictOf: () => ({ ready: false, error: "no face" }),
  });

  await check.start();
  await settle();

  assert.equal(check.ready, false);
  assert.equal(check.error, "no face");
  assert.ok(seen.verdicts >= 1);
  check.reset();
});

// The whole reason for `generation`: every await is a chance for the camera to
// be replaced underneath a run, and a resumption that did not notice would
// write a dead camera's verdict over the live one's.
test("a reset mid-flight discards the detector the old run was building", async () => {
  let release;
  const closed = [];
  const { check } = harness({
    createDetector: () => new Promise((resolve) => {
      release = () => resolve({
        available: true,
        detect: async () => "raw",
        close: () => closed.push("stale"),
      });
    }),
  });

  const running = check.start();
  // A tick, so the run gets past `video.play()` and is actually sitting in
  // `createDetector` when the camera is replaced under it.
  await settle();
  check.reset();
  release();
  await running;
  await settle();

  assert.deepEqual(closed, ["stale"], "the stale detector was kept instead of closed");
  assert.equal(check.ready, true, "a reset leaves nothing to wait for");
});

test("a reset clears the video element it was rendering into", async () => {
  const { check, video } = harness();

  await check.start();
  assert.notEqual(video.srcObject, null);

  check.reset();
  assert.equal(video.srcObject, null);
});

// `running` and `generation` were separate flags for the same fact, and a
// second start over a live run used to build a second detector.
test("a second start while one is running does nothing", async () => {
  let built = 0;
  const { check } = harness({
    createDetector: async () => {
      built += 1;
      return { available: false };
    },
  });

  await Promise.all([check.start(), check.start()]);

  assert.equal(built, 1);
});
