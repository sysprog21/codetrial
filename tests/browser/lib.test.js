// Run with: node --test tests/browser/*.test.js
// Covers the browser judge/report contracts that used to be asserted by
// substring-matching web/interview.js from Rust.

import { test } from "node:test";
import assert from "node:assert/strict";
import { read } from "./source.js";

import {
  ACTIVE_CONTRACT,
  FRAMEWORKS,
  frameworkChecklist,
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
  providerUiState,
  renderValue,
  replayTimeline,
  responseWindowLabel,
  responseWindows,
  sanitizeReport,
  sessionReport,
  testPayload,
  timeWarningPayload,
  topics,
} from "../../web/lib.js";

test("provider degradation states distinguish availability and evaluation truth", () => {
  const expected = {
    connecting: [false, false], live: [true, false], reconnecting: [true, false],
    interviewer_reconnecting: [true, false],
    degraded: [false, true], report_generating: [true, false],
    incomplete_report: [false, true], retry_ready: [false, true],
  };
  for (const [kind, [personalized, retry]] of Object.entries(expected)) {
    const state = providerUiState(kind, "provider refused\nsecret");
    assert.equal(state.personalized, personalized, kind);
    assert.equal(state.retry, retry, kind);
    assert.ok(state.label && state.message, kind);
    assert.doesNotMatch(state.message, /\n/);
  }
  assert.match(providerUiState("degraded").message, /will not create a personalized evaluation/);
  assert.doesNotMatch(providerUiState("degraded", "https:\/\/key:secret@example.test").message, /secret|example/);
  assert.match(providerUiState("degraded", "429 Too Many Requests").message, /busy or rate limited/);
  assert.match(providerUiState("incomplete_report").message, /No scores or verdict were created/);
  assert.notEqual(providerUiState("live").message, providerUiState("reconnecting").message);
});

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
  // the Greek is 70, "ab" is 2, so the pair occupies units 79 and 80. A
  // `.slice(0, 80)` keeps unit 79 and drops its partner. The letters are a
  // fixture chosen for byte and unit width, not for any language: each is one
  // UTF-16 unit and two bytes, and the emoji is the pair that can be split.
  const label = "αβγδε".repeat(14) + "ab\u{1F3A5}tail";
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
  assert.ok(event.detail.startsWith("camera=αβγδε"), event.detail);
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
    // Every report the agent writes names its loop, so a well-formed one has
    // it here. Absence means a report written before loops existed, and that
    // is the one case sanitizeReport must not invent a value for.
    interviewLoop: "coding_behavioral",
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
    interviewContract: null,
    mode: undefined,
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

test("report contract migration preserves legacy and rejects unknown provenance", () => {
  const legacy = sanitizeReport({ incomplete: true, summary: "old report" });
  assert.equal(legacy.interviewContract, null);
  assert.equal(legacy.summary, "old report");

  const active = {
    bundleVersion: 4,
    livePromptVersion: 1,
    reportPromptVersion: 4,
    rubricVersion: 1,
    reportSchemaVersion: 1,
  };
  assert.deepEqual(sanitizeReport({ incomplete: true, interviewContract: active }).interviewContract, active);

  for (const interviewContract of [
    { ...active, reportSchemaVersion: 2 },
    { ...active, rubricVersion: "1" },
    { ...active, extra: 1 },
    null,
  ]) {
    const report = sanitizeReport({ codingScore: 99, decision: "HIRE", interviewContract });
    assert.equal(report.incomplete, true);
    assert.match(report.summary, /unsupported or malformed interview contract/);
    assert.equal("codingScore" in report, false);
  }
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

  const protoField = sanitizeReport(JSON.parse(`{"frameworkEvidence":[{
    "atMs":1000,"phase":"algorithm","source":"candidate_speech","kind":"observed",
    "confidence":90,"summary":"Explained invariant","frameworkVersion":1,
    "__proto__":"newer report field"
  }]}`)).frameworkEvidence[0];
  assert.equal(Object.hasOwn(protoField, "__proto__"), true);
  assert.equal(protoField.__proto__, "newer report field");

  // Round-tripping them is for a newer client's fields, not for a payload.
  // The account sync refuses an oversized report outright rather than
  // trimming it, so one unbounded field here costs the whole account copy.
  const huge = sanitizeReport({
    frameworkEvidence: [{
      atMs: 1000, phase: "algorithm", source: "candidate_speech", kind: "observed",
      confidence: 90, summary: "Explained invariant", frameworkVersion: 1,
      future: "x".repeat(4096),
    }],
  });
  assert.equal(huge.frameworkEvidence.length, 1, "the row itself is still kept");
  assert.equal(huge.frameworkEvidence[0].future, undefined, "the payload is not");
  assert.equal(huge.frameworkEvidence[0].summary, "Explained invariant");
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

test("report mode is kept only where a report actually recorded one", () => {
  // Reports written before the practice/scored split still say which they were
  // and the viewer shows it. Nothing produces one now, so a report without one
  // must not have a mode invented for it: the header would announce a
  // distinction that no longer exists.
  assert.equal(sanitizeReport({ incomplete: true, mode: "practice" }).mode, "practice");
  assert.equal(sanitizeReport({ incomplete: true }).mode, undefined);
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

  const offline = sessionReport(scored);
  assert.equal(offline.incomplete, true);
  assert.equal(offline.decision, undefined);
  assert.equal(offline.codingScore, undefined);
  assert.equal(offline.frameworkAssessment, undefined);
  assert.match(offline.summary, /no personalized scores, verdict, or feedback/);
});

test("offline practice reports local activity without fabricated evaluation", () => {
  // Nothing ran and nobody spoke: there is no evidence to score, so this is
  // reported as no evaluation rather than as a 40.
  const empty = sessionReport({ joinedRoom: false, passed: 0, total: 0, candidateTurns: 0 });
  assert.equal(empty.incomplete, true);
  assert.match(empty.summary, /No interviewer joined/);

  for (const report of [
    sessionReport({ joinedRoom: false, passed: 0, total: 0, candidateTurns: 1 }),
    sessionReport({ joinedRoom: false, passed: 2, total: 3, candidateTurns: 0 }),
  ]) {
    assert.equal(report.incomplete, true);
    for (const key of ["codingScore", "communicationScore", "decision", "codingFeedback", "communicationFeedback", "frameworkAssessment"]) {
      assert.equal(report[key], undefined, `${key} must not be fabricated offline`);
    }
  }
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

test("the two frameworks stay apart and tick only what the interviewer banked", () => {
  // Never both at once. A candidate in the coding round is working through
  // REACTO, and the four behavioral steps are not theirs to think about yet.
  const coding = frameworkChecklist("coding", ["repeat", "algorithm"]);
  assert.equal(coding.name, "REACTO");
  assert.deepEqual(coding.steps.map((step) => step.id), ["repeat", "example", "algorithm", "coding", "test", "optimizations"]);
  assert.deepEqual(coding.steps.filter((step) => step.done).map((step) => step.id), ["repeat", "algorithm"]);

  const behavioral = frameworkChecklist("behavioral", ["situation", "result"]);
  assert.equal(behavioral.name, "STAR");
  assert.deepEqual(behavioral.steps.map((step) => step.id), ["situation", "task", "action", "result"]);
  assert.deepEqual(behavioral.steps.filter((step) => step.done).map((step) => step.id), ["situation", "result"]);

  // The packet is untrusted like every other. An unknown phase is a version
  // skew or someone else's idea of a step, and neither belongs on screen.
  const hostile = frameworkChecklist("coding", ["repeat", "situation", "<script>", 7, null]);
  assert.deepEqual(hostile.steps.filter((step) => step.done).map((step) => step.id), ["repeat"]);

  // An unknown round falls back rather than rendering an empty list, because a
  // checklist with no steps reads as an interview with nothing to do.
  assert.equal(frameworkChecklist("nonsense", []).name, "REACTO");
  assert.deepEqual(frameworkChecklist("coding", "not-a-list").steps.filter((step) => step.done), []);

  // Every step explains itself. The card is read once, while waiting, so a
  // label with no clause behind it is a step the candidate cannot act on.
  for (const framework of Object.values(FRAMEWORKS)) {
    for (const step of framework.steps) {
      assert.ok(step.hint && step.hint.length > 10, `${step.id} has no usable explanation`);
    }
  }

  // Each framework says which exercise it is for, so a table of one can stand
  // without borrowing context from a table of the other.
  for (const framework of Object.values(FRAMEWORKS)) {
    assert.ok(framework.scenario && framework.scenario.length > 20, `${framework.name} does not say what it scores`);
  }

  // No id appears in both, or a tick in one round would light up the other.
  const ids = Object.values(FRAMEWORKS).flatMap((framework) => framework.steps.map((step) => step.id));
  assert.equal(new Set(ids).size, ids.length);
});

// The response window: the gap between the interviewer finishing a question and
// the interviewer speaking again.
//
// The shapes below have one test each, because `recordAvatarState` writes only
// on a change and an interview can stop anywhere. They are all about what the
// function refuses to invent: a duration it cannot measure, a window it cannot
// close, a turn nobody said, and a window in front of a question nobody asked.
//
// The fixtures divide in two, and the division is the point. Ones built from
// `listening` and `speaking` alone are sequences a real recording can hold,
// because those are the two states `src/livekit.rs` publishes. Ones containing
// `thinking` are reachable only by a deployment that publishes it, and are here
// to pin that the code still honors it; a suite made only of those asserted a
// property of its own fixtures through three review rounds.
//
// `at` is the candidate's own browser clock throughout. Ordering is the array's,
// which is the server's `seq` order, and `at` is read for the duration and for
// nothing else, which is the same rule `src/recording/replay.rs` applies to its
// own rows.
const avatar = (at, state, responseWindow) => ({
  kind: "avatar",
  at,
  payload: { state, ...(responseWindow === undefined ? {} : { responseWindow }) },
});
const life = (at, state) => ({ kind: "lifecycle", at, payload: { state } });
const said = (at, speaker, text, responseWindow) => ({
  kind: "transcript",
  at,
  payload: { speaker, text, ...(responseWindow === undefined ? {} : { responseWindow }) },
});

test("a response window pairs listening with the next close", () => {
  const windows = responseWindows([
    { kind: "editor", at: 900, payload: { code: "" } },
    avatar(1000, "speaking"),
    avatar(2000, "listening"),
    said(6000, "you", "I would use a hash map"),
    avatar(6500, "thinking"),
    avatar(7000, "speaking"),
  ]);
  assert.equal(windows.length, 1);
  assert.equal(windows[0].at, 2000);
  assert.equal(windows[0].duration, 4500);
  assert.deepEqual(windows[0].turn, { at: 6000, text: "I would use a hash map" });
  // The position of the row that opened it, so the page can put the window
  // beside the moment it happened over without re-deriving the order from `at`.
  assert.equal(windows[0].index, 2);

  // The interviewer's own lines are not the candidate's answer. A candidate turn
  // recorded after the window closed still belongs to it, until the interviewer
  // asks again: that is the delivery race the next test is about, and the last
  // turn before a new question is the one that closed the old one.
  const later = responseWindows([
    avatar(0, "speaking"),
    avatar(100, "listening", 0),
    said(1000, "interviewer", "and the complexity?"),
    said(2000, "you", "linear"),
    avatar(3000, "thinking"),
    said(4000, "you", "well, amortized", 0),
  ]);
  assert.deepEqual(later[0].turn, { at: 4000, text: "well, amortized" });

  // The last candidate turn inside the window, not the first: a turn is
  // recorded when its stream closes, so the one that closed the window is the
  // last to land before the interviewer thought.
  const twice = responseWindows([
    avatar(0, "speaking"),
    avatar(100, "listening"),
    said(1000, "you", "hmm"),
    said(2000, "you", "a hash map"),
    avatar(3000, "thinking"),
  ]);
  assert.equal(twice[0].turn.text, "a hash map");
});

test("a response window that never closed keeps a null duration", () => {
  // The interview stopped mid-answer. The window is still worth showing, so it
  // is returned rather than dropped, and the duration is absent rather than
  // measured to the end of the array.
  const windows = responseWindows([
    avatar(0, "speaking"),
    avatar(1000, "listening", 0),
    said(4000, "you", "let me think"),
  ]);
  assert.equal(windows.length, 1);
  assert.equal(windows[0].at, 1000);
  assert.equal(windows[0].duration, null);
  assert.deepEqual(windows[0].turn, { at: 4000, text: "let me think" });

  // And an interview with no `avatar` rows at all has no windows rather than
  // one that spans it.
  assert.deepEqual(responseWindows([said(1, "you", "hello")]), []);
  assert.deepEqual(responseWindows([]), []);
  assert.deepEqual(responseWindows(undefined), []);
});

test("a response window holding no candidate turn is kept with a null turn", () => {
  // The interviewer asked, waited, and asked again. That is exactly the shape
  // worth seeing, so the window survives with nothing in it rather than being
  // dropped for being empty.
  const windows = responseWindows([
    avatar(0, "speaking"),
    avatar(1000, "listening"),
    avatar(9000, "thinking"),
    avatar(9500, "speaking"),
    avatar(10_000, "listening", 1),
    said(12_000, "you", "sorry, could you repeat that", 1),
    avatar(13_000, "thinking"),
  ]);
  assert.equal(windows.length, 2);
  assert.equal(windows[0].duration, 8000);
  assert.equal(windows[0].turn, null);
  assert.equal(windows[1].duration, 3000);
  assert.equal(windows[1].turn.text, "sorry, could you repeat that");

  // A transcript before any window opened belongs to no window.
  const early = responseWindows([
    said(0, "you", "before anything"),
    avatar(500, "speaking"),
    avatar(1000, "listening"),
    avatar(2000, "thinking"),
  ]);
  assert.equal(early[0].turn, null);
});

test("a re-prompt closes the unanswered window and opens its own", () => {
  // This case was specified the other way round, as "a repeated listening starts
  // no second window", on the reasoning that restarting the clock at the
  // re-prompt would report the shorter of two gaps as the whole of it. That
  // reasoning does not survive what the server publishes. `src/livekit.rs`
  // writes only `listening` and `speaking`, so `speaking` is what closes a
  // window; a re-prompt therefore closes the ask nobody answered and opens the
  // one they did. Two windows, and the first shows no candidate transcript,
  // which is the shape that case existed to make visible. Folding them into one
  // long window would have hidden the unanswered ask inside the answered one.
  const windows = responseWindows([
    avatar(500, "speaking"),
    avatar(1000, "listening"),
    avatar(2000, "speaking"),
    avatar(3000, "listening", 1),
    said(4000, "you", "got it", 1),
    avatar(5000, "speaking"),
  ]);
  assert.equal(windows.length, 2);
  assert.deepEqual(
    windows.map((span) => [span.at, span.duration, span.turn?.text ?? null]),
    [
      [1000, 1000, null],
      [3000, 2000, "got it"],
    ],
  );

  // A window that closed is closed: another close does not reopen it, and a
  // `listening` the interviewer was not speaking before opens nothing.
  const stray = responseWindows([
    avatar(500, "speaking"),
    avatar(1000, "listening", 0),
    avatar(2000, "thinking"),
    avatar(3000, "thinking"),
    avatar(4000, "listening"),
  ]);
  assert.equal(stray.length, 1, "a listening after a thinking follows no question");
  assert.equal(stray[0].duration, 1000);

  // An `avatar` state nothing here pairs on neither opens nor closes anything.
  assert.deepEqual(responseWindows([avatar(0, "initializing"), avatar(1, "speaking")]), []);
});

test("a thinking before its listening measures nothing rather than a negative", () => {
  // A `thinking` stamped before the `listening` it closes is not a negative
  // duration. It is a clock that moved, and there is nothing a reader can be
  // told about a negative second.
  const backwards = responseWindows([
    avatar(0, "speaking"),
    avatar(9000, "listening"),
    avatar(4000, "thinking"),
  ]);
  assert.equal(backwards.length, 1);
  assert.equal(backwards[0].duration, null);

  // Zero is a measurement and stays one.
  assert.equal(
    responseWindows([avatar(0, "speaking"), avatar(9000, "listening"), avatar(9000, "thinking")])[0]
      .duration,
    0,
  );

  // A payload with no clock at all measures nothing rather than NaN.
  const clockless = responseWindows([
    { kind: "avatar", at: 0, payload: { state: "speaking" } },
    { kind: "avatar", payload: { state: "listening" } },
    { kind: "avatar", at: 5000, payload: { state: "thinking" } },
  ]);
  assert.equal(clockless[0].duration, null);
});

test("a replay body that is not a list is an empty replay, not a throw", () => {
  // The page reads `body.events || []` off a parsed JSON body on the path where
  // the response was already `ok`. A string or an object there used to reach
  // `.entries()` and throw, which paints a blank panel where the page had a
  // sentence ready to explain itself.
  for (const nonsense of [undefined, null, "events", 5, {}, { events: [] }]) {
    assert.deepEqual(responseWindows(nonsense), [], JSON.stringify(nonsense));
    assert.deepEqual(replayTimeline(nonsense), { moments: [], windows: [], timeline: [] });
  }

  // A transcript with nothing in it is not a turn. Our own producer guards on
  // `text.trim()` and cannot emit one, which is exactly why nothing downstream
  // would notice if a later one did: the window would silently lose the note
  // saying it holds no transcript.
  for (const empty of [{}, { speaker: "you" }, { speaker: "you", text: "   " }, { speaker: "you", text: { a: 1 } }]) {
    const windows = responseWindows([
      avatar(0, "speaking"),
      avatar(100, "listening"),
      { kind: "transcript", at: 1000, payload: empty },
      avatar(2000, "thinking"),
    ]);
    assert.equal(windows[0].turn, null, JSON.stringify(empty));
  }

  // A null element is skipped rather than read.
  assert.equal(
    responseWindows([null, avatar(0, "speaking"), avatar(1, "listening"), undefined]).length,
    1,
  );
});

test("the replay timeline interleaves windows with the moments without joining them", () => {
  // The whole reason the windows are a separate list: `data-moment` indexes
  // `moments` and the page rebuilds the code and the test result by scanning its
  // prefix, so a window mixed into that array would shift every index and then
  // be scanned over as neither an editor nor a test snapshot.
  const editor = (at, code) => ({ kind: "editor", at, payload: { code } });
  const tests = (at, passed) => ({ kind: "tests", at, payload: { passed, total: 5 } });
  const events = [
    avatar(-100, "speaking"),
    avatar(0, "listening"),
    editor(100, "first"),
    said(200, "you", "a hash map"),
    avatar(300, "thinking"),
    tests(400, 3),
    avatar(500, "speaking"),
    avatar(600, "listening"),
    editor(700, "second"),
  ];
  const { moments, windows, timeline } = replayTimeline(events);

  assert.deepEqual(
    moments.map((moment) => moment.kind),
    ["editor", "tests", "editor"],
    "moments are editor and tests snapshots, and nothing else",
  );
  assert.equal(windows.length, 2);

  // Interleaved in seq order, not appended after each other. Both an "append
  // every window at the end" assembly and a "prepend them" one produce a
  // different list here, which source-text assertions could not tell apart.
  assert.deepEqual(timeline, [
    { window: 0 },
    { moment: 0 },
    { moment: 1 },
    { window: 1 },
    { moment: 2 },
  ]);

  // A replay with no avatar rows has no windows, so the page has nothing to
  // explain and hides the paragraph explaining it.
  assert.deepEqual(replayTimeline([editor(0, "only")]).windows, []);
});

test("the contract this build scores is the shape sanitizeReport accepts", () => {
  // The reason `ACTIVE_CONTRACT` is exported rather than function-local. While
  // it lived inside `sanitizeReport`, nothing could read it, and moving it left
  // the whole suite green with the supported-contract branch no longer
  // rendering: a precondition nobody checks is a precondition that goes.
  //
  // Not the versions themselves. `the_interview_contract_is_the_same_bundle_on_
  // both_sides` in `tests/agent.rs` already compares the whole object against
  // the server's, which is the stronger check because it is the disagreement
  // that matters. What is left for here is the shape, which that test reads
  // rather than asserts.
  assert.deepEqual(
    Object.keys(ACTIVE_CONTRACT).sort(),
    ["bundleVersion", "livePromptVersion", "reportPromptVersion", "reportSchemaVersion", "rubricVersion"],
    "five versions, and a sixth added without a migration fails here",
  );
  // The bounds `reportContract` enforces on a report's own bundle. A build
  // whose active contract could not pass its own validator would refuse every
  // report including the ones it just produced.
  for (const [key, value] of Object.entries(ACTIVE_CONTRACT)) {
    assert.ok(
      Number.isSafeInteger(value) && value >= 1 && value <= 999,
      `${key} is in the range sanitizeReport accepts`,
    );
  }
});

test("the window label says only what its words table gives it", () => {
  // The panel may not accuse, and this is the form of that rule a test can
  // observe. Sentinels rather than the real copy: anything in the output that
  // the table did not put there is prose the renderer invented, whether it came
  // from a literal, a helper, a computed lookup, or a second table declared
  // beside the first. All four were written during review and all four passed a
  // version of this check that read the renderer's source instead.
  // One sentinel per key, read off `WINDOW_WORDS` in web/replay.js rather than
  // listed here, so a key added there without a sentinel fails instead of going
  // unchecked. Listing them was the earlier version, and the comment claimed
  // this while the file did the other thing.
  const table = read("web/replay.js");
  const keys = [
    ...table
      .slice(table.indexOf("const WINDOW_WORDS = {"), table.indexOf("\n};", table.indexOf("const WINDOW_WORDS = {")))
      .matchAll(/^\s{2}(\w+):/gm),
  ].map((match) => match[1]);
  assert.ok(keys.length >= 5, `only ${keys.length} words found in WINDOW_WORDS`);
  const sentinels = Object.fromEntries(keys.map((key, index) => [key, `Q${index}q`]));
  // Every field of a window crossed with every other, rather than the cells
  // somebody thought of. Enumerating by hand missed the no-duration-with-a-turn
  // cell once, and then missed `at` entirely: every span was `at: 0`, so a
  // branch on `span.at > 0`, which is true of every window in every real
  // recording, put a label on the page that this test could not see. A product
  // is the only version of "all of them" that stays true when a field is added.
  const spans = [];
  for (const at of [0, 1_770_000_000_000]) {
    for (const duration of [null, 0, 4200, 12_345_678]) {
      for (const turn of [null, { at: at + 1, text: "a hash map" }]) {
        for (const isPaused of [false, true]) {
          for (const matched of [false, true]) {
            spans.push({ at, duration, turn, paused: isPaused, matched });
          }
        }
      }
    }
  }
  for (const span of spans) {
    const parts = responseWindowLabel(span, sentinels);
    let rest = parts.join("|");
    for (const value of Object.values(sentinels)) rest = rest.split(value).join("");
    // What survives is a clock reading and the separators between it, which is
    // the only thing in the label that is not a word somebody chose. Stricter
    // than "no letters", and implying it.
    assert.doesNotMatch(
      rest,
      /[^0-9.|]/,
      `the label said something its words table did not give it: ${parts.join("|")}`,
    );
  }

  // The span's own shape, frozen. The fixtures above are built by hand, so a
  // field added to a window in `responseWindows` and read in
  // `responseWindowLabel` is a field this test never sets and never sees: a
  // `verdict` written that way put prose on every long window and passed the
  // whole suite. Keys read off a real window rather than listed, so adding one
  // fails here and is a decision somebody makes.
  // The union over every shape a window comes in, not one sample. Freezing the
  // keys of a normally-closed window left a field that only a paused or an
  // unclosed one carries invisible, which is the same "no fixture sets it" hole
  // one layer up.
  const shapes = [
    [avatar(0, "speaking"), avatar(1000, "listening"), said(2000, "you", "x"), avatar(3000, "speaking")],
    [avatar(0, "speaking"), avatar(1000, "listening")],
    [
      avatar(0, "speaking"),
      life(500, "paused"),
      avatar(1000, "listening"),
      avatar(9000, "speaking"),
    ],
    [avatar(0, "speaking"), avatar(9000, "listening"), avatar(1000, "speaking")],
  ];
  const real = shapes.flatMap((rows) => responseWindows(rows));
  assert.ok(real.length >= shapes.length, "every shape produced a window to read keys off");
  assert.deepEqual(
    [...new Set(real.flatMap((span) => Object.keys(span)))].sort(),
    ["at", "duration", "index", "matched", "paused", "turn"],
    "a field added to a window is one the sentinels above never set and never see",
  );
  assert.deepEqual(
    Object.keys(spans[0]).sort().filter((key) => key !== "index"),
    Object.keys(real[0]).sort().filter((key) => key !== "index"),
    "and the sentinel spans carry every field a real one does",
  );

  // Read by name below rather than by position, so the assertions survive the
  // words table being reordered.
  // Each optional piece appears exactly when its case holds, so a window is not
  // silently missing the one caveat it needed.
  const label = (span) => responseWindowLabel(span, sentinels);
  const turn = { at: 1, text: "a hash map" };
  assert.deepEqual(label({ at: 1, duration: 4200, turn }), [sentinels.label, `4.2${sentinels.seconds}`]);
  assert.deepEqual(label({ at: 1, duration: 0, turn }), [sentinels.label, `0.0${sentinels.seconds}`]);
  assert.deepEqual(
    label({ at: 1, duration: null, turn: null, matched: true }),
    [sentinels.label, sentinels.unmeasured, sentinels.noTurn],
  );
  assert.deepEqual(label({ at: 1, duration: null, turn }), [sentinels.label, sentinels.unmeasured]);
  assert.deepEqual(
    label({ at: 1, duration: 300_000, turn: null, paused: true, matched: true }),
    [sentinels.label, `300.0${sentinels.seconds}`, sentinels.paused, sentinels.noTurn],
    "a paused window says so before it says nothing was recorded",
  );

  // The two shapes an empty window comes in are two different sentences, and
  // which one is chosen is the whole of the fix: a window a transcript could
  // have named and did not is a fact about the interview, and a window nothing
  // could have named is a fact about the recording. Asserted as a pair, because
  // a renderer that reached for one word for both would pass either half alone.
  assert.deepEqual(
    label({ at: 1, duration: 4200, turn: null, matched: true }),
    [sentinels.label, `4.2${sentinels.seconds}`, sentinels.noTurn],
    "a window that could be named and was not says nothing was recorded",
  );
  assert.deepEqual(
    label({ at: 1, duration: 4200, turn: null, matched: false }),
    [sentinels.label, `4.2${sentinels.seconds}`, sentinels.unmatchedTurn],
    "a window nothing could have named says that instead",
  );
});

test("a turn recorded after the interviewer moved on still closes its window", () => {
  // Two independent deliveries. `recordAvatarState` fires on the agent's state
  // attribute and `consumeTranscript` records the turn only when its text stream
  // finishes, so the row saying what the candidate said routinely lands after
  // the row saying the interviewer had moved on. Attributing only inside the
  // window printed "no transcript in this window" beside ordinary answered
  // questions, which is the one wrong statement this panel can make: every other
  // caveat is a number withheld, and that one is a claim about the record.
  const windows = responseWindows([
    avatar(500, "speaking"),
    avatar(1000, "listening", 0),
    avatar(4000, "thinking"),
    said(4200, "you", "a hash map", 0),
    avatar(4500, "speaking"),
  ]);
  assert.equal(windows.length, 1);
  assert.equal(windows[0].duration, 3000, "the duration is still the two avatar rows");
  assert.equal(windows[0].turn.text, "a hash map");

  // That a turn cannot reach back past a question the interviewer has since
  // asked is the same fixture as "a response window holding no candidate turn is
  // kept with a null turn" above, and is asserted there.

  // The interviewer's own line after a window closed is still not the
  // candidate's.
  const theirs = responseWindows([
    avatar(0, "speaking"),
    avatar(100, "listening"),
    avatar(1000, "thinking"),
    said(2000, "interviewer", "let me rephrase"),
  ]);
  assert.equal(theirs[0].turn, null);

  const legacy = responseWindows([
    avatar(0, "speaking"),
    avatar(100, "listening"),
    avatar(1000, "thinking"),
    said(2000, "you", "goodbye"),
  ]);
  assert.equal(legacy[0].turn, null, "a markerless row after the close is ambiguous");
});

test("a delayed turn is matched by when its stream started", () => {
  const events = [
    avatar(0, "speaking"),
    avatar(1000, "listening", 0),
    avatar(5000, "speaking"),
    avatar(6000, "listening", 1),
    said(7000, "you", "the first answer", 0),
    avatar(8000, "speaking"),
  ];
  assert.deepEqual(
    responseWindows(events).map((span) => span.turn?.text ?? null),
    ["the first answer", null],
  );

  events[4] = { kind: "transcript", at: 7000, payload: { speaker: "you", text: "legacy" } };
  assert.deepEqual(
    responseWindows(events).map((span) => span.turn?.text ?? null),
    [null, null],
    "an old multi-window replay does not guess which question a late row answered",
  );

  events[4] = said(7000, "you", "the first answer", 0);
  events.splice(0, 2);
  assert.deepEqual(
    responseWindows(events).map((span) => span.turn?.text ?? null),
    [null],
    "a missing earlier window cannot shift its transcript onto the next visible one",
  );
});

test("windows are computed from the two states the server actually publishes", () => {
  // The whole feature rested on a state nothing emits. `src/livekit.rs` declares
  // `listening` and `speaking` and every `set_agent_state` call writes one of
  // them; `thinking` is a label in the browser and a pose on the avatar and is
  // never published. A window that closed only on `thinking` never closed, so a
  // real interview drew exactly one row reading "duration not recorded" however
  // many questions it contained. This fixture is the sequence a real recording
  // holds, and it must not contain a `thinking` row.
  const recorded = [
    avatar(0, "listening"),
    avatar(1000, "speaking"),
    avatar(20_000, "listening"),
    said(45_000, "you", "linear"),
    avatar(47_000, "speaking"),
    avatar(60_000, "listening", 1),
    said(70_000, "you", "a hash map", 1),
    avatar(72_000, "speaking"),
  ];
  // Read out of the server rather than asserted of the fixture. A check that
  // `recorded` holds no `thinking` cannot fail from any change to
  // `src/livekit.rs`, so it would not have caught the bug it was written after:
  // the browser paired on a state the server never publishes, and every fixture
  // in this file fed it. This is the cross-language pin `INTEGRITY_DETAIL_MAX`
  // already uses in the other direction, and it fails the day a third state is
  // declared without `responseWindows` being taught what it means.
  const livekit = read("src/livekit.rs");
  // Anchored loosely enough to catch a third state however it is declared: a
  // pattern that only matched the exact current spelling would fail open on a
  // `pub const`, which is the direction a pin must never fail.
  const declared = [...livekit.matchAll(/const\s+(AGENT_STATE_\w+)\s*:\s*&str\s*=\s*"(\w+)"/g)];
  const published = new Set(declared.map((match) => match[2]));
  assert.deepEqual(published, new Set(["listening", "speaking"]));
  // And nothing publishes a state that is not one of those constants. Matched
  // across newlines and past a trailing comma, because rustfmt wraps a long call
  // and this file already contains one wrapped that way: a pattern requiring the
  // close paren on the same line matched no wrapped call at all, so a fourth
  // state added in the shape the formatter produces failed this open.
  const publishes = [...livekit.matchAll(/(?<!fn )set_agent_state\(([^{};]*?)\)\s*\.await/g)].map(
    (match) =>
      match[1]
        .split(",")
        .map((argument) => argument.trim())
        .filter(Boolean)
        .at(-1),
  );
  assert.ok(publishes.length >= declared.length, `only ${publishes.length} publishes found`);
  assert.deepEqual(
    new Set(publishes),
    new Set(declared.map((match) => match[1])),
    "every publish names one of the declared constants",
  );
  for (const state of published) {
    assert.ok(
      recorded.some((event) => event.payload?.state === state),
      `the fixture has to exercise ${state}, which is a state the server sends`,
    );
  }
  assert.ok(
    !recorded.some((event) => !published.has(event.payload?.state ?? "listening")),
    "and only states the server sends, which is the property this test exists for",
  );
  assert.deepEqual(
    responseWindows(recorded).map((span) => [span.at, span.duration, span.turn?.text ?? null]),
    [
      [20_000, 27_000, "linear"],
      [60_000, 12_000, "a hash map"],
    ],
    "one window per question, each with a duration and the answer that closed it",
  );

  // `thinking` still closes a window, for a deployment that publishes it, and
  // closes it earlier than the reply would: the difference is the model round
  // trip that closing on `speaking` puts inside the number.
  assert.equal(
    responseWindows([
      avatar(0, "speaking"),
      avatar(1000, "listening"),
      avatar(3000, "thinking"),
      avatar(5000, "speaking"),
    ])[0].duration,
    2000,
    "whichever close comes first is the one that counts",
  );
});

test("no window opens before the interviewer has been heard speaking", () => {
  // `updateAgentState` in web/interview.js runs on first sight of the agent
  // participant and records `published || "listening"`, so the first `avatar`
  // row of almost every real interview is a `listening` written before a
  // question exists. Opened on, it swallowed the greeting and the first
  // question and reported the interviewer's own speech as elapsed response
  // time, which is the one number on this panel a reviewer would act on.
  const greeting = responseWindows([
    avatar(0, "listening"),
    avatar(1000, "speaking"),
    avatar(60_000, "listening"),
    said(64_000, "you", "a hash map"),
    avatar(65_000, "thinking"),
  ]);
  assert.equal(greeting.length, 1, "the connect-time listening is not a question ending");
  assert.equal(greeting[0].at, 60_000);
  assert.equal(greeting[0].duration, 5000, "and the greeting is not part of the answer");

  // An interviewer that never speaks leaves nothing to measure a gap after,
  // however many states it publishes.
  assert.deepEqual(responseWindows([avatar(0, "listening")]), []);
  assert.deepEqual(
    responseWindows([avatar(0, "listening"), avatar(1, "thinking"), avatar(2, "listening")]),
    [],
  );

  // The previous row, not a latch. An earlier version remembered only that the
  // interviewer had spoken at some point, which admitted the `listening` after a
  // `thinking`: the interviewer thought, said nothing, and went back to waiting,
  // and a window opened in front of a question nobody asked. These two sequences
  // differ only in that row and have to differ in their answer, which is exactly
  // what a latch cannot do and why one is not used.
  const asked = responseWindows([
    avatar(0, "speaking"),
    avatar(100, "listening"),
    avatar(200, "thinking"),
    avatar(300, "speaking"),
    avatar(400, "listening"),
    avatar(500, "speaking"),
  ]);
  assert.deepEqual(
    asked.map((span) => [span.at, span.duration]),
    [
      [100, 100],
      [400, 100],
    ],
    "each listening follows a speaking, so each is a question ending",
  );
  const unasked = responseWindows([
    avatar(0, "speaking"),
    avatar(100, "listening"),
    avatar(200, "thinking"),
    avatar(400, "listening"),
    avatar(500, "speaking"),
  ]);
  assert.deepEqual(
    unasked.map((span) => [span.at, span.duration]),
    [[100, 100]],
    "and the one that follows a thinking is not",
  );
});

test("a window the interview was paused during says so", () => {
  // `applyPause` in web/interview.js records a `lifecycle` paused row, and its
  // own comment says why: so the gap is visible rather than passing as thinking
  // time. The response window is exactly the thing it would otherwise pass as,
  // and a five-minute break rendered as a five-minute silence is the one reading
  // on this panel that misleads a reviewer outright.
  // Paused during the candidate's own turn, which is the ordinary case: no
  // `avatar` row is written at all and the open window simply runs through it.
  const during = responseWindows([
    avatar(0, "speaking"),
    avatar(100, "listening"),
    life(1000, "paused"),
    life(200_000, "resumed"),
    avatar(201_000, "speaking"),
  ]);
  assert.equal(during.length, 1);
  assert.equal(during[0].paused, true);
  assert.equal(during[0].duration, 200_900, "the duration is not shortened, it is explained");

  // Paused while the interviewer is mid-turn: src/livekit.rs publishes
  // `listening`, so the window this opens is the break itself. The two rows are
  // written by two different sides and either can land first, so both orders
  // have to mark it.
  for (const order of [
    [life(1000, "paused"), avatar(1100, "listening")],
    [avatar(1000, "listening"), life(1100, "paused")],
  ]) {
    const windows = responseWindows([avatar(0, "speaking"), ...order, avatar(300_000, "speaking")]);
    assert.equal(windows.length, 1);
    assert.equal(windows[0].paused, true, JSON.stringify(order.map((row) => row.kind)));
  }

  // A window after the resume is not marked, or every window after the first
  // break would carry it.
  const after = responseWindows([
    avatar(0, "speaking"),
    avatar(100, "listening"),
    life(1000, "paused"),
    life(2000, "resumed"),
    avatar(3000, "speaking"),
    avatar(4000, "listening"),
    avatar(5000, "speaking"),
  ]);
  assert.deepEqual(
    after.map((span) => span.paused),
    [true, false],
  );

  // A lifecycle row that is not a pause changes nothing, and there are several:
  // started, round_transition, rounds_final, ended.
  const other = responseWindows([
    avatar(0, "speaking"),
    avatar(100, "listening"),
    life(1000, "round_transition"),
    avatar(2000, "speaking"),
  ]);
  assert.equal(other[0].paused, false);
});

test("the interview ending is not a question, and a lost resume is not the rest of the interview", () => {
  // `send_wrap_up_and_wait` in src/livekit.rs finishes by setting `listening`
  // again, and the browser keeps recording past `ended` to write the final
  // rounds. Without stopping at `ended`, every timed-out interview finished with
  // a window opened by the goodbye and closed by nothing, rendered as an
  // interview that stopped mid-answer.
  const wrapped = responseWindows([
    avatar(0, "speaking"),
    avatar(1000, "listening"),
    avatar(2000, "speaking"),
    life(3000, "ended"),
    avatar(4000, "speaking"),
    avatar(5000, "listening"),
    life(6000, "rounds_final"),
  ]);
  assert.deepEqual(
    wrapped.map((span) => [span.at, span.duration]),
    [[1000, 1000]],
    "the goodbye opens nothing and the rows after it are not read",
  );

  // A dropped batch can take the `resumed` row with it. Marking every window
  // after it as paused would spread one lost row across the whole interview, and
  // that mark is the only affirmative claim this panel makes. The interviewer
  // speaking is what bounds it: `watch_prompt` returns nothing while paused and
  // `handle_gemini_event` drops every audio event then, so there is no path from
  // a paused interview to a `speaking` row.
  const lost = responseWindows([
    avatar(0, "speaking"),
    avatar(10_000, "listening"),
    life(11_000, "paused"),
    avatar(61_000, "speaking"),
    avatar(70_000, "listening"),
    avatar(80_000, "speaking"),
  ]);
  assert.deepEqual(
    lost.map((span) => span.paused),
    [true, false],
    "the window that saw the pause is marked and the interview after it is not",
  );
});
