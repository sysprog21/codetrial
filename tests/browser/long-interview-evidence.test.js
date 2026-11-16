import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import { fileURLToPath } from "node:url";

import evidence_ from "../../scripts/long-interview-evidence.cjs";
import sched from "../../scripts/long-interview-schedule.cjs";

const {
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
} = evidence_;
const { answerFor, generateSchedule } = sched;

const ROOT = fileURLToPath(new URL("../..", import.meta.url));
const BUNDLE = generateSchedule("coding", {
  durationMin: 90,
  observationTargetS: 5400,
});
const CP = BUNDLE.schedule.checkpoints[0];
const ANSWER = answerFor(BUNDLE, CP.checkpoint_id);
const START = 1_700_000_000_000;

// What the Rust producer wrote: three replies (the first cites an executed
// read_editor call), then a completed, an interrupted one and a swallowed
// ending. Structure is the producer's; only the clocks and the authored
// content are put back, because a fixture can hold neither.
const FIXTURE = JSON.parse(
  fs.readFileSync(`${ROOT}/tests/fixtures/acquisition-report.json`, "utf8"),
);

function at(identity, seconds, uncertainty = 4) {
  identity.clock.emitted_unix_ms = START + Math.round(seconds * 1000);
  identity.clock.clock_uncertainty_ms = uncertainty;
}

/// The fixture as a faithful run of checkpoint 1: reply A answers it after
/// reading the editor, replies B and C come long after the window.
function faithful(edit = () => {}) {
  const report = structuredClone(FIXTURE);
  const { replies, batches, receipts } = report.capture;
  replies[0].text = ANSWER.reply.text;
  batches[0].calls[0].name = ANSWER.tool_calls[0].name;
  batches[0].calls[0].result = ANSWER.tool_calls[0].result_text;
  receipts[0].name = ANSWER.tool_calls[0].name;
  at(batches[0].identity, ANSWER.tool_calls[0].at_s);
  at(replies[0].identity, ANSWER.reply.started_at_s);
  at(replies[1].identity, 400);
  at(replies[2].identity, 500);
  edit(report);
  return report;
}

const OPTIONS = { scheduleStartUnixMs: START };
const assess = (report, options = OPTIONS) =>
  assessCheckpoint(BUNDLE, report, CP.checkpoint_id, options);

/// One test per row: the edit, applied to a faithful run, must give `verdict`
/// with a reason matching the pattern.
function rows(table, verdict, suffix) {
  for (const [name, edit, pattern] of table) {
    test(`${name} ${suffix}`, () => {
      const result = assess(faithful(edit));
      assert.equal(result.verdict, verdict, reasonsOf(result));
      assert.match(reasonsOf(result), pattern);
    });
  }
}

function reasonsOf(result) {
  return result.reasons.join(" | ");
}

test("a faithful reply that cites an executed read passes", () => {
  const report = faithful();
  const { identity } = report.capture.replies[0];
  assert.deepEqual(assess(report), {
    checkpoint_id: CP.checkpoint_id,
    verdict: "pass",
    reasons: [],
    // Where it was said, for the reader that ties an answer to a handover.
    reply: {
      session: identity.session,
      socket: identity.socket,
      clock_start_unix_ms: identity.clock.start_unix_ms,
    },
    // The first checkpoint reads the source back; nothing it asks was spoken.
    earliest_fact_heard_s: null,
  });
});

test("a nudge after the answer, in the same window, does not void it", () => {
  const result = assess(
    faithful((r) => at(r.capture.replies[1].identity, 150)),
  );
  assert.equal(result.verdict, "pass", reasonsOf(result));
  assert.equal(result.reply.socket, 1);
});

test("with two replies in the window and neither answering, each is named", () => {
  const result = assess(
    faithful((r) => {
      r.capture.replies[0].text = "Nothing useful.";
      at(r.capture.replies[1].identity, 150);
    }),
  );
  assert.equal(result.verdict, "fail", reasonsOf(result));
  assert.match(reasonsOf(result), /reply 1: .*\| reply 2: /);
});

test("a later nudge that repeats the answer without the read does not pass", () => {
  const result = assess(
    faithful((r) => {
      const [a, b] = r.capture.replies;
      b.text = a.text;
      a.text = "Nothing useful.";
      at(b.identity, 150);
    }),
  );
  assert.equal(result.verdict, "fail", reasonsOf(result));
  assert.match(reasonsOf(result), /reply 2: .*no read_editor call/);
});

test("one reply that cannot be assessed keeps a failing checkpoint open", () => {
  const result = assess(
    faithful((r) => {
      r.capture.replies[0].text = "Nothing useful.";
      r.capture.replies[1].revised = true;
      at(r.capture.replies[1].identity, 150);
    }),
  );
  assert.equal(result.verdict, "not_assessed", reasonsOf(result));
  assert.match(reasonsOf(result), /reply 2: .*rewritten/);
});

test("an answer that passes stands when the nudge after it cannot be assessed", () => {
  const result = assess(
    faithful((r) => {
      r.capture.replies[1].revised = true;
      at(r.capture.replies[1].identity, 150);
    }),
  );
  assert.equal(result.verdict, "pass", reasonsOf(result));
  assert.equal(result.reply.socket, 1);
});

/// What the driver records for each clip it sent: the actual dispatch time,
/// `late` seconds after the planned one.
const sentAt = (bundle, late = 0) =>
  bundle.schedule.steps
    .filter((step) => step.clip_id)
    .map((step) => ({
      clip_id: step.clip_id,
      action_id: step.action_id,
      started_at_s: step.at_s,
      playing_at_s: step.at_s + late,
    }));

test("a fact is heard from when its clip was sent, not when it was planned", () => {
  const key = BUNDLE.answer_key.checkpoints.find(
    (k) => k.checkpoint_id === "c07_cp_state",
  );
  const onTime = earliestSpokenFactS(BUNDLE, key, sentAt(BUNDLE));
  assert.ok(Number.isFinite(onTime));
  assert.equal(
    earliestSpokenFactS(BUNDLE, key, sentAt(BUNDLE, 30)),
    onTime + 30,
  );
  // A playback that began long after the dispatch is heard from when it began.
  assert.ok(
    earliestSpokenFactS(BUNDLE, key, sentAt(BUNDLE, 700)) > onTime + 600,
  );
  // A record from before playback began says nothing of when it was heard.
  const dispatchOnly = sentAt(BUNDLE).map(({ playing_at_s, ...rest }) => rest);
  assert.equal(earliestSpokenFactS(BUNDLE, key, dispatchOnly), null);
  // A clip with no dispatch record may never have been sent, and no record of
  // any says nothing was heard: unknown, never an earlier time.
  assert.equal(earliestSpokenFactS(BUNDLE, key, []), null);
  assert.equal(earliestSpokenFactS(BUNDLE, key, undefined), null);
});

test("only what the candidate alone said can show the conversation survived", () => {
  const transmitted = sentAt(BUNDLE);
  const reach = (id) =>
    earliestSpokenFactS(
      BUNDLE,
      BUNDLE.answer_key.checkpoints.find((k) => k.checkpoint_id === id),
      transmitted,
    );
  // The constraints and the stage are spoken too, but the problem's own brief
  // gives the constraints and the session evidence gives the stage, so a
  // checkpoint that asks only for them shows nothing remembered. The seeds and
  // the question left open are said by the candidate and nowhere else.
  const reaching = BUNDLE.schedule.checkpoints
    .map((cp) => cp.checkpoint_id)
    .filter((id) => reach(id) !== null);
  assert.deepEqual(reaching, [
    "c07_cp_state",
    "c08_cp_state",
    "c13_cp_state",
    "c14_cp_state",
    "cp_final",
  ]);
  // A seed is heard by the end of the clip that says it, long before its
  // recall; the question left open is said late, before the final checkpoint.
  for (const id of reaching.slice(0, 4)) assert.ok(reach(id) < 700, id);
  assert.ok(reach("cp_final") > 4000);
  // A question that quoted a seed would not make the seed its own: with every
  // clip that says the seed made to say nothing, and the checkpoint's own
  // question quoting it, nothing was heard.
  const quoting = structuredClone(BUNDLE);
  const key = quoting.answer_key.checkpoints.find(
    (k) => k.checkpoint_id === "c07_cp_state",
  );
  const facts = quoting.answer_key.facts.filter(
    (f) =>
      key.element_fact_ids.includes(f.id) &&
      sched.CONVERSATION_FACT_IDS.includes(f.id),
  );
  assert.ok(facts.length > 0);
  const question = quoting.schedule.clips.find(
    (c) =>
      c.clip_id ===
      quoting.schedule.checkpoints.find(
        (cp) => cp.checkpoint_id === key.checkpoint_id,
      ).clip_id,
  );
  assert.equal(question.role, "checkpoint");
  for (const fact of facts) {
    for (const clip of quoting.schedule.clips) {
      clip.source_text = clip.source_text.split(fact.text).join("nothing");
    }
    question.source_text += ` ${fact.text}`;
  }
  assert.equal(earliestSpokenFactS(quoting, key, transmitted), null);
});

test("a malformed send record leaves the fact time unknown", () => {
  const key = BUNDLE.answer_key.checkpoints.find(
    (k) => k.checkpoint_id === "c07_cp_state",
  );
  for (const transmitted of [
    "none",
    {},
    [null],
    [{ playing_at_s: 1 }],
    undefined,
  ]) {
    assert.equal(earliestSpokenFactS(BUNDLE, key, transmitted), null);
  }
});

test("the raw producer report cannot place anything in time", () => {
  const result = assess(structuredClone(FIXTURE));
  assert.equal(result.verdict, "not_assessed");
  assert.match(reasonsOf(result), /clock/);
});

const UNREADABLE = [
  ["an observer failure", (r) => (r.arming_failure = "Scope"), /observer/],
  ["a latched capture", (r) => (r.capture.failure = "Limit"), /latched Limit/],
  ["no capture", (r) => (r.capture = null), /no capture/],
  [
    "a reply without positions",
    (r) => delete r.capture.replies[0].receipts_last,
    /positions/,
  ],
  ["a batch without calls", (r) => delete r.capture.batches[0].calls, /calls/],
  [
    "a batch with an unreadable call",
    (r) => (r.capture.batches[0].calls = [null]),
    /calls/,
  ],
  [
    "a receipt without an id",
    (r) => delete r.capture.receipts[0].id,
    /receipt/,
  ],
];
rows(UNREADABLE, "not_assessed", "makes every checkpoint not_assessed");

test("an unknown schedule origin or checkpoint is not assessed", () => {
  assert.equal(assess(faithful(), {}).verdict, "not_assessed");
  assert.equal(
    assessCheckpoint(BUNDLE, faithful(), "nope", OPTIONS).verdict,
    "not_assessed",
  );
});

test("a clock reading too uncertain to place an event is unusable", () => {
  const result = assess(
    faithful((r) => at(r.capture.replies[0].identity, 140, 5000)),
  );
  assert.equal(result.verdict, "not_assessed");
});

const UNKNOWN = [
  [
    "a swallowed ending",
    (r) => (r.capture.boundaries[0].accepted = false),
    /swallowed/,
  ],
  [
    "a reply sealed by a cut before any ending",
    (r) => (r.capture.boundaries[0].replies_before = 2),
    /sealed the reply/,
  ],
  [
    "no ending at all",
    (r) => (r.capture.boundaries = []),
    /no provider ending/,
  ],
  [
    "a reply that continued across a tool call",
    (r) => (r.capture.replies[0].receipts_last = 2),
    /continued across/,
  ],
  [
    "a call received on another socket",
    (r) => (r.capture.receipts[0].identity.socket = 2),
    /replacement/,
  ],
  [
    "a rewritten reply",
    (r) => (r.capture.replies[0].revised = true),
    /rewritten/,
  ],
  [
    "a reply never sealed",
    (r) => (r.capture.replies[0].final_segment = false),
    /never sealed/,
  ],
  [
    "a reply just before the window, within the uncertainty",
    (r) => at(r.capture.replies[1].identity, CP.stimulus_end_s - 0.5, 1000),
    /window edge/,
  ],
  [
    "a reply just after the window, within the uncertainty",
    (r) =>
      at(
        r.capture.replies[1].identity,
        CP.stimulus_end_s + CP.response_window_s + 0.5,
        1000,
      ),
    /window edge/,
  ],
  [
    "a reply within the clock uncertainty of the question's start",
    (r) => at(r.capture.replies[2].identity, CP.at_s + 0.2, 1000),
    /window edge/,
  ],
  [
    "the faithful reply, after a recording that ended its speech early",
    (r) => at(r.capture.replies[0].identity, CP.stimulus_end_s - 2),
    /before the question's estimated end/,
  ],
  [
    "the faithful reply too close to the start of the window",
    (r) => at(r.capture.replies[0].identity, CP.stimulus_end_s + 0.2, 1000),
    /window edge/,
  ],
  [
    "a generation other than the reply's ended it",
    (r) => (r.capture.boundaries[0].identity.trace_turn = 9),
    /another generation/,
  ],
  [
    "a call from another generation",
    (r) => (r.capture.receipts[0].identity.trace_turn = 9),
    /another generation/,
  ],
  [
    "an ending of another generation inside the turn",
    (r) => {
      const stray = structuredClone(r.capture.boundaries[0]);
      stray.accepted = false;
      stray.replies_before = 0;
      r.capture.boundaries.unshift(stray);
    },
    /another generation lies inside/,
  ],
  [
    "an ending taken on another socket inside the turn",
    (r) => {
      const other = structuredClone(r.capture.boundaries[0]);
      other.identity.socket = 2;
      other.replies_before = 0;
      r.capture.boundaries.unshift(other);
    },
    /another generation lies inside/,
  ],
  [
    "a send nobody observed",
    (r) => (r.capture.batches[0].local_send_completed = null),
    /not observed/,
  ],
  [
    "a call with no usable clock reading",
    (r) => at(r.capture.batches[0].identity, 136, 5000),
    /usable clock/,
  ],
  [
    "a call too close to the limit it must clear",
    (r) => at(r.capture.batches[0].identity, 125.2, 1000),
    /uncertainty of a limit/,
  ],
  [
    "a cited call with no executed batch",
    (r) => (r.capture.batches[0].calls[0].id = "other"),
    /no executed batch/,
  ],
  [
    "a reply that continued across an executed batch",
    (r) => (r.capture.replies[0].batches_last = 2),
    /continued across/,
  ],
  [
    "an earlier segment of the same turn",
    (r) => {
      // The faithful reply is B, which follows A with no ending between.
      const [a, b] = r.capture.replies;
      b.text = a.text;
      at(b.identity, ANSWER.reply.started_at_s);
      at(a.identity, 400);
      r.capture.boundaries[0].replies_before = 0;
    },
    /same turn/,
  ],
];
rows(UNKNOWN, "not_assessed", "is not assessed, never a pass or a fail");

const FAILS = [
  [
    "no reply in the window",
    (r) => at(r.capture.replies[0].identity, 300),
    /no reply was recorded/,
  ],
  [
    "a reply without the nonce",
    (r) => (r.capture.replies[0].text = "something unrelated"),
    /nonce/,
  ],
  [
    "an interrupted reply",
    (r) => {
      // The faithful reply is the interrupted one (reply C).
      const [a, , c] = r.capture.replies;
      c.text = a.text;
      at(c.identity, ANSWER.reply.started_at_s);
      at(a.identity, 400);
    },
    /interrupted/,
  ],
  [
    "a call received while output was being discarded",
    (r) => (r.capture.receipts[0].discarded = true),
    /no read_editor call/,
  ],
  [
    "a call that was received and never executed",
    (r) => (r.capture.receipts[0].batch = null),
    /no read_editor call/,
  ],
  [
    "a call executed after the reply began",
    (r) => {
      r.capture.replies[0].batches_before = 0;
      r.capture.replies[0].batches_last = 0;
    },
    /no read_editor call/,
  ],
  [
    "a response that was not sent",
    (r) => (r.capture.batches[0].local_send_completed = false),
    /did not succeed/,
  ],
  [
    "a tool that answered with an error",
    (r) => (r.capture.batches[0].calls[0].result = { error: "denied" }),
    /did not succeed/,
  ],
  [
    "a result that lacks the revision",
    (r) => (r.capture.batches[0].calls[0].result = "nothing useful"),
    /lacks/,
  ],
];
rows(FAILS, "fail", "fails the checkpoint");

test("a tool result is matched on its strings, not its JSON escaping", () => {
  const report = faithful((r) => {
    r.capture.batches[0].calls[0].result = {
      frameworkState: { phases: [] },
      result: `line one\n"${ANSWER.tool_calls[0].result_text}"\n`,
    };
  });
  assert.equal(assess(report).verdict, "pass");
});

test("a reply that began before the question was asked does not count against it", () => {
  // The previous question's answer, say: before at_s, so outside both the
  // early span and the window.
  const report = faithful((r) =>
    at(r.capture.replies[2].identity, CP.at_s - 5),
  );
  assert.equal(assess(report).verdict, "pass");
});

test("an ending from another socket does not close the reply", () => {
  const report = faithful((r) => {
    const other = structuredClone(r.capture.boundaries[0]);
    other.identity.socket = 2;
    other.ending = "Interrupted";
    r.capture.boundaries.unshift(other);
  });
  assert.equal(assess(report).verdict, "pass");
});

test("a reply on another socket between a reply and its ending is not its sealer", () => {
  const report = faithful((r) => {
    r.capture.replies[1].identity.socket = 2;
    r.capture.boundaries[0].replies_before = 2;
  });
  assert.equal(assess(report).verdict, "pass");
});

test("a later reply does not inherit the previous turn's call", () => {
  const report = faithful((r) => {
    const [a, b] = r.capture.replies;
    b.text = a.text;
    at(b.identity, ANSWER.reply.started_at_s);
    at(a.identity, 400);
  });
  const result = assess(report);
  assert.equal(result.verdict, "fail", reasonsOf(result));
  assert.match(reasonsOf(result), /no read_editor call/);
});

test("a report missing its parts is named", () => {
  for (const [edit, pattern] of [
    [(r) => delete r.capture.boundaries, /boundaries is missing/],
    [
      (r) => delete r.capture.boundaries[0].replies_before,
      /boundary has no positions/,
    ],
    [(r) => delete r.capture.replies[0].identity, /reply has no positions/],
  ]) {
    const report = structuredClone(FIXTURE);
    edit(report);
    assert.match(reportProblems(report).join(" "), pattern);
  }
});

test("the interview is verified only when every checkpoint passed", () => {
  const pass = { checkpoint_id: "a", verdict: "pass", reasons: [] };
  const fail = { checkpoint_id: "b", verdict: "fail", reasons: ["x"] };
  const unknown = {
    checkpoint_id: "c",
    verdict: "not_assessed",
    reasons: ["y"],
  };
  assert.equal(summarize([pass, pass]).status, "verified");
  assert.equal(summarize([pass, unknown]).status, "not_assessed");
  assert.deepEqual(summarize([pass, unknown]).uncovered, ["c"]);
  assert.equal(summarize([pass, fail, unknown]).status, "failed");
  assert.equal(summarize([]).status, "not_assessed");
});

test("judging the interview covers every scheduled checkpoint", () => {
  const verdict = judgeInterview(BUNDLE, faithful(), {
    scheduleStartUnixMs: START,
  });
  assert.equal(verdict.checkpoints.length, BUNDLE.schedule.checkpoints.length);
  assert.equal(verdict.checkpoints[0].verdict, "pass");
  // The run holds one faithful reply, so every other checkpoint found none.
  assert.equal(verdict.status, "failed");
  const broken = judgeInterview(
    BUNDLE,
    faithful((r) => (r.capture.failure = "Limit")),
    {
      scheduleStartUnixMs: START,
    },
  );
  assert.equal(broken.status, "not_assessed");
  assert.equal(broken.uncovered.length, BUNDLE.schedule.checkpoints.length);
});

test("an unreadable report is named, not thrown on", () => {
  assert.deepEqual(reportProblems(null), ["no report"]);
  assert.deepEqual(reportProblems(faithful()), []);
});

// The capture file the browser check leaves, with the long-run report fields
// the merge reads.
function capture() {
  const plan = { durationMin: 90, observationTargetS: 5400 };
  return {
    flow: "long-interview",
    plan,
    report: {
      status: "complete",
      exit_code: 0,
      anchor: { source: "server_clock" },
      validation_fixture: false,
      schedule_mode: "coding",
      assessment_scope: "structural",
      qualification: "not_evaluated",
      reply_content: "not_assessed",
      schedule_sha256: BUNDLE.schedule.schedule_sha256,
      schedule_origin_unix_ms: START,
    },
  };
}

/// The serving fixture's report: the acquisition report it left, ended ok.
const servingReport = () => ({
  outcome: "ok",
  bound: "127.0.0.1:0",
  ...faithful(),
});

function merged(edit = () => {}) {
  const [file, fixture] = [capture(), servingReport()];
  edit(file, fixture);
  return mergeVerdict(file, fixture);
}

test("the merge fills in reply_content and nothing else", () => {
  const result = merged();
  assert.equal(result.report.reply_content, "failed");
  assert.deepEqual(result.report.reply_content_checkpoints, {
    pass: 1,
    fail: BUNDLE.schedule.checkpoints.length - 1,
    not_assessed: 0,
  });
  assert.equal(result.report.qualification, "not_evaluated");
  assert.equal(result.report.status, "complete");
  assert.match(result.report.reply_content_reasons[0], /no reply was recorded/);
});

const UNJUDGED = [
  [
    "a fixture that did not end ok",
    (file, fixture) => (fixture.outcome = "shutdown bound"),
    /ended with shutdown bound/,
  ],
  [
    "a schedule other than the planned one",
    (file) => (file.report.schedule_sha256 = "0"),
    /not the one rebuilt/,
  ],
  [
    "an unknown schedule origin",
    (file) => delete file.report.schedule_origin_unix_ms,
    /origin/,
  ],
  [
    "a latched capture",
    (file, fixture) => (fixture.capture.failure = "Limit"),
    /latched/,
  ],
  [
    "a plan that builds no schedule",
    (file) => (file.plan.durationMin = 1),
    /cannot be rebuilt/,
  ],
  [
    "a surface that is not one",
    (file) => (file.report.schedule_mode = "dispatch"),
    /cannot be rebuilt/,
  ],
  [
    "a run that failed",
    (file) => (file.report.exit_code = 1),
    /did not complete cleanly/,
  ],
  [
    "a run that did not finish",
    (file) => (file.report.status = "failed"),
    /did not complete cleanly/,
  ],
  [
    "a clock not anchored to the server",
    (file) => (file.report.anchor.source = "first_token_request"),
    /not anchored/,
  ],
  [
    "a run that played tone fixtures",
    (file) => (file.report.validation_fixture = true),
    /tone fixtures/,
  ],
  [
    "a capture that does not say what it played",
    (file) => delete file.report.validation_fixture,
    /tone fixtures/,
  ],
];
for (const [name, edit, pattern] of UNJUDGED) {
  test(`${name} leaves reply_content not_assessed with the reason`, () => {
    const result = merged(edit);
    assert.equal(result.report.reply_content, "not_assessed");
    assert.match(result.report.reply_content_reasons.join(" "), pattern);
  });
}

test("only the verdict fields move, and a pilot stays not evaluated", () => {
  const file = capture();
  const after = mergeVerdict(file, servingReport(), "pilot");
  assert.equal(after.report.qualification, "not_evaluated");
  assert.deepEqual(after.report.qualification_reasons, []);
  assert.equal(after.report.assessment_scope, "structural");
  const VERDICT = [
    "reply_content",
    "reply_content_checkpoints",
    "reply_content_reasons",
    "qualification",
    "qualification_reasons",
    "rotations_completed",
    "rotations_verified",
  ];
  const without = (report) =>
    Object.fromEntries(
      Object.entries(report).filter(([key]) => !VERDICT.includes(key)),
    );
  assert.deepEqual(without(after.report), without(file.report));
  assert.deepEqual(
    Object.keys(after.report).filter((key) => key.startsWith("reply_content")),
    VERDICT.slice(0, 3),
  );
});

test("judging again without a run kind leaves a qualification as it was", () => {
  const file = capture();
  Object.assign(file.report, {
    qualification: "pass",
    qualification_reasons: [],
    assessment_scope: "qualification",
    transmitted: sentAt(BUNDLE),
  });
  const after = mergeVerdict(file, servingReport());
  assert.equal(after.report.reply_content, "failed");
  assert.equal(after.report.qualification, "pass");
  assert.equal(after.report.assessment_scope, "qualification");
  assert.equal("rotations_completed" in after.report, false);
});

test("a passed qualification with an empty or malformed send record does not stay passed", () => {
  for (const transmitted of [
    [],
    "none",
    {},
    [{ started_at_s: 1 }],
    [{ playing_at_s: 1 }],
  ]) {
    const file = capture();
    Object.assign(file.report, {
      qualification: "pass",
      qualification_reasons: [],
      assessment_scope: "qualification",
      transmitted,
    });
    const after = mergeVerdict(file, servingReport());
    assert.equal(after.report.qualification, "inconclusive");
  }
});

test("an older passed qualification with no dispatch evidence does not stay passed", () => {
  const file = capture();
  Object.assign(file.report, {
    qualification: "pass",
    qualification_reasons: [],
    assessment_scope: "qualification",
  });
  assert.equal("transmitted" in file.report, false);
  const after = mergeVerdict(file, servingReport());
  assert.equal(after.report.qualification, "inconclusive");
  assert.match(
    after.report.qualification_reasons.join("\n"),
    /when each clip was sent/,
  );
  // What was not a pass is not moved.
  const failed = capture();
  failed.report.qualification = "fail";
  assert.equal(
    mergeVerdict(failed, servingReport()).report.qualification,
    "fail",
  );
});

test("a qualification run is evaluated and says so", () => {
  const after = mergeVerdict(capture(), servingReport(), "qualification");
  // One checkpoint answered and the rest silent: the content failed.
  assert.equal(after.report.qualification, "fail");
  assert.equal(after.report.assessment_scope, "qualification");
  assert.match(
    after.report.qualification_reasons.join("\n"),
    /checkpoints failed/,
  );
  assert.equal(after.report.exit_code, 0);
});

test("a verified interview carries an empty reasons list, not a stale one", () => {
  const pass = { checkpoint_id: "a", verdict: "pass", reasons: [] };
  const fields = verdictFields(summarize([pass, pass]));
  assert.equal(fields.reply_content, "verified");
  assert.deepEqual(fields.reply_content_reasons, []);
  assert.deepEqual(fields.reply_content_checkpoints, {
    pass: 2,
    fail: 0,
    not_assessed: 0,
  });
});

test("a missing fixture report is named, and other flows are left alone", () => {
  const file = capture();
  const result = mergeVerdict(file, null);
  assert.equal(result.report.reply_content, "not_assessed");
  assert.match(result.report.reply_content_reasons[0], /left no report/);
  assert.equal(mergeVerdict({ ...file, flow: "soak" }, null), null);
});

function withFiles(run) {
  const dir = fs.mkdtempSync(`${os.tmpdir()}/evidence-`);
  try {
    fs.writeFileSync(`${dir}/capture.json`, JSON.stringify(capture()));
    fs.writeFileSync(`${dir}/out.json`, JSON.stringify(servingReport()));
    run(dir);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

test("the command rewrites the capture in place and reports one line", () => {
  withFiles((dir) => {
    const lines = [];
    const code = main(
      ["--capture", `${dir}/capture.json`, "--fixture", `${dir}/out.json`],
      { out: (line) => lines.push(line) },
    );
    assert.equal(code, 0);
    assert.match(lines[0], /^long-interview reply_content=failed pass=1 /);
    const after = JSON.parse(fs.readFileSync(`${dir}/capture.json`, "utf8"));
    assert.equal(after.report.reply_content, "failed");
  });
});

// For a qualification the exit is the verdict, and whatever kept the run from
// being judged is inconclusive rather than nothing to report.
test("each qualification verdict leaves with its own exit", () => {
  assert.equal(qualificationExit("pass"), 0);
  assert.equal(qualificationExit("fail"), 1);
  for (const other of ["inconclusive", "not_evaluated", "pending", undefined]) {
    assert.equal(qualificationExit(other), 77, String(other));
  }
});

test("a qualification run kind makes the exit the verdict", () => {
  withFiles((dir) => {
    const run = (kind, captureFile = `${dir}/capture.json`) => {
      const lines = [];
      const code = main(
        [
          "--capture",
          captureFile,
          "--fixture",
          `${dir}/out.json`,
          "--run-kind",
          kind,
        ],
        { out: (line) => lines.push(line), err: () => {} },
      );
      return [code, lines.join("\n")];
    };
    assert.equal(run("pilot")[0], 0);
    assert.equal(run("")[0], 0, "outside a budgeted run");
    const [code, lines] = run("qualification");
    assert.equal(code, 1);
    assert.match(lines, /qualification=fail rotations_completed=0 /);
    assert.match(lines, /long-interview qualification: .*checkpoints failed/);

    // Nothing failed and nothing was shown: no rotation facts at all.
    const quiet = capture();
    quiet.plan.durationMin = 1;
    fs.writeFileSync(`${dir}/quiet.json`, JSON.stringify(quiet));
    const [unknown, why] = run("qualification", `${dir}/quiet.json`);
    assert.equal(unknown, 77);
    assert.match(why, /qualification=inconclusive/);

    assert.equal(run("qualification", `${dir}/no`)[0], 77, "no capture");
    assert.equal(run("qualification", "")[0], 77, "no capture named");
    fs.writeFileSync(`${dir}/soak.json`, JSON.stringify({ flow: "soak" }));
    assert.equal(
      run("qualification", `${dir}/soak.json`)[0],
      77,
      "a capture of another flow",
    );
    assert.equal(run("pilot", `${dir}/no`)[0], 2);

    // A capture judged but not written back was not judged: its `.partial`
    // cannot be written where a directory stands.
    fs.mkdirSync(`${dir}/capture.json.partial`);
    assert.equal(run("qualification")[0], 77, "a capture it cannot write");
    assert.equal(run("pilot")[0], 2);
    assert.equal(run("pilot", `${dir}/soak.json`)[0], 0);
  });
});

// An exception would leave the process with the runtime's own status, 1, which
// is the status of a qualification that failed.
test("a judge that throws is a run nobody judged, never a failed one", () => {
  withFiles((dir) => {
    // A tool result nested deeper than the stack that walks it.
    const deep = servingReport();
    for (const batch of deep.capture.batches) {
      for (const call of batch.calls) call.result = "DEEP";
    }
    const nested = `${"[".repeat(100000)}${"]".repeat(100000)}`;
    fs.writeFileSync(
      `${dir}/out.json`,
      JSON.stringify(deep).replaceAll('"DEEP"', nested),
    );
    const run = (kind) => {
      const said = [];
      const code = main(
        [
          "--capture",
          `${dir}/capture.json`,
          "--fixture",
          `${dir}/out.json`,
          "--run-kind",
          kind,
        ],
        { out: () => {}, err: (line) => said.push(line) },
      );
      return [code, said.join("\n")];
    };
    const [code, said] = run("qualification");
    assert.equal(code, 77);
    assert.match(said, /cannot judge /);
    assert.equal(run("pilot")[0], 2);
    const kept = JSON.parse(fs.readFileSync(`${dir}/capture.json`, "utf8"));
    assert.deepEqual(kept, capture());
  });
});

test("a fixture report that is there but unreadable is not called missing", () => {
  withFiles((dir) => {
    const run = (fixture) => {
      main(["--capture", `${dir}/capture.json`, "--fixture", fixture], {
        out: () => {},
      });
      const after = JSON.parse(fs.readFileSync(`${dir}/capture.json`, "utf8"));
      return after.report.reply_content_reasons[0];
    };
    fs.writeFileSync(`${dir}/cut.json`, '{"outcome": "ok", "secret reply te');
    const reason = run(`${dir}/cut.json`);
    assert.match(reason, /report could not be read: SyntaxError$/);
    assert.doesNotMatch(reason, /secret/);
    assert.match(run(`${dir}/never-written.json`), /left no report/);
  });
});

test("a capture that is there but unreadable exits 2 and is named so", () => {
  withFiles((dir) => {
    fs.writeFileSync(`${dir}/capture.json`, "{");
    const errs = [];
    const code = main(
      ["--capture", `${dir}/capture.json`, "--fixture", `${dir}/out.json`],
      { err: (line) => errs.push(line) },
    );
    assert.equal(code, 2);
    assert.match(errs.join(" "), /SyntaxError/);
    assert.equal(fs.readFileSync(`${dir}/capture.json`, "utf8"), "{");
  });
});

test("the fixture check names a report that is there but unreadable", () => {
  withFiles((dir) => {
    fs.writeFileSync(`${dir}/cut.json`, '{"outcome"');
    const errs = [];
    assert.equal(
      main(["--check-fixture", `${dir}/cut.json`], {
        err: (line) => errs.push(line),
      }),
      1,
    );
    assert.match(errs.join(" "), /could not be read: SyntaxError/);
  });
});

test("a command line or capture it cannot use exits 2 and writes nothing", () => {
  withFiles((dir) => {
    const before = fs.readFileSync(`${dir}/capture.json`, "utf8");
    const err = () => {};
    for (const argv of [
      [],
      ["--capture", `${dir}/capture.json`],
      ["--fixture", `${dir}/out.json`],
      ["--capture", `${dir}/nope`, "--fixture", `${dir}/out.json`],
    ]) {
      assert.equal(main(argv, { err }), 2, argv.join(" "));
    }
    assert.equal(fs.readFileSync(`${dir}/capture.json`, "utf8"), before);
  });
});

test("a serving fixture report is accepted by its shape and its outcome", () => {
  const without = (key) => {
    const copy = servingReport();
    delete copy[key];
    return copy;
  };
  assert.deepEqual(fixtureProblems(servingReport()), []);
  const rows = [
    [null, /not an object/],
    [[], /not an object/],
    [{}, /no outcome/],
    [{ outcome: "shutdown bound" }, /ended with shutdown bound/],
    [without("arming_failure"), /no arming_failure/],
    [without("bound"), /no bound/],
    [{ ...servingReport(), capture: null }, /no capture/],
  ];
  for (const [report, pattern] of rows) {
    assert.match(fixtureProblems(report).join(" "), pattern);
  }
  const lacking = servingReport();
  delete lacking.capture.boundaries;
  delete lacking.capture.failure;
  assert.match(fixtureProblems(lacking).join(" "), /no failure field/);
  assert.match(fixtureProblems(lacking).join(" "), /no boundaries list/);
  // An observer that failed says so itself, and the judge names it.
  const failed = { ...servingReport(), arming_failure: "Scope", capture: null };
  assert.deepEqual(fixtureProblems(failed), []);
  // A latched failure inside a well-formed report is the judge's to name.
  const latched = servingReport();
  latched.capture.failure = "Limit";
  assert.deepEqual(fixtureProblems(latched), []);
});

test("the check command exits 1 on a report it would not accept", () => {
  withFiles((dir) => {
    const err = [];
    const sink = (line) => err.push(line);
    assert.equal(
      main(["--check-fixture", `${dir}/out.json`], { err: sink }),
      0,
    );
    fs.writeFileSync(`${dir}/bad.json`, JSON.stringify({ outcome: "boom" }));
    assert.equal(
      main(["--check-fixture", `${dir}/bad.json`], { err: sink }),
      1,
    );
    assert.match(err.join(" "), /ended with boom/);
    assert.equal(main(["--check-fixture", `${dir}/missing`], { err: sink }), 1);
    assert.equal(main(["--check-fixture"], { err: sink }), 1);
  });
});
