// The driver of BROWSER_CHECK_FLOW=long-interview: the schedule, the clips, the
// run budget, the three timestamps and the failure classes, joined into one
// run that is decided by evidence rather than by having reached its last line.
//
// The pieces are tested on their own in long-interview-schedule.cjs,
// long-interview-timing.cjs and run-budget-guard.cjs. This file is the only
// place that calls them in order, and it touches the browser through a
// `controller` object so a fake one can stand in for the page in tests.
//
//   controller: playClip(entry, bytes) -> Promise<void>
//               applyRevision(revision) -> Promise<void>
//               setThinking(on) -> Promise<void>
//               agentSpeaking() -> Promise<boolean>
//               agentReplyCount() -> Promise<number>
//               end() -> Promise<{performanceReceived, reportReceived, clicked}>
//               awaitServerDeadline() -> Promise<{performanceReceived, reportReceived}>
//   clock:      now() -> wall ms, sleep(ms) -> Promise
//
// Two kinds of clip may drive it. Recorded speech, authored outside the
// checkout, is the only audio that asks the model to answer. The synthetic
// tones of long-interview-schedule.cjs exist to prove this wiring and are
// accepted only as a named validation fixture of a smoke run; nothing they
// produce can be reported as a speech run.
//
// Nothing here has driven a real browser. The live behaviour needs a real browser run of the long interview.
"use strict";

const fs = require("fs");
const path = require("path");
const sched = require("./long-interview-schedule.cjs");
const timing_ = require("./long-interview-timing.cjs");
const { BudgetDenied } = require("./run-budget-guard.cjs");

const CLIP_DIR_ENV = "BROWSER_CHECK_LONG_CLIP_DIR";
const TONE_FIXTURE_ENV = "BROWSER_CHECK_LONG_TONE_FIXTURE";
const ARTIFACT_DIR_ENV = "BROWSER_CHECK_LONG_ARTIFACT_DIR";
const MODE_ENV = "BROWSER_CHECK_LONG_MODE";
const END_ACTION_ID = "end";
const DEFAULT_MODE = "coding";
const TICK_MS = 1000;
const SPEAKING_POLL_MS = 250;

const CLIP_SOURCE = Object.freeze({
  speech: "recorded_speech",
  toneFixture: "tone_validation_fixture",
});

/// Raised before anything is dispatched: the run is not the one that was
/// asked for. The process exits 2, like every other refusal.
class DriverRefused extends Error {}

const { sha256 } = sched;
const CLEAN = Object.freeze({
  errors: [],
  leftover: { agents: 0, processes: 0, rooms: 0 },
});
const sleepReal = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const realClock = { now: () => Date.now(), sleep: sleepReal };

const isSafeClipFile = (name) =>
  typeof name === "string" &&
  name === path.basename(name) &&
  /\.wav$/.test(name);

/// The interview surface the run drives: the coding editor unless the
/// whiteboard is named. The clips and the budget pin are per surface.
function modeFromEnv(env) {
  const mode = env[MODE_ENV] || DEFAULT_MODE;
  if (!["coding", "whiteboard"].includes(mode)) {
    throw new DriverRefused(`${MODE_ENV} must be coding or whiteboard`);
  }
  return mode;
}

/// The clips this run will play and what they are. A non-smoke run needs a
/// manifest of recorded speech; the tone set is available only when the
/// fixture is named and the run is a smoke, and is never both.
function resolveClipSource(env, plan, schedule, io = {}) {
  const readFile = io.readFile ?? ((file) => fs.readFileSync(file));
  const fixture = env[TONE_FIXTURE_ENV];
  const dir = env[CLIP_DIR_ENV];
  if (fixture !== undefined && fixture !== "" && fixture !== "1") {
    throw new DriverRefused(`${TONE_FIXTURE_ENV} must be 1 or unset`);
  }
  if (fixture === "1") {
    if (dir) {
      throw new DriverRefused(
        `${TONE_FIXTURE_ENV} and ${CLIP_DIR_ENV} name two clip sources`,
      );
    }
    if (plan.runKind !== "smoke") {
      throw new DriverRefused(
        `the tone validation fixture contains no speech and is for a smoke run only, not a ${plan.runKind} run`,
      );
    }
    const set = sched.syntheticClipSet(schedule);
    return {
      kind: CLIP_SOURCE.toneFixture,
      manifest: set.manifest,
      audio: set.audio,
    };
  }
  if (!dir) {
    throw new DriverRefused(
      `a long interview needs ${CLIP_DIR_ENV}: a directory holding manifest.json and the recorded speech clips it lists (this checkout cannot make them)`,
    );
  }
  let manifest;
  try {
    manifest = JSON.parse(readFile(path.join(dir, "manifest.json")).toString());
  } catch (error) {
    throw new DriverRefused(`clip manifest unreadable: ${error.message}`);
  }
  const audio = new Map();
  for (const entry of manifest.clips ?? []) {
    if (!isSafeClipFile(entry.file) || entry.file !== `${entry.clip_id}.wav`) {
      throw new DriverRefused(
        `clip ${entry.clip_id}: ${entry.file} is not its own plain file`,
      );
    }
    try {
      audio.set(entry.clip_id, readFile(path.join(dir, entry.file)));
    } catch (error) {
      throw new DriverRefused(
        `clip ${entry.clip_id} unreadable: ${error.message}`,
      );
    }
  }
  return { kind: CLIP_SOURCE.speech, manifest, audio };
}

/// Everything a run must satisfy before a candidate joins. Throws
/// DriverRefused with every reason; returns the pieces the run needs.
function prepareLongRun({ env, plan, budget, io }) {
  const reasons = [];
  const mode = modeFromEnv(env);
  let schedule;
  try {
    schedule = sched.generateSchedule(mode, {
      durationMin: plan.durationMin,
      observationTargetS: plan.observationTargetS,
    }).schedule;
  } catch (error) {
    throw new DriverRefused(`no schedule: ${error.message}`);
  }
  reasons.push(...sched.validateSchedule(schedule));
  const clipSource = resolveClipSource(env, plan, schedule, io);
  const speech = clipSource.kind === CLIP_SOURCE.speech;
  reasons.push(
    ...sched.validateClipSet(schedule, clipSource.manifest, clipSource.audio, {
      requireSpeech: speech,
    }),
  );
  if (!speech && clipSource.manifest.contains_speech !== false) {
    reasons.push("the validation fixture claims speech");
  }
  if (speech) {
    // Speech is authored elsewhere and attributed. The generator's own tones
    // under a recorded-speech label are the one forgery this file can see.
    if (
      typeof clipSource.manifest.tool !== "string" ||
      clipSource.manifest.tool === ""
    ) {
      reasons.push("the recorded-speech manifest names no tool that made it");
    }
    for (const clip of schedule.clips) {
      const bytes = clipSource.audio.get(clip.clip_id);
      if (bytes && sha256(bytes) === sha256(sched.syntheticClipBytes(clip))) {
        reasons.push(
          `${clip.clip_id}: the clip is the generator's synthetic tone under a recorded-speech label`,
        );
      }
    }
  }
  let playback = [];
  try {
    playback = sched.playbackPlan(schedule, clipSource.manifest);
    reasons.push(...sched.validatePlaybackPlan(playback, schedule));
  } catch (error) {
    reasons.push(error.message);
  }
  if (!budget || typeof budget.guarded !== "function") {
    reasons.push("a long interview needs a run budget to dispatch through");
  } else {
    const same =
      Array.isArray(budget.actions) &&
      budget.actions.length === schedule.actions.length &&
      budget.actions.every((id, i) => id === schedule.actions[i]);
    if (!same) {
      reasons.push(
        "the run budget manifest actions are not the schedule's action IDs",
      );
    }
    if (!clipSource.manifest.manifest_sha256) {
      reasons.push("the clip manifest has no hash to pin");
    } else if (
      budget.fixtureSha256 !==
      sched.budgetFields(schedule, clipSource.manifest).fixture_sha256
    ) {
      reasons.push(
        "the run budget fixture_sha256 does not pin this schedule and clip manifest",
      );
    }
  }
  if (reasons.length) {
    throw new DriverRefused(
      `the long interview cannot start: ${reasons.join("; ")}`,
    );
  }
  return { mode, schedule, clipSource, playback };
}

/// The timing windows for a plan. A smoke that observes less than the whole
/// interview is judged against what it set out to observe, and is never
/// reported as a full interview for it.
function timingForPlan(plan) {
  const base = timing_.timingFor(plan.durationMin);
  const target = plan.observationTargetS;
  if (target >= base.durationS) return base;
  return {
    ...base,
    durationS: target,
    observationEndS: target,
    lastStimulusBoundaryS: target - base.stimulusReserveS,
    finalQuestionAtS:
      target - base.stimulusReserveS - timing_.FINAL_QUESTION_BEFORE_BOUNDARY_S,
    partial: true,
  };
}

/// Plays the schedule and holds the session to the full duration, then ends it
/// deliberately. Resolves with what happened, a runtime failure included: the
/// timestamps of a run that stopped early are the evidence that it did.
async function runLongInterview(o) {
  const {
    plan,
    schedule,
    playback,
    clipSource,
    budget,
    controller,
    joinedAtMs,
    interviewStartMs,
  } = o;
  const clock = o.clock ?? realClock;
  const observe = o.observe ?? (async () => {});
  const failure = o.failure ?? new Promise(() => {});
  const sinceJoin = (ms) => (ms - joinedAtMs) / 1000;

  const run = {
    joinedAtMs,
    interviewStartMs,
    dispatched: [],
    transmitted: [],
    uncovered: [],
    lastStimulusAtMs: null,
    observationEndMs: null,
    reportCompleteMs: null,
    finalAskedAtMs: null,
    finalRepliedAtMs: null,
    finalClipEndMs: null,
    performanceReceived: false,
    reportReceived: false,
    endClicked: null,
    aborted: null,
    budgetDenied: null,
  };
  const playByAction = new Map(playback.map((e) => [e.action_id, e]));
  let replyBaseline = null;

  const tick = async () => {
    await observe();
    if (
      run.finalAskedAtMs !== null &&
      run.finalRepliedAtMs === null &&
      clock.now() >= run.finalClipEndMs
    ) {
      const count = await controller.agentReplyCount();
      // The baseline is the count when the question ended, so only a reply
      // that begins after it can answer it.
      if (replyBaseline === null) replyBaseline = count;
      else if (count > replyBaseline) {
        run.finalRepliedAtMs = clock.now();
      }
    }
  };
  const waitUntil = async (targetMs) => {
    while (clock.now() < targetMs) {
      await tick();
      // The failure is listed first so that, when both are already settled, the
      // agent's death wins over one more tick.
      // A tick lands on the moment the final question ends, so the reply
      // baseline is read then and not up to a tick later.
      const untilQuestionEnds =
        run.finalClipEndMs !== null && run.finalClipEndMs > clock.now()
          ? run.finalClipEndMs - clock.now()
          : TICK_MS;
      await Promise.race([
        failure,
        clock.sleep(
          Math.min(
            TICK_MS,
            untilQuestionEnds,
            Math.max(1, targetMs - clock.now()),
          ),
        ),
      ]);
    }
  };

  const dispatch = async (step) => {
    let sentMs = null;
    await budget.guarded(step.action_id, async () => {
      // Sent from here, after the permit, so a slow permit does not move the
      // time. The timestamp is kept even when the page then throws: a
      // stimulus that may have been partly delivered still spent the clock.
      sentMs = clock.now();
      run.lastStimulusAtMs = sentMs;
      if (step.clip_id) {
        const entry = playByAction.get(step.action_id);
        if (!entry) throw new Error(`no playback entry for ${step.action_id}`);
        if (step.checkpoint_id === "cp_final") {
          run.finalAskedAtMs = sentMs;
          // A reply that starts while the question is still being asked is
          // not an answer to it.
          run.finalClipEndMs = sentMs + entry.duration_s * 1000;
        }
        await controller.playClip(entry, clipSource.audio.get(entry.clip_id));
        run.transmitted.push({
          clip_id: entry.clip_id,
          action_id: step.action_id,
          started_at_s: sinceJoin(sentMs),
        });
      } else if (step.kind === "thinking_start") {
        await controller.setThinking(true);
      } else if (step.kind === "thinking_end") {
        await controller.setThinking(false);
      } else if (step.kind === "revision") {
        await controller.applyRevision(step.revision);
      } else {
        throw new Error(`step ${step.action_id} has no kind this driver sends`);
      }
    });
    run.dispatched.push({
      action_id: step.action_id,
      planned_s: step.at_s,
      sent_s: sinceJoin(sentMs),
    });
  };

  const bargeWhenSpeaking = async (step) => {
    const prime = playByAction.get(step.action_id.replace(/_barge$/, "_prime"));
    const from = Math.max(
      clock.now(),
      joinedAtMs + (prime ? prime.end_s : step.at_s) * 1000,
    );
    await waitUntil(from);
    const until = Math.min(
      joinedAtMs + step.latest_s * 1000,
      from + step.wait_for_agent_speaking_max_s * 1000,
    );
    while (clock.now() <= until) {
      // The logs are read here too: a wait for the interviewer to speak is
      // seconds in which its rolling output keeps moving.
      await observe();
      if (await controller.agentSpeaking()) {
        await dispatch(step);
        return;
      }
      await Promise.race([failure, clock.sleep(SPEAKING_POLL_MS)]);
    }
    // Not covered is not passed: the report lists it.
    run.uncovered.push(step.action_id);
  };

  try {
    for (const step of schedule.steps) {
      if (step.kind === "join") {
        // The join click was dispatched through the guard by the caller, at
        // joinedAtMs, because the page cannot be reached without it.
        if (!o.joinDispatched) {
          throw new Error("the join was not dispatched through the run budget");
        }
        run.dispatched.push({
          action_id: step.action_id,
          planned_s: 0,
          sent_s: 0,
        });
        continue;
      }
      await waitUntil(joinedAtMs + step.at_s * 1000);
      if (step.conditional) await bargeWhenSpeaking(step);
      else await dispatch(step);
    }
    await waitUntil(interviewStartMs + plan.observationTargetS * 1000);
    run.observationEndMs = clock.now();
    let ended;
    if (plan.endMode === "manual") {
      ended = await budget.guarded(END_ACTION_ID, () => controller.end());
    } else {
      ended = await controller.awaitServerDeadline();
    }
    run.performanceReceived = ended?.performanceReceived === true;
    run.reportReceived = ended?.reportReceived === true;
    run.endClicked = ended?.clicked === undefined ? null : ended.clicked;
    run.reportCompleteMs = clock.now();
  } catch (error) {
    run.aborted = error?.message ?? String(error);
    if (error instanceof BudgetDenied) run.budgetDenied = error.message;
  }
  return run;
}

/// Reasons the stimuli the page received are not the schedule's: an action
/// skipped, sent twice, or sent outside its clip's window.
function judgeDispatch(schedule, playback, run) {
  const errors = [];
  const sent = new Map();
  for (const d of run.dispatched) {
    sent.set(d.action_id, (sent.get(d.action_id) ?? 0) + 1);
  }
  for (const step of schedule.steps) {
    const n = sent.get(step.action_id) ?? 0;
    if (step.conditional && run.uncovered.includes(step.action_id)) {
      if (n !== 0) errors.push(`${step.action_id}: sent and also uncovered`);
    } else if (n !== 1) {
      errors.push(`${step.action_id}: dispatched ${n} times, not once`);
    }
  }
  errors.push(...sched.verifyTransmitted(playback, run.transmitted).errors);
  return errors;
}

/// The run's verdict, its failure classes and the exit code the process
/// should leave with. `observation` is judgeObservation's result and
/// `episodes` the parsed replacement episodes of the room.
function finalizeLongRun(o) {
  const { plan, schedule, playback, run, clipSource, observation } = o;
  const timing = timingForPlan(plan);
  const anchor = timing_.anchorInterview({
    clockStarts: (o.clock ?? []).map((row) => row.startUnixMs),
    tokenRequestsMs: o.tokenRequestsMs ?? [],
  });
  const ts = {
    anchorMs: anchor.anchorMs,
    lastStimulusAtMs: run.lastStimulusAtMs,
    observationEndMs: run.observationEndMs,
    reportCompleteMs: run.reportCompleteMs,
    launchMs: o.launchMs,
    teardownEndMs: o.teardownEndMs,
  };
  const stamps = timing_.judgeTimestamps(timing, ts);
  const dispatchErrors = run.aborted
    ? []
    : judgeDispatch(schedule, playback, run);
  const sOf = (ms) =>
    Number.isFinite(ms) && Number.isFinite(anchor.anchorMs)
      ? (ms - anchor.anchorMs) / 1000
      : null;
  const goAways = (o.episodes ?? [])
    .filter((e) => e.trigger === "goaway")
    .map((e) => ({
      atS: Number.isFinite(e.startInterviewMs)
        ? e.startInterviewMs / 1000
        : null,
      resolvedAtS:
        e.result === "installed" && Number.isFinite(e.endInterviewMs)
          ? e.endInterviewMs / 1000
          : null,
    }));
  const cleanupAssessed = o.cleanup !== undefined;
  const evidence = {
    goAways: o.episodes ? goAways : undefined,
    finalQuestion: {
      askedAtS: sOf(run.finalAskedAtMs),
      repliedAtS: sOf(run.finalRepliedAtMs),
    },
    logs: o.logs,
    performanceReceived: run.performanceReceived,
    reportReceived: run.reportReceived,
    tokenExpired: o.tokenExpired === true,
    browserDisconnected: o.browserDisconnected === true,
    // What was left active after the worker is gone is the launcher's to
    // report. Without its report the worker classifies the rest of the run and
    // says that it did not assess cleanup, rather than failing every run with
    // a class that hides the real one.
    cleanup: cleanupAssessed ? o.cleanup : CLEAN,
    budgetExhausted: run.budgetDenied !== null,
  };
  const resolved = timing_.resolveRun(evidence, timing);

  const failures = [];
  if (run.aborted) failures.push(`the run stopped: ${run.aborted}`);
  failures.push(...dispatchErrors);
  if (observation && observation.verdict !== "pass") {
    failures.push(...observation.reasons);
  }
  failures.push(...stamps.reasons);
  // The most serious thing that happened names the exit code. A failure the
  // classes do not name is a plain failure, which outranks an inconclusive
  // no-GoAway but not a cleanup, artifact or budget class.
  let exitCode = resolved.exit_code;
  if (
    failures.length &&
    (exitCode === timing_.EXIT.pass || exitCode === timing_.EXIT.noGoAway)
  ) {
    exitCode = timing_.EXIT.failed;
  }
  const status =
    exitCode === resolved.exit_code
      ? resolved.status
      : exitCode === timing_.EXIT.failed
        ? "failed"
        : resolved.status;

  const speechClips = clipSource.kind === CLIP_SOURCE.speech;
  const covered =
    !run.aborted && dispatchErrors.length === 0 && run.uncovered.length === 0;
  const passed = exitCode === timing_.EXIT.pass && failures.length === 0;
  // The duration is measured from the server's own clock; the first token
  // request is earlier than the interview and would flatter the exposure.
  const fullInterview =
    stamps.fullInterview &&
    !run.aborted &&
    !timing.partial &&
    anchor.source === "server_clock";
  const report = {
    flow: "long-interview",
    status,
    exit_code: exitCode,
    run_kind: plan.runKind,
    end_mode: plan.endMode,
    duration_min: plan.durationMin,
    observation_target_s: plan.observationTargetS,
    clip_source: clipSource.kind,
    clip_manifest_sha256: clipSource.manifest.manifest_sha256,
    schedule_sha256: schedule.schedule_sha256,
    // A tone fixture proves the wiring. It asked the model nothing, whatever
    // else the run did.
    speech_run:
      speechClips &&
      clipSource.manifest.contains_speech === true &&
      passed &&
      covered,
    validation_fixture: !speechClips,
    full_interview: passed && fullInterview,
    coverage_complete: covered,
    cleanup_assessed: cleanupAssessed,
    end_clicked: run.endClicked,
    schedule_start_delay_s: (run.joinedAtMs - run.interviewStartMs) / 1000,
    exposure_s: stamps.exposureS,
    timestamps: stamps.timestamps,
    anchor: {
      source: anchor.source,
      reloads: anchor.reloads,
      errors: anchor.errors,
    },
    stimuli: {
      scheduled: schedule.steps.length,
      dispatched: run.dispatched.length,
      uncovered: run.uncovered,
    },
    failures,
    failure_classes: resolved.artifacts.map((a) => a.class),
    not_exercised: [
      "reply content: checkpoint answers are not judged by this driver",
      ...(cleanupAssessed
        ? []
        : ["cleanup: nothing active after teardown was checked by the worker"]),
      ...(run.endClicked === false
        ? [
            "end: the page's own countdown ended the interview before End was clicked",
          ]
        : []),
      ...(speechClips ? [] : ["speech: the tone fixture contains none"]),
      "logs: eviction of the agent's rolling output is not detected; SERVER_LOG is the complete record and its absence marks the logs incomplete",
    ],
  };
  return { report, resolved, exitCode, failures };
}

/// Writes one artifact per failure class. A run with a class and nowhere to
/// put it still returns its exit code, and says so on stderr.
function writeArtifacts(env, resolved) {
  if (resolved.artifacts.length === 0) return [];
  const capture = env.BROWSER_CHECK_CAPTURE;
  const dir =
    env[ARTIFACT_DIR_ENV] ||
    (capture
      ? path.join(
          path.dirname(path.resolve(capture)),
          "long-interview-failures",
        )
      : null);
  if (!dir) {
    console.error(
      `long-interview: no ${ARTIFACT_DIR_ENV} or BROWSER_CHECK_CAPTURE to hold the failure artifacts: ${JSON.stringify(resolved.artifacts)}`,
    );
    return [];
  }
  return timing_.writeFailureArtifacts(dir, resolved);
}

module.exports = {
  ARTIFACT_DIR_ENV,
  CLIP_DIR_ENV,
  CLIP_SOURCE,
  DriverRefused,
  END_ACTION_ID,
  MODE_ENV,
  TONE_FIXTURE_ENV,
  finalizeLongRun,
  judgeDispatch,
  modeFromEnv,
  prepareLongRun,
  realClock,
  resolveClipSource,
  runLongInterview,
  timingForPlan,
  writeArtifacts,
};
