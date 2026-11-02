import assert from "node:assert/strict";
import { test } from "node:test";
import { createRequire } from "node:module";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
const require = createRequire(import.meta.url);
const {
  loadEvidence,
  partitionFloor,
} = require("../../scripts/replay-evidence.cjs");

const selected = { pid: 42, room: "test-room", session: 7, socket: 1 };
const clock = (emitted_ms) => ({
  start_unix_ms: 100000,
  emitted_ms,
  emitted_unix_ms: 100000 + emitted_ms,
  clock_uncertainty_ms: 2,
});
function gate(ms, extra = {}) {
  return `codetrial replay_gate ${JSON.stringify({ ...selected, event: "gate_boundary", version: 1, turn: null, stage: "turn_complete", interview_ms: ms, clock: clock(ms), ...extra })}\n`;
}
function floor(extra = {}) {
  const f = {
    ...selected,
    seq: 1,
    kind: "pause",
    outcome: "respond",
    at: "0:02.500",
    gap_ms: 400,
    overlap_ms: "unknown",
    playout_ahead_ms: "unknown",
    suspect: "unknown",
    opened_ms: 1100,
    resolved_ms: 1500,
    activity_end_ms: "unknown",
    ...clock(2500),
    ...extra,
  };
  return `codetrial floor ${Object.entries(f)
    .map(([k, v]) => `${k}=${v}`)
    .join(" ")}\n`;
}
function artifacts(t, floorText = floor(), latencyText = gate(0) + gate(2500)) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "replay-evidence-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const floorPath = path.join(dir, "floor.log");
  const latencyPath = path.join(dir, "latency.log");
  fs.writeFileSync(floorPath, floorText);
  fs.writeFileSync(latencyPath, latencyText);
  const input = { floorPath, latencyPath, finalized: true, drained: true };
  return { input, load: () => loadEvidence(input) };
}
function partition(evidence, extra = {}) {
  return partitionFloor({
    evidence,
    selected,
    harness: [
      { clock_id: "node:test", mono_ms: 0, unix_ms: 100000, uncertainty_ms: 1 },
      {
        clock_id: "node:test",
        mono_ms: 3000,
        unix_ms: 103000,
        uncertainty_ms: 1,
      },
    ],
    window: { clock_id: "node:test", low: 1000, high: 2000 },
    sampleSpan: { from_ms: 0, through_ms: 2500 },
    maxUncertaintyMs: 10,
    ...extra,
  });
}

test("joins actual floor fields by opening time even when resolution emits at teardown", (t) => {
  const evidence = artifacts(t).load();
  const result = partition(evidence);
  assert.equal(result.definitely_in.length, 1);
  assert.deepEqual(result.definitely_in[0].opening, { low: 1097, high: 1104 });
  assert.equal(result.definitely_in[0].resolved_ms, 1500);
  assert.equal(result.definitely_in[0].clock.emitted_ms, 2500);
  assert.equal(
    result.ambiguous.length +
      result.unusable.length +
      result.invalidations.length,
    0,
  );
});
test("late preexisting emission cannot be selected as injected activity", (t) => {
  const result = partition(artifacts(t, floor({ opened_ms: 100 })).load());
  assert.equal(result.definitely_in.length, 0);
  assert.equal(result.definitely_out.length, 1);
  assert.equal(result.preexisting_open.length, 1);
});
test("all potentially matching records are returned without choosing a favorable outcome", (t) => {
  const result = partition(
    artifacts(
      t,
      floor() +
        floor({
          seq: 2,
          opened_ms: 1600,
          resolved_ms: 1700,
          outcome: "continue",
        }),
    ).load(),
  );
  assert.equal(result.definitely_in.length, 2);
  assert.deepEqual(
    result.definitely_in.map((r) => r.outcome),
    ["respond", "continue"],
  );
});
test("endpoint touches and uncertainty are ambiguous", (t) => {
  for (const opened_ms of [996, 1000, 1997, 2003]) {
    const result = partition(
      artifacts(t, floor({ opened_ms, resolved_ms: 2100 })).load(),
    );
    assert.equal(result.definitely_in.length, 0);
    assert.equal(result.ambiguous.length, 1);
  }
});
test("identities and generations never merge", (t) => {
  const other = floor({ pid: 43, seq: 2 });
  const result = partition(artifacts(t, floor() + other).load());
  assert.equal(result.definitely_in.length, 1);
  const mismatch = partition(artifacts(t, floor({ socket: 2 })).load());
  assert.equal(mismatch.invalidations.length, 1);
  const swap = partition(
    artifacts(
      t,
      floor(),
      gate(0) + gate(1200, { stage: "socket_replaced" }) + gate(2500),
    ).load(),
  );
  assert.equal(swap.invalidations.length, 1);
});
test("floor keys survive socket changes and duplicate keys cannot be concealed by them", (t) => {
  assert.throws(
    () => artifacts(t, floor() + floor({ socket: 2 })).load(),
    /duplicate-evidence/,
  );
  assert.throws(
    () => artifacts(t, floor().replace("seq=1", "seq=1 seq=2")).load(),
    /malformed-or-duplicate-field/,
  );
});
test("missing numeric clocks are visible as unusable; malformed numbers and reversed times refuse", (t) => {
  const result = partition(
    artifacts(t, floor({ opened_ms: "unknown" })).load(),
  );
  assert.equal(result.unusable.length, 1);
  for (const [change, reason] of [
    [{ opened_ms: "1.2" }, "malformed-number"],
    [{ resolved_ms: "unknown" }, "missing-floor-resolution"],
    [{ opened_ms: 1600, resolved_ms: 1500 }, "reversed-floor-time"],
    [{ resolved_ms: 2501 }, "floor-time-after-emission"],
    [{ outcome: "unresolved" }, "unresolved-floor-has-resolution"],
    [{ kind: "overlap", outcome: "respond" }, "invalid-floor-outcome"],
  ])
    assert.throws(
      () => artifacts(t, floor(change)).load(),
      new RegExp(`replay evidence: ${reason}$`),
    );
  assert.equal(
    artifacts(
      t,
      floor({ outcome: "unresolved", resolved_ms: "unknown" }),
    ).load().records[0].outcome,
    "unresolved",
  );
});
test("framing, capture gaps and encoding fail before unrelated-line classification", (t) => {
  for (const text of [
    floor().trimEnd(),
    `codetrial capture_incomplete family=future reason=future\n`,
    "x".repeat(4096) + "\n",
    "prefix " + floor(),
    floor().replace("\n", "\r\n"),
  ])
    assert.throws(
      () => artifacts(t, text).load(),
      /unterminated-stream|capture-incomplete|invalid-line-framing|unframed-diagnostic/,
    );
  assert.throws(
    () => artifacts(t, Buffer.from([0xff, 10])).load(),
    /invalid-utf8/,
  );
  assert.equal(
    artifacts(t, "x".repeat(4095) + "\n")
      .load()
      .records.filter((r) => r.family === "floor").length,
    0,
  );
});
test("JSON duplicate/escaped keys, mixed families and duplicated lines refuse", (t) => {
  for (const text of [
    gate(0).replace('"pid":42', '"pid":42,"pid":43'),
    gate(0).replace('"pid":42', '"pid":42,"p\\u0069d":43'),
    gate(0).replace('"pid":42', '"__proto__":{},"pid":42'),
  ])
    assert.throws(
      () => artifacts(t, floor(), text).load(),
      /duplicate object key|forbidden object key/,
    );
  assert.throws(() => artifacts(t, gate(0)).load(), /mixed-diagnostic-family/);
  assert.throws(
    () => artifacts(t, floor(), gate(0) + gate(0)).load(),
    /duplicate-evidence/,
  );
});
test("finalization is explicit and missing files or symlinks cannot become evidence", (t) => {
  const files = artifacts(t);
  files.input.drained = false;
  assert.throws(files.load, /evidence-not-finalized/);
  files.input.drained = true;
  const link = files.input.floorPath + ".link";
  fs.symlinkSync(files.input.floorPath, link);
  files.input.floorPath = link;
  assert.throws(files.load, /symlink/);
  fs.unlinkSync(link);
  assert.throws(files.load, /readable bounded regular file/);
});

function reply(ms, extra = {}) {
  return `codetrial latency ${JSON.stringify({ ...selected, event: "reply_latency", version: 2, turn: 1, stage: "audio_accepted", origin_socket: 1, obligation_outcome: "open", interview_ms: ms, clock: clock(ms), ...extra })}\n`;
}

test("normal reply, tool acknowledgements and pressure summaries share the latency stream", (t) => {
  // Captured from the credential-free Rust recovery test; identity and event times are normalized.
  const pressure = JSON.parse(
    fs.readFileSync(
      new URL(
        "../fixtures/replay-evidence/socket-pressure.json",
        import.meta.url,
      ),
      "utf8",
    ),
  );
  const terminal = { ...pressure[1], socket: 1 };
  const tool = (ms) =>
    reply(ms, {
      stage: "tool_response",
      sent: true,
      hint_given: true,
      accepted_tool_names: ["log_hint"],
      tool_names: ["log_hint"],
      tool_count: 1,
    });
  const data = artifacts(
    t,
    floor(),
    gate(0) +
      tool(900) +
      tool(901) +
      reply(950) +
      gate(2500) +
      `codetrial latency ${JSON.stringify(terminal)}\n`,
  ).load();
  assert.equal(data.records.filter((r) => r.family === "latency").length, 3);
  assert.equal(data.records.filter((r) => r.family === "pressure").length, 1);
  assert.equal(partition(data).definitely_in.length, 1);
  assert.deepEqual(
    data.records.find((r) => r.stage === "tool_response").accepted_tool_names,
    ["log_hint"],
  );
  assert.equal(partition(data).invalidations.length, 0);
  const replacement = `codetrial latency ${JSON.stringify(pressure[0])}\n`;
  assert.equal(
    partition(artifacts(t, floor(), gate(0) + replacement + gate(2500)).load())
      .invalidations.length,
    1,
  );
  const discarded = { ...pressure[0], socket: 999, end: "discarded" };
  assert.equal(
    partition(
      artifacts(
        t,
        floor(),
        gate(0) +
          `codetrial latency ${JSON.stringify(discarded)}\n` +
          gate(2500),
      ).load(),
    ).invalidations.length,
    0,
  );
});

test("invalid auxiliary pressure records refuse instead of disappearing", (t) => {
  const [base] = JSON.parse(
    fs.readFileSync(
      new URL(
        "../fixtures/replay-evidence/socket-pressure.json",
        import.meta.url,
      ),
      "utf8",
    ),
  );
  for (const change of [
    { version: 3 },
    { end: "future" },
    { reader_end: "future" },
    { received: "1" },
    { interview_ms: -1 },
  ]) {
    const line = `codetrial latency ${JSON.stringify({ ...base, ...change })}\n`;
    assert.throws(
      () => artifacts(t, floor(), gate(0) + line).load(),
      /invalid-pressure/,
    );
  }
});

test("a leading UTF-8 BOM cannot hide physical bytes or create a framed diagnostic", (t) => {
  const bare = floor().trimEnd();
  const exact =
    bare +
    " filler=" +
    "x".repeat(4095 - Buffer.byteLength(bare + " filler=")) +
    "\n";
  assert.equal(Buffer.byteLength(exact), 4096);
  assert.equal(
    artifacts(t, exact)
      .load()
      .records.filter((r) => r.family === "floor").length,
    1,
  );
  assert.throws(
    () => artifacts(t, "\ufeff" + exact).load(),
    /invalid-line-framing/,
  );
  assert.throws(
    () => artifacts(t, "\ufeff" + floor()).load(),
    /unframed-diagnostic/,
  );
  assert.throws(
    () => artifacts(t, "é".repeat(2048) + "\n").load(),
    /invalid-line-framing/,
  );
});

test("JSON payload BOMs and conflicting per-socket pressure summaries refuse", (t) => {
  assert.throws(
    () =>
      artifacts(
        t,
        floor(),
        gate(0).replace(
          "codetrial replay_gate ",
          "codetrial replay_gate \ufeff",
        ),
      ).load(),
    /not UTF-8 JSON/,
  );
  const [base] = JSON.parse(
    fs.readFileSync(
      new URL(
        "../fixtures/replay-evidence/socket-pressure.json",
        import.meta.url,
      ),
      "utf8",
    ),
  );
  assert.throws(
    () =>
      artifacts(
        t,
        floor(),
        `codetrial latency ${JSON.stringify(base)}\ncodetrial latency ${JSON.stringify({ ...base, end: "terminal" })}\n`,
      ).load(),
    /duplicate-evidence/,
  );
});
