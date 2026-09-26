// Run with: node --test tests/browser/*.test.js

import { test } from "node:test";
import assert from "node:assert/strict";
import { consumeSharedFocus, pickProblem, practiceFocus, storeSharedFocus, suggestDifficulty } from "../../web/problem-picker.js";

const bank = [
  { id: "passed", difficulty: "Easy" },
  { id: "fresh", difficulty: "Easy" },
  { id: "medium", difficulty: "Medium" },
  { id: "hard", difficulty: "Hard" },
];
const hired = (problemId) => ({ problemId, report: { decision: "HIRE" } });
const completed = (problemId, at) => ({ problemId, at, report: { decision: "HIRE" } });
const first = () => 0;
const day = 24 * 60 * 60 * 1000;

test("a problem already passed is not what gets recommended next", () => {
  const choice = pickProblem(bank, new Set(["Easy"]), [hired("passed")], first);
  assert.equal(choice.picked.id, "fresh");
  assert.equal(choice.repeat, false);
});

test("a due completed problem takes priority over an unseen one", () => {
  const now = 10 * day;
  const choice = pickProblem(
    bank,
    new Set(["Easy"]),
    [completed("passed", now - day)],
    first,
    now,
  );
  assert.equal(choice.picked.id, "passed");
  assert.equal(choice.review.intervalDays, 1);
});

for (const [name, levels, reports, avoid, expected, review] of [
  ["another due review still takes priority", ["Easy"],
    [completed("passed", 0), completed("hard", 0)], "passed", "hard", true],
  ["the only due review yields to an unseen problem", ["Easy"],
    [completed("passed", 0)], "passed", "fresh", false],
  ["the only unseen problem yields to another eligible problem", ["Easy"],
    [hired("passed")], "fresh", "passed", false],
  ["the only eligible problem stays selected", ["Medium"],
    [], "medium", "medium", false],
  ["the only due review stays selected when no level is eligible", [],
    [completed("passed", 0)], "passed", "passed", true],
]) {
  test(`on a redraw, ${name}`, () => {
    const choice = pickProblem(bank, new Set(levels), reports, first, 10 * day, avoid);
    assert.equal(choice.picked.id, expected);
    assert.equal(Boolean(choice.review), review);
  });
}

test("a completed problem stays out of the queue until its interval has elapsed", () => {
  const now = 10 * day;
  const choice = pickProblem(
    bank,
    new Set(["Easy"]),
    [completed("passed", now - day + 1)],
    first,
    now,
  );
  assert.equal(choice.picked.id, "fresh");
  assert.equal(choice.review, null);
});

test("each successful review lengthens the next interval", () => {
  const now = 10 * day;
  const choice = pickProblem(
    bank,
    new Set(["Easy"]),
    [completed("passed", now - 3 * day), completed("passed", now - 4 * day)],
    first,
    now,
  );
  assert.equal(choice.picked.id, "passed");
  assert.equal(choice.review.intervalDays, 3);
});

test("a difficulty nobody selected is never recommended as a new problem", () => {
  // `first` would return "passed" if the Easy entries were still in the pool.
  const choice = pickProblem(bank, new Set(["Hard"]), [], first);
  assert.equal(choice.picked.id, "hard");
});

test("a due review at a level the lobby moved past is still recommended", () => {
  const now = 10 * day;
  const choice = pickProblem(bank, new Set(["Medium"]), [completed("passed", now - day)], first, now);
  assert.equal(choice.picked.id, "passed");
  assert.equal(choice.review.intervalDays, 1);
});

test("a failed review resets the interval", () => {
  const now = 10 * day;
  const choice = pickProblem(
    bank,
    new Set(["Easy"]),
    [
      { problemId: "passed", at: now - day, report: { decision: "NO_HIRE" } },
      completed("passed", now - 2 * day),
    ],
    first,
    now,
  );
  assert.equal(choice.picked.id, "passed");
  assert.equal(choice.review.intervalDays, 1);
});

test("an older failed review does not erase newer successes", () => {
  const now = 10 * day;
  const choice = pickProblem(
    bank,
    new Set(["Easy"]),
    [
      completed("passed", now - 3 * day),
      completed("passed", now - 4 * day),
      { problemId: "passed", at: now - 5 * day, report: { decision: "NO_HIRE" } },
    ],
    first,
    now,
  );
  assert.equal(choice.picked.id, "passed");
  assert.equal(choice.review.intervalDays, 3);
});

// A failure at either end of the history proves nothing about the reset: the
// newest one sets the interval by being newest, and the oldest one resets a
// streak that had not started. Only a failure with successes on both sides
// tells "the streak restarts here" apart from "count every pass ever".
test("a failure resets the streak the successes after it rebuild", () => {
  const now = 10 * day;
  const choice = pickProblem(
    bank,
    new Set(["Easy"]),
    [
      completed("passed", now - day),
      { problemId: "passed", at: now - 2 * day, report: { decision: "NO_HIRE" } },
      completed("passed", now - 3 * day),
    ],
    first,
    now,
  );
  assert.equal(choice.picked.id, "passed");
  assert.equal(
    choice.review.intervalDays,
    1,
    "one success since the failure, not two across it",
  );
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

test("the newest assessed plan supplies one candidate-facing practice focus", () => {
  const focus = practiceFocus([
    {
      report: {
        decision: "NO_HIRE",
        improvementPlan: [
          { weakness: "Test boundaries", drill: "Build a test table", successCriterion: "Predict each output" },
          { weakness: "Explain complexity", drill: "Narrate bounds", successCriterion: "Name time and space" },
        ],
      },
    },
  ]);
  assert.deepEqual(focus, {
    weakness: "Test boundaries",
    drill: "Build a test table",
    successCriterion: "Predict each output",
    occurrences: 1,
  });
});

test("a recurring focus outranks a one-off item while keeping the newest matching drill", () => {
  const focus = practiceFocus([
    { report: { decision: "HIRE", improvementPlan: [{ weakness: "Explain complexity", drill: "Narrate bounds", successCriterion: "Name time and space" }] } },
    { report: { decision: "NO_HIRE", improvementPlan: [{ weakness: "Test boundaries", drill: "Build a test table", successCriterion: "Predict each output" }] } },
    { report: { decision: "HIRE", improvementPlan: [{ weakness: "Test boundaries", drill: "Name a boundary", successCriterion: "Cover four classes" }] } },
  ]);
  assert.deepEqual(focus, {
    weakness: "Test boundaries",
    drill: "Build a test table",
    successCriterion: "Predict each output",
    occurrences: 2,
  });
});

test("a duplicated plan item in one report is not a recurring focus", () => {
  const focus = practiceFocus([
    { report: { decision: "HIRE", improvementPlan: [
      { weakness: "Test boundaries", drill: "Build a test table", successCriterion: "Predict each output" },
      { weakness: "Test boundaries", drill: "Name a boundary", successCriterion: "Cover four classes" },
    ] } },
    { report: { decision: "NO_HIRE", improvementPlan: [
      { weakness: "Explain complexity", drill: "Narrate bounds", successCriterion: "Name time and space" },
    ] } },
  ]);

  assert.deepEqual(focus, {
    weakness: "Test boundaries",
    drill: "Build a test table",
    successCriterion: "Predict each output",
    occurrences: 1,
  });
});

test("incomplete, ungraded, and malformed reports cannot create a practice focus", () => {
  assert.equal(practiceFocus([
    { report: { incomplete: true, decision: "NO_HIRE", improvementPlan: [{ weakness: "x", drill: "y", successCriterion: "z" }] } },
    { report: { decision: null, improvementPlan: [{ weakness: "x", drill: "y", successCriterion: "z" }] } },
    { report: { decision: "HIRE", improvementPlan: [{ weakness: "x" }] } },
  ]), null);
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

test("a shared focus is handed over once, in session storage, and unsharing clears it", () => {
  const store = new Map();
  const storage = {
    getItem: (key) => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => store.set(key, String(value)),
    removeItem: (key) => store.delete(key),
  };
  storeSharedFocus(storage, "Test boundaries");
  assert.equal(consumeSharedFocus(storage), "Test boundaries");
  assert.equal(consumeSharedFocus(storage), "", "a reload does not share it again");

  storeSharedFocus(storage, "Test boundaries");
  storeSharedFocus(storage, null);
  assert.equal(consumeSharedFocus(storage), "", "choosing not to share clears an earlier one");

  const broken = { getItem() { throw new Error("blocked"); }, setItem() { throw new Error("blocked"); }, removeItem() { throw new Error("blocked"); } };
  storeSharedFocus(broken, "Test boundaries");
  assert.equal(consumeSharedFocus(broken), "", "blocked storage shares nothing");
});

test("the interview reads the focus from session storage, never from its address", async () => {
  const { readFile } = await import("node:fs/promises");
  const source = await readFile(new URL("../../web/interview.js", import.meta.url), "utf8");
  assert.doesNotMatch(source, /params\.get\("focus"\)/);
  assert.match(source, /practiceFocus: consumeSharedFocus\(tabStorage\)/);
  assert.match(source, /const tabStorage = storageArea\("sessionStorage"\);/);
});
