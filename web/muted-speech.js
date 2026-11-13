/// Notices a candidate talking while their microphone is muted.
///
/// Muting disables the tracks Jim, the room and the recorder read, so every one
/// of them hears silence. This listens to a clone of the microphone instead:
/// the clone is enabled, is never added to a stream, published or recorded,
/// and only its level is read, on this page. It lives from the mute until the
/// first sustained speech, an unmute, the input it cloned stopping, or the end
/// of the interview, whichever comes first, so one muted stretch reminds at
/// most once.
///
/// Nothing here touches the page, so a test can drive it with a fake level
/// source.

/// A level above this is speech. `MIC_SILENT_PEAK` (0.02) only proves the
/// microphone produces a signal, and a fan, steady typing or room noise lifted
/// by auto gain crosses it. Speech through the auto gain the page asks for
/// peaks far higher: the fake capture the browser tests play reaches 0.3 to 1.0
/// in its words, and with `MUTED_SPEECH_DIP_MS` it reminds within 3.7 s of
/// any point in its loop at this level, in every run captured from the page.
export const MUTED_SPEECH_PEAK = 0.1;
/// How long the level must stay above `MUTED_SPEECH_PEAK` before audio counts
/// as voiced: about a syllable. Key clicks and a knock on the desk end before
/// it and never count at all.
export const MUTED_SPEECH_ONSET_MS = 100;
/// The longest dip below `MUTED_SPEECH_PEAK` that does not restart the onset.
/// Speech drops for a reading or two between syllables and words, 20 to 40 ms
/// in the fake capture, and an onset restarted at every one of those leaves
/// too little voiced time to remind on most of its loop. Fast typing leaves at
/// least 80 ms between keys (ten keys a second, each one a 20 ms spike), so
/// 60 ms bridges the first and never the second. The dip itself is not voiced.
export const MUTED_SPEECH_DIP_MS = 60;
/// The reminder fires once voiced time covers `MUTED_SPEECH_VOICED_MS` of the
/// last `MUTED_SPEECH_WINDOW_MS`. Speech pauses between words, so continuous
/// sound for a whole second would miss most sentences; half of one is a
/// phrase.
export const MUTED_SPEECH_WINDOW_MS = 1000;
export const MUTED_SPEECH_VOICED_MS = 500;

/// One verdict per level: true once the recent audio holds enough speech.
/// `at` is the audio clock's time at the end of the stretch the level covers,
/// so the thresholds are audio time however late the message arrives. Jim
/// talking resets everything, because his voice from the speakers reaches the
/// microphone too and is not the candidate answering.
export function createMutedSpeechDetector() {
  /// Voiced intervals as [from, to] times, oldest first.
  const voiced = [];
  let aboveSince = null;
  /// The time of the last level above `MUTED_SPEECH_PEAK`.
  let loudAt = null;
  let previous = null;
  const reset = () => {
    voiced.length = 0;
    aboveSince = null;
    previous = null;
  };
  return (level, jimSpeaking, at) => {
    if (jimSpeaking) {
      reset();
      return false;
    }
    const loud = level > MUTED_SPEECH_PEAK;
    if (loud) {
      aboveSince ??= at;
      loudAt = at;
    } else if (loudAt === null || at - loudAt > MUTED_SPEECH_DIP_MS) {
      aboveSince = null;
    }
    // Voiced from the onset on, only for loud levels, and only for the time
    // since the last level.
    if (loud && aboveSince !== null) {
      const from = Math.max(previous ?? at, aboveSince + MUTED_SPEECH_ONSET_MS);
      if (at > from) voiced.push([from, at]);
    }
    previous = at;
    const start = at - MUTED_SPEECH_WINDOW_MS;
    while (voiced.length && voiced[0][1] <= start) voiced.shift();
    let total = 0;
    for (const [from, to] of voiced) total += to - Math.max(from, start);
    return total >= MUTED_SPEECH_VOICED_MS;
  };
}

/// The default level source: the clone's audio through an `AudioWorkletNode`
/// that posts `{ level, at }` from the audio thread
/// (`muted-speech-worklet.js`). The node has no output and nothing reaches
/// `context.destination`, so the clone is measured and never played. Returns
/// `{ stop }` at once; a `stop` before the module has loaded closes the context
/// as soon as it can.
export function startAudioLevels(stream, onLevel, onError) {
  let stopped = false;
  let context = null;
  let source = null;
  let node = null;
  const close = () => {
    if (node) node.port.onmessage = null;
    source?.disconnect();
    node?.disconnect();
    void context?.close().catch(() => {});
  };
  void (async () => {
    try {
      context = new AudioContext();
      await context.audioWorklet.addModule(
        new URL("./muted-speech-worklet.js", import.meta.url),
      );
      if (stopped) return close();
      node = new AudioWorkletNode(context, "muted-speech-level", {
        numberOfInputs: 1,
        numberOfOutputs: 0,
      });
      node.port.onmessage = ({ data }) => onLevel(data.level, data.at);
      // A processor that throws after setup fires this once and then posts
      // nothing, so without it the clone would stay open with no level coming.
      node.onprocessorerror = () => {
        if (!stopped) onError("muted-speech-level processor failed");
      };
      source = context.createMediaStreamSource(stream);
      source.connect(node);
      // Muting is a click, so the context may start; a context the browser
      // keeps suspended posts nothing, which costs a reminder and no more.
      await context.resume();
    } catch (error) {
      close();
      if (!stopped) onError(String(error?.message || error));
    }
  })();
  return {
    stop() {
      stopped = true;
      close();
    },
  };
}

export function createMutedSpeechWatch({
  jimSpeaking,
  onSpeech,
  startLevels = startAudioLevels,
}) {
  let generation = 0;
  let clone = null;
  let levels = null;
  /// The `ended` listener on the track being watched, removed by `stop` so
  /// that every mute of the same microphone does not leave one more behind.
  let listening = null;

  const stop = () => {
    generation += 1;
    levels?.stop();
    levels = null;
    clone?.stop();
    clone = null;
    listening?.track.removeEventListener?.("ended", listening.onEnded);
    listening = null;
  };

  return {
    /// Listen to `track` until speech, `stop`, or the track ending. A watch
    /// already running is replaced, so a microphone switch while muted moves
    /// it to the new input. Any failure leaves mute exactly as it was.
    watch(track) {
      stop();
      if (!track || track.readyState !== "live") return;
      const mine = generation;
      try {
        clone = track.clone();
        clone.enabled = true;
      } catch {
        clone = null;
        return;
      }
      const onEnded = () => {
        if (mine === generation) stop();
      };
      track.addEventListener?.("ended", onEnded);
      listening = { track, onEnded };
      const heard = createMutedSpeechDetector();
      levels = startLevels(
        new MediaStream([clone]),
        (level, at) => {
          if (mine !== generation) return;
          // `stop()` fires no `ended`, and the page stops the input that way
          // when the room drops or a switch strands it. The clone is a track of
          // its own, so it would keep the device open behind them.
          if (track.readyState !== "live") {
            stop();
            return;
          }
          if (!heard(level, jimSpeaking(), at)) return;
          stop();
          onSpeech();
        },
        () => {
          if (mine === generation) stop();
        },
      );
    },
    stop,
  };
}
