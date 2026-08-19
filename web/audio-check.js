// Pre-flight media gate.
//
// A voice interview is worthless if the candidate cannot hear the interviewer
// or the interviewer cannot hear them, and both failures are silent: browsers
// suspend audio output until a user gesture, and a muted or missing microphone
// still yields a live track that carries nothing. Integrity mode also requires
// an active camera before the room starts.
//
// The logic here is deliberately free of Web Audio and DOM types so it can be
// tested under `node --test`; interview.js supplies the real nodes.

/// Below this the microphone is producing silence, not speech. Normalized 0..1
/// against a full-scale signal; ordinary speech peaks far above it, while a
/// muted device sits at or near zero.
export const MIC_SILENT_PEAK = 0.02;
/// Frames of continuous sound before the microphone counts as proven. One frame
/// above the threshold is a chair creak or a door; at ~60fps this is roughly a
/// tenth of a second, which is a syllable.
export const MIC_CONFIRM_FRAMES = 6;

/// The quietest peak across the most recent frames, so a single spike cannot
/// open the gate. Returns 0 until enough frames exist to judge.
export function sustainedPeak(recentPeaks) {
  if (recentPeaks.length < MIC_CONFIRM_FRAMES) return 0;
  return Math.min(...recentPeaks.slice(-MIC_CONFIRM_FRAMES));
}

/// Analyser byte data is centred on 128. Returns the loudest deviation seen,
/// normalized to 0..1.
export function peakLevel(samples) {
  let peak = 0;
  // Indexed rather than for-of: this reads a Uint8Array 60 times a second now
  // that the avatar's mouth depends on it, and the iterator protocol allocates
  // per call and defeats the typed-array fast path.
  for (let index = 0; index < samples.length; index += 1) {
    const deviation = Math.abs(samples[index] - 128) / 128;
    if (deviation > peak) peak = deviation;
  }
  return peak;
}

/// What still blocks joining, given what the checks have observed so far.
/// Output is confirmed by the candidate, because no API can tell us a speaker
/// made a sound they actually heard.
///
/// `steps` reports each direction independently so the panel can tick one off
/// as soon as it passes, rather than staying blank until everything does.
export function mediaReadiness({
  browserSupported = true,
  outputConfirmed = false,
  micPeak = 0,
  micError = null,
  cameraReady = false,
  cameraError = null,
  faceReady = true,
  faceError = null,
} = {}) {
  const steps = {
    output: Boolean(outputConfirmed),
    mic: !micError && micPeak >= MIC_SILENT_PEAK,
    camera: !cameraError && !faceError && Boolean(cameraReady) && Boolean(faceReady),
  };

  if (!browserSupported) {
    return {
      steps,
      ready: false,
      blocker: "browser",
      message: "This browser cannot start the required camera and microphone checks.",
    };
  }
  if (micError) {
    return {
      steps,
      ready: false,
      blocker: "mic-error",
      message: `Microphone unavailable: ${micError}. Grant access, then test again.`,
    };
  }
  if (cameraError) {
    return {
      steps,
      ready: false,
      blocker: "camera-error",
      message: `Camera unavailable: ${cameraError}. Grant access, then test again.`,
    };
  }
  if (faceError) {
    return {
      steps,
      ready: false,
      blocker: "face-error",
      message: `Camera face check failed: ${faceError}. Keep one person in frame.`,
    };
  }
  if (!steps.mic) {
    return {
      steps,
      ready: false,
      blocker: "mic-silent",
      message: "No sound is reaching the microphone yet. Say something to test it.",
    };
  }
  if (!steps.camera) {
    return {
      steps,
      ready: false,
      blocker: "camera",
      message: "Camera video or face presence is not ready yet. Keep one person in frame.",
    };
  }
  if (!steps.output) {
    return {
      steps,
      ready: false,
      blocker: "output",
      message: "Play the test tone and confirm you heard it.",
    };
  }
  return {
    steps,
    ready: true,
    blocker: null,
    message: "Media checks passed. You can start the interview.",
  };
}

export function videoTrackReady(track) {
  return Boolean(track) && track.readyState === "live" && track.enabled !== false && track.muted !== true;
}

/// Whether the browser will actually emit sound. A context stays suspended
/// until a user gesture, which is exactly the failure the old post-hoc banner
/// reported too late.
export function outputUsable(audioContextState) {
  return audioContextState === "running";
}
