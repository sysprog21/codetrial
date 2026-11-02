import assert from "node:assert/strict";
import { test } from "node:test";
import { createRequire } from "node:module";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { spawnSync } from "node:child_process";
const require = createRequire(import.meta.url);
const {
  loadFixture,
  validateManifest,
} = require("../../scripts/replay-fixture.cjs");

function audio(format = "pcm_s16le", durationMs = 100) {
  const rate = format === "pcm_s16le" ? 16000 : 48000;
  const pcm = Buffer.alloc((rate * 2 * durationMs) / 1000);
  for (let i = 0; i < pcm.length / 2; i++)
    pcm.writeInt16LE(Math.round(6000 * Math.sin(i / 8)), i * 2);
  if (format === "pcm_s16le") return pcm;
  const header = Buffer.alloc(44);
  header.write("RIFF");
  header.writeUInt32LE(36 + pcm.length, 4);
  header.write("WAVEfmt ", 8);
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);
  header.writeUInt16LE(1, 22);
  header.writeUInt32LE(rate, 24);
  header.writeUInt32LE(rate * 2, 28);
  header.writeUInt16LE(2, 32);
  header.writeUInt16LE(16, 34);
  header.write("data", 36);
  header.writeUInt32LE(pcm.length, 40);
  return Buffer.concat([header, pcm]);
}

const digest = (bytes) =>
  crypto.createHash("sha256").update(bytes).digest("hex");
function fixture(t, format = "pcm_s16le") {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "replay-fixture-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  let bytes = audio(format);
  const manifest = {
    version: 1,
    id: "backchannel",
    origin: "synthetic-tone",
    audio: {
      file: format === "pcm_s16le" ? "clip.pcm" : "clip.wav",
      sha256: digest(bytes),
      format,
      sample_rate_hz: format === "pcm_s16le" ? 16000 : 48000,
      channels: 1,
      pause: null,
    },
    gate: { state: "speaking", tool: null, timeout_ms: 10000, max_age_ms: 250 },
    observation_ms: 5000,
    expected: { kind: "overlap", outcome: "continued" },
  };
  const filename = path.join(dir, "manifest.json");
  const write = () => {
    fs.writeFileSync(path.join(dir, manifest.audio.file), bytes);
    fs.writeFileSync(filename, JSON.stringify(manifest));
  };
  write();
  return {
    manifest,
    filename,
    dir,
    write,
    setBytes(b) {
      bytes = b;
      manifest.audio.sha256 = digest(b);
      write();
    },
  };
}

test("loads named WAV and PCM with exact byte/manifest identity before any transport", (t) => {
  for (const [format, transport, frames] of [
    ["pcm_s16le", "socket", 1600],
    ["wav_pcm_s16le", "browser", 4800],
  ]) {
    const f = fixture(t, format);
    const loaded = loadFixture(f.filename, transport);
    assert.equal(loaded.manifest.id, "backchannel");
    assert.equal(loaded.frames, frames);
    assert.equal(loaded.duration_ms, 100);
    assert.deepEqual(loaded.bytes, audio(format));
    assert.equal(loaded.manifest_sha256, digest(fs.readFileSync(f.filename)));
    assert.throws(
      () =>
        loadFixture(f.filename, transport === "browser" ? "socket" : "browser"),
      /match transport/,
    );
  }
});

test("refuses malformed manifest fields, stale gates and invalid floor pairs", (t) => {
  const f = fixture(t);
  for (const change of [
    (m) => {
      m.extra = true;
    },
    (m) => {
      m.version = 2;
    },
    (m) => {
      m.id = "../escape";
    },
    (m) => {
      m.audio.file = "../clip.pcm";
    },
    (m) => {
      m.audio.file = "/tmp/clip.pcm";
    },
    (m) => {
      m.audio.sha256 = "0";
    },
    (m) => {
      m.audio.channels = 2;
    },
    (m) => {
      m.audio.sample_rate_hz = 48000;
    },
    (m) => {
      m.audio.format = "mp3";
    },
    (m) => {
      m.gate.state = "caption-looks-like-hint";
    },
    (m) => {
      m.gate.state = "hint";
    },
    (m) => {
      m.gate.tool = "log_hint";
    },
    (m) => {
      m.gate.timeout_ms = 0;
    },
    (m) => {
      m.gate.timeout_ms = 120001;
    },
    (m) => {
      m.gate.max_age_ms = 5001;
    },
    (m) => {
      m.gate.max_age_ms = m.gate.timeout_ms + 1;
    },
    (m) => {
      m.observation_ms = 0.5;
    },
    (m) => {
      m.observation_ms = Infinity;
    },
    (m) => {
      m.expected = { kind: ["overlap"], outcome: ["continued"] };
    },
    (m) => {
      m.expected = { kind: ["pause"], outcome: ["continue"] };
    },
    (m) => {
      m.expected = { kind: null, outcome: "continue" };
    },
    (m) => {
      m.expected = { kind: "pause", outcome: {} };
    },
    (m) => {
      m.expected.outcome = "continue";
    },
    (m) => {
      m.expected.outcome = "unresolved";
    },
  ]) {
    const m = structuredClone(f.manifest);
    change(m);
    assert.throws(
      () => validateManifest(m),
      /replay fixture:/,
      change.toString(),
    );
  }
  for (const [kind, outcome] of [
    ["overlap", "continued"],
    ["overlap", "interrupted"],
    ["pause", "respond"],
  ]) {
    validateManifest({ ...f.manifest, expected: { kind, outcome } });
  }
});

test("loader accepts PCM whose high-bit bytes only resemble container signatures", (t) => {
  const f = fixture(t);
  for (const head of [
    [0xd2, 0xc9, 0xc6, 0xc6],
    [0xc9, 0xc4, 0x33],
  ]) {
    const bytes = audio();
    Buffer.from(head).copy(bytes);
    f.setBytes(bytes);
    assert.doesNotThrow(() => loadFixture(f.filename, "socket"));
  }
});
test("loader rejects changed bytes, odd/empty PCM, container headers and symlinks", (t) => {
  const f = fixture(t);
  fs.writeFileSync(path.join(f.dir, "clip.pcm"), Buffer.alloc(3200));
  assert.throws(() => loadFixture(f.filename, "socket"), /digest mismatch/);
  for (const bytes of [
    Buffer.alloc(0),
    Buffer.alloc(3),
    Buffer.from("RIFFabcd"),
    Buffer.from("RIFXabcd"),
    Buffer.from("RF64abcd"),
    Buffer.from("fLaCabcd"),
    Buffer.from("OggSabcd"),
    Buffer.from("FORMabcd"),
    Buffer.from("WAVEabcd"),
    Buffer.from("ID3xxxxx"),
  ]) {
    f.setBytes(bytes);
    assert.throws(() => loadFixture(f.filename, "socket"), /frames|header/);
  }
  f.setBytes(audio());
  fs.writeFileSync(path.join(f.dir, "matching.pcm"), audio());
  fs.unlinkSync(path.join(f.dir, "clip.pcm"));
  fs.symlinkSync("matching.pcm", path.join(f.dir, "clip.pcm"));
  assert.throws(() => loadFixture(f.filename, "socket"), { code: "ELOOP" });
  const linkedManifest = path.join(f.dir, "linked.json");
  fs.symlinkSync("manifest.json", linkedManifest);
  assert.throws(() => loadFixture(linkedManifest, "socket"), { code: "ELOOP" });
});

test("WAV decoder checks rate, bits, alignment, lengths and duration", (t) => {
  const f = fixture(t, "wav_pcm_s16le");
  for (const mutate of [
    (b) => b.writeUInt32LE(16000, 24),
    (b) => b.writeUInt16LE(8, 34),
    (b) => b.writeUInt16LE(4, 32),
    (b) => b.writeUInt32LE(123, 28),
    (b) => b.writeUInt32LE(0xffffffff, 40),
  ]) {
    const b = audio("wav_pcm_s16le");
    mutate(b);
    f.setBytes(b);
    assert.throws(() => loadFixture(f.filename, "browser"), /WAV|chunk/);
  }
  f.setBytes(audio("wav_pcm_s16le", 0));
  assert.throws(() => loadFixture(f.filename, "browser"), /duration/);
});

test("pause/continue requires a real internal quiet interval with signal on both sides", (t) => {
  const f = fixture(t);
  f.manifest.expected = { kind: "pause", outcome: "continue" };
  f.write();
  assert.throws(() => loadFixture(f.filename, "socket"), /audio.pause/);
  f.manifest.audio.pause = { start_ms: 20, end_ms: 70 };
  const b = audio();
  f.setBytes(b);
  assert.throws(() => loadFixture(f.filename, "socket"), /not quiet/);
  b.fill(0, 20 * 32, 70 * 32);
  f.setBytes(b);
  assert.equal(loadFixture(f.filename, "socket").duration_ms, 100);
  f.manifest.audio.pause.end_ms = 100;
  f.write();
  assert.throws(() => loadFixture(f.filename, "socket"), /inside the clip/);
  f.manifest.audio.pause.end_ms = 70;
  b.fill(0);
  f.setBytes(b);
  assert.throws(
    () => loadFixture(f.filename, "socket"),
    /signal before and after/,
  );
});

test("caps are checked at the limit and one frame/byte past, without provider access", (t) => {
  const f = fixture(t);
  f.setBytes(audio("pcm_s16le", 30000));
  assert.equal(loadFixture(f.filename, "socket").duration_ms, 30000);
  f.setBytes(Buffer.concat([audio("pcm_s16le", 30000), Buffer.alloc(2)]));
  assert.throws(
    () => loadFixture(f.filename, "socket"),
    /bounded regular file/,
  );
  fs.writeFileSync(f.filename, " ".repeat(16384));
  assert.throws(() => loadFixture(f.filename, "socket"), /UTF-8 JSON/);
  fs.appendFileSync(f.filename, " ");
  assert.throws(
    () => loadFixture(f.filename, "socket"),
    /bounded regular file/,
  );
});

test("CLI exits nonzero on invalid input and prints only offline fixture metadata", (t) => {
  const f = fixture(t);
  const run = (...args) =>
    spawnSync(process.execPath, ["scripts/replay-fixture.cjs", ...args], {
      encoding: "utf8",
    });
  const good = run(f.filename, "socket");
  assert.equal(good.status, 0, good.stderr);
  const output = JSON.parse(good.stdout);
  assert.equal(output.byte_length, 3200);
  assert.equal(Object.hasOwn(output, "bytes"), false);
  assert.equal(run(f.filename, "invalid").status, 2);
  fs.writeFileSync(f.filename, Buffer.from([0xff, 0x7b, 0x7d]));
  assert.equal(run(f.filename, "socket").status, 2);
});

test("duplicate JSON keys, including escaped equivalent keys, are refused", (t) => {
  const f = fixture(t);
  const text = fs.readFileSync(f.filename, "utf8");
  for (const key of ['"id"', '"\\u0069d"']) {
    fs.writeFileSync(
      f.filename,
      text.replace('"id":', `${key}:"shadow", "id":`),
    );
    assert.throws(
      () => loadFixture(f.filename, "socket"),
      /duplicate object key/,
    );
  }
  fs.writeFileSync(
    f.filename,
    text.replace('"continued"', '"continued","outcome":"continued"'),
  );
  assert.throws(
    () => loadFixture(f.filename, "socket"),
    /duplicate object key/,
  );
});

test(
  "non-regular FIFOs refuse before a blocking read or provider access",
  { skip: process.platform === "win32" },
  (t) => {
    const f = fixture(t);
    const fifo = path.join(f.dir, "input.pipe");
    const made = spawnSync("mkfifo", [fifo], { encoding: "utf8" });
    assert.equal(made.status, 0, made.stderr);
    const run = (filename) =>
      spawnSync(
        process.execPath,
        ["scripts/replay-fixture.cjs", filename, "socket"],
        { encoding: "utf8", timeout: 5000 },
      );
    const manifestResult = run(fifo);
    assert.equal(manifestResult.error, undefined);
    assert.equal(manifestResult.status, 2);
    assert.match(manifestResult.stderr, /bounded regular file/);
    f.manifest.audio.file = "input.pipe";
    fs.writeFileSync(f.filename, JSON.stringify(f.manifest));
    const audioResult = run(f.filename);
    assert.equal(audioResult.error, undefined);
    assert.equal(audioResult.status, 2);
    assert.match(audioResult.stderr, /bounded regular file/);
  },
);

test("valid gate variants and timing endpoints are distinct from invalid combinations", (t) => {
  const f = fixture(t);
  for (const [state, tool] of [
    ["listening", null],
    ["hint", "log_hint"],
    ["tool", "read_editor"],
  ]) {
    const m = structuredClone(f.manifest);
    m.gate = { state, tool, timeout_ms: 120000, max_age_ms: 5000 };
    m.observation_ms = 120000;
    validateManifest(m);
    m.gate = { state, tool, timeout_ms: 1, max_age_ms: 1 };
    m.observation_ms = 1;
    validateManifest(m);
  }
  const m = structuredClone(f.manifest);
  m.gate.timeout_ms = 100;
  m.gate.max_age_ms = 101;
  assert.throws(() => validateManifest(m), /age exceeds timeout/);
  m.gate = { state: "tool", tool: null, timeout_ms: 100, max_age_ms: 50 };
  assert.throws(() => validateManifest(m), /requires a tool name/);
});

test("WAV ancillary chunks are accepted, but duplicate fmt/data chunks are refused", (t) => {
  const f = fixture(t, "wav_pcm_s16le");
  const wav = audio("wav_pcm_s16le");
  const withChunk = (chunk) => {
    const b = Buffer.concat([wav, chunk]);
    b.writeUInt32LE(b.length - 8, 4);
    return b;
  };
  f.setBytes(withChunk(Buffer.from("JUNK\x01\x00\x00\x00\x00\x00", "binary")));
  assert.equal(loadFixture(f.filename, "browser").frames, 4800);
  for (const chunk of [wav.subarray(12, 36), wav.subarray(36)]) {
    f.setBytes(withChunk(chunk));
    assert.throws(
      () => loadFixture(f.filename, "browser"),
      /one fmt and one data/,
    );
  }
  const odd = Buffer.concat([wav, Buffer.alloc(2)]);
  odd.writeUInt32LE(odd.length - 8, 4);
  odd.writeUInt32LE(wav.length - 44 + 1, 40);
  f.setBytes(odd);
  assert.throws(() => loadFixture(f.filename, "browser"), /whole frames/);
});

test("growth after fstat is bounded by the read cap, not the old file size", (t) => {
  const f = fixture(t);
  const text = fs.readFileSync(f.filename, "utf8");
  fs.writeFileSync(f.filename, text.padEnd(16384));
  const realStat = fs.fstatSync;
  let grew = false;
  t.mock.method(fs, "fstatSync", (fd) => {
    const stat = realStat(fd);
    if (!grew) {
      grew = true;
      fs.appendFileSync(f.filename, " ");
    }
    return stat;
  });
  assert.throws(() => loadFixture(f.filename, "socket"), /exceeds byte limit/);
  assert.equal(grew, true);
});

test("WAV duration and file-byte ceilings accept exact boundaries and reject excess", (t) => {
  const f = fixture(t, "wav_pcm_s16le");
  f.setBytes(audio("wav_pcm_s16le", 30000));
  assert.equal(loadFixture(f.filename, "browser").duration_ms, 30000);
  f.setBytes(audio("wav_pcm_s16le", 30001));
  assert.throws(() => loadFixture(f.filename, "browser"), /duration/);
  const wav = audio("wav_pcm_s16le");
  const chunk = Buffer.alloc(2945536 - wav.length);
  chunk.write("JUNK");
  chunk.writeUInt32LE(chunk.length - 8, 4);
  const atLimit = Buffer.concat([wav, chunk]);
  atLimit.writeUInt32LE(atLimit.length - 8, 4);
  f.setBytes(atLimit);
  assert.equal(loadFixture(f.filename, "browser").bytes.length, 2945536);
  assert.equal(loadFixture(f.filename, "browser").frames, 4800);
  f.setBytes(Buffer.concat([atLimit, Buffer.alloc(1)]));
  assert.throws(
    () => loadFixture(f.filename, "browser"),
    /bounded regular file/,
  );
});

test("pause threshold is inclusive at +/-500 and bounds require a real internal interval", (t) => {
  const f = fixture(t);
  f.manifest.expected = { kind: "pause", outcome: "continue" };
  f.manifest.audio.pause = { start_ms: 20, end_ms: 70 };
  const b = audio();
  b.fill(0, 20 * 32, 70 * 32);
  for (const value of [500, -500]) {
    b.writeInt16LE(value, 40 * 32);
    f.setBytes(b);
    assert.equal(loadFixture(f.filename, "socket").frames, 1600);
  }
  for (const value of [501, -501]) {
    b.writeInt16LE(value, 40 * 32);
    f.setBytes(b);
    assert.throws(() => loadFixture(f.filename, "socket"), /not quiet/);
  }
  for (const pause of [
    { start_ms: 0, end_ms: 70 },
    { start_ms: 20, end_ms: 20 },
    { start_ms: 20, end_ms: 19 },
  ]) {
    f.manifest.audio.pause = pause;
    f.write();
    assert.throws(() => loadFixture(f.filename, "socket"), /audio.pause/);
  }
});
