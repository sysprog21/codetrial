// The clock, the budget windows and the failure verdicts of
// BROWSER_CHECK_FLOW=long-interview, as pure functions.
//
// A long interview has three moments that are easy to run together and must
// not be: the last stimulus the harness is allowed to send, the end of the
// full interview observation, and the moment the end and report work is done.
// Reading the first as the second would call a short observation a full
// interview; reading the third as the second would count the report's wait as
// interview time. This file names each, derives the windows around them, and
// turns the ways a late session loses its evidence into an artifact with a
// status and an exit code instead of a pass.
//
// Nothing here opens a browser or claims a run happened. The numbers are
// harness ceilings, not production latency guarantees.
"use strict";

const fs = require("fs");
const path = require("path");
const {
  FINAL_BEFORE_WINDOW_END_S,
  STOP_BEFORE_END_S,
} = require("./long-interview-schedule.cjs");

/// Up to this much of the existing serial handover and progress grace can still
/// be spent by a rotation that began just before the last stimulus.
const HANDOVER_GRACE_S = 90;
/// Margin past that grace, so a stimulus never lands in it.
const STIMULUS_MARGIN_S = 30;
/// No stimulus is sent in the last STIMULUS_RESERVE_S of the selected duration.
const STIMULUS_RESERVE_S = STOP_BEFORE_END_S;
/// The final question starts this long before the stimulus boundary.
const FINAL_QUESTION_BEFORE_BOUNDARY_S = FINAL_BEFORE_WINDOW_END_S;

/// Budget windows outside the selected interview duration.
const SETUP_BUDGET_S = 300;
const END_REPORT_BUDGET_S = 300;
const TEARDOWN_BUDGET_S = 60;
const WALL_OVERHEAD_S =
  SETUP_BUDGET_S + END_REPORT_BUDGET_S + TEARDOWN_BUDGET_S;
/// The reserve the budget controller holds back at the end of the wall cap:
/// the end and report window and the teardown.
const CLEANUP_RESERVE_S = END_REPORT_BUDGET_S + TEARDOWN_BUDGET_S;
/// The report generator's own limit. It bounds one step of the end window, not
/// the settlement of evidence, the farewell, the delivery or the cleanup.
const REPORT_GENERATION_LIMIT_S = 125;

/// Mirrors of the constants in scripts/interview-run-budget.py; a test reads
/// both and fails when they differ.
const RESOURCE_CADENCE_S = 30;
const MAX_SAMPLE_LINE_BYTES = 4096;
const CONTROL_FILES_ALLOWANCE_BYTES = 256 * 1024;
const ARCHIVE_EVIDENCE_ALLOWANCE_BYTES = 256 * 1024 * 1024;

const EXIT = Object.freeze({
  pass: 0,
  failed: 1,
  incompleteBudget: 4,
  incompleteCleanup: 5,
  incompleteArtifact: 6,
  /// Gemini sent no GoAway: nothing was learned about rotation, which is
  /// neither a pass nor a failure of it.
  noGoAway: 77,
});

const isInt = (value) => Number.isSafeInteger(value);

/// The windows of a run of `durationMin` minutes, in seconds on the interview
/// clock unless the name says otherwise.
function timingFor(durationMin, options = {}) {
  if (!isInt(durationMin) || durationMin <= 0) {
    throw new Error("durationMin must be a positive integer");
  }
  const reserveS = options.reserveS ?? STIMULUS_RESERVE_S;
  const durationS = durationMin * 60;
  const lastStimulusBoundaryS = durationS - reserveS;
  return {
    durationS,
    stimulusReserveS: reserveS,
    lastStimulusBoundaryS,
    finalQuestionAtS: lastStimulusBoundaryS - FINAL_QUESTION_BEFORE_BOUNDARY_S,
    observationEndS: durationS,
    setupBudgetS: SETUP_BUDGET_S,
    endReportBudgetS: END_REPORT_BUDGET_S,
    teardownBudgetS: TEARDOWN_BUDGET_S,
    wallCapS: durationS + WALL_OVERHEAD_S,
    cleanupReserveS: CLEANUP_RESERVE_S,
  };
}

/// Why a stimulus reserve cannot be trusted against the settings the server
/// resolved for this run. The reserve must cover the grace a rotation may
/// still spend plus the margin, whatever the settings say.
function validateReserve({
  reserveS = STIMULUS_RESERVE_S,
  resolvedGraceS = HANDOVER_GRACE_S,
  marginS = STIMULUS_MARGIN_S,
} = {}) {
  const errors = [];
  if (!isInt(resolvedGraceS) || resolvedGraceS < 0) {
    errors.push("the resolved handover grace is not a whole number of seconds");
    return errors;
  }
  if (!isInt(reserveS) || reserveS < resolvedGraceS + marginS) {
    errors.push(
      `the stimulus reserve ${reserveS}s is shorter than the resolved handover grace ${resolvedGraceS}s plus the ${marginS}s margin`,
    );
  }
  return errors;
}

/// The archive bound for a wall cap, as scripts/interview-run-budget.py
/// derives it: one sample per cadence plus the first and the last at their
/// worst-case size, plus what the controller itself writes. The ceiling is the
/// floor plus the evidence allowance, so the bound is finite and derived.
function archiveBounds(wallCapS, cadenceS = RESOURCE_CADENCE_S) {
  const samples = Math.floor(wallCapS / cadenceS) + 2;
  const floor = samples * MAX_SAMPLE_LINE_BYTES + CONTROL_FILES_ALLOWANCE_BYTES;
  return { samples, floor, ceiling: floor + ARCHIVE_EVIDENCE_ALLOWANCE_BYTES };
}

/// Every reason the manifest's wall windows and archive bound do not fit a long
/// interview. The cap is the selected duration plus setup, end and report, and
/// teardown, each its own window; the reserve is the last two.
function validateBudgetWindows(manifest) {
  const errors = [];
  const selected = manifest.selected_duration_s;
  const cap = manifest.outer_wall_cap_s;
  const reserve = manifest.cleanup_reserve_s;
  if (!isInt(selected) || !isInt(cap) || !isInt(reserve)) {
    return [
      "selected_duration_s, outer_wall_cap_s and cleanup_reserve_s must be integers",
    ];
  }
  if (cap !== selected + WALL_OVERHEAD_S) {
    errors.push(
      `outer_wall_cap_s ${cap} is not the selected duration ${selected} plus ${WALL_OVERHEAD_S}s (setup ${SETUP_BUDGET_S}, end and report ${END_REPORT_BUDGET_S}, teardown ${TEARDOWN_BUDGET_S})`,
    );
  }
  if (reserve !== CLEANUP_RESERVE_S) {
    errors.push(
      `cleanup_reserve_s ${reserve} is not the end and report window plus teardown, ${CLEANUP_RESERVE_S}s`,
    );
  }
  const limit = manifest.max_archive_bytes;
  if (!isInt(limit) || limit <= 0) {
    errors.push("max_archive_bytes must be a positive integer");
  } else {
    const { floor, ceiling } = archiveBounds(cap);
    if (limit < floor) {
      errors.push(
        `max_archive_bytes ${limit} is below the ${floor} the sampler and controller files need`,
      );
    } else if (limit > ceiling) {
      errors.push(
        `max_archive_bytes ${limit} is above the ${ceiling} derived from the wall cap`,
      );
    }
  }
  return errors;
}

/// The one start of a run, from the evidence that cannot be reset by a page
/// reload: the `start_unix_ms` every interview clock sample repeats, and
/// failing that the first token request. A reload requests another token and
/// re-reads the countdown, so neither of those may move the anchor.
///
/// `clockStarts`: the `start_unix_ms` values of the clock samples.
/// `tokenRequestsMs`: the wall time of every `/api/token` request.
function anchorInterview({ clockStarts = [], tokenRequestsMs = [] }) {
  const errors = [];
  const starts = [...new Set(clockStarts.filter(Number.isFinite))];
  if (starts.length > 1) {
    errors.push(
      `the interview clock named ${starts.length} different starts: ${starts.join(", ")}`,
    );
  }
  const requests = tokenRequestsMs
    .filter(Number.isFinite)
    .sort((a, b) => a - b);
  if (starts.length === 1) {
    return {
      anchorMs: starts[0],
      source: "server_clock",
      reloads: Math.max(0, requests.length - 1),
      errors,
    };
  }
  if (starts.length === 0 && requests.length > 0) {
    return {
      anchorMs: requests[0],
      source: "first_token_request",
      reloads: requests.length - 1,
      errors,
    };
  }
  if (errors.length === 0) {
    errors.push("no server clock start and no token request anchor the run");
  }
  return { anchorMs: null, source: "none", reloads: 0, errors };
}

/// Judges the three timestamps and the wall budget of one run, and records the
/// exposure that actually happened.
///
/// `ts` (wall milliseconds): anchorMs, lastStimulusAtMs (null when the run sent
/// none), observationEndMs, reportCompleteMs (null when the end and report work
/// never finished), launchMs and teardownEndMs for the wall cap.
function judgeTimestamps(timing, ts) {
  const reasons = [];
  const at = (ms) => (Number.isFinite(ms) ? (ms - ts.anchorMs) / 1000 : null);
  if (!Number.isFinite(ts.anchorMs)) {
    return {
      verdict: "fail",
      reasons: ["the run has no anchor to measure its timestamps from"],
      timestamps: {},
      exposureS: null,
      fullInterview: false,
    };
  }
  const lastStimulusS = at(ts.lastStimulusAtMs);
  const observationEndS = at(ts.observationEndMs);
  const reportCompleteS = at(ts.reportCompleteMs);
  if (lastStimulusS !== null && lastStimulusS > timing.lastStimulusBoundaryS) {
    reasons.push(
      `a stimulus was sent at ${lastStimulusS}s, after the ${timing.lastStimulusBoundaryS}s boundary: it spends the reserve`,
    );
  }
  if (observationEndS === null) {
    reasons.push("the full observation never ended");
  } else if (observationEndS < timing.durationS) {
    reasons.push(
      `the observation ended at ${observationEndS}s, short of the ${timing.durationS}s interview: it is not a full ${timing.durationS / 60} minute interview`,
    );
  }
  if (reportCompleteS === null) {
    reasons.push("the end and report work never completed");
  } else if (observationEndS !== null) {
    if (reportCompleteS < observationEndS) {
      reasons.push("the report completed before the observation ended");
    } else if (reportCompleteS - observationEndS > timing.endReportBudgetS) {
      reasons.push(
        `end and report took ${reportCompleteS - observationEndS}s, over the ${timing.endReportBudgetS}s window`,
      );
    }
  }
  // Wall time and teardown are unknown without their timestamps, and an
  // unknown is not within budget.
  if (!Number.isFinite(ts.teardownEndMs) || !Number.isFinite(ts.launchMs)) {
    reasons.push(
      "the wall time of the run is unknown: no launch or teardown timestamp",
    );
  } else {
    const wall = (ts.teardownEndMs - ts.launchMs) / 1000;
    if (wall > timing.wallCapS) {
      reasons.push(
        `the run used ${wall}s of wall time, over the ${timing.wallCapS}s cap`,
      );
    }
    if (
      Number.isFinite(ts.reportCompleteMs) &&
      (ts.teardownEndMs - ts.reportCompleteMs) / 1000 > timing.teardownBudgetS
    ) {
      reasons.push(`teardown took longer than ${timing.teardownBudgetS}s`);
    }
  }
  return {
    verdict: reasons.length ? "fail" : "pass",
    reasons,
    timestamps: {
      lastStimulusS,
      observationEndS,
      reportCompleteS,
    },
    // What the interview actually spent, not what was selected.
    exposureS: observationEndS,
    fullInterview:
      observationEndS !== null && observationEndS >= timing.durationS,
  };
}

/// Classes of a replacement episode against the stimulus boundary.
///
/// A rotation counts toward the completed minimum only when a GoAway-triggered
/// replacement installed a socket before the boundary. One begun at or after
/// it is late: the stimuli had stopped, so the room was idle or the end was
/// preempting it, and neither tests a handover under load.
function classifyRotation(episode, timing) {
  const startS = Number.isFinite(episode.startInterviewMs)
    ? episode.startInterviewMs / 1000
    : null;
  const late = startS !== null && startS >= timing.lastStimulusBoundaryS;
  if (episode.trigger !== "goaway") return "not_a_goaway";
  if (startS === null) return "unknown_start";
  if (late) {
    if (episode.result === "installed") return "late_idle";
    if (episode.result === "failed" || episode.result === "no_setup") {
      return "late_failed";
    }
    return "late_terminal_preempted";
  }
  switch (episode.result) {
    case "installed":
      return "completed";
    case "cancelled":
      return "cancelled";
    case "incomplete":
      return "incomplete";
    default:
      return "failed";
  }
}

function judgeRotations(episodes, timing, { minCompleted = 2 } = {}) {
  const classes = {};
  const rows = episodes.map((episode) => {
    const kind = classifyRotation(episode, timing);
    classes[kind] = (classes[kind] ?? 0) + 1;
    return { ...episode, class: kind };
  });
  const completed = classes.completed ?? 0;
  const reasons = [];
  if (classes.late_failed) {
    reasons.push(
      `${classes.late_failed} rotation(s) failed after the stimulus boundary`,
    );
  }
  if (completed < minCompleted) {
    const late =
      (classes.late_idle ?? 0) + (classes.late_terminal_preempted ?? 0);
    reasons.push(
      `${completed} completed rotation(s), fewer than the ${minCompleted} required` +
        (late ? `; ${late} late rotation(s) do not count` : ""),
    );
  }
  return {
    verdict: reasons.length ? "fail" : "pass",
    reasons,
    completed,
    classes,
    rows,
  };
}

const FAILURE_CLASSES = Object.freeze({
  near_boundary_goaway: { status: "failed", exit: EXIT.failed },
  pending_final_reply: { status: "failed", exit: EXIT.failed },
  truncated_logs: {
    status: "incomplete_artifact",
    exit: EXIT.incompleteArtifact,
  },
  evicted_logs: {
    status: "incomplete_artifact",
    exit: EXIT.incompleteArtifact,
  },
  missing_performance: { status: "failed", exit: EXIT.failed },
  missing_report: { status: "failed", exit: EXIT.failed },
  token_expiry: { status: "failed", exit: EXIT.failed },
  browser_disconnect: { status: "failed", exit: EXIT.failed },
  cleanup_error: { status: "incomplete_cleanup", exit: EXIT.incompleteCleanup },
  budget_exhausted: {
    status: "incomplete_budget",
    exit: EXIT.incompleteBudget,
  },
  no_goaway: { status: "inconclusive", exit: EXIT.noGoAway },
});

/// What the run's evidence says went wrong, each as its own class with the
/// reasons behind it. Anything the evidence does not state is unknown, and an
/// unknown is a failure for the evidence a verdict needs: the log, the
/// performance packet, the report and the cleanup. It is never read as fine.
///
/// `ev`: goAways [{atS, resolvedAtS}], finalQuestion {askedAtS, repliedAtS},
/// logs {truncated, evicted, complete}, performanceReceived, reportReceived,
/// tokenExpired, browserDisconnected, cleanup {errors, leftover {agents,
/// processes, rooms}}, budgetExhausted. Agent presence after the deliberate
/// end is not asked for.
function classifyEvidence(ev, timing) {
  const found = [];
  const add = (kind, ...reasons) => found.push({ kind, reasons });

  if (!Array.isArray(ev.goAways)) {
    add("truncated_logs", "whether Gemini sent a GoAway is unknown");
  }
  for (const goAway of Array.isArray(ev.goAways) ? ev.goAways : []) {
    const resolved = Number.isFinite(goAway.resolvedAtS);
    if (!Number.isFinite(goAway.atS)) {
      add(
        "near_boundary_goaway",
        "a GoAway has no time on the interview clock",
      );
    } else if (
      goAway.atS >= timing.finalQuestionAtS &&
      (!resolved || goAway.resolvedAtS > timing.observationEndS)
    ) {
      add(
        "near_boundary_goaway",
        `a GoAway at ${goAway.atS}s had no handover by the ${timing.observationEndS}s end of the observation`,
      );
    }
  }
  const final = ev.finalQuestion;
  if (!final || !Number.isFinite(final.askedAtS)) {
    add("pending_final_reply", "no final question was observed to be asked");
  } else {
    const answered =
      Number.isFinite(final.repliedAtS) &&
      final.repliedAtS <= timing.observationEndS;
    if (!answered) {
      add(
        "pending_final_reply",
        `the final question asked at ${final.askedAtS}s had no reply by the end of the observation`,
      );
    }
  }
  const logs = ev.logs ?? {};
  if (logs.truncated) add("truncated_logs", "a required log was cut short");
  if (logs.evicted) add("evicted_logs", "lines of a required log were evicted");
  if (!logs.truncated && !logs.evicted && logs.complete !== true) {
    add("truncated_logs", "the completeness of the required logs is unknown");
  }
  if (ev.performanceReceived !== true) {
    add("missing_performance", "no performance packet was received");
  }
  if (ev.reportReceived !== true) {
    add("missing_report", "no report was received");
  }
  if (ev.tokenExpired)
    add("token_expiry", "the join token expired during the run");
  if (ev.browserDisconnected) {
    add("browser_disconnect", "the browser disconnected during the run");
  }
  const cleanup = ev.cleanup ?? {};
  const leftover = cleanup.leftover;
  const cleanupReasons = (cleanup.errors ?? []).map(
    (e) => `cleanup error: ${e}`,
  );
  if (
    !leftover ||
    !["agents", "processes", "rooms"].every((name) => isInt(leftover[name]))
  ) {
    cleanupReasons.push(
      "whether any agent, process or room was left active is unknown",
    );
  } else {
    for (const name of ["agents", "processes", "rooms"]) {
      if (leftover[name] > 0) {
        cleanupReasons.push(`${leftover[name]} ${name} left active`);
      }
    }
  }
  if (cleanupReasons.length) add("cleanup_error", ...cleanupReasons);
  if (ev.budgetExhausted)
    add("budget_exhausted", "the run budget was exhausted");
  if (found.length === 0 && ev.goAways.length === 0) {
    add("no_goaway", "Gemini sent no GoAway, so no rotation was exercised");
  }
  return found;
}

/// The artifacts, status and exit code of a run. Cleanup outranks the artifact,
/// which outranks the budget, which outranks a plain failure, which outranks
/// the inconclusive no-GoAway: the exit code names the most serious thing that
/// happened, and every class still has its own artifact.
function resolveRun(ev, timing) {
  const found = classifyEvidence(ev, timing);
  const artifacts = found.map(({ kind, reasons }) => ({
    class: kind,
    status: FAILURE_CLASSES[kind].status,
    exit_code: FAILURE_CLASSES[kind].exit,
    reasons,
  }));
  const order = [
    EXIT.incompleteCleanup,
    EXIT.incompleteArtifact,
    EXIT.incompleteBudget,
    EXIT.failed,
    EXIT.noGoAway,
  ];
  const exit = order.find((code) =>
    artifacts.some((a) => a.exit_code === code),
  );
  const top = artifacts.find((a) => a.exit_code === exit);
  return {
    status: top ? top.status : "complete",
    exit_code: exit ?? EXIT.pass,
    artifacts,
  };
}

/// Writes one `failure-<class>.json` per class. Nothing is written for a clean
/// run, and what an earlier run left in the directory is removed first, so the
/// files present are always this run's. A failure never lands as a pass
/// because its artifact could not be written: the error propagates.
function writeFailureArtifacts(dir, resolved) {
  fs.mkdirSync(dir, { recursive: true });
  for (const name of fs.readdirSync(dir)) {
    if (/^failure-.+\.json$/.test(name)) fs.rmSync(path.join(dir, name));
  }
  const written = [];
  for (const artifact of resolved.artifacts) {
    const file = path.join(dir, `failure-${artifact.class}.json`);
    fs.writeFileSync(file, `${JSON.stringify(artifact, null, 2)}\n`);
    written.push(file);
  }
  return written;
}

const USAGE =
  "usage: long-interview-timing.cjs plan --duration-min N [--resolved-grace-s S] | validate-manifest --manifest FILE";

/// A whole number from the command line, or NaN: an empty or fractional value
/// is refused rather than read as zero.
const wholeFlag = (raw, fallback) =>
  raw === undefined
    ? fallback
    : /^[0-9]+$/.test(raw)
      ? Number(raw)
      : Number.NaN;

function run(argv) {
  const [command, ...rest] = argv;
  const flags = {};
  for (let i = 0; i < rest.length; i += 2) {
    flags[rest[i].replace(/^--/, "")] = rest[i + 1];
  }
  if (command === "plan") {
    const timing = timingFor(wholeFlag(flags["duration-min"], Number.NaN));
    const problems = validateReserve({
      reserveS: timing.stimulusReserveS,
      resolvedGraceS: wholeFlag(flags["resolved-grace-s"], HANDOVER_GRACE_S),
    });
    if (problems.length) {
      for (const problem of problems) console.error(problem);
      return 2;
    }
    console.log(JSON.stringify(timing, null, 2));
    return 0;
  }
  if (command === "validate-manifest") {
    const manifest = JSON.parse(fs.readFileSync(flags.manifest, "utf8"));
    const errors = validateBudgetWindows(manifest);
    for (const error of errors) console.error(error);
    return errors.length ? 2 : 0;
  }
  console.error(USAGE);
  return 2;
}

/// Bad arguments are exit 2, the same as a refused environment, and never 1,
/// which is a failed run.
function main(argv) {
  try {
    return run(argv);
  } catch (error) {
    console.error(`${error.message}\n${USAGE}`);
    return 2;
  }
}

if (require.main === module) process.exit(main(process.argv.slice(2)));

module.exports = {
  isInt,
  ARCHIVE_EVIDENCE_ALLOWANCE_BYTES,
  CLEANUP_RESERVE_S,
  CONTROL_FILES_ALLOWANCE_BYTES,
  END_REPORT_BUDGET_S,
  EXIT,
  FAILURE_CLASSES,
  FINAL_QUESTION_BEFORE_BOUNDARY_S,
  HANDOVER_GRACE_S,
  MAX_SAMPLE_LINE_BYTES,
  REPORT_GENERATION_LIMIT_S,
  RESOURCE_CADENCE_S,
  SETUP_BUDGET_S,
  STIMULUS_MARGIN_S,
  STIMULUS_RESERVE_S,
  TEARDOWN_BUDGET_S,
  WALL_OVERHEAD_S,
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
};
