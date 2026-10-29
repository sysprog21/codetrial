import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import timing_ from "../../scripts/long-interview-timing.cjs";
import sched from "../../scripts/long-interview-schedule.cjs";

const {
  EXIT,
  FAILURE_CLASSES,
  anchorInterview,
  archiveBounds,
  classifyEvidence,
  classifyRotation,
  judgeRotations,
  judgeTimestamps,
  resolveRun,
  timingFor,
  validateBudgetWindows,
  validateReserve,
  writeFailureArtifacts,
} = timing_;

const ROOT = fileURLToPath(new URL("../..", import.meta.url));
const BUDGET = path.join(ROOT, "scripts", "interview-run-budget.py");

const t90 = timingFor(90);

test("the stimulus reserve is the handover grace plus a margin, and the schedule agrees", () => {
  assert.equal(timing_.HANDOVER_GRACE_S, 90);
  assert.equal(timing_.STIMULUS_MARGIN_S, 30);
  assert.equal(timing_.STIMULUS_RESERVE_S, 120);
  assert.equal(
    timing_.STIMULUS_RESERVE_S,
    timing_.HANDOVER_GRACE_S + timing_.STIMULUS_MARGIN_S,
  );
  const bundle = sched.generateSchedule("coding", {
    durationMin: 90,
    observationTargetS: 5400,
  });
  assert.equal(t90.lastStimulusBoundaryS, 5280);
  assert.equal(bundle.schedule.observation_window_s, t90.lastStimulusBoundaryS);
  // The final question starts 30s before that boundary.
  assert.equal(t90.finalQuestionAtS, 5250);
  assert.equal(bundle.schedule.final_at_s, t90.finalQuestionAtS);
  for (const step of bundle.schedule.steps) {
    assert.ok(
      step.at_s <= t90.finalQuestionAtS,
      `${step.action_id} at ${step.at_s}s is inside the reserve`,
    );
  }
});

test("the three moments are three numbers and the observation is the full duration", () => {
  assert.equal(t90.lastStimulusBoundaryS, 5280);
  assert.equal(t90.observationEndS, 5400);
  assert.notEqual(t90.lastStimulusBoundaryS, t90.observationEndS);
  assert.equal(t90.durationS, 5400);
  const t10 = timingFor(10);
  assert.equal(t10.lastStimulusBoundaryS, 480);
  assert.equal(t10.observationEndS, 600);
});

test("setup, end and report, and teardown sit outside the interview and the cap is the sum", () => {
  assert.equal(timing_.SETUP_BUDGET_S, 300);
  assert.equal(timing_.END_REPORT_BUDGET_S, 300);
  assert.equal(timing_.TEARDOWN_BUDGET_S, 60);
  assert.equal(timing_.WALL_OVERHEAD_S, 660);
  for (const minutes of [10, 20, 30, 60, 90]) {
    assert.equal(timingFor(minutes).wallCapS, minutes * 60 + 660);
  }
  // The report generator's own limit bounds one step, not the window.
  assert.ok(timing_.REPORT_GENERATION_LIMIT_S < timing_.END_REPORT_BUDGET_S);
});

test("the reserve is revalidated against the resolved settings", () => {
  assert.deepEqual(validateReserve(), []);
  assert.deepEqual(validateReserve({ resolvedGraceS: 90 }), []);
  assert.match(
    validateReserve({ resolvedGraceS: 91 }).join(),
    /shorter than the resolved handover grace 91s plus the 30s margin/,
  );
  assert.match(
    validateReserve({ reserveS: 100 }).join(),
    /reserve 100s is shorter/,
  );
  assert.match(
    validateReserve({ resolvedGraceS: Number.NaN }).join(),
    /not a whole number/,
  );
});

function manifestFor(selected, overrides = {}) {
  const cap = selected + 660;
  return {
    selected_duration_s: selected,
    outer_wall_cap_s: cap,
    cleanup_reserve_s: 360,
    max_archive_bytes: archiveBounds(cap).floor,
    ...overrides,
  };
}

test("the wall windows and the archive bound are enforced on the manifest", () => {
  assert.deepEqual(validateBudgetWindows(manifestFor(5400)), []);
  const cases = [
    [
      { outer_wall_cap_s: 5400 + 659 },
      /is not the selected duration 5400 plus 660s/,
    ],
    [
      { outer_wall_cap_s: 5400 + 661 },
      /is not the selected duration 5400 plus 660s/,
    ],
    [{ cleanup_reserve_s: 359 }, /end and report window plus teardown, 360s/],
    [
      { max_archive_bytes: undefined },
      /max_archive_bytes must be a positive integer/,
    ],
    [{ max_archive_bytes: 0 }, /max_archive_bytes must be a positive integer/],
    [{ max_archive_bytes: archiveBounds(6060).floor - 1 }, /is below the/],
    [{ max_archive_bytes: archiveBounds(6060).ceiling + 1 }, /is above the/],
  ];
  for (const [patch, pattern] of cases) {
    assert.match(
      validateBudgetWindows(manifestFor(5400, patch)).join(),
      pattern,
      JSON.stringify(patch),
    );
  }
});

test("the archive bound matches the one the budget controller derives", () => {
  const script = `
import importlib.util, json, sys
spec = importlib.util.spec_from_file_location("irb", sys.argv[1])
irb = importlib.util.module_from_spec(spec)
sys.modules["irb"] = irb
spec.loader.exec_module(irb)
rows = [[cap, *irb.archive_bounds(cap)] for cap in (690, 1860, 3660, 6060)]
print(json.dumps({"rows": rows, "cadence": irb.RESOURCE_CADENCE_S,
  "line": irb.MAX_SAMPLE_LINE_BYTES,
  "control": irb.CONTROL_FILES_ALLOWANCE_BYTES,
  "evidence": irb.ARCHIVE_EVIDENCE_ALLOWANCE_BYTES,
  "exits": [irb.EXIT_CHILD_FAILED, irb.EXIT_INCOMPLETE_BUDGET,
            irb.EXIT_INCOMPLETE_CLEANUP, irb.EXIT_INCOMPLETE_ARTIFACT]}))
`;
  const result = spawnSync("python3", ["-I", "-c", script, BUDGET], {
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  const py = JSON.parse(result.stdout);
  assert.equal(py.cadence, timing_.RESOURCE_CADENCE_S);
  assert.equal(py.line, timing_.MAX_SAMPLE_LINE_BYTES);
  assert.equal(py.control, timing_.CONTROL_FILES_ALLOWANCE_BYTES);
  assert.equal(py.evidence, timing_.ARCHIVE_EVIDENCE_ALLOWANCE_BYTES);
  for (const [cap, floor, ceiling] of py.rows) {
    assert.deepEqual(
      { floor: archiveBounds(cap).floor, ceiling: archiveBounds(cap).ceiling },
      { floor, ceiling },
      `cap ${cap}`,
    );
  }
  assert.deepEqual(py.exits, [
    EXIT.failed,
    EXIT.incompleteBudget,
    EXIT.incompleteCleanup,
    EXIT.incompleteArtifact,
  ]);
});

test("the controller accepts the manifest the timing module accepts, and refuses what it refuses", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "lit-"));
  try {
    const validate = (patch) => {
      const hex = "a".repeat(64);
      const manifest = {
        schema_version: 1,
        run_id: "timing-1",
        binaries: { candidate: { path: process.execPath, sha256: hex } },
        settings_sha256: hex,
        fixture_sha256: hex,
        end_mode: "manual",
        actions: ["candidate_join"],
        action_ceiling: 2,
        limits: {},
        scopes: { scripted_actions: "hard", wall_time: "hard" },
        ...manifestFor(5400),
        ...patch,
      };
      const file = path.join(dir, "m.json");
      fs.writeFileSync(file, JSON.stringify(manifest));
      return spawnSync(
        "python3",
        ["-I", BUDGET, "validate", "--manifest", file],
        { encoding: "utf8" },
      );
    };
    assert.equal(validate({}).status, 0);
    const low = validate({ max_archive_bytes: 1000 });
    assert.equal(low.status, 2);
    assert.match(low.stderr, /max_archive_bytes 1000 is below/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

const ms = (s) => 1_000_000_000 + s * 1000;

test("the anchor is the server clock's start and a reload never moves it", () => {
  const clock = anchorInterview({
    clockStarts: [ms(0), ms(0), ms(0)],
    tokenRequestsMs: [ms(-2), ms(1500)],
  });
  assert.deepEqual(
    { anchor: clock.anchorMs, source: clock.source, reloads: clock.reloads },
    { anchor: ms(0), source: "server_clock", reloads: 1 },
  );
  // Without a clock sample the first token request is the unique start, not
  // the request a reload made later.
  const early = anchorInterview({
    tokenRequestsMs: [ms(1500), ms(-2), ms(900)],
  });
  assert.equal(early.anchorMs, ms(-2));
  assert.equal(early.source, "first_token_request");
  assert.equal(early.reloads, 2);
  assert.deepEqual(anchorInterview({ tokenRequestsMs: [] }).anchorMs, null);
  assert.match(anchorInterview({}).errors.join(), /anchor the run/);
  const split = anchorInterview({ clockStarts: [ms(0), ms(900)] });
  assert.match(split.errors.join(), /2 different starts/);
});

const passingStamps = (overrides = {}) => ({
  anchorMs: ms(0),
  launchMs: ms(-300),
  lastStimulusAtMs: ms(5270),
  observationEndMs: ms(5400),
  reportCompleteMs: ms(5400 + 200),
  teardownEndMs: ms(5400 + 200 + 30),
  ...overrides,
});

test("a run that stopped stimuli, observed the whole interview and reported in time passes", () => {
  const result = judgeTimestamps(t90, passingStamps());
  assert.deepEqual(result.reasons, []);
  assert.equal(result.verdict, "pass");
  assert.equal(result.fullInterview, true);
  assert.equal(result.exposureS, 5400);
  assert.deepEqual(result.timestamps, {
    lastStimulusS: 5270,
    observationEndS: 5400,
    reportCompleteS: 5600,
  });
});

test("each timestamp rule fails on its own", () => {
  const rows = [
    [{ lastStimulusAtMs: ms(5281) }, /spends the reserve/],
    [{ observationEndMs: ms(5399) }, /not a full 90 minute interview/],
    [{ observationEndMs: null }, /full observation never ended/],
    [{ reportCompleteMs: null }, /never completed/],
    [{ reportCompleteMs: ms(5399) }, /before the observation ended/],
    [{ reportCompleteMs: ms(5701) }, /over the 300s window/],
    [{ teardownEndMs: ms(5600 + 61) }, /teardown took longer than 60s/],
    [{ teardownEndMs: ms(5400 + 361 + 300) }, /over the 6060s cap/],
    [{ teardownEndMs: null }, /wall time of the run is unknown/],
    [{ launchMs: undefined }, /wall time of the run is unknown/],
  ];
  for (const [patch, pattern] of rows) {
    const result = judgeTimestamps(t90, passingStamps(patch));
    assert.equal(result.verdict, "fail", JSON.stringify(patch));
    assert.match(result.reasons.join(), pattern, JSON.stringify(patch));
  }
  // A last stimulus exactly at the boundary is allowed; one after it is not.
  assert.equal(
    judgeTimestamps(t90, passingStamps({ lastStimulusAtMs: ms(5280) })).verdict,
    "pass",
  );
});

test("a shorter observation is recorded with its actual exposure and is not full", () => {
  const result = judgeTimestamps(
    t90,
    passingStamps({ observationEndMs: ms(5000), reportCompleteMs: ms(5100) }),
  );
  assert.equal(result.fullInterview, false);
  assert.equal(result.exposureS, 5000);
  assert.equal(result.verdict, "fail");
});

test("a run with no anchor has no timestamps to judge", () => {
  const result = judgeTimestamps(t90, { anchorMs: null });
  assert.equal(result.verdict, "fail");
  assert.equal(result.exposureS, null);
});

const goaway = (startS, result, trigger = "goaway") => ({
  trigger,
  result,
  startInterviewMs: startS === null ? null : startS * 1000,
});

test("late terminal-preempted and idle rotations are classified and never count", () => {
  assert.equal(classifyRotation(goaway(1000, "installed"), t90), "completed");
  assert.equal(classifyRotation(goaway(5279, "installed"), t90), "completed");
  assert.equal(classifyRotation(goaway(5280, "installed"), t90), "late_idle");
  assert.equal(
    classifyRotation(goaway(5300, "cancelled"), t90),
    "late_terminal_preempted",
  );
  assert.equal(
    classifyRotation(goaway(5350, "incomplete"), t90),
    "late_terminal_preempted",
  );
  assert.equal(classifyRotation(goaway(900, "cancelled"), t90), "cancelled");
  assert.equal(classifyRotation(goaway(900, "failed"), t90), "failed");
  assert.equal(classifyRotation(goaway(900, "incomplete"), t90), "incomplete");
  assert.equal(
    classifyRotation(goaway(900, "installed", "watchdog"), t90),
    "not_a_goaway",
  );
  assert.equal(
    classifyRotation(goaway(null, "installed"), t90),
    "unknown_start",
  );
});

test("a rotation that failed after the boundary is a failure, not a preemption", () => {
  assert.equal(classifyRotation(goaway(5300, "failed"), t90), "late_failed");
  assert.equal(classifyRotation(goaway(5300, "no_setup"), t90), "late_failed");
  const result = judgeRotations(
    [
      goaway(600, "installed"),
      goaway(1300, "installed"),
      goaway(5300, "failed"),
    ],
    t90,
  );
  assert.equal(result.verdict, "fail");
  assert.match(result.reasons.join(), /failed after the stimulus boundary/);
});

test("two completed rotations are required and late ones do not make up the number", () => {
  const two = judgeRotations(
    [goaway(600, "installed"), goaway(1300, "installed")],
    t90,
  );
  assert.equal(two.verdict, "pass");
  const padded = judgeRotations(
    [
      goaway(600, "installed"),
      goaway(5290, "installed"),
      goaway(5300, "cancelled"),
    ],
    t90,
  );
  assert.equal(padded.verdict, "fail");
  assert.equal(padded.completed, 1);
  assert.deepEqual(padded.classes, {
    completed: 1,
    late_idle: 1,
    late_terminal_preempted: 1,
  });
  assert.match(padded.reasons.join(), /2 late rotation\(s\) do not count/);
  assert.equal(judgeRotations([], t90).verdict, "fail");
});

const clean = () => ({
  goAways: [{ atS: 900, resolvedAtS: 930 }],
  finalQuestion: { askedAtS: 5250, repliedAtS: 5290 },
  logs: { truncated: false, evicted: false, complete: true },
  performanceReceived: true,
  reportReceived: true,
  tokenExpired: false,
  browserDisconnected: false,
  cleanup: { errors: [], leftover: { agents: 0, processes: 0, rooms: 0 } },
  budgetExhausted: false,
});

test("a clean run has no artifact and exits 0", () => {
  const resolved = resolveRun(clean(), t90);
  assert.deepEqual(resolved, {
    status: "complete",
    exit_code: 0,
    artifacts: [],
  });
});

// Each failure the leaf names gets its own class and a failed or incomplete
// artifact, never a pass.
const failureRows = [
  [
    "near_boundary_goaway",
    { goAways: [{ atS: 5260, resolvedAtS: null }] },
    "failed",
    1,
  ],
  [
    "near_boundary_goaway",
    { goAways: [{ atS: 5260, resolvedAtS: 5450 }] },
    "failed",
    1,
  ],
  [
    "pending_final_reply",
    { finalQuestion: { askedAtS: 5250, repliedAtS: null } },
    "failed",
    1,
  ],
  [
    "pending_final_reply",
    { finalQuestion: { askedAtS: 5250, repliedAtS: 5401 } },
    "failed",
    1,
  ],
  [
    "truncated_logs",
    { logs: { truncated: true, evicted: false, complete: false } },
    "incomplete_artifact",
    6,
  ],
  [
    "evicted_logs",
    { logs: { truncated: false, evicted: true, complete: false } },
    "incomplete_artifact",
    6,
  ],
  ["truncated_logs", { logs: {} }, "incomplete_artifact", 6],
  ["missing_performance", { performanceReceived: false }, "failed", 1],
  ["missing_performance", { performanceReceived: undefined }, "failed", 1],
  ["missing_report", { reportReceived: false }, "failed", 1],
  ["token_expiry", { tokenExpired: true }, "failed", 1],
  ["browser_disconnect", { browserDisconnected: true }, "failed", 1],
  [
    "cleanup_error",
    {
      cleanup: {
        errors: ["room delete refused"],
        leftover: { agents: 0, processes: 0, rooms: 0 },
      },
    },
    "incomplete_cleanup",
    5,
  ],
  [
    "cleanup_error",
    {
      cleanup: { errors: [], leftover: { agents: 0, processes: 1, rooms: 0 } },
    },
    "incomplete_cleanup",
    5,
  ],
  ["cleanup_error", { cleanup: { errors: [] } }, "incomplete_cleanup", 5],
  ["budget_exhausted", { budgetExhausted: true }, "incomplete_budget", 4],
];

test("every named late-session failure has its class, status and exit code", () => {
  for (const [kind, patch, status, code] of failureRows) {
    const resolved = resolveRun({ ...clean(), ...patch }, t90);
    const found = resolved.artifacts.find((a) => a.class === kind);
    assert.ok(found, `${kind} ${JSON.stringify(patch)}`);
    assert.equal(found.status, status, kind);
    assert.equal(found.exit_code, code, kind);
    assert.equal(resolved.exit_code, code, kind);
    assert.notEqual(resolved.status, "complete", kind);
    assert.ok(found.reasons.length > 0);
  }
  // The table above covers every class the module defines except no_goaway.
  const covered = new Set(failureRows.map((row) => row[0]));
  for (const name of Object.keys(FAILURE_CLASSES)) {
    if (name !== "no_goaway")
      assert.ok(covered.has(name), `${name} is untested`);
  }
});

test("evidence a run does not state is a failure, not a pass", () => {
  const without = (name) => {
    const ev = clean();
    delete ev[name];
    return resolveRun(ev, t90);
  };
  const final = without("finalQuestion");
  assert.equal(final.exit_code, 1);
  assert.ok(final.artifacts.some((a) => a.class === "pending_final_reply"));
  const goAways = without("goAways");
  assert.equal(goAways.exit_code, EXIT.incompleteArtifact);
  assert.notEqual(goAways.exit_code, EXIT.noGoAway);
  const untimed = resolveRun({ ...clean(), goAways: [{}] }, t90);
  assert.ok(untimed.artifacts.some((a) => a.class === "near_boundary_goaway"));
});

test("a GoAway resolved before the end of the observation is not a boundary failure", () => {
  const resolved = resolveRun(
    { ...clean(), goAways: [{ atS: 5260, resolvedAtS: 5300 }] },
    t90,
  );
  assert.equal(resolved.exit_code, 0);
});

test("no GoAway exits 77, and only when nothing else went wrong", () => {
  const quiet = resolveRun({ ...clean(), goAways: [] }, t90);
  assert.equal(quiet.exit_code, 77);
  assert.equal(quiet.status, "inconclusive");
  const broken = resolveRun(
    { ...clean(), goAways: [], performanceReceived: false },
    t90,
  );
  assert.equal(broken.exit_code, 1);
  assert.equal(
    broken.artifacts.some((a) => a.class === "no_goaway"),
    false,
  );
});

test("the exit code names the most serious thing and every class keeps its artifact", () => {
  const resolved = resolveRun(
    {
      ...clean(),
      browserDisconnected: true,
      logs: { truncated: true },
      budgetExhausted: true,
      cleanup: {
        errors: ["x"],
        leftover: { agents: 1, processes: 0, rooms: 0 },
      },
    },
    t90,
  );
  assert.equal(resolved.exit_code, EXIT.incompleteCleanup);
  assert.equal(resolved.status, "incomplete_cleanup");
  assert.deepEqual(resolved.artifacts.map((a) => a.class).sort(), [
    "browser_disconnect",
    "budget_exhausted",
    "cleanup_error",
    "truncated_logs",
  ]);
  const noCleanup = resolveRun(
    { ...clean(), budgetExhausted: true, logs: { truncated: true } },
    t90,
  );
  assert.equal(noCleanup.exit_code, EXIT.incompleteArtifact);
  const budgetOnly = resolveRun({ ...clean(), budgetExhausted: true }, t90);
  assert.equal(budgetOnly.exit_code, EXIT.incompleteBudget);
  assert.equal(budgetOnly.status, "incomplete_budget");
});

test("agent presence after the deliberate end is not asked for", () => {
  const ev = { ...clean(), agentPresentAfterEnd: false };
  assert.equal(classifyEvidence(ev, t90).length, 0);
});

test("failure artifacts are written one per class and none for a clean run", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "lit-art-"));
  try {
    assert.deepEqual(writeFailureArtifacts(dir, resolveRun(clean(), t90)), []);
    const resolved = resolveRun(
      { ...clean(), tokenExpired: true, performanceReceived: false },
      t90,
    );
    const files = writeFailureArtifacts(dir, resolved).map((f) =>
      path.basename(f),
    );
    assert.deepEqual(files.sort(), [
      "failure-missing_performance.json",
      "failure-token_expiry.json",
    ]);
    const body = JSON.parse(
      fs.readFileSync(path.join(dir, "failure-token_expiry.json"), "utf8"),
    );
    assert.equal(body.status, "failed");
    assert.equal(body.exit_code, 1);
    // A later clean run leaves nothing of the earlier failure behind.
    assert.deepEqual(writeFailureArtifacts(dir, resolveRun(clean(), t90)), []);
    assert.deepEqual(
      fs.readdirSync(dir).filter((name) => name.startsWith("failure-")),
      [],
    );
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("the CLI prints a plan and refuses a reserve its grace outgrows", () => {
  const run = (...args) =>
    spawnSync(
      process.execPath,
      [path.join(ROOT, "scripts", "long-interview-timing.cjs"), ...args],
      { encoding: "utf8" },
    );
  const ok = run("plan", "--duration-min", "60");
  assert.equal(ok.status, 0, ok.stderr);
  assert.equal(JSON.parse(ok.stdout).wallCapS, 3600 + 660);
  const refused = run(
    "plan",
    "--duration-min",
    "60",
    "--resolved-grace-s",
    "95",
  );
  assert.equal(refused.status, 2);
  assert.match(refused.stderr, /shorter than the resolved handover grace 95s/);
  // Bad arguments are a refusal (2), never the exit of a failed run (1), and an
  // empty grace is not read as zero.
  for (const args of [
    ["plan", "--duration-min", "abc"],
    ["plan"],
    ["plan", "--duration-min", "60", "--resolved-grace-s", ""],
    ["validate-manifest", "--manifest", "/nonexistent/manifest.json"],
  ]) {
    const bad = run(...args);
    assert.equal(bad.status, 2, args.join(" "));
  }
});
