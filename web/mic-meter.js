/// The microphone level meter, and the loudest sustained peak it has proven.
///
/// Its own module for the reason `devices.js` and `face-check.js` are theirs:
/// the generation counter, the retry timer and the rolling window of peaks are
/// one piece of state in three parts, and the invariant that binds them -- a
/// departed microphone proves nothing about the one replacing it -- is worth
/// testing directly rather than reading off the preflight that uses it.
///
/// Nothing here touches the page. The level goes out through `onLevel` and the
/// caller paints the bar, so this can run against a fake meter in a test.

import { MIC_CONFIRM_FRAMES, peakLevel, sustainedPeak } from "./audio-check.js";

/// The default `startMeter`: a real `AudioContext` over a real stream.
///
/// Injected rather than called directly, the way `createFaceCheck` takes its
/// detector, because a test cannot build one of these and the thing worth
/// testing is not the analyser.
export async function startMediaMeter(stream, onPeak, onError, shouldStop) {
  try {
    const context = new (window.AudioContext || window.webkitAudioContext)();
    const analyser = context.createAnalyser();
    analyser.fftSize = 1024;
    context.createMediaStreamSource(stream).connect(analyser);
    const samples = new Uint8Array(analyser.frequencyBinCount);
    const tick = () => {
      if (shouldStop()) {
        void context.close().catch(() => {});
        return;
      }
      analyser.getByteTimeDomainData(samples);
      onPeak(peakLevel(samples));
      requestAnimationFrame(tick);
    };
    tick();
  } catch (error) {
    onError(String(error?.message || error));
  }
}

/// One meter at a time. The pool can hand back a replacement microphone, and
/// both ways back in -- a fresh track and the meter's own retry -- would
/// otherwise leave the previous AudioContext reading the track that went away
/// and reporting levels from it every frame.
export function createMicMeter({
  pool,
  isFinished,
  onLevel,
  onFailure,
  startMeter = startMediaMeter,
  retryMs = 2000,
}) {
  let peak = 0;
  let generation = 0;
  let retry = null;
  const recent = [];

  /// Nothing a departed microphone proved carries over to its replacement.
  const forgetPeak = () => {
    peak = 0;
    recent.length = 0;
  };

  const watch = () => {
    const mine = ++generation;
    return startMeter(
      pool.stream,
      (level) => {
        pool.setError("audio", null);
        recent.push(level);
        if (recent.length > MIC_CONFIRM_FRAMES) recent.shift();
        // Latched once proven: the candidate should not have to keep talking
        // to hold the Start button open while they read the screen.
        peak = Math.max(peak, sustainedPeak(recent));
        onLevel(level);
      },
      (error) => {
        pool.setError("audio", error);
        forgetPeak();
        onFailure(error);
        // Not if this meter was forgotten on the way through. `onFailure`
        // repaints, the repaint drops an ended track, and dropping it forgets
        // this meter -- all before the line below runs, so a retry armed here
        // is one `forget` already cleared and cannot clear again. It would
        // start a meter over a stream the pool has let go of.
        if (mine !== generation || isFinished()) return;
        // The gate has no bypass, so it must recover on its own once the
        // candidate grants access or plugs a device back in. Distinct from the
        // pool's retry: this one restarts the level meter over a track we
        // already hold, that one asks the browser for a device again.
        retry = setTimeout(watch, retryMs);
      },
      () => isFinished() || mine !== generation,
    );
  };

  return {
    peak: () => peak,
    start: watch,
    /// The track is gone. The generation bump is what stops the meter still
    /// running over it from reporting another level.
    forget() {
      generation += 1;
      forgetPeak();
      clearTimeout(retry);
    },
    stop() {
      clearTimeout(retry);
    },
  };
}
