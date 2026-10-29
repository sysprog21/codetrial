// The configuration and the verdict of BROWSER_CHECK_FLOW=long-interview.
//
// A long-interview claim is only worth what the interview it observed was
// worth, so everything that could shorten, stretch or clamp that interview is
// refused here before a candidate joins: the duration, the run kind, how the
// run ends, and the observation target. Pure functions, no browser, so the
// shell entry point and the CJS entry point can both reach them without
// credentials.
//
// The manifest schema belongs to scripts/interview-run-budget.py; what its
// fields mean for a browser run is decided here.
"use strict";

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");
const {
  STIMULUS_RESERVE_S,
  isInt,
  validateBudgetWindows,
  validateReserve,
} = require("./long-interview-timing.cjs");

/// The product's interview range, `MIN_DURATION_MIN` and `MAX_DURATION_MIN` in
/// src/config.rs. Mirrored, not widened: a contract test reads both.
const PRODUCT_MIN_DURATION_MIN = 10;
const PRODUCT_MAX_DURATION_MIN = 90;

/// Durations the harness will run. The default matches the page's own.
const DEFAULT_DURATION_MIN = 20;
const HARNESS_DURATIONS_MIN = [10, 20, 30, 60, 90];

/// `INTERVIEW_DEADLINE_GRACE` in src/livekit.rs: how long past the planned
/// duration the agent waits before ending the interview itself.
const SERVER_DEADLINE_GRACE_S = 120;

/// `CLOCK_SAMPLE_MS` in src/livekit/latency.rs.
const CLOCK_SAMPLE_S = 60;

/// A run shorter than this has no clock sample to find.
const CLOCK_EVIDENCE_FROM_S = 2 * CLOCK_SAMPLE_S;

const RUN_KINDS = ["smoke", "pilot", "qualification", "experiment", "clip"];
const END_MODES = [
  "manual",
  "server_deadline",
  "absence",
  "terminal_cancel",
  "native_close",
];

/// The end modes each kind of browser run may use. `explicit_end` is the
/// pre-run-kind spelling of `manual` and is not a long interview.
const KIND_END_MODES = {
  smoke: ["manual", "server_deadline"],
  pilot: ["manual"],
  qualification: ["manual"],
  experiment: ["manual"],
};

/// The duration each qualifying kind is pinned to. The experiment's frozen
/// profile is not defined anywhere in this repository, so it is a manifest
/// field (`experiment_profile_min`) that must name one of these two lengths and
/// agree with the duration, rather than a number chosen here.
const KIND_DURATION_MIN = { pilot: 60, qualification: 90 };
const EXPERIMENT_PROFILE_MIN = [20, 30];

const ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
const COUNT_PATTERN = /^[1-9][0-9]*$/;

function parseDurationMin(raw) {
  if (raw === undefined) return { value: DEFAULT_DURATION_MIN };
  if (!COUNT_PATTERN.test(raw)) {
    return {
      error: `BROWSER_CHECK_DURATION_MIN must be a whole number of minutes, not ${JSON.stringify(raw)}`,
    };
  }
  const value = Number(raw);
  if (value < PRODUCT_MIN_DURATION_MIN || value > PRODUCT_MAX_DURATION_MIN) {
    return {
      error: `BROWSER_CHECK_DURATION_MIN=${value} is outside the product range ${PRODUCT_MIN_DURATION_MIN}..${PRODUCT_MAX_DURATION_MIN} minutes`,
    };
  }
  if (!HARNESS_DURATIONS_MIN.includes(value)) {
    return {
      error: `BROWSER_CHECK_DURATION_MIN must be one of ${HARNESS_DURATIONS_MIN.join(", ")}, not ${value}`,
    };
  }
  return { value };
}

/// The recording cap the server would apply to the token's duration, from the
/// environment the server inherits or the config file it is given.
function recordingCapMin(env, readFile) {
  let raw = env.CODETRIAL_RECORDING_MAX_MINUTES;
  if (raw === undefined && env.BROWSER_CHECK_CONFIG_PATH) {
    let text = "";
    try {
      text = readFile(env.BROWSER_CHECK_CONFIG_PATH);
    } catch {
      // An unreadable config is the server's problem to report.
    }
    for (const line of text.split("\n")) {
      const match =
        /^\s*(?:export\s+)?CODETRIAL_RECORDING_MAX_MINUTES=(.*)$/.exec(line);
      if (match) raw = match[1].trim().replace(/^(["'])(.*)\1$/, "$2");
    }
  }
  if (raw === undefined) return { cap: null };
  if (!COUNT_PATTERN.test(raw)) {
    return {
      error: `CODETRIAL_RECORDING_MAX_MINUTES must be a whole number of minutes, not ${JSON.stringify(raw)}`,
    };
  }
  return { cap: Number(raw) };
}

/// `duration_ceiling` in src/web/token.rs: what the token endpoint would clamp
/// a request to where recording is on.
function tokenCeilingMin(cap) {
  if (cap === null) return PRODUCT_MAX_DURATION_MIN;
  return Math.min(
    PRODUCT_MAX_DURATION_MIN,
    Math.max(PRODUCT_MIN_DURATION_MIN, cap),
  );
}

/// Every reason the manifest cannot drive a browser long interview of
/// `durationMin` minutes. Empty when it can.
function validateManifest(manifest, durationMin) {
  const errors = [];
  if (manifest === null || typeof manifest !== "object") {
    return ["manifest must be a JSON object"];
  }
  const kind = manifest.run_kind;
  const endMode = manifest.end_mode;
  if (!RUN_KINDS.includes(kind)) {
    return [`run_kind must be one of ${RUN_KINDS.join(", ")}`];
  }
  if (kind === "clip" || endMode === "native_close") {
    errors.push(
      "clip runs and native_close are native-worker-only; a browser run cannot use them",
    );
  } else if (endMode === "explicit_end") {
    errors.push("end_mode explicit_end is not a long interview; use manual");
  } else if (!END_MODES.includes(endMode)) {
    errors.push(`end_mode must be one of ${END_MODES.join(", ")}`);
  } else if (!KIND_END_MODES[kind].includes(endMode)) {
    errors.push(
      `a ${kind} run cannot end by ${endMode}; allowed: ${KIND_END_MODES[kind].join(", ")}`,
    );
  }
  if (errors.length) return errors;

  if (manifest.duration_min !== durationMin) {
    errors.push(
      `manifest duration_min ${JSON.stringify(manifest.duration_min)} does not match BROWSER_CHECK_DURATION_MIN ${durationMin}`,
    );
  }
  const target = manifest.observation_target_s;
  if (!isInt(target) || target <= 0) {
    errors.push("observation_target_s must be a positive integer");
    return errors;
  }
  const full = durationMin * 60;
  if (kind === "smoke") {
    if (endMode === "server_deadline") {
      if (target !== full + SERVER_DEADLINE_GRACE_S) {
        errors.push(
          `a deadline smoke observes the duration plus ${SERVER_DEADLINE_GRACE_S}s (${full + SERVER_DEADLINE_GRACE_S}), not ${target}`,
        );
      }
    } else if (target > full) {
      errors.push(
        `smoke observation_target_s ${target} is longer than the ${full}s interview`,
      );
    }
  } else if (target !== full) {
    errors.push(
      `a ${kind} run must observe the whole interview: observation_target_s ${target} is not ${full}`,
    );
  }
  if (KIND_DURATION_MIN[kind] && durationMin !== KIND_DURATION_MIN[kind]) {
    errors.push(
      `a ${kind} run requires a ${KIND_DURATION_MIN[kind]} minute interview, not ${durationMin}`,
    );
  }
  if (kind === "experiment") {
    const profile = manifest.experiment_profile_min;
    if (!EXPERIMENT_PROFILE_MIN.includes(profile)) {
      errors.push(
        `an experiment run must name its frozen profile: experiment_profile_min ${EXPERIMENT_PROFILE_MIN.join(" or ")}`,
      );
    } else if (profile !== durationMin) {
      errors.push(
        `experiment_profile_min ${profile} does not match the ${durationMin} minute interview`,
      );
    }
  } else if ("experiment_profile_min" in manifest) {
    errors.push("experiment_profile_min applies to experiment runs only");
  }
  const selected = manifest.selected_duration_s;
  if (!isInt(selected) || selected < target) {
    errors.push(
      "selected_duration_s must be an integer that covers observation_target_s",
    );
  } else if (kind !== "smoke" && selected !== target) {
    errors.push(
      `a ${kind} run must set selected_duration_s to its ${target}s target`,
    );
  }
  return errors;
}

/// The handover grace the stimulus reserve is revalidated against: the
/// harness's existing 90 seconds unless BROWSER_CHECK_HANDOVER_GRACE_S names
/// what the resolved settings use. A value that is not a whole number is
/// refused by validateReserve rather than defaulted.
function resolvedGraceS(env) {
  const raw = env.BROWSER_CHECK_HANDOVER_GRACE_S;
  if (raw === undefined || raw === "") return 90;
  return COUNT_PATTERN.test(raw) ? Number(raw) : Number.NaN;
}

/// The plan for BROWSER_CHECK_FLOW=long-interview, or every reason there is
/// none. `env` is the process environment; `io` lets a test stand in for the
/// file system.
function planFromEnv(env, io = {}) {
  const readFile = io.readFile ?? ((file) => fs.readFileSync(file, "utf8"));
  const readBytes = io.readBytes ?? ((file) => fs.readFileSync(file));
  const isDirectory =
    io.isDirectory ??
    ((dir) => {
      try {
        return fs.statSync(dir).isDirectory();
      } catch {
        return false;
      }
    });
  const errors = [];

  if (env.BROWSER_CHECK_FLOW !== "long-interview") {
    return { errors: ["BROWSER_CHECK_FLOW must be long-interview"] };
  }
  if (!["rust", "dispatch"].includes(env.BROWSER_CHECK_AGENT)) {
    errors.push(
      `BROWSER_CHECK_FLOW=long-interview needs BROWSER_CHECK_AGENT=rust or dispatch, not ${env.BROWSER_CHECK_AGENT ?? "unset"}`,
    );
  }
  // browser-check.sh hands the soak variables on as "0" and "" when no soak
  // was asked for; anything else is a soak setting this flow would ignore.
  if (!["", "0", undefined].includes(env.BROWSER_CHECK_SOAK_SECONDS)) {
    errors.push(
      "BROWSER_CHECK_SOAK_SECONDS applies to BROWSER_CHECK_FLOW=soak, not long-interview",
    );
  }
  if (!["", undefined].includes(env.BROWSER_CHECK_SOAK_VERDICT_MODE)) {
    errors.push(
      "BROWSER_CHECK_SOAK_VERDICT_MODE applies to BROWSER_CHECK_FLOW=soak, not long-interview",
    );
  }

  const duration = parseDurationMin(env.BROWSER_CHECK_DURATION_MIN);
  if (duration.error) errors.push(duration.error);
  const recording = recordingCapMin(env, readFile);
  if (recording.error) errors.push(recording.error);
  else if (duration.value !== undefined) {
    const ceiling = tokenCeilingMin(recording.cap);
    if (duration.value > ceiling) {
      errors.push(
        `the recording cap (CODETRIAL_RECORDING_MAX_MINUTES=${recording.cap}) would clamp the token to ${ceiling} minutes, below the ${duration.value} requested`,
      );
    }
  }

  if (env.BROWSER_CHECK_CAPTURE) {
    const dir = path.dirname(path.resolve(env.BROWSER_CHECK_CAPTURE));
    if (!isDirectory(dir)) {
      errors.push(`BROWSER_CHECK_CAPTURE directory ${dir} does not exist`);
    }
  }

  const manifestPath = env.BROWSER_CHECK_RUN_MANIFEST;
  let manifest = null;
  if (!manifestPath) {
    errors.push(
      "a long interview needs a budgeted run: start it through scripts/interview-run-budget.py run",
    );
  } else {
    try {
      const raw = readBytes(manifestPath);
      manifest = JSON.parse(raw.toString("utf8"));
      const digest = crypto.createHash("sha256").update(raw).digest("hex");
      if (digest !== env.BROWSER_CHECK_RUN_MANIFEST_SHA256) {
        errors.push(
          "BROWSER_CHECK_RUN_MANIFEST_SHA256 does not match the manifest",
        );
      }
    } catch (error) {
      errors.push(`manifest unreadable: ${error.message}`);
    }
  }
  if (manifest) {
    const runId = env.BROWSER_CHECK_RUN_ID;
    if (typeof runId !== "string" || !ID_PATTERN.test(runId)) {
      errors.push(
        "BROWSER_CHECK_RUN_ID must match [A-Za-z0-9][A-Za-z0-9._-]{0,63}",
      );
    } else if (manifest.run_id !== runId) {
      errors.push("BROWSER_CHECK_RUN_ID does not match the manifest");
    }
    if (duration.value !== undefined) {
      errors.push(...validateManifest(manifest, duration.value));
      // The wall windows and the archive bound are the budget controller's to
      // enforce and this flow's to refuse to start without.
      errors.push(...validateBudgetWindows(manifest));
      errors.push(
        ...validateReserve({
          reserveS: STIMULUS_RESERVE_S,
          resolvedGraceS: resolvedGraceS(env),
        }),
      );
    }
  }
  if (errors.length) return { errors };
  return {
    errors,
    plan: {
      runKind: manifest.run_kind,
      endMode: manifest.end_mode,
      durationMin: duration.value,
      observationTargetS: manifest.observation_target_s,
      runId: manifest.run_id,
    },
  };
}

/// The harness-wide duration in minutes for the URL: the plan's for a long
/// interview, the page's own 20 for every other flow.
function pageDurationMin(env) {
  if (env.BROWSER_CHECK_FLOW !== "long-interview") return DEFAULT_DURATION_MIN;
  const duration = parseDurationMin(env.BROWSER_CHECK_DURATION_MIN);
  if (duration.error) throw new Error(duration.error);
  return duration.value;
}

const FLOWS = ["voice", "report", "barge", "avatar", "soak", "long-interview"];

/// The validation both entry points share, and all either does under
/// BROWSER_CHECK_VALIDATE_ENV_ONLY. Returns the reasons the environment is
/// refused, empty when it is not.
function validateEnvironment(env, io) {
  const flow = env.BROWSER_CHECK_FLOW ?? "voice";
  if (!FLOWS.includes(flow)) {
    return [`BROWSER_CHECK_FLOW must be ${FLOWS.join(", ")}`];
  }
  if (flow === "long-interview") {
    return planFromEnv({ ...env, BROWSER_CHECK_FLOW: flow }, io).errors;
  }
  // The legacy flows keep their own timing; a duration chosen for them would
  // be a long-interview claim by another name.
  if (env.BROWSER_CHECK_DURATION_MIN !== undefined) {
    return [
      `BROWSER_CHECK_DURATION_MIN applies to BROWSER_CHECK_FLOW=long-interview, not ${flow}`,
    ];
  }
  return [];
}

/// Entry point for a validation-only run: prints the refusal and exits 2, or
/// returns when the environment is acceptable.
function validateOnly(env = process.env, io) {
  const errors = validateEnvironment(env, io);
  if (errors.length) {
    for (const error of errors) console.error(error);
    process.exit(2);
  }
}

/// Minutes a runtime line of `room` reported, from the complete lines of `log`.
/// `starting interview:` is the bootstrap the agent was handed; `reached its
/// server-side deadline:` is the agent ending the interview itself.
function reportedMinutes(log, room, prefix) {
  const found = [];
  const pattern = new RegExp(
    `^${prefix} room=(\\S+)(?: problem=\\S+)? duration=(\\d+)min$`,
  );
  for (const line of String(log).split("\n")) {
    const match = pattern.exec(line.trimEnd());
    if (match && match[1] === room) found.push(Number(match[2]));
  }
  return found;
}

const bootstrapMinutes = (log, room) =>
  reportedMinutes(log, room, "starting interview:");
const deadlineMinutes = (log, room) =>
  reportedMinutes(log, room, "interview reached its server-side deadline:");

/// The countdown the page shows, `MM:SS`, in seconds; null for anything else.
function parseCountdown(text) {
  const match = /^(\d{2,3}):([0-5]\d)$/.exec(String(text).trim());
  return match ? Number(match[1]) * 60 + Number(match[2]) : null;
}

/// Judges what a long interview observed against its plan. Every input is a
/// fact the harness read, and a missing one is a failure: a run that found no
/// token, no bootstrap line or no clock sample has not shown that it ran the
/// interview it asked for.
///
/// `obs`: tokenDurationsMin, bootstrapDurationsMin, deadlineDurationsMin
/// (arrays of the minutes each source reported), countdownStartS, clock
/// (soak-judge `parseClock` rows), harnessStartMs, nowMs, observedS,
/// graceStimuli.
function judgeObservation(plan, obs) {
  const reasons = [];
  const want = plan.durationMin;
  const sources = [
    ["token", obs.tokenDurationsMin],
    ["bootstrap", obs.bootstrapDurationsMin],
  ];
  for (const [name, values] of sources) {
    if (!values || values.length === 0) {
      reasons.push(`no ${name} duration was observed`);
    } else if (values.some((value) => value !== want)) {
      reasons.push(
        `${name} duration ${JSON.stringify(values)} is not the requested ${want} minutes`,
      );
    }
  }
  const full = want * 60;
  const start = obs.countdownStartS;
  // The page is read some time after it opened; that time is the slack.
  const slack = obs.countdownSlackS ?? SERVER_DEADLINE_GRACE_S;
  if (!Number.isFinite(start)) {
    reasons.push("no countdown was observed");
  } else if (start > full || start < full - slack) {
    reasons.push(
      `countdown started at ${start}s, not within ${slack}s of ${full}s`,
    );
  }
  if (obs.observedS < plan.observationTargetS) {
    reasons.push(
      `observed ${obs.observedS}s of the ${plan.observationTargetS}s target`,
    );
  }

  const clock = obs.clock ?? [];
  if (plan.observationTargetS >= CLOCK_EVIDENCE_FROM_S) {
    if (clock.length === 0) {
      reasons.push("no interview clock sample was observed");
    } else {
      const anchors = new Set(clock.map((row) => row.startUnixMs));
      const [origin] = [...anchors];
      if (anchors.size !== 1 || !Number.isFinite(origin)) {
        reasons.push(
          "interview clock origin is missing or changed between samples",
        );
      } else if (
        origin < obs.harnessStartMs - 5000 ||
        origin > obs.nowMs + 5000
      ) {
        reasons.push(
          "interview clock origin is outside the window this run was alive",
        );
      }
      const elapsed = clock.map((row) => row.elapsedMs);
      if (!elapsed.every(Number.isFinite)) {
        reasons.push("interview clock has a sample with no elapsed time");
      }
      if (elapsed.some((ms, i) => i > 0 && ms < elapsed[i - 1])) {
        reasons.push("interview clock went backwards");
      }
      const max = Math.max(...elapsed);
      if (max < (plan.observationTargetS - CLOCK_SAMPLE_S) * 1000) {
        reasons.push(
          `interview clock reached ${max}ms, short of the ${plan.observationTargetS}s target`,
        );
      }
      if (max > (full + SERVER_DEADLINE_GRACE_S) * 1000) {
        reasons.push(
          `interview clock reached ${max}ms, past the duration plus its grace`,
        );
      }
    }
  }

  if (plan.endMode === "server_deadline") {
    const deadlines = obs.deadlineDurationsMin ?? [];
    if (deadlines.length === 0) {
      reasons.push("the server never reached its deadline");
    } else if (deadlines.some((value) => value !== want)) {
      reasons.push(
        `deadline duration ${JSON.stringify(deadlines)} is not the requested ${want} minutes`,
      );
    }
    if (obs.graceStimuli !== 0) {
      reasons.push("the harness sent a stimulus during the deadline grace");
    }
  }
  return { verdict: reasons.length ? "fail" : "pass", reasons };
}

if (require.main === module) validateOnly(process.env);

module.exports = {
  CLOCK_SAMPLE_S,
  DEFAULT_DURATION_MIN,
  HARNESS_DURATIONS_MIN,
  PRODUCT_MAX_DURATION_MIN,
  PRODUCT_MIN_DURATION_MIN,
  SERVER_DEADLINE_GRACE_S,
  bootstrapMinutes,
  deadlineMinutes,
  judgeObservation,
  pageDurationMin,
  parseCountdown,
  parseDurationMin,
  planFromEnv,
  validateEnvironment,
  validateManifest,
  validateOnly,
};
