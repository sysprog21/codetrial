import assert from "node:assert/strict";
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
import { test } from "node:test";
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const {
  readEvidenceSnapshot,
  loadEvidence,
} = require("../../scripts/replay-evidence.cjs");
const {
  pinGateSession,
  selectToolGate,
  provisionalAge,
  verifyToolGate,
} = require("../../scripts/replay-gates.cjs");
const identity = { pid: 42, room: "gate-test", session: 7, socket: 2 };
const gate = { state: "hint", tool: "log_hint", max_age_ms: 250 };
const clock = (ms) => ({
  start_unix_ms: 100000,
  emitted_ms: ms,
  emitted_unix_ms: 100000 + ms,
  clock_uncertainty_ms: 2,
});
function reply(ms, extra = {}) {
  return `codetrial latency ${JSON.stringify({ ...identity, event: "reply_latency", version: 2, turn: 1, stage: "tool_response", origin_socket: 1, obligation_outcome: "open", interview_ms: ms, clock: clock(ms), sent: true, hint_given: true, accepted_tool_names: ["log_hint"], closing: false, ...extra })}\n`;
}
function boundary(ms, extra = {}) {
  return `codetrial replay_gate ${JSON.stringify({ ...identity, event: "gate_boundary", version: 1, turn: null, stage: "turn_complete", interview_ms: ms, clock: clock(ms), ...extra })}\n`;
}
function trial(t, prefix = reply(1000)) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "replay-gates-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const input = {
    floorPath: path.join(dir, "floor.log"),
    latencyPath: path.join(dir, "latency.log"),
  };
  fs.writeFileSync(input.floorPath, "");
  fs.writeFileSync(input.latencyPath, boundary(0, { socket: 1 }) + prefix);
  const live = () => readEvidenceSnapshot(input);
  const choose = (extra = {}) =>
    selectToolGate({
      evidence: live(),
      roomName: identity.room,
      gate,
      ...extra,
    });
  const candidate = choose();
  const finish = (more = "") => {
    fs.appendFileSync(input.latencyPath, more + boundary(3000));
    return loadEvidence({ ...input, finalized: true, drained: true });
  };
  const verify = (evidence, extra = {}) =>
    verifyToolGate({
      evidence,
      roomName: identity.room,
      candidate,
      gate,
      harness: [
        {
          clock_id: "node:test",
          mono_ms: 0,
          unix_ms: 100000,
          uncertainty_ms: 1,
        },
        {
          clock_id: "node:test",
          mono_ms: 4000,
          unix_ms: 104000,
          uncertainty_ms: 1,
        },
      ],
      sampleSpan: { from_ms: 0, through_ms: 3000 },
      injection: { clock_id: "node:test", low: 1100, high: 1101 },
      observationEndMs: 2000,
      maxUncertaintyMs: 20,
      ...extra,
    });
  return { input, live, choose, candidate, finish, verify };
}
test("actual live-reader candidate remains provisional until finalized posthoc verification", (t) => {
  const r = trial(t);
  assert.equal(r.live().finalized, false);
  assert.equal(r.candidate.status, "candidate");
  assert.equal(r.candidate.event.origin_socket, 1);
  assert.equal(r.candidate.selected.socket, 2);
  assert.equal(r.verify(r.live()).reason, "evidence-not-finalized");
  assert.equal(r.verify(r.finish()).status, "aligned");
  const age = provisionalAge({
    candidate: r.candidate,
    harnessSample: {
      clock_id: "node:test",
      mono_ms: 1100,
      unix_ms: 101100,
      uncertainty_ms: 1,
    },
    maxAgeMs: 250,
  });
  assert.equal(age.status, "candidate");
  assert.equal(age.age_upper_ms, 103);
  assert.equal(age.provisional, true);
});
test("withheld/exhausted, unsent, closing and legacy unknown closing cannot open hint context", (t) => {
  for (const change of [
    { hint_given: false },
    { sent: false },
    { accepted_tool_names: [] },
    { closing: true },
    { closing: undefined },
  ]) {
    const r = trial(t, reply(1000, change));
    assert.notEqual(r.candidate.status, "candidate");
  }
  const r = trial(t);
  assert.equal(
    r.choose({ gate: { state: "tool", tool: "future_tool" } }).status,
    "unsupported_gate",
  );
});
test("same-turn later accepted or failed batch supersedes the selected event before injection", (t) => {
  for (const sent of [false, true]) {
    const r = trial(t);
    assert.equal(
      r.verify(r.finish(reply(1050, { sent }))).reason,
      "superseded-gate",
    );
  }
  const r = trial(t, reply(1000) + reply(1050));
  assert.equal(r.candidate.event.interview_ms, 1050);
  assert.equal(r.verify(r.finish()).status, "aligned");
});
test("gate-only null-turn boundaries and their optional latency twin invalidate without arrival ordering", (t) => {
  const r = trial(t);
  const data = r.finish(
    boundary(1050) +
      reply(1050, { stage: "turn_complete", obligation_outcome: "answered" }),
  );
  assert.equal(r.verify(data).reason, "superseded-gate");
  const only = trial(t);
  assert.equal(
    only.verify(only.finish(boundary(1050))).reason,
    "superseded-gate",
  );
  const equal = trial(t, reply(1000) + boundary(1000));
  assert.notEqual(equal.candidate.status, "candidate");
});
test("ordinary completion after injection is an outcome while interval overlap is unknown", (t) => {
  const r = trial(t);
  assert.equal(r.verify(r.finish(boundary(1500))).status, "aligned");
  const overlap = trial(t);
  assert.equal(
    overlap.verify(overlap.finish(boundary(1100))).reason,
    "ambiguous-injection-order",
  );
});
test("replacement under the old socket invalidates through observation end, but one before a fresh ack does not", (t) => {
  const r = trial(t);
  assert.equal(
    r.verify(r.finish(boundary(1500, { socket: 1, stage: "socket_replaced" })))
      .reason,
    "transport-or-session-change",
  );
  const before = trial(
    t,
    boundary(500, { socket: 1, stage: "socket_replaced" }) + reply(1000),
  );
  assert.equal(before.candidate.status, "candidate");
  assert.equal(before.verify(before.finish()).status, "aligned");
  const touch = trial(t);
  assert.equal(
    touch.verify(
      touch.finish(boundary(2000, { socket: 1, stage: "socket_replaced" })),
    ).reason,
    "ambiguous-observation-boundary",
  );
});
function pressure(ms, extra = {}) {
  return `codetrial latency ${JSON.stringify({ ...identity, event: "socket_pressure", version: 2, interview_ms: ms, end: "discarded", reader_end: "stopped", capacity: 256, occupancy_high_water: 0, blocked_sends: 0, blocked_ms_total: 0, blocked_ms_max: 0, received: 0, consumed: 0, ...extra })}\n`;
}
test("a discarded socket invalidates through observation end like a replacement", (t) => {
  const r = trial(t);
  assert.equal(
    r.verify(r.finish(pressure(1500, { socket: 1 }))).reason,
    "transport-or-session-change",
  );
});
test("a boundary received before the acknowledgement but handled after it expires the candidate", (t) => {
  const r = trial(
    t,
    boundary(950, {
      stage: "interrupted",
      event_end_ms: 1050,
      clock: clock(1050),
    }) + reply(1000),
  );
  assert.notEqual(r.candidate.status, "candidate");
  assert.equal(r.candidate.reason, "ambiguous-context-order");
});
test("same-turn audio is neutral; different-turn audio before injection expires context", (t) => {
  const r = trial(t, reply(1000) + reply(1050, { stage: "audio_accepted" }));
  assert.equal(r.candidate.status, "candidate");
  assert.equal(r.verify(r.finish()).status, "aligned");
  const other = trial(t);
  assert.equal(
    other.verify(
      other.finish(reply(1050, { stage: "audio_accepted", turn: 2 })),
    ).reason,
    "superseded-gate",
  );
});
test("equal-ms distinct acknowledgements stay ambiguous by exact record identity", (t) => {
  const r = trial(
    t,
    reply(1000, { tool_batches: 1 }) + reply(1000, { tool_batches: 2 }),
  );
  assert.notEqual(r.candidate.status, "candidate");
  const later = trial(t);
  assert.equal(
    later.verify(later.finish(reply(1000, { tool_batches: 2 }))).reason,
    "ambiguous-context-order",
  );
});
test("joined room and unique producer identity are pinned without choosing a favorable session", (t) => {
  const r = trial(t, reply(1000) + reply(1001, { pid: 43 }));
  assert.equal(r.candidate.reason, "ambiguous-context");
  assert.equal(r.choose({ expectedPid: 42 }).status, "candidate");
  assert.equal(
    pinGateSession({ evidence: r.live(), roomName: "wrong-room" }).reason,
    "missing-context",
  );
  assert.equal(r.choose({ expectedPid: 99 }).reason, "missing-context");
});
test("posthoc refuses stale clocks, substituted domains and incomplete context", (t) => {
  const r = trial(t);
  const evidence = r.finish();
  assert.equal(
    r.verify(evidence, { gate: { ...gate, max_age_ms: 100 } }).reason,
    "candidate-gate-mismatch",
  );
  assert.equal(
    r.verify(evidence, {
      injection: { clock_id: "page:other", low: 1100, high: 1101 },
    }).reason,
    "injection-clock-mismatch",
  );
  const unknown = trial(t);
  assert.equal(
    unknown.verify(unknown.finish(reply(1200, { stage: "future" }))).reason,
    "incomplete-context-evidence",
  );
});

test("freshness limits distinguish fresh, definitely stale and age uncertainty", (t) => {
  const r = trial(t);
  const evidence = r.finish();
  assert.equal(
    r.verify(evidence, {
      injection: { clock_id: "node:test", low: 1247, high: 1247 },
    }).status,
    "aligned",
  );
  assert.equal(
    r.verify(evidence, {
      injection: { clock_id: "node:test", low: 1253, high: 1253 },
    }).reason,
    "ambiguous-gate-age",
  );
  assert.equal(
    r.verify(evidence, {
      injection: { clock_id: "node:test", low: 1260, high: 1261 },
    }).reason,
    "stale-gate",
  );
  for (const [unix_ms, status] of [
    [101247, "candidate"],
    [101250, "unknown"],
    [101260, "expired"],
  ])
    assert.equal(
      provisionalAge({
        candidate: r.candidate,
        harnessSample: {
          clock_id: "node:test",
          mono_ms: unix_ms - 100000,
          unix_ms,
          uncertainty_ms: 1,
        },
      }).status,
      status,
    );
  assert.equal(
    r.verify(evidence, { gate: { ...gate, max_age_ms: 100 } }).reason,
    "candidate-gate-mismatch",
  );
});

test("closing without another batch invalidates a selected gate and nonopen obligations never qualify", (t) => {
  const r = trial(t);
  assert.equal(
    r.verify(r.finish(boundary(1050, { stage: "closing" }))).reason,
    "transport-or-session-change",
  );
  for (const obligation_outcome of ["answered", "superseded", "ended"]) {
    const other = trial(t, reply(1000, { obligation_outcome }));
    assert.notEqual(other.candidate.status, "candidate");
  }
});

test("a closing-request reduction interval cannot become an exact late boundary", (t) => {
  const r = trial(t);
  assert.equal(
    r.verify(
      r.finish(
        boundary(1050, {
          stage: "close_requested",
          event_end_ms: 1150,
          clock: clock(1150),
        }),
      ),
    ).reason,
    "ambiguous-injection-order",
  );
  const early = trial(t);
  assert.equal(
    early.verify(
      early.finish(
        boundary(1040, {
          stage: "close_requested",
          event_end_ms: 1060,
          clock: clock(1060),
        }),
      ),
    ).reason,
    "superseded-gate",
  );
  const overlappingAck = trial(
    t,
    boundary(900, {
      stage: "close_requested",
      event_end_ms: 1050,
      clock: clock(1050),
    }) + reply(1000),
  );
  assert.equal(overlappingAck.candidate.reason, "ambiguous-context-order");
});

test("posthoc rejects other sockets, a changed anchor, missing pid and lost sample brackets", (t) => {
  for (const stage of ["tool_response", "audio_accepted"]) {
    const r = trial(t);
    assert.equal(
      r.verify(r.finish(reply(1500, { stage, socket: 3 }))).reason,
      "transport-or-session-change",
    );
  }
  const changed = trial(t);
  assert.equal(
    changed.verify(
      changed.finish(
        boundary(1500, {
          stage: "socket_replaced",
          clock: { ...clock(1500), start_unix_ms: 100001 },
        }),
      ),
    ).reason,
    "invalid-provider-sample",
  );
  const missing = trial(t);
  assert.equal(
    missing.verify(missing.finish(), { expectedPid: 99 }).reason,
    "missing-context",
  );
  const bracket = trial(t);
  assert.equal(
    bracket.verify(bracket.finish(boundary(1500)), {
      sampleSpan: { from_ms: 1200, through_ms: 3000 },
    }).reason,
    "event-outside-provider-samples",
  );
});

test("legacy context anywhere in the pinned capture prevents a false freshness certificate", (t) => {
  const r = trial(t);
  assert.equal(
    r.verify(r.finish(reply(100, { closing: undefined }))).reason,
    "incomplete-context-evidence",
  );
  const unknownTool = trial(t);
  assert.throws(
    () =>
      unknownTool.finish(reply(1200, { accepted_tool_names: ["future_tool"] })),
    /invalid-tool-context/,
  );
});

test("posthoc does not discard a delayed interval starting before the selected acknowledgement", (t) => {
  const r = trial(t);
  const evidence = r.finish(
    boundary(900, {
      stage: "close_requested",
      event_end_ms: 1150,
      clock: clock(1150),
    }),
  );
  assert.equal(r.verify(evidence).reason, "ambiguous-context-order");
  const earlier = trial(t);
  assert.equal(
    earlier.verify(
      earlier.finish(
        boundary(980, {
          stage: "closing",
          event_end_ms: 1003,
          clock: clock(1003),
        }),
      ),
    ).reason,
    "ambiguous-context-order",
  );
});
