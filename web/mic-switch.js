// Which microphone the interview captures, and moving it to another one
// without leaving the room.
//
// Free of DOM and LiveKit types, like meet-audio.js, so the decisions and the
// order of the swap run under `node --test` instead of being asserted as
// source text. interview.js supplies the stream, the published track, the
// select and the integrity publisher.

export const INPUT_NOTES = {
  refused:
    "That microphone could not be opened. Your previous microphone is still in use.",
  lost: "Your microphone disconnected. The interview moved to the default microphone.",
  none: "Your microphone disconnected and no other microphone could be opened. Plug one in or choose a microphone.",
  stranded:
    "The microphone could not be switched, and Jim cannot hear you. Choose a microphone again.",
};

const ALIAS_IDS = new Set(["default", "communications"]);

/// Inputs worth offering, the same way `outputOptions` decides outputs: a
/// device with no id is the placeholder a browser reports before a grant, and
/// it would select to "" and switch to whatever the default happens to be.
///
/// `current` is the track being sent, so the select shows the input in use
/// rather than the one last asked for. Its device id is the first match and
/// its label the fallback: a track opened without a device id can report an
/// id the list does not carry, and an unmatched select shows its first row,
/// which names a microphone the candidate is not using.
export function inputOptions(devices, current) {
  // Chrome lists a `default` input beside the physical one, and Windows a
  // `communications` one too, all sharing its `groupId`; offered as rows of
  // their own, picking the alias of the input in use reopened it and recorded
  // a switch that changed nothing. The preflight picker in #411 skips the same
  // two ids. Rows are not merged by `groupId` itself, since an audio interface
  // or a webcam can put several real inputs in one group.
  const inputs = devices.filter(
    (device) =>
      device.kind === "audioinput" &&
      device.deviceId &&
      !ALIAS_IDS.has(device.deviceId),
  );
  // A track opened as the default reports the alias's id, which no row carries,
  // so only then does its group pick the physical row, and only a group with a
  // single real input names one.
  const byId = inputs.find((device) => device.deviceId === current?.deviceId);
  const inGroup = current?.groupId
    ? inputs.filter((device) => device.groupId === current.groupId)
    : [];
  const byGroup =
    ALIAS_IDS.has(current?.deviceId) && inGroup.length === 1
      ? inGroup[0]
      : null;
  const byLabel = current?.label
    ? inputs.find((device) => device.label === current.label)
    : null;
  const selected = byId ?? byGroup ?? byLabel ?? null;
  return inputs.map((device, index) => ({
    deviceId: device.deviceId,
    groupId: device.groupId || "",
    label: device.label || `Microphone ${index + 1}`,
    selected: device === selected,
  }));
}

/// The integrity detail for a switch. The label goes last because a device
/// label is the browser's localized text and may itself contain `;` or `=`,
/// which a reader splitting the fields left to right then never reaches.
export function switchDetail(reason, track) {
  return `reason=${reason};microphone=${track?.label || "unnamed"}`;
}

// The publication's own mute flag is a server-side mute: the SFU stops
// forwarding the track, so the agent receives no frames at all, not the
// silence that ends a spoken turn. The Mic button therefore mutes the track
// only, and the flag is raised just around a swap and kept down otherwise.

/// Raises the flag for a muted candidate before LiveKit is handed a new input,
/// since LiveKit enables a track it is handed from that flag.
export async function holdMute(published, enabled) {
  if (!enabled && !published.isMuted) await published.mute();
}

/// Lowers the flag, whether a swap raised it or LiveKit did after a loss, and
/// keeps a muted candidate's input disabled throughout. LiveKit enables the
/// track and announces it in one synchronous step, so the listener puts the
/// button's state back before anything else can run.
export async function liftMute(published, enabled) {
  if (!published.isMuted) return;
  const keep = () => {
    if (published.mediaStreamTrack)
      published.mediaStreamTrack.enabled = enabled();
  };
  published.on?.("unmuted", keep);
  try {
    await published.unmute();
  } finally {
    published.off?.("unmuted", keep);
  }
  keep();
}

const audioTrackOf = (stream) =>
  stream?.getTracks?.().find((track) => track.kind === "audio") ?? null;

const deviceIdOf = (track) => track?.getSettings?.().deviceId ?? null;
const groupIdOf = (track) => track?.getSettings?.().groupId || null;

/// `reason` is "chosen" for a pick from the select and "lost" when the input
/// in use ended, in which case `lost` is the track that ended. Every outcome
/// but "switched" leaves the stream, the published track and the device
/// exactly as they were.
///
/// Serialized like `applyAudioOutput`: a candidate can pick again while the
/// permission prompt for the last pick is still up, and two swaps resolving in
/// either order would leave the stream holding one track while LiveKit sends
/// the other.
export function createMicrophoneSwitch({
  open,
  streamOf,
  publishedTrack,
  isLive,
  micEnabled,
  onSwitched,
  onFailed,
}) {
  let chain = Promise.resolve();
  // The input being handed to the sender. Between the open and the commit it
  // is in neither the page's stream nor reach of the Mic button, so a mute
  // pressed meanwhile has to be applied to it here.
  let inFlight = null;

  const run = async (deviceId, reason, lost, groupId) => {
    const stream = streamOf();
    if (!isLive() || !stream) return "idle";
    // Every await below can outlast the interview or the stream it started
    // with, and whatever it would then show or keep open belongs to neither.
    const current = () => isLive() && streamOf() === stream;
    const held = audioTrackOf(stream);
    if (
      reason === "chosen" &&
      held?.readyState === "live" &&
      (deviceIdOf(held) === deviceId ||
        (ALIAS_IDS.has(deviceIdOf(held)) &&
          groupId &&
          groupIdOf(held) === groupId))
    )
      return "unchanged";
    // A loss queued behind a pick that has since replaced the lost track would
    // otherwise move the candidate off the input they just chose. Only another
    // track means that: an empty stream is a loss whose earlier recovery
    // stranded, and the device that set off this retry may be the way back.
    if (reason === "lost" && held && held !== lost) return "unchanged";
    // Checked before the prompt, not after: replacing a track that was never
    // published sends nothing, and the candidate would be told the switch
    // worked while Jim kept hearing silence.
    if (!publishedTrack()) return "idle";

    // No input is being sent. Dropping the old one from the stream, dead or
    // not, keeps the page from claiming it is in use and lets the next pick,
    // even of the same device, open a fresh input rather than read as
    // unchanged.
    const strand = (error) => {
      // `stop()` fires no `ended`, so an input still working when it is
      // dropped here leaves no trace in the integrity trail unless the caller
      // records one; an input that had already ended recorded its own.
      const droppedLive = held?.readyState === "live";
      if (held) {
        stream.removeTrack(held);
        held.stop();
      }
      onFailed(reason, error, "stranded", droppedLive);
      return "stranded";
    };

    let track;
    try {
      track = await open(deviceId);
      inFlight = track;
    } catch (error) {
      if (!current()) return "idle";
      if (held?.readyState !== "live") return strand(error);
      onFailed(reason, error, "refused");
      return "refused";
    }
    // The prompt can outlast the interview, or the room that published the old
    // track, and a device opened for neither would stay on behind the report.
    const published = publishedTrack();
    if (!current() || !published) {
      track.stop();
      return "idle";
    }

    // Everything that can fail once the sender may hold the new input sits in
    // one try, so every failure reaches the hand-back below rather than escaping
    // with the stream and the sender on different tracks.
    let failure = null;
    try {
      // `userProvidedTrack` keeps LiveKit from stopping a track this page owns
      // when the next switch replaces it. The page stops the old one itself
      // below, after the new one is already being sent, so the swap has no gap.
      //
      // A fresh track starts enabled, and LiveKit enables it again from the
      // publication's flag inside the replace, so a muted candidate's flag is
      // held up across the swap and the new input is never live.
      track.enabled = micEnabled();
      await holdMute(published, micEnabled());
      await published.replaceTrack(track, { userProvidedTrack: true });
      // Down again afterwards, also when LiveKit raised it after a loss, or the
      // agent would hear nothing rather than a muted candidate's silence.
      await liftMute(published, micEnabled);
    } catch (error) {
      failure = error;
    }
    if (!current()) {
      track.stop();
      return "idle";
    }
    // An input unplugged during those awaits has already fired `ended`, before
    // anything was listening for it, so committing it would leave the
    // interview silent with no loss left to follow.
    if (!failure && track.readyState === "ended")
      failure = new Error("the new input ended during the switch");

    if (!failure) {
      track.enabled = micEnabled();
      if (held) {
        stream.removeTrack(held);
        // `stop()` fires no `ended`, so the old track's integrity listener
        // stays quiet. A switch is not a microphone stopping; the event the
        // caller publishes for it says what happened instead.
        held.stop();
      }
      stream.addTrack(track);
      onSwitched(track, reason);
      return "switched";
    }

    track.stop();
    // The sender goes back to the input still working, where there is one. A
    // rejected replace may already have handed the new input to the sender,
    // so the hand-back runs whatever failed; when the sender never left the
    // old input, LiveKit returns from replacing a track with itself at once.
    let restored = false;
    if (held?.readyState === "live") {
      try {
        held.enabled = micEnabled();
        await holdMute(published, micEnabled());
        await published.replaceTrack(held, { userProvidedTrack: true });
        // LiveKit was already listening when the new input ended, so it may
        // have muted the publication, which would keep the restored input
        // silent to the interviewer while the page reads "Mic on".
        await liftMute(published, micEnabled);
        held.enabled = micEnabled();
        restored = true;
      } catch (error) {
        failure = error;
      }
      if (!current()) return "idle";
    }
    if (restored) {
      onFailed(reason, failure, "refused");
      return "refused";
    }
    return strand(failure);
  };

  return {
    /// The Mic button's other half: the input in the stream it reaches itself.
    setEnabled(enabled) {
      if (inFlight && inFlight.readyState !== "ended")
        inFlight.enabled = enabled;
    },
    /// `groupId` is the picked row's group, passed only when no other row
    /// shares it. It decides "unchanged" only when the input in use was opened
    /// under an alias id, which no row carries.
    switchTo(deviceId, reason = "chosen", lost = null, groupId = null) {
      const next = chain
        .then(() => run(deviceId, reason, lost, groupId))
        .finally(() => {
          inFlight = null;
        });
      // A throw from a caller's callback must not wedge every later switch
      // behind a rejected promise.
      chain = next.catch(() => {});
      return next;
    },
  };
}
