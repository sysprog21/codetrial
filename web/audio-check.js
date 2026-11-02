// Pre-flight media gate.
//
// A voice interview is worthless if the candidate cannot hear the interviewer
// or the interviewer cannot hear them, and both failures are silent: browsers
// suspend audio output until a user gesture, and a muted or missing microphone
// still yields a live track that carries nothing. A camera additionally proves
// face presence unless the unrecorded candidate explicitly continues without
// one.
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
  cameraSkipped = false,
  faceReady = true,
  faceError = null,
} = {}) {
  const steps = {
    output: Boolean(outputConfirmed),
    mic: !micError && micPeak >= MIC_SILENT_PEAK,
    camera:
      cameraSkipped ||
      (!cameraError &&
        !faceError &&
        Boolean(cameraReady) &&
        Boolean(faceReady)),
  };

  if (!browserSupported) {
    return {
      steps,
      ready: false,
      blocker: "browser",
      message:
        "This browser cannot start the required camera and microphone checks.",
    };
  }
  if (micError) {
    return {
      steps,
      ready: false,
      blocker: "mic-error",
      message: `Microphone unavailable: ${micError}. ${micErrorAdvice(micError)}`,
    };
  }
  if (cameraError && !cameraSkipped) {
    return {
      steps,
      ready: false,
      blocker: "camera-error",
      message: `Camera unavailable: ${cameraError}. ${cameraErrorAdvice(cameraError)}`,
    };
  }
  if (faceError && !cameraSkipped) {
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
      message:
        "No sound is reaching the microphone yet. Say something to test it.",
    };
  }
  if (!steps.camera) {
    return {
      steps,
      ready: false,
      blocker: "camera",
      message:
        "Camera video or face presence is not ready yet. Keep one person in frame.",
    };
  }
  if (!steps.output) {
    return {
      steps,
      ready: false,
      blocker: "output",
      message:
        'Play the test tone or voice to check your audio, or click "I heard it" if it already works.',
    };
  }
  return {
    steps,
    ready: true,
    blocker: null,
    message: "Media checks passed. You can start the interview.",
  };
}

/// Sorts a camera or microphone error into what the candidate can do about it,
/// so the advice here and the reason `interview.js` records for a skipped
/// camera cannot disagree about the same error.
///
/// A missing device and a refused permission are checked first, so an error
/// that names either is never reported as a busy device. `busy` is a device
/// the browser holds a grant for but cannot start, which is not a permission
/// problem. On Windows a camera is exclusive, so a meeting or recording app
/// with its video on (Zoom, OBS, Teams) makes Chromium fail with
/// `NotReadableError` ("Could not start video source", "Device in use", or
/// `TrackStartError` in older builds) and Firefox with `AbortError: Starting
/// video failed`. A microphone is shared by default and is only busy while an
/// app holds it in exclusive mode, which Chromium reports with the same
/// `NotReadableError: Device in use`, or "Could not start audio source". The
/// pool's own "no active video track" is the same situation seen later: the
/// grant stands but the track stopped, for example behind a privacy shutter or
/// with the device unplugged. "no active audio track" is narrower, a grant that
/// came back with no audio track at all; an unplugged microphone ends its
/// track, and asking again fails as not found. Anything else is `other`.
const NO_DEVICE = /not ?found|no camera/i;
const DENIED_DEVICE = /permission|denied|not ?allowed/i;
const BUSY_DEVICE =
  /NotReadableError|TrackStartError|AbortError|could not start (?:audio|video) source|starting video failed|device in use|no active (?:audio|video) track/i;

export function deviceErrorKind(error) {
  const words = String(error || "");
  if (NO_DEVICE.test(words)) return "no_device";
  if (DENIED_DEVICE.test(words)) return "denied";
  if (BUSY_DEVICE.test(words)) return "busy";
  return "other";
}

/// The microphone counterpart of the camera advice below. It has no
/// presentation route to offer: the microphone stays with CodeTrial for the
/// whole interview, so the other app has to let go of it before Start.
function micErrorAdvice(micError) {
  if (deviceErrorKind(micError) === "busy") {
    return "The microphone is allowed but will not start: another app may be holding it for itself, for example a recording or call app using exclusive mode. Free it and this check picks it up again by itself.";
  }
  return "Grant access, then test again.";
}

/// "Grant access" sends a busy camera to settings that are already right. The
/// pool retries a failed device every RETRY_MS, so the check recovers on its
/// own once the camera is free; there is no button to press. Ticking the
/// presentation box hands the camera back to the other app after Start, but
/// it also stops the camera being watched and recorded, so the advice says so.
function cameraErrorAdvice(cameraError) {
  if (deviceErrorKind(cameraError) === "busy") {
    return 'The camera is allowed but will not start: another app (for example Zoom, OBS or Teams) may be using it, or it is covered or unplugged. Free it and this check picks it up again by itself. If that app needs the camera during the interview, pass this check first, then tick "I will present this interview in Google Meet", start the interview, and turn that app\'s video back on; CodeTrial then stops watching and recording the camera.';
  }
  return "Grant access, then test again.";
}

export function videoTrackReady(track) {
  return (
    Boolean(track) &&
    track.readyState === "live" &&
    track.enabled !== false &&
    track.muted !== true
  );
}

/// Whether the browser will actually emit sound. A context stays suspended
/// until a user gesture, which is exactly the failure the old post-hoc banner
/// reported too late.
export function outputUsable(audioContextState) {
  return audioContextState === "running";
}

/// Every signal the media gate reads, in one call.
///
/// A function rather than eight reads at each call site, so a new signal is
/// added here and both callers get it. The camera's error is the one thing
/// written on the way through: it is the only signal that can only be judged
/// against a track the candidate already granted.
///
/// `micPeak` arrives as a function rather than a value because the drop below
/// changes it: dropping an ended microphone fires the meter's `onLost`, which
/// forgets the peak it had proven. A peak read at the call site is read before
/// that, so an unplugged microphone would answer this sample with the level
/// the device that went away once reached -- and the sample that matters most
/// is the one `finish` takes when the candidate clicks Start.
export function preflightReadiness({
  pool,
  browserSupported,
  outputConfirmed,
  micPeak,
  faceCheck,
  cameraSkipped = false,
}) {
  // Keyed on the track, not on the pool's stream. The stream exists from the
  // moment the preflight asks for a device, so testing it here would overwrite
  // the reason the request actually failed -- the permission the candidate
  // denied -- with "no active video track" on every frame, and paint the panel
  // red while the prompt is still on screen.
  const camera = pool.trackOf("video");
  if (camera)
    pool.setError(
      "video",
      videoTrackReady(camera) ? null : "no active video track",
    );
  // An ended track never revives, and while it sits in the stream the retry
  // sees a device of that kind and asks for nothing. The gate has no bypass,
  // so an unplugged device would strand the candidate. A muted track can come
  // back on its own, so only the ended one is dropped.
  //
  // Both kinds, one rule. The camera is dropped here rather than left to the
  // retry tick because its liveness is judged per frame just above; the
  // microphone has no such judgement, because a meter over a device that went
  // away reports silence rather than an error. `dropTrack` fires `onLost`, so
  // whatever was running over the track is torn down with it.
  for (const kind of ["audio", "video"]) {
    const track = pool.trackOf(kind);
    if (track?.readyState !== "ended") continue;
    pool.dropTrack(track);
    pool.retry();
  }
  return mediaReadiness({
    browserSupported,
    outputConfirmed,
    micPeak: micPeak(),
    micError: pool.errorOf("audio"),
    cameraReady: videoTrackReady(pool.trackOf("video")),
    cameraError: pool.errorOf("video"),
    faceReady: faceCheck.ready,
    faceError: faceCheck.error,
    cameraSkipped,
  });
}
