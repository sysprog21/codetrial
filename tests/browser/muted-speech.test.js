// Run with: node --test tests/browser/muted-speech.test.js
//
// Drives the detector and the watch with a fake level source and fake tracks,
// so what is asserted is when the reminder fires and what is left listening.
// Every level carries an explicit audio time, because the threshold is a
// duration and the time a message reaches the main thread is not.

import { test } from "node:test";
import assert from "node:assert/strict";

import { MIC_SILENT_PEAK } from "../../web/audio-check.js";
import {
  MUTED_SPEECH_ONSET_MS,
  MUTED_SPEECH_PEAK,
  MUTED_SPEECH_VOICED_MS,
  createMutedSpeechDetector,
  createMutedSpeechWatch,
  startAudioLevels,
} from "../../web/muted-speech.js";

globalThis.MediaStream ??= class {
  constructor(tracks) {
    this.tracks = tracks;
  }
};

const SPEECH = 0.3;
/// Continuous speech fires once it has been voiced for the voiced share of the
/// window, and a level only counts as voiced after the onset.
const FIRES_AT = MUTED_SPEECH_ONSET_MS + MUTED_SPEECH_VOICED_MS;

/// Levels every `every` ms of audio from `from` for `ms`, each one `level` (or
/// what `level` returns for that time). Returns the time of the first level
/// `sample` answers true for, or null.
function feed(
  sample,
  { every = 20, from = 0, ms = 1500, level = SPEECH } = {},
) {
  for (let index = 0; index * every < ms; index += 1) {
    const at = from + index * every;
    const value = typeof level === "function" ? level(at) : level;
    if (sample(value, at)) return at;
  }
  return null;
}

/// When a fresh detector first fires for `options`.
function firesAt(options, jimSpeaking = () => false) {
  const heard = createMutedSpeechDetector();
  return feed((level, at) => heard(level, jimSpeaking(at), at), options);
}

function fakeTrack() {
  const listeners = new Map();
  const track = {
    readyState: "live",
    enabled: false,
    clones: [],
    addEventListener(name, listener) {
      if (!listeners.has(name)) listeners.set(name, new Set());
      listeners.get(name).add(listener);
    },
    removeEventListener(name, listener) {
      listeners.get(name)?.delete(listener);
    },
    listenerCount: (name) => listeners.get(name)?.size ?? 0,
    end() {
      track.readyState = "ended";
      for (const listener of [...(listeners.get("ended") ?? [])]) listener();
    },
    clone() {
      const copy = { enabled: false, stopped: false };
      copy.stop = () => (copy.stopped = true);
      track.clones.push(copy);
      return copy;
    },
  };
  return track;
}

/// Records every level source the watch starts, and how often each is stopped.
function fakeSource() {
  const runs = [];
  const start = (stream, onLevel, onError) => {
    const run = { stream, onLevel, onError, stops: 0 };
    runs.push(run);
    return { stop: () => (run.stops += 1) };
  };
  return { runs, start };
}

function watchWith(source, jimSpeaking = () => false) {
  const heard = [];
  const watch = createMutedSpeechWatch({
    jimSpeaking,
    onSpeech: () => heard.push(true),
    startLevels: source.start,
  });
  return { watch, heard };
}

/// Plays speech into one source run, with explicit audio times.
function speak(run, options) {
  feed((level, at) => {
    run.onLevel(level, at);
    return false;
  }, options);
}

test("continuous speech fires at exactly the onset plus the voiced share", () => {
  assert.equal(firesAt(), FIRES_AT);
  assert.equal(firesAt({ every: 10 }), FIRES_AT);
});

// The interval between messages is the worklet's to choose; the threshold is
// audio time either way, to within one message.
for (const every of [10, 40]) {
  test(`levels every ${every} ms fire when they do every 20 ms`, () => {
    const at = firesAt({ every });
    assert.ok(Math.abs(at - FIRES_AT) <= every, `fired at ${at} ms`);
  });
}

test("a click or a knock does not fire", () => {
  // 30 ms of sound in every 120 ms: each falls silent for longer than a dip
  // before the onset is reached.
  const knocks = (at) => (at % 120 < 30 ? SPEECH : 0);
  for (const every of [10, 20])
    assert.equal(firesAt({ every, ms: 3000, level: knocks }), null);
});

// Ten to seven keys a second, each a 20 ms spike: every gap between keys is
// longer than a dip, so the onset is never reached.
test("fast typing never fires", () => {
  for (const period of [100, 120, 150]) {
    const typing = (at) => (at % period < 20 ? SPEECH : 0);
    for (const every of [10, 20])
      assert.equal(
        firesAt({ every, ms: 5000, level: typing }),
        null,
        `a key every ${period} ms`,
      );
  }
  // Uneven, as typing is: the next key 100 to 150 ms after the last.
  let next = 0;
  let step = 0;
  const uneven = (at) => {
    if (at >= next) {
      step = (step + 37) % 51;
      next += 100 + step;
    }
    return next - at > 100 + step - 20 ? SPEECH : 0;
  };
  assert.equal(firesAt({ every: 10, ms: 5000, level: uneven }), null);
});

// Speech drops for a reading or two between syllables, and fast typing leaves
// 80 ms or more between keys: a 40 ms dip keeps the onset, an 80 ms one starts
// it again.
test("a dip between syllables keeps the onset, and a gap between keys restarts it", () => {
  const dip = (ms) => (at) => (at >= 200 && at < 200 + ms ? 0 : SPEECH);
  // The dip's own time is not voiced, so the reminder waits that much longer.
  assert.equal(firesAt({ level: dip(40) }), FIRES_AT + 40);
  // The onset counts again from the end of the first loud level after the gap,
  // so the reminder also waits out the onset again. The voiced time before the
  // gap still counts.
  assert.equal(
    firesAt({ level: dip(80) }),
    FIRES_AT + 80 + 20 + MUTED_SPEECH_ONSET_MS,
  );
});

// A fan, steady typing or room noise lifted by auto gain sits well above the
// level that only proves a live microphone, and must never pass for speech.
test("a steady hum below the speech level never fires", () => {
  assert.ok(MUTED_SPEECH_PEAK >= 4 * MIC_SILENT_PEAK);
  for (const level of [MIC_SILENT_PEAK + 0.01, 0.05, MUTED_SPEECH_PEAK])
    assert.equal(firesAt({ ms: 10000, level }), null, `${level}`);
  assert.equal(firesAt({ level: MUTED_SPEECH_PEAK + 0.01 }), FIRES_AT);
});

test("speech after a pause fires once the onset and voiced time are met again", () => {
  const heard = createMutedSpeechDetector();
  const sample = (level, at) => heard(level, false, at);
  // 300 ms of speech, then silence long enough to leave the window.
  assert.equal(feed(sample, { ms: 300 }), null);
  assert.equal(feed(sample, { from: 300, ms: 1700, level: 0 }), null);
  assert.equal(feed(sample, { from: 2000 }), 2000 + FIRES_AT);
});

test("Jim speaking resets everything", () => {
  // Jim speaks at 550 ms, just before continuous speech would have fired.
  const jim = (at) => at >= 550 && at < 560;
  assert.equal(firesAt({ every: 10 }, jim), 560 + FIRES_AT);
});

test("the source reads an enabled clone, never the muted track", () => {
  const source = fakeSource();
  const { watch } = watchWith(source);
  const track = fakeTrack();
  watch.watch(track);
  assert.equal(track.enabled, false);
  assert.equal(track.clones.length, 1);
  assert.equal(track.clones[0].enabled, true);
  assert.deepEqual(source.runs[0].stream.tracks, [track.clones[0]]);
});

test("speech reminds once and releases the clone", () => {
  const source = fakeSource();
  const { watch, heard } = watchWith(source);
  const track = fakeTrack();
  watch.watch(track);
  speak(source.runs[0]);
  assert.equal(heard.length, 1);
  assert.equal(track.clones[0].stopped, true);
  assert.equal(source.runs[0].stops, 1);
});

test("no reminder while Jim speaks", () => {
  const source = fakeSource();
  const { watch, heard } = watchWith(source, () => true);
  const track = fakeTrack();
  watch.watch(track);
  speak(source.runs[0]);
  assert.equal(heard.length, 0);
  assert.equal(track.clones[0].stopped, false);
  assert.equal(source.runs[0].stops, 0);
});

test("unmute, device loss and a failed source each release the clone", () => {
  for (const release of [
    ({ watch }) => watch.stop(),
    ({ track }) => track.end(),
    ({ source }) => source.runs[0].onError("no context"),
  ]) {
    const source = fakeSource();
    const { watch, heard } = watchWith(source);
    const track = fakeTrack();
    watch.watch(track);
    release({ watch, track, source });
    assert.equal(track.clones[0].stopped, true);
    assert.equal(source.runs[0].stops, 1);
    speak(source.runs[0]);
    assert.equal(heard.length, 0);
  }
});

// `stop()` fires no `ended`: a dropped room and a stranded switch both stop the
// input that way, and the clone would otherwise keep the device open.
test("an input stopped without `ended` releases the clone at the next level", () => {
  const source = fakeSource();
  const { watch, heard } = watchWith(source);
  const track = fakeTrack();
  watch.watch(track);
  track.readyState = "ended";
  speak(source.runs[0]);
  assert.equal(heard.length, 0);
  assert.equal(track.clones[0].stopped, true);
  assert.equal(source.runs[0].stops, 1);
});

// A busy main thread receives the worklet's messages late and all at once. Each
// still carries the audio time it covers, so a burst of 1.5 s of speech is 1.5 s
// of speech, however little wall time passes while it is handled.
test("a burst of late levels credits the audio time they cover", () => {
  const source = fakeSource();
  const { watch, heard } = watchWith(source);
  const track = fakeTrack();
  watch.watch(track);
  speak(source.runs[0], { from: 5000 });
  assert.equal(heard.length, 1);
  assert.equal(track.clones[0].stopped, true);
});

// Each mute adds an `ended` listener to the same microphone track, so one left
// behind by every stop would pile up over an interview's toggles.
test("stopping removes the watch's listener from the track", () => {
  const source = fakeSource();
  const { watch } = watchWith(source);
  const track = fakeTrack();
  for (let toggle = 0; toggle < 5; toggle += 1) {
    watch.watch(track);
    assert.equal(track.listenerCount("ended"), 1);
    watch.stop();
    assert.equal(track.listenerCount("ended"), 0);
  }
  watch.watch(track);
  speak(source.runs.at(-1));
  assert.equal(track.listenerCount("ended"), 0);
});

test("a switched microphone moves the watch to the new input", () => {
  const source = fakeSource();
  const { watch, heard } = watchWith(source);
  const first = fakeTrack();
  const second = fakeTrack();
  watch.watch(first);
  watch.watch(second);
  assert.equal(first.clones[0].stopped, true);
  assert.equal(first.listenerCount("ended"), 0);
  speak(source.runs[0]);
  assert.equal(heard.length, 0);
  speak(source.runs[1]);
  assert.equal(heard.length, 1);
});

test("a track that cannot be cloned or has ended starts nothing", () => {
  const source = fakeSource();
  const { watch } = watchWith(source);
  const ended = fakeTrack();
  ended.readyState = "ended";
  watch.watch(ended);
  const refusing = fakeTrack();
  refusing.clone = () => {
    throw new Error("not supported");
  };
  watch.watch(refusing);
  watch.watch(null);
  assert.equal(source.runs.length, 0);
  assert.equal(ended.clones.length, 0);
});

/// Fake Web Audio for `startAudioLevels`: records what is built, connected,
/// disconnected and closed. `loads` is what `addModule` returns.
function fakeAudio(t, loads = Promise.resolve()) {
  const made = { contexts: [], nodes: [], sources: [], destination: false };
  const saved = [globalThis.AudioContext, globalThis.AudioWorkletNode];
  t.after(() => {
    [globalThis.AudioContext, globalThis.AudioWorkletNode] = saved;
  });
  globalThis.AudioContext = class {
    constructor() {
      this.closed = false;
      this.modules = [];
      this.audioWorklet = {
        addModule: (url) => {
          this.modules.push(String(url));
          return loads;
        },
      };
      made.contexts.push(this);
    }
    get destination() {
      made.destination = true;
      return {};
    }
    createMediaStreamSource(stream) {
      const source = { stream, connected: [], disconnected: false };
      source.connect = (node) => source.connected.push(node);
      source.disconnect = () => (source.disconnected = true);
      made.sources.push(source);
      return source;
    }
    resume() {
      return Promise.resolve();
    }
    close() {
      this.closed = true;
      return Promise.resolve();
    }
  };
  globalThis.AudioWorkletNode = class {
    constructor(context, name, options) {
      Object.assign(this, { context, name, options, disconnected: false });
      this.port = { onmessage: null };
      made.nodes.push(this);
    }
    disconnect() {
      this.disconnected = true;
    }
  };
  return made;
}

const settle = () => new Promise((done) => setTimeout(done, 0));

test("the level source measures the clone on the audio thread and never plays it", async (t) => {
  const made = fakeAudio(t);
  const stream = new MediaStream([]);
  const levels = [];
  const errors = [];
  const run = startAudioLevels(
    stream,
    (level, at) => levels.push([level, at]),
    (error) => errors.push(error),
  );
  await settle();
  const [context] = made.contexts;
  const [node] = made.nodes;
  assert.match(context.modules[0], /\/muted-speech-worklet\.js$/);
  assert.equal(node.name, "muted-speech-level");
  assert.equal(node.options.numberOfOutputs, 0);
  assert.equal(made.sources[0].stream, stream);
  assert.deepEqual(made.sources[0].connected, [node]);
  assert.equal(made.destination, false);
  node.port.onmessage({ data: { level: 0.3, at: 20 } });
  assert.deepEqual(levels, [[0.3, 20]]);
  run.stop();
  assert.equal(made.sources[0].disconnected, true);
  assert.equal(node.disconnected, true);
  assert.equal(node.port.onmessage, null);
  assert.equal(context.closed, true);
  assert.deepEqual(errors, []);
});

// Muting and unmuting quickly stops the source while the module still loads;
// the context it made must not outlive that, nor start reading the clone.
test("a level source stopped before its worklet loads starts nothing", async (t) => {
  let load;
  const made = fakeAudio(t, new Promise((done) => (load = done)));
  const errors = [];
  const run = startAudioLevels(
    new MediaStream([]),
    () => {},
    (error) => errors.push(error),
  );
  run.stop();
  load();
  await settle();
  assert.equal(made.contexts[0].closed, true);
  assert.deepEqual(made.nodes, []);
  assert.deepEqual(made.sources, []);
  assert.deepEqual(errors, []);
});

test("a worklet that fails to load reports the failure and closes", async (t) => {
  const made = fakeAudio(t, Promise.reject(new Error("no worklet")));
  const errors = [];
  startAudioLevels(
    new MediaStream([]),
    () => {},
    (error) => errors.push(error),
  );
  await settle();
  assert.deepEqual(errors, ["no worklet"]);
  assert.equal(made.contexts[0].closed, true);
});

// A processor that throws once running fires `processorerror` and then posts
// nothing, so the watch would otherwise wait on the clone for good.
test("a processor that fails after setup releases the clone and closes", async (t) => {
  const made = fakeAudio(t);
  const track = fakeTrack();
  const { watch, heard } = watchWith({ start: startAudioLevels });
  watch.watch(track);
  await settle();
  const [node] = made.nodes;
  node.onprocessorerror?.();
  assert.equal(track.clones[0].stopped, true);
  assert.equal(made.contexts[0].closed, true);
  assert.equal(node.disconnected, true);
  assert.equal(track.listenerCount("ended"), 0);
  assert.deepEqual(heard, []);
});
