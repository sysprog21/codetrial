/// The level the muted-speech watch reads, measured on the audio thread.
///
/// A level read once a frame, as the preflight meter in `mic-meter.js` reads
/// it, stops whenever the page's main thread is busy, and face detection and
/// the avatar keep it busy during an interview. This runs with the audio:
/// it takes the peak of every render quantum and posts the loudest of each
/// `INTERVAL_S` of audio with the audio clock's time at its end. A main thread
/// that stalls receives the messages late, in a burst, but every one still
/// carries the audio time it covers.
///
/// Loaded with `audioWorklet.addModule`, so it is its own file and imports
/// nothing.

/// 20 ms of audio: finer than the 60 ms dip and the 100 ms onset the detector
/// in `muted-speech.js` measures with it.
const INTERVAL_S = 0.02;

class MutedSpeechLevel extends AudioWorkletProcessor {
  constructor() {
    super();
    this.peak = 0;
    this.since = currentTime;
  }

  process(inputs) {
    for (const channel of inputs[0] ?? []) {
      for (let index = 0; index < channel.length; index += 1) {
        const sample = Math.abs(channel[index]);
        if (sample > this.peak) this.peak = sample;
      }
    }
    // `currentTime` is the start of this quantum; the level covers up to its end.
    const end = currentTime + (inputs[0]?.[0]?.length ?? 128) / sampleRate;
    if (end - this.since >= INTERVAL_S) {
      this.port.postMessage({ level: this.peak, at: end * 1000 });
      this.peak = 0;
      this.since = end;
    }
    return true;
  }
}

registerProcessor("muted-speech-level", MutedSpeechLevel);
