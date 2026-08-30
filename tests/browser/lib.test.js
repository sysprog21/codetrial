// Run with: node --test tests/browser/*.test.js
// Covers the browser judge/report contracts that used to be asserted by
// substring-matching web/interview.js from Rust.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  captionWindow,
  TIME_WARNING_S,
  acceptsReport,
  checkAnswer,
  clamp,
  codeUpdatePayload,
  canonicalJson,
  countdown,
  deepEqual,
  endInterviewPayload,
  escapeHtml,
  formatTime,
  INTEGRITY_DETAIL_MAX,
  integrityEventPayload,
  normalize,
  orPlaceholder,
  renderValue,
  sanitizeReport,
  resumeDeadline,
  sessionReport,
  testPayload,
  timeWarningPayload,
  topics,
} from "../../web/lib.js";

const twoSum = { checker: "twoSum" };
const palindrome = { checker: "palindrome" };
const exact = { checker: "exact" };
const tripletSet = { checker: "tripletSet" };
const integerRows = { checker: "integerRows" };
const integerCombinations = { checker: "integerCombinations" };
const anagramGroups = { checker: "anagramGroups" };
const balancedBst = { checker: "balancedBst" };
const topologicalOrder = { checker: "topologicalOrder" };
const approxNumber = { checker: "approxNumber" };

test("twoSum checker accepts any valid index pair, not just the expected one", () => {
  const testCase = { input: [[2, 7, 11, 15], 9], expected: [0, 1] };

  assert.equal(checkAnswer(twoSum, testCase, [0, 1]), true);
  assert.equal(checkAnswer(twoSum, testCase, [1, 0]), true);
});

test("twoSum checker rejects reused, out-of-range, and non-integer indices", () => {
  const testCase = { input: [[3, 3], 6], expected: [0, 1] };

  assert.equal(checkAnswer(twoSum, testCase, [0, 1]), true, "duplicate values are solvable");
  assert.equal(checkAnswer(twoSum, testCase, [0, 0]), false, "same element twice");
  assert.equal(checkAnswer(twoSum, testCase, [0, 5]), false, "index past the end");
  assert.equal(checkAnswer(twoSum, testCase, [0, -1]), false, "negative index");
  assert.equal(checkAnswer(twoSum, testCase, [0, 1.5]), false, "non-integer index");
  assert.equal(checkAnswer(twoSum, testCase, [0]), false, "wrong arity");
  assert.equal(checkAnswer(twoSum, testCase, "01"), false, "not an array");
  assert.equal(checkAnswer(twoSum, { input: [[1, 2], 9], expected: [] }, [0, 1]), false, "sum mismatch");
});

test("palindrome checker accepts any equal-length palindromic substring", () => {
  const testCase = { input: ["cbbd"], expected: "bb" };

  assert.equal(checkAnswer(palindrome, testCase, "bb"), true);
  assert.equal(checkAnswer(palindrome, { input: ["aba"], expected: "aba" }, "aba"), true);
  assert.equal(checkAnswer(palindrome, testCase, "cb"), false, "not a palindrome");
  assert.equal(checkAnswer(palindrome, testCase, "bbbb"), false, "wrong length");
  assert.equal(checkAnswer(palindrome, testCase, "xx"), false, "not a substring of the input");
  assert.equal(checkAnswer(palindrome, testCase, 42), false, "not a string");
});

test("default checker compares deeply and treats undefined as null", () => {
  assert.equal(checkAnswer(exact, { input: [], expected: [1, [2, 3]] }, [1, [2, 3]]), true);
  assert.equal(checkAnswer(exact, { input: [], expected: [1, 2] }, [1, 2, 3]), false);
  assert.equal(checkAnswer(exact, { input: [], expected: null }, undefined), true);
  assert.equal(normalize(undefined), null);
  assert.equal(deepEqual([undefined], [null]), true);
  assert.equal(deepEqual({ a: 1 }, { a: 1 }), false, "objects are not compared structurally");
});

test("approxNumber checker accepts small floating point drift", () => {
  assert.equal(checkAnswer(approxNumber, { input: [], expected: 9.261 }, 9.261000001), true);
  assert.equal(checkAnswer(approxNumber, { input: [], expected: 9.261 }, 9.27), false);
  assert.equal(checkAnswer(approxNumber, { input: [], expected: 1 }, "1"), false);
});

test("arrayBag checker ignores order but preserves multiplicity", () => {
  const spec = { checker: "arrayBag" };

  assert.equal(checkAnswer(spec, { input: [], expected: [0, 1, 3, 0, 4] }, [4, 0, 3, 1, 0]), true);
  assert.equal(checkAnswer(spec, { input: [], expected: ["ad", "ae", "af"] }, ["af", "ad", "ae"]), true);
  assert.equal(checkAnswer(spec, { input: [], expected: ["aa", "aa"] }, ["aa"]), false);
  assert.equal(checkAnswer(spec, { input: [], expected: [2, 2] }, [2]), false);
  assert.equal(checkAnswer(spec, { input: [], expected: [2, 2] }, [2, 3]), false);
  assert.equal(checkAnswer(spec, { input: [], expected: [0, 9] }, [9, 0]), true);
});

test("tripletSet checker ignores triplet and result order", () => {
  const testCase = { input: [], expected: [[-1, -1, 2], [-1, 0, 1]] };

  assert.equal(checkAnswer(tripletSet, testCase, [[1, -1, 0], [2, -1, -1]]), true);
  assert.equal(checkAnswer(tripletSet, testCase, [[-1, 0, 1]]), false, "missing triplet");
  assert.equal(checkAnswer(tripletSet, testCase, [[-1, 0, 1], [-1, -1, 3]]), false, "wrong sum");
  assert.equal(checkAnswer(tripletSet, testCase, [[-1, 0]]), false, "wrong triplet size");
});

test("integer row checkers handle nested backtracking outputs", () => {
  assert.equal(checkAnswer(integerRows, { input: [], expected: [[1, 2], [2, 1]] }, [[2, 1], [1, 2]]), true);
  assert.equal(checkAnswer(integerRows, { input: [], expected: [[1, 2]] }, [[2, 1]]), false, "permutation row order matters");
  assert.equal(checkAnswer(integerCombinations, { input: [], expected: [[2, 2, 3], [7]] }, [[7], [3, 2, 2]]), true);
  assert.equal(checkAnswer(integerCombinations, { input: [], expected: [[1, 1]] }, [[1]]), false, "row multiplicity matters");
});

test("anagramGroups checker ignores group and word order", () => {
  const testCase = { input: [], expected: [["eat", "tea", "ate"], ["tan", "nat"], ["bat"]] };

  assert.equal(checkAnswer(anagramGroups, testCase, [["bat"], ["nat", "tan"], ["ate", "eat", "tea"]]), true);
  assert.equal(checkAnswer(anagramGroups, testCase, [["bat"], ["nat"], ["ate", "eat", "tea"]]), false, "missing word");
  assert.equal(checkAnswer(anagramGroups, testCase, [["bat", "tab"], ["nat", "tan"], ["ate", "eat"]]), false, "wrong grouping");
  assert.equal(checkAnswer(anagramGroups, { input: [], expected: [["", ""]] }, [[""]]), false, "duplicate count matters");
});

test("balancedBst checker accepts any balanced BST with the same inorder values", () => {
  assert.equal(checkAnswer(balancedBst, { input: [[1, 3]], expected: [3, 1] }, [1, null, 3]), true);
  assert.equal(checkAnswer(balancedBst, { input: [[1, 2, 3, 4]], expected: [] }, [2, 1, 3, null, null, null, 4]), true);
  assert.equal(checkAnswer(balancedBst, { input: [[1, 2, 3, 4]], expected: [] }, [1, null, 2, null, 3, null, 4]), false);
  assert.equal(checkAnswer(balancedBst, { input: [[1, 2, 3]], expected: [] }, [2, 3, 1]), false);
});

test("topologicalOrder checker accepts any valid course order", () => {
  const testCase = { input: [4, [[1, 0], [2, 0], [3, 1], [3, 2]]], expected: [0, 1, 2, 3] };

  assert.equal(checkAnswer(topologicalOrder, testCase, [0, 2, 1, 3]), true);
  assert.equal(checkAnswer(topologicalOrder, testCase, [0, 1, 2, 3]), true);
  assert.equal(checkAnswer(topologicalOrder, testCase, [0, 1, 3, 2]), false, "course before prerequisite");
  assert.equal(checkAnswer(topologicalOrder, testCase, [0, 1, 1, 3]), false, "duplicate course");
  assert.equal(checkAnswer(topologicalOrder, testCase, [0, 1, 3]), false, "missing course");
  assert.equal(checkAnswer(topologicalOrder, { input: [2, [[1, 0], [0, 1]]], expected: [] }, []), true);
  assert.equal(checkAnswer(topologicalOrder, { input: [2, [[1, 0], [0, 1]]], expected: [] }, [0, 1]), false);
});

test("renderValue serializes and truncates long output", () => {
  assert.equal(renderValue([1, 2]), "[1,2]");
  assert.equal(renderValue(undefined), "undefined");
  assert.equal(renderValue("x"), '"x"');

  const long = renderValue("a".repeat(400));
  assert.equal(long.length, 118, "117 chars plus the ellipsis");
  assert.ok(long.endsWith("…"));

  const cyclic = {};
  cyclic.self = cyclic;
  assert.equal(renderValue(cyclic), "[object Object]", "falls back instead of throwing");
});

test("testPayload keeps the agent wire contract and caps failures at four", () => {
  const summary = {
    passed: 1,
    total: 7,
    language: "python",
    setupError: "",
    cases: [
      { label: "ok", pass: true },
      ...Array.from({ length: 6 }, (_, index) => ({
        label: `bad-${index}`,
        pass: false,
        expected: "1",
        got: "2",
        error: "",
      })),
    ],
  };

  const payload = testPayload(summary);

  assert.deepEqual(Object.keys(payload).sort(), [
    "at",
    "failures",
    "language",
    "passed",
    "setupError",
    "total",
  ]);
  assert.equal(payload.setupError, null, "empty setup error normalizes to null");
  assert.equal(payload.failures.length, 4);
  assert.deepEqual(Object.keys(payload.failures[0]).sort(), ["error", "expected", "got", "label"]);
  assert.equal(payload.failures[0].error, null);
});

test("data-channel payloads keep the keys the Rust agent decodes", () => {
  // src/agent.rs reads code/language off code_update, and type plus
  // remainingSeconds / reason / code / language off control.
  assert.deepEqual(codeUpdatePayload("x = 1", "python"), {
    code: "x = 1",
    language: "python",
  });
  assert.deepEqual(codeUpdatePayload("x = 1", "python", 1234), {
    code: "x = 1",
    language: "python",
    at: 1234,
  });
  assert.deepEqual(timeWarningPayload(300), {
    type: "time_warning",
    remainingSeconds: 300,
  });
  assert.deepEqual(endInterviewPayload("time_up", "done", "javascript"), {
    type: "end_interview",
    reason: "time_up",
    code: "done",
    language: "javascript",
  });
});

test("integrity events are canonical hashed chain payloads", async () => {
  const first = await integrityEventPayload({
    type: "SESSION_START",
    at: "2026-08-15T00:00:00.000Z",
    severity: "info",
    source: "media",
    durationMs: 0,
    detail: "ok",
  });
  const second = await integrityEventPayload({
    type: "SCREEN_SHARE_STOPPED",
    at: "2026-08-15T00:00:05.000Z",
    severity: "high",
    source: "screen",
    durationMs: 5000,
    sourceEventIds: ["1", "2"],
  }, first);

  assert.equal(topics.integrity, "integrity");
  assert.equal(first.seq, 1);
  assert.equal(first.prevHash, "");
  assert.match(first.hash, /^[0-9a-f]{64}$/);
  assert.equal(second.seq, 2);
  assert.equal(second.prevHash, first.hash);
  assert.deepEqual(first.sourceEventIds, []);
  assert.deepEqual(second.sourceEventIds, ["1", "2"]);
  assert.match(
    canonicalJson({ b: 2, a: 1 }),
    /^\{"a":1,"b":2\}$/,
    "hash input must be deterministic",
  );
});

test("a camera label is carried as detail without outgrowing the hashed bound", async () => {
  // The preflight names the camera because nothing in the media path can tell a
  // virtual camera from a real one. Device labels are long and full of
  // punctuation, and `detail` is hashed: a producer that emits more than
  // INTEGRITY_DETAIL_MAX builds a hash the agent cannot reproduce, which stops
  // the chain for the rest of the interview. That is not hypothetical, it is
  // why the bound is commented in web/lib.js.
  // Long enough to be cut, with an astral character across the cut. `.slice()`
  // counts UTF-16 units, so it would stop early and leave half a surrogate pair
  // behind; the agent counts code points and would hash the other string. That
  // divergence is the entire reason `integrityDetail` uses `Array.from`. A label
  // that fits under the bound exercises none of it.
  // Placed so the emoji's two UTF-16 units straddle index 80: "camera=" is 7,
  // the kana are 70, "ab" is 2, so the pair occupies units 79 and 80. A
  // `.slice(0, 80)` keeps unit 79 and drops its partner.
  const label = "仮想カメラ".repeat(14) + "ab\u{1F3A5}tail";
  const event = await integrityEventPayload({
    type: "MEDIA_PREFLIGHT_PASSED",
    source: "preflight",
    detail: `camera=${label}`,
  });
  assert.equal(Array.from(event.detail).length, INTEGRITY_DETAIL_MAX);
  // A well-formed pair is one element from `Array.from`; a lone surrogate is a
  // one-unit element in the surrogate range, which is what a UTF-16 cut leaves.
  assert.ok(
    !Array.from(event.detail).some((c) => c.length === 1 && c.charCodeAt(0) >= 0xd800 && c.charCodeAt(0) <= 0xdfff),
    "truncation split a surrogate pair",
  );
  assert.ok(event.detail.startsWith("camera=仮想カメラ"), event.detail);
});

test("a device label cannot reorder or hide the report it is printed in", async () => {
  // The label is whatever the candidate named their device, and it is rendered
  // into the interviewer's report where escapeHtml handles markup and nothing
  // handles a right-to-left override. `hidden_or_reordering` in
  // src/agent/integrity.rs refuses the identical set: a character kept here and
  // dropped there is a hash the agent cannot reproduce, which stops the chain.
  const hostile = "camera=\u202Egnitautis\u200B\u00AD\u2066\uFEFF end";
  const event = await integrityEventPayload({ type: "MEDIA_PREFLIGHT_PASSED", detail: hostile });
  assert.equal(event.detail, "camera=gnitautis end");

  // Localized labels survive. Stripping them would leave the evidence
  // unreadable for the people most likely to need it.
  const localized = await integrityEventPayload({
    type: "MEDIA_PREFLIGHT_PASSED",
    detail: "camera=FaceTime HD \u30AB\u30E1\u30E9",
  });
  assert.equal(localized.detail, "camera=FaceTime HD \u30AB\u30E1\u30E9");

  // The zero-width non-joiner is orthographic here, not decoration, which is
  // why the U+200B run is not a solid range on either side.
  const persian = "camera=\u0645\u06CC\u200C\u0631\u0648\u062F";
  const kept = await integrityEventPayload({ type: "MEDIA_PREFLIGHT_PASSED", detail: persian });
  assert.equal(kept.detail, persian);
});

test("every Unicode bidi control is stripped, not the ones we thought of", async () => {
  // Bidi_Control is a closed set of twelve. Listing ranges by hand is how
  // U+061C ARABIC LETTER MARK was missed, so assert the property rather than
  // the list: the next code point nobody thinks of fails here.
  const BIDI_CONTROL = [
    0x061c, 0x200e, 0x200f, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e,
    0x2066, 0x2067, 0x2068, 0x2069,
  ];
  for (const codePoint of BIDI_CONTROL) {
    const event = await integrityEventPayload({
      type: "MEDIA_PREFLIGHT_PASSED",
      detail: `a${String.fromCodePoint(codePoint)}b`,
    });
    assert.equal(event.detail, "ab", `U+${codePoint.toString(16).toUpperCase()} reached the report`);
  }
});

test("a stored report cannot smuggle a reordering character past the render", () => {
  // `integrityDetail` runs in the producer and the agent refuses what it missed,
  // but src/web/interviews.rs stores a POSTed report body verbatim and
  // web/replay.js renders it back through `sanitizeReport`. That path crosses
  // neither filter, and `escapeHtml` does not touch bidi.
  const clean = sanitizeReport({
    problemId: "two-sum",
    summary: "looked\u202Efine",
    integrityEvents: [{
      type: "CAMERA_STOPPED", at: "00:01", severity: "high",
      source: "camera", detail: "camera=\u202Egnitautis\u200B",
    }],
  });
  assert.equal(clean.integrityEvents[0].detail, "camera=gnitautis");
  assert.equal(clean.summary, "lookedfine");
});

test("only the agent may deliver a report, and only on the report topic", () => {
  const agentByKind = { kind: "AGENT" };
  const agentByPermission = { permissions: { agent: true } };
  const agentByIdentity = { identity: "interviewer-interview-abc" };
  const candidate = { kind: "STANDARD", identity: "candidate-abc", permissions: { agent: false } };

  for (const agent of [agentByKind, agentByPermission, agentByIdentity]) {
    assert.equal(acceptsReport(topics.report, agent), true);
  }
  // Every participant holds canPublishData, so a peer must not be able to
  // drive the report UI.
  assert.equal(acceptsReport(topics.report, candidate), false, "candidate cannot publish a report");
  assert.equal(acceptsReport(topics.report, undefined), false, "unknown sender cannot publish a report");
  assert.equal(acceptsReport(topics.code, agentByKind), false, "wrong topic");
});

test("escapeHtml neutralizes every markup character", () => {
  assert.equal(escapeHtml(`<img src=x onerror="a">&'`), "&lt;img src=x onerror=&quot;a&quot;&gt;&amp;&#39;");
});

test("sanitizeReport clamps scores and strips markup from a hostile report", () => {
  const report = sanitizeReport({
    codingScore: "<img src=x onerror=alert(1)>",
    communicationScore: 5000,
    decision: "MAYBE",
    summary: { toString: () => "not a string" },
    codingFeedback: { strengths: ["a", "b", "c", "d", "e"], improvements: "nope" },
    communicationFeedback: null,
    integrityEvents: [
      {
        type: "<script>",
        at: "2026-08-15T00:00:00.000Z",
        severity: "high",
        source: "screen",
        durationMs: 5,
        seq: 1,
        prevHash: "",
        hash: "a".repeat(64),
        detail: "<b>stopped</b>",
        sourceEventIds: ["1", "<2>"],
      },
    ],
    hintsUsed: -3,
  });

  assert.equal(report.codingScore, 0, "non-numeric score collapses to 0, never markup");
  assert.equal(report.communicationScore, 100);
  assert.equal(report.decision, "NO_HIRE");
  assert.equal(report.summary, "");
  assert.deepEqual(report.codingFeedback.strengths, ["a", "b", "c", "d"]);
  assert.deepEqual(report.codingFeedback.improvements, []);
  assert.deepEqual(report.communicationFeedback, { strengths: [], improvements: [] });
  assert.equal(report.integrityEvents[0].type, "<script>");
  assert.equal(report.integrityEvents[0].detail, "<b>stopped</b>");
  assert.deepEqual(report.integrityEvents[0].sourceEventIds, ["1"]);
  assert.equal(report.hintsUsed, 0);
  assert.equal(sanitizeReport({ hintsUsed: JSON.parse("1e999") }).hintsUsed, 0, "non-finite counts collapse to 0, like scores");
  assert.equal(sanitizeReport({ hintsUsed: 500 }).hintsUsed, 99, "absurd counts are clamped");
});

// `/api/reports` answers an oversized body with a 413 the UI has nothing to do
// with, so a report has to fit by construction. The shape below is the largest
// one sanitizeReport can produce: every list at its cap, every string at its
// cap, and every character the most expensive one that survives sanitizing.
// Control characters used to be that, at six bytes each once JSON escapes them
// to `\u0001`, but `boundedText` now strips them before render. The dearest
// survivor is an astral character at four UTF-8 bytes per code point, and code
// points are the unit the bounds are counted in, so that is the worst case.
test("sanitizeReport bounds text so the largest possible report still fits", () => {
  const long = "\u{1F3A5}".repeat(50_000);
  const full = { strengths: [long, long, long, long], improvements: [long, long, long, long] };
  const report = sanitizeReport({
    summary: long,
    codingFeedback: full,
    communicationFeedback: full,
    integrityEvents: Array.from({ length: 80 }, (_, index) => ({
      type: long,
      at: long,
      severity: "high",
      source: long,
      durationMs: 1,
      seq: index + 1,
      prevHash: "a".repeat(64),
      hash: "b".repeat(64),
      detail: long,
      sourceEventIds: [long, long, long, long, long],
    })),
  });

  assert.equal(report.codingFeedback.strengths.length, 4);
  assert.equal(report.communicationFeedback.improvements.length, 4);
  // 27: `MAX_INTEGRITY_EVENTS` in src/agent.rs caps the evidence at 25, and the
  // report packet merges the first and last heartbeat in beside it.
  assert.equal(report.integrityEvents.length, 27);
  assert.ok(new TextEncoder().encode(JSON.stringify(report)).length < 64 * 1024, "MAX_REPORT_BYTES in src/web/mod.rs");
});

// A report with nothing in it is stored in localStorage and POSTed to
// /api/reports verbatim, so it comes back through sanitizeReport. Falling
// through to the scored branch would read the missing numbers as 0 and coerce
// the missing decision to NO_HIRE, re-fabricating the rejection one layer later
// than the bug that was just fixed.
test("sanitizeReport keeps an unevaluated session unevaluated", () => {
  const report = sanitizeReport({
    incomplete: true,
    summary: "The interviewer disconnected and this session produced no evaluation.",
    hintsUsed: 0,
  });

  assert.equal(report.incomplete, true);
  assert.equal(report.decision, undefined, "no verdict may be invented on the way out of storage");
  assert.equal(report.codingScore, undefined);
  assert.equal(report.communicationScore, undefined);
  assert.match(report.summary, /produced no evaluation/);

  // And a scored report is untouched by the new branch.
  const scored = sanitizeReport({ codingScore: 82, communicationScore: 74, decision: "HIRE", hintsUsed: 1 });
  assert.equal(scored.incomplete, undefined);
  assert.equal(scored.decision, "HIRE");
});

test("sanitizeReport preserves a well-formed agent report", () => {
  const report = sanitizeReport({
    codingScore: 82,
    communicationScore: 74,
    decision: "HIRE",
    summary: "You solved it.",
    codingFeedback: { strengths: ["clear"], improvements: ["edge cases"] },
    communicationFeedback: { strengths: ["narrated"], improvements: ["slow down"] },
    integrityEvents: [{ type: "SESSION_START", at: "now", severity: "info", source: "media", durationMs: 0, seq: 1, prevHash: "", hash: "a".repeat(64), detail: null, sourceEventIds: [] }],
    integrityChainSeq: 412,
    integrityDropped: 380,
    hintsUsed: 2,
  });

  assert.deepEqual(report, {
    mode: "scored",
    interviewLoop: "coding_behavioral",
    rounds: [],
    codingScore: 82,
    communicationScore: 74,
    decision: "HIRE",
    summary: "You solved it.",
    codingFeedback: { strengths: ["clear"], improvements: ["edge cases"] },
    communicationFeedback: { strengths: ["narrated"], improvements: ["slow down"] },
    integrityEvents: [{ type: "SESSION_START", at: "now", severity: "info", source: "media", durationMs: 0, seq: 1, prevHash: "", hash: "a".repeat(64), detail: null, sourceEventIds: [] }],
    // The event list is one row and the chain reached 412, which is the whole
    // point of carrying these: the list is a subsequence and says so.
    integrityChainSeq: 412,
    integrityDropped: 380,
    hintsUsed: 2,
    improvementPlan: [],
    frameworkAssessment: null,
    frameworkEvidence: [],
  });
});

test("round summaries require plan-consistent kinds budgets and statuses", () => {
  const coding = sanitizeReport({ interviewLoop: "coding_only", rounds: [
    { kind: "coding", budgetMin: 45, status: "complete" },
    { kind: "behavioral", budgetMin: 0, status: "not_configured" },
  ] });
  assert.equal(coding.interviewLoop, "coding_only");
  assert.equal(coding.rounds[0].status, "complete");
  const combined = sanitizeReport({ rounds: [
    { kind: "coding", budgetMin: 37, status: "incomplete" },
    { kind: "behavioral", budgetMin: 8, status: "skipped" },
  ] });
  assert.equal(combined.rounds[1].status, "skipped");
  for (const rounds of [
    [{ kind: "behavioral", budgetMin: 8, status: "started" }, { kind: "coding", budgetMin: 37, status: "complete" }],
    [{ kind: "coding", budgetMin: 37, status: "started" }, { kind: "behavioral", budgetMin: 8, status: "complete" }],
    [{ kind: "coding", budgetMin: 37, status: "complete" }, { kind: "behavioral", budgetMin: 7, status: "complete" }],
  ]) assert.deepEqual(sanitizeReport({ rounds }).rounds, []);
});

test("framework assessment requires ten unique scores or explicit gaps", () => {
  const phases = ["Repeat", "Example", "Algorithm", "Coding", "Test", "Optimizations", "Situation", "Task", "Action", "Result"];
  const base = {
    codingFeedback: { improvements: ["Explain complexity"] },
    communicationFeedback: { improvements: [] },
    improvementPlan: [{ phase: "Algorithm", weakness: "Explain complexity", impact: "high", frequency: 1, drill: "Narrate", durationMin: 5, successCriterion: "Justify bounds", selfReview: ["time"] }],
    frameworkAssessment: {
      rubricVersion: 1,
      phases: phases.map((phase) => ({ phase, score: phase === "Algorithm" ? 72 : null, weaknessTags: phase === "Algorithm" ? ["Explain complexity", "invented"] : [] })),
    },
  };
  const assessment = sanitizeReport(base).frameworkAssessment;
  assert.equal(assessment.phases[2].score, 72);
  assert.deepEqual(assessment.phases[2].weaknessTags, ["Explain complexity"]);
  assert.ok(assessment.phases.slice(6).every((row) => row.score === null));

  const duplicate = structuredClone(base);
  duplicate.frameworkAssessment.phases[9].phase = "Action";
  assert.equal(sanitizeReport(duplicate).frameworkAssessment, null);
  const hostile = structuredClone(base);
  hostile.frameworkAssessment.phases[2].score = "72";
  assert.equal(sanitizeReport(hostile).frameworkAssessment, null);
});

test("framework evidence preserves valid kinds and drops hostile or contradictory rows", () => {
  const report = sanitizeReport({
    frameworkEvidence: [
      { atMs: 9000, phase: "algorithm", source: "candidate_speech", kind: "observed", confidence: 90, summary: "Explained invariant", frameworkVersion: 1, future: "ignored" },
      { atMs: 4000, phase: "test", source: "test_event", kind: "inferred", confidence: 70, summary: "Predicted a boundary", frameworkVersion: 1 },
      { atMs: 12000, phase: "result", source: "session_timing", kind: "skipped", confidence: 100, summary: "Cutoff prevented assessment", frameworkVersion: 1 },
      { atMs: 1, phase: "<script>", source: "candidate_speech", kind: "observed", confidence: 100, summary: "bad", frameworkVersion: 1 },
      { atMs: 2, phase: "action", source: "session_timing", kind: "observed", confidence: 100, summary: "contradiction", frameworkVersion: 1 },
    ],
  });
  assert.deepEqual(report.frameworkEvidence.map((item) => item.kind), ["inferred", "observed", "skipped"]);
  assert.equal(report.frameworkEvidence[1].future, "ignored", "unknown fields round-trip but are never rendered");
  assert.deepEqual(sanitizeReport({}).frameworkEvidence, []);
});

test("invalid framework rows cannot crowd valid evidence out of the cap", () => {
  const hostile = Array.from({ length: 64 }, () => ({ phase: "hostile" }));
  const valid = { atMs: 1, phase: "repeat", source: "candidate_speech", kind: "observed", confidence: 80, summary: "Restated the problem", frameworkVersion: 1 };
  assert.deepEqual(sanitizeReport({ frameworkEvidence: [...hostile, valid] }).frameworkEvidence, [valid]);
});

test("improvement plan drops unrelated and hostile items and ranks valid drills", () => {
  const report = sanitizeReport({
    codingFeedback: { improvements: ["Explain complexity", "Test boundaries"] },
    communicationFeedback: { improvements: ["Name your action"] },
    improvementPlan: [
      { phase: "Test", weakness: "Test boundaries", impact: "low", frequency: 2, drill: "Build a test table", durationMin: 99, successCriterion: "Cover four classes", selfReview: ["boundary"] },
      { phase: "Algorithm", weakness: "Explain complexity", impact: "high", frequency: 3, drill: "Narrate complexity", durationMin: 10, successCriterion: "Justify both bounds", selfReview: ["time", "space"] },
      { phase: "Action", weakness: "Name your action", impact: "medium", frequency: 1, drill: "Rewrite my contribution", durationMin: 5, successCriterion: "Name my decision", selfReview: ["Uses I"] },
      { phase: "Action", weakness: "unrelated", impact: "high", frequency: 9, drill: "bad", durationMin: 5, successCriterion: "bad", selfReview: ["bad"] },
      { phase: "<script>", weakness: "Name your action", impact: "high", frequency: 1, drill: "bad", durationMin: 5, successCriterion: "bad", selfReview: ["bad"] },
    ],
  });
  assert.deepEqual(report.improvementPlan.map((item) => item.phase), ["Algorithm", "Action", "Test"]);
  assert.equal(report.improvementPlan[2].durationMin, 30);
});

test("a partial improvement plan is rejected instead of hiding an emitted weakness", () => {
  const report = sanitizeReport({
    codingFeedback: { improvements: ["Explain complexity", "Test boundaries"] },
    communicationFeedback: { improvements: [] },
    improvementPlan: [{ phase: "Algorithm", weakness: "Explain complexity", impact: "high", frequency: 1, drill: "Narrate", durationMin: 5, successCriterion: "Justify bounds", selfReview: ["time"] }],
  });
  assert.deepEqual(report.improvementPlan, []);
});

test("timer, clamp, and placeholder helpers", () => {
  assert.equal(formatTime(0), "00:00");
  assert.equal(formatTime(65), "01:05");
  assert.equal(formatTime(2700), "45:00");
  assert.equal(clamp(5, 10, 90), 10);
  assert.equal(clamp(500, 10, 90), 90);
  assert.deepEqual(orPlaceholder([]), ["(none captured)"]);
  assert.deepEqual(orPlaceholder(["kept"]), ["kept"]);
});

test("practice pause restores the exact wall-clock time remaining", () => {
  assert.equal(resumeDeadline(60_000, 10_000, 17_500), 67_500);
  assert.equal(67_500 - 17_500, 60_000 - 10_000);
  assert.equal(resumeDeadline(60_000, 17_500, 10_000), 60_000, "a backward clock cannot shorten it");
});

test("report mode accepts only practice and defaults legacy or hostile values to scored", () => {
  assert.equal(sanitizeReport({ incomplete: true, mode: "practice" }).mode, "practice");
  assert.equal(sanitizeReport({ incomplete: true }).mode, "scored");
  assert.equal(sanitizeReport({ incomplete: true, mode: "<script>" }).mode, "scored");
});

// The interview clock. Both assertions here are about a value that jumps: the
// tab is throttled while hidden and stops entirely on an OS suspend, so every
// rule below has to survive skipping straight past the number it cares about.
test("the countdown derives from the deadline rather than accumulating", () => {
  const endsAt = 1_000_000;

  assert.equal(countdown(2700, endsAt, endsAt - 2_700_000).remaining, 2700);
  assert.equal(countdown(45, endsAt, endsAt).remaining, 0);

  // Never negative: an overdue deadline reads as no time left, not as a
  // negative timer counting up.
  assert.equal(countdown(1, endsAt, endsAt + 60_000).remaining, 0);
});

test("the time warning fires on the crossing, not on the number", () => {
  const endsAt = 1_000_000;
  const at = (remaining) => endsAt - remaining * 1000;

  assert.equal(countdown(TIME_WARNING_S + 100, endsAt, at(TIME_WARNING_S)).warn, true);

  // The failure this replaced: a throttled tab skipping from 400 to 240 never
  // equals 300, so an equality test would let the interview run to the end with
  // the agent never told the candidate was near time.
  assert.equal(countdown(400, endsAt, at(240)).warn, true);

  // Once, though. A warning re-sent on every later tick is a nag, and the agent
  // treats each one as news.
  assert.equal(countdown(TIME_WARNING_S, endsAt, at(240)).warn, false);
  assert.equal(countdown(TIME_WARNING_S + 100, endsAt, at(TIME_WARNING_S + 1)).warn, false);

  // `urgent` paints, so unlike `warn` it stays true for the rest of the run.
  assert.equal(countdown(400, endsAt, at(240)).urgent, true);
  assert.equal(countdown(TIME_WARNING_S, endsAt, at(240)).urgent, true);
  assert.equal(countdown(400, endsAt, at(400)).urgent, false);

  assert.equal(countdown(10, endsAt, endsAt).expired, true);
  assert.equal(countdown(10, endsAt, at(1)).expired, false);
});

// The rule that decides whether this browser is allowed to put a hiring verdict
// on the screen. It shipped wrong once, keyed off whether the socket was still
// open, and rendered a green HIRE badge for a network failure.
test("a session that reached an interviewer is never scored by the browser", () => {
  const scored = { joinedRoom: false, passed: 10, total: 10, candidateTurns: 4 };

  // Same passing session, but an interviewer was there: no score, no decision,
  // and it says so rather than showing a blank card.
  const graded = sessionReport({ ...scored, joinedRoom: true });
  assert.equal(graded.incomplete, true);
  assert.equal(graded.decision, undefined);
  assert.equal(graded.codingScore, undefined);
  assert.match(graded.summary, /interviewer never returned a report/);

  // `joinedRoom` outranks everything, including a session that produced
  // nothing: the interviewer was there, so the interviewer grades it.
  assert.match(
    sessionReport({ joinedRoom: true, passed: 0, total: 0, candidateTurns: 0 }).summary,
    /interviewer never returned a report/,
  );

  assert.equal(sessionReport(scored).incomplete, undefined);
  assert.equal(sessionReport(scored).decision, "HIRE");
});

test("offline practice scores only a session that actually did something", () => {
  // Nothing ran and nobody spoke: there is no evidence to score, so this is
  // reported as no evaluation rather than as a 40.
  const empty = sessionReport({ joinedRoom: false, passed: 0, total: 0, candidateTurns: 0 });
  assert.equal(empty.incomplete, true);
  assert.match(empty.summary, /No interviewer joined/);

  // Either one alone is enough to have produced something worth reporting.
  assert.equal(
    sessionReport({ joinedRoom: false, passed: 0, total: 0, candidateTurns: 1 }).incomplete,
    undefined,
  );
  assert.equal(
    sessionReport({ joinedRoom: false, passed: 0, total: 3, candidateTurns: 0 }).incomplete,
    undefined,
  );
});

// Through sessionReport rather than the scorer directly: the guard and the
// scores are one decision, and a test that reaches past the guard would keep
// passing if the guard stopped calling it.
test("the offline decision follows the test cases and speaking follows the turns", () => {
  const at = (passed, total, candidateTurns) =>
    sessionReport({ joinedRoom: false, passed, total, candidateTurns });

  assert.equal(at(10, 10, 1).codingScore, 100);
  assert.equal(at(10, 10, 1).decision, "HIRE");

  // The boundary is inclusive, and it is the only place a pass is decided.
  assert.equal(at(7, 10, 1).codingScore, 70);
  assert.equal(at(7, 10, 1).decision, "HIRE");
  assert.equal(at(69, 100, 1).decision, "NO_HIRE");

  // Being greeted is not communicating: only the candidate's own turns count,
  // which is why this reads turns rather than transcript length.
  assert.equal(at(10, 10, 0).communicationScore, 45);
  assert.equal(at(10, 10, 3).communicationScore, 70);

  // No tests run at all is not a zero, which would read as a failed attempt.
  assert.equal(at(0, 0, 2).codingScore, 40);
  assert.match(at(0, 0, 2).summary, /ended before tests were run/);
});

test("the caption window opens at a sentence boundary, never mid-word", () => {
  const budget = 40;
  const said = "I am Jim. We will do Two Sum today. Which language would you like?";

  assert.equal(captionWindow("I am Jim.", budget), "I am Jim.");
  assert.equal(captionWindow(said, budget), "Which language would you like?");

  // The invariant a character tail breaks. Replay the turn one fragment at a
  // time, the way transcript updates actually arrive, and every window has to
  // be a suffix that starts where a word starts. `...${text.slice(-n)}` fails
  // this on almost every fragment, which is what made the bar unreadable: the
  // opening of the line kept shifting by a character while it was being read.
  for (let end = 1; end <= said.length; end += 1) {
    // trimEnd because the window is trimmed: a fragment that arrives with a
    // trailing space must not read as the window having invented one.
    const grown = said.slice(0, end).trimEnd();
    const window = captionWindow(grown, budget);
    assert.ok(window.length <= budget, `${window.length} > ${budget}: ${window}`);
    assert.ok(!window.startsWith("..."), `slid by character at ${end}: ${window}`);
    assert.ok(grown.endsWith(window), `not a suffix at ${end}: ${window}`);
    const before = grown[grown.length - window.length - 1];
    assert.ok(
      before === undefined || /\s/.test(before),
      `opened mid-word at ${end}: ...${before}|${window}`,
    );
  }

  // Budgets too small for the ellipsis drop it rather than overrun. Nothing
  // calls it this way today, but a function that silently exceeds the one
  // number it is given is a bad neighbour, and `slice(-0)` returning the whole
  // string makes zero its own case.
  for (const tiny of [0, 1, 2, 3, 4]) {
    const out = captionWindow("a".repeat(50), tiny);
    assert.ok(out.length <= tiny, `maxChars=${tiny} produced ${out.length}`);
  }

  // No boundary anywhere to fall back on: still shows the tail, because an
  // empty caption bar is worse than one that starts mid-word. The ellipsis is
  // rendered, so it counts against the budget rather than being bolted on
  // outside it: this used to return three characters more than it was given.
  const fallback = captionWindow("a".repeat(200), budget);
  assert.ok(fallback.startsWith("..."), fallback);
  assert.ok(fallback.length <= budget, `${fallback.length} > ${budget}`);

  // A decimal is not a sentence boundary, or the window opens mid-number.
  const decimal = `${"x".repeat(50)}. It runs in 3.14 seconds flat.`;
  assert.equal(captionWindow(decimal, budget), "It runs in 3.14 seconds flat.");

  // The boundary at the very end of the text opens a sentence with nothing in
  // it. Taking it blanked the bar the moment a long line was finished, which is
  // when there is most to read, and the trailing space is why the loop above
  // never sees this.
  for (const finished of [`${"x".repeat(50)}. `, `${"x".repeat(50)}...`]) {
    const window = captionWindow(finished, budget);
    assert.ok(window.trim().length > 0, `blanked the bar on ${JSON.stringify(finished)}`);
    assert.ok(window.length <= budget, `${window.length} > ${budget}`);
  }
});
