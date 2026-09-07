// Run with: node --test tests/browser/*.test.js
//
// The gate decides whether a candidate is allowed to meet the interviewer, so
// its rules are worth pinning: a muted microphone and a browser that never
// unblocked audio must both keep the room closed. Camera is also required
// before the room opens.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  MIC_CONFIRM_FRAMES,
  MIC_SILENT_PEAK,
  mediaReadiness,
  outputUsable,
  peakLevel,
  sustainedPeak,
  videoTrackReady,
} from "../../web/audio-check.js";

const silence = () => new Uint8Array(64).fill(128);
const speech = () => Uint8Array.from({ length: 64 }, (_, i) => (i % 2 ? 200 : 60));
const passingMedia = {
  outputConfirmed: true,
  micPeak: MIC_SILENT_PEAK,
  cameraReady: true,
};

test("peak level reads deviation from the analyser midpoint", () => {
  assert.equal(peakLevel(silence()), 0, "a flat 128 signal is silence");
  assert.equal(peakLevel(new Uint8Array([128, 128, 0])), 1, "full negative swing");
  assert.equal(peakLevel(new Uint8Array([128, 256])), 1, "full positive swing");
  assert.ok(peakLevel(speech()) > MIC_SILENT_PEAK, "ordinary speech clears the threshold");
  assert.equal(peakLevel(new Uint8Array()), 0, "no samples yet is not a crash");
});

// A candidate who taps the desk once has not proven they can be heard, and the
// old all-time maximum opened the gate on any single spike.
test("one loud frame does not prove the microphone works", () => {
  const loud = 0.5;
  const quiet = 0;

  assert.equal(sustainedPeak([]), 0, "nothing to judge yet");
  assert.equal(sustainedPeak([loud, loud]), 0, "too few frames to judge");
  assert.equal(
    sustainedPeak([quiet, quiet, quiet, quiet, quiet, loud]),
    0,
    "a single spike among silence is not speech",
  );
  assert.equal(
    sustainedPeak(Array(MIC_CONFIRM_FRAMES).fill(loud)),
    loud,
    "a run of loud frames is speech",
  );
  assert.equal(
    sustainedPeak([...Array(20).fill(quiet), ...Array(MIC_CONFIRM_FRAMES).fill(loud)]),
    loud,
    "only the most recent frames matter",
  );
  assert.ok(
    sustainedPeak(Array(MIC_CONFIRM_FRAMES).fill(MIC_SILENT_PEAK)) >= MIC_SILENT_PEAK,
    "sustained speech at the threshold still opens the gate",
  );
});

test("a silent microphone keeps the room closed", () => {
  const state = mediaReadiness({ ...passingMedia, micPeak: 0 });

  assert.equal(state.ready, false);
  assert.equal(state.blocker, "mic-silent");
  assert.match(state.message, /microphone/i);
});

test("a denied microphone reports the reason and stays closed", () => {
  const state = mediaReadiness({ ...passingMedia, micError: "Permission denied" });

  assert.equal(state.ready, false);
  assert.equal(state.blocker, "mic-error");
  assert.match(state.message, /Permission denied/);
});

test("hearing nothing keeps the room closed even with a working microphone", () => {
  const state = mediaReadiness({ ...passingMedia, outputConfirmed: false });

  assert.equal(state.ready, false);
  assert.equal(state.blocker, "output");
});

test("all required media proven opens the room", () => {
  const state = mediaReadiness(passingMedia);

  assert.equal(state.ready, true);
  assert.equal(state.blocker, null);
});

test("the default state is closed, not open", () => {
  // A missing field must never read as "verified"; the gate is the only thing
  // standing between a broken device and a wasted interview.
  assert.equal(mediaReadiness().ready, false);
  assert.equal(mediaReadiness({}).ready, false);
});

test("each step reports itself so the panel can tick it off independently", () => {
  assert.deepEqual(mediaReadiness().steps, { output: false, mic: false, camera: false });
  assert.deepEqual(
    mediaReadiness({ micPeak: 1 }).steps,
    { output: false, mic: true, camera: false },
    "a working mic ticks even while output is unproven",
  );
  assert.deepEqual(
    mediaReadiness({ outputConfirmed: true }).steps,
    { output: true, mic: false, camera: false },
    "hearing the tone ticks even while the mic is silent",
  );
  assert.deepEqual(mediaReadiness(passingMedia).steps, { output: true, mic: true, camera: true });
});

test("a microphone failure un-ticks its step even after it once passed", () => {
  const state = mediaReadiness({ ...passingMedia, micError: "Device lost" });

  assert.equal(state.steps.mic, false, "a lost device must not keep a stale tick");
  assert.equal(state.steps.output, true, "the other step is unaffected");
  assert.equal(state.ready, false);
});

test("the tick threshold is the same one that gates joining", () => {
  const justUnder = mediaReadiness({ ...passingMedia, micPeak: MIC_SILENT_PEAK - 0.001 });
  const exactly = mediaReadiness(passingMedia);

  assert.equal(justUnder.steps.mic, false);
  assert.equal(justUnder.ready, false);
  assert.equal(exactly.steps.mic, true);
  assert.equal(exactly.ready, true, "a ticked step must never disagree with the join button");
});

test("an unsupported browser keeps the room closed", () => {
  const state = mediaReadiness({ ...passingMedia, browserSupported: false });

  assert.equal(state.ready, false);
  assert.equal(state.blocker, "browser");
  assert.match(state.message, /browser/i);
});

test("camera tracks must be live and enabled", () => {
  assert.equal(videoTrackReady({ readyState: "live", enabled: true, muted: false }), true);
  assert.equal(videoTrackReady({ readyState: "ended", enabled: true, muted: false }), false);
  assert.equal(videoTrackReady({ readyState: "live", enabled: false, muted: false }), false);
  assert.equal(videoTrackReady({ readyState: "live", enabled: true, muted: true }), false);
  assert.equal(videoTrackReady(null), false);
});

test("loaded face detector can block camera readiness", () => {
  const missing = mediaReadiness({ ...passingMedia, faceReady: false });
  const multiple = mediaReadiness({ ...passingMedia, faceError: "multiple faces detected" });

  assert.equal(missing.ready, false);
  assert.equal(missing.blocker, "camera");
  assert.equal(multiple.ready, false);
  assert.equal(multiple.blocker, "face-error");
});

test("output counts as usable only once the context is running", () => {
  assert.equal(outputUsable("running"), true);
  assert.equal(outputUsable("suspended"), false, "suspended is the autoplay block");
  assert.equal(outputUsable("closed"), false);
  assert.equal(outputUsable(undefined), false);
});

// The one arm of the blocker ladder nothing reached. Its microphone twin has
// two tests; a camera the browser refused had none, so `camera-error` could
// have been deleted outright and the gate would have fallen through to
// whichever check came next and reported the wrong cause.
test("a refused camera reports the camera as the reason and stays closed", () => {
  const state = mediaReadiness({ ...passingMedia, cameraReady: false, cameraError: "NotReadableError" });
  assert.equal(state.ready, false);
  assert.equal(state.blocker, "camera-error");
  assert.match(state.message, /Camera unavailable: NotReadableError/);
  // Un-ticked, and only it: a camera failure is not evidence about the mic.
  assert.equal(state.steps.camera, false);
  assert.equal(state.steps.mic, true);
  assert.equal(state.steps.output, true);

  // The error alone un-ticks the step. A camera that reported itself ready and
  // then failed is the case where `!cameraError` is the deciding conjunct
  // rather than a second opinion about `cameraReady`, and it is how a candidate
  // whose camera was unplugged mid-check keeps a green tick they should not.
  const late = mediaReadiness({ ...passingMedia, cameraReady: true, cameraError: "NotReadableError" });
  assert.equal(late.steps.camera, false, "an error un-ticks the step on its own");
  assert.equal(late.blocker, "camera-error");
});

// The ladder is ordered, and the order is the claim: a candidate with two
// things wrong is told about the microphone first, because it is the one the
// interview cannot start without.
test("a microphone failure outranks a camera failure", () => {
  const state = mediaReadiness({
    ...passingMedia,
    cameraReady: false,
    cameraError: "NotReadableError",
    micError: "NotAllowedError",
  });
  assert.equal(state.blocker, "mic-error");
  assert.equal(state.steps.camera, false, "the camera step still shows what it knows");
});
