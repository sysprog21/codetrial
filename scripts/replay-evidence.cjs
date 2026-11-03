// Complete diagnostic artifacts are evidence; arrival order is never event order.
const { sha256 } = require("./long-interview-schedule.cjs");
const { TextDecoder } = require("node:util");
const { readReplayFile, decodeReplayJson } = require("./replay-input.cjs");
const { createClockJoin, whole: integer } = require("./replay-clock.cjs");
const { PREFIXES: CAPTURE_PREFIXES } = require("./floor-capture.cjs");

const FILE_BYTES = 16 * 1024 * 1024;
const PREFIXES = Object.values(CAPTURE_PREFIXES).flat();
const CLOCK_KEYS = [
  "start_unix_ms",
  "emitted_ms",
  "emitted_unix_ms",
  "clock_uncertainty_ms",
];
const TOOLS = new Set([
  "read_editor",
  "read_board",
  "read_conversation",
  "log_hint",
  "record_framework_evidence",
  "end_interview",
]);

function knownReplayTool(name) {
  return TOOLS.has(name);
}

class ReplayEvidenceError extends Error {
  constructor(reason) {
    super(`replay evidence: ${reason}`);
    this.reason = reason;
  }
}
function requireValue(condition, reason) {
  if (!condition) throw new ReplayEvidenceError(reason);
}
function room(n) {
  return typeof n === "string" && /^[a-zA-Z0-9_.:-]{1,128}$/.test(n);
}
function optional(n) {
  return n === null || integer(n);
}
const sameScope = (a, b) =>
  a.pid === b.pid && a.room === b.room && a.session === b.session;
function clockId(record) {
  return `producer:${sha256(
    JSON.stringify([
      record.pid,
      record.room,
      record.session,
      record.clock.start_unix_ms,
    ]),
  )}`;
}

function tokens(text) {
  const result = Object.create(null);
  for (const token of text.split(" ")) {
    const match = /^([a-z][a-z0-9_]*)=(\S+)$/.exec(token);
    requireValue(
      match && !Object.hasOwn(result, match[1]),
      "malformed-or-duplicate-field",
    );
    result[match[1]] = match[2];
  }
  return result;
}
function numeric(value) {
  if (value === undefined || value === "unknown") return null;
  requireValue(
    /^(0|[1-9][0-9]*)$/.test(value) && integer(Number(value)),
    "malformed-number",
  );
  return Number(value);
}
function identity(record) {
  requireValue(
    integer(record.pid) &&
      room(record.room) &&
      integer(record.session) &&
      integer(record.socket),
    "invalid-identity",
  );
}
function clock(value) {
  if (value === null || value === undefined) return null;
  requireValue(
    value && typeof value === "object" && !Array.isArray(value),
    "invalid-clock",
  );
  for (const key of CLOCK_KEYS)
    requireValue(optional(value[key]), "invalid-clock");
  return Object.fromEntries(CLOCK_KEYS.map((key) => [key, value[key]]));
}
function floor(text) {
  const fields = tokens(text);
  const record = {
    family: "floor",
    room: fields.room,
    kind: fields.kind,
    outcome: fields.outcome,
  };
  for (const key of [
    "pid",
    "session",
    "socket",
    "seq",
    "opened_ms",
    "resolved_ms",
    "activity_end_ms",
  ])
    record[key] = numeric(fields[key]);
  requireValue(
    room(record.room) &&
      integer(record.session) &&
      integer(record.socket) &&
      integer(record.seq),
    "invalid-floor-identity",
  );
  const pairs =
    record.kind === "pause"
      ? ["respond", "continue", "unresolved"]
      : record.kind === "overlap"
        ? ["continued", "interrupted", "unresolved"]
        : [];
  requireValue(pairs.includes(record.outcome), "invalid-floor-outcome");
  record.clock = Object.fromEntries(
    CLOCK_KEYS.map((key) => [key, numeric(fields[key])]),
  );
  // Older logs have no numeric opening clocks; they remain visible but unusable.
  record.usable =
    integer(record.pid) &&
    integer(record.opened_ms) &&
    Object.values(record.clock).every(integer);
  if (record.usable) {
    if (record.outcome !== "unresolved")
      requireValue(integer(record.resolved_ms), "missing-floor-resolution");
    else
      requireValue(
        record.resolved_ms === null,
        "unresolved-floor-has-resolution",
      );
    requireValue(
      [record.resolved_ms, record.activity_end_ms].every(
        (time) => time === null || time >= record.opened_ms,
      ),
      "reversed-floor-time",
    );
    requireValue(
      [record.opened_ms, record.resolved_ms, record.activity_end_ms].every(
        (time) => time === null || time <= record.clock.emitted_ms,
      ),
      "floor-time-after-emission",
    );
  }
  return record;
}
function jsonRecord(text, family) {
  const value = decodeReplayJson(Buffer.from(text), "replay evidence");
  requireValue(
    value && typeof value === "object" && !Array.isArray(value),
    "invalid-record",
  );
  identity(value);
  if (family === "latency" && value.event === "socket_pressure") {
    requireValue(
      value.version === 2 && optional(value.interview_ms),
      "invalid-pressure-schema",
    );
    requireValue(
      ["replacement", "terminal", "discarded"].includes(value.end) &&
        [
          "read_idle",
          "closed",
          "failed",
          "credential",
          "receiver_closed",
          "stopped",
        ].includes(value.reader_end),
      "invalid-pressure-outcome",
    );
    const counts = [
      "capacity",
      "occupancy_high_water",
      "blocked_sends",
      "blocked_ms_total",
      "blocked_ms_max",
      "received",
      "consumed",
    ];
    for (const key of counts)
      requireValue(integer(value[key]), "invalid-pressure-count");
    return {
      family: "pressure",
      pid: value.pid,
      room: value.room,
      session: value.session,
      socket: value.socket,
      interview_ms: value.interview_ms,
      stage: value.end,
      reader_end: value.reader_end,
      usable: integer(value.interview_ms),
    };
  }
  requireValue(
    value.event ===
      (family === "latency" ? "reply_latency" : "gate_boundary") &&
      value.version === (family === "latency" ? 2 : 1),
    "unsupported-record-schema",
  );
  requireValue(
    typeof value.stage === "string" &&
      /^[a-z_]{1,64}$/.test(value.stage) &&
      optional(value.interview_ms),
    "invalid-event-time-or-stage",
  );
  requireValue(optional(value.turn), "invalid-turn");
  const record = {
    family,
    pid: value.pid,
    room: value.room,
    session: value.session,
    socket: value.socket,
    turn: value.turn,
    stage: value.stage,
    interview_ms: value.interview_ms,
    clock: clock(value.clock),
  };
  record.usable =
    integer(record.interview_ms) &&
    record.clock !== null &&
    Object.values(record.clock).every(integer);
  if (record.usable)
    requireValue(
      record.interview_ms <= record.clock.emitted_ms,
      "event-after-emission",
    );
  if (family === "gate") {
    record.event_end_ms =
      value.event_end_ms === undefined
        ? record.interview_ms
        : value.event_end_ms;
    requireValue(optional(record.event_end_ms), "invalid-event-interval");
    if (record.usable)
      requireValue(
        integer(record.event_end_ms) &&
          record.event_end_ms >= record.interview_ms &&
          record.event_end_ms <= record.clock.emitted_ms,
        "invalid-event-interval",
      );
  }
  if (family === "latency") {
    requireValue(integer(value.origin_socket), "invalid-origin-socket");
    record.origin_socket = value.origin_socket;
    requireValue(
      value.obligation_outcome === null ||
        ["open", "answered", "superseded", "ended"].includes(
          value.obligation_outcome,
        ),
      "invalid-obligation-outcome",
    );
    record.obligation_outcome = value.obligation_outcome;
    if (value.stage === "tool_response") {
      requireValue(
        typeof value.sent === "boolean" &&
          typeof value.hint_given === "boolean" &&
          Array.isArray(value.accepted_tool_names) &&
          value.accepted_tool_names.length <= 64 &&
          value.accepted_tool_names.every(knownReplayTool),
        "invalid-tool-context",
      );
      record.sent = value.sent;
      record.hint_given = value.hint_given;
      requireValue(
        value.closing === undefined || typeof value.closing === "boolean",
        "invalid-closing-context",
      );
      record.closing = value.closing ?? null;
      record.accepted_tool_names = [...value.accepted_tool_names];
    }
  }
  return record;
}
function parseStream(bytes, family) {
  requireValue(
    Buffer.isBuffer(bytes) && bytes.length <= FILE_BYTES,
    "invalid-stream-size",
  );
  let text;
  try {
    text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(
      bytes,
    );
  } catch {
    throw new ReplayEvidenceError("invalid-utf8");
  }
  requireValue(text.length === 0 || text.endsWith("\n"), "unterminated-stream");
  const records = [];
  const seen = new Set();
  for (const line of text.split("\n").slice(0, -1)) {
    requireValue(
      Buffer.byteLength(line) + 1 <= 4096 && !line.includes("\r"),
      "invalid-line-framing",
    );
    requireValue(
      !line.includes("codetrial capture_incomplete"),
      "capture-incomplete",
    );
    const prefixes = PREFIXES.filter((prefix) => line.includes(prefix));
    if (prefixes.length === 0) continue;
    requireValue(
      prefixes.length === 1 && line.startsWith(prefixes[0]),
      "unframed-diagnostic",
    );
    const prefix = prefixes[0];
    const body = line.slice(prefix.length);
    let record;
    if (prefix === "codetrial floor ") {
      requireValue(family === "floor", "mixed-diagnostic-family");
      record = floor(body);
    } else {
      requireValue(family === "latency", "mixed-diagnostic-family");
      if (
        prefix === "codetrial latency " ||
        prefix === "codetrial replay_gate "
      )
        record = jsonRecord(
          body,
          prefix === "codetrial latency " ? "latency" : "gate",
        );
      else if (
        prefix === "replacement start: " ||
        prefix === "replacement end: "
      ) {
        const fields = tokens(body);
        record = {
          family: "replacement",
          room: fields.room,
          pid: numeric(fields.pid),
          session: numeric(fields.session),
          interview_ms: numeric(fields.interview_ms),
          stage: prefix === "replacement start: " ? "start" : "end",
        };
        requireValue(
          integer(record.pid) && room(record.room) && integer(record.session),
          "invalid-replacement-identity",
        );
        record.usable = integer(record.interview_ms);
      } else continue;
    }
    const key =
      record.family === "pressure"
        ? JSON.stringify([
            "pressure",
            record.pid,
            record.room,
            record.session,
            record.socket,
          ])
        : record.family === "floor"
          ? JSON.stringify([
              record.pid,
              record.room,
              record.session,
              record.seq,
            ])
          : line;
    requireValue(!seen.has(key), "duplicate-evidence");
    seen.add(key);
    record.evidence_sha256 = sha256(line + "\n");
    records.push(record);
  }
  return records;
}

function readEvidenceSnapshot({ floorPath, latencyPath }) {
  const records = [
    ...parseStream(
      readReplayFile(floorPath, FILE_BYTES, "replay evidence"),
      "floor",
    ),
    ...parseStream(
      readReplayFile(latencyPath, FILE_BYTES, "replay evidence"),
      "latency",
    ),
  ];
  return { finalized: false, drained: false, records };
}

function loadEvidence({ floorPath, latencyPath, finalized, drained }) {
  requireValue(
    finalized === true && drained === true,
    "evidence-not-finalized",
  );
  return {
    ...readEvidenceSnapshot({ floorPath, latencyPath }),
    finalized: true,
    drained: true,
  };
}

function joinEvidenceClock({
  evidence,
  selected,
  harness,
  window,
  sampleSpan,
  maxUncertaintyMs,
}) {
  requireValue(
    evidence?.finalized === true && evidence?.drained === true,
    "evidence-not-finalized",
  );
  identity(selected);
  requireValue(
    !window ||
      (Number.isFinite(window.low) &&
        Number.isFinite(window.high) &&
        window.low < window.high &&
        Array.isArray(harness) &&
        harness.every((sample) => sample.clock_id === window.clock_id)),
    "invalid-trial-window",
  );
  requireValue(
    sampleSpan &&
      integer(sampleSpan.from_ms) &&
      integer(sampleSpan.through_ms) &&
      sampleSpan.from_ms < sampleSpan.through_ms,
    "invalid-sample-span",
  );
  const relevant = evidence.records.filter(
    (record) =>
      sameScope(record, selected) ||
      (record.family === "floor" &&
        record.pid === null &&
        record.room === selected.room),
  );
  const samples = relevant
    .filter(
      (record) =>
        record.usable &&
        record.clock &&
        record.clock.emitted_ms >= sampleSpan.from_ms &&
        record.clock.emitted_ms <= sampleSpan.through_ms,
    )
    .map((record) => ({ ...record.clock, clock_id: clockId(record) }));
  const joined = createClockJoin({
    harness,
    provider: samples,
    maxUncertaintyMs,
  });
  const rawWindow = window ? joined.providerWindow(window) : null;
  return {
    records: relevant,
    clock: joined,
    rawWindow,
    project(record, at = record.interview_ms) {
      requireValue(sameScope(record, selected), "event-scope-mismatch");
      return joined.project(
        at,
        record.clock ? clockId(record) : joined.provider_clock_id,
      );
    },
  };
}

function partitionFloor({
  evidence,
  selected,
  harness,
  window,
  sampleSpan,
  maxUncertaintyMs,
}) {
  requireValue(window, "invalid-trial-window");
  const {
    records: relevant,
    clock: joined,
    rawWindow,
    project,
  } = joinEvidenceClock({
    evidence,
    selected,
    harness,
    window,
    sampleSpan,
    maxUncertaintyMs,
  });
  const partitions = {
    definitely_in: [],
    ambiguous: [],
    definitely_out: [],
    preexisting_open: [],
    unusable: [],
    invalidations: [],
  };
  for (const record of relevant) {
    if (!record.usable) {
      partitions.unusable.push(record);
      continue;
    }
    if (record.family !== "floor") {
      // A gate observation spans the time its boundary took effect, which can
      // be later than the provider event that queued it.
      const eventEnd = record.event_end_ms ?? record.interview_ms;
      if (eventEnd < rawWindow.low || record.interview_ms > rawWindow.high)
        continue;
      // Auxiliary event offsets use the same anchor as this validated
      // process/room/session; the producer does not include a clock pair there.
      const at = project(record);
      const last = project(record, eventEnd);
      if (
        last.high >= window.low &&
        at.low <= window.high &&
        (record.family === "replacement" ||
          (record.family === "pressure" &&
            ["replacement", "terminal"].includes(record.stage)) ||
          (["latency", "gate"].includes(record.family) &&
            record.socket !== selected.socket) ||
          record.stage === "socket_replaced")
      )
        partitions.invalidations.push(record);
      continue;
    }
    const end =
      record.kind === "overlap" ? record.activity_end_ms : record.resolved_ms;
    if (record.opened_ms < rawWindow.low || record.opened_ms > rawWindow.high) {
      partitions.definitely_out.push(record);
      if (
        record.opened_ms < rawWindow.low &&
        (end === null || end >= rawWindow.low)
      )
        partitions.preexisting_open.push(record);
      continue;
    }
    const opening = project(record, record.opened_ms);
    if (
      record.socket !== selected.socket &&
      opening.high >= window.low &&
      opening.low <= window.high
    )
      partitions.invalidations.push(record);
    const selectedRecord = { ...record, opening };
    if (opening.low <= window.low && (end === null || end >= rawWindow.low))
      partitions.preexisting_open.push(selectedRecord);
    if (opening.high < window.low || opening.low > window.high)
      partitions.definitely_out.push(selectedRecord);
    else if (opening.low > window.low && opening.high < window.high)
      partitions.definitely_in.push(selectedRecord);
    else partitions.ambiguous.push(selectedRecord);
  }
  return {
    ...partitions,
    clock: {
      clock_id: joined.clock_id,
      provider_clock_id: joined.provider_clock_id,
      uncertainty_ms: joined.uncertainty_ms,
    },
  };
}

module.exports = {
  sameScope,
  knownReplayTool,
  readEvidenceSnapshot,
  loadEvidence,
  joinEvidenceClock,
  partitionFloor,
  ReplayEvidenceError,
};
