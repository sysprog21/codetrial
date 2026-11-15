import { peakLevel } from "./audio-check.js";
/// Browser-observed gap between Jim entering "speaking" and the first
/// detectable remote audio activity. This is not end-to-end latency.
export function createAudioResponseTiming({
  now = () => performance.now(),
  report = () => {},
} = {}) {
  let speaking = false;
  let startedAt = null;

  function speakingChanged(nextSpeaking) {
    if (nextSpeaking === speaking) return;

    speaking = nextSpeaking;

    if (speaking) {
      startedAt = now();
    } else {
      startedAt = null;
    }
  }

  function audioActivity() {
    if (!speaking || startedAt === null) return;

    const elapsedMs = now() - startedAt;
    startedAt = null;

    report(elapsedMs);
  }

  function cancelPending() {
    startedAt = null;
  }

  function reset() {
    speaking = false;
    cancelPending();
  }

  return {
    speakingChanged,
    audioActivity,
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
    createContext = () => new AudioContext(),
    createStream = (mediaTrack) => new MediaStream([mediaTrack]),
    schedule = requestAnimationFrame,
    cancel = cancelAnimationFrame,
    gestureTarget = globalThis.document,
    visibilityTarget = globalThis.document,
  } = {},
) {
  const context = createContext();
  let source;
  let analyser;

  try {
    if (context.state === "suspended") {
      void context.resume?.().catch(() => {});
    }

    source = context.createMediaStreamSource(createStream(track));
    analyser = context.createAnalyser();

    analyser.fftSize = 1024;
    source.connect(analyser);
  } catch (error) {
    source?.disconnect();
    void context.close?.().catch(() => {});
    throw error;
  }

  const samples = new Uint8Array(analyser.fftSize);

  let stopped = false;
  let frame = null;
  let unavailable = null;

  function notifyAvailability(available) {
    const nextUnavailable = !available;

    if (unavailable === nextUnavailable) return;

    unavailable = nextUnavailable;

    if (available) onAvailable();
    else onUnavailable();
  }

  function visibilityChanged() {
    if (stopped) return;

    if (visibilityTarget?.hidden) {
      notifyAvailability(false);
    }
  }

  visibilityTarget?.addEventListener("visibilitychange", visibilityChanged);

  function resumeOnGesture() {
    if (stopped || context.state !== "suspended") return;
    void context.resume?.().catch(() => {});
  }

  gestureTarget?.addEventListener("pointerdown", resumeOnGesture);
  gestureTarget?.addEventListener("keydown", resumeOnGesture);

  function tick() {
    if (stopped) return;

    const available =
      context.state === "running" &&
      track.readyState === "live" &&
      !visibilityTarget?.hidden;

    notifyAvailability(available);

    if (available) {
      analyser.getByteTimeDomainData(samples);

      if (peakLevel(samples) > threshold) {
        onActivity();
      }
    }

    frame = schedule(tick);
  }
  frame = schedule(tick);

  return () => {
    if (stopped) return;

    stopped = true;

    gestureTarget?.removeEventListener("pointerdown", resumeOnGesture);
    gestureTarget?.removeEventListener("keydown", resumeOnGesture);

    visibilityTarget?.removeEventListener(
      "visibilitychange",
      visibilityChanged,
    );

    cancel(frame);
    source.disconnect();

    void context.close().catch(() => {});
  };
}
