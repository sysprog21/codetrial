// Pure helpers shared by the interview page. Kept free of DOM and LiveKit
// access so `web/tests/lib.test.js` can exercise them under `node --test`.

const HTML_ESCAPES = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  "\"": "&quot;",
  "'": "&#39;",
};

export function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (char) => HTML_ESCAPES[char]);
}

export function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

export function formatTime(seconds) {
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  return `${String(minutes).padStart(2, "0")}:${String(rest).padStart(2, "0")}`;
}

export function orPlaceholder(items) {
  return items.length ? items : ["(none captured)"];
}

export function normalize(value) {
  return value === undefined ? null : value;
}

export function deepEqual(left, right) {
  if (Object.is(left, right)) return true;
  if (Array.isArray(left) && Array.isArray(right)) return left.length === right.length && left.every((value, index) => deepEqual(normalize(value), normalize(right[index])));
  return false;
}

function closeNumber(left, right) {
  return typeof left === "number" && typeof right === "number" && Math.abs(left - right) <= 1e-5;
}

function sortedValues(value) {
  return Array.isArray(value) ? [...value].sort((left, right) => String(left).localeCompare(String(right), undefined, { numeric: true })) : value;
}

function sortedTriplets(value) {
  if (!Array.isArray(value)) return null;
  const triplets = [];
  for (const item of value) {
    if (!Array.isArray(item) || item.length !== 3 || !item.every(Number.isInteger)) return null;
    triplets.push([...item].sort((left, right) => left - right));
  }
  return triplets.sort((left, right) => left[0] - right[0] || left[1] - right[1] || left[2] - right[2]);
}

function sortedIntegerRows(value, sortInner) {
  if (!Array.isArray(value)) return null;
  const rows = [];
  for (const item of value) {
    if (!Array.isArray(item) || !item.every(Number.isInteger)) return null;
    rows.push(sortInner ? [...item].sort((left, right) => left - right) : [...item]);
  }
  return rows.sort((left, right) => JSON.stringify(left).localeCompare(JSON.stringify(right), undefined, { numeric: true }));
}

function sortedAnagramGroups(value) {
  if (!Array.isArray(value)) return null;
  const groups = [];
  for (const item of value) {
    if (!Array.isArray(item) || !item.every((word) => typeof word === "string")) return null;
    groups.push([...item].sort());
  }
  return groups.sort((left, right) => left.join("\0").localeCompare(right.join("\0")));
}

function treeFromLevelOrder(values) {
  if (!Array.isArray(values) || !values.length || values[0] === null) return null;
  const root = { val: values[0], left: null, right: null };
  const queue = [root];
  let index = 1;
  for (const node of queue) {
    if (index < values.length && values[index] !== null) {
      node.left = { val: values[index], left: null, right: null };
      queue.push(node.left);
    }
    index++;
    if (index < values.length && values[index] !== null) {
      node.right = { val: values[index], left: null, right: null };
      queue.push(node.right);
    }
    index++;
  }
  return root;
}

function isBalancedBst(testCase, actual) {
  const expected = testCase.input[0];
  const root = treeFromLevelOrder(actual);
  const values = [];
  function height(node) {
    if (!node) return 0;
    const left = height(node.left);
    const right = height(node.right);
    if (left < 0 || right < 0 || Math.abs(left - right) > 1) return -1;
    return Math.max(left, right) + 1;
  }
  function inorder(node) {
    if (!node) return;
    inorder(node.left);
    values.push(node.val);
    inorder(node.right);
  }
  if (height(root) < 0) return false;
  inorder(root);
  return deepEqual(values, expected);
}

function isTopologicalOrder(testCase, actual) {
  const [numCourses, prerequisites] = testCase.input;
  if (!Array.isArray(actual)) return false;
  if (testCase.expected.length === 0) return actual.length === 0;
  if (actual.length !== numCourses) return false;
  const positions = new Map();
  for (let index = 0; index < actual.length; index++) {
    const course = actual[index];
    if (!Number.isInteger(course) || course < 0 || course >= numCourses || positions.has(course)) return false;
    positions.set(course, index);
  }
  return prerequisites.every(([course, prerequisite]) => positions.get(prerequisite) < positions.get(course));
}

export function checkAnswer(spec, testCase, actual) {
  if (spec.checker === "twoSum") {
    const [nums, target] = testCase.input;
    if (!Array.isArray(actual) || actual.length !== 2) return false;
    const [i, j] = actual;
    return Number.isInteger(i) && Number.isInteger(j) && i !== j && i >= 0 && j >= 0 && i < nums.length && j < nums.length && nums[i] + nums[j] === target;
  }
  if (spec.checker === "palindrome") {
    const [text] = testCase.input;
    const expected = testCase.expected;
    return typeof actual === "string" && actual.length === expected.length && text.includes(actual) && actual === actual.split("").reverse().join("");
  }
  if (spec.checker === "arrayBag") {
    return deepEqual(sortedValues(actual), sortedValues(testCase.expected));
  }
  if (spec.checker === "tripletSet") {
    return deepEqual(sortedTriplets(actual), sortedTriplets(testCase.expected));
  }
  if (spec.checker === "integerRows") {
    return deepEqual(sortedIntegerRows(actual, false), sortedIntegerRows(testCase.expected, false));
  }
  if (spec.checker === "integerCombinations") {
    return deepEqual(sortedIntegerRows(actual, true), sortedIntegerRows(testCase.expected, true));
  }
  if (spec.checker === "anagramGroups") {
    return deepEqual(sortedAnagramGroups(actual), sortedAnagramGroups(testCase.expected));
  }
  if (spec.checker === "balancedBst") {
    return isBalancedBst(testCase, actual);
  }
  if (spec.checker === "topologicalOrder") {
    return isTopologicalOrder(testCase, actual);
  }
  if (spec.checker === "approxNumber") {
    return closeNumber(actual, testCase.expected);
  }
  return deepEqual(normalize(actual), normalize(testCase.expected));
}

export function renderValue(value) {
  let text;
  try {
    text = value === undefined ? "undefined" : JSON.stringify(value) || String(value);
  } catch {
    text = String(value);
  }
  return text.length > 120 ? `${text.slice(0, 117)}…` : text;
}

/// LiveKit data-channel topics. Must match the TOPIC_* constants in
/// src/runtime.rs.
export const topics = {
  code: "code_update",
  control: "control",
  integrity: "integrity",
  report: "report",
  tests: "test_results",
  transcript: "lk.transcription",
};

export function isAgent(participant) {
  return Boolean(participant) && (participant.kind === "AGENT" || participant.permissions?.agent || participant.identity?.startsWith("interviewer-"));
}

// Only the interviewer agent may end the session. Every other participant holds
// `canPublishData`, so an unchecked report topic would let a peer drive the
// candidate's report UI.
export function acceptsReport(topic, participant) {
  return topic === topics.report && isAgent(participant);
}

// Data-channel payload builders. The agent decodes these by key, so they are
// a wire contract; web/tests/lib.test.js pins the exact shapes.

export function codeUpdatePayload(code, language, at) {
  return at === undefined ? { code, language } : { code, language, at };
}

export function timeWarningPayload(remainingSeconds) {
  return { type: "time_warning", remainingSeconds };
}

export function endInterviewPayload(reason, code, language) {
  return { type: "end_interview", reason, code, language };
}

export function testPayload(summary) {
  return {
    passed: summary.passed,
    total: summary.total,
    language: summary.language,
    setupError: summary.setupError || null,
    failures: summary.cases.filter((item) => !item.pass).slice(0, 4).map((item) => ({ label: item.label, expected: item.expected, got: item.got, error: item.error || null })),
    at: Date.now(),
  };
}

export function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value ?? null);
}

/// Must equal `MAX_INTEGRITY_TEXT` in `src/agent.rs`. The agent normalizes this
/// field before recomputing the hash, so any value the producer emits above
/// this bound is unverifiable by construction, and the rejection is silent.
export const INTEGRITY_DETAIL_MAX = 80;

export async function integrityEventPayload(input, previous = { seq: 0, hash: "" }) {
  const sourceEventIds = Array.isArray(input.sourceEventIds)
    ? input.sourceEventIds.map(String).filter((value) => /^\d+$/.test(value)).slice(0, 4).map((value) => value.slice(0, 12))
    : [];
  const event = {
    seq: previous.seq + 1,
    prevHash: previous.hash,
    type: String(input.type || ""),
    at: input.at || new Date().toISOString(),
    severity: input.severity || "info",
    source: input.source || "media",
    durationMs: Math.max(0, Math.trunc(Number(input.durationMs) || 0)),
    // 80, matching MAX_INTEGRITY_TEXT in src/agent.rs, and that is not a
    // coincidence to be maintained by memory. The agent truncates `detail` to
    // 80 BEFORE recomputing the hash, so emitting anything longer produced a
    // hash the verifier could not reproduce. The event was rejected, the
    // expected sequence never advanced, and every later event was rejected too,
    // silently: a normal camera heartbeat detail is 83 characters, so every
    // camera interview shipped a report containing two integrity events and
    // rendered the rest as "(none captured)".
    detail: input.detail === undefined ? null : String(input.detail).slice(0, INTEGRITY_DETAIL_MAX),
    sourceEventIds,
  };
  const body = {
    seq: event.seq,
    prevHash: event.prevHash,
    type: event.type,
    at: event.at,
    severity: event.severity,
    source: event.source,
    durationMs: event.durationMs,
    detail: event.detail,
  };
  if (event.sourceEventIds.length) body.sourceEventIds = event.sourceEventIds;
  const bytes = new TextEncoder().encode(canonicalJson(body));
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  event.hash = [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
  return event;
}

// The report arrives over a LiveKit data channel, so treat every field as
// untrusted: scores are rendered into innerHTML and must not carry markup.
export function sanitizeReport(raw) {
  const bounded = (value, max) => {
    const number = Math.trunc(Number(value));
    return Number.isFinite(number) ? clamp(number, 0, max) : 0;
  };
  const score = (value) => bounded(value, 100);
  const feedback = (section) => ({
    strengths: stringList(section?.strengths),
    improvements: stringList(section?.improvements),
  });
  // A report with nothing in it must survive normalization as a report with
  // nothing in it. Falling through to the fields below would score the missing
  // numbers as 0 and coerce the missing decision to NO_HIRE, which is how a
  // candidate who never spoke got a rejection in the first place; doing it
  // again on the way out of storage would just move the fabrication later.
  if (raw?.incomplete) {
    return {
      incomplete: true,
      summary: typeof raw?.summary === "string" ? boundedText(raw.summary) : "",
      integrityEvents: integrityEvents(raw?.integrityEvents),
      hintsUsed: bounded(raw?.hintsUsed, 99),
    };
  }
  return {
    codingScore: score(raw?.codingScore),
    communicationScore: score(raw?.communicationScore),
    decision: raw?.decision === "HIRE" ? "HIRE" : "NO_HIRE",
    summary: typeof raw?.summary === "string" ? boundedText(raw.summary) : "",
    codingFeedback: feedback(raw?.codingFeedback),
    communicationFeedback: feedback(raw?.communicationFeedback),
    integrityEvents: integrityEvents(raw?.integrityEvents),
    // Bounded like the scores: `JSON.parse` turns 1e999 into Infinity, which
    // would otherwise render as "Infinity hints used" and land in history.
    hintsUsed: bounded(raw?.hintsUsed, 99),
  };
}

function integrityEvents(events) {
  if (!Array.isArray(events)) return [];
  return events.slice(0, 25).map((event) => ({
    type: typeof event?.type === "string" ? boundedText(event.type, 40) : "",
    at: typeof event?.at === "string" ? boundedText(event.at, 40) : "",
    severity: ["info", "warning", "high", "critical"].includes(event?.severity) ? event.severity : "info",
    source: typeof event?.source === "string" ? boundedText(event.source, 20) : "",
    durationMs: clamp(Math.trunc(Number(event?.durationMs) || 0), 0, 86_400_000),
    seq: clamp(Math.trunc(Number(event?.seq) || 0), 0, Number.MAX_SAFE_INTEGER),
    prevHash: typeof event?.prevHash === "string" ? boundedText(event.prevHash, 64) : "",
    hash: typeof event?.hash === "string" ? boundedText(event.hash, 64) : "",
    detail: typeof event?.detail === "string" ? boundedText(event.detail, 80) : null,
    sourceEventIds: stringList(event?.sourceEventIds).filter((value) => /^\d+$/.test(value)).map((value) => boundedText(value, 12)),
  }));
}

// Counts alone do not bound the payload: `/api/reports` rejects an oversized
// body with a 413 the candidate can do nothing about, so long grader text or an
// event flood would silently cost someone their history.
const MAX_REPORT_TEXT = 300;

function boundedText(value, max = MAX_REPORT_TEXT) {
  return String(value).slice(0, max);
}

function stringList(value) {
  return Array.isArray(value) ? value.slice(0, 4).map((item) => boundedText(item)) : [];
}
