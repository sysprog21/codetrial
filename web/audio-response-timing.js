import { startMediaMeter } from "./mic-meter.js";
/// Browser-observed gap between Jim entering "speaking" and the first
/// detectable remote audio activity. This is not end-to-end latency.
export function createAudioResponseTiming({
  now = () => performance.now(),
  report = () => {},
  diagnose = () => {},
} = {}) {
  let speaking = false;
  let startedAt = null;
  let active = false;
  let armed = false;

  function speakingChanged(nextSpeaking, unavailableReason = null) {
    if (nextSpeaking === speaking) return;

    speaking = nextSpeaking;

    if (speaking) {
      if (unavailableReason || active) {
        diagnose(unavailableReason || "audio already active");
      } else {
        startedAt = now();
        armed = false;
      }
    } else {
      cancelPending("speaking stopped before audio");
    }
  }

  function audioActivity() {
    active = true;
    if (!speaking || startedAt === null) return;
    if (!armed) {
      cancelPending("audio already active before a quiet frame");
      return;
    }

    const elapsedMs = now() - startedAt;
    startedAt = null;

    report(elapsedMs);
  }

  function audioQuiet() {
    active = false;
    // A pre-transition quiet frame cannot rule out audio arriving first.
    if (startedAt !== null) armed = true;
  }

  function cancelPending(reason = "measurement canceled") {
    if (startedAt !== null) diagnose(reason);
    startedAt = null;
    armed = false;
  }

  function reset(reason = "timing reset") {
    speaking = false;
    active = false;
    cancelPending(reason);
  }

  return {
    speakingChanged,
    audioActivity,
    audioQuiet,
    cancelPending,
    reset,
  };
}
/// Observe remote audio activity without routing it to another output.
/// The returned function releases the analyser and its AudioContext.
export function startAudioActivityMonitor(
  track,
  onActivity,
  {
    threshold = 0.05,
    onAvailable = () => {},
    onUnavailable = () => {},
    onQuiet = () => {},
    createContext = () => new AudioContext(),
    createStream = (mediaTrack) => new MediaStream([mediaTrack]),
    schedule = requestAnimationFrame,
    cancel = cancelAnimationFrame,
    gestureTarget = globalThis.document,
    visibilityTarget = globalThis.document,
  } = {},
) {
  const stream = createStream(track);
  const context = createContext();
  let stopped = false;
  let unavailable;

  function notifyAvailability(reason) {
    if (unavailable === reason) return;
    unavailable = reason;
    if (reason === null) onAvailable();
    else onUnavailable(reason);
  }

  function visibilityChanged() {
    if (stopped) return;

    if (visibilityTarget?.hidden) {
      notifyAvailability("page hidden");
    }
  }

  function resumeOnGesture() {
    if (stopped || context.state !== "suspended") return;
    void context.resume?.().catch(() => {});
  }

  const stopMeter = startMediaMeter(
    stream,
    (peak) => {
      if (peak > threshold) onActivity();
      else onQuiet();
    },
    (message) => {
      throw new Error(message);
    },
    () => stopped,
    {
      createContext: () => context,
      schedule,
      cancel,
      sampleSize: 1024,
      deferFirstFrame: true,
      shouldSample: () => {
        const reason = visibilityTarget?.hidden
          ? "page hidden"
          : context.state !== "running"
            ? `audio context ${context.state}`
            : track.readyState !== "live"
              ? "track ended"
              : track.muted
                ? "track muted"
                : track.enabled === false
                  ? "track disabled"
                  : null;

        notifyAvailability(reason);
        return reason === null;
      },
    },
  );
  try {
    resumeOnGesture();
  } catch (error) {
    stopMeter();
    throw error;
  }
  visibilityTarget?.addEventListener("visibilitychange", visibilityChanged);
  gestureTarget?.addEventListener("pointerdown", resumeOnGesture);
  gestureTarget?.addEventListener("keydown", resumeOnGesture);

  return () => {
    if (stopped) return;

    stopped = true;

    gestureTarget?.removeEventListener("pointerdown", resumeOnGesture);
    gestureTarget?.removeEventListener("keydown", resumeOnGesture);

    visibilityTarget?.removeEventListener(
      "visibilitychange",
      visibilityChanged,
    );

    stopMeter();
  };
}
