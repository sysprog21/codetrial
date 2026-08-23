// Watching the preflight camera for a face, and saying whether one is there.
//
// One record, because a reset has to clear all of it. `generation` is what
// makes a detect() still in flight for a replaced camera harmless: it compares
// against the record's current value and finds itself stale. `running` and
// `generation` were separate flags for the same fact, and a reset that forgot
// either of them left the panel judging a dead camera.
//
// Lifted out of the preflight for the same reason the device pool was: every
// bug this has had was a lifecycle bug, and a lifecycle nothing can drive is
// one nothing can test.

const CHECK_MS = 1000;

export function createFaceCheck({
  video,
  trackOf,
  isReady,
  isFinished,
  createDetector,
  verdictOf,
  onVerdict,
  checkMs = CHECK_MS,
}) {
  let state = { running: false, generation: 0, detector: null, ready: true, error: null };

  // Replaces the record rather than clearing five fields, so a field added
  // later cannot be left behind by a reset that predates it.
  const reset = () => {
    void state.detector?.close?.();
    video.srcObject = null;
    state = { running: false, generation: state.generation + 1, detector: null, ready: true, error: null };
  };

  const start = async () => {
    if (state.running || !isReady()) return;
    state.running = true;
    const generation = ++state.generation;
    // Every await below is a chance for the camera to be replaced underneath
    // this run, so each resumption re-checks that it is still the current one.
    const stale = () => generation !== state.generation;
    video.muted = true;
    video.playsInline = true;
    video.srcObject = new MediaStream([trackOf()]);
    await video.play?.().catch(() => {});
    const detector = await createDetector();
    if (stale()) {
      void detector.close?.();
      return;
    }
    state.detector = detector;
    // No detector means no verdict to wait for, not a failed one.
    state.ready = !detector.available;
    onVerdict();
    if (!detector.available) return;
    const check = async () => {
      if (isFinished() || stale() || !detector.available) return;
      const verdict = verdictOf(await detector.detect(video));
      if (stale()) return;
      ({ ready: state.ready, error: state.error } = verdict);
      onVerdict();
      setTimeout(check, checkMs);
    };
    void check();
  };

  return {
    start,
    reset,
    close: () => void state.detector?.close?.(),
    get ready() {
      return state.ready;
    },
    get error() {
      return state.error;
    },
  };
}
