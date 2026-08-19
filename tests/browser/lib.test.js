// Run with: node --test tests/browser/*.test.js
// Covers the browser judge/report contracts that used to be asserted by
// substring-matching web/interview.js from Rust.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  acceptsReport,
  checkAnswer,
  clamp,
  codeUpdatePayload,
  canonicalJson,
  deepEqual,
  endInterviewPayload,
  escapeHtml,
  formatTime,
  integrityEventPayload,
  normalize,
  orPlaceholder,
  renderValue,
  sanitizeReport,
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
// cap, and every character a control one. Those cost six bytes each once JSON
// escapes them to `\u0001`, which beats even a four-byte character, so this is
// the case the budget has to survive.
test("sanitizeReport bounds text so the largest possible report still fits", () => {
  const long = "\u0001".repeat(50_000);
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
  assert.equal(report.integrityEvents.length, 25);
  assert.ok(new TextEncoder().encode(JSON.stringify(report)).length < 64 * 1024, "MAX_REPORT_BYTES in src/web.rs");
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
    hintsUsed: 2,
  });

  assert.deepEqual(report, {
    codingScore: 82,
    communicationScore: 74,
    decision: "HIRE",
    summary: "You solved it.",
    codingFeedback: { strengths: ["clear"], improvements: ["edge cases"] },
    communicationFeedback: { strengths: ["narrated"], improvements: ["slow down"] },
    integrityEvents: [{ type: "SESSION_START", at: "now", severity: "info", source: "media", durationMs: 0, seq: 1, prevHash: "", hash: "a".repeat(64), detail: null, sourceEventIds: [] }],
    hintsUsed: 2,
  });
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
