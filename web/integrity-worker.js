import {
  FACE_SAMPLE_GAP_MS,
  FACE_SAMPLE_UNAVAILABLE,
  createFacePresenceDetector,
  createFacePresenceTracker,
  faceEventSeverity,
} from "./face-presence.js";

export const SCREEN_DIFF_INTERVAL_MS = 1000;
export const FACE_DETECT_INTERVAL_MS = 300;
export const TRACKING_INTERVAL_MS = 100;

const intervals = {
  screen: [["screen_diff", SCREEN_DIFF_INTERVAL_MS]],
  camera: [
    ["face_detect", FACE_DETECT_INTERVAL_MS],
    ["tracking", TRACKING_INTERVAL_MS],
  ],
};

export function createAnalysisState() {
  return { lastRun: new Map(), pendingFrames: new Map() };
}

export function applyFrame(state, frame) {
  const source = frame?.source;
  // `at` is a monotonic reading taken on the main thread. There is no sane
  // fallback: a worker's own clock has a different origin, and mixing the two
  // yields intervals that never happened.
  if (!intervals[source] || !Number.isFinite(frame.at)) return [];
  const at = frame.at;
  state.pendingFrames.set(source, (state.pendingFrames.get(source) || 0) + 1);

  const analyses = [];
  for (const [name, intervalMs] of intervals[source]) {
    const key = `${source}:${name}`;
    const previous = state.lastRun.get(key);
    if (previous === undefined || at - previous >= intervalMs) {
      state.lastRun.set(key, at);
      analyses.push(name);
    }
  }
  if (!analyses.length) return [];

  const frameCount = state.pendingFrames.get(source);
  state.pendingFrames.set(source, 0);
  return [{
    source,
    at,
    analyses,
    frameCount,
    transport: frame.transport || "metadata",
    width: frame.width || 0,
    height: frame.height || 0,
  }];
}

export function analysisDetail(output) {
  return `source=${output.source};analysis=${output.analyses.join(",")};frames=${output.frameCount};transport=${output.transport}`;
}

function closeFrame(frame) {
  frame?.close?.();
}

const inWorker = typeof WorkerGlobalScope !== "undefined" && self instanceof WorkerGlobalScope;

if (inWorker) {
  const state = createAnalysisState();
  const faceTracker = createFacePresenceTracker();
  let faceDetectorPromise = null;
  let faceWorker = null;
  let faceWorkerFailed = false;
  let faceRequestId = 0;
  let faceDetectorReportedDown = false;
  let faceDetectionInFlight = false;
  let lastFaceSampleAt = null;
  const faceRequests = new Map();

  // Frames hold GPU memory until something closes them, and only a successful
  // transfer hands that duty to the face worker. Every path that does not
  // transfer closes the frame here, because the caller cannot tell which
  // happened and a frame arrives every 300 ms.
  const detectFace = (frame) => {
    if (typeof Worker === "function") return detectFaceInClassicWorker(frame);
    faceDetectorPromise ||= createFacePresenceDetector();
    return faceDetectorPromise
      .then((detector) => detector.available && frame
        ? detector.detect(frame)
        : FACE_SAMPLE_UNAVAILABLE)
      .finally(() => closeFrame(frame));
  };

  // Answers every outstanding request and gives up on the worker for good. A
  // worker that died answers nothing, and without this the map grows by one
  // never-settled promise per frame: three per second for the length of an
  // interview, each pinning the async handler waiting on it.
  //
  // Giving up is the point of the latch. Clearing the handle alone would let the
  // next frame construct a replacement, so a worker that cannot start would be
  // respawned three times a second for the rest of the interview, each spawn
  // refetching the script and recompiling the MediaPipe bundle behind it.
  const abandonFaceWorker = (reason) => {
    faceWorker?.terminate();
    faceWorker = null;
    faceWorkerFailed = true;
    const pending = [...faceRequests.values()];
    faceRequests.clear();
    for (const done of pending) done({ ...FACE_SAMPLE_UNAVAILABLE, reason });
  };

  const faceWorkerHandle = () => {
    if (faceWorkerFailed) return null;
    if (faceWorker) return faceWorker;
    faceWorker = new Worker("/face-worker.js");
    // Bound once, not per request: the handler routes by id, so rebinding it on
    // every frame only rebuilt the same closure.
    faceWorker.onmessage = (event) => {
      const result = event.data || {};
      const done = faceRequests.get(result.id);
      if (!done) return;
      faceRequests.delete(result.id);
      done(result.type === "result"
        ? { available: true, count: result.count, confidence: result.confidence }
        : { ...FACE_SAMPLE_UNAVAILABLE, reason: result.reason });
    };
    // Fires when the worker script itself fails to load or throws at top level,
    // which is exactly when no reply is ever coming.
    faceWorker.onerror = () => abandonFaceWorker("face worker failed to start");
    return faceWorker;
  };

  const detectFaceInClassicWorker = (frame) =>
    new Promise((resolve) => {
      const worker = faceWorkerHandle();
      if (!worker) {
        closeFrame(frame);
        resolve(FACE_SAMPLE_UNAVAILABLE);
        return;
      }
      const id = ++faceRequestId;
      faceRequests.set(id, resolve);
      try {
        worker.postMessage({ type: "detect", id, frame }, [frame]);
      } catch {
        // Nothing was transferred, so the frame is still ours to close. Resolved
        // rather than rejected: an unreadable frame is the detector failing to
        // look, which the caller already knows how to report.
        faceRequests.delete(id);
        closeFrame(frame);
        resolve(FACE_SAMPLE_UNAVAILABLE);
      }
    });

  self.addEventListener("message", (event) => {
    const message = event.data || {};
    if (message.type !== "frame") return;
    void (async () => {
      const outputs = applyFrame(state, message);
      // The detector is slower than the sampler, by seconds on a cold start.
      // Skipping the frame beats queueing it: it would be stale by the time
      // anything looked at it, and the sample rate should follow the detector
      // rather than pile work up behind it.
      const detectDue = message.source === "camera"
        && message.frame
        && !faceDetectionInFlight
        && outputs.some((output) => output.analyses.includes("face_detect"));
      if (detectDue) {
        // A detector that throws rather than resolving is still a detector that
        // could not look. Without this the whole handler rejects: no
        // FACE_DETECTOR_UNAVAILABLE, no analysis message, and face monitoring
        // stops for the rest of the interview with nothing said about it.
        faceDetectionInFlight = true;
        const sample = await detectFace(message.frame)
          .catch(() => FACE_SAMPLE_UNAVAILABLE)
          .finally(() => {
            faceDetectionInFlight = false;
          });
        message.frame = null;
        if (!sample.available) {
          // Said once, not once per frame, and never fed to the tracker: a
          // detector that failed to load is a fact about the browser, not about
          // whether anyone is sitting in front of it.
          if (!faceDetectorReportedDown) {
            faceDetectorReportedDown = true;
            faceTracker.reset();
            lastFaceSampleAt = null;
            self.postMessage({
              type: "integrity-event",
              eventType: "FACE_DETECTOR_UNAVAILABLE",
              source: "camera",
              severity: "warning",
              durationMs: 0,
              detail: sample.reason
                ? `face detection unavailable: ${sample.reason}`
                : "face detection unavailable",
            });
          }
        } else {
          faceDetectorReportedDown = false;
          // Frames are detected concurrently, so a slow one can land after a
          // newer one already has. A timestamp that moves backwards would give
          // the tracker negative durations and anomalies out of nothing, so the
          // stale result is dropped rather than applied.
          const fresh = lastFaceSampleAt === null || message.at > lastFaceSampleAt;
          if (fresh) {
            if (lastFaceSampleAt !== null && message.at - lastFaceSampleAt > FACE_SAMPLE_GAP_MS) {
              faceTracker.reset();
            }
            lastFaceSampleAt = message.at;
          }
          const faceEvent = fresh ? faceTracker.update(sample, message.at) : null;
          if (faceEvent) {
            self.postMessage({
              type: "integrity-event",
              eventType: faceEvent.type,
              source: "camera",
              severity: faceEventSeverity(faceEvent.type),
              // Carried as a field, not only inside `detail`: the agent and the
              // report read the structured value, and dropping it here is what
              // made every face event arrive with durationMs 0.
              durationMs: faceEvent.durationMs,
              // Confidence is omitted rather than reported as zero when the
              // detector does not expose it. The vendored MediaPipe build keeps
              // the score under two minified names, and `faces=1;confidence=0.00`
              // in an integrity record reads as a detection nobody believed.
              detail: faceEvent.sample.confidence > 0
                ? `faces=${faceEvent.sample.count};confidence=${faceEvent.sample.confidence.toFixed(2)}`
                : `faces=${faceEvent.sample.count}`,
            });
          }
        }
      }
      closeFrame(message.frame);
      for (const output of outputs) {
        self.postMessage({ type: "analysis", ...output, detail: analysisDetail(output) });
      }
    })();
  });
}
