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
  audio: {
    echoCancellation: true,
    noiseSuppression: true,
    autoGainControl: true,
  },
  video: true,
};

/// The microphone constraints with a device named, exactly: a switch the
/// candidate asked for that opened some other input and called it theirs would
/// be worse than one that said it failed. No id asks for the default.
function microphoneConstraints(deviceId) {
  if (!deviceId) return CONSTRAINTS.audio;
  return { ...CONSTRAINTS.audio, deviceId: { exact: deviceId } };
}

// The only capture call in the first-party tree, and the getUserMedia allowlist
// in meet-tab-audio.test.js holds it to that. The preflight pool and a
// mid-interview microphone switch both ask through here, so a switch is a
// second device through the same door rather than a second door.
function capture(mediaDevices, constraints) {
  return mediaDevices.getUserMedia(constraints);
}

/// One live microphone track for an interview already under way, where there
/// is no pool to retry and no preflight to explain a failure: the caller keeps
/// the input it has and says why. Anything else the browser hands back is
/// stopped, so a refusal leaves no device open behind the candidate's back.
export async function openMicrophone(mediaDevices, deviceId) {
  const granted = await capture(mediaDevices, {
    audio: microphoneConstraints(deviceId),
  });
  const tracks = granted.getTracks();
  const track = tracks.find(
    (each) => each.kind === "audio" && each.readyState !== "ended",
  );
  for (const spare of tracks) if (spare !== track) spare.stop();
  if (!track) throw new Error("no active audio track");
  return track;
}

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
    generation: 0,
    disabled: false,
    error: null,
    accept: () => true,
    onTrack: () => {},
    onLost: () => {},
  });
  const devices = { audio: device("audio"), video: device("video") };

  // `stream` stays the single owner of the tracks, because it is what the
  // candidate joins the room with. The records describe the request, not the
  // result, so nothing can disagree with the stream about what is live.
  const trackOf = (kind) =>
    stream?.getTracks().find((each) => each.kind === kind) ?? null;

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
    const track = isFinished()
      ? null
      : granted.getTracks().find((each) => each.kind === kind);
    for (const spare of granted.getTracks()) if (spare !== track) spare.stop();
    return track;
  };

  // `pending` matters as much as the shared timer: a retry must not open a
  // second prompt while the first is still on screen.
  const requestDevice = (device) => {
    if (isFinished()) return;
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

    if (device.disabled || device.pending || trackOf(device.kind)) return;
    device.pending = true;
    const generation = device.generation;
    void capture(mediaDevices, device.constraints)
      .then((granted) => {
        if (
          device.disabled ||
          isFinished() ||
          generation !== device.generation
        ) {
          granted.getTracks().forEach((track) => track.stop());
          return;
        }
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
      })
      .catch((error) => {
        if (isFinished() || generation !== device.generation) return;
        // Some browsers leave a DOMException's message empty and name the
        // failure instead. Keep both: the preflight words are the candidate's
        // only explanation, and the optional-camera path distinguishes denied
        // permission from no device.
        device.error =
          [error?.name, error?.message].filter(Boolean).join(": ") ||
          String(error);
        retry();
      })
      .finally(() => {
        device.pending = false;
        // A selection changed while the browser was answering the old request.
        // Reuse the pending guard so repeated changes never open more prompts.
        if (generation !== device.generation) requestDevice(device);
        onChange();
      });
  };

  const start = () => {
    if (!supported) {
      for (const device of Object.values(devices))
        device.error = "getUserMedia is not supported";
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
    selectMicrophone(deviceId) {
      const audio = devices.audio;
      if (
        isFinished() ||
        (audio.constraints.audio.deviceId?.exact || "") === deviceId
      )
        return;
      audio.generation += 1;
      audio.constraints = { audio: microphoneConstraints(deviceId) };
      audio.error = null;
      const held = trackOf("audio");
      if (held) {
        stream.removeTrack(held);
        held.stop();
      }
      audio.onLost();
      start();
      onChange();
    },
    disable(kind) {
      const device = devices[kind];
      device.disabled = true;
      const held = trackOf(kind);
      if (!held) return;
      stream.removeTrack(held);
      held.stop();
      device.onLost();
    },
    // Goes through `start` so the request meets the same `pending` guard and
    // retry timer as any other, rather than opening a second prompt. The error
    // from the attempt that led to the skip is dropped, or its banner would
    // sit beside the new prompt; a request that fails sets it again.
    enable(kind) {
      devices[kind].disabled = false;
      devices[kind].error = null;
      start();
    },
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
