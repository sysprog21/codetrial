"use strict";

// Turns the fixture's acquisition report into the evidence `judgeCheckpoint`
// reads, and says so plainly when it cannot. The report holds ordered facts
// only (receipts, batches, reply segments and turn endings, each with the
// socket it came from and how many of the others preceded it); this module
// is where they become a claim, and a claim is made only when the facts leave
// one reading. Anything else is `not_assessed` with the reason, never a pass
// and never a guess at a failure.

const fs = require("node:fs");

const sched = require("./long-interview-schedule.cjs");
const { qualify } = require("./long-interview-qualify.cjs");
const { EXIT } = require("./long-interview-timing.cjs");
const { whole: isCount } = require("./replay-clock.cjs");

const { generateSchedule, judgeCheckpoint } = sched;

// A reading whose sampling uncertainty exceeds this cannot place an event
// against a window edge.
const MAX_CLOCK_UNCERTAINTY_MS = 1000;

// The lists a capture holds, which the judge and the acceptance check both read.
const CAPTURE_LISTS = ["receipts", "batches", "replies", "boundaries"];

/// Why the report cannot be read at all, or an empty list.
function reportProblems(report) {
  const problems = [];
  if (!report || typeof report !== "object") return ["no report"];
  if (report.arming_failure != null) {
    problems.push(`the observer failed: ${report.arming_failure}`);
  }
  const capture = report.capture;
  if (!capture || typeof capture !== "object") {
    problems.push("no capture was admitted");
    return problems;
  }
  if (capture.failure != null) {
    problems.push(`the capture latched ${capture.failure}`);
  }
  for (const name of CAPTURE_LISTS) {
    if (!Array.isArray(capture[name])) problems.push(`${name} is missing`);
  }
  if (problems.length) return problems;
  // A row the judge walks is readable when it is there, carries its
  // identity and passes its own check; one that is not makes the capture one
  // it cannot judge, never an exception halfway through the merge.
  const readable = (rows, ok) =>
    rows.every((row) => row && row.identity && ok(row));
  const counted = (rows, fields) =>
    readable(rows, (row) => fields.every((field) => isCount(row[field])));
  if (
    !counted(capture.replies, [
      "receipts_before",
      "batches_before",
      "receipts_last",
      "batches_last",
    ])
  ) {
    problems.push("a reply has no positions");
  }
  if (
    !counted(capture.boundaries, [
      "receipts_before",
      "batches_before",
      "replies_before",
    ])
  ) {
    problems.push("a boundary has no positions");
  }
  const hasId = (row) => row && typeof row.id === "string";
  if (
    !readable(
      capture.batches,
      (batch) => Array.isArray(batch.calls) && batch.calls.every(hasId),
    )
  ) {
    problems.push("a batch has no calls");
  }
  if (!readable(capture.receipts, hasId)) {
    problems.push("a receipt has no id");
  }
  return problems;
}

/// A reading of the interview clock as seconds since the schedule began with
/// its own sampling uncertainty, or null when it cannot place an event.
function readClock(identity, scheduleStartUnixMs, maxUncertaintyMs) {
  const clock = identity?.clock;
  if (!clock || !Number.isFinite(clock.emitted_unix_ms)) return null;
  const uncertainty = clock.clock_uncertainty_ms;
  if (!Number.isFinite(uncertainty) || uncertainty > maxUncertaintyMs) {
    return null;
  }
  return {
    s: (clock.emitted_unix_ms - scheduleStartUnixMs) / 1000,
    u: uncertainty / 1000,
  };
}

/// Where a reading sits against a threshold: -1 before it, 1 after it, 0 when
/// its uncertainty reaches across, which is no answer.
function sideOf(reading, threshold) {
  if (reading.s + reading.u < threshold) return -1;
  return reading.s - reading.u > threshold ? 1 : 0;
}

/// Every string a tool result carries, one per line. The fact matchers look
/// for tokens in text, and JSON escaping would hide a token that spans a
/// newline or holds a quote.
function resultText(value) {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return value.map(resultText).join("\n");
  if (value && typeof value === "object") {
    return Object.values(value).map(resultText).join("\n");
  }
  return "";
}

/// Where the turn reply `index` belongs to began: the last ending that the
/// loop took on its socket and that arrived before the reply did. An ending of
/// another generation (swallowed, or from another socket) that falls inside the
/// turn leaves ownership of everything in it unknown.
function turnStart(capture, index) {
  const socket = capture.replies[index].identity.socket;
  let receipts = 0;
  let replies = 0;
  const strays = [];
  for (const boundary of capture.boundaries) {
    if (boundary.replies_before > index) continue;
    if (boundary.accepted && boundary.identity.socket === socket) {
      receipts = Math.max(receipts, boundary.receipts_before);
      replies = Math.max(replies, boundary.replies_before);
    } else {
      strays.push(boundary);
    }
  }
  const stray = strays.some((boundary) => boundary.receipts_before > receipts);
  return { receipts, replies, stray };
}

/// The segments of `socket` with index in [from, to).
const segmentsOf = (capture, socket, from, to) =>
  capture.replies
    .slice(from, to)
    .filter((reply) => reply.identity.socket === socket);

/// How reply `index` ended: "complete" or "interrupted" when the first
/// provider ending after it, on its socket and in its generation, was taken
/// and nothing else sealed it first, otherwise "unknown" with the reason.
function endingOf(capture, index) {
  const reply = capture.replies[index];
  const socket = reply.identity.socket;
  for (const boundary of capture.boundaries) {
    if (boundary.identity.socket !== socket) continue;
    if (boundary.replies_before <= index) continue;
    if (
      segmentsOf(capture, socket, index + 1, boundary.replies_before).length
    ) {
      return {
        kind: "unknown",
        why: "a cut or rollover sealed the reply before any provider ending",
      };
    }
    if (!boundary.accepted) {
      return {
        kind: "unknown",
        why: "its ending was swallowed by the interruption guard",
      };
    }
    if (
      reply.identity.trace_turn == null ||
      boundary.identity.trace_turn !== reply.identity.trace_turn
    ) {
      return {
        kind: "unknown",
        why: "the ending belongs to another generation than the reply",
      };
    }
    return {
      kind: boundary.ending === "Interrupted" ? "interrupted" : "complete",
    };
  }
  return { kind: "unknown", why: "no provider ending followed the reply" };
}

/// The tool calls reply `index` may cite, or the reason the facts do not say.
/// Calls count only when they were received on the reply's own socket and
/// generation after the previous ending, executed before the reply began, and
/// not received while output was being discarded.
function citedCalls(capture, index) {
  const reply = capture.replies[index];
  const socket = reply.identity.socket;
  if (
    reply.receipts_before !== reply.receipts_last ||
    reply.batches_before !== reply.batches_last
  ) {
    return { why: "the reply continued across a tool call" };
  }
  const turn = turnStart(capture, index);
  if (turn.stray) {
    return { why: "an ending of another generation lies inside the turn" };
  }
  if (segmentsOf(capture, socket, turn.replies, index).length) {
    return { why: "an earlier segment of the same turn may own the calls" };
  }
  const window = capture.receipts.slice(turn.receipts, reply.receipts_before);
  if (window.some((receipt) => receipt.identity.socket !== socket)) {
    return {
      why: "a socket replacement lies between the question and the reply",
    };
  }
  const calls = [];
  for (const receipt of window) {
    if (receipt.discarded) continue;
    if (receipt.batch == null || receipt.batch >= reply.batches_before) {
      continue;
    }
    if (
      reply.identity.trace_turn == null ||
      receipt.identity.trace_turn !== reply.identity.trace_turn
    ) {
      return { why: "a call belongs to another generation than the reply" };
    }
    calls.push(receipt);
  }
  return { calls };
}

/// The evidence for one cited call, or the reason it cannot be read.
function toolCall(capture, receipt, cp, key, options) {
  const batch = capture.batches.find((b) => b.sequence === receipt.batch);
  const call = batch?.calls.find((candidate) => candidate.id === receipt.id);
  if (!call) return { why: `call ${receipt.id} has no executed batch` };
  if (batch.local_send_completed == null) {
    return { why: `the send of call ${receipt.id} was not observed` };
  }
  const reading = readClock(
    batch.identity,
    options.scheduleStartUnixMs,
    options.maxClockUncertaintyMs,
  );
  if (!reading) {
    return { why: `call ${receipt.id} has no usable clock reading` };
  }
  const earliest = Math.max(
    cp.at_s,
    ...(key.tools ?? [key.tool])
      .filter((need) => need.name === call.name)
      .map((need) => need.not_before_s),
  );
  const latest = cp.stimulus_end_s + cp.response_window_s;
  if (sideOf(reading, earliest) === 0 || sideOf(reading, latest) === 0) {
    return {
      why: `call ${receipt.id} lies within the clock uncertainty of a limit`,
    };
  }
  const failed =
    call.result && typeof call.result === "object" && "error" in call.result;
  return {
    call: {
      call_id: call.id,
      name: call.name,
      at_s: reading.s,
      ok: !failed && batch.local_send_completed === true,
      result_text: resultText(call.result),
    },
  };
}

const notAssessed = (checkpointId, reasons) => ({
  checkpoint_id: checkpointId,
  verdict: "not_assessed",
  reasons,
});

const judge = (bundle, evidence) => ({
  checkpoint_id: evidence.checkpoint_id,
  ...judgeCheckpoint(bundle, evidence),
});

/// When the candidate had finished saying the earliest thing a checkpoint asks
/// for that only the conversation carries, in schedule seconds, or null when it
/// asks for none (`CONVERSATION_FACT_IDS` in scripts/long-interview-schedule.cjs
/// says which). A fact about the source is read back from a tool, and one the
/// problem's brief gives was never at risk, so neither shows anything kept
/// across a handover. A checkpoint's own question is not a seed, whatever it
/// quotes.
function earliestSpokenFactS(bundle, key) {
  const { clips, steps } = bundle.schedule;
  const heard = [];
  for (const fact of bundle.answer_key.facts) {
    if (!key.element_fact_ids.includes(fact.id)) continue;
    if (!sched.CONVERSATION_FACT_IDS.includes(fact.id)) continue;
    for (const clip of clips) {
      if (clip.role === "checkpoint") continue;
      if (!clip.source_text.includes(fact.text)) continue;
      for (const step of steps) {
        if (step.clip_id !== clip.clip_id) continue;
        // To the end of the clip's whole budget, its trailing quiet included:
        // a clip dispatched a little late or spoken a little slowly had still
        // ended by then.
        heard.push(step.at_s + sched.clipBudgetS(clip));
      }
    }
  }
  return heard.length ? Math.min(...heard) : null;
}

/// The replies that started inside the checkpoint's window, as {index, at}
/// in order, or a reason when the clock cannot say. A second one is usual: the
/// interviewer's own nudge ("mm-hm", "just checking in") after an answer.
function repliesInWindow(capture, from, to, settings) {
  const inside = [];
  for (const [index, reply] of capture.replies.entries()) {
    const reading = readClock(
      reply.identity,
      settings.scheduleStartUnixMs,
      settings.maxClockUncertaintyMs,
    );
    if (!reading) return { why: "a reply has no usable clock reading" };
    const [start, end] = [sideOf(reading, from), sideOf(reading, to)];
    if (start === 0 || end === 0) {
      return {
        why: "a reply lies within the clock uncertainty of the window edge",
      };
    }
    if (start === 1 && end === -1) inside.push({ index, at: reading.s });
  }
  return { replies: inside };
}

/// One checkpoint's verdict: "pass", "fail" or "not_assessed", with reasons.
function assessCheckpoint(bundle, report, checkpointId, options) {
  const settings = {
    maxClockUncertaintyMs: MAX_CLOCK_UNCERTAINTY_MS,
    ...options,
  };
  const problems = reportProblems(report);
  if (problems.length) return notAssessed(checkpointId, problems);
  if (!Number.isFinite(settings.scheduleStartUnixMs)) {
    return notAssessed(checkpointId, ["the schedule origin is unknown"]);
  }
  const cp = bundle.schedule.checkpoints.find(
    (c) => c.checkpoint_id === checkpointId,
  );
  const key = bundle.answer_key.checkpoints.find(
    (c) => c.checkpoint_id === checkpointId,
  );
  if (!cp || !key) return notAssessed(checkpointId, ["unknown checkpoint"]);
  const capture = report.capture;

  // The window opens at the question's estimated end, from its word count; a
  // recording that finished speaking sooner can be answered before that. Such
  // a reply cannot be told from one that talked over the question, so neither
  // is held against the reply as a missing answer.
  const early = repliesInWindow(capture, cp.at_s, cp.stimulus_end_s, settings);
  if (early.why) return notAssessed(checkpointId, [early.why]);
  if (early.replies.length) {
    return notAssessed(checkpointId, [
      "a reply began before the question's estimated end",
    ]);
  }

  const found = repliesInWindow(
    capture,
    cp.stimulus_end_s,
    cp.stimulus_end_s + cp.response_window_s,
    settings,
  );
  if (found.why) return notAssessed(checkpointId, [found.why]);
  if (!found.replies.length)
    return judge(bundle, { checkpoint_id: checkpointId });

  // Each reply is judged on its own. One that answers passes the checkpoint,
  // whatever the interviewer said before or after it; with none passing, the
  // checkpoint fails only when every reply was assessed.
  const results = found.replies.map((one) =>
    assessReply(bundle, capture, { cp, key, checkpointId }, one, settings),
  );
  const passed = results.find((result) => result.verdict === "pass");
  if (passed) return passed;
  if (results.length === 1) return results[0];
  const verdict = results.every((result) => result.verdict === "fail")
    ? "fail"
    : "not_assessed";
  return {
    checkpoint_id: checkpointId,
    verdict,
    reasons: results.flatMap((result, i) =>
      result.reasons.map((reason) => `reply ${i + 1}: ${reason}`),
    ),
  };
}

/// One reply's verdict on a checkpoint: "pass", "fail" or "not_assessed".
function assessReply(
  bundle,
  capture,
  { cp, key, checkpointId },
  found,
  settings,
) {
  const { index, at } = found;
  const reply = capture.replies[index];
  if (reply.revised) {
    return notAssessed(checkpointId, [
      "the reply's text was rewritten while it was shown",
    ]);
  }
  if (!reply.final_segment) {
    return notAssessed(checkpointId, ["the reply was never sealed"]);
  }
  const ending = endingOf(capture, index);
  if (ending.kind === "unknown") return notAssessed(checkpointId, [ending.why]);
  const cited = citedCalls(capture, index);
  if (!cited.calls) return notAssessed(checkpointId, [cited.why]);
  const calls = [];
  for (const receipt of cited.calls) {
    const read = toolCall(capture, receipt, cp, key, settings);
    if (!read.call) return notAssessed(checkpointId, [read.why]);
    calls.push(read.call);
  }
  return {
    ...judge(bundle, {
      checkpoint_id: checkpointId,
      reply: {
        text: reply.text,
        started_at_s: at,
        completed: ending.kind === "complete",
        interrupted: ending.kind === "interrupted",
        tool_call_ids: calls.map((call) => call.call_id),
      },
      tool_calls: calls,
    }),
    // Where the judged reply was said and how far back its question reached,
    // which is what ties a passing answer to the handover before it.
    reply: {
      session: reply.identity.session,
      socket: reply.identity.socket,
      clock_start_unix_ms: reply.identity.clock?.start_unix_ms,
    },
    earliest_fact_heard_s: earliestSpokenFactS(bundle, key),
  };
}

/// "verified" only when every checkpoint was assessed and passed; "failed" when
/// any assessed one failed; otherwise "not_assessed".
function summarize(checkpoints) {
  const failed = checkpoints.some((c) => c.verdict === "fail");
  const verified =
    checkpoints.length > 0 && checkpoints.every((c) => c.verdict === "pass");
  return {
    status: failed ? "failed" : verified ? "verified" : "not_assessed",
    checkpoints,
    uncovered: checkpoints
      .filter((c) => c.verdict === "not_assessed")
      .map((c) => c.checkpoint_id),
  };
}

function judgeInterview(bundle, report, options) {
  return summarize(
    bundle.schedule.checkpoints.map((cp) =>
      assessCheckpoint(bundle, report, cp.checkpoint_id, options),
    ),
  );
}

const MAX_REASONS = 20;

/// Why a fixture report that is there cannot vouch for the run, read the same
/// by the keep check and by the judge: unreadable, or ended other than well.
function fixtureRefusal(report) {
  if (report?.unreadable) {
    return `the serving fixture's report could not be read: ${report.unreadable}`;
  }
  if (typeof report?.outcome === "string" && report.outcome !== "ok") {
    return `the serving fixture ended with ${report.outcome}`;
  }
  return null;
}

/// Why the serving fixture's report is not one to accept, from its shape and
/// the outcome it names alone. A failure the observer or the capture reports
/// about itself is not a reason here: the judge says not_assessed for it, and
/// the run keeps its own result. A report that holds neither a capture nor an
/// observer failure is malformed, and refuses the run.
function fixtureProblems(report) {
  const refusal = fixtureRefusal(report);
  if (refusal) return [refusal];
  if (!report || typeof report !== "object" || Array.isArray(report)) {
    return ["the serving fixture's report is not an object"];
  }
  if (typeof report.outcome !== "string") {
    return ["the serving fixture's report names no outcome"];
  }
  const problems = [];
  for (const key of ["arming_failure", "bound"]) {
    if (!(key in report)) problems.push(`the report has no ${key}`);
  }
  const capture = report.capture;
  // An observer that failed admits no capture and says so itself; that is the
  // judge's to name. A report with neither is not one an interview left.
  if (capture == null && report.arming_failure != null) return problems;
  if (!capture || typeof capture !== "object") {
    return [...problems, "the report holds no capture"];
  }
  if (!("failure" in capture))
    problems.push("the capture has no failure field");
  for (const name of CAPTURE_LISTS) {
    if (!Array.isArray(capture[name]))
      problems.push(`the capture has no ${name} list`);
  }
  return problems;
}

/// Judges a capture against its fixture report, or says in every
/// checkpoint's reason why it could not.
function judgeCapture(capture, fixture) {
  const { report } = capture;
  let bundle;
  try {
    bundle = generateSchedule(report.schedule_mode, {
      durationMin: capture.plan.durationMin,
      observationTargetS: capture.plan.observationTargetS,
    });
  } catch (error) {
    return summarize([
      notAssessed("interview", [
        `the schedule cannot be rebuilt: ${error.message}`,
      ]),
    ]);
  }
  const refuse = (reason) =>
    summarize(
      bundle.schedule.checkpoints.map((cp) =>
        notAssessed(cp.checkpoint_id, [reason]),
      ),
    );
  if (report.exit_code !== 0 || report.status !== "complete") {
    return refuse("the run did not complete cleanly");
  }
  // A tone fixture asked the model nothing, so there is no content to judge.
  if (report.validation_fixture !== false) {
    return refuse("the run played tone fixtures, not spoken questions");
  }
  if (report.anchor?.source !== "server_clock") {
    return refuse("the interview clock was not anchored to the server");
  }
  if (!fixture) return refuse("the serving fixture left no report");
  const refusal =
    fixtureRefusal(fixture) ??
    (fixture.outcome === "ok"
      ? null
      : "the serving fixture's report names no outcome");
  if (refusal) return refuse(refusal);
  if (bundle.schedule.schedule_sha256 !== report.schedule_sha256) {
    return refuse("the schedule that ran is not the one rebuilt from the plan");
  }
  return judgeInterview(bundle, fixture, {
    scheduleStartUnixMs: report.schedule_origin_unix_ms,
  });
}

/// The three report fields a verdict sets. The reasons are always present,
/// empty for a verified interview, so that rewriting a report with a better
/// verdict does not keep an earlier one's.
function verdictFields(verdict) {
  const counts = { pass: 0, fail: 0, not_assessed: 0 };
  for (const checkpoint of verdict.checkpoints) counts[checkpoint.verdict] += 1;
  const reasons = verdict.checkpoints
    .filter((checkpoint) => checkpoint.verdict !== "pass")
    .map(
      (checkpoint) => `${checkpoint.checkpoint_id}: ${checkpoint.reasons[0]}`,
    );
  return {
    reply_content: verdict.status,
    reply_content_checkpoints: counts,
    reply_content_reasons: reasons.slice(0, MAX_REASONS),
  };
}

/// The capture file with its long-run report's `reply_content` filled in from
/// the serving fixture's report, and with the qualification verdict that
/// content was the last thing missing for (scripts/long-interview-qualify.cjs).
/// The report's own exit code does not move; what the process leaves with is
/// `main`'s to say.
function mergeVerdict(capture, fixture, runKind, teardownFailed = false) {
  const report = capture?.report;
  if (capture?.flow !== "long-interview" || !report) return null;
  const verdict = judgeCapture(capture, fixture);
  const judged = { ...report, ...verdictFields(verdict) };
  // Without a run kind this is a reader looking at the content again, not the
  // run's own judgment: whatever qualification the capture holds is left.
  if (!runKind) return { ...capture, report: judged };
  const outcome = qualify({
    runKind,
    report: judged,
    checkpoints: verdict.checkpoints,
    teardownFailed,
  });
  return {
    ...capture,
    report: {
      ...judged,
      ...outcome,
      assessment_scope:
        outcome.qualification === "not_evaluated"
          ? "structural"
          : "qualification",
    },
  };
}

/// A JSON file, null when it is not there, or why it cannot be read: a report
/// that is there but unreadable is a different failure from one the fixture
/// never wrote, and the verdict says which.
const readJson = (file) => {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch (error) {
    // The kind of failure and no more: a parse error's message quotes the
    // text around it, which can be the interviewer's reply.
    if (error.code === "ENOENT") return null;
    return { unreadable: error.code ?? error.name };
  }
};

/// What a qualification run leaves with: 0 for a pass, 1 for a failure, and
/// 77 for anything else, an inconclusive verdict or one nobody could read.
function qualificationExit(qualification) {
  if (qualification === "pass") return EXIT.pass;
  if (qualification === "fail") return EXIT.failed;
  return EXIT.inconclusive;
}

/// `--capture FILE --fixture FILE [--run-kind KIND] [--teardown-failed]`:
/// rewrites the capture
/// with the verdict. `--teardown-failed` says the fixture or a capture failed
/// to stop after the worker exited, which fails a qualification.
///
/// For every run kind but `qualification` it exits 0 when it merged or had
/// nothing to judge and 2 when it could not read the capture, judge it or write
/// it back, and the caller treats either as no change to the run's own result.
/// For a qualification the exit is the verdict: 0 pass, 1 fail, 77
/// inconclusive, which is also what a run it could not judge comes to.
function main(argv, { out = console.log, err = console.error } = {}) {
  if (argv[0] === "--check-fixture") {
    const problems = fixtureProblems(readJson(argv[1] ?? ""));
    problems.forEach((problem) => err(`fixture report: ${problem}`));
    return problems.length ? 1 : 0;
  }
  const flag = (name) => {
    const at = argv.indexOf(name);
    return at < 0 ? undefined : argv[at + 1];
  };
  const [captureFile, fixtureFile] = [flag("--capture"), flag("--fixture")];
  const runKind = flag("--run-kind");
  const qualifying = runKind === "qualification";
  const unjudged = (message) => {
    err(message);
    if (!qualifying) return 2;
    out("long-interview qualification=inconclusive: the run was not judged");
    return EXIT.inconclusive;
  };
  if (!captureFile || !fixtureFile) {
    return unjudged(
      "usage: long-interview-evidence.cjs --capture FILE --fixture FILE [--run-kind KIND] [--teardown-failed]",
    );
  }

  const capture = readJson(captureFile);
  if (!capture || capture.unreadable) {
    const why = capture?.unreadable ? `: ${capture.unreadable}` : "";
    return unjudged(`cannot read the capture ${captureFile}${why}`);
  }
  // A judgment that throws still says it was not judged, so a direct caller,
  // who has no shell reading the printed verdict, does not see a bare 1.
  let merged;
  try {
    merged = mergeVerdict(
      capture,
      readJson(fixtureFile),
      runKind,
      argv.includes("--teardown-failed"),
    );
  } catch (error) {
    return unjudged(`cannot judge ${captureFile}: ${error.message}`);
  }
  if (!merged) {
    // Another flow's capture is nothing to judge, and only a qualification
    // has anything to say about that.
    if (!qualifying) return 0;
    return unjudged(`${captureFile} holds no long-interview report`);
  }
  const partial = `${captureFile}.partial`;
  try {
    fs.writeFileSync(partial, `${JSON.stringify(merged, null, 2)}\n`);
    fs.renameSync(partial, captureFile);
  } catch (error) {
    return unjudged(`cannot write ${captureFile}: ${error.message}`);
  }
  const { report } = merged;
  const counts = report.reply_content_checkpoints;
  out(
    `long-interview reply_content=${report.reply_content} pass=${counts.pass} fail=${counts.fail} not_assessed=${counts.not_assessed}`,
  );
  if (!qualifying) return 0;
  out(
    `long-interview qualification=${report.qualification} rotations_completed=${report.rotations_completed} rotations_verified=${report.rotations_verified}`,
  );
  report.qualification_reasons.forEach((reason) =>
    out(`long-interview qualification: ${reason}`),
  );
  return qualificationExit(report.qualification);
}

if (require.main === module) process.exit(main(process.argv.slice(2)));

module.exports = {
  MAX_CLOCK_UNCERTAINTY_MS,
  assessCheckpoint,
  earliestSpokenFactS,
  fixtureProblems,
  judgeInterview,
  main,
  mergeVerdict,
  qualificationExit,
  reportProblems,
  summarize,
  verdictFields,
};
