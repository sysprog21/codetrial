const {
  joinEvidenceClock,
  knownReplayTool,
  sameScope,
} = require("./replay-evidence.cjs");
const { number: finite, whole: integer } = require("./replay-clock.cjs");

const BOUNDARIES = new Set([
  "turn_complete",
  "interrupted",
  "held",
  "socket_replaced",
]);
const GATE_STAGES = new Set([...BOUNDARIES, "closing", "close_requested"]);
const unknown = (reason) => ({ status: "unknown", reason });
const excluded = (reason) => ({ status: "excluded", reason });
const ack = (r) => r.family === "latency" && r.stage === "tool_response";
const boundary = (r) =>
  r.family === "gate" || (r.family === "latency" && BOUNDARIES.has(r.stage));
const replacement = (r) =>
  r.family === "replacement" ||
  r.stage === "socket_replaced" ||
  (r.family === "pressure" &&
    (r.stage === "replacement" || r.stage === "discarded"));
const terminal = (r) =>
  (r.family === "pressure" && r.stage === "terminal") ||
  (r.family === "gate" && r.stage === "closing");
const supported = (g) =>
  g &&
  ["hint", "tool"].includes(g.state) &&
  knownReplayTool(g.tool) &&
  (g.state !== "hint" || g.tool === "log_hint");
const eligible = (r, g) =>
  (r.obligation_outcome === null || r.obligation_outcome === "open") &&
  r.sent === true &&
  r.closing === false &&
  r.accepted_tool_names.includes(g.tool) &&
  (g.state !== "hint" || r.hint_given === true);
const invalidating = (r) => boundary(r) || replacement(r) || terminal(r);
const incomplete = (r) =>
  (ack(r) && ![null, "open"].includes(r.obligation_outcome)) ||
  !r.usable ||
  !known(r) ||
  (ack(r) && typeof r.closing !== "boolean");
const known = (r) =>
  r.family === "replacement" ||
  r.family === "pressure" ||
  (r.family === "gate" && GATE_STAGES.has(r.stage)) ||
  (r.family === "latency" &&
    (BOUNDARIES.has(r.stage) ||
      ["tool_response", "audio_accepted"].includes(r.stage)));

// The room came from the join token. A known child pid is stronger than picking
// a matching event from a shared server log; otherwise the context must be unique.
function pinGateSession({ evidence, roomName, expectedPid }) {
  if (
    !evidence ||
    !Array.isArray(evidence.records) ||
    typeof roomName !== "string" ||
    (expectedPid !== undefined && !integer(expectedPid))
  )
    return unknown("invalid-context-selector");
  const contexts = new Map();
  for (const record of evidence.records) {
    if (
      record.family === "floor" ||
      record.room !== roomName ||
      (expectedPid !== undefined && record.pid !== expectedPid)
    )
      continue;
    contexts.set(JSON.stringify([record.pid, record.session]), {
      pid: record.pid,
      room: record.room,
      session: record.session,
    });
  }
  if (contexts.size !== 1)
    return unknown(contexts.size ? "ambiguous-context" : "missing-context");
  return { status: "pinned", selected: [...contexts.values()][0] };
}

function selectToolGate({ evidence, roomName, expectedPid, gate }) {
  if (!supported(gate))
    return { status: "unsupported_gate", reason: "unsupported-tool-context" };
  const pinned = pinGateSession({ evidence, roomName, expectedPid });
  if (pinned.status !== "pinned") return pinned;
  const events = evidence.records.filter(
    (r) => sameScope(r, pinned.selected) && r.family !== "floor",
  );
  if (
    events.some(
      (r) =>
        incomplete(r) ||
        !integer(r.interview_ms) ||
        (ack(r) && !/^[a-f0-9]{64}$/.test(r.evidence_sha256 ?? "")),
    )
  )
    return unknown("incomplete-context-evidence");
  events.sort((a, b) => a.interview_ms - b.interview_ms);
  let candidate;
  let invalidUntil = -1;
  let reason = "no-accepted-tool";
  for (let i = 0; i < events.length;) {
    let until = i + 1;
    while (
      until < events.length &&
      events[until].interview_ms === events[i].interview_ms
    )
      until++;
    const group = events.slice(i, until);
    i = until;
    const acks = group.filter(ack);
    const invalidators = group.filter(invalidating);
    const invalid = invalidators.length > 0;
    if (invalid)
      invalidUntil = Math.max(
        invalidUntil,
        ...invalidators.map((r) => r.event_end_ms ?? r.interview_ms),
      );
    if (acks.length) {
      if (
        acks.length !== 1 ||
        invalid ||
        acks[0].interview_ms <= invalidUntil
      ) {
        candidate = undefined;
        reason = "ambiguous-context-order";
        continue;
      }
      candidate = eligible(acks[0], gate) ? acks[0] : undefined;
      reason = acks[0].closing ? "closing-context" : "no-accepted-tool";
    } else if (invalid) {
      candidate = undefined;
      reason = "context-boundary";
    }
    if (
      candidate &&
      group.some(
        (r) =>
          r.family === "latency" &&
          r.stage === "audio_accepted" &&
          (r.turn !== candidate.turn || r.socket !== candidate.socket),
      )
    ) {
      candidate = undefined;
      reason = "different-output-context";
    }
  }
  if (!candidate) return { status: "unavailable", reason };
  return {
    status: "candidate",
    selected: { ...pinned.selected, socket: candidate.socket },
    id: candidate.evidence_sha256,
    gate: structuredClone(gate),
    event: structuredClone(candidate),
  };
}

function provisionalAge({ candidate, harnessSample }) {
  const maxAgeMs = candidate?.gate?.max_age_ms;
  if (
    candidate?.status !== "candidate" ||
    typeof candidate.id !== "string" ||
    candidate.event?.evidence_sha256 !== candidate.id
  )
    return unknown("candidate-missing");
  const c = candidate.event.clock;
  if (
    !c ||
    ![
      c.start_unix_ms,
      c.emitted_ms,
      c.emitted_unix_ms,
      candidate.event.interview_ms,
    ].every(integer) ||
    !integer(c.clock_uncertainty_ms) ||
    c.clock_uncertainty_ms < 2 ||
    c.clock_uncertainty_ms > 5000 ||
    candidate.event.interview_ms > c.emitted_ms ||
    !integer(harnessSample?.unix_ms) ||
    !finite(harnessSample?.mono_ms) ||
    harnessSample.mono_ms < 0 ||
    typeof harnessSample?.clock_id !== "string" ||
    !finite(harnessSample?.uncertainty_ms) ||
    harnessSample.uncertainty_ms < 1 ||
    harnessSample.uncertainty_ms > 5000 ||
    !integer(maxAgeMs) ||
    maxAgeMs < 1 ||
    maxAgeMs > 5000
  )
    return unknown("invalid-clock-sample");
  const base = c.emitted_unix_ms - c.emitted_ms + candidate.event.interview_ms;
  const low = base - c.clock_uncertainty_ms;
  const high = base + 1 + c.clock_uncertainty_ms;
  const upper = harnessSample.unix_ms + harnessSample.uncertainty_ms - low;
  const lower = harnessSample.unix_ms - harnessSample.uncertainty_ms - high;
  if (![low, high, upper, lower].every(finite) || lower <= 0)
    return unknown("ambiguous-clock-order");
  const age = {
    provisional: true,
    age_upper_ms: upper,
    age_lower_ms: lower,
    clock_id: harnessSample.clock_id,
  };
  if (lower > maxAgeMs)
    return { status: "expired", reason: "stale-gate", ...age };
  if (upper > maxAgeMs)
    return { status: "unknown", reason: "ambiguous-gate-age", ...age };
  return { status: "candidate", ...age };
}

function verifyToolGate({
  evidence,
  roomName,
  expectedPid,
  candidate,
  gate,
  harness,
  sampleSpan,
  injection,
  observationEndMs,
  maxUncertaintyMs,
}) {
  if (!supported(gate))
    return { status: "unsupported_gate", reason: "unsupported-tool-context" };
  if (evidence?.finalized !== true || evidence?.drained !== true)
    return unknown("evidence-not-finalized");
  const pinned = pinGateSession({ evidence, roomName, expectedPid });
  if (pinned.status !== "pinned") return pinned;
  if (
    candidate?.status !== "candidate" ||
    !sameScope(candidate.selected, pinned.selected)
  )
    return unknown("candidate-context-mismatch");
  if (
    ["state", "tool", "max_age_ms"].some(
      (key) => candidate.gate?.[key] !== gate[key],
    )
  )
    return unknown("candidate-gate-mismatch");
  const record = evidence.records.find(
    (r) => r.evidence_sha256 === candidate.id && sameScope(r, pinned.selected),
  );
  if (
    !record ||
    !ack(record) ||
    !eligible(record, gate) ||
    record.socket !== candidate.selected.socket
  )
    return unknown("candidate-missing-or-changed");
  if (
    !injection ||
    !finite(injection.low) ||
    !finite(injection.high) ||
    injection.low > injection.high ||
    !finite(observationEndMs) ||
    observationEndMs <= injection.high ||
    !integer(gate.max_age_ms) ||
    gate.max_age_ms < 1 ||
    gate.max_age_ms > 5000
  )
    return unknown("invalid-injection-bracket");
  try {
    const join = joinEvidenceClock({
      evidence,
      selected: { ...candidate.selected },
      harness,
      sampleSpan,
      maxUncertaintyMs,
    });
    if (injection.clock_id !== join.clock.clock_id)
      return unknown("injection-clock-mismatch");
    const at = join.project(record);
    if (at.high >= injection.low) return unknown("ambiguous-gate-time");
    const age = injection.high - at.low;
    if (injection.low - at.high > gate.max_age_ms)
      return excluded("stale-gate");
    if (age > gate.max_age_ms) return unknown("ambiguous-gate-age");
    const raw = join.clock.providerWindow({
      clock_id: injection.clock_id,
      low: at.low,
      high: observationEndMs,
    });
    for (const event of join.records) {
      if (event.family === "floor") continue;
      if (incomplete(event)) return unknown("incomplete-context-evidence");
      const end = event.event_end_ms ?? event.interview_ms;
      if (
        event.evidence_sha256 === candidate.id ||
        end < record.interview_ms ||
        event.interview_ms > raw.high
      )
        continue;
      if (
        event.interview_ms < record.interview_ms ||
        (event.interview_ms === record.interview_ms &&
          (ack(event) || invalidating(event)))
      )
        return unknown("ambiguous-context-order");
      const first = join.project(event);
      const last = join.project(event, end);
      const when = { low: first.low, high: last.high };
      const sameOutput =
        event.family === "latency" &&
        event.stage === "audio_accepted" &&
        event.turn === record.turn &&
        event.socket === record.socket;
      const otherSocket =
        ["latency", "gate"].includes(event.family) &&
        event.socket !== record.socket;
      const invalidToEnd = replacement(event) || terminal(event) || otherSocket;
      if (invalidToEnd) {
        if (when.high < observationEndMs)
          return excluded("transport-or-session-change");
        if (when.low <= observationEndMs)
          return unknown("ambiguous-observation-boundary");
      }
      const invalidAtInjection =
        ack(event) ||
        boundary(event) ||
        (event.family === "latency" && !sameOutput);
      if (invalidAtInjection) {
        if (when.high < injection.low) return excluded("superseded-gate");
        if (when.low <= injection.high)
          return unknown("ambiguous-injection-order");
      }
    }
    return {
      status: "aligned",
      candidate_id: candidate.id,
      selected: { ...candidate.selected },
      age_upper_ms: age,
      clock_id: injection.clock_id,
    };
  } catch (error) {
    // Evidence and clock errors carry a reason code; anything else is a bug.
    if (typeof error?.reason !== "string") throw error;
    return unknown(error.reason);
  }
}
module.exports = {
  pinGateSession,
  selectToolGate,
  provisionalAge,
  verifyToolGate,
};
