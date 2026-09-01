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

/// The characters neither side keeps: control characters, plus the format
/// characters that reorder or hide the text around them.
///
/// Must strip exactly what `hidden_or_reordering` in `src/agent/integrity.rs`
/// refuses. That filter is all-or-nothing: a `detail` carrying one of these
/// becomes null there, so the agent hashes `"detail":null` while the browser
/// hashed the text, the hashes differ, the event is refused, and because
/// sequence continuity is required every later event is refused too. It is the
/// 160-versus-80 length bound again, with a character set instead of a length.
///
/// Written as explicit code points rather than `\p{Cf}`, because Rust's std has
/// no general-category lookup: a category on this side and a hand-written list
/// on that one would drift.
///
/// U+200C and U+200D are deliberately absent from the U+200B run. The
/// zero-width non-joiner is orthographic in Persian and Urdu and the zero-width
/// joiner builds Indic conjuncts, so stripping them corrupts exactly the
/// localized labels this filter was widened to keep. Neither reorders anything:
/// they change how the glyphs beside them connect.
const INTEGRITY_DETAIL_STRIPPED =
  /[\p{Cc}\u00AD\u061C\u200B\u200E\u200F\u202A-\u202E\u2060-\u2064\u2066-\u2069\uFEFF]/gu;

/// A control character is a separator, so prose keeps a space where one stood.
/// Only the render path wants that; `integrityDetail` feeds a hash and has to
/// match the agent, which substitutes nothing.
const CONTROL_SEPARATOR = /\p{Cc}/gu;

/// Code points, not UTF-16 units, matching Rust's `chars().take()`. Slicing a
/// string directly stops early on any astral character and can leave half a
/// surrogate pair behind, which is a different string from the one the agent
/// hashes.
function codePoints(text, max) {
  return Array.from(text).slice(0, max).join("");
}

function integrityDetail(value) {
  return codePoints(String(value).replace(INTEGRITY_DETAIL_STRIPPED, ""), INTEGRITY_DETAIL_MAX);
}

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
    detail: input.detail === undefined ? null : integrityDetail(input.detail),
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

/// The two frameworks, kept apart on purpose.
///
/// A candidate in the coding round is working through REACTO and has no use for
/// the four behavioral steps; showing all ten at once was the confusion this
/// replaced. Ids match the `phase` spelling the interviewer records evidence
/// under, so a tick is a lookup rather than a translation.
export const FRAMEWORKS = {
  coding: {
    name: "REACTO",
    scenario: "Working a problem: what was asked for at each step of the coding round",
    // One clause each, written for someone reading it once while waiting for
    // the interviewer to speak. Long enough to act on, short enough that the
    // whole flow is legible before the card goes away.
    steps: [
      { id: "repeat", label: "Repeat", hint: "say the problem back in your own words" },
      { id: "example", label: "Example", hint: "walk one ordinary case and one edge case" },
      { id: "algorithm", label: "Algorithm", hint: "explain the approach and its cost before you type" },
      { id: "coding", label: "Coding", hint: "write what you just described" },
      { id: "test", label: "Test", hint: "predict what should happen, then run it" },
      { id: "optimizations", label: "Optimizations", hint: "confirm the complexity and name one improvement" },
    ],
  },
  behavioral: {
    name: "STAR",
    scenario: "Recounting past work: what a behavioral answer is listened for",
    steps: [
      { id: "situation", label: "Situation", hint: "where you were and what was going on" },
      { id: "task", label: "Task", hint: "what you were responsible for" },
      { id: "action", label: "Action", hint: "what you personally did, not the team" },
      { id: "result", label: "Result", hint: "how it turned out, and what you took from it" },
    ],
  },
};

// The report arrives over a LiveKit data channel, so treat every field as
// untrusted: scores are rendered into innerHTML and must not carry markup.
/// The ten REACTO and STAR phases, in the order src/agent.rs IMPROVEMENT_PHASES
/// and validate_framework_assessment require. Written once: it was three
/// literals here and in progress.js, and a phase added to one of them would have
/// been silently unassessed by the others.
export const frameworkPhases = Object.values(FRAMEWORKS).flatMap((framework) =>
  framework.steps.map((step) => step.label));

/// The two closed enums the server owns (InterviewMode::parse and
/// InterviewLoop::parse in src/agent.rs). Anything else is the default, which is
/// what makes a legacy or hostile value safe rather than an error. Stated once
/// here because seven modules were each restating the same ternary.


/// The steps of one round, each marked done or not.
///
/// Anything the interviewer sends that is not a known id is dropped rather than
/// rendered: the packet is untrusted like every other, and an unknown phase is
/// either a version skew or someone else's idea of a step.
export function frameworkChecklist(round, phases) {
  const framework = FRAMEWORKS[round] || FRAMEWORKS.coding;
  const done = new Set(Array.isArray(phases) ? phases.filter((phase) => typeof phase === "string") : []);
  return {
    name: framework.name,
    steps: framework.steps.map((step) => ({ ...step, done: done.has(step.id) })),
  };
}

function interviewMode(value) {
  return value === "practice" ? "practice" : "scored";
}

export function codingLoop(value) {
  return value === "coding_only" ? "coding_only" : "coding_behavioral";
}

/// Reports written before the practice/scored split was removed still carry a
/// mode, and the viewer shows what they say. Nothing produces one any more.
export function modeLabel(value) {
  return interviewMode(value) === "practice" ? "Practice" : "Scored";
}

export function loopLabel(value) {
  return codingLoop(value) === "coding_only" ? "Coding only" : "Coding + behavioral";
}

const textEncoder = new TextEncoder();

export function sanitizeReport(raw) {
  const activeContract = { bundleVersion: 4, livePromptVersion: 1, reportPromptVersion: 4, reportSchemaVersion: 1, rubricVersion: 1 };
  const contractKeys = Object.keys(activeContract);
  const candidateContract = raw?.interviewContract;
  const contractValues = candidateContract && typeof candidateContract === "object" && !Array.isArray(candidateContract)
    ? Object.keys(candidateContract).sort().join(",") === [...contractKeys].sort().join(",")
      && contractKeys.every((key) => Number.isSafeInteger(candidateContract[key])
        && candidateContract[key] >= 1 && candidateContract[key] <= 999)
      ? Object.fromEntries(contractKeys.map((key) => [key, candidateContract[key]])) : null
    : null;
  const interviewContract = candidateContract === undefined ? null : contractValues;
  const unsupportedContract = candidateContract !== undefined
    && (interviewContract === null || contractKeys.some((key) => interviewContract[key] !== activeContract[key]));
  // Only what the report actually recorded. Defaulting this to "scored" put a
  // mode on every new report and made the header announce a distinction that no
  // longer exists; a report written before the split still says what it was.
  const mode = raw?.mode === undefined ? undefined : interviewMode(raw.mode);
  // Defaulted for the round arithmetic below, which has always assumed the
  // two-round shape, but reported only where the report recorded it. Naming a
  // loop on a report written before loops existed describes a session that
  // never ran, the same way defaulting the mode did.
  const interviewLoop = codingLoop(raw?.interviewLoop);
  const recordedLoop = raw?.interviewLoop === undefined ? undefined : interviewLoop;
  const roundKinds = ["coding", "behavioral"];
  const codingStatuses = new Set(["complete", "incomplete"]);
  const behavioralStatuses = new Set(["complete", "started", "skipped", "not_configured"]);
  const rounds = Array.isArray(raw?.rounds) && raw.rounds.length === 2
    ? raw.rounds.map((round, index) => round?.kind === roundKinds[index]
      && Number.isInteger(round.budgetMin) && round.budgetMin >= 0 && round.budgetMin <= 90
      && (index === 0 ? codingStatuses : behavioralStatuses).has(round.status)
      ? { kind: round.kind, budgetMin: round.budgetMin, status: round.status } : null)
    : [];
  const roundSummary = rounds.length === 2 && rounds.every(Boolean)
    && rounds[0].budgetMin + rounds[1].budgetMin >= 10
    && rounds[0].budgetMin + rounds[1].budgetMin <= 90
    && (interviewLoop === "coding_only"
      ? rounds[1].budgetMin === 0 && rounds[1].status === "not_configured"
      : rounds[1].budgetMin === 8 && rounds[1].status !== "not_configured")
    ? rounds : [];
  const bounded = (value, max) => {
    const number = Math.trunc(Number(value));
    return Number.isFinite(number) ? clamp(number, 0, max) : 0;
  };
  const score = (value) => bounded(value, 100);
  // The chain checkpoint. The event list is a subsequence, so these are what
  // say how far the agent verified and how much it dropped on purpose; a report
  // from before they existed has none of them, and `null` reads as "this report
  // cannot tell you", which is the truth rather than a zero.
  //
  // `typeof value === "number"` rather than `Number.isFinite(Number(value))`,
  // because `Number(null)` is 0 and the agent sends null for a chain that never
  // advanced. That read an absent checkpoint as "verified through event 0, and
  // every event is listed above", which is a completeness claim about a report
  // that has no idea, and the exact thing the absent branch exists to refuse.
  const count = (value) =>
    typeof value === "number" && Number.isFinite(value) ? bounded(value, Number.MAX_SAFE_INTEGER) : null;
  const checkpoint = (raw) => ({
    integrityChainSeq: count(raw?.integrityChainSeq),
    integrityDropped: count(raw?.integrityDropped),
  });
  const feedback = (section) => ({
    strengths: stringList(section?.strengths),
    improvements: stringList(section?.improvements),
  });
  const codingFeedback = feedback(raw?.codingFeedback);
  const communicationFeedback = feedback(raw?.communicationFeedback);
  const weaknesses = new Set([...codingFeedback.improvements, ...communicationFeedback.improvements]);
  const phases = new Set(frameworkPhases);
  const plannedWeaknesses = new Set();
  const impactRank = { high: 3, medium: 2, low: 1 };
  const candidatePlan = (Array.isArray(raw?.improvementPlan) ? raw.improvementPlan : [])
    .slice(0, 16)
    .map((item) => {
      const phase = typeof item?.phase === "string" ? item.phase : "";
      const weakness = typeof item?.weakness === "string" ? boundedText(item.weakness, 400).trim() : "";
      const impact = typeof item?.impact === "string" ? item.impact : "";
      const drill = typeof item?.drill === "string" ? boundedText(item.drill, 400).trim() : "";
      const successCriterion = typeof item?.successCriterion === "string"
        ? boundedText(item.successCriterion, 400).trim() : "";
      const frequency = Math.trunc(Number(item?.frequency));
      const durationMin = Math.trunc(Number(item?.durationMin));
      const selfReview = Array.isArray(item?.selfReview)
        ? item.selfReview.slice(0, 4).map((check) => boundedText(check, 240).trim()).filter(Boolean)
        : [];
      if (!phases.has(phase) || plannedWeaknesses.has(weakness) || !weaknesses.has(weakness)
        || !impactRank[impact] || !Number.isFinite(frequency) || frequency < 1
        || !Number.isFinite(durationMin) || durationMin < 1 || !drill
        || !successCriterion || selfReview.length === 0) return null;
      plannedWeaknesses.add(weakness);
      return {
        phase,
        weakness,
        impact,
        frequency: clamp(frequency, 1, 99),
        drill,
        durationMin: clamp(durationMin, 1, 30),
        successCriterion,
        selfReview,
      };
    })
    .filter(Boolean)
    .sort((left, right) => impactRank[right.impact] - impactRank[left.impact]
      || right.frequency - left.frequency)
    .slice(0, 8);
  const improvementPlan = plannedWeaknesses.size === weaknesses.size
    && [...weaknesses].every((weakness) => plannedWeaknesses.has(weakness))
    ? candidatePlan
    : [];
  const assessmentPhases = [...phases];
  const candidateAssessment = raw?.frameworkAssessment;
  const assessmentRows = Array.isArray(candidateAssessment?.phases)
    ? candidateAssessment.phases : [];
  const seenAssessmentPhases = new Set();
  const normalizedAssessment = new Map();
  const assessmentVersion = candidateAssessment?.rubricVersion;
  let assessmentValid = Number.isSafeInteger(assessmentVersion) && assessmentVersion >= 1
    && assessmentRows.length === assessmentPhases.length
    && (!interviewContract || assessmentVersion === interviewContract.rubricVersion);
  for (const item of assessmentRows) {
    const phase = typeof item?.phase === "string" ? item.phase : "";
    const score = item?.score;
    const scoreValid = score === null
      || (typeof score === "number" && Number.isInteger(score) && score >= 0 && score <= 100);
    if (!phases.has(phase) || seenAssessmentPhases.has(phase) || !scoreValid
      || !Array.isArray(item?.weaknessTags)) {
      assessmentValid = false;
      continue;
    }
    seenAssessmentPhases.add(phase);
    const allowedTags = new Set(improvementPlan
      .filter((entry) => entry.phase === phase)
      .map((entry) => entry.weakness));
    const weaknessTags = [...new Set(item.weaknessTags
      .filter((tag) => typeof tag === "string")
      .map((tag) => boundedText(tag, 400).trim())
      .filter((tag) => tag && allowedTags.has(tag)))]
      .slice(0, 4);
    normalizedAssessment.set(phase, { phase, score, weaknessTags });
  }
  const frameworkAssessment = assessmentValid
    && seenAssessmentPhases.size === assessmentPhases.length
    ? {
      rubricVersion: assessmentVersion,
      phases: assessmentPhases.map((phase) => normalizedAssessment.get(phase)),
    }
    : null;
  const knownEvidenceFields = new Set([
    "atMs", "phase", "source", "kind", "confidence", "summary", "frameworkVersion",
  ]);
  const evidencePhases = new Set(frameworkPhases.map((phase) => phase.toLowerCase()));
  const frameworkSources = new Set(["candidate_speech", "editor_snapshot", "test_event", "session_timing"]);
  const frameworkKinds = new Set(["observed", "inferred", "skipped"]);
  const frameworkEvidence = (Array.isArray(raw?.frameworkEvidence) ? raw.frameworkEvidence : [])
    .map((item) => {
      const phase = typeof item?.phase === "string" ? item.phase : "";
      const source = typeof item?.source === "string" ? item.source : "";
      const kind = typeof item?.kind === "string" ? item.kind : "";
      const atMs = Math.trunc(Number(item?.atMs));
      const confidence = Math.trunc(Number(item?.confidence));
      const frameworkVersion = Math.trunc(Number(item?.frameworkVersion));
      const summary = typeof item?.summary === "string" ? boundedText(item.summary, 240).trim() : "";
      if (!evidencePhases.has(phase) || !frameworkSources.has(source) || !frameworkKinds.has(kind)
        || (source === "session_timing") !== (kind === "skipped")
        || !Number.isFinite(atMs) || atMs < 0 || !Number.isFinite(confidence)
        || confidence < 0 || confidence > 100 || !Number.isFinite(frameworkVersion)
        || frameworkVersion < 1 || !summary) return null;
      // Unknown fields round-trip, so a newer report re-saved by an older
      // client does not quietly lose what that client could not name. They
      // are also the only part of an evidence row with no size of its own,
      // and the whole report has to fit what the account sync accepts, which
      // refuses the request rather than trimming it: over that, the candidate
      // keeps the local copy and the account copy simply never arrives. So
      // they are carried while they are a field rather than a payload.
      // The common row has nothing unknown on it and this runs over whatever
      // length arrived from storage or the wire, before the cap below trims it,
      // so that row allocates nothing and is never serialized to be measured.
      // Null prototype, not `{}`: assigning a key named `__proto__` to a plain
      // object runs the inherited setter, which ignores a string and drops the
      // field. A newer client's field is not ours to name, so it cannot be ours
      // to lose either.
      let extras = null;
      for (const key of Object.keys(item)) {
        if (!knownEvidenceFields.has(key)) (extras ??= Object.create(null))[key] = item[key];
      }
      // Spreading null spreads nothing, which is what both the common row and
      // an over-budget one want.
      const carried = extras && textEncoder.encode(JSON.stringify(extras)).length <= 512 ? extras : null;
      return {
        ...carried,
        // A year, which no interview approaches: this is a sanity bound on a
        // timestamp that arrives as untrusted JSON, not a statement about how
        // long a session runs.
        atMs: clamp(atMs, 0, 31_536_000_000),
        phase,
        source,
        kind,
        confidence,
        summary,
        frameworkVersion,
      };
    })
    .filter(Boolean)
    .slice(0, 64)
    .sort((left, right) => left.atMs - right.atMs);
  // A report with nothing in it must survive normalization as a report with
  // nothing in it. Falling through to the fields below would score the missing
  // numbers as 0 and coerce the missing decision to NO_HIRE, which is how a
  // candidate who never spoke got a rejection in the first place; doing it
  // again on the way out of storage would just move the fabrication later.
  if (raw?.incomplete || unsupportedContract) {
    return {
      interviewContract,
      mode,
      interviewLoop: recordedLoop,
      rounds: roundSummary,
      incomplete: true,
      summary: unsupportedContract
        ? "This report uses an unsupported or malformed interview contract and cannot be scored by this version of CodeTrial."
        : typeof raw?.summary === "string" ? boundedText(raw.summary) : "",
      integrityEvents: integrityEvents(raw?.integrityEvents),
      ...checkpoint(raw),
      hintsUsed: bounded(raw?.hintsUsed, 99),
      improvementPlan: [],
      frameworkAssessment: null,
      frameworkEvidence,
    };
  }
  return {
    interviewContract,
    mode,
    interviewLoop: recordedLoop,
    rounds: roundSummary,
    codingScore: score(raw?.codingScore),
    communicationScore: score(raw?.communicationScore),
    decision: raw?.decision === "HIRE" ? "HIRE" : "NO_HIRE",
    summary: typeof raw?.summary === "string" ? boundedText(raw.summary) : "",
    codingFeedback,
    communicationFeedback,
    improvementPlan,
    frameworkAssessment,
    frameworkEvidence,
    integrityEvents: integrityEvents(raw?.integrityEvents),
    ...checkpoint(raw),
    // Bounded like the scores: `JSON.parse` turns 1e999 into Infinity, which
    // would otherwise render as "Infinity hints used" and land in history.
    hintsUsed: bounded(raw?.hintsUsed, 99),
  };
}

/// The evidence cap in src/agent.rs plus the two heartbeats that
/// `report_with_integrity_events` merges in beside it. Slicing at the evidence
/// cap alone would drop the closing device-state sample, which is the half of
/// the pair that says how the interview ended.
///
/// Exported so `tests/agent.rs` can hold it against `MAX_INTEGRITY_EVENTS`, the
/// way `INTEGRITY_DETAIL_MAX` is already held against `MAX_INTEGRITY_TEXT`. A
/// cap raised on one side and not the other truncates the report in silence.
export const MAX_INTEGRITY_ROWS = 27;

function integrityEvents(events) {
  if (!Array.isArray(events)) return [];
  return events.slice(0, MAX_INTEGRITY_ROWS).map((event) => ({
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

// Code points, not UTF-16 units, because `detail` now carries whatever the
// candidate named their camera. `slice` cut an 80-code-point label down to 44
// and left a lone surrogate on the end, which renders as a replacement glyph in
// the evidence a reviewer reads. Matches the bound `integrityDetail` enforces.
/// The last boundary before render, which is why the strip happens here and not
/// only in `integrityDetail`.
///
/// That one runs in the producer and `hidden_or_reordering` runs in the agent,
/// and one path crosses neither: `save_report_handler` in
/// src/web/interviews.rs stores a POSTed report body verbatim, and
/// web/replay.js reads it back through `sanitizeReport` into `reportMarkup`. A
/// client posting its own report could otherwise put a right-to-left override
/// straight into the rendered evidence, because `escapeHtml` handles markup and
/// nothing handles reordering. On the agent-delivered path this is a no-op.
///
/// Every rendered free-text field in `sanitizeReport` comes through here.
function boundedText(value, max = MAX_REPORT_TEXT) {
  return codePoints(
    String(value).replace(CONTROL_SEPARATOR, " ").replace(INTEGRITY_DETAIL_STRIPPED, ""),
    max,
  );
}

function stringList(value) {
  return Array.isArray(value) ? value.slice(0, 4).map((item) => boundedText(item)) : [];
}

/// When the interview starts reading as nearly over, both to the candidate and
/// to the interviewer.
export const TIME_WARNING_S = 300;

/// How much time is left, and what crossing that number means.
///
/// Split from the tick that paints it because both decisions here are about a
/// clock that jumps: a throttled or suspended tab skips whole minutes. That is
/// why the warning is a crossing and not an equality, and why it needs the
/// previous value rather than deriving everything from `endsAt` alone;
/// `remaining === TIME_WARNING_S` never fires when the value goes from 400 to
/// 240 in one tick. `now` is a parameter so this is assertable without waiting
/// out an interview.
export function countdown(previous, endsAt, now) {
  const remaining = Math.max(0, Math.round((endsAt - now) / 1000));
  return {
    remaining,
    urgent: remaining <= TIME_WARNING_S,
    warn: previous > TIME_WARNING_S && remaining <= TIME_WARNING_S,
    expired: remaining === 0,
  };
}

/// The browser never scores anybody; this decides which honest incomplete
/// summary describes the provider state and locally observed activity.
///
/// A session that reached a real interviewer is graded by that interviewer or
/// not at all. Asking whether the socket is open right now is the wrong
/// question, and was asked here for one round: a dropped connection nulls the
/// room, which routed a candidate who had passed their tests straight into a
/// local scorer and rendered a green HIRE badge for a network failure, saved
/// it to localStorage and POSTed it to `/api/reports`. Whether an interviewer
/// was ever present is a different fact from whether the connection survived,
/// and only the first one decides this.
export function sessionReport({ joinedRoom, passed, total, candidateTurns }) {
  if (joinedRoom) {
    return {
      incomplete: true,
      summary:
        "This interview did not produce an evaluation: the interviewer never returned a report. Nothing you did was assessed, and no result was recorded.",
      hintsUsed: 0,
    };
  }

  return {
    incomplete: true,
    summary: total || candidateTurns
      ? `Offline mode recorded local activity${total ? ` and ${passed}/${total} browser test cases passed` : ""}. No live interviewer assessed it, so no personalized scores, verdict, or feedback were created.`
      : "No interviewer joined and this session produced no evaluation. Nothing you did was assessed, and no result was recorded.",
    hintsUsed: 0,
  };
}

/// Where each sentence begins. A terminator only ends a sentence when
/// whitespace follows it, so "3.14" and "e.g." stay in one piece. Written as a
/// scan rather than a regex because the regex that expresses this needs
/// lookbehind, which is exactly the kind of thing Safari has been late to.
function sentenceStarts(text) {
  const starts = [0];
  for (let i = 0; i < text.length - 1; i += 1) {
    if (!".!?".includes(text[i])) continue;
    // "?!" and "..." are one boundary, not two or three.
    let end = i;
    while (end + 1 < text.length && ".!?".includes(text[end + 1])) end += 1;
    if (/\s/.test(text[end + 1] ?? " ")) starts.push(end + 1);
    i = end;
  }
  return starts;
}

/// The caption bar holds one line, so a long turn has to be trimmed. Trimming
/// to the last N characters is what made it unreadable: the window slid by a
/// character on every transcript fragment, so the start of the sentence being
/// read walked off the left edge while it was being read. Measured against the
/// audio the caption was within 50ms, so the complaint was never about sync.
///
/// Sentences instead. The window holds the most recent whole sentences that
/// fit, so it stays still while a sentence is being spoken and jumps once when
/// the next one starts. A single sentence longer than the budget falls back to
/// a character tail, because an empty caption bar is worse than one that opens
/// mid-word. The ellipsis comes out of the budget rather than being added on
/// top of it: this is the one line the bar has.
export function captionWindow(text, maxChars) {
  if (text.length <= maxChars) return text;
  // Ascending, so the first start that fits is also the one keeping the most
  // sentences.
  for (const start of sentenceStarts(text)) {
    const rest = text.slice(start).trim();
    // A terminator at the very end of the text opens a sentence that has no
    // words in it yet, and every later start is emptier still. Returning that
    // one blanked the bar the moment a long sentence finished, which is exactly
    // when there is most to read.
    if (!rest) break;
    if (rest.length <= maxChars) return rest;
  }
  // Below four characters the ellipsis cannot fit beside anything, so it is
  // dropped rather than pushing the line over the budget it was given. Zero is
  // its own case because `slice(-0)` is `slice(0)`, which returns everything.
  if (maxChars < 4) return maxChars > 0 ? text.slice(-maxChars) : "";
  return `...${text.slice(-(maxChars - 3))}`;
}

export function providerUiState(kind, detail = "") {
  const detailText = String(detail).toLowerCase();
  const reason = /429|rate limit|too many|busy|capacity/.test(detailText)
    ? "The live interview service is busy or rate limited."
    : /quota|connection minutes/.test(detailText)
      ? "The live interview provider has no available session capacity."
      : /microphone|publish/.test(detailText)
        ? "The microphone could not be connected to the live interview."
        : "The live interview provider could not be reached.";
  const states = {
    connecting: { label: "Connecting", message: "Connecting to the live interviewer.", personalized: false, retry: false },
    live: { label: "Live", message: "The live interviewer is connected.", personalized: true, retry: false },
    reconnecting: { label: "Reconnecting", message: "Reconnecting to the interviewer. Keep working; your code is safe.", personalized: true, retry: false },
    degraded: { label: "Offline", message: `${reason} You can still work the problem, but it will not create a personalized evaluation.`, personalized: false, retry: true },
    report_generating: { label: "Preparing report", message: "Preparing your personalized report. A slow grader can take up to a minute.", personalized: true, retry: false },
    incomplete_report: { label: "Incomplete report", message: "The provider could not produce a valid personalized evaluation. No scores or verdict were created.", personalized: false, retry: true },
    retry_ready: { label: "Retry available", message: "The report is still unavailable. Leave safely, then retry the interview when the provider recovers.", personalized: false, retry: true },
  };
  return states[kind] || states.degraded;
}
