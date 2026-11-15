"use strict";
// Whether a long interview qualified, decided once the run is over from what
// its report and its judged checkpoints say. Nothing here reads a log or a
// reply: the rotation facts are the soak judge's (scripts/soak-judge.cjs), the
// rotation classes and their minimum are the timing module's
// (scripts/long-interview-timing.cjs), the checkpoint verdicts are the
// evidence consumer's (scripts/long-interview-evidence.cjs), and this module
// only holds the rule that joins them.
//
// The rule has three outcomes. `fail` is something the run established
// against itself. `inconclusive` is evidence that is not there, which is never
// a pass and never a failure. `pass` needs every positive fact below to be
// present and of the right type, so a report that lost a field cannot qualify
// by comparing `undefined` with a number.

const {
  EXIT,
  FAILURE_CLASSES,
  MIN_COMPLETED_ROTATIONS,
} = require("./long-interview-timing.cjs");
const { whole } = require("./replay-clock.cjs");

const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;

/// The replacements that were rotations completed under load: the soak judge
/// called each a rotation, its episode installed a socket before the stimuli
/// stopped, and it can be placed on a new socket and on the interview clock.
///
/// A socket is opened once. Two rows that claim the same one, or the same
/// replacement, contradict each other unless they are the same row listed
/// twice, and a contradiction is no rotation at all: otherwise one answer on
/// that socket would verify both.
function rotationsOf(rotation) {
  const rows = Array.isArray(rotation?.replacements)
    ? rotation.replacements
    : [];
  // Every row that names a socket, a replacement or an episode makes its
  // claim, a rotation or not: a watchdog rebuild that says it opened the same
  // socket contradicts the rotation as much as a second rotation would.
  const claims = new Map();
  const claim = (key, row) => {
    if (!claims.has(key)) claims.set(key, new Set());
    claims.get(key).add(row);
  };
  const candidates = new Map();
  for (const r of rows) {
    if (!r || !whole(r.session)) continue;
    const row = JSON.stringify(r);
    if (whole(r.new_socket)) claim(`socket:${r.session}:${r.new_socket}`, row);
    // A socket is replaced once, so two rows that both replaced it contradict.
    if (whole(r.old_socket)) {
      claim(`replaced:${r.session}:${r.old_socket}`, row);
    }
    if (whole(r.replacement)) {
      claim(`replacement:${r.session}:${r.replacement}`, row);
    }
    if (whole(r.episode)) claim(`episode:${r.session}:${r.episode}`, row);
    if (r.rotation !== true || r.episode_class !== "completed") continue;
    const placed = [
      r.episode,
      r.replacement,
      r.old_socket,
      r.new_socket,
      r.started_interview_ms,
      r.interview_ms,
    ];
    if (!placed.every(whole) || r.new_socket <= r.old_socket) continue;
    if (r.started_interview_ms > r.interview_ms) continue;
    candidates.set(row, r);
  }
  for (const claimants of claims.values()) {
    if (claimants.size > 1) for (const row of claimants) candidates.delete(row);
  }
  return [...candidates.values()];
}

/// The consumer's per-checkpoint verdicts when they are ones this rule can
/// read: a list of rows, each a checkpoint named once with a verdict it knows.
/// Anything else is no verdicts at all, so a list that was cut or padded
/// cannot qualify on the rows that happen to be whole.
function readable(checkpoints) {
  if (!Array.isArray(checkpoints)) return null;
  const ids = new Set();
  for (const c of checkpoints) {
    if (!c || typeof c.checkpoint_id !== "string") return null;
    if (!["pass", "fail", "not_assessed"].includes(c.verdict)) return null;
    if (ids.has(c.checkpoint_id)) return null;
    ids.add(c.checkpoint_id);
  }
  return checkpoints;
}

/// Whether a passed checkpoint shows the conversation survived `rotation`: its
/// reply came on the socket that handover opened, and it asked for something
/// the candidate had finished saying before the handover began, so on an
/// earlier socket. What was said while it was under way may have reached the
/// new socket directly and shows nothing.
function verifies(checkpoint, rotation, scheduleOriginUnixMs) {
  const reply = checkpoint?.reply;
  if (checkpoint?.verdict !== "pass" || !reply) return false;
  if (reply.session !== rotation.session) return false;
  if (reply.socket !== rotation.new_socket) return false;
  if (!whole(reply.clock_start_unix_ms) || !whole(scheduleOriginUnixMs)) {
    return false;
  }
  if (!Number.isFinite(checkpoint.earliest_fact_heard_s)) return false;
  // The start of the handover on the schedule's clock: the interview clock
  // began at `clock_start_unix_ms` and the episode began
  // `started_interview_ms` into it.
  const beganS =
    (reply.clock_start_unix_ms +
      rotation.started_interview_ms -
      scheduleOriginUnixMs) /
    1000;
  return checkpoint.earliest_fact_heard_s <= beganS;
}

/// The verdict for one run.
///
/// `runKind` is the validated manifest's, not the report's: a report that went
/// missing must not turn a qualification into a run nobody has to judge.
/// `report` is the long-run report and `checkpoints` the consumer's
/// per-checkpoint verdicts, null when it could not judge. `teardownFailed` is
/// the launcher's word that stopping the fixture or a capture after the worker
/// failed, which the report cannot say because it was written before.
function qualify({ runKind, report, checkpoints, teardownFailed = false }) {
  const rotation = report?.rotation;
  const rotations = rotationsOf(rotation);
  const judged = readable(checkpoints);
  const verified = rotations.filter((r) =>
    (judged ?? []).some((c) => verifies(c, r, report?.schedule_origin_unix_ms)),
  );
  const counts = {
    rotations_completed: rotations.length,
    rotations_verified: verified.length,
  };
  if (runKind !== "qualification") {
    return {
      qualification: "not_evaluated",
      qualification_reasons: [],
      ...counts,
    };
  }

  const failed = [];
  const unknown = [];
  /// One fact: established against the run, present and good, or not there.
  const grade = (bad, good, failure, missing) => {
    if (bad) failed.push(failure);
    else if (!good) unknown.push(missing);
  };
  const texts = (value) =>
    Array.isArray(value) ? value.filter((r) => typeof r === "string") : [];

  if (teardownFailed) failed.push("the run's teardown failed");
  // A GoAway that never came and logs that were cut short or evicted leave the
  // run with no evidence to judge, which is not a failure of it; the other
  // classes are. The timing module's table says which classes those are.
  const unjudgeable = (code) =>
    code === EXIT.noGoAway || code === EXIT.incompleteArtifact;
  const NO_EVIDENCE = Object.keys(FAILURE_CLASSES).filter((name) =>
    unjudgeable(FAILURE_CLASSES[name].exit),
  );
  if (!report || typeof report !== "object") {
    unknown.push("the run left no report");
  } else {
    // A run that saw no GoAway learned nothing about rotation. Its exit, its
    // status and its class say so, and none of the three is a failure.
    const code = report.exit_code;
    grade(
      Number.isSafeInteger(code) && code !== 0 && !unjudgeable(code),
      code === 0,
      `the run exited ${code}`,
      code === EXIT.noGoAway
        ? "the run saw no GoAway"
        : code === EXIT.incompleteArtifact
          ? "the run's logs were incomplete"
          : "the report has no exit code",
    );
    const status = report.status;
    grade(
      typeof status === "string" &&
        !["complete", "inconclusive", "incomplete_artifact"].includes(status),
      status === "complete",
      `the run's status is ${status}`,
      "the report does not say the run completed",
    );
    // The worker says `full_interview` only of a run that passed and whose
    // clock was anchored to the server's, so its `false` is a finding about how
    // long the interview ran only when neither an unjudgeable exit (no GoAway,
    // incomplete artifact) nor an unanchored clock explains it. An observation
    // that was recorded as ending before the duration is short whatever the
    // exit, since a cut archive does not lengthen it.
    const anchored = report.anchor?.source === "server_clock";
    const endedS = report.timestamps?.observationEndS;
    const measuredShort =
      Number.isFinite(endedS) &&
      Number.isFinite(report.duration_min) &&
      endedS < report.duration_min * 60;
    grade(
      report.full_interview === false &&
        anchored &&
        (!unjudgeable(code) || measuredShort),
      report.full_interview === true,
      "the run was not a full interview",
      anchored
        ? "the report does not say it was a full interview"
        : "the interview clock was not anchored to the server, so how long it ran is not known",
    );
    grade(
      report.validation_fixture === true,
      report.validation_fixture === false,
      "the run played the tone fixture, not spoken questions",
      "the report does not say which clips were played",
    );
    const classes = report.failure_classes;
    const real = texts(classes).filter((name) => !NO_EVIDENCE.includes(name));
    grade(
      real.length > 0,
      Array.isArray(classes) && classes.length === 0,
      `failure classes: ${real.join(", ")}`,
      "the report does not show a run free of failure classes",
    );
    grade(
      report.decoded_signal === "failed",
      ["verified", "not_required"].includes(report.decoded_signal),
      "the decoded signal failed after a swap",
      "the decoded signal after the swaps was not verified",
    );
  }

  if (!rotation || typeof rotation !== "object") {
    unknown.push("the report has no rotation facts");
  } else {
    const said = texts(rotation.reasons).join("; ");
    // The rotation judge reads the server's log; with it cut short or evicted
    // its fail and its late count say nothing about the run.
    const logsGone = texts(report?.failure_classes).some(
      (name) => FAILURE_CLASSES[name]?.exit === EXIT.incompleteArtifact,
    );
    grade(
      rotation.verdict === "fail" && !logsGone,
      rotation.verdict === "pass",
      `the rotation judge failed: ${said}`,
      `the rotation judge did not pass: ${said}`,
    );
    grade(
      whole(rotation.late_failed_total) &&
        rotation.late_failed_total > 0 &&
        !logsGone,
      rotation.late_failed_total === 0,
      `${plural(rotation.late_failed_total, "rotation")} failed after the stimuli stopped`,
      "the late replacement episodes were not counted",
    );
    grade(
      whole(rotation.failed_handovers_total) &&
        rotation.failed_handovers_total > 0 &&
        !logsGone,
      rotation.failed_handovers_total === 0,
      `${plural(rotation.failed_handovers_total, "GoAway handover")} failed`,
      "the failed handovers were not counted",
    );
    if (rotation.episodes_unresolved !== 0) {
      unknown.push(
        whole(rotation.episodes_unresolved)
          ? `${plural(rotation.episodes_unresolved, "replacement episode")} did not resolve on record`
          : "the replacement episodes were not counted",
      );
    }
    if (!Array.isArray(rotation.replacements)) {
      unknown.push("the report does not list the run's replacements");
    }
  }

  if (!judged) {
    unknown.push("the checkpoint verdicts are missing or cannot be read");
  } else {
    const failures = judged.filter((c) => c.verdict === "fail");
    if (failures.length) {
      failed.push(
        `${plural(failures.length, "checkpoint")} failed: ${failures
          .map((c) => c.checkpoint_id)
          .join(", ")}`,
      );
    }
  }
  // Verified rotations are completed ones, so the one count covers both.
  if (verified.length < MIN_COMPLETED_ROTATIONS) {
    unknown.push(
      `${plural(rotations.length, "completed rotation")}, ${verified.length} verified by an answer that reached back across it; ${MIN_COMPLETED_ROTATIONS} are needed`,
    );
  }

  return {
    qualification: failed.length
      ? "fail"
      : unknown.length
        ? "inconclusive"
        : "pass",
    qualification_reasons: [...failed, ...unknown],
    ...counts,
  };
}

module.exports = { qualify };
