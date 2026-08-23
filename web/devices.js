// Asking the browser for a microphone and a camera, and keeping asking.
//
// One record per device rather than a `let` per signal. The lifecycle is the
// same for both -- ask, hold a track, say why not, retry -- and the bugs here
// came from writing it twice: two retry timers that did not know about each
// other, and a "was a request made" flag doing duty as "is there a working
// device". What differs per device is `accept` and `onTrack`, so those are what
// the caller passes.
//
// Lifted out of the preflight because it was the largest thing in a closure
// nothing could reach, and a device pool that cannot be driven by a test is one
// whose retry behavior is only ever checked by hand.

const RETRY_MS = 2000;

// `schedule` is injected so a test can fire the retry rather than wait for it.
// The bug this guards -- two timers where there should be one -- only shows up
// as a count of requests, and a count read off a sleep is a count read off how
// busy the machine was.
export function createDevicePool({
  mediaDevices,
  isFinished,
  onChange,
  retryMs = RETRY_MS,
  schedule = setTimeout,
  unschedule = clearTimeout,
}) {
  const supported = Boolean(mediaDevices?.getUserMedia);
  let stream = null;
  let retryTimer = null;

  const devices = {
    audio: { kind: "audio", constraints: { audio: true }, pending: false, error: null, accept: () => true, onTrack: () => {} },
    video: { kind: "video", constraints: { video: true }, pending: false, error: null, accept: () => true, onTrack: () => {} },
  };

  // `stream` stays the single owner of the tracks, because it is what the
  // candidate joins the room with. The records describe the request, not the
  // result, so nothing can disagree with the stream about what is live.
  const trackOf = (kind) => stream?.getTracks().find((each) => each.kind === kind) ?? null;

  // One timer for both devices, not one each. Each device used to schedule its
  // own retry and both called back here, so denying both prompts doubled the
  // number of in-flight requests every two seconds until the tab died.
  const retry = () => {
    if (isFinished()) return;
    retryTimer ??= schedule(() => {
      retryTimer = null;
      start();
    }, retryMs);
  };

  // A track that resolves after the preflight has no owner: the stream the
  // candidate joins with is already handed over, so a late one would only turn
  // a device back on behind their back. Anything unclaimed is stopped.
  const claimTrack = (granted, kind) => {
    const track = isFinished() ? null : granted.getTracks().find((each) => each.kind === kind);
    for (const spare of granted.getTracks()) if (spare !== track) spare.stop();
    return track;
  };

  // `pending` matters as much as the shared timer: a retry must not open a
  // second prompt while the first is still on screen.
  const requestDevice = (device) => {
    // An ended track never revives, and while it sits in the stream it looks
    // like a device this pool already holds, so the retry asks for nothing and
    // the candidate is stranded with no way back. The preflight does this for
    // the camera, because it has its own reason to notice one going away and
    // reset the face check; nobody was doing it for the microphone.
    const held = trackOf(device.kind);
    if (held?.readyState === "ended") stream.removeTrack(held);

    if (device.pending || trackOf(device.kind)) return;
    device.pending = true;
    void mediaDevices.getUserMedia(device.constraints).then((granted) => {
      const track = claimTrack(granted, device.kind);
      if (!device.accept(track)) {
        track?.stop();
        device.error = `no active ${device.kind} track`;
        retry();
        return;
      }
      device.error = null;
      stream.addTrack(track);
      device.onTrack();
    }).catch((error) => {
      device.error = String(error?.message || error);
      retry();
    }).finally(() => {
      device.pending = false;
      onChange();
    });
  };

  const start = () => {
    if (!supported) {
      for (const device of Object.values(devices)) device.error = "getUserMedia is not supported";
      onChange();
      return;
    }
    stream ||= new MediaStream();
    for (const device of Object.values(devices)) requestDevice(device);
  };

  return {
    supported,
    get stream() {
      return stream;
    },
    /// `accept` decides whether a granted track counts, and `onTrack` runs once
    /// one does. Set before `start`, because the first grant can arrive inside it.
    configure(kind, { accept, onTrack }) {
      Object.assign(devices[kind], { accept, onTrack });
    },
    start,
    retry,
    trackOf,
    errorOf: (kind) => devices[kind].error,
    setError(kind, error) {
      devices[kind].error = error;
    },
    // An ended track never revives, and while it sits in the stream the retry
    // sees a track of that kind and asks for nothing.
    dropTrack(track) {
      stream?.removeTrack(track);
    },
    cancelRetry() {
      unschedule(retryTimer);
      retryTimer = null;
    },
  };
}
