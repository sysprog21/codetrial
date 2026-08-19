// MediaPipe's bundle has no worker path. It fetches its wasm loader by setting
// `src` on a created <script>, appending it to `document.body`, and waiting for
// the load event, then publishes the module on `window`. A classic worker has
// `importScripts`, which is that same operation done synchronously, so the three
// DOM members the bundle actually touches are bridged onto it and nothing else
// is invented. Without this the bundle throws on `document` and the detector
// reports itself unavailable, which is what the interview page was showing.
//
// The emscripten loader underneath already supports workers on its own; only
// this outer bundle needs the bridge. Re-check it when upgrading the vendored
// package: `tests/browser/face-detector.test.js` runs the real thing in a real
// browser and fails if the bridge stops fitting.
self.window = self;
self.document = {
  createElement: (tag) => (tag === "canvas" ? new OffscreenCanvas(1, 1) : {
    setAttribute(name, value) {
      this[name] = value;
    },
    addEventListener(type, listener) {
      this[`on${type}`] = listener;
    },
  }),
  body: {
    appendChild(element) {
      try {
        importScripts(element.src);
        element.onload?.();
      } catch {
        element.onerror?.();
      }
    },
  },
  // Emscripten's GL layer registers webglcontextlost handlers on the document.
  // There is no document to lose a context, so these accept and drop.
  addEventListener() {},
  removeEventListener() {},
};

importScripts("/vendor/face-detection/face_detection.js");

const VENDOR_BASE = "/vendor/face-detection/";

let detector = null;

// A classic worker cannot import from `face-presence.js`, which is what forces
// two copies: this base path, and `normalize` below, which is the twin of
// `normalizeFaceResults` and has to be changed with it. The base is at least a
// path and not a name table: the vendored files keep MediaPipe's own names, so
// renaming a file is the only way to break this one.
function locateFile(file) {
  return `${VENDOR_BASE}${file}`;
}

// Memoised as a promise, not as the instance. Assigning the instance before
// `initialize()` resolved meant a frame arriving during the cold start, which
// takes seconds, got a detector that was not ready.
function ensureDetector() {
  detector ||= (async () => {
    const detection = new FaceDetection({ locateFile });
    detection.setOptions({ model: "short", minDetectionConfidence: 0.5 });
    await detection.initialize();
    return detection;
  })();
  return detector;
}

function normalize(results = {}) {
  const detections = Array.isArray(results.detections) ? results.detections : [];
  const confidence = detections.reduce((best, detection) => {
    const score = Array.isArray(detection.score) ? detection.score[0] : detection.score;
    return Math.max(best, Number.isFinite(score) ? score : 0);
  }, 0);
  return { count: detections.length, confidence };
}

// MediaPipe has one wasm heap and one result callback, so two `send` calls in
// flight trap it with "memory access out of bounds" and take the detector down
// for the rest of the session. Frames arrive faster than a cold start can
// answer them, so they are detected strictly one at a time.
let inFlight = Promise.resolve();

self.onmessage = (event) => {
  const message = event.data || {};
  if (message.type !== "detect") return;
  inFlight = inFlight.then(() => detect(message));
};

async function detect(message) {
  try {
    const faceDetection = await ensureDetector();
    const results = await new Promise((resolve, reject) => {
      faceDetection.onResults(resolve);
      Promise.resolve(faceDetection.send({ image: message.frame })).catch(reject);
    });
    self.postMessage({ type: "result", id: message.id, ...normalize(results) });
  } catch (error) {
    // The reason travels: "face detection is not running" with nothing behind
    // it is what made a missing asset and a missing DOM look identical from
    // the outside.
    self.postMessage({
      type: "unavailable",
      id: message.id,
      reason: String(error?.message || error).slice(0, 100),
    });
  } finally {
    message.frame?.close?.();
  }
}
