import { test } from "node:test";
import assert from "node:assert/strict";
import {
  availableTopics,
  clearPracticeAttempts,
  filterProblems,
  practiceAttemptScope,
  readPracticeAttempts,
  recentPerformance,
  recordPracticeAttempt,
} from "../../web/practice-insights.js";

const problems = [
  { id: "array-easy", difficulty: "Easy", topics: ["Array"] },
  { id: "graph-medium", difficulty: "Medium", topics: ["Graph", "DFS"] },
  { id: "array-hard", difficulty: "Hard", topics: ["Array", "Dynamic Programming"] },
];
const attempt = (problemId, decision, topics = undefined) => ({
  problemId,
  report: { decision, ...(topics ? { topics } : {}) },
});

test("topics are local, unique, and sorted", () => {
  assert.deepEqual(availableTopics(problems), ["Array", "DFS", "Dynamic Programming", "Graph"]);
});

test("difficulty and topic filters combine", () => {
  const filtered = filterProblems(problems, [], {
    difficulties: new Set(["Easy", "Medium"]),
    topics: new Set(["Graph"]),
  });
  assert.deepEqual(filtered.map((problem) => problem.id), ["graph-medium"]);
});

test("not practiced excludes every assessed attempt", () => {
  const filtered = filterProblems(problems, [attempt("array-easy", "NO_HIRE")], {
    difficulties: new Set(["Easy", "Medium", "Hard"]),
    status: "unseen",
  });
  assert.deepEqual(filtered.map((problem) => problem.id), ["graph-medium", "array-hard"]);
});

test("failed before includes a problem even after a later pass", () => {
  const filtered = filterProblems(problems, [
    attempt("array-easy", "HIRE"),
    attempt("array-easy", "NO_HIRE"),
  ], { status: "failed" });
  assert.deepEqual(filtered.map((problem) => problem.id), ["array-easy"]);
});

test("recent performance derives topic weakness and pass rate", () => {
  const result = recentPerformance(problems, [
    attempt("graph-medium", "NO_HIRE"),
    attempt("array-easy", "HIRE"),
    attempt("graph-medium", "NO_HIRE"),
  ]);
  assert.equal(result.passRate, 33);
  assert.equal(result.currentStreak, 1);
  assert.deepEqual(result.weakTopics.map((row) => row.topic), ["DFS", "Graph"]);
});

test("reported topics take precedence over bank fallback topics", () => {
  const result = recentPerformance(problems, [
    attempt("array-easy", "NO_HIRE", ["Hash Table"]),
  ]);
  assert.deepEqual(result.weakTopics.map((row) => row.topic), ["Hash Table"]);
});

test("recent weak topics filter offers related bank problems", () => {
  const filtered = filterProblems(problems, [
    attempt("array-easy", "NO_HIRE"),
    attempt("array-hard", "NO_HIRE"),
    attempt("graph-medium", "HIRE"),
  ], { status: "weak" });
  assert.deepEqual(filtered.map((problem) => problem.id), ["array-easy", "array-hard"]);
});

test("ungraded and malformed history does not become a failure", () => {
  const filtered = filterProblems(problems, [null, {}, { problemId: "array-easy", report: {} }], {
    status: "failed",
  });
  assert.deepEqual(filtered, []);
});

test("starting an interview is saved even before a report exists", () => {
  const values = new Map();
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
  };
  recordPracticeAttempt("graph-medium", { storage, now: 123, scope: "github:alice" });
  assert.deepEqual(readPracticeAttempts({ storage, scope: "github:alice" }), [
    { problemId: "graph-medium", at: 123 },
  ]);
  assert.deepEqual(
    filterProblems(problems, [], {
      status: "attempted",
      attempts: readPracticeAttempts({ storage, scope: "github:alice" }),
    })
      .map((problem) => problem.id),
    ["graph-medium"],
  );
});

test("clearing progress removes locally saved interview starts", () => {
  const values = new Map();
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: (key) => values.delete(key),
  };
  recordPracticeAttempt("graph-medium", { storage, now: 123, scope: "github:alice" });
  recordPracticeAttempt("array-easy", { storage, now: 456, scope: "github:bob" });
  assert.equal(clearPracticeAttempts({ storage, scope: "github:alice" }), true);
  assert.deepEqual(readPracticeAttempts({ storage, scope: "github:alice" }), []);
  assert.deepEqual(readPracticeAttempts({ storage, scope: "github:bob" }), [
    { problemId: "array-easy", at: 456 },
  ]);
});

test("practice attempts are isolated by signed-in account", () => {
  const values = new Map();
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
  };
  const alice = practiceAttemptScope({ signedIn: true, user: { login: "Alice" } });
  const bob = practiceAttemptScope({ signedIn: true, user: { login: "bob" } });
  const anonymous = practiceAttemptScope({ signedIn: false });
  recordPracticeAttempt("graph-medium", { storage, now: 100, scope: alice });
  recordPracticeAttempt("array-hard", { storage, now: 200, scope: bob });
  recordPracticeAttempt("array-easy", { storage, now: 300, scope: anonymous });
  assert.deepEqual(readPracticeAttempts({ storage, scope: alice }), [
    { problemId: "graph-medium", at: 100 },
  ]);
  assert.deepEqual(readPracticeAttempts({ storage, scope: bob }), [
    { problemId: "array-hard", at: 200 },
  ]);
  assert.deepEqual(readPracticeAttempts({ storage, scope: anonymous }), [
    { problemId: "array-easy", at: 300 },
  ]);
});

test("legacy origin-wide attempts migrate only to the anonymous scope", () => {
  const values = new Map([["codetrial.practiceAttempts", JSON.stringify([
    { problemId: "graph-medium", at: 100 },
  ])]]);
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: (key) => values.delete(key),
  };

  assert.deepEqual(readPracticeAttempts({ storage, scope: "github:alice" }), []);
  assert.ok(values.has("codetrial.practiceAttempts"));
  assert.deepEqual(readPracticeAttempts({ storage, scope: "anonymous" }), [
    { problemId: "graph-medium", at: 100 },
  ]);
  assert.equal(values.has("codetrial.practiceAttempts"), false);
  assert.deepEqual(readPracticeAttempts({ storage, scope: "github:alice" }), []);
});

test("a start without a later report is incomplete, not failed", () => {
  const attempts = [{ problemId: "graph-medium", at: 200 }];
  assert.deepEqual(
    filterProblems(problems, [], { status: "incomplete", attempts }).map((problem) => problem.id),
    ["graph-medium"],
  );
  assert.deepEqual(filterProblems(problems, [], { status: "failed", attempts }), []);
});

test("a newer assessed report resolves an incomplete start", () => {
  const reports = [{ ...attempt("graph-medium", "HIRE"), at: 300 }];
  const attempts = [{ problemId: "graph-medium", at: 200 }];
  assert.deepEqual(filterProblems(problems, reports, { status: "incomplete", attempts }), []);
  assert.deepEqual(
    filterProblems(problems, reports, { status: "passed", attempts }).map((problem) => problem.id),
    ["graph-medium"],
  );
});

test("an unscored saved report is incomplete and no longer unseen", () => {
  const reports = [{ problemId: "array-easy", at: 100, report: { incomplete: true } }];
  assert.deepEqual(
    filterProblems(problems, reports, { status: "incomplete" }).map((problem) => problem.id),
    ["array-easy"],
  );
  assert.deepEqual(
    filterProblems(problems, reports, { status: "unseen" }).map((problem) => problem.id),
    ["graph-medium", "array-hard"],
  );
});
