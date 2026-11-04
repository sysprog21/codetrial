export function createPeerAudio({
  sdk,
  container,
  enableSound,
  onReconnect = async () => {},
  onDisconnect = () => {},
  onError = () => {},
}) {
  let room = null;
  let generation = 0;
  let desiredMuted = false;
  const attached = new Map();
  const drop = (track) => {
    if (!attached.has(track)) return;
    for (const element of track.detach()) element.remove();
    attached.delete(track);
  };
  const attach = (track) => {
    if (track.kind !== "audio" || attached.has(track)) return;
    const element = track.attach();
    attached.set(track, element);
    container.append(element);
    Promise.resolve(element.play()).catch((error) => {
      if (error.name === "NotAllowedError") enableSound.hidden = false;
      else onError(error);
    });
  };
  function clearRemote() {
    for (const track of [...attached.keys()]) drop(track);
  }
  async function disconnect() {
    generation++;
    const old = room;
    room = null;
    clearRemote();
    if (old) {
      for (const publication of old.localParticipant.trackPublications.values())
        publication.track?.stop();
      await old.disconnect();
    }
  }
  enableSound.addEventListener("click", async () => {
    try {
      await room?.startAudio();
      await Promise.all(
        [...attached.values()].map((element) => element.play()),
      );
      enableSound.hidden = true;
    } catch (error) {
      onError(error);
    }
  });
  return {
    async connect(url, token) {
      await disconnect();
      const epoch = generation;
      const next = new sdk.Room({
        disconnectOnPageLeave: false,
        stopLocalTrackOnUnpublish: true,
      });
      room = next;
      next.on(sdk.RoomEvent.TrackSubscribed, attach);
      next.on(sdk.RoomEvent.TrackUnsubscribed, drop);
      next.on(sdk.RoomEvent.Disconnected, () => {
        if (room === next) {
          clearRemote();
          onDisconnect();
        }
      });
      next.on(sdk.RoomEvent.Reconnected, () => {
        if (room !== next) return;
        clearRemote();
        for (const participant of next.remoteParticipants.values()) {
          for (const publication of participant.trackPublications.values())
            if (publication.track) attach(publication.track);
        }
        void onReconnect().catch(onError);
      });
      await next.connect(url, token);
      if (generation !== epoch || room !== next) {
        await next.disconnect();
        return null;
      }
      await next.localParticipant.setMicrophoneEnabled(!desiredMuted);
      if (generation !== epoch || room !== next) {
        for (const publication of next.localParticipant.trackPublications.values())
          publication.track?.stop();
        await next.disconnect();
        return null;
      }
      return next;
    },
    async mute(muted) {
      desiredMuted = muted;
      await room?.localParticipant.setMicrophoneEnabled(!muted);
    },
    async device(id) {
      await room?.switchActiveDevice("audioinput", id);
    },
    disconnect,
  };
}

/// Serialises microphone toggles: each click flips the last requested state, so
/// clicks made while the SDK call is pending cannot cancel each other out.
export function createMuteToggle({ mute, onChange, onError }) {
  let applied = false;
  let wanted = false;
  let queue = Promise.resolve();
  let latest = 0;
  return () => {
    const next = !wanted;
    const mine = ++latest;
    wanted = next;
    queue = queue
      .then(() => mute(next))
      .then(() => {
        applied = next;
        onChange(next);
      })
      .catch((failure) => {
        // Later clicks were counted from this one; only the latest may undo it.
        if (mine === latest) wanted = applied;
        onError(failure);
      });
    return queue;
  };
}
