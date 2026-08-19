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
