// Offline input validation shared by the observational replay transports.
const fs = require("node:fs");
const path = require("node:path");
const { readReplayFile, decodeReplayJson } = require("./replay-input.cjs");
const { parseWav, sha256 } = require("./long-interview-schedule.cjs");

const MANIFEST_BYTES = 16384;
const CLIP_MS = 30000;
const WAV_BYTES = 48000 * 2 * (CLIP_MS / 1000) + 65536;
const PCM_BYTES = 16000 * 2 * (CLIP_MS / 1000);
const PAIRS = new Set([
  "overlap/continued",
  "overlap/interrupted",
  "pause/respond",
  "pause/continue",
]);

function refuse(message) {
  throw new Error(`replay fixture: ${message}`);
}

function fields(value, keys, where) {
  if (!value || typeof value !== "object" || Array.isArray(value))
    refuse(`${where} must be an object`);
  const actual = Object.keys(value);
  if (
    actual.length !== keys.length ||
    keys.some((key) => !Object.hasOwn(value, key))
  )
    refuse(`${where} requires exactly ${keys.join(", ")}`);
}

function integer(value, min, max, where) {
  if (!Number.isSafeInteger(value) || value < min || value > max)
    refuse(`${where} must be an integer in ${min}..${max}`);
}

function validateManifest(manifest) {
  fields(
    manifest,
    ["version", "id", "origin", "audio", "gate", "observation_ms", "expected"],
    "manifest",
  );
  if (manifest.version !== 1) refuse("unsupported version");
  if (
    typeof manifest.id !== "string" ||
    !/^[a-z][a-z0-9-]{0,63}$/.test(manifest.id)
  )
    refuse(
      "id must be 1..64 lowercase letters, digits or hyphens, starting with a letter",
    );
  if (!["synthetic-tone", "synthetic-speech"].includes(manifest.origin))
    refuse("origin must be synthetic-tone or synthetic-speech");
  const audio = manifest.audio;
  fields(
    audio,
    ["file", "sha256", "format", "sample_rate_hz", "channels", "pause"],
    "audio",
  );
  if (
    typeof audio.file !== "string" ||
    !/^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/.test(audio.file)
  )
    refuse("audio.file must be a filename beside the manifest");
  if (typeof audio.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(audio.sha256))
    refuse("audio.sha256 must be 64 lowercase hex digits");
  if (!["wav_pcm_s16le", "pcm_s16le"].includes(audio.format))
    refuse("unsupported audio format");
  if (audio.channels !== 1) refuse("audio.channels must be 1");
  if (
    audio.sample_rate_hz !== (audio.format === "wav_pcm_s16le" ? 48000 : 16000)
  )
    refuse("WAV must be 48000 Hz; raw PCM must be 16000 Hz");
  const gate = manifest.gate;
  fields(gate, ["state", "tool", "timeout_ms", "max_age_ms"], "gate");
  if (!["speaking", "listening", "hint", "tool"].includes(gate.state))
    refuse("unsupported gate state");
  if (gate.state === "hint") {
    if (gate.tool !== "log_hint") refuse("hint gate requires tool=log_hint");
  } else if (gate.state === "tool") {
    if (
      typeof gate.tool !== "string" ||
      !/^[a-z][a-z0-9_]{0,63}$/.test(gate.tool)
    )
      refuse("tool gate requires a tool name");
  } else if (gate.tool !== null)
    refuse("speaking/listening gate requires tool=null");
  integer(gate.timeout_ms, 1, 120000, "gate.timeout_ms");
  integer(gate.max_age_ms, 1, 5000, "gate.max_age_ms");
  if (gate.max_age_ms > gate.timeout_ms) refuse("gate age exceeds timeout");
  integer(manifest.observation_ms, 1, 120000, "observation_ms");
  fields(manifest.expected, ["kind", "outcome"], "expected");
  if (
    typeof manifest.expected.kind !== "string" ||
    typeof manifest.expected.outcome !== "string" ||
    !PAIRS.has(`${manifest.expected.kind}/${manifest.expected.outcome}`)
  )
    refuse("unsupported expected kind/outcome pair");
  if (
    manifest.expected.kind === "pause" &&
    manifest.expected.outcome === "continue"
  ) {
    fields(audio.pause, ["start_ms", "end_ms"], "audio.pause");
    integer(audio.pause.start_ms, 1, CLIP_MS - 1, "audio.pause.start_ms");
    integer(
      audio.pause.end_ms,
      audio.pause.start_ms + 1,
      CLIP_MS - 1,
      "audio.pause.end_ms",
    );
  } else if (audio.pause !== null)
    refuse("audio.pause is only used by pause/continue");
  return manifest;
}

function inspectAudio(audio, bytes) {
  if (sha256(bytes) !== audio.sha256) refuse("audio digest mismatch");
  let frames;
  let sampleAt;
  if (audio.format === "pcm_s16le") {
    if (
      ["RIFF", "RIFX", "RF64", "WAVE", "FORM", "OggS", "fLaC"].includes(
        bytes.toString("latin1", 0, 4),
      ) ||
      bytes.toString("latin1", 0, 3) === "ID3"
    )
      refuse("raw PCM contains a container header");
    if (!bytes.length || bytes.length % 2)
      refuse("raw PCM must contain whole nonempty s16le frames");
    frames = bytes.length / 2;
    sampleAt = (i) => bytes.readInt16LE(i * 2);
  } else {
    const wav = parseWav(bytes);
    if (wav.error) refuse(wav.error);
    if (wav.channels !== 1 || wav.sampleRate !== 48000 || wav.bits !== 16)
      refuse("WAV header disagrees with declared mono 48000 Hz s16le format");
    // parseWav decodes frames; check header consistency and ambiguity too.
    let fmtCount = 0;
    let dataCount = 0;
    for (let offset = 12; offset < bytes.length;) {
      const name = bytes.toString("ascii", offset, offset + 4);
      const size = bytes.readUInt32LE(offset + 4);
      const body = offset + 8;
      if (name === "fmt ") {
        fmtCount++;
        if (
          size < 16 ||
          bytes.readUInt32LE(body + 8) !== 96000 ||
          bytes.readUInt16LE(body + 12) !== 2
        )
          refuse("WAV byte rate or block alignment is invalid");
      }
      if (name === "data") dataCount++;
      offset = body + size + (size % 2);
    }
    if (fmtCount !== 1 || dataCount !== 1)
      refuse("WAV requires one fmt and one data chunk");
    frames = wav.samples.length;
    sampleAt = (i) => wav.samples[i];
  }
  const durationMs = (frames * 1000) / audio.sample_rate_hz;
  if (durationMs <= 0 || durationMs > CLIP_MS)
    refuse(`clip duration must be positive and at most ${CLIP_MS} ms`);
  if (audio.pause !== null) {
    const from = (audio.pause.start_ms * audio.sample_rate_hz) / 1000;
    const to = (audio.pause.end_ms * audio.sample_rate_hz) / 1000;
    if (to >= frames)
      refuse("annotated pause must be frame-aligned and inside the clip");
    let before = false;
    let after = false;
    for (let i = 0; i < frames; i++) {
      if (Math.abs(sampleAt(i)) <= 500) continue;
      if (i < from) before = true;
      else if (i < to) refuse("annotated pause is not quiet");
      else after = true;
    }
    if (!before || !after)
      refuse("annotated pause requires signal before and after it");
  }
  return { frames, duration_ms: durationMs };
}

function loadFixture(filename, transport) {
  if (!["browser", "socket"].includes(transport))
    refuse("transport must be browser or socket");
  const manifestBytes = readReplayFile(
    filename,
    MANIFEST_BYTES,
    "replay fixture",
  );
  const manifest = decodeReplayJson(manifestBytes, "replay fixture");
  validateManifest(manifest);
  const expectedFormat =
    transport === "browser" ? "wav_pcm_s16le" : "pcm_s16le";
  if (manifest.audio.format !== expectedFormat)
    refuse("audio format does not match transport");
  const directory = path.dirname(fs.realpathSync(filename));
  const audioPath = path.join(directory, manifest.audio.file);
  const bytes = readReplayFile(
    audioPath,
    transport === "browser" ? WAV_BYTES : PCM_BYTES,
    "replay fixture",
  );
  const audio = inspectAudio(manifest.audio, bytes);
  return {
    manifest,
    manifest_sha256: sha256(manifestBytes),
    bytes,
    ...audio,
  };
}

module.exports = { loadFixture, validateManifest, inspectAudio };

if (require.main === module) {
  try {
    if (process.argv.length !== 4)
      refuse("usage: node scripts/replay-fixture.cjs MANIFEST browser|socket");
    const { bytes, ...fixture } = loadFixture(process.argv[2], process.argv[3]);
    console.log(
      JSON.stringify({ ...fixture, byte_length: bytes.length }, null, 2),
    );
  } catch (error) {
    console.error(error.message);
    process.exitCode = 2;
  }
}
