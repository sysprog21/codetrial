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

// Named rather than left to the browser's defaults. The interviewer interrupts
// itself on the candidate's voice activity, so a laptop speaker feeding the
// microphone is heard as the candidate starting to talk and the reply is
// abandoned mid-sentence. Every engine enables echo cancellation for a bare
// `audio: true` today, which is exactly why asking for it costs nothing and
// stops a future default from moving under us.
const CONSTRAINTS = {
  audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
  video: true,
};

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

  const device = (kind) => ({
    kind,
    constraints: { [kind]: CONSTRAINTS[kind] },
    pending: false,
    error: null,
    accept: () => true,
    onTrack: () => {},
    onLost: () => {},
  });
  const devices = { audio: device("audio"), video: device("video") };

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
    // the candidate is stranded with no way back.
    //
    // `onLost` is what makes it safe for the pool to do this rather than the
    // preflight: whatever the caller was running over that track -- a face
    // check, a level meter -- has to be torn down in the same step, or the
    // replacement arrives and nothing notices it is a different device.
    const held = trackOf(device.kind);
    if (held?.readyState === "ended") {
      stream.removeTrack(held);
      device.onLost();
    }

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
    /// `accept` decides whether a granted track counts, `onTrack` runs once one
    /// does, and `onLost` runs when the pool drops one that ended. Set before
    /// `start`, because the first grant can arrive inside it. Merged rather than
    /// assigned, so naming two of the three does not blank the third.
    configure(kind, overrides) {
      Object.assign(devices[kind], overrides);
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
    /// Fires `onLost` like the pool's own drop does, so a caller that notices an
    /// ended track first does not have to pair the removal with the teardown by
    /// hand. That pairing is what left the face check reset at two sites.
    dropTrack(track) {
      if (!track) return;
      stream?.removeTrack(track);
      devices[track.kind]?.onLost();
    },
    cancelRetry() {
      unschedule(retryTimer);
      retryTimer = null;
    },
  };
}
