import { test } from "node:test";
import assert from "node:assert/strict";

import {
  FACE_DETECT_INTERVAL_MS,
  SCREEN_DIFF_INTERVAL_MS,
  TRACKING_INTERVAL_MS,
  analysisDetail,
  applyFrame,
  createAnalysisState,
} from "../../web/integrity-worker.js";

test("integrity worker exposes the cascade schedule", () => {
  assert.equal(SCREEN_DIFF_INTERVAL_MS, 1000);
  assert.equal(FACE_DETECT_INTERVAL_MS, 300);
  assert.equal(TRACKING_INTERVAL_MS, 100);
});

test("screen analysis is throttled to one diff per second", () => {
  const state = createAnalysisState();

  assert.deepEqual(applyFrame(state, { source: "screen", at: 0, transport: "VideoFrame" })[0].analyses, ["screen_diff"]);
  assert.deepEqual(applyFrame(state, { source: "screen", at: 999, transport: "VideoFrame" }), []);
  assert.deepEqual(applyFrame(state, { source: "screen", at: 1000, transport: "VideoFrame" })[0].analyses, ["screen_diff"]);
});

test("camera analysis aggregates frames while respecting each interval", () => {
  const state = createAnalysisState();

  assert.deepEqual(applyFrame(state, { source: "camera", at: 0, transport: "ImageBitmap" })[0].analyses, ["face_detect", "tracking"]);
  assert.deepEqual(applyFrame(state, { source: "camera", at: 50, transport: "ImageBitmap" }), []);
  const tracking = applyFrame(state, { source: "camera", at: 100, transport: "ImageBitmap" })[0];
  assert.deepEqual(tracking.analyses, ["tracking"]);
  assert.equal(tracking.frameCount, 2);
  const faceAndTracking = applyFrame(state, { source: "camera", at: 300, transport: "ImageBitmap" })[0];
  assert.deepEqual(faceAndTracking.analyses, ["face_detect", "tracking"]);
});

test("worker detail is compact and contains no frame payload", () => {
  const detail = analysisDetail({
    source: "screen",
    analyses: ["screen_diff"],
    frameCount: 3,
    transport: "canvas",
  });

  assert.equal(detail, "source=screen;analysis=screen_diff;frames=3;transport=canvas");
});

test("frame dimensions and counts survive aggregation whatever the transport", () => {
  const state = createAnalysisState();
  const output = applyFrame(state, {
    source: "screen",
    at: 0,
    transport: "canvas",
    width: 160,
    height: 90,
  })[0];

  assert.equal(output.transport, "canvas");
  assert.equal(output.width, 160);
  assert.equal(output.height, 90);
  assert.equal(output.frameCount, 1);
});

// The guard on the way in, both arms. A frame naming a source the schedule
// does not know, or carrying no usable clock, produces no analysis rather than
// an entry under a made-up interval: `at` is a main-thread monotonic reading
// and the worker has no second opinion about it, so there is no fallback that
// would not invent an interval that never happened.
test("a frame the schedule cannot place produces no analysis", () => {
  const state = createAnalysisState();
  for (const source of ["clipboard", "", null, undefined]) {
    assert.deepEqual(applyFrame(state, { source, at: 0 }), [], `source ${JSON.stringify(source)}`);
  }
  for (const at of [undefined, null, NaN, Infinity, -Infinity, "0"]) {
    assert.deepEqual(applyFrame(state, { source: "screen", at }), [], `at ${String(at)}`);
  }
  assert.deepEqual(applyFrame(state, null), []);
  assert.deepEqual(applyFrame(state, undefined), []);

  // Refused, and not counted either: a rejected frame that still incremented
  // the pending tally would inflate the next real analysis's frameCount with
  // frames the worker never looked at.
  const [output] = applyFrame(state, { source: "screen", at: 0 });
  assert.equal(output.frameCount, 1, "only the frame that was accepted is counted");
});

// What the worker reports when the main thread sends a frame with no shape
// attached. These defaults are what a reviewer reads off the event, so a
// missing one renders "transport=undefined" in the integrity detail line.
test("a frame carrying no shape is reported with defaults, not undefined", () => {
  const [output] = applyFrame(createAnalysisState(), { source: "screen", at: 0 });
  assert.equal(output.transport, "metadata");
  assert.equal(output.width, 0);
  assert.equal(output.height, 0);
  assert.match(analysisDetail(output), /transport=metadata$/);
  // Zero is a shape the browser really reports for a track that has not sized
  // itself yet, and it must survive as 0 rather than becoming something else.
  const [zero] = applyFrame(createAnalysisState(), { source: "screen", at: 0, width: 0, height: 0, transport: "" });
  assert.equal(zero.transport, "metadata", "an empty transport is no transport");
  assert.equal(zero.width, 0);
});

// A clock that goes backwards is what a throttled or resumed tab produces. The
// interval test is a forward-difference, so a backwards reading must not be
// read as "long enough has passed".
test("a clock that goes backwards does not open the next window early", () => {
  const state = createAnalysisState();
  assert.deepEqual(applyFrame(state, { source: "screen", at: 10_000 })[0].analyses, ["screen_diff"]);
  // Earlier than the last run: the difference is negative, so nothing fires.
  assert.deepEqual(applyFrame(state, { source: "screen", at: 9_000 }), []);
  assert.deepEqual(applyFrame(state, { source: "screen", at: 0 }), []);
  // And the schedule is still anchored to the last frame that ran, so the
  // window reopens a second after that one rather than after the stale reading.
  const reopened = applyFrame(state, { source: "screen", at: 11_000 })[0];
  assert.deepEqual(reopened.analyses, ["screen_diff"]);
  // Three, and on this analysis rather than the next one: the two frames the
  // backwards clock suppressed were counted rather than dropped, and they are
  // reported by the analysis that reopens the window, along with its own.
  assert.equal(reopened.frameCount, 3, "a suppressed frame is still a frame that arrived");
  // Counted once, so the next window starts again from its own frame.
  assert.equal(applyFrame(state, { source: "screen", at: 12_000 })[0].frameCount, 1);
});
