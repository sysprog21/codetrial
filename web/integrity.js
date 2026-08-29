/// The integrity signal: whether the camera and microphone are still the ones
/// the candidate started with.
///
/// Split out of `interview.js` because it is a set of timers and a worker with
/// one lifetime, and the failure it guards against is silent. A sampler or a
/// heartbeat left running after the room closes is a request every few seconds
/// about an interview that ended; one stopped too early is a gap in the record
/// with nothing saying there is a gap. Both are teardown-order bugs, and this
/// file is where that order is written down.
///
/// `updatePresenceBanner` arrives as a function: the banner state belongs to
/// the page, and importing it back would make the two files circular.

import { TRACKING_INTERVAL_MS } from "./integrity-worker.js";

// One row per presence event, so a new one is a row rather than another branch
// on the same value in the worker message handler. `clears` empties the banner;
// `endsInterview` is the only event allowed to close the session by itself.
//
// Here rather than in the page script because nothing in it touches the DOM or
// the interview: it is the vocabulary of the worker's messages, which is what
// this module is for. The page imports it for the banner text.
export const PRESENCE_EVENTS = {
  FACE_DETECTED: { clears: true },
  FACE_MISSING: { banner: "Stay in front of the camera: no interviewee detected." },
  MULTIPLE_FACES: { banner: "Take the interview alone: more than one face detected." },
  FACE_DETECTOR_UNAVAILABLE: { banner: "Face detection is not running in this browser." },
  FACE_MISSING_SEVERE: {
    banner: "Interview ended automatically: away from the camera too long.",
    endsInterview: true,
  },
};

let state = null;
let nodes = null;
let updatePresenceBanner = () => {};
/// The page owns the LiveKit publish path and the end-of-interview sequence.
/// Both are called from here and neither belongs here.
let publishIntegrityEvent = () => {};
let endInterview = () => {};

export function initIntegrity(deps) {
  ({ state, nodes, updatePresenceBanner, publishIntegrityEvent, endInterview } = deps);
}

const INTEGRITY_HEARTBEAT_MS = 5000;

export function monitorIntegrityTracks() {
  // A released camera is simply not in the stream, so this watches nothing and
  // no CAMERA_STOPPED is invented out of our own teardown.
  watchIntegrityTrack(state.localUserStream?.getVideoTracks?.()[0], "CAMERA_STOPPED", "camera");
  watchIntegrityTrack(state.localUserStream?.getAudioTracks?.()[0], "MICROPHONE_STOPPED", "microphone");
  startIntegrityTrackStateMonitor();
}

export function watchIntegrityTrack(track, stoppedType, source) {
  if (!track?.addEventListener) return;
  track.addEventListener("ended", () => {
    void publishIntegrityEvent({ type: stoppedType, source, severity: "high" });
  });
  track.addEventListener("mute", () => {
    void publishIntegrityEvent({ type: `${source.toUpperCase()}_MUTED`, source, severity: "warning" });
  });
  track.addEventListener("unmute", () => {
    void publishIntegrityEvent({ type: `${source.toUpperCase()}_UNMUTED`, source, severity: "info" });
  });
}

export function startIntegrityHeartbeat() {
  clearInterval(state.integrityHeartbeatTimer);
  state.integrityHeartbeatTimer = setInterval(() => {
    void publishIntegrityEvent({
      type: "INTEGRITY_HEARTBEAT",
      source: "heartbeat",
      severity: "info",
      detail: integrityHeartbeatDetail(),
    });
  }, INTEGRITY_HEARTBEAT_MS);
}

export function integrityHeartbeatDetail() {
  const camera = state.localUserStream?.getVideoTracks?.()[0];
  const mic = state.localUserStream?.getAudioTracks?.()[0];
  const status = (track) => `${track?.readyState || "missing"}/${track?.enabled === false ? "off" : "on"}/${track?.muted ? "muted" : "unmuted"}`;
  return `camera=${status(camera)};microphone=${status(mic)};analyzer=${state.integrityAnalysisDetail}`;
}

export function startIntegrityTrackStateMonitor() {
  clearInterval(state.integrityTrackStateTimer);
  state.integrityTrackStates.clear();
  const tracks = () => [
    ["camera", state.localUserStream?.getVideoTracks?.()[0]],
    ["microphone", state.localUserStream?.getAudioTracks?.()[0]],
  ];
  const poll = () => {
    for (const [source, track] of tracks()) {
      if (!track) continue;
      const current = `${track.readyState || "missing"}/${track.enabled === false ? "off" : "on"}`;
      const previous = state.integrityTrackStates.get(source);
      state.integrityTrackStates.set(source, current);
      if (previous && previous !== current) {
        void publishIntegrityEvent({
          type: `${source.toUpperCase()}_STATE_CHANGED`,
          source,
          severity: current.includes("ended") || current.includes("/off") ? "warning" : "info",
          detail: current,
        });
      }
    }
  };
  poll();
  state.integrityTrackStateTimer = setInterval(poll, 1000);
}

export function startIntegrityWorker() {
  stopIntegrityWorker();
  if (typeof Worker !== "function") {
    state.integrityAnalysisDetail = "worker_unavailable";
    return;
  }
  state.integrityAnalysisDetail = "worker_starting";
  const worker = new Worker("/integrity-worker.js", { type: "module" });
  state.integrityWorker = worker;
  worker.onmessage = (event) => {
    if (event.data?.type === "analysis") state.integrityAnalysisDetail = event.data.detail;
    if (event.data?.type === "integrity-event") {
      const eventType = event.data.eventType;
      updatePresenceBanner(eventType);
      const published = publishIntegrityEvent({
        type: eventType,
        source: event.data.source,
        severity: event.data.severity,
        durationMs: event.data.durationMs,
        detail: event.data.detail,
      });
      // The event that caused the termination goes out before the end control
      // does. The other order lets the agent start generating the report on the
      // control message while the reason for it is still queued, and the report
      // then omits why it exists.
      if (PRESENCE_EVENTS[eventType]?.endsInterview) {
        void published.then(() => endInterview("face_missing_severe"));
      }
    }
  };

  worker.onerror = () => {
    state.integrityAnalysisDetail = "worker_error";
  };
  state.integrityFrameSamplers = [
    startIntegrityFrameSampler("camera", state.localUserStream?.getVideoTracks?.()[0], nodes.cameraIntegrityVideo, TRACKING_INTERVAL_MS),
  ].filter(Boolean);
}

export function stopIntegrityWorker() {
  for (const stop of state.integrityFrameSamplers) stop();
  state.integrityFrameSamplers = [];
  state.integrityWorker?.terminate();
  state.integrityWorker = null;
  state.integrityAnalysisDetail = "not_configured";
}

export function startIntegrityFrameSampler(source, track, video, intervalMs) {
  if (!track || !video || !state.integrityWorker) return null;
  const ownsVideo = !video.srcObject;
  if (ownsVideo) {
    video.muted = true;
    video.playsInline = true;
    video.srcObject = new MediaStream([track]);
    void video.play?.().catch(() => {});
  }
  const timer = setInterval(() => {
    void postIntegrityFrame(source, track, video);
  }, intervalMs);
  void postIntegrityFrame(source, track, video);
  return () => {
    clearInterval(timer);
    if (ownsVideo) video.srcObject = null;
  };
}

export async function postIntegrityFrame(source, track, video) {
  // `muted` as well as `readyState`. An ended track stops the sampler already,
  // but a track another application has taken stays `live` and simply delivers
  // nothing: the element keeps handing over its last frame, or a black one, and
  // the detector charges every one of them as an absent candidate until the
  // 15s severe threshold ends the interview. Opening the camera in Google Meet
  // does exactly that. A camera that was taken away is a gap in sampling, and
  // face-presence.js is explicit that a gap is not an observation. The `mute`
  // listener still publishes CAMERA_MUTED, so the evidence survives for human
  // review; it just no longer masquerades as the candidate walking off.
  //
  // The analyzer detail has to say so. The heartbeat publishes it every five
  // seconds, and left at its last successful value it would keep reporting
  // camera analyses while nothing has been sampled for minutes -- an integrity
  // record claiming an observation that never happened.
  if (track.muted) state.integrityAnalysisDetail = `${source}_muted`;
  if (track.readyState !== "live" || track.muted || !state.integrityWorker) return;
  const worker = state.integrityWorker;
  const message = {
    type: "frame",
    source,
    // Monotonic, not wall clock. A lid close, an OS suspend or an NTP step
    // moves `Date.now()` forward by minutes between two frames, and the face
    // tracker reads the difference as "absent for minutes" and ends the
    // interview on the first frame after resume.
    at: performance.now(),
    width: video.videoWidth || 0,
    height: video.videoHeight || 0,
    transport: "metadata",
  };
  if (video.readyState < 2) {
    worker.postMessage(message);
    return;
  }
  // `ImageBitmap`, not the cheaper `VideoFrame` handle. The only consumer of
  // these pixels is MediaPipe, which reads `image.width`: a `VideoFrame` carries
  // `codedWidth`/`displayWidth` and no `width`, so it reached the detector as an
  // undefined size and threw, and every interview reported face detection as
  // unavailable. A transport the consumer cannot read is not an optimisation.
  // `createImageBitmap` decodes off the main thread, so this costs the editor
  // nothing.
  let frame = null;
  try {
    if (typeof createImageBitmap === "function") {
      frame = await createImageBitmap(video);
      message.frame = frame;
      message.transport = "ImageBitmap";
      worker.postMessage(message, [frame]);
      return;
    }
  } catch {
    frame?.close?.();
    delete message.frame;
    message.transport = "metadata";
  }
  worker.postMessage(message);
}
