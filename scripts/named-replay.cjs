// Named-fixture browser replay (docs/observational-replay.md). It is an
// observation input: it injects a validated clip at a stated gate and records
// what it saw, and never judges an outcome or changes provider policy. The
// interruption-only smoke in barge-replay.cjs stays separate.

const fs = require("node:fs");
const path = require("node:path");
const { loadFixture } = require("./replay-fixture.cjs");
const { BudgetDenied } = require("./run-budget-guard.cjs");

// The run-budget action the injection is permitted as. A manifest lists it
// beside candidate_join; the launcher accepts any plain action ID.
const REPLAY_ACTION = "replay_inject";
const JOIN_ACTION = "candidate_join";
// A trial that was excluded observed nothing it can be judged on: neither a
// pass nor a failure of the model.
const REPLAY_EXCLUDED_EXIT = 77;
// The page shows only these two states; a hint or tool gate has no page
// observable and needs a caller-supplied observer.
const PAGE_GATES = new Set(["speaking", "listening"]);

// Injected before page code, serialised with toString like installBargeReplay,
// so everything it uses is declared inside it. Microphone tracks stay silent
// until inject(); each acquisition has its own output track, as in a browser.
// Results are plain objects rather than throws, which Playwright would flatten.
function installNamedReplay(wavBase64, prepareTimeoutMs = 10000) {
  const media = navigator.mediaDevices;
  const original = media.getUserMedia.bind(media);
  const outputs = [];
  let context;
  let buffer = null;
  let played = false;
  let attempt = null;
  media.getUserMedia = async (constraints) => {
    const stream = await original(constraints);
    if (constraints?.audio) {
      context ??= new AudioContext();
      const destination = context.createMediaStreamDestination();
      for (const track of stream.getAudioTracks()) {
        stream.removeTrack(track);
        track.stop();
      }
      stream.addTrack(destination.stream.getAudioTracks()[0]);
      outputs.push(destination);
    }
    return stream;
  };
  const agentState = () =>
    document.querySelector("#agent-state")?.textContent ?? "";
  // A suspended context never settles resume(); the harness must not wait on
  // it for the whole run.
  const bounded = (promise) => {
    let timer;
    return Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(new Error("timed out")),
          prepareTimeoutMs,
        );
      }),
    ]).finally(() => clearTimeout(timer));
  };
  const pageText = { speaking: "Speaking", listening: "Listening" };
  globalThis.namedReplay = {
    observe: () => ({ state: agentState(), at: performance.now() }),
    async prepare() {
      if (!context || !outputs.length || played)
        return { excluded: "missing-microphone" };
      try {
        const bytes = Uint8Array.from(atob(wavBase64), (ch) =>
          ch.charCodeAt(0),
        );
        buffer = await bounded(context.decodeAudioData(bytes.buffer));
        await bounded(context.resume());
      } catch {
        return { excluded: "decode-or-resume-failed" };
      }
      return {
        sampleRate: buffer.sampleRate,
        duration: buffer.duration,
        frames: buffer.length,
        channels: buffer.numberOfChannels,
      };
    },
    // Synchronous on purpose: the state read and source.start() share a turn,
    // so no other script on the page runs between the last check and the
    // injection.
    inject(state) {
      const observedState = agentState();
      const wanted = pageText[state];
      if (wanted && !observedState.includes(wanted))
        return { excluded: "misaligned", observedState };
      if (!buffer || played) return { excluded: "missing-microphone" };
      const source = context.createBufferSource();
      source.buffer = buffer;
      for (const destination of outputs) source.connect(destination);
      played = true;
      const startMs = performance.now();
      attempt = {
        observed_state: observedState,
        start_ms: startMs,
        planned_end_ms: startMs + buffer.duration * 1000,
        ended_ms: null,
      };
      source.onended = () => {
        attempt.ended_ms = performance.now();
      };
      source.start();
      return attempt;
    },
    attempt: () => attempt,
  };
}

const isFile = (name) => {
  try {
    return Boolean(name) && fs.statSync(name).isFile();
  } catch {
    return false;
  }
};

// The actions a replay spends: the join, then the injection.
const actionErrors = (actions) =>
  [JOIN_ACTION, REPLAY_ACTION]
    .filter((action) => !actions?.includes(action))
    .map(
      (action) =>
        `the run manifest must list the ${action} action for named replay`,
    );

// What a live replay needs before it joins a room, as reasons it would be
// refused. `runBudget` is the joined budget, or undefined when only the
// environment can be judged so far.
function replayInputErrors({ env, runBudget }) {
  const errors = [];
  if (!isFile(env.CODETRIAL_CONFIG_ENV))
    errors.push(
      "named replay requires an isolated config (CODETRIAL_CONFIG_ENV naming an existing file)",
    );
  if (runBudget === null)
    errors.push("named replay requires a joined run budget");
  else if (runBudget) errors.push(...actionErrors(runBudget.actions));
  return errors;
}

// Throws when a live replay may not join: browser-check.cjs calls it with the
// joined budget, ahead of any browser or agent.
function requireNamedReplayInputs({ env, runBudget }) {
  const errors = replayInputErrors({ env, runBudget });
  if (errors.length) throw new Error(errors.join("; "));
}

// The environment rules of BROWSER_CHECK_FLOW=named-replay, judged before
// anything is built or started and shared by browser-check.sh and
// browser-check.cjs. Returns the reasons it is refused, empty when it is not.
// The fixture is read here too: an invalid one refuses before any connection.
function validateReplayEnvironment(env) {
  const flow = env.BROWSER_CHECK_FLOW ?? "voice";
  const manifest = env.BROWSER_CHECK_REPLAY_MANIFEST;
  if (flow !== "named-replay")
    return manifest === undefined
      ? []
      : [
          `BROWSER_CHECK_REPLAY_MANIFEST applies to BROWSER_CHECK_FLOW=named-replay, not ${flow}`,
        ];
  const errors = [];
  if (!manifest || !path.isAbsolute(manifest))
    errors.push(
      "BROWSER_CHECK_FLOW=named-replay requires BROWSER_CHECK_REPLAY_MANIFEST, an absolute path",
    );
  else {
    try {
      const { gate } = loadFixture(manifest, "browser").manifest;
      if (!PAGE_GATES.has(gate.state))
        errors.push(
          `gate state ${gate.state} needs an observer that browser-check does not supply; use speaking or listening`,
        );
    } catch (error) {
      errors.push(error.message);
    }
  }
  if (!env.BROWSER_CHECK_CAPTURE)
    errors.push(
      "BROWSER_CHECK_FLOW=named-replay requires BROWSER_CHECK_CAPTURE, where it keeps its record",
    );
  if (!["rust", "dispatch"].includes(env.BROWSER_CHECK_AGENT ?? "home"))
    errors.push(
      "BROWSER_CHECK_FLOW=named-replay needs BROWSER_CHECK_AGENT=rust or dispatch, whose floor records it retains",
    );
  if (env.BROWSER_CHECK_BARGE_AUDIO_FILE)
    errors.push(
      "BROWSER_CHECK_BARGE_AUDIO_FILE does not apply to named-replay, whose clip is in the manifest",
    );
  if (!env.BROWSER_CHECK_RUN_MANIFEST)
    errors.push(
      "named replay needs a budgeted run: start it through scripts/interview-run-budget.py run",
    );
  else {
    // The launcher hashed this file; its action list is judged here, before
    // anything is built, rather than only once the budget is joined.
    try {
      const budget = JSON.parse(
        fs.readFileSync(env.BROWSER_CHECK_RUN_MANIFEST, "utf8"),
      );
      errors.push(...actionErrors(budget.actions));
    } catch {
      errors.push("the run manifest could not be read as JSON");
    }
  }
  errors.push(...replayInputErrors({ env, runBudget: undefined }));
  return errors;
}

const POLL_MS = 100;

// `recheck` reports a hint or tool gate: () => { ok, age_ms }. A caller that
// has none cannot replay those gates, so the trial is refused untouched.
// `admit` spends the run budget for the injection. Both are awaited before the
// final check, because anything asynchronous can outlast the gate. `onStart`
// receives the record as soon as it exists; the same object is returned.
async function runNamedReplay({
  page,
  fixture,
  admit,
  recheck = null,
  onStart = () => {},
  now = () => performance.now(),
  sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
}) {
  const { gate, audio } = fixture.manifest;
  const record = {
    fixture: {
      id: fixture.manifest.id,
      manifest_sha256: fixture.manifest_sha256,
      audio_sha256: audio.sha256,
      expected: fixture.manifest.expected,
    },
    // `admitted` says whether the replay_inject permit was spent, which the
    // record of an excluded trial must not leave to be guessed: 1 once the
    // launcher acknowledged it, 0 while no permit call was made, null when the
    // call failed, because a lost reply can follow a permit already counted.
    trial: {
      started: 1,
      admitted: 0,
      injected: 0,
      excluded: null,
      reason: null,
    },
    gate: {
      state: gate.state,
      source: PAGE_GATES.has(gate.state) ? "page-text" : "recheck",
      observed_state: null,
      age_ms: null,
      wait_ms: null,
      recheck_to_ack_ms: null,
    },
    audio: null,
    injection: null,
  };
  // The caller can keep the record from here on, so a failure anywhere after
  // this point still leaves the attempt it was in.
  onStart(record);
  const exclude = (code, reason = null) => {
    record.trial.excluded = code;
    record.trial.reason = reason;
    return record;
  };
  // A missing or null age is unknown, never fresh: null <= max is true in JS.
  const fresh = (result) =>
    Boolean(result?.ok) &&
    Number.isFinite(result.age_ms) &&
    result.age_ms >= 0 &&
    result.age_ms <= gate.max_age_ms;
  const matches = (observed) =>
    observed.state.includes(
      gate.state === "speaking" ? "Speaking" : "Listening",
    );

  if (!PAGE_GATES.has(gate.state) && !recheck)
    return exclude("unsupported_gate");

  // One reading of the gate, whichever way it is observed: the page text for
  // speaking and listening, the caller's observer for a hint or tool.
  const gateHolds = async () => {
    if (PAGE_GATES.has(gate.state)) {
      const observed = await page.evaluate(() =>
        globalThis.namedReplay.observe(),
      );
      return matches(observed)
        ? { ok: true, age_ms: 0, state: observed.state }
        : { ok: false };
    }
    const seen = await recheck();
    return fresh(seen) ? { ok: true, age_ms: seen.age_ms } : { ok: false };
  };

  // A throw from the page or the observer must not lose the attempt.
  let injectCalled = false;
  try {
    const waitStart = now();
    for (;;) {
      const held = await gateHolds();
      // An observation that returned after the deadline does not open the gate.
      if (held.ok && now() - waitStart <= gate.timeout_ms) {
        record.gate.age_ms = held.age_ms;
        if (held.state) record.gate.observed_state = held.state;
        break;
      }
      if (now() - waitStart >= gate.timeout_ms) {
        record.gate.wait_ms = now() - waitStart;
        return exclude("gate-timeout");
      }
      await sleep(POLL_MS);
    }
    record.gate.wait_ms = now() - waitStart;

    const prepared = await page.evaluate(() =>
      globalThis.namedReplay.prepare(),
    );
    if (prepared.excluded) return exclude(prepared.excluded);
    record.audio = {
      sample_rate_hz: prepared.sampleRate,
      duration_s: prepared.duration,
      frames: prepared.frames,
      channels: prepared.channels,
    };

    // Decoding and resume are asynchronous; do not spend budget on a gate that
    // already closed.
    if (!(await gateHolds()).ok) return exclude("misaligned");

    try {
      await admit();
    } catch (error) {
      record.trial.admitted = null;
      return exclude("unavailable", error.message);
    }
    record.trial.admitted = 1;

    // Admission is asynchronous too: check again immediately before injecting.
    let recheckedAt = null;
    if (!PAGE_GATES.has(gate.state)) {
      const held = await gateHolds();
      recheckedAt = now();
      if (!held.ok) return exclude("misaligned");
      record.gate.age_ms = held.age_ms;
    }
    const injectedAt = now();
    injectCalled = true;
    const attempt = await page.evaluate(
      (state) => globalThis.namedReplay.inject(state),
      gate.state,
    );
    if (attempt.excluded) return exclude(attempt.excluded);
    if (recheckedAt !== null)
      record.gate.recheck_to_ack_ms = now() - recheckedAt;
    record.gate.observed_state = attempt.observed_state;
    record.trial.injected = 1;
    record.injection = {
      page_clock: {
        id: "page.performance",
        start_ms: attempt.start_ms,
        planned_end_ms: attempt.planned_end_ms,
        ended_ms: attempt.ended_ms,
      },
      node_clock: {
        id: "node.performance",
        request_ms: injectedAt,
        ack_ms: now(),
      },
    };
    return record;
  } catch (error) {
    // After the inject call went out, whether it played is unknown.
    if (injectCalled) record.trial.injected = null;
    return exclude("unavailable", String(error?.message ?? error));
  }
}

// The page learns when the clip ended only after the call that injected it has
// returned. The harness calls this once the observation window is over and the
// record gets the actual end, or null when the source never ended.
async function settleNamedReplay(page, record) {
  if (!record.injection) return record;
  const attempt = await page.evaluate(() => globalThis.namedReplay.attempt());
  record.injection.page_clock.ended_ms = attempt?.ended_ms ?? null;
  return record;
}

// The whole body of BROWSER_CHECK_FLOW=named-replay, apart from the page and
// the capture file, so its exit rules can be driven without a browser. `keep`
// is given the record at the start and after every change, which is how a
// failure mid-trial still leaves the attempt in the capture. A refused permit
// is rethrown, after the record is kept, for the caller's budget exit; any
// other exclusion is returned with the exit code that says nothing was
// observed. The observation window starts after the clip's last frame, and
// still runs when the injection call's acknowledgement was lost, since the
// clip may have played.
async function namedReplayFlow({
  page,
  fixture,
  runBudget,
  keep,
  wait,
  redact = (text) => text,
  now,
  sleep,
}) {
  let denied = null;
  const record = await runNamedReplay({
    page,
    fixture,
    now,
    sleep,
    onStart: keep,
    admit: async () => {
      try {
        await runBudget.permit(REPLAY_ACTION);
      } catch (error) {
        if (error instanceof BudgetDenied) denied = error;
        throw error;
      }
    },
  });
  // The reason is a raw message from the page or the budget socket.
  if (record.trial.reason) record.trial.reason = redact(record.trial.reason);
  keep(record);
  if (denied) throw denied;
  if (record.injection || record.trial.injected === null) {
    await wait(
      Math.ceil((record.audio?.duration_s ?? 0) * 1000) +
        fixture.manifest.observation_ms,
    );
    if (record.injection) {
      await settleNamedReplay(page, record);
      keep(record);
    }
  }
  return {
    record,
    exitCode: record.trial.excluded ? REPLAY_EXCLUDED_EXIT : 0,
  };
}

module.exports = {
  REPLAY_ACTION,
  REPLAY_EXCLUDED_EXIT,
  installNamedReplay,
  requireNamedReplayInputs,
  validateReplayEnvironment,
  namedReplayFlow,
  runNamedReplay,
  settleNamedReplay,
};
