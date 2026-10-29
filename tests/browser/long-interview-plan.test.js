import { test } from "node:test";
import assert from "node:assert/strict";
import crypto from "node:crypto";

import plan_ from "../../scripts/long-interview-plan.cjs";
import timing_ from "../../scripts/long-interview-timing.cjs";

const { archiveBounds } = timing_;

const {
  bootstrapMinutes,
  deadlineMinutes,
  judgeObservation,
  pageDurationMin,
  parseCountdown,
  parseDurationMin,
  planFromEnv,
} = plan_;

const qualification = {
  runKind: "qualification",
  endMode: "manual",
  durationMin: 90,
  observationTargetS: 5400,
};
const deadlineSmoke = {
  runKind: "smoke",
  endMode: "server_deadline",
  durationMin: 20,
  observationTargetS: 1320,
};

const start = 1_000_000;
const clock = (elapsedSeconds, origin = start + 2000) =>
  elapsedSeconds.map((s, i) => ({
    pid: 1,
    session: 1,
    startUnixMs: origin,
    elapsedMs: s * 1000,
    sample: i + 1,
  }));

// What a passing 90 minute manual run saw.
function passing(plan = qualification, patch = {}) {
  const minutes = plan.durationMin;
  return {
    tokenDurationsMin: [minutes],
    bootstrapDurationsMin: [minutes],
    deadlineDurationsMin: [],
    countdownStartS: minutes * 60 - 20,
    clock: clock([60, 120, minutes * 60 - 30, minutes * 60]),
    harnessStartMs: start,
    nowMs: start + plan.observationTargetS * 1000,
    observedS: plan.observationTargetS,
    graceStimuli: 0,
    ...patch,
  };
}

test("a run that saw what it asked for passes", () => {
  assert.deepEqual(judgeObservation(qualification, passing()), {
    verdict: "pass",
    reasons: [],
  });
});

test("a deadline smoke passes only with the server's own deadline line", () => {
  const seen = passing(deadlineSmoke, {
    deadlineDurationsMin: [20],
    clock: clock([60, 600, 1200, 1320]),
  });
  assert.equal(judgeObservation(deadlineSmoke, seen).verdict, "pass");
  const noDeadline = judgeObservation(deadlineSmoke, {
    ...seen,
    deadlineDurationsMin: [],
  });
  assert.match(noDeadline.reasons.join(), /never reached its deadline/);
  const stimulus = judgeObservation(deadlineSmoke, {
    ...seen,
    graceStimuli: 1,
  });
  assert.match(stimulus.reasons.join(), /stimulus during the deadline grace/);
  const wrongMinutes = judgeObservation(deadlineSmoke, {
    ...seen,
    deadlineDurationsMin: [10],
  });
  assert.match(wrongMinutes.reasons.join(), /deadline duration/);
});

const failures = [
  [
    "a token clamped below the request",
    { tokenDurationsMin: [60] },
    /token duration \[60\]/,
  ],
  [
    "a bootstrap handed another length",
    { bootstrapDurationsMin: [90, 20] },
    /bootstrap duration/,
  ],
  [
    "no token response found",
    { tokenDurationsMin: [] },
    /no token duration was observed/,
  ],
  [
    "no bootstrap line found",
    { bootstrapDurationsMin: [] },
    /no bootstrap duration was observed/,
  ],
  [
    "a countdown that never ran",
    { countdownStartS: null },
    /no countdown was observed/,
  ],
  [
    "a countdown of a shorter interview",
    { countdownStartS: 1200 },
    /countdown started at 1200s/,
  ],
  ["no clock sample at all", { clock: [] }, /no interview clock sample/],
  [
    "a clock origin from before the run",
    { clock: clock([60, 5400], start - 600_000) },
    /outside the window this run was alive/,
  ],
  [
    "a clock origin that moved",
    {
      clock: [...clock([60]), ...clock([5400], start + 4000)].map((r, i) => ({
        ...r,
        sample: i + 1,
      })),
    },
    /origin is missing or changed/,
  ],
  [
    "an unanchored clock",
    { clock: clock([60, 5400], null) },
    /origin is missing or changed/,
  ],
  [
    "a clock that went backwards",
    { clock: clock([60, 5400, 5000]) },
    /went backwards/,
  ],
  [
    "a clock that stopped early",
    { clock: clock([60, 3000]) },
    /short of the 5400s target/,
  ],
  [
    "a clock past duration plus grace",
    { clock: clock([60, 5400, 5600]) },
    /past the duration plus its grace/,
  ],
  [
    "an observation shorter than the target",
    { observedS: 5399 },
    /observed 5399s of the 5400s target/,
  ],
];
for (const [name, patch, reason] of failures) {
  test(`fails: ${name}`, () => {
    const verdict = judgeObservation(
      qualification,
      passing(qualification, patch),
    );
    assert.equal(verdict.verdict, "fail");
    assert.match(verdict.reasons.join("; "), reason);
  });
}

test("a short smoke is not asked for clock samples it cannot have", () => {
  const smoke = {
    runKind: "smoke",
    endMode: "manual",
    durationMin: 10,
    observationTargetS: 30,
  };
  const seen = passing(smoke, { clock: [], countdownStartS: 590 });
  assert.equal(judgeObservation(smoke, seen).verdict, "pass");
});

test("runtime lines are read for their own room only", () => {
  const log = [
    "starting interview: room=a problem=two-sum duration=60min",
    "starting interview: room=b problem=two-sum duration=20min",
    "starting interview: room=a problem=two-sum duration=60min extra",
    "interview reached its server-side deadline: room=a duration=60min",
  ].join("\n");
  assert.deepEqual(bootstrapMinutes(log, "a"), [60]);
  assert.deepEqual(bootstrapMinutes(log, "b"), [20]);
  assert.deepEqual(bootstrapMinutes(log, "c"), []);
  assert.deepEqual(deadlineMinutes(log, "a"), [60]);
  assert.deepEqual(deadlineMinutes(log, "b"), []);
});

test("the countdown text is minutes and seconds only", () => {
  assert.equal(parseCountdown("90:00"), 5400);
  assert.equal(parseCountdown(" 19:41 "), 1181);
  for (const text of ["9:00", "00:60", "", "soon", "1:00:00"]) {
    assert.equal(parseCountdown(text), null, text);
  }
});

test("duration spellings: only the supported minutes, no coercion", () => {
  assert.deepEqual(parseDurationMin(undefined), { value: 20 });
  for (const ok of ["10", "20", "30", "60", "90"]) {
    assert.deepEqual(parseDurationMin(ok), { value: Number(ok) });
  }
  for (const bad of [
    "9",
    "91",
    "45",
    "0",
    "-1",
    "",
    "1e1",
    "20.0",
    " 20",
    "+20",
  ]) {
    assert.ok(parseDurationMin(bad).error, JSON.stringify(bad));
  }
});

test("the page keeps its twenty minutes outside a long interview", () => {
  assert.equal(pageDurationMin({ BROWSER_CHECK_FLOW: "soak" }), 20);
  assert.equal(
    pageDurationMin({
      BROWSER_CHECK_FLOW: "long-interview",
      BROWSER_CHECK_DURATION_MIN: "60",
    }),
    60,
  );
  assert.throws(() =>
    pageDurationMin({
      BROWSER_CHECK_FLOW: "long-interview",
      BROWSER_CHECK_DURATION_MIN: "61",
    }),
  );
});

// planFromEnv over an injected file system.
function plan(manifest, env = {}, files = {}) {
  const raw = Buffer.from(JSON.stringify(manifest));
  return planFromEnv(
    {
      BROWSER_CHECK_FLOW: "long-interview",
      BROWSER_CHECK_AGENT: "rust",
      BROWSER_CHECK_RUN_MANIFEST: "/m.json",
      BROWSER_CHECK_RUN_ID: "run-1",
      BROWSER_CHECK_RUN_MANIFEST_SHA256: crypto
        .createHash("sha256")
        .update(raw)
        .digest("hex"),
      ...env,
    },
    {
      readBytes: () => raw,
      readFile: (file) => {
        if (file in files) return files[file];
        throw new Error("absent");
      },
      isDirectory: () => true,
    },
  );
}
const manifest = {
  run_id: "run-1",
  run_kind: "qualification",
  end_mode: "manual",
  duration_min: 90,
  observation_target_s: 5400,
  selected_duration_s: 5400,
  outer_wall_cap_s: 5400 + 660,
  cleanup_reserve_s: 360,
  max_archive_bytes: archiveBounds(5400 + 660).floor,
};

test("a valid plan carries what the flow runs on", () => {
  const result = plan(manifest, { BROWSER_CHECK_DURATION_MIN: "90" });
  assert.deepEqual(result.errors, []);
  assert.deepEqual(result.plan, {
    runKind: "qualification",
    endMode: "manual",
    durationMin: 90,
    observationTargetS: 5400,
    runId: "run-1",
  });
});

test("the recording cap is read from the config file when the environment is silent", () => {
  const env = {
    BROWSER_CHECK_DURATION_MIN: "90",
    BROWSER_CHECK_CONFIG_PATH: "/c.env",
  };
  const clamped = plan(manifest, env, {
    "/c.env": "export CODETRIAL_RECORDING_MAX_MINUTES='45'\n",
  });
  assert.match(clamped.errors.join(), /would clamp the token to 45 minutes/);
  const roomy = plan(manifest, env, {
    "/c.env": "CODETRIAL_RECORDING_MAX_MINUTES=90\n",
  });
  assert.deepEqual(roomy.errors, []);
  // A cap below the product floor is raised to it by the server, so a ten
  // minute run is still honest and a twenty minute one is not.
  const floor = plan(
    {
      ...manifest,
      run_kind: "smoke",
      duration_min: 20,
      observation_target_s: 60,
      selected_duration_s: 60,
    },
    { BROWSER_CHECK_DURATION_MIN: "20", CODETRIAL_RECORDING_MAX_MINUTES: "3" },
  );
  assert.match(floor.errors.join(), /would clamp the token to 10 minutes/);
});

test("a long interview without a budgeted run is refused", () => {
  const result = planFromEnv({
    BROWSER_CHECK_FLOW: "long-interview",
    BROWSER_CHECK_AGENT: "dispatch",
  });
  assert.match(result.errors.join(), /needs a budgeted run/);
});

test("the countdown may start late by the time the page took to read, no more", () => {
  const late = passing(qualification, {
    countdownStartS: 5400 - 200,
    countdownSlackS: 205,
  });
  assert.equal(judgeObservation(qualification, late).verdict, "pass");
  const tooLate = passing(qualification, {
    countdownStartS: 5400 - 200,
    countdownSlackS: 150,
  });
  assert.match(
    judgeObservation(qualification, tooLate).reasons.join(),
    /countdown started at 5200s/,
  );
});

test("a clock sample with no numeric field never passes", () => {
  const rows = clock([60, 5400]);
  rows[1] = { ...rows[1], startUnixMs: undefined };
  const origin = judgeObservation(
    qualification,
    passing(qualification, { clock: rows }),
  );
  assert.match(origin.reasons.join(), /origin is missing or changed/);
  const elapsed = clock([60, 5400]);
  elapsed[0] = { ...elapsed[0], elapsedMs: NaN };
  const result = judgeObservation(
    qualification,
    passing(qualification, { clock: elapsed }),
  );
  assert.match(result.reasons.join(), /no elapsed time/);
});
