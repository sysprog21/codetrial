import { test } from "node:test";
import assert from "node:assert/strict";

import qualify_ from "../../scripts/long-interview-qualify.cjs";

const { qualify } = qualify_;

const SESSION = 1_700_000_000_500;
const START = 1_700_000_000_000;
const ORIGIN = START + 2000;

/// A GoAway handover into `socket` at `minutes` on the interview clock.
const swap = (socket, minutes, over = {}) => ({
  rotation: true,
  episode_class: "completed",
  session: SESSION,
  episode: socket - 1,
  replacement: socket - 1,
  old_socket: socket - 1,
  new_socket: socket,
  // The episode began a second and a half before its socket came up.
  started_interview_ms: minutes * 60_000 - 1500,
  interview_ms: minutes * 60_000,
  ...over,
});

/// A checkpoint answered on `socket` that asked for something the candidate
/// had finished saying at `heardS` on the schedule clock.
const answered = (id, socket, heardS = 18, verdict = "pass") => ({
  checkpoint_id: id,
  verdict,
  reasons: verdict === "pass" ? [] : ["reply lacks constraints"],
  reply: { session: SESSION, socket, clock_start_unix_ms: START },
  earliest_fact_heard_s: heardS,
});

const unanswered = (id, why = "a socket replacement lies between") => ({
  checkpoint_id: id,
  verdict: "not_assessed",
  reasons: [why],
});

/// A 90 minute run that qualifies: three handovers, an answer after each that
/// reaches back before it, and one checkpoint nobody could assess.
function good(edit = () => {}) {
  const input = {
    runKind: "qualification",
    report: {
      exit_code: 0,
      status: "complete",
      full_interview: true,
      anchor: { source: "server_clock" },
      validation_fixture: false,
      failure_classes: [],
      decoded_signal: "verified",
      schedule_origin_unix_ms: ORIGIN,
      rotation: {
        verdict: "pass",
        reasons: [],
        episodes_unresolved: 0,
        late_failed_total: 0,
        failed_handovers_total: 0,
        replacements: [swap(2, 9), swap(3, 18), swap(4, 27)],
      },
    },
    checkpoints: [
      answered("c01_cp_state", 1),
      answered("c02_cp_state", 2),
      unanswered("c03_cp_state"),
      answered("c04_cp_state", 3),
      answered("c06_cp_state", 4),
    ],
  };
  edit(input);
  return input;
}

const verdictOf = (edit) => qualify(good(edit));

test("a run with two verified handovers and nothing failed qualifies", () => {
  assert.deepEqual(qualify(good()), {
    qualification: "pass",
    qualification_reasons: [],
    rotations_completed: 3,
    rotations_verified: 3,
  });
});

test("only a qualification run is evaluated", () => {
  for (const runKind of ["pilot", "smoke", "experiment", undefined]) {
    assert.deepEqual(
      verdictOf((i) => {
        i.runKind = runKind;
        i.report.exit_code = 1;
      }),
      {
        qualification: "not_evaluated",
        qualification_reasons: [],
        rotations_completed: 3,
        rotations_verified: 3,
      },
      String(runKind),
    );
  }
});

// Each of these is something the run established about itself.
const FAILS = [
  ["a nonzero exit", (i) => (i.report.exit_code = 1), /exited 1/],
  ["a budget exit", (i) => (i.report.exit_code = 4), /exited 4/],
  ["an unfinished run", (i) => (i.report.status = "failed"), /status/],
  [
    "an interview cut short",
    (i) => (i.report.full_interview = false),
    /full interview/,
  ],
  [
    "a failure class",
    (i) => (i.report.failure_classes = ["logs_incomplete"]),
    /logs_incomplete/,
  ],
  ["tone clips", (i) => (i.report.validation_fixture = true), /tone fixture/],
  [
    "a rotation failure",
    (i) => {
      i.report.rotation.verdict = "fail";
      i.report.rotation.reasons = ["2 rebuilds from local state"];
    },
    /2 rebuilds from local state/,
  ],
  [
    "a rotation that failed after the stimuli stopped",
    (i) => (i.report.rotation.late_failed_total = 1),
    /1 rotation failed after the stimuli stopped/,
  ],
  [
    "a GoAway handover that failed under load",
    (i) => (i.report.rotation.failed_handovers_total = 1),
    /1 GoAway handover failed/,
  ],
  [
    "a decoded signal failure",
    (i) => (i.report.decoded_signal = "failed"),
    /decoded signal failed/,
  ],
  [
    "a teardown that failed after the worker",
    (i) => (i.teardownFailed = true),
    /the run's teardown failed/,
  ],
  [
    "a failed checkpoint",
    (i) => i.checkpoints.push(answered("c07_cp_state", 4, 18, "fail")),
    /1 checkpoint failed: c07_cp_state/,
  ],
];

test("what a run established against itself fails it", () => {
  for (const [name, edit, reason] of FAILS) {
    const result = verdictOf(edit);
    assert.equal(result.qualification, "fail", name);
    assert.match(result.qualification_reasons.join("\n"), reason, name);
  }
});

// Each of these is evidence that is not there, which is never a pass and
// never a failure.
const UNKNOWN = [
  ["no report", (i) => (i.report = null), /no report/],
  ["no exit code", (i) => delete i.report.exit_code, /exit code/],
  ["no status", (i) => delete i.report.status, /completed/],
  ["no span", (i) => delete i.report.full_interview, /full interview/],
  [
    // Not a full interview only because the clock could not be placed on the
    // server's: how long it ran is not known, which is not a short run.
    "a clock not anchored to the server",
    (i) => {
      i.report.full_interview = false;
      i.report.anchor = { source: "first_token_request" };
    },
    /not anchored to the server/,
  ],
  ["no clip kind", (i) => delete i.report.validation_fixture, /clips/],
  [
    "no failure classes",
    (i) => delete i.report.failure_classes,
    /failure classes/,
  ],
  [
    "failure classes that are not a list",
    (i) => (i.report.failure_classes = "none"),
    /failure classes/,
  ],
  ["no rotation facts", (i) => delete i.report.rotation, /rotation/],
  [
    "no GoAway",
    (i) => {
      i.report.rotation.verdict = "inconclusive";
      i.report.rotation.reasons = ["no transport restart was requested"];
    },
    /no transport restart/,
  ],
  [
    "an unresolved episode",
    (i) => (i.report.rotation.episodes_unresolved = 1),
    /1 replacement episode/,
  ],
  [
    "an uncounted episode",
    (i) => delete i.report.rotation.episodes_unresolved,
    /replacement episode/,
  ],
  [
    "failed handovers nobody counted",
    (i) => delete i.report.rotation.failed_handovers_total,
    /failed handovers were not counted/,
  ],
  [
    "late episodes nobody counted",
    (i) => delete i.report.rotation.late_failed_total,
    /late replacement episodes/,
  ],
  [
    "more replacements than a report lists",
    (i) => (i.report.rotation.replacements = null),
    /does not list/,
  ],
  [
    "one handover",
    (i) => (i.report.rotation.replacements = [swap(2, 9)]),
    /1 completed rotation, 1 verified/,
  ],
  [
    "a signal nobody assessed",
    (i) => (i.report.decoded_signal = "not_assessed"),
    /decoded signal/,
  ],
  ["no signal field", (i) => delete i.report.decoded_signal, /decoded signal/],
  ["no checkpoints", (i) => (i.checkpoints = null), /checkpoint/],
  [
    "one verified handover",
    (i) => (i.checkpoints = [answered("c02_cp_state", 2)]),
    /3 completed rotations, 1 verified/,
  ],
  [
    "no schedule origin",
    (i) => delete i.report.schedule_origin_unix_ms,
    /3 completed rotations, 0 verified/,
  ],
];

test("missing evidence is inconclusive, never a pass", () => {
  for (const [name, edit, reason] of UNKNOWN) {
    const result = verdictOf(edit);
    assert.equal(result.qualification, "inconclusive", name);
    assert.match(result.qualification_reasons.join("\n"), reason, name);
  }
});

// A run that saw no GoAway learned nothing about rotation: the worker exits 77
// with its own status and class for it, and none of the three is a failure.
test("a run with no GoAway is inconclusive, as its own exit says", () => {
  const result = verdictOf((i) => {
    i.report.exit_code = 77;
    i.report.status = "inconclusive";
    // The worker reports a full interview only for a run that passed.
    i.report.full_interview = false;
    i.report.failure_classes = ["no_goaway"];
    i.report.rotation = {
      verdict: "inconclusive",
      reasons: ["no transport restart was requested during the soak"],
      episodes_unresolved: 0,
      late_failed_total: 0,
      failed_handovers_total: 0,
      replacements: [],
    };
    i.checkpoints = [answered("c01_cp_state", 1)];
  });
  assert.equal(result.qualification, "inconclusive");
  assert.match(result.qualification_reasons.join("\n"), /saw no GoAway/);
  // Beside a real failure class it is still a failure.
  assert.equal(
    verdictOf(
      (i) => (i.report.failure_classes = ["no_goaway", "logs_incomplete"]),
    ).qualification,
    "fail",
  );
});

// Logs that were cut short or evicted leave the run unjudgeable, not failed:
// the exit and status say so (6, incomplete_artifact), and the capture must too.
for (const name of ["truncated_logs", "evicted_logs"]) {
  test(`${name} is inconclusive, not a failure`, () => {
    const result = verdictOf((i) => {
      i.report.exit_code = 6;
      i.report.status = "incomplete_artifact";
      i.report.failure_classes = [name];
    });
    assert.equal(result.qualification, "inconclusive");
    assert.match(result.qualification_reasons.join("\n"), /logs/);
    // Beside a failure that was established it is still a failure.
    assert.equal(
      verdictOf((i) => {
        i.report.exit_code = 6;
        i.report.status = "incomplete_artifact";
        i.report.failure_classes = [name, "token_expiry"];
      }).qualification,
      "fail",
    );
  });
}

// With the logs gone the rotation judge has nothing to read, so its fail and
// its late count are as unknown as the logs: inconclusive, not a failure.
test("a rotation verdict read from cut logs is not held against the run", () => {
  for (const cut of ["truncated_logs", "evicted_logs"]) {
    const result = verdictOf((i) => {
      i.report.exit_code = 6;
      i.report.status = "incomplete_artifact";
      i.report.failure_classes = [cut];
      i.report.rotation = {
        verdict: "fail",
        reasons: ["no replacement lines"],
        episodes_unresolved: 0,
        late_failed_total: 1,
        replacements: [],
      };
    });
    assert.equal(result.qualification, "inconclusive", cut);
  }
  // The same rotation block with intact logs is a failure.
  const intact = verdictOf((i) => {
    i.report.rotation = {
      verdict: "fail",
      reasons: ["no replacement lines"],
      episodes_unresolved: 0,
      late_failed_total: 1,
      replacements: [],
    };
  });
  assert.equal(intact.qualification, "fail");
});

// An archive that was cut says nothing about how long the interview ran, but
// an observation that ended short is established whatever else went wrong.
test("a short interview stays failed beside an incomplete artifact", () => {
  const cut = (observationEndS) =>
    verdictOf((i) => {
      i.report.exit_code = 6;
      i.report.status = "incomplete_artifact";
      i.report.failure_classes = ["truncated_logs"];
      i.report.full_interview = false;
      i.report.duration_min = 90;
      i.report.timestamps = { observationEndS };
    });
  const short = cut(3000);
  assert.equal(short.qualification, "fail");
  assert.match(short.qualification_reasons.join("\n"), /not a full interview/);
  // Without an observation time the length is as unknown as the logs.
  assert.equal(cut(null).qualification, "inconclusive");
  // One that reached the duration is not short.
  assert.equal(cut(5400).qualification, "inconclusive");
});

test("a failure outranks missing evidence and both are reported", () => {
  const result = verdictOf((i) => {
    i.report.exit_code = 4;
    i.report.decoded_signal = "not_assessed";
  });
  assert.equal(result.qualification, "fail");
  assert.match(result.qualification_reasons.join("\n"), /exited 4/);
  assert.match(result.qualification_reasons.join("\n"), /decoded signal/);
});

test("a signal no swap owed is not held against the run", () => {
  assert.equal(
    verdictOf((i) => (i.report.decoded_signal = "not_required")).qualification,
    "pass",
  );
});

test("unassessed checkpoints block nothing but their own claim", () => {
  const result = verdictOf((i) => {
    for (let n = 10; n < 30; n += 1) i.checkpoints.push(unanswered(`x${n}`));
  });
  assert.equal(result.qualification, "pass");
});

// What makes a replacement a rotation, one condition at a time.
const NOT_ROTATIONS = [
  ["one the soak judge did not call a rotation", { rotation: false }],
  ["one with no rotation flag", { rotation: undefined }],
  ["one begun after the stimuli stopped", { episode_class: "late_idle" }],
  ["one whose episode was not classed", { episode_class: null }],
  ["one with no socket", { new_socket: null }],
  ["one that opened no new socket", { old_socket: 9 }],
  [
    "one that says it reopened its own socket",
    { old_socket: 2, new_socket: 2 },
  ],
  ["one with no clock", { interview_ms: null }],
  ["one whose start is not on record", { started_interview_ms: null }],
  ["one that came up before it began", { started_interview_ms: 9e9 }],
  ["one with no session", { session: null }],
];

test("only a rotation completed under load, placed on a socket and a clock, counts", () => {
  for (const [name, over] of NOT_ROTATIONS) {
    const result = verdictOf((i) => {
      i.report.rotation.replacements = [
        swap(2, 9, over),
        swap(3, 18, over),
        swap(4, 27),
      ];
    });
    assert.equal(result.rotations_completed, 1, name);
    assert.equal(result.qualification, "inconclusive", name);
  }
});

test("a replacement that went back to an older socket is not a rotation", () => {
  // Alone in its claims, so only the socket ordering can exclude it.
  const result = verdictOf((i) => {
    i.report.rotation.replacements = [
      swap(2, 9),
      swap(3, 18, { old_socket: 4 }),
      swap(4, 27),
    ];
  });
  assert.equal(result.rotations_completed, 2);
});

test("a handover is verified only by an answer that reaches back across it", () => {
  const verified = (checkpoints, replacements) =>
    verdictOf((i) => {
      i.checkpoints = checkpoints;
      if (replacements) i.report.rotation.replacements = replacements;
    }).rotations_verified;
  // The handover into socket 2 began at 8:58.5 on the interview clock, which
  // itself began 2 s before the schedule did: 536.5 s on the schedule's clock.
  const opened = 536.5;
  assert.equal(verified([answered("a", 2, opened)]), 1, "said just before");
  assert.equal(
    verified([answered("a", 2, opened + 1)]),
    0,
    "still being said when the handover began",
  );
  assert.equal(verified([answered("a", 2, opened + 60)]), 0, "said after it");
  assert.equal(
    verified([answered("a", 2, 18, "fail")]),
    0,
    "a failed answer verifies nothing",
  );
  assert.equal(
    verified([answered("a", 2), answered("b", 2), answered("c", 2)]),
    1,
    "three answers on one socket verify one handover",
  );
  assert.equal(
    verified([answered("a", 5), answered("b", 1)]),
    0,
    "an answer on a socket no rotation opened",
  );
  const elsewhere = answered("a", 2);
  elsewhere.reply.session = SESSION + 1;
  assert.equal(verified([elsewhere]), 0, "another session's socket 2");
  const unplaced = answered("a", 2);
  delete unplaced.reply.clock_start_unix_ms;
  assert.equal(verified([unplaced]), 0, "no clock to place the swap on");
  const unseeded = answered("a", 2);
  unseeded.earliest_fact_heard_s = null;
  assert.equal(verified([unseeded]), 0, "no record of when it was said");
  assert.equal(
    verified([answered("a", 2)], [swap(2, 9), swap(2, 9), swap(3, 18)]),
    1,
    "the same replacement listed twice is one rotation",
  );
});

test("two rows that claim one socket or one replacement are no rotation", () => {
  const counted = (replacements, checkpoints) =>
    verdictOf((i) => {
      i.report.rotation.replacements = replacements;
      if (checkpoints) i.checkpoints = checkpoints;
    });
  // One real handover listed under a second episode id: one answer on its
  // socket must not verify two rotations.
  const twice = counted(
    [swap(2, 9), swap(2, 9, { episode: 7 })],
    [answered("a", 2), answered("b", 2)],
  );
  assert.equal(twice.rotations_completed, 0);
  assert.equal(twice.qualification, "inconclusive");
  // The same replacement id on two sockets, and two replacements of one
  // episode, contradict each other the same way.
  assert.equal(
    counted([swap(2, 9), swap(3, 18, { replacement: 1 }), swap(4, 27)])
      .rotations_completed,
    1,
  );
  assert.equal(
    counted([swap(2, 9), swap(3, 18, { episode: 1 }), swap(4, 27)])
      .rotations_completed,
    1,
  );
  // A rebuild that is no rotation still contradicts the rotation whose socket
  // it says it opened.
  assert.equal(
    counted([
      swap(2, 9),
      swap(2, 12, {
        trigger: "watchdog",
        rotation: false,
        episode: 8,
        replacement: 8,
      }),
      swap(3, 18),
    ]).rotations_completed,
    1,
  );
  // A socket is replaced once: two rows that both say they replaced socket 2
  // contradict each other, whatever they say they opened.
  assert.equal(
    counted([
      swap(3, 9, { old_socket: 2 }),
      swap(4, 18, { old_socket: 2 }),
      swap(5, 27),
    ]).rotations_completed,
    1,
  );
  // The same row twice is one rotation, not a contradiction.
  assert.equal(
    counted([swap(2, 9), swap(2, 9), swap(3, 18)]).rotations_completed,
    2,
  );
});

test("checkpoint verdicts this rule cannot read are no verdicts", () => {
  const withRow = (row) => verdictOf((i) => i.checkpoints.push(row));
  for (const [name, row] of [
    ["a row with no verdict", { checkpoint_id: "x" }],
    ["an unknown verdict", { checkpoint_id: "x", verdict: "passed" }],
    ["a row with no id", { verdict: "pass" }],
    ["an empty row", null],
    ["a checkpoint named twice", answered("c01_cp_state", 1)],
  ]) {
    const result = withRow(row);
    assert.equal(result.qualification, "inconclusive", name);
    assert.equal(result.rotations_verified, 0, name);
    assert.match(result.qualification_reasons.join("\n"), /cannot be read/);
  }
});

test("a malformed number is missing evidence, not zero", () => {
  for (const bad of [NaN, "3", -1, 1.5, undefined]) {
    const result = verdictOf((i) => {
      i.report.rotation.episodes_unresolved = bad;
    });
    assert.equal(result.qualification, "inconclusive", String(bad));
  }
  assert.equal(
    verdictOf((i) => (i.report.rotation.replacements = "none")).qualification,
    "inconclusive",
  );
});
