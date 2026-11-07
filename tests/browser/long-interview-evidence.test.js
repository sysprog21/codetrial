import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import { fileURLToPath } from "node:url";

import evidence_ from "../../scripts/long-interview-evidence.cjs";
import sched from "../../scripts/long-interview-schedule.cjs";

const {
  assessCheckpoint,
  fixtureProblems,
  judgeInterview,
  main,
  mergeVerdict,
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
  const result = assess(faithful());
  assert.deepEqual(result, {
    checkpoint_id: CP.checkpoint_id,
    verdict: "pass",
    reasons: [],
  });
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
    "a second reply inside the window",
    (r) => at(r.capture.replies[1].identity, 150),
    /2 replies/,
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

test("only the three verdict fields move", () => {
  const file = capture();
  const after = mergeVerdict(file, servingReport());
  const VERDICT = [
    "reply_content",
    "reply_content_checkpoints",
    "reply_content_reasons",
  ];
  const without = (report) =>
    Object.fromEntries(
      Object.entries(report).filter(([key]) => !VERDICT.includes(key)),
    );
  assert.deepEqual(without(after.report), without(file.report));
  assert.deepEqual(
    Object.keys(after.report).filter((key) => key.startsWith("reply_content")),
    VERDICT,
  );
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
