export const FACE_ANOMALY_THRESHOLD_MS = 2000;
export const FACE_SEVERE_THRESHOLD_MS = 15000;
export const FACE_VENDOR_BASE = "/vendor/face-detection/";
const FACE_WORKER_URL = "/face-worker.js";
// A gap longer than this between two face samples means nothing was watched in
// between: a suspended laptop, a throttled background tab, a detector that
// stalled. The tracker is reset rather than charged for the interval.
export const FACE_SAMPLE_GAP_MS = 5000;

// What a detector reports when it could not look, as distinct from looking and
// finding nobody. The tracker never sees one of these.
export const FACE_SAMPLE_UNAVAILABLE = {
  available: false,
  count: 0,
  confidence: 0,
};

// The vendored files keep the names MediaPipe asks for, so this is a join and
// not a translation table. It used to be a table, duplicated here and in
// `face-worker.js`, and both copies were missing the two `.wasm` entries: the
// emscripten loader asks `locateFile` for its own binary, got the untranslated
// name back, and every request 404ed. Renaming one file is now the only way to
// break this, instead of renaming one file and forgetting one of two maps.
export function faceAssetUrl(file, baseUrl = FACE_VENDOR_BASE) {
  return `${baseUrl}${file}`;
}

// The most confident face's box and keypoints, in frame-relative units, for
// the task workspace's look-away rule; null when there is no face or the
// detector gives no geometry. Nothing else about the face is kept.
export function faceGeometry(detections, score) {
  let best = null;
  for (const detection of detections) {
    if (!best || score(detection) > score(best)) best = detection;
  }
  const box = best?.boundingBox;
  if (
    !box ||
    !["xCenter", "yCenter", "width", "height"].every((key) =>
      Number.isFinite(box[key]),
    )
  )
    return null;
  // All or none: the head-down measure reads eyes, nose and mouth by
  // position, so dropping one bad point would shift the rest into the wrong
  // places. Without them the measure falls back to the box.
  const points = Array.isArray(best.landmarks) ? best.landmarks : [];
  const landmarks = points.every(
    (point) => Number.isFinite(point?.x) && Number.isFinite(point?.y),
  )
    ? points.map((point) => [point.x, point.y])
    : [];
  return {
    box: [box.xCenter, box.yCenter, box.width, box.height],
    landmarks,
  };
}

// The twin of `normalize` in `face-worker.js`; change both together.
export function normalizeFaceResults(results = {}) {
  const detections = Array.isArray(results.detections)
    ? results.detections
    : [];
  const score = (detection) => {
    const value = Array.isArray(detection.score)
      ? detection.score[0]
      : detection.score;
    return Number.isFinite(value) ? value : 0;
  };
  const confidence = detections.reduce(
    (best, detection) => Math.max(best, score(detection)),
    0,
  );
  return {
    count: detections.length,
    confidence,
    face: faceGeometry(detections, score),
  };
}

/// Whether the preflight camera step may pass, given one detector sample.
///
/// Two rules, and neither is obvious enough to leave inline in the preflight.
///
/// A gap in sampling is not an observation: an unavailable sample carries
/// count 0, and reading that as "no face" once left the Start button disabled
/// while the candidate sat in front of a working camera, reading "stay in
/// frame" with no way past. `createFacePresenceTracker` already says this about
/// the interview; the preflight has the same rule for the same reason.
///
/// And exactly one face passes, while zero is "not yet" and two or more is an
/// error the candidate can act on. The preflight used to gate the error on a
/// `faceDetectorLoaded` flag that was set immediately before the sampling loop
/// started and never cleared, so it was always true by the time it was read.
export function facePresenceVerdict(sample) {
  if (!sample?.available) return { ready: true, error: null };
  return {
    ready: sample.count === 1,
    error: sample.count > 1 ? "multiple faces detected" : null,
  };
}

function faceEventType(count) {
  if (count === 0) return "FACE_MISSING";
  if (count > 1) return "MULTIPLE_FACES";
  return "FACE_DETECTED";
}

// Severity is a property of the event kind, so it lives beside the kinds rather
// than being re-derived by whatever ships the event. A new kind adds a row here
// instead of an arm in a conditional somewhere else.
const FACE_EVENT_SEVERITY = {
  FACE_DETECTED: "info",
  FACE_MISSING_SEVERE: "critical",
};

export function faceEventSeverity(type) {
  return FACE_EVENT_SEVERITY[type] || "warning";
}

export function createFacePresenceTracker({
  thresholdMs = FACE_ANOMALY_THRESHOLD_MS,
  severeThresholdMs = FACE_SEVERE_THRESHOLD_MS,
} = {}) {
  let stable = null;
  let pending = null;
  let pendingSince = 0;
  // Kept apart from pendingSince, which restarts on every flicker. Someone who
  // has genuinely left the frame still flickers: a poster on the wall, a photo,
  // a passing shadow all register as a face for one sample. Running the severe
  // clock off the debounce clock means those never accumulate and the severe
  // threshold is never reached, and the mirror-image case charges a present
  // candidate for time they were there.
  let missingSince = 0;
  let severeEmitted = false;

  return {
    // A gap in sampling is not an observation. The caller resets rather than
    // letting a suspended tab or a stalled detector look like an absence.
    reset() {
      stable = null;
      pending = null;
      pendingSince = 0;
      missingSince = 0;
      severeEmitted = false;
    },
    update(sample, now) {
      const next = faceEventType(sample.count);
      if (next === stable) {
        pending = null;
        if (
          stable === "FACE_MISSING" &&
          !severeEmitted &&
          now - missingSince >= severeThresholdMs
        ) {
          severeEmitted = true;
          return {
            type: "FACE_MISSING_SEVERE",
            sample,
            durationMs: now - missingSince,
          };
        }
        return null;
      }
      if (next === "FACE_DETECTED") {
        stable = next;
        pending = null;
        missingSince = 0;
        severeEmitted = false;
        return { type: next, sample, durationMs: 0 };
      }
      if (pending !== next) {
        pending = next;
        pendingSince = now;
        return null;
      }
      if (now - pendingSince < thresholdMs) return null;
      stable = next;
      pending = null;
      if (stable === "FACE_MISSING") missingSince = pendingSince;
      return { type: stable, sample, durationMs: now - pendingSince };
    },
  };
}

export async function createFacePresenceDetector({
  scope = globalThis,
  loadScript = defaultLoadScript,
  baseUrl = FACE_VENDOR_BASE,
  Detector = null,
  // `false` skips the browser's own detector, which reports a count but no
  // face geometry.
  native = true,
} = {}) {
  try {
    const NativeFaceDetector = scope.FaceDetector;
    if (!Detector && native && typeof NativeFaceDetector === "function") {
      const detector = new NativeFaceDetector({ fastMode: true });
      return {
        available: true,
        async detect(image) {
          const detections = await detector.detect(image);
          return {
            available: true,
            count: detections.length,
            confidence: detections.length ? 1 : 0,
          };
        },
        close() {},
      };
    }
    // On a page, MediaPipe runs in `face-worker.js` rather than here. Its
    // emscripten glue builds functions from text, and the document's policy
    // withholds `'unsafe-eval'`: loaded into the page, the bundle throws during
    // initialization and the preflight quietly loses its face check. The worker
    // is the one response the server grants eval to.
    if (
      !Detector &&
      scope.document &&
      typeof scope.Worker === "function" &&
      typeof scope.createImageBitmap === "function"
    ) {
      return createWorkerFaceDetector(scope);
    }
    await loadScript(scope, baseUrl);
    const FaceDetection = Detector || scope.FaceDetection;
    if (typeof FaceDetection !== "function")
      return { available: false, reason: "detector_unavailable" };
    const detector = new FaceDetection({
      locateFile: (file) => faceAssetUrl(file, baseUrl),
    });
    detector.setOptions?.({ model: "short", minDetectionConfidence: 0.5 });
    await detector.initialize?.();
    return {
      available: true,
      detect(image) {
        return new Promise((resolve) => {
          detector.onResults((results) =>
            resolve({ available: true, ...normalizeFaceResults(results) }),
          );
          // A detection that threw says nothing about the candidate. Reporting
          // it as zero faces is what let a wasm load failure end a valid
          // interview fifteen seconds later.
          Promise.resolve(detector.send({ image })).catch(() =>
            resolve(FACE_SAMPLE_UNAVAILABLE),
          );
        });
      },
      close() {
        return detector.close?.();
      },
    };
  } catch {
    return { available: false, reason: "detector_load_failed" };
  }
}

/// The detector behind `face-worker.js`, speaking the protocol
/// `integrity-worker.js` already uses: an `ImageBitmap` transferred in, a
/// count and confidence back, matched by id. A worker that fails to start
/// answers every outstanding and later request as unavailable rather than
/// leaving them unsettled.
function createWorkerFaceDetector(scope) {
  const worker = new scope.Worker(FACE_WORKER_URL);
  const pending = new Map();
  let nextId = 0;
  let failed = false;
  const settleAll = () => {
    failed = true;
    const waiting = [...pending.values()];
    pending.clear();
    for (const done of waiting) done(FACE_SAMPLE_UNAVAILABLE);
  };
  worker.onmessage = (event) => {
    const result = event.data || {};
    const done = pending.get(result.id);
    if (!done) return;
    pending.delete(result.id);
    done(
      result.type === "result"
        ? {
            available: true,
            count: result.count,
            confidence: result.confidence,
            face: result.face ?? null,
          }
        : FACE_SAMPLE_UNAVAILABLE,
    );
  };
  worker.onerror = settleAll;
  return {
    available: true,
    async detect(image) {
      if (failed) return FACE_SAMPLE_UNAVAILABLE;
      let frame;
      try {
        frame = await scope.createImageBitmap(image);
      } catch {
        return FACE_SAMPLE_UNAVAILABLE;
      }
      if (failed) {
        frame.close?.();
        return FACE_SAMPLE_UNAVAILABLE;
      }
      return new Promise((resolve) => {
        const id = ++nextId;
        pending.set(id, resolve);
        try {
          worker.postMessage({ type: "detect", id, frame }, [frame]);
        } catch {
          pending.delete(id);
          frame.close?.();
          resolve(FACE_SAMPLE_UNAVAILABLE);
        }
      });
    },
    close() {
      settleAll();
      worker.terminate();
    },
  };
}

async function defaultLoadScript(scope, baseUrl) {
  if (scope.FaceDetection) return;
  await import(`${baseUrl}face_detection.js`);
}
