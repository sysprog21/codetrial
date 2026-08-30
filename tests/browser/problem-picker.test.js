// Run with: node --test tests/browser/*.test.js

import { test } from "node:test";
import assert from "node:assert/strict";
import { pickProblem, suggestDifficulty } from "../../web/problem-picker.js";

const bank = [
  { id: "passed", difficulty: "Easy" },
  { id: "fresh", difficulty: "Easy" },
  { id: "medium", difficulty: "Medium" },
  { id: "hard", difficulty: "Hard" },
];
const hired = (problemId) => ({ problemId, report: { decision: "HIRE" } });
const first = () => 0;

test("a problem already passed is not what gets recommended next", () => {
  const choice = pickProblem(bank, new Set(["Easy"]), [hired("passed")], first);
  assert.equal(choice.picked.id, "fresh");
  assert.equal(choice.repeat, false);
});

test("a difficulty nobody selected is never recommended", () => {
  // `first` would return "passed" if the Easy entries were still in the pool.
  const choice = pickProblem(bank, new Set(["Hard"]), [], first);
  assert.equal(choice.picked.id, "hard");
});

test("selecting several difficulties draws from all of them", () => {
  const last = () => 1 - Number.EPSILON;
  assert.equal(pickProblem(bank, new Set(["Medium", "Hard"]), [], last).picked.id, "hard");
  assert.equal(pickProblem(bank, new Set(["Medium", "Hard"]), [], first).picked.id, "medium");
});

test("a level with nothing left to pass says so instead of pretending", () => {
  const choice = pickProblem(bank, new Set(["Easy"]), [hired("passed"), hired("fresh")], first);
  assert.equal(choice.picked.id, "passed");
  assert.equal(choice.repeat, true, "the caller has to be able to say this is a repeat");
});

test("a report that did not end in a hire leaves the problem in the pool", () => {
  const choice = pickProblem(
    bank,
    new Set(["Easy"]),
    [{ problemId: "passed", report: { decision: "NO_HIRE" } }],
    first,
  );
  assert.equal(choice.picked.id, "passed");
});

test("history entries the picker cannot read do not throw", () => {
  // Local storage carries whatever an older build wrote, and an interview that
  // ended without a report writes the entry anyway.
  const junk = [null, {}, { problemId: "passed" }, { problemId: "passed", report: null }];
  assert.equal(pickProblem(bank, new Set(["Easy"]), junk, first).picked.id, "passed");
});

test("no problem at the selected level is null, not a crash", () => {
  assert.equal(pickProblem([], new Set(["Easy"]), [], first), null);
});

const bySuggestion = [
  { id: "easy-a", difficulty: "Easy" },
  { id: "easy-b", difficulty: "Easy" },
  { id: "medium-a", difficulty: "Medium" },
  { id: "medium-b", difficulty: "Medium" },
  { id: "hard-a", difficulty: "Hard" },
  { id: "hard-b", difficulty: "Hard" },
];
// Newest first, the order both /api/reports and history.js hand over.
const attempts = (...pairs) =>
  pairs.map(([problemId, decision]) => ({ problemId, report: { decision } }));

test("no history is no opinion about the level", () => {
  assert.equal(suggestDifficulty(bySuggestion, []), null);
});

test("one result is not a streak", () => {
  assert.equal(suggestDifficulty(bySuggestion, attempts(["easy-a", "HIRE"])), null);
});

test("two passes in a row move the candidate up", () => {
  assert.deepEqual(
    suggestDifficulty(bySuggestion, attempts(["easy-a", "HIRE"], ["easy-b", "HIRE"])),
    { difficulty: "Medium", from: "Easy", reason: "passed" },
  );
});

test("two misses in a row move the candidate down", () => {
  assert.deepEqual(
    suggestDifficulty(bySuggestion, attempts(["medium-a", "NO_HIRE"], ["medium-b", "NO_HIRE"])),
    { difficulty: "Easy", from: "Medium", reason: "struggled" },
  );
});

test("a mixed pair leaves the candidate where they are", () => {
  assert.equal(
    suggestDifficulty(bySuggestion, attempts(["medium-a", "HIRE"], ["medium-b", "NO_HIRE"])),
    null,
  );
});

test("the streak is read at the level last attempted, not across levels", () => {
  // One Medium on top of two passed Easy problems: the Easy streak is history,
  // and one Medium is not yet a streak of its own.
  assert.equal(
    suggestDifficulty(
      bySuggestion,
      attempts(["medium-a", "HIRE"], ["easy-a", "HIRE"], ["easy-b", "HIRE"]),
    ),
    null,
  );
});

test("older attempts at the level do not dilute the recent streak", () => {
  assert.deepEqual(
    suggestDifficulty(
      bySuggestion,
      attempts(["easy-a", "HIRE"], ["easy-b", "HIRE"], ["easy-a", "NO_HIRE"]),
    ),
    { difficulty: "Medium", from: "Easy", reason: "passed" },
  );
});

test("passing at the top stays at the top and still says why", () => {
  assert.deepEqual(
    suggestDifficulty(bySuggestion, attempts(["hard-a", "HIRE"], ["hard-b", "HIRE"])),
    { difficulty: "Hard", from: "Hard", reason: "passed" },
  );
});

test("struggling at the bottom stays at the bottom", () => {
  assert.deepEqual(
    suggestDifficulty(bySuggestion, attempts(["easy-a", "NO_HIRE"], ["easy-b", "NO_HIRE"])),
    { difficulty: "Easy", from: "Easy", reason: "struggled" },
  );
});

test("a report for a problem no longer in the bank is not counted", () => {
  // Two passes, but one is for a problem since dropped, so there is no streak.
  assert.equal(
    suggestDifficulty(bySuggestion, attempts(["retired-problem", "HIRE"], ["easy-a", "HIRE"])),
    null,
  );
});

test("junk history is no opinion, not a crash", () => {
  assert.equal(suggestDifficulty(bySuggestion, [null, {}, { problemId: "easy-a" }]), null);
});

test("an interview that ended without a verdict is not counted as a failure", () => {
  // The entry is written whether or not the agent graded it, so reading
  // "not HIRE" as a miss marched candidates down a level for ungraded runs.
  assert.equal(
    suggestDifficulty(bySuggestion, [
      { problemId: "medium-a", report: {} },
      { problemId: "medium-b", report: { decision: null } },
    ]),
    null,
  );
});

test("an ungraded entry does not decide which level is the current one", () => {
  // Newest is an ungraded Hard attempt; the streak belongs to the Medium pair
  // under it, which is the level the candidate actually has results at.
  assert.deepEqual(
    suggestDifficulty(bySuggestion, [
      { problemId: "hard-a", report: {} },
      ...attempts(["medium-a", "HIRE"], ["medium-b", "HIRE"]),
    ]),
    { difficulty: "Hard", from: "Medium", reason: "passed" },
  );
});
