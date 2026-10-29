// The long-interview driver, run against a fake controller and a virtual
// clock. No browser, no network and no model: these prove the wiring (what is
// dispatched through the budget, which timestamps are recorded, which refusals
// and failure classes fire), not that a real interview behaves. A real browser
// run of the long interview is a separate check.
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import driver from "../../scripts/long-interview-driver.cjs";
import page_ from "../../scripts/long-interview-page.cjs";
import sched from "../../scripts/long-interview-schedule.cjs";
import guard_ from "../../scripts/run-budget-guard.cjs";
import timing_ from "../../scripts/long-interview-timing.cjs";

const {
  CLIP_SOURCE,
  DriverRefused,
  finalizeLongRun,
  prepareLongRun,
  runLongInterview,
  writeArtifacts,
} = driver;
const { BudgetDenied, RunBudget } = guard_;
const { EXIT } = timing_;

const ROOT = fileURLToPath(new URL("../..", import.meta.url));
const T0 = 1_700_000_000_000;

const SMOKE = {
  runKind: "smoke",
  endMode: "manual",
  durationMin: 10,
  observationTargetS: 600,
  runId: "r1",
};
const DEADLINE = {
  runKind: "smoke",
  endMode: "server_deadline",
  durationMin: 10,
  observationTargetS: 720,
  runId: "r1",
};
const SHORT = { ...SMOKE, observationTargetS: 480 };
const QUALIFICATION = {
  runKind: "qualification",
  endMode: "manual",
  durationMin: 90,
  observationTargetS: 5400,
  runId: "r1",
};
const TONES = { BROWSER_CHECK_LONG_TONE_FIXTURE: "1" };

/// A budget the way the launcher's manifest would pin this schedule.
function budgetFor(plan, env = TONES, extra = {}) {
  const schedule = sched.generateSchedule("coding", {
    durationMin: plan.durationMin,
    observationTargetS: plan.observationTargetS,
  }).schedule;
  const set = sched.syntheticClipSet(schedule);
  const budget = {
    actions: [...schedule.actions],
    fixtureSha256: sched.budgetFields(schedule, set.manifest).fixture_sha256,
    ids: [],
    inGuard: false,
    denyAt: Infinity,
    async guarded(id, dispatch) {
      if (budget.ids.length + 1 >= budget.denyAt) {
        throw new BudgetDenied("action ceiling reached");
      }
      budget.ids.push(id);
      budget.inGuard = true;
      try {
        return await dispatch();
      } finally {
        budget.inGuard = false;
      }
    },
    ...extra,
  };
  return budget;
}

function virtualClock(start = T0) {
  let now = start;
  return {
    now: () => now,
    sleep: async (ms) => {
      now += ms;
    },
    advance: (ms) => {
      now += ms;
    },
  };
}

/// Every method asserts it runs inside a budget permit, so a stimulus that
/// skipped the guard throws instead of being counted.
function fakeController(clock, budget, options = {}) {
  const calls = [];
  const needGuard = (what) => {
    if (!budget.inGuard) throw new Error(`${what} ran outside the budget`);
  };
  let finalAt = null;
  let endAt = null;
  let primeAt = null;
  return {
    calls,
    async playClip(entry, bytes) {
      needGuard(`playClip ${entry.clip_id}`);
      assert.ok(bytes && bytes.length > 44, `${entry.clip_id} has audio`);
      calls.push(["play", entry.clip_id, clock.now()]);
      if (entry.clip_id.endsWith("_prime")) primeAt = clock.now();
      if (entry.clip_id === "final_checkpoint") {
        finalAt = clock.now();
        endAt = finalAt + entry.duration_s * 1000;
      }
      // The page takes a moment to start the clip, which moves the ticks off
      // the whole seconds the schedule lands on.
      if (options.playDelayMs) clock.advance(options.playDelayMs);
    },
    async applyRevision(revision) {
      needGuard("applyRevision");
      calls.push(["revise", revision.index]);
    },
    async setThinking(on) {
      needGuard("setThinking");
      calls.push(["thinking", on]);
    },
    async agentSpeaking() {
      if (options.neverSpeaks) return false;
      return primeAt !== null && clock.now() >= primeAt + 14000;
    },
    async agentReplyCount() {
      if (options.noFinalReply) return 0;
      // A reply that starts while the question is still being asked.
      if (options.earlyReply) {
        return finalAt !== null && clock.now() >= finalAt + 1000 ? 1 : 0;
      }
      if (options.promptReply) {
        return endAt !== null && clock.now() >= endAt + 100 ? 1 : 0;
      }
      return endAt !== null && clock.now() >= endAt + 3000 ? 1 : 0;
    },
    async end() {
      needGuard("end");
      calls.push(["end", clock.now()]);
      return { performanceReceived: true, reportReceived: true };
    },
    async awaitServerDeadline() {
      calls.push(["deadline", clock.now()]);
      return { performanceReceived: true, reportReceived: true };
    },
  };
}

const GOOD_CLEANUP = {
  errors: [],
  leftover: { agents: 0, processes: 0, rooms: 0 },
};
const GOAWAY = [
  {
    trigger: "goaway",
    startInterviewMs: 100000,
    endInterviewMs: 104000,
    result: "installed",
  },
];

async function play(
  plan,
  { env = TONES, budgetOptions, controllerOptions, failure, mutate } = {},
) {
  const budget = budgetFor(plan, env, budgetOptions);
  const prepared = prepareLongRun({ env, plan, budget });
  const clock = virtualClock();
  const controller = fakeController(clock, budget, controllerOptions);
  // The media gate dispatched the join before the driver is called.
  await budget.guarded("candidate_join", async () => {});
  const joinedAtMs = clock.now();
  const run = await runLongInterview({
    plan,
    ...prepared,
    budget,
    controller,
    clock,
    joinedAtMs,
    interviewStartMs: joinedAtMs - 5000,
    joinDispatched: true,
    failure,
  });
  if (mutate) mutate(run);
  clock.advance(1000);
  return { plan, budget, prepared, clock, controller, run, joinedAtMs };
}

function finalize(played, overrides = {}) {
  const { plan, prepared, run, joinedAtMs, clock } = played;
  return finalizeLongRun({
    plan,
    ...prepared,
    run,
    observation: { verdict: "pass", reasons: [] },
    clock: [{ startUnixMs: joinedAtMs - 8000, elapsedMs: 0 }],
    tokenRequestsMs: [joinedAtMs - 8000],
    episodes: GOAWAY,
    logs: { truncated: false, evicted: false, complete: true },
    launchMs: joinedAtMs - 20000,
    teardownEndMs: clock.now(),
    cleanup: GOOD_CLEANUP,
    ...overrides,
  });
}

test("a tone fixture smoke dispatches every scheduled action through the budget and ends once", async () => {
  // Expected: the guard sees candidate_join, then each schedule step in order
  // except none (the fake agent speaks, so the barge is covered), then "end".
  const played = await play(SMOKE);
  const { schedule } = played.prepared;
  assert.equal(played.run.aborted, null);
  assert.deepEqual(played.budget.ids, [...schedule.actions, "end"]);
  assert.deepEqual(played.run.uncovered, []);
  const finished = finalize(played);
  assert.deepEqual(finished.failures, []);
  assert.equal(finished.exitCode, EXIT.pass);
  assert.equal(finished.report.status, "complete");
});

test("the tone fixture can never be reported as a speech run", async () => {
  const finished = finalize(await play(SMOKE));
  assert.equal(finished.report.clip_source, CLIP_SOURCE.toneFixture);
  assert.equal(finished.report.validation_fixture, true);
  assert.equal(finished.report.speech_run, false);
  assert.ok(finished.report.not_exercised.some((n) => /contains none/.test(n)));
});

test("three timestamps are recorded and a full interview is reported only from them", async () => {
  const played = await play(SMOKE);
  const { run, joinedAtMs } = played;
  assert.ok(Number.isFinite(run.lastStimulusAtMs));
  assert.ok(Number.isFinite(run.observationEndMs));
  assert.ok(Number.isFinite(run.reportCompleteMs));
  // The held session reaches the full target on the interview clock, then ends.
  assert.equal(run.observationEndMs - (joinedAtMs - 5000), 600 * 1000);
  assert.ok(run.lastStimulusAtMs < run.observationEndMs);
  const calls = played.controller.calls;
  assert.equal(calls.filter((c) => c[0] === "end").length, 1);
  assert.equal(calls.at(-1)[0], "end");
  const finished = finalize(played);
  assert.equal(finished.report.full_interview, true);
  assert.equal(
    finished.report.exposure_s,
    finished.report.timestamps.observationEndS,
  );
  assert.ok(finished.report.timestamps.lastStimulusS < 480);
});

test("a dropped timestamp fails the run and it is not a full interview", async () => {
  const drops = [
    ["observationEndMs", /never ended|ended/],
    ["reportCompleteMs", /never completed/],
  ];
  for (const [field, reason] of drops) {
    const played = await play(SMOKE, { mutate: (run) => (run[field] = null) });
    const finished = finalize(played);
    assert.notEqual(finished.exitCode, EXIT.pass, field);
    assert.equal(finished.report.full_interview, false, field);
    assert.match(finished.failures.join("; "), reason, field);
  }
  const played = await play(SMOKE);
  const noWall = finalize(played, { teardownEndMs: undefined });
  assert.notEqual(noWall.exitCode, EXIT.pass);
  assert.match(noWall.failures.join("; "), /wall time of the run is unknown/);
  const noAnchor = finalize(played, { tokenRequestsMs: [], clock: [] });
  assert.notEqual(noAnchor.exitCode, EXIT.pass);
  assert.equal(noAnchor.report.full_interview, false);
});

test("a session that stopped early is not reported as a full interview", async () => {
  const played = await play(SMOKE, {
    mutate: (run) => {
      run.observationEndMs = run.joinedAtMs + 300 * 1000;
    },
  });
  const finished = finalize(played);
  assert.equal(finished.exitCode, EXIT.failed);
  assert.equal(finished.report.full_interview, false);
  assert.match(finished.failures.join("; "), /not a full 10 minute interview/);
});

test("a smoke that observes less than the interview says so and is never full", async () => {
  const finished = finalize(await play(SHORT));
  assert.equal(finished.exitCode, EXIT.pass);
  assert.equal(finished.report.full_interview, false);
});

test("a stimulus after the boundary spends the reserve and fails", async () => {
  const played = await play(SMOKE, {
    mutate: (run) => (run.lastStimulusAtMs = run.observationEndMs - 60 * 1000),
  });
  const finished = finalize(played);
  assert.equal(finished.exitCode, EXIT.failed);
  assert.match(finished.failures.join("; "), /spends the reserve/);
});

test("a skipped guarded action is caught as a missing dispatch", async () => {
  const played = await play(SMOKE);
  const { schedule, playback } = played.prepared;
  const skipped = schedule.steps.find((s) => s.kind === "thinking_start");
  const run = {
    ...played.run,
    dispatched: played.run.dispatched.filter(
      (d) => d.action_id !== skipped.action_id,
    ),
  };
  const errors = driver.judgeDispatch(schedule, playback, run);
  assert.deepEqual(errors, [
    `${skipped.action_id}: dispatched 0 times, not once`,
  ]);
  const finished = finalize({ ...played, run });
  assert.equal(finished.exitCode, EXIT.failed);
  assert.equal(finished.report.full_interview, false);
});

test("a clip that was never transmitted fails even when its action was counted", async () => {
  const played = await play(SMOKE);
  const { schedule, playback } = played.prepared;
  const run = {
    ...played.run,
    transmitted: played.run.transmitted.filter(
      (t) => t.clip_id !== "c01_say_loop",
    ),
  };
  assert.match(
    driver.judgeDispatch(schedule, playback, run).join("; "),
    /c01_say_loop: planned but never transmitted/,
  );
});

test("an action sent twice is caught", async () => {
  const played = await play(SMOKE);
  const { schedule, playback } = played.prepared;
  const twice = played.run.dispatched.find(
    (d) => d.action_id === "c01_say_plan",
  );
  const run = { ...played.run, dispatched: [...played.run.dispatched, twice] };
  assert.match(
    driver.judgeDispatch(schedule, playback, run).join("; "),
    /c01_say_plan: dispatched 2 times/,
  );
});

test("no stimulus reaches the page outside a budget permit", async () => {
  const budget = budgetFor(SMOKE);
  const prepared = prepareLongRun({ env: TONES, plan: SMOKE, budget });
  const clock = virtualClock();
  const controller = fakeController(clock, budget);
  // A guard that grants without running the dispatch leaves the controller
  // untouched, and one that runs it outside a permit makes the controller throw.
  budget.guarded = async (id, dispatch) => {
    budget.ids.push(id);
    return dispatch();
  };
  const run = await runLongInterview({
    plan: SMOKE,
    ...prepared,
    budget,
    controller,
    clock,
    joinedAtMs: T0,
    interviewStartMs: T0,
    joinDispatched: true,
  });
  assert.match(run.aborted, /ran outside the budget/);
  assert.equal(run.transmitted.length, 0);
});

test("a denied permit stops the run, dispatches nothing more and exits as an exhausted budget", async () => {
  const played = await play(SMOKE, { budgetOptions: { denyAt: 6 } });
  assert.equal(played.budget.ids.length, 5);
  assert.match(played.run.budgetDenied, /action ceiling/);
  assert.equal(played.run.observationEndMs, null);
  const sent = played.controller.calls.length;
  assert.ok(sent <= 4, `${sent} stimuli before the denial`);
  const finished = finalize(played);
  assert.equal(finished.exitCode, EXIT.incompleteBudget);
  assert.equal(finished.report.full_interview, false);
  assert.ok(finished.report.failure_classes.includes("budget_exhausted"));
});

test("an agent that dies mid-run aborts the run and it reports no interview", async () => {
  const dying = Promise.reject(new Error("rust agent exited early"));
  dying.catch(() => {});
  const played = await play(SMOKE, { failure: dying });
  assert.match(played.run.aborted, /rust agent exited early/);
  const finished = finalize(played);
  assert.notEqual(finished.exitCode, EXIT.pass);
  assert.equal(finished.report.full_interview, false);
  assert.equal(finished.report.speech_run, false);
});

test("an uncovered barge consumes no permit and is listed, not passed", async () => {
  const played = await play(SMOKE, {
    controllerOptions: { neverSpeaks: true },
  });
  assert.deepEqual(played.run.uncovered, ["c01_barge"]);
  assert.ok(!played.budget.ids.includes("c01_barge"));
  const finished = finalize(played);
  assert.deepEqual(finished.report.stimuli.uncovered, ["c01_barge"]);
  assert.equal(finished.exitCode, EXIT.pass);
  // The duration was held, but the coverage was not complete and says so.
  assert.equal(finished.report.coverage_complete, false);
  assert.equal(finished.report.speech_run, false);
});

test("a reply that starts just after the final question ended is its answer", async () => {
  const finished = finalize(
    await play(SMOKE, {
      controllerOptions: { promptReply: true, playDelayMs: 300 },
    }),
  );
  assert.deepEqual(finished.report.failure_classes, []);
  assert.equal(finished.exitCode, EXIT.pass);
});

test("a reply that starts while the final question is still asked is not its answer", async () => {
  const finished = finalize(
    await play(SMOKE, { controllerOptions: { earlyReply: true } }),
  );
  assert.ok(finished.report.failure_classes.includes("pending_final_reply"));
  assert.equal(finished.exitCode, EXIT.failed);
});

test("an unanswered final question is a pending final reply", async () => {
  const played = await play(SMOKE, {
    controllerOptions: { noFinalReply: true },
  });
  const finished = finalize(played);
  assert.ok(finished.report.failure_classes.includes("pending_final_reply"));
  assert.equal(finished.exitCode, EXIT.failed);
});

const LEFTOVER = {
  errors: [],
  leftover: { agents: 1, processes: 0, rooms: 0 },
};

test("what was left active after teardown exits as incomplete cleanup", async () => {
  const finished = finalize(await play(SMOKE), { cleanup: LEFTOVER });
  assert.equal(finished.exitCode, EXIT.incompleteCleanup);
  assert.ok(finished.report.failure_classes.includes("cleanup_error"));
  assert.equal(finished.report.cleanup_assessed, true);
  assert.equal(finished.report.full_interview, false);
});

test("a worker that cannot assess cleanup says so and keeps its other classes", async () => {
  const unassessed = finalize(await play(SMOKE), { cleanup: undefined });
  assert.equal(unassessed.report.cleanup_assessed, false);
  assert.equal(unassessed.exitCode, EXIT.pass);
  assert.ok(unassessed.report.not_exercised.some((n) => /^cleanup:/.test(n)));
  // A budget denial still exits as one, not as cleanup.
  const denied = finalize(await play(SMOKE, { budgetOptions: { denyAt: 6 } }), {
    cleanup: undefined,
  });
  assert.equal(denied.exitCode, EXIT.incompleteBudget);
});

test("an expired token and a closed browser are their own failure classes", async () => {
  const played = await play(SMOKE);
  const expired = finalize(played, { tokenExpired: true });
  assert.ok(expired.report.failure_classes.includes("token_expiry"));
  assert.equal(expired.exitCode, EXIT.failed);
  const closed = finalize(played, { browserDisconnected: true });
  assert.ok(closed.report.failure_classes.includes("browser_disconnect"));
  assert.equal(closed.exitCode, EXIT.failed);
});

test("the duration is claimed only from the server clock", async () => {
  const played = await play(SMOKE);
  const viaToken = finalize(played, { clock: [] });
  assert.equal(viaToken.report.anchor.source, "first_token_request");
  assert.equal(viaToken.report.full_interview, false);
  assert.equal(finalize(played).report.anchor.source, "server_clock");
  assert.equal(finalize(played).report.schedule_start_delay_s, 5);
});

test("a stimulus whose page action throws still spends its timestamp", async () => {
  const budget = budgetFor(SMOKE);
  const prepared = prepareLongRun({ env: TONES, plan: SMOKE, budget });
  const clock = virtualClock();
  const controller = fakeController(clock, budget);
  controller.playClip = async () => {
    throw new Error("page closed under the clip");
  };
  const run = await runLongInterview({
    plan: SMOKE,
    ...prepared,
    budget,
    controller,
    clock,
    joinedAtMs: T0,
    interviewStartMs: T0,
    joinDispatched: true,
  });
  assert.match(run.aborted, /page closed/);
  assert.equal(run.lastStimulusAtMs, T0);
  assert.equal(run.transmitted.length, 0);
});

test("a join that did not go through the guard stops the run before any stimulus", async () => {
  const budget = budgetFor(SMOKE);
  const prepared = prepareLongRun({ env: TONES, plan: SMOKE, budget });
  const clock = virtualClock();
  const controller = fakeController(clock, budget);
  const run = await runLongInterview({
    plan: SMOKE,
    ...prepared,
    budget,
    controller,
    clock,
    joinedAtMs: T0,
    interviewStartMs: T0,
    joinDispatched: false,
  });
  assert.match(run.aborted, /join was not dispatched through the run budget/);
  assert.deepEqual(budget.ids, []);
  assert.equal(controller.calls.length, 0);
});

test("a run with no GoAway is inconclusive, not a pass", async () => {
  const finished = finalize(await play(SMOKE), { episodes: [] });
  assert.equal(finished.exitCode, EXIT.noGoAway);
  assert.equal(finished.report.full_interview, false);
});

test("the server deadline route sends no end and waits for the agent", async () => {
  const played = await play(DEADLINE);
  assert.ok(!played.budget.ids.includes("end"));
  assert.ok(played.controller.calls.some((c) => c[0] === "deadline"));
  assert.equal(
    played.run.observationEndMs - (played.joinedAtMs - 5000),
    720 * 1000,
  );
});

test("failure classes land as artifacts, a clean run writes none", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "li-driver-"));
  try {
    const bad = finalize(await play(SMOKE), { cleanup: LEFTOVER });
    const written = writeArtifacts(
      { BROWSER_CHECK_LONG_ARTIFACT_DIR: dir },
      bad.resolved,
    );
    assert.deepEqual(
      written.map((f) => path.basename(f)),
      ["failure-cleanup_error.json"],
    );
    assert.equal(
      JSON.parse(fs.readFileSync(written[0], "utf8")).exit_code,
      EXIT.incompleteCleanup,
    );
    const clean = finalize(await play(SMOKE));
    assert.deepEqual(
      writeArtifacts(
        { BROWSER_CHECK_LONG_ARTIFACT_DIR: dir + "/none" },
        clean.resolved,
      ),
      [],
    );
    assert.equal(fs.existsSync(dir + "/none"), false);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("recorded speech reports a speech run only when every clip was covered", async () => {
  // Stand-ins for recordings: the tones with their level changed, so they are
  // not the generator's bytes. They exist only to reach the speech branch of
  // the report; the driver cannot hear whether a file is speech.
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "li-speech-"));
  try {
    const schedule = sched.generateSchedule("coding", {
      durationMin: 10,
      observationTargetS: 600,
    }).schedule;
    const set = sched.syntheticClipSet(schedule);
    for (const [id, bytes] of set.audio) {
      const wav = sched.parseWav(bytes);
      set.audio.set(
        id,
        sched.wavBytes(Array.from(wav.samples, (v) => Math.round(v * 0.9))),
      );
    }
    const manifest = sched.buildClipManifest(
      schedule,
      set.audio,
      "recorded-speech",
      {
        tool: "test recording",
        tool_version: "0",
      },
    );
    for (const [id, bytes] of set.audio)
      fs.writeFileSync(path.join(dir, `${id}.wav`), bytes);
    fs.writeFileSync(path.join(dir, "manifest.json"), JSON.stringify(manifest));
    const env = { BROWSER_CHECK_LONG_CLIP_DIR: dir };
    const budget = budgetFor(SMOKE, env, {
      fixtureSha256: sched.budgetFields(schedule, manifest).fixture_sha256,
    });
    const make = async (controllerOptions) => {
      const prepared = prepareLongRun({
        env,
        plan: SMOKE,
        budget: Object.assign(budget, { ids: [] }),
      });
      const clock = virtualClock();
      const controller = fakeController(clock, budget, controllerOptions);
      await budget.guarded("candidate_join", async () => {});
      const run = await runLongInterview({
        plan: SMOKE,
        ...prepared,
        budget,
        controller,
        clock,
        joinedAtMs: clock.now(),
        interviewStartMs: clock.now() - 5000,
        joinDispatched: true,
      });
      clock.advance(1000);
      return finalize({ plan: SMOKE, prepared, run, joinedAtMs: T0, clock });
    };
    const covered = await make({});
    assert.equal(covered.exitCode, EXIT.pass);
    assert.equal(covered.report.clip_source, CLIP_SOURCE.speech);
    assert.equal(covered.report.validation_fixture, false);
    assert.equal(covered.report.speech_run, true);
    const uncovered = await make({ neverSpeaks: true });
    assert.equal(uncovered.report.speech_run, false);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("refusals: no clips, a second source, tones outside a smoke, tones as speech", () => {
  const budget = budgetFor(SMOKE);
  assert.throws(
    () => prepareLongRun({ env: {}, plan: SMOKE, budget }),
    (e) =>
      e instanceof DriverRefused &&
      /needs BROWSER_CHECK_LONG_CLIP_DIR/.test(e.message),
  );
  assert.throws(
    () =>
      prepareLongRun({
        env: { ...TONES, BROWSER_CHECK_LONG_CLIP_DIR: "/x" },
        plan: SMOKE,
        budget,
      }),
    /two clip sources/,
  );
  assert.throws(
    () =>
      prepareLongRun({
        env: { BROWSER_CHECK_LONG_TONE_FIXTURE: "yes" },
        plan: SMOKE,
        budget,
      }),
    /must be 1 or unset/,
  );
  // Tones named as the fixture in a run that is not a smoke.
  for (const plan of [
    QUALIFICATION,
    {
      ...QUALIFICATION,
      runKind: "pilot",
      durationMin: 60,
      observationTargetS: 3600,
    },
  ]) {
    assert.throws(
      () => prepareLongRun({ env: TONES, plan, budget: budgetFor(plan) }),
      (e) =>
        e instanceof DriverRefused &&
        /no speech and is for a smoke run only/.test(e.message),
      plan.runKind,
    );
  }
  // The tone files handed over as a clip directory: the manifest says they hold
  // no speech, so a non-smoke run refuses them on that ground alone.
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "li-tones-"));
  try {
    const schedule = sched.generateSchedule("coding", {
      durationMin: 90,
      observationTargetS: 5400,
    }).schedule;
    const set = sched.syntheticClipSet(schedule);
    for (const [id, bytes] of set.audio)
      fs.writeFileSync(path.join(dir, `${id}.wav`), bytes);
    fs.writeFileSync(
      path.join(dir, "manifest.json"),
      JSON.stringify(set.manifest),
    );
    const budgetQ = {
      ...budgetFor(QUALIFICATION),
      fixtureSha256: sched.budgetFields(schedule, set.manifest).fixture_sha256,
    };
    assert.throws(
      () =>
        prepareLongRun({
          env: { BROWSER_CHECK_LONG_CLIP_DIR: dir },
          plan: QUALIFICATION,
          budget: budgetQ,
        }),
      /contain no speech/,
    );
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("refusals: the budget must carry this schedule's actions and pin", () => {
  const dropped = budgetFor(SMOKE);
  dropped.actions = dropped.actions.filter((id) => id !== "c01_think_start");
  assert.throws(
    () => prepareLongRun({ env: TONES, plan: SMOKE, budget: dropped }),
    /not the schedule's action IDs/,
  );
  const renamed = budgetFor(SMOKE);
  renamed.actions = renamed.actions.map((id) =>
    id === "c01_revise" ? "c01_other" : id,
  );
  assert.throws(
    () => prepareLongRun({ env: TONES, plan: SMOKE, budget: renamed }),
    /not the schedule's action IDs/,
  );
  const unpinned = budgetFor(SMOKE);
  unpinned.fixtureSha256 = "0".repeat(64);
  assert.throws(
    () => prepareLongRun({ env: TONES, plan: SMOKE, budget: unpinned }),
    /does not pin this schedule/,
  );
  assert.throws(
    () => prepareLongRun({ env: TONES, plan: SMOKE, budget: null }),
    /needs a run budget/,
  );
});

test("a real RunBudget is asked for exactly the schedule's actions, then end", async () => {
  const schedule = sched.generateSchedule("coding", {
    durationMin: 10,
    observationTargetS: 600,
  }).schedule;
  const set = sched.syntheticClipSet(schedule);
  const asked = [];
  const budget = new RunBudget("/nowhere", "r1", "d");
  budget.nextSeq = 1;
  budget.actions = [...schedule.actions];
  budget.fixtureSha256 = sched.budgetFields(
    schedule,
    set.manifest,
  ).fixture_sha256;
  budget.call = async (request) => {
    asked.push(request.action_id);
    return {
      v: 1,
      ok: true,
      granted: true,
      action_id: request.action_id,
      seq: request.seq,
    };
  };
  const prepared = prepareLongRun({ env: TONES, plan: SMOKE, budget });
  const clock = virtualClock();
  const stub = fakeController(clock, { inGuard: true });
  await budget.guarded("candidate_join", async () => {});
  const run = await runLongInterview({
    plan: SMOKE,
    ...prepared,
    budget,
    controller: stub,
    clock,
    joinedAtMs: clock.now(),
    interviewStartMs: clock.now(),
    joinDispatched: true,
  });
  assert.equal(run.aborted, null);
  assert.deepEqual(asked, [...schedule.actions, "end"]);
  assert.equal(budget.nextSeq, asked.length + 1);
});

test("the page controller drives the editor, Thinking, speaking state and the end", async () => {
  const events = [];
  const state = { pressed: "false", agent: "Speaking…" };
  const locator = (what) => ({
    getAttribute: async () => state.pressed,
    click: async () => {
      events.push(["click", what]);
      if (what === "#thinking")
        state.pressed = state.pressed === "true" ? "false" : "true";
    },
    innerText: async () => state.agent,
    fill: async (text) => events.push(["fill", text.split("\n")[0]]),
    waitFor: async () => events.push(["wait", what]),
    isVisible: async () => what.startsWith("heading") && state.packet === true,
    isEnabled: async () => state.endEnabled !== false,
    filter: () => ({ count: async () => 3 }),
  });
  const page = {
    waitForTimeout: async () => {},
    waitForFunction: async (_fn, [want]) => {
      if (state.pressed !== String(want)) throw new Error("timeout");
    },
    locator: (what) => locator(what),
    getByLabel: (what) => locator(what),
    getByRole: (role, o) => locator(`${role}:${o.name}`),
    evaluate: async (fn, arg) => (arg === undefined ? true : 1.5),
  };
  const languages = [];
  const controller = page_.pageController(page, {
    selectLanguage: async (_p, language) => languages.push(language),
  });
  const rev = sched.generateSchedule("coding", {
    durationMin: 10,
    observationTargetS: 600,
  }).schedule.revisions[0];
  await controller.applyRevision({ index: 1, marker: rev.marker });
  await controller.applyRevision({ index: 1, marker: rev.marker });
  assert.deepEqual(languages, ["JavaScript"]);
  assert.equal(events.filter((e) => e[0] === "fill").length, 2);
  assert.match(events.find((e) => e[0] === "fill")[1], new RegExp(rev.marker));
  await assert.rejects(
    () => controller.applyRevision({ index: 1, marker: "rev01-wrong" }),
    /not the scheduled one/,
  );
  await controller.setThinking(true);
  assert.equal(state.pressed, "true");
  await controller.setThinking(true);
  await controller.setThinking(false);
  assert.equal(events.filter((e) => e[1] === "#thinking").length, 2);
  assert.equal(await controller.agentSpeaking(), true);
  state.agent = "Listening";
  assert.equal(await controller.agentSpeaking(), false);
  assert.equal(await controller.agentReplyCount(), 3);
  await controller.playClip(
    { clip_id: "c01_say_plan" },
    Buffer.from("RIFFxxxx"),
  );
  assert.deepEqual(await controller.end(), {
    performanceReceived: true,
    reportReceived: true,
    clicked: true,
  });
  assert.ok(events.some((e) => e[1] === "button:End interview"));
  // The page's own countdown got there first: the button is disabled and the
  // packet is up. Nothing is clicked, and the result says so.
  events.length = 0;
  state.endEnabled = false;
  state.packet = true;
  assert.deepEqual(await controller.end(), {
    performanceReceived: true,
    reportReceived: true,
    clicked: false,
  });
  assert.equal(events.filter((e) => e[0] === "click").length, 0);
  // Disabled while the page is still generating its report: the packet is
  // waited for, not a click.
  state.packet = false;
  const waiting = await controller.end();
  assert.equal(waiting.clicked, false);
  assert.equal(events.filter((e) => e[0] === "click").length, 0);
});

test("the microphone script parses and the 660 second soak contract is untouched", () => {
  assert.doesNotThrow(() => new Function(page_.microphoneInitScript()));
  const shell = fs.readFileSync(
    path.join(ROOT, "scripts", "browser-check.sh"),
    "utf8",
  );
  assert.match(shell, /SOAK_SECONDS=\$\{BROWSER_CHECK_SOAK_SECONDS:-660\}/);
  const cjs = fs.readFileSync(
    path.join(ROOT, "scripts", "browser-check.cjs"),
    "utf8",
  );
  assert.match(cjs, /const CONNECTION_CAP_SECONDS = 600;/);
  // The soak flow never reaches the long-interview driver.
  assert.match(cjs, /if \(flow === "soak"\) \{\s+await soakInterview\(/);
});

test("the generator's own tones under a recorded-speech label are refused", () => {
  const schedule = sched.generateSchedule("coding", {
    durationMin: 10,
    observationTargetS: 600,
  }).schedule;
  const set = sched.syntheticClipSet(schedule);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "li-forged-"));
  try {
    for (const [id, bytes] of set.audio)
      fs.writeFileSync(path.join(dir, `${id}.wav`), bytes);
    const relabel = (source) => {
      const manifest = sched.buildClipManifest(
        schedule,
        set.audio,
        "recorded-speech",
        source,
      );
      fs.writeFileSync(
        path.join(dir, "manifest.json"),
        JSON.stringify(manifest),
      );
      return {
        BROWSER_CHECK_LONG_CLIP_DIR: dir,
        fixtureSha256: sched.budgetFields(schedule, manifest).fixture_sha256,
      };
    };
    const attempt = (source) => {
      const { fixtureSha256, ...env } = relabel(source);
      return prepareLongRun({
        env,
        plan: SMOKE,
        budget: { ...budgetFor(SMOKE), fixtureSha256 },
      });
    };
    assert.throws(
      () => attempt({ tool: "a recorder", tool_version: "1" }),
      /synthetic tone under a recorded-speech label/,
    );
    assert.throws(() => attempt({}), /names no tool that made it/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("browser-check hands the driver the guarded join, the schedule check before launch and the class exit code", () => {
  const cjs = fs.readFileSync(
    path.join(ROOT, "scripts", "browser-check.cjs"),
    "utf8",
  );
  // The join click is the first budgeted action and goes through the guard.
  assert.match(
    cjs,
    /flow === "long-interview"\s*\?\s*\(dispatch\) =>\s*runBudget\.guarded\("candidate_join", async \(\) => \{\s*await dispatch\(\);\s*await silenceGateTone\(page\);\s*joinGuarded = true;/,
  );
  // A refused permit anywhere, the join included, is the budget class.
  assert.match(cjs, /error instanceof BudgetDenied\) process\.exit\(4\)/);
  // The schedule and its clips are judged before a browser exists.
  const prepare = cjs.indexOf("prepareLongRun({");
  const launch = cjs.indexOf("browser = await chromium.launch(");
  assert.ok(prepare > 0 && launch > prepare, "prepared before the launch");
  // The microphone is replaced before the page loads, the flow's exit code
  // is the class's, and the soak and report flows never call the driver.
  assert.ok(
    cjs.indexOf("await installMicrophone(page)") <
      cjs.indexOf(
        "await page.goto(\n      interviewUrl(\n        PROBLEM_IDS.twoSum,",
      ),
  );
  // Chromium holds the injected AudioContext suspended without this flag, and
  // the media gate then reads the microphone as silent.
  assert.match(
    cjs,
    /BROWSER_CHECK_FLOW === "long-interview"\s*\?\s*\["--autoplay-policy=no-user-gesture-required"\]/,
  );
  assert.match(
    cjs,
    /error instanceof LongInterviewExit\) process\.exit\(error\.exitCode\)/,
  );
  assert.equal(cjs.match(/runLongInterview\(/g).length, 1);
});

test("the injected microphone carries a gate tone until the join, then only clips", () => {
  const nodes = [];
  class FakeContext {
    constructor() {
      this.state = "suspended";
    }
    createMediaStreamDestination() {
      return { stream: { getAudioTracks: () => ["track"] } };
    }
    createOscillator() {
      return {
        frequency: {},
        connect() {},
        start: () => nodes.push("osc"),
      };
    }
    createGain() {
      const node = { gain: { value: 1 }, connect() {} };
      nodes.push(node);
      return node;
    }
    async resume() {
      this.state = "running";
    }
  }
  const mediaDevices = { getUserMedia: async () => ({ original: true }) };
  const window = {};
  new Function(
    "navigator",
    "AudioContext",
    "MediaStream",
    "window",
    page_.microphoneInitScript(),
  )(
    { mediaDevices },
    FakeContext,
    class {
      constructor(tracks) {
        this.tracks = tracks;
      }
    },
    window,
  );
  return mediaDevices.getUserMedia({ audio: true }).then((stream) => {
    assert.deepEqual(stream.tracks, ["track"]);
    const gate = nodes.find((n) => n.gain);
    assert.equal(gate.gain.value, 0.3, "the gate hears sound before the join");
    assert.ok(nodes.includes("osc"));
    assert.equal(window.__longInterviewMic.silenceGate(), true);
    assert.equal(gate.gain.value, 0, "silent once the join is clicked");
  });
});

test("silenceGateTone refuses a page whose tone never started", async () => {
  await assert.rejects(
    () => page_.silenceGateTone({ evaluate: async () => undefined }),
    /gate tone was never started/,
  );
  await page_.silenceGateTone({ evaluate: async () => true });
});

test("the whiteboard surface is chosen by the environment and drawn with the pointer", async () => {
  assert.equal(driver.modeFromEnv({}), "coding");
  assert.equal(
    driver.modeFromEnv({ [driver.MODE_ENV]: "whiteboard" }),
    "whiteboard",
  );
  assert.throws(
    () => driver.modeFromEnv({ [driver.MODE_ENV]: "board" }),
    /coding or whiteboard/,
  );
  const events = [];
  const page = {
    locator: () => ({
      boundingBox: async () => ({ x: 100, y: 200, width: 800, height: 500 }),
    }),
    getByRole: () => ({ isDisabled: async () => false }),
    mouse: {
      move: async (x, y) => events.push(["move", x, y]),
      down: async () => events.push(["down"]),
      up: async () => events.push(["up"]),
    },
  };
  const controller = page_.pageController(page, {
    selectLanguage: async () => assert.fail("no language on a board"),
    mode: "whiteboard",
  });
  const rev = sched.generateSchedule("whiteboard", {
    durationMin: 10,
    observationTargetS: 600,
  }).schedule.revisions[0];
  await controller.applyRevision({ index: 1, marker: rev.marker });
  assert.equal(events.filter((e) => e[0] === "down").length, 4);
  assert.equal(events.filter((e) => e[0] === "up").length, 4);
  await assert.rejects(
    () => controller.applyRevision({ index: 1, marker: "rev01-wrong" }),
    /not the scheduled one/,
  );
});

test("a run the page's countdown ended is reported as such, and its capture is kept", () => {
  const cjs = fs.readFileSync(
    path.join(ROOT, "scripts", "browser-check.cjs"),
    "utf8",
  );
  // The generic failure path writes an incomplete capture; it must leave the
  // long interview's own report alone.
  assert.match(
    cjs,
    /\(floorLogPath \|\| probingAudio\) &&\s*!longCaptureWritten/,
  );
  assert.match(cjs, /longCaptureWritten = true;\s*fs\.writeFileSync/);
});
