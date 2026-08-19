import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";

import {
  FACE_ANOMALY_THRESHOLD_MS,
  createFacePresenceDetector,
  createFacePresenceTracker,
  faceAssetUrl,
  facePresenceVerdict,
  normalizeFaceResults,
} from "../../web/face-presence.js";

const vendorFile = (file) => new URL(`../../web/vendor/face-detection/${file}`, import.meta.url);

/// `SHA256SUMS` is the manifest `make verify-vendor` already checks the
/// directory against, so it is the one list of vendored names. Restating them
/// here would be a fourth copy to keep in step with the directory, the manifest
/// and the server test.
const vendoredFiles = readFileSync(vendorFile("SHA256SUMS"), "utf8")
  .split("\n")
  .filter(Boolean)
  .map((line) => line.split(/\s+/)[1]);

/// MediaPipe hands `locateFile` its own internal names, and the emscripten
/// loaders ask for their own `.wasm` that way too. A vendored file stored under
/// any other name 404s and the detector never initialises, which the browser
/// could only report as "face detection is not running". That is exactly what
/// happened: the files were renamed, and the two hand-written translation
/// tables that made the rename work were both missing the `.wasm` entries.
test("every asset MediaPipe asks for is vendored under that exact name", () => {
  assert.ok(vendoredFiles.length >= 7, "the manifest should list the vendored assets");
  for (const file of vendoredFiles) {
    // MediaPipe's own name, so no translation table stands between the two.
    assert.match(file, /^face_detection/, `${file} is not named the way MediaPipe asks for it`);
    assert.equal(faceAssetUrl(file), `/vendor/face-detection/${file}`);
    assert.ok(existsSync(vendorFile(file)), `${file} is not vendored under that name`);
  }
});

/// The loaders name their own binary rather than being told, so the list above
/// is only right for as long as it matches what is compiled into them.
test("the wasm loaders request the binaries that are vendored", () => {
  for (const loader of [
    "face_detection_solution_wasm_bin.js",
    "face_detection_solution_simd_wasm_bin.js",
  ]) {
    const wanted = loader.replace(/\.js$/, ".wasm");
    assert.ok(
      readFileSync(vendorFile(loader), "utf8").includes(wanted),
      `${loader} does not name ${wanted}`,
    );
    assert.ok(existsSync(vendorFile(wanted)), `${wanted} is not vendored`);
  }
});

test("face detector wrapper loads, configures, and normalizes results", async () => {
  const calls = [];
  class Detector {
    constructor(options) {
      calls.push(["ctor", options.locateFile("face_detection_short.binarypb")]);
    }
    setOptions(options) {
      calls.push(["options", options.model, options.minDetectionConfidence]);
    }
    async initialize() {
      calls.push(["initialize"]);
    }
    onResults(callback) {
      this.callback = callback;
    }
    async send() {
      this.callback({ detections: [{ score: [0.75] }] });
    }
  }

  const detector = await createFacePresenceDetector({
    Detector,
    loadScript: async () => {},
  });
  const sample = await detector.detect({});

  assert.equal(detector.available, true);
  assert.deepEqual(sample, { available: true, count: 1, confidence: 0.75 });
  assert.deepEqual(calls, [
    ["ctor", "/vendor/face-detection/face_detection_short.binarypb"],
    ["options", "short", 0.5],
    ["initialize"],
  ]);
});

test("face detector unavailable skips instead of blocking", async () => {
  const detector = await createFacePresenceDetector({ loadScript: async () => {} });

  assert.equal(detector.available, false);
});

test("face detector uses native FaceDetector when available", async () => {
  class FaceDetector {
    async detect() {
      return [{ boundingBox: {} }, { boundingBox: {} }];
    }
  }

  const detector = await createFacePresenceDetector({
    scope: { FaceDetector },
    loadScript: async () => {
      throw new Error("native path should not load MediaPipe");
    },
  });

  assert.equal(detector.available, true);
  assert.deepEqual(await detector.detect({}), { available: true, count: 2, confidence: 1 });
});

test("a detection that throws reports unavailable, not zero faces", async () => {
  class Detector {
    setOptions() {}
    async initialize() {}
    onResults() {}
    send() {
      return Promise.reject(new Error("wasm gone"));
    }
  }

  const detector = await createFacePresenceDetector({ Detector, loadScript: async () => {} });

  // Zero faces would put the tracker fifteen seconds away from ending a valid
  // interview over a failure that says nothing about the candidate.
  assert.equal((await detector.detect({})).available, false);
});

test("face tracker thresholds missing, severe, and multiple-face anomalies", () => {
  const tracker = createFacePresenceTracker({
    thresholdMs: FACE_ANOMALY_THRESHOLD_MS,
    severeThresholdMs: 15000,
  });

  assert.equal(tracker.update({ count: 0, confidence: 0 }, 0), null);
  assert.equal(tracker.update({ count: 0, confidence: 0 }, FACE_ANOMALY_THRESHOLD_MS - 1), null);
  assert.equal(tracker.update({ count: 0, confidence: 0 }, FACE_ANOMALY_THRESHOLD_MS).type, "FACE_MISSING");
  assert.equal(tracker.update({ count: 0, confidence: 0 }, 14999), null);
  assert.equal(tracker.update({ count: 0, confidence: 0 }, 15000).type, "FACE_MISSING_SEVERE");
  assert.equal(tracker.update({ count: 1, confidence: 0.9 }, 16000).type, "FACE_DETECTED");
  assert.equal(tracker.update({ count: 0, confidence: 0 }, 17000), null);
  assert.equal(tracker.update({ count: 0, confidence: 0 }, 19000).type, "FACE_MISSING");
  assert.equal(tracker.update({ count: 2, confidence: 0.8 }, 20000), null);
  assert.equal(tracker.update({ count: 2, confidence: 0.8 }, 22000).type, "MULTIPLE_FACES");
  assert.equal(tracker.update({ count: 1, confidence: 0.7 }, 22001).type, "FACE_DETECTED");
});

test("detector flicker does not restart the severe clock", () => {
  const tracker = createFacePresenceTracker({ thresholdMs: 2000, severeThresholdMs: 15000 });

  assert.equal(tracker.update({ count: 0, confidence: 0 }, 0), null);
  assert.equal(tracker.update({ count: 0, confidence: 0 }, 2000).type, "FACE_MISSING");
  // An empty chair in front of a poster registers one face now and then. Timing
  // the absence off the debounce clock let each of these push the severe
  // threshold out, so an absence long enough to matter never reached it.
  for (let at = 3000; at < 15000; at += 3000) {
    assert.equal(tracker.update({ count: 2, confidence: 0.8 }, at), null);
    assert.equal(tracker.update({ count: 0, confidence: 0 }, at + 100), null);
  }

  const severe = tracker.update({ count: 0, confidence: 0 }, 17000);
  assert.equal(severe.type, "FACE_MISSING_SEVERE");
  // Measured from when they went missing, not from the last flicker, which
  // would have made this 5000 and kept the threshold permanently out of reach.
  assert.equal(severe.durationMs, 17000);
});

test("a gap in sampling is not an absence", () => {
  const tracker = createFacePresenceTracker({ thresholdMs: 2000, severeThresholdMs: 15000 });

  assert.equal(tracker.update({ count: 0, confidence: 0 }, 0), null);
  assert.equal(tracker.update({ count: 0, confidence: 0 }, 2000).type, "FACE_MISSING");
  // The caller resets when frames stop arriving. Without it the first frame
  // after a suspended lid or a throttled tab is already past the threshold.
  tracker.reset();
  assert.equal(tracker.update({ count: 0, confidence: 0 }, 600000), null);
  assert.equal(tracker.update({ count: 0, confidence: 0 }, 602000).type, "FACE_MISSING");
});

test("face normalization handles empty and multi-face results", () => {
  assert.deepEqual(normalizeFaceResults({}), { count: 0, confidence: 0 });
  assert.deepEqual(
    normalizeFaceResults({ detections: [{ score: [0.4] }, { score: [0.8] }] }),
    { count: 2, confidence: 0.8 },
  );
});

// The preflight camera gate. It is mandatory, it has no bypass, and the two
// ways it can be wrong are opposite: pass someone who is not there, or trap
// someone who is. The second already happened once.
test("a gap in sampling is not an absence", () => {
  // An unavailable sample carries count 0. Reading that as "no face" left the
  // Start button disabled while the candidate sat in front of a working camera,
  // reading "stay in front of the camera", with nothing they could do about it.
  assert.deepEqual(facePresenceVerdict({ available: false, count: 0 }), { ready: true, error: null });
  assert.deepEqual(facePresenceVerdict(undefined), { ready: true, error: null });
});

test("exactly one face passes the preflight", () => {
  assert.deepEqual(facePresenceVerdict({ available: true, count: 1 }), { ready: true, error: null });
});

test("an empty frame is not ready, and says nothing the candidate cannot act on", () => {
  // No error: "nobody is in frame yet" is the ordinary state before someone
  // sits down, not a failure to report.
  assert.deepEqual(facePresenceVerdict({ available: true, count: 0 }), { ready: false, error: null });
});

test("more than one face is an error the candidate can act on", () => {
  const two = facePresenceVerdict({ available: true, count: 2 });
  assert.equal(two.ready, false);
  assert.match(two.error, /multiple faces/);
  assert.equal(facePresenceVerdict({ available: true, count: 9 }).error, two.error);
});
