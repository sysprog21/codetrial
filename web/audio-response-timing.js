import { peakLevel } from "./audio-check.js";

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
/// Owns the diagnostic monitor for the current room and remote track.
export function createAudioTimingController({
  startMonitor = startAudioActivityMonitor,
  onMonitorError = () => {},
  ...timingOptions
} = {}) {
  const timing = createAudioResponseTiming(timingOptions);
  let monitor = null;

  function release(track = null, reason = "audio monitor released") {
    // A late unsubscribe may refer to a track we already replaced.
    if (track && track !== monitor?.track) return;

    const previous = monitor;
    monitor = null;
    previous?.stop?.();
    timing.cancelPending(reason);
  }

  function attach(track, room) {
    if (monitor?.track === track && monitor.room === room) return;
    release(null, "audio track replaced");

    const mediaTrack = track.mediaStreamTrack;
    if (!mediaTrack) return;

    const current = { track, room, stop: null, ready: false };
    monitor = current;
    try {
      current.stop = startMonitor(
        mediaTrack,
        () => {
          if (monitor === current && current.ready) timing.audioActivity();
        },
        {
          onAvailable() {
            if (monitor === current) current.ready = true;
          },
          onQuiet() {
            if (monitor === current) timing.audioQuiet();
          },
          onUnavailable(reason) {
            if (monitor !== current) return;
            current.ready = false;
            timing.cancelPending(reason);
          },
        },
      );
    } catch (error) {
      release();
      onMonitorError(error);
    }
  }

  return {
    attach,
    release,
    reset: timing.reset,
    disconnect(room, currentRoom) {
      // Report delivery can clear the page's room before disconnect emits.
      if (currentRoom !== room && (currentRoom || monitor?.room !== room))
        return;

      release(null, "room disconnected");
      timing.reset();
    },
    speakingChanged(speaking, connected) {
      timing.speakingChanged(
        speaking,
        !connected
          ? "room disconnected"
          : monitor?.ready
            ? null
            : "monitor not ready",
      );
    },
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
  let source;
  let frame = null;

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

  function stop() {
    if (stopped) return;
    stopped = true;
    if (frame !== null) cancel(frame);
    gestureTarget?.removeEventListener("pointerdown", resumeOnGesture);
    gestureTarget?.removeEventListener("keydown", resumeOnGesture);
    visibilityTarget?.removeEventListener(
      "visibilitychange",
      visibilityChanged,
    );
    source?.disconnect();
    void context.close().catch(() => {});
  }

  try {
    const analyser = context.createAnalyser();
    analyser.fftSize = 1024;
    source = context.createMediaStreamSource(stream);
    source.connect(analyser);
    const samples = new Uint8Array(analyser.fftSize);

    function tick() {
      frame = null;
      if (stopped) return;

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
      if (!stopped && reason === null) {
        analyser.getByteTimeDomainData(samples);
        if (peakLevel(samples) > threshold) onActivity();
        else onQuiet();
      }
      if (!stopped) frame = schedule(tick);
    }

    frame = schedule(tick);
    resumeOnGesture();
    visibilityTarget?.addEventListener("visibilitychange", visibilityChanged);
    gestureTarget?.addEventListener("pointerdown", resumeOnGesture);
    gestureTarget?.addEventListener("keydown", resumeOnGesture);
  } catch (error) {
    stop();
    throw error;
  }

  return stop;
}
