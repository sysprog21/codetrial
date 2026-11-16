import assert from "node:assert/strict";
import { test } from "node:test";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const {
  installNamedReplay,
  namedReplayFlow,
  requireNamedReplayInputs,
  runNamedReplay,
  settleNamedReplay,
  validateReplayEnvironment,
} = require("../../scripts/named-replay.cjs");
const { BudgetDenied } = require("../../scripts/run-budget-guard.cjs");
const { loadFixture } = require("../../scripts/replay-fixture.cjs");

function wav(durationMs = 200) {
  const rate = 48000;
  const pcm = Buffer.alloc((rate * 2 * durationMs) / 1000);
  for (let i = 0; i < pcm.length / 2; i++)
    pcm.writeInt16LE(Math.round(6000 * Math.sin(i / 8)), i * 2);
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

function manifestFor(bytes, gate = {}) {
  return {
    version: 1,
    id: "short-backchannel",
    origin: "synthetic-tone",
    audio: {
      file: "clip.wav",
      sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
      format: "wav_pcm_s16le",
      sample_rate_hz: 48000,
      channels: 1,
      pause: null,
    },
    gate: {
      state: "speaking",
      tool: null,
      timeout_ms: 1000,
      max_age_ms: 250,
      ...gate,
    },
    observation_ms: 5000,
    expected: { kind: "overlap", outcome: "continued" },
  };
}

function loaded(t, gate = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "named-replay-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const bytes = wav();
  fs.writeFileSync(path.join(dir, "clip.wav"), bytes);
  const filename = path.join(dir, "manifest.json");
  fs.writeFileSync(filename, JSON.stringify(manifestFor(bytes, gate)));
  return loadFixture(filename, "browser");
}

// A page whose agent-state text, decode, resume and playback the test steers.
function pageDouble(
  fixture,
  { microphone = true, prepareTimeoutMs = 10000 } = {},
) {
  let state = "Listening";
  let played = 0;
  let decoded = 0;
  let connects = 0;
  let stopped = 0;
  let clock = 1000;
  let onDecode = () => {};
  let resumeHangs = false;
  let decodeRejects = false;
  const calls = [];
  const track = {
    kind: "audio",
    stop() {
      stopped++;
    },
  };
  const videoTrack = { kind: "video" };
  const replacement = { kind: "audio", stop() {} };
  const tracks = [track, videoTrack];
  const stream = {
    getAudioTracks: () => tracks.filter((item) => item.kind === "audio"),
    removeTrack(item) {
      tracks.splice(tracks.indexOf(item), 1);
    },
    addTrack(item) {
      tracks.push(item);
    },
  };
  let source;
  const sandbox = {
    navigator: { mediaDevices: { getUserMedia: async () => stream } },
    document: { querySelector: () => ({ textContent: state }) },
    atob: () => "wav",
    setTimeout,
    clearTimeout,
    performance: { now: () => clock },
    AudioContext: class {
      createMediaStreamDestination() {
        return { stream: { getAudioTracks: () => [replacement] } };
      }
      async decodeAudioData() {
        decoded++;
        if (decodeRejects) throw new Error("undecodable");
        onDecode();
        return {
          duration: 0.2,
          sampleRate: 48000,
          length: 9600,
          numberOfChannels: 1,
        };
      }
      resume() {
        return resumeHangs ? new Promise(() => {}) : Promise.resolve();
      }
      createBufferSource() {
        source = {
          connect() {
            connects++;
          },
          start() {
            played++;
          },
        };
        return source;
      }
    },
  };
  vm.createContext(sandbox);
  vm.runInContext(
    `(${installNamedReplay.toString()})(${JSON.stringify(
      fixture.bytes.toString("base64"),
    )}, ${prepareTimeoutMs})`,
    sandbox,
  );
  const page = {
    async evaluate(fn, arg) {
      calls.push(fn.toString());
      return vm.runInContext(
        `(${fn.toString()})(${JSON.stringify(arg ?? null)})`,
        sandbox,
      );
    },
  };
  return {
    page,
    calls,
    sandbox,
    tracks,
    videoTrack,
    replacement,
    ready: () =>
      microphone
        ? sandbox.navigator.mediaDevices.getUserMedia({ audio: true })
        : null,
    hangResume: () => {
      resumeHangs = true;
    },
    rejectDecode: () => {
      decodeRejects = true;
    },
    connects: () => connects,
    stopped: () => stopped,
    state: (value) => {
      state = value;
    },
    onDecode: (fn) => {
      onDecode = fn;
    },
    tick: (ms) => {
      clock += ms;
    },
    played: () => played,
    decoded: () => decoded,
    ended: () => source.onended(),
  };
}

function clockPair() {
  let at = 0;
  return {
    now: () => at,
    sleep: async (ms) => {
      at += ms;
    },
    advance: (ms) => {
      at += ms;
    },
  };
}

const inputs = {
  // Any regular file stands in for the isolated config; only its existence counts.
  env: { CODETRIAL_CONFIG_ENV: fileURLToPath(import.meta.url) },
  runBudget: { actions: ["candidate_join", "replay_inject"] },
};

test("a replay without an isolated config and a usable budget is refused", () => {
  assert.throws(
    () => requireNamedReplayInputs({ ...inputs, env: {} }),
    /isolated config/,
  );
  assert.throws(
    () => requireNamedReplayInputs({ ...inputs, runBudget: null }),
    /run budget/,
  );
  assert.throws(
    () =>
      requireNamedReplayInputs({
        ...inputs,
        runBudget: { actions: ["candidate_join"] },
      }),
    /replay_inject/,
  );
  assert.throws(
    () =>
      requireNamedReplayInputs({
        ...inputs,
        runBudget: { actions: ["replay_inject"] },
      }),
    /candidate_join/,
  );
  assert.doesNotThrow(() => requireNamedReplayInputs(inputs));
});

test("the environment rules name every reason a replay is refused", (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "named-replay-env-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  fs.writeFileSync(path.join(dir, "clip.wav"), wav());
  const good = manifestFor(wav());
  const file = path.join(dir, "manifest.json");
  fs.writeFileSync(file, JSON.stringify(good));
  const run = path.join(dir, "run.json");
  const writeRun = (actions) =>
    fs.writeFileSync(run, JSON.stringify({ run_id: "run-1", actions }));
  writeRun(["candidate_join", "replay_inject"]);
  const env = {
    BROWSER_CHECK_FLOW: "named-replay",
    BROWSER_CHECK_AGENT: "rust",
    BROWSER_CHECK_REPLAY_MANIFEST: file,
    BROWSER_CHECK_RUN_MANIFEST: run,
    BROWSER_CHECK_CAPTURE: "/capture.json",
    CODETRIAL_CONFIG_ENV: fileURLToPath(import.meta.url),
  };
  assert.deepEqual(validateReplayEnvironment(env), []);
  const cases = [
    [
      { BROWSER_CHECK_REPLAY_MANIFEST: undefined },
      /requires BROWSER_CHECK_REPLAY_MANIFEST/,
    ],
    [{ BROWSER_CHECK_REPLAY_MANIFEST: "manifest.json" }, /absolute path/],
    [{ BROWSER_CHECK_AGENT: "home" }, /rust or dispatch/],
    [{ BROWSER_CHECK_AGENT: undefined }, /rust or dispatch/],
    [{ BROWSER_CHECK_BARGE_AUDIO_FILE: "/x.wav" }, /does not apply/],
    [{ BROWSER_CHECK_RUN_MANIFEST: undefined }, /budgeted run/],
    [{ BROWSER_CHECK_CAPTURE: undefined }, /requires BROWSER_CHECK_CAPTURE/],
    [{ CODETRIAL_CONFIG_ENV: undefined }, /isolated config/],
    [{ CODETRIAL_CONFIG_ENV: "/no/such/config.env" }, /existing file/],
  ];
  for (const [change, expected] of cases) {
    const errors = validateReplayEnvironment({ ...env, ...change });
    assert.ok(
      errors.some((message) => expected.test(message)),
      `${JSON.stringify(change)}: ${errors}`,
    );
  }
  // The budget's action list is judged now, not once the run has joined.
  for (const [actions, expected] of [
    [["candidate_join"], /replay_inject/],
    [["replay_inject"], /candidate_join/],
  ]) {
    writeRun(actions);
    assert.ok(
      validateReplayEnvironment(env).some((message) => expected.test(message)),
      JSON.stringify(actions),
    );
  }
  fs.writeFileSync(run, "not json");
  assert.ok(
    validateReplayEnvironment(env).some((message) =>
      /could not be read as JSON/.test(message),
    ),
  );
  writeRun(["candidate_join", "replay_inject"]);
  // browser-check supplies no observer, so a hint or tool clip would join the
  // room and spend a permit before it could be refused.
  for (const gate of [
    { state: "hint", tool: "log_hint" },
    { state: "tool", tool: "run_tests" },
  ]) {
    fs.writeFileSync(file, JSON.stringify(manifestFor(wav(), gate)));
    assert.ok(
      validateReplayEnvironment(env).some((message) =>
        /needs an observer/.test(message),
      ),
      JSON.stringify(gate),
    );
  }
  fs.writeFileSync(file, JSON.stringify({ ...good, id: "Bad Id" }));
  assert.ok(
    validateReplayEnvironment(env).some((message) =>
      message.startsWith("replay fixture"),
    ),
    "an invalid fixture is refused before any connection",
  );
  // Another flow may not carry a replay manifest, and otherwise owes nothing.
  assert.deepEqual(
    validateReplayEnvironment({ BROWSER_CHECK_FLOW: "voice" }),
    [],
  );
  assert.match(
    validateReplayEnvironment({
      BROWSER_CHECK_FLOW: "voice",
      BROWSER_CHECK_REPLAY_MANIFEST: file,
    })[0],
    /applies to BROWSER_CHECK_FLOW=named-replay, not voice/,
  );
});

test("a Speaking gate injects once and records format, gate and clocks", async (t) => {
  const fixture = loaded(t);
  const p = pageDouble(fixture);
  await p.ready();
  p.state("Speaking");
  const c = clockPair();
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    admit: async () => {},
    ...c,
  });
  assert.equal(record.trial.excluded, null);
  assert.equal(record.trial.injected, 1);
  assert.equal(p.played(), 1);
  assert.equal(record.fixture.id, "short-backchannel");
  assert.equal(record.fixture.manifest_sha256, fixture.manifest_sha256);
  assert.deepEqual(record.audio, {
    sample_rate_hz: 48000,
    duration_s: 0.2,
    frames: 9600,
    channels: 1,
  });
  assert.equal(record.gate.state, "speaking");
  assert.equal(record.gate.observed_state, "Speaking");
  assert.equal(record.gate.source, "page-text");
  assert.equal(record.injection.page_clock.id, "page.performance");
  assert.equal(record.injection.page_clock.start_ms, 1000);
  assert.equal(record.injection.page_clock.planned_end_ms, 1200);
  assert.equal(record.injection.page_clock.ended_ms, null);
  assert.equal(record.injection.node_clock.id, "node.performance");
  p.tick(200);
  p.ended();
  const settled = await p.page.evaluate(() => globalThis.namedReplay.attempt());
  assert.equal(settled.ended_ms, 1200);
});

test("a gate that never opens times out without decoding or injecting", async (t) => {
  const fixture = loaded(t);
  const p = pageDouble(fixture);
  await p.ready();
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    admit: async () => {},
    ...clockPair(),
  });
  assert.equal(record.trial.excluded, "gate-timeout");
  assert.equal(record.trial.injected, 0);
  assert.equal(p.decoded(), 0);
  assert.equal(p.played(), 0);
  assert.ok(record.gate.wait_ms >= 1000);
});

test("a Listening gate matches Listening and not Speaking", async (t) => {
  const fixture = loaded(t, { state: "listening" });
  const p = pageDouble(fixture);
  await p.ready();
  p.state("Speaking");
  const speaking = await runNamedReplay({
    page: p.page,
    fixture,
    admit: async () => {},
    ...clockPair(),
  });
  assert.equal(speaking.trial.excluded, "gate-timeout");
  assert.equal(p.played(), 0);
  p.state("Listening");
  const listening = await runNamedReplay({
    page: p.page,
    fixture,
    admit: async () => {},
    ...clockPair(),
  });
  assert.equal(listening.trial.injected, 1);
  assert.equal(p.played(), 1);
});

test("a state change during decode excludes the trial as misaligned", async (t) => {
  const fixture = loaded(t);
  const p = pageDouble(fixture);
  await p.ready();
  p.state("Speaking");
  p.onDecode(() => p.state("Listening"));
  let admitted = 0;
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    admit: async () => {
      admitted++;
    },
    ...clockPair(),
  });
  assert.equal(record.trial.excluded, "misaligned");
  assert.equal(p.played(), 0);
  assert.equal(record.injection, null);
  assert.equal(admitted, 0, "no budget is spent on a closed gate");
});

test("a state change during budget admission excludes the trial", async (t) => {
  const fixture = loaded(t);
  const p = pageDouble(fixture);
  await p.ready();
  p.state("Speaking");
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    admit: async () => p.state("Listening"),
    ...clockPair(),
  });
  assert.equal(record.trial.excluded, "misaligned");
  assert.equal(p.played(), 0);
});

test("a refused budget admission excludes the trial as unavailable", async (t) => {
  const fixture = loaded(t);
  const p = pageDouble(fixture);
  await p.ready();
  p.state("Speaking");
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    admit: async () => {
      throw new Error("budget exhausted");
    },
    ...clockPair(),
  });
  assert.equal(record.trial.excluded, "unavailable");
  assert.match(record.trial.reason, /budget exhausted/);
  assert.equal(p.played(), 0);
});

test("a page with no fresh microphone excludes the trial", async (t) => {
  const fixture = loaded(t);
  const p = pageDouble(fixture, { microphone: false });
  p.state("Speaking");
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    admit: async () => {},
    ...clockPair(),
  });
  assert.equal(record.trial.excluded, "missing-microphone");
  assert.equal(p.played(), 0);
});

test("a second injection on the same page is refused", async (t) => {
  const fixture = loaded(t);
  const p = pageDouble(fixture);
  await p.ready();
  p.state("Speaking");
  const first = await runNamedReplay({
    page: p.page,
    fixture,
    admit: async () => {},
    ...clockPair(),
  });
  assert.equal(first.trial.injected, 1);
  const second = await runNamedReplay({
    page: p.page,
    fixture,
    admit: async () => {},
    ...clockPair(),
  });
  assert.equal(second.trial.excluded, "missing-microphone");
  assert.equal(p.played(), 1);
});

test("a hint gate without an observer is unsupported and touches nothing", async (t) => {
  const fixture = loaded(t, { state: "hint", tool: "log_hint" });
  const p = pageDouble(fixture);
  await p.ready();
  p.state("Speaking");
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    admit: async () => {},
    ...clockPair(),
  });
  assert.equal(record.trial.excluded, "unsupported_gate");
  assert.equal(p.calls.length, 0);
  assert.equal(p.played(), 0);
});

test("a hint gate injects only on a fresh recheck taken after admission", async (t) => {
  const fixture = loaded(t, { state: "hint", tool: "log_hint" });
  const p = pageDouble(fixture);
  await p.ready();
  const order = [];
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    recheck: async () => {
      order.push("recheck");
      return { ok: true, age_ms: 40 };
    },
    admit: async () => {
      order.push("admit");
    },
    ...clockPair(),
  });
  assert.equal(record.trial.injected, 1);
  assert.equal(record.gate.source, "recheck");
  assert.equal(record.gate.age_ms, 40);
  // Seen to open the gate, after decode before admission, and once more after
  // admission, immediately before injection.
  assert.deepEqual(order, ["recheck", "recheck", "admit", "recheck"]);
  assert.equal(typeof record.gate.recheck_to_ack_ms, "number");
});

test("a hint that goes stale during admission is not injected", async (t) => {
  const fixture = loaded(t, { state: "hint", tool: "log_hint" });
  const p = pageDouble(fixture);
  await p.ready();
  let admitted = false;
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    recheck: async () =>
      admitted ? { ok: false, age_ms: null } : { ok: true, age_ms: 10 },
    admit: async () => {
      admitted = true;
    },
    ...clockPair(),
  });
  assert.equal(record.trial.excluded, "misaligned");
  assert.equal(p.played(), 0);
});

test("a hint older than the fixture maximum age is not injected", async (t) => {
  const fixture = loaded(t, { state: "hint", tool: "log_hint" });
  const p = pageDouble(fixture);
  await p.ready();
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    recheck: async () => ({ ok: true, age_ms: 251 }),
    admit: async () => {},
    ...clockPair(),
  });
  assert.equal(record.trial.excluded, "gate-timeout");
  assert.equal(p.played(), 0);
});

test("an unknown or null hint age is never fresh", async (t) => {
  const fixture = loaded(t, { state: "hint", tool: "log_hint" });
  for (const age_ms of [null, undefined, -1, Number.NaN]) {
    const p = pageDouble(fixture);
    await p.ready();
    const record = await runNamedReplay({
      page: p.page,
      fixture,
      recheck: async () => ({ ok: true, age_ms }),
      admit: async () => {},
      ...clockPair(),
    });
    assert.equal(record.trial.excluded, "gate-timeout", String(age_ms));
    assert.equal(p.played(), 0);
  }
});

test("a throwing observer or page keeps the attempt", async (t) => {
  const fixture = loaded(t, { state: "hint", tool: "log_hint" });
  const p = pageDouble(fixture);
  await p.ready();
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    recheck: async () => {
      throw new Error("log unreadable");
    },
    admit: async () => {},
    ...clockPair(),
  });
  assert.equal(record.trial.started, 1);
  assert.equal(record.trial.excluded, "unavailable");
  assert.match(record.trial.reason, /log unreadable/);
  assert.equal(record.trial.injected, 0);
});

test("a failure of the inject call leaves injection unknown", async (t) => {
  const fixture = loaded(t);
  const p = pageDouble(fixture);
  await p.ready();
  p.state("Speaking");
  const page = {
    async evaluate(fn, arg) {
      if (String(fn).includes("inject(")) throw new Error("page crashed");
      return p.page.evaluate(fn, arg);
    },
  };
  const record = await runNamedReplay({
    page,
    fixture,
    admit: async () => {},
    ...clockPair(),
  });
  assert.equal(record.trial.excluded, "unavailable");
  assert.equal(record.trial.injected, null);
  assert.match(record.trial.reason, /page crashed/);
});

test("a hint that goes stale during decode spends no budget", async (t) => {
  const fixture = loaded(t, { state: "hint", tool: "log_hint" });
  const p = pageDouble(fixture);
  await p.ready();
  let stale = false;
  p.onDecode(() => {
    stale = true;
  });
  let admitted = 0;
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    recheck: async () =>
      stale ? { ok: false, age_ms: null } : { ok: true, age_ms: 10 },
    admit: async () => {
      admitted++;
    },
    ...clockPair(),
  });
  assert.equal(record.trial.excluded, "misaligned");
  assert.equal(admitted, 0, "no budget is spent on a closed gate");
  assert.equal(p.played(), 0);
});

test("an observation that returns after the deadline does not open the gate", async (t) => {
  const fixture = loaded(t, { state: "hint", tool: "log_hint" });
  const p = pageDouble(fixture);
  await p.ready();
  const c = clockPair();
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    recheck: async () => {
      c.advance(1500);
      return { ok: true, age_ms: 10 };
    },
    admit: async () => {},
    ...c,
  });
  assert.equal(record.trial.excluded, "gate-timeout");
  assert.equal(p.played(), 0);
  assert.equal(p.decoded(), 0);
});

test("settling the record adds the actual end once the source ends", async (t) => {
  const fixture = loaded(t);
  const p = pageDouble(fixture);
  await p.ready();
  p.state("Speaking");
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    admit: async () => {},
    ...clockPair(),
  });
  await settleNamedReplay(p.page, record);
  assert.equal(record.injection.page_clock.ended_ms, null);
  p.tick(200);
  p.ended();
  await settleNamedReplay(p.page, record);
  assert.equal(record.injection.page_clock.ended_ms, 1200);
  const excluded = { trial: { excluded: "gate-timeout" }, injection: null };
  assert.equal(await settleNamedReplay(p.page, excluded), excluded);
});

test("the page half replaces only audio tracks, silences them and feeds every output", async (t) => {
  const f = pageDouble(loaded(t));
  await f.ready();
  assert.equal(f.stopped(), 1, "the original microphone track is stopped");
  assert.deepEqual(f.tracks, [f.videoTrack, f.replacement]);
  // A request with no audio is left alone.
  await f.sandbox.navigator.mediaDevices.getUserMedia({ video: true });
  assert.equal(f.stopped(), 1);
  assert.deepEqual(f.tracks, [f.videoTrack, f.replacement]);
  // A second acquisition has its own output, and both receive the clip.
  await f.sandbox.navigator.mediaDevices.getUserMedia({ audio: true });
  f.state("Speaking");
  await f.page.evaluate(() => globalThis.namedReplay.prepare());
  await f.page.evaluate(() => globalThis.namedReplay.inject("speaking"));
  assert.equal(f.connects(), 2);
  assert.equal(f.played(), 1);
});

test("inject answers in the same turn it reads the state", async (t) => {
  const f = pageDouble(loaded(t));
  await f.ready();
  f.state("Speaking");
  await f.page.evaluate(() => globalThis.namedReplay.prepare());
  // An async inject would hand back a promise, and an await before start()
  // would let the page change under it.
  const result = f.sandbox.namedReplay.inject("speaking");
  assert.equal(typeof result.then, "undefined");
  assert.equal(result.observed_state, "Speaking");
  assert.equal(f.played(), 1);
});

test("a context that never resumes or a clip that will not decode is an exclusion", async (t) => {
  const hang = pageDouble(loaded(t), { prepareTimeoutMs: 20 });
  await hang.ready();
  hang.state("Speaking");
  hang.hangResume();
  const hung = await runNamedReplay({
    page: hang.page,
    fixture: loaded(t),
    admit: async () => {},
    ...clockPair(),
  });
  assert.equal(hung.trial.excluded, "decode-or-resume-failed");
  assert.equal(hung.trial.admitted, 0);
  assert.equal(hang.played(), 0);

  const bad = pageDouble(loaded(t));
  await bad.ready();
  bad.state("Speaking");
  bad.rejectDecode();
  const undecodable = await runNamedReplay({
    page: bad.page,
    fixture: loaded(t),
    admit: async () => {},
    ...clockPair(),
  });
  assert.equal(undecodable.trial.excluded, "decode-or-resume-failed");
  assert.equal(bad.played(), 0);
});

test("the record carries what the judge reads", async (t) => {
  const fixture = loaded(t);
  const p = pageDouble(fixture);
  await p.ready();
  const c = clockPair();
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    admit: async () => {},
    now: c.now,
    // The gate opens during the second poll, so the wait is measured.
    sleep: async (ms) => {
      await c.sleep(ms);
      p.state("Speaking");
    },
  });
  assert.equal(record.fixture.audio_sha256, fixture.manifest.audio.sha256);
  assert.deepEqual(record.fixture.expected, {
    kind: "overlap",
    outcome: "continued",
  });
  assert.equal(record.gate.wait_ms, 100);
  assert.equal(record.trial.admitted, 1);
  assert.equal(record.injection.node_clock.ack_ms >= 100, true);
});

test("a hint record keeps the page text and the recheck-to-ack gap", async (t) => {
  const fixture = loaded(t, { state: "hint", tool: "log_hint" });
  const p = pageDouble(fixture);
  await p.ready();
  const c = clockPair();
  const page = {
    async evaluate(fn, arg) {
      const result = await p.page.evaluate(fn, arg);
      if (String(fn).includes("inject(")) c.advance(7);
      return result;
    },
  };
  const record = await runNamedReplay({
    page,
    fixture,
    recheck: async () => ({ ok: true, age_ms: 10 }),
    admit: async () => {},
    ...c,
  });
  assert.equal(record.gate.observed_state, "Listening");
  assert.equal(record.gate.recheck_to_ack_ms, 7);
});

test("the record is kept from the start, so a failure mid-trial leaves the attempt", async (t) => {
  const fixture = loaded(t);
  const p = pageDouble(fixture);
  await p.ready();
  p.state("Speaking");
  const kept = [];
  let seenInAdmit;
  const record = await runNamedReplay({
    page: p.page,
    fixture,
    onStart: (started) => kept.push(started),
    admit: async () => {
      seenInAdmit = JSON.parse(JSON.stringify(kept[0]));
    },
    ...clockPair(),
  });
  assert.equal(kept.length, 1);
  assert.equal(kept[0], record, "the same object is kept and returned");
  assert.equal(seenInAdmit.trial.started, 1);
  assert.equal(seenInAdmit.trial.admitted, 0);
  assert.notEqual(seenInAdmit.audio, null, "decode was already recorded");
  assert.equal(seenInAdmit.injection, null);
});

function flow(t, { gate, permit, inject } = {}) {
  const fixture = loaded(t, gate);
  const p = pageDouble(fixture);
  const kept = [];
  const waits = [];
  const permits = [];
  const page = inject
    ? {
        evaluate(fn, arg) {
          if (String(fn).includes("inject(")) return inject();
          return p.page.evaluate(fn, arg);
        },
      }
    : p.page;
  const run = async () =>
    namedReplayFlow({
      page,
      fixture,
      runBudget: {
        permit:
          permit ??
          (async (action) => {
            permits.push(action);
          }),
      },
      keep: (record) => kept.push(JSON.parse(JSON.stringify(record))),
      wait: async (ms) => waits.push(ms),
      redact: (text) => text.replaceAll("secret", "[redacted]"),
      ...clockPair(),
    });
  return { p, kept, waits, permits, run };
}

test("the flow spends one replay_inject permit, waits out the window and settles", async (t) => {
  const f = flow(t);
  await f.p.ready();
  f.p.state("Speaking");
  const { record, exitCode } = await f.run();
  assert.deepEqual(f.permits, ["replay_inject"]);
  assert.equal(exitCode, 0);
  // The window starts after the clip's last frame: 200 ms of audio, then 5 s.
  assert.deepEqual(f.waits, [5200]);
  assert.equal(record.trial.injected, 1);
  assert.equal(
    f.kept.length >= 3,
    true,
    "kept at the start and after each change",
  );
  assert.equal(f.kept[0].trial.injected, 0);
});

test("an excluded trial exits 77 and an injected one exits 0", async (t) => {
  const f = flow(t);
  await f.p.ready();
  // Listening, never Speaking: the gate times out.
  const { record, exitCode } = await f.run();
  assert.equal(record.trial.excluded, "gate-timeout");
  assert.equal(exitCode, 77);
  assert.deepEqual(f.waits, [], "no window after a trial that never injected");
  assert.deepEqual(f.permits, []);
});

test("a refused permit is rethrown for the budget exit after the record is kept", async (t) => {
  const f = flow(t, {
    permit: async () => {
      throw new BudgetDenied("action ceiling reached");
    },
  });
  await f.p.ready();
  f.p.state("Speaking");
  await assert.rejects(f.run(), BudgetDenied);
  const last = f.kept.at(-1);
  assert.equal(last.trial.excluded, "unavailable");
  // A refused or lost permit may still have been counted by the launcher.
  assert.equal(last.trial.admitted, null);
  assert.deepEqual(f.waits, []);
});

test("another failure to admit is an exclusion, not a budget exit", async (t) => {
  const f = flow(t, {
    permit: async () => {
      throw new Error("socket reset");
    },
  });
  await f.p.ready();
  f.p.state("Speaking");
  const { record, exitCode } = await f.run();
  assert.equal(record.trial.excluded, "unavailable");
  assert.equal(record.trial.admitted, null, "a failed call may have counted");
  assert.equal(exitCode, 77);
});

test("a lost injection acknowledgement still observes through the possible window", async (t) => {
  const f = flow(t, {
    inject: async () => {
      throw new Error("page crashed");
    },
  });
  await f.p.ready();
  f.p.state("Speaking");
  const { record, exitCode } = await f.run();
  assert.equal(record.trial.injected, null);
  assert.equal(exitCode, 77);
  assert.deepEqual(f.waits, [5200], "the clip may have played");
});

test("the reason kept in the capture is redacted", async (t) => {
  const f = flow(t, {
    permit: async () => {
      throw new Error("token secret rejected");
    },
  });
  await f.p.ready();
  f.p.state("Speaking");
  const { record } = await f.run();
  assert.equal(record.trial.reason, "token [redacted] rejected");
  assert.equal(JSON.stringify(f.kept.at(-1)).includes("secret"), false);
});
