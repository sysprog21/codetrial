// Run with: node --test tests/browser/*.test.js

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  consumeSharedFocus,
  pickProblem,
  practiceFocus,
  sequelSuggestion,
  storeSharedFocus,
  suggestDifficulty,
  clearSelectedPracticeFocus,
  practiceFocusFrom,
  practiceFocusOwner,
  practiceFocusAccountOwner,
  readSelectedPracticeFocus,
  readSelectedPracticeFocusSelection,
  storeSelectedPracticeFocus,
} from "../../web/problem-picker.js";
import { ACTIVE_CONTRACT } from "../../web/lib.js";

const bank = [
  { id: "passed", difficulty: "Easy" },
  { id: "fresh", difficulty: "Easy" },
  { id: "medium", difficulty: "Medium" },
  { id: "hard", difficulty: "Hard" },
];
const hired = (problemId) => ({ problemId, report: { decision: "HIRE" } });
const completed = (problemId, at) => ({
  problemId,
  at,
  report: { decision: "HIRE" },
});
const first = () => 0;
const day = 24 * 60 * 60 * 1000;

test("explicit random draws reach every eligible problem despite overdue reviews", () => {
  const problems = [
    { id: "due-a", difficulty: "Easy" },
    { id: "due-b", difficulty: "Easy" },
    { id: "unseen", difficulty: "Easy" },
    { id: "outside", difficulty: "Hard" },
  ];
  const reports = ["due-a", "due-b", "outside"].map((id) => completed(id, 0));
  for (const avoid of [undefined, "due-a", "due-b", "unseen"]) {
    const expected = problems.filter(
      (problem) => problem.difficulty === "Easy" && problem.id !== avoid,
    );
    for (let index = 0; index < expected.length; index++) {
      const choice = pickProblem(
        problems,
        new Set(["Easy"]),
        reports,
        () => (index + 0.5) / expected.length,
        10 * day,
        avoid,
        undefined,
        "random",
      );
      assert.equal(choice.picked.id, expected[index].id);
      assert.equal(choice.review, null);
      assert.equal(choice.repeat, false);
    }
  }
});

test("explicit random draws retain a sole eligible problem and respect empty filters", () => {
  const draw = (levels) =>
    pickProblem(
      bank,
      new Set(levels),
      [completed("passed", 0)],
      first,
      10 * day,
      "medium",
      undefined,
      "random",
    );
  assert.equal(draw(["Medium"]).picked.id, "medium");
  assert.equal(draw([]), null);
});

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
  [
    "another due review still takes priority",
    ["Easy"],
    [completed("passed", 0), completed("hard", 0)],
    "passed",
    "hard",
    true,
  ],
  [
    "the only due review yields to an unseen problem",
    ["Easy"],
    [completed("passed", 0)],
    "passed",
    "fresh",
    false,
  ],
  [
    "the only unseen problem yields to another eligible problem",
    ["Easy"],
    [hired("passed")],
    "fresh",
    "passed",
    false,
  ],
  [
    "the only eligible problem stays selected",
    ["Medium"],
    [],
    "medium",
    "medium",
    false,
  ],
  [
    "the only due review stays selected when no level is eligible",
    [],
    [completed("passed", 0)],
    "passed",
    "passed",
    true,
  ],
]) {
  test(`on a redraw, ${name}`, () => {
    const choice = pickProblem(
      bank,
      new Set(levels),
      reports,
      first,
      10 * day,
      avoid,
    );
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
  const choice = pickProblem(
    bank,
    new Set(["Medium"]),
    [completed("passed", now - day)],
    first,
    now,
  );
  assert.equal(choice.picked.id, "passed");
  assert.equal(choice.review.intervalDays, 1);
});

test("a due review outside an opt-in filter stays the first priority", () => {
  const now = 10 * day;
  const filtered = bank.filter((problem) => problem.id !== "passed");
  const choice = pickProblem(
    filtered,
    new Set(["Easy"]),
    [completed("passed", now - day)],
    first,
    now,
    undefined,
    bank,
  );
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
      {
        problemId: "passed",
        at: now - 5 * day,
        report: { decision: "NO_HIRE" },
      },
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
      {
        problemId: "passed",
        at: now - 2 * day,
        report: { decision: "NO_HIRE" },
      },
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
  assert.equal(
    pickProblem(bank, new Set(["Medium", "Hard"]), [], last).picked.id,
    "hard",
  );
  assert.equal(
    pickProblem(bank, new Set(["Medium", "Hard"]), [], first).picked.id,
    "medium",
  );
});

test("a level with nothing left to pass says so instead of pretending", () => {
  const choice = pickProblem(
    bank,
    new Set(["Easy"]),
    [hired("passed"), hired("fresh")],
    first,
  );
  assert.equal(choice.picked.id, "passed");
  assert.equal(
    choice.repeat,
    true,
    "the caller has to be able to say this is a repeat",
  );
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
  const junk = [
    null,
    {},
    { problemId: "passed" },
    { problemId: "passed", report: null },
  ];
  assert.equal(
    pickProblem(bank, new Set(["Easy"]), junk, first).picked.id,
    "passed",
  );
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
  assert.equal(
    suggestDifficulty(bySuggestion, attempts(["easy-a", "HIRE"])),
    null,
  );
});

test("two passes in a row move the candidate up", () => {
  assert.deepEqual(
    suggestDifficulty(
      bySuggestion,
      attempts(["easy-a", "HIRE"], ["easy-b", "HIRE"]),
    ),
    { difficulty: "Medium", from: "Easy", reason: "passed" },
  );
});

test("two misses in a row move the candidate down", () => {
  assert.deepEqual(
    suggestDifficulty(
      bySuggestion,
      attempts(["medium-a", "NO_HIRE"], ["medium-b", "NO_HIRE"]),
    ),
    { difficulty: "Easy", from: "Medium", reason: "struggled" },
  );
});

test("a mixed pair leaves the candidate where they are", () => {
  assert.equal(
    suggestDifficulty(
      bySuggestion,
      attempts(["medium-a", "HIRE"], ["medium-b", "NO_HIRE"]),
    ),
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
    suggestDifficulty(
      bySuggestion,
      attempts(["hard-a", "HIRE"], ["hard-b", "HIRE"]),
    ),
    { difficulty: "Hard", from: "Hard", reason: "passed" },
  );
});

test("struggling at the bottom stays at the bottom", () => {
  assert.deepEqual(
    suggestDifficulty(
      bySuggestion,
      attempts(["easy-a", "NO_HIRE"], ["easy-b", "NO_HIRE"]),
    ),
    { difficulty: "Easy", from: "Easy", reason: "struggled" },
  );
});

test("a report for a problem no longer in the bank is not counted", () => {
  // Two passes, but one is for a problem since dropped, so there is no streak.
  assert.equal(
    suggestDifficulty(
      bySuggestion,
      attempts(["retired-problem", "HIRE"], ["easy-a", "HIRE"]),
    ),
    null,
  );
});

test("junk history is no opinion, not a crash", () => {
  assert.equal(
    suggestDifficulty(bySuggestion, [null, {}, { problemId: "easy-a" }]),
    null,
  );
});

test("an unsupported contract still steers the picker by its saved verdict", () => {
  // The practice summary leaves these out of its totals; the picker reads the
  // entry as saved, so a pass recorded under a newer bundle is still a pass.
  const future = {
    ...ACTIVE_CONTRACT,
    bundleVersion: ACTIVE_CONTRACT.bundleVersion + 1,
  };
  const now = 10 * day;
  const passedUnder = (problemId, interviewContract) => ({
    problemId,
    at: now - day + 1,
    report: { decision: "HIRE", interviewContract },
  });
  for (const interviewContract of [future, { ...ACTIVE_CONTRACT, extra: 1 }]) {
    const choice = pickProblem(
      bank,
      new Set(["Easy"]),
      [passedUnder("passed", interviewContract)],
      first,
      now,
    );
    assert.equal(choice.picked.id, "fresh");
  }
  assert.deepEqual(
    suggestDifficulty(bySuggestion, [
      {
        problemId: "easy-a",
        report: { decision: "HIRE", interviewContract: future },
      },
      {
        problemId: "easy-b",
        report: { decision: "HIRE", interviewContract: future },
      },
    ]),
    { difficulty: "Medium", from: "Easy", reason: "passed" },
  );
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
          {
            weakness: "Test boundaries",
            drill: "Build a test table",
            successCriterion: "Predict each output",
          },
          {
            weakness: "Explain complexity",
            drill: "Narrate bounds",
            successCriterion: "Name time and space",
          },
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
    {
      report: {
        decision: "HIRE",
        improvementPlan: [
          {
            weakness: "Explain complexity",
            drill: "Narrate bounds",
            successCriterion: "Name time and space",
          },
        ],
      },
    },
    {
      report: {
        decision: "NO_HIRE",
        improvementPlan: [
          {
            weakness: "Test boundaries",
            drill: "Build a test table",
            successCriterion: "Predict each output",
          },
        ],
      },
    },
    {
      report: {
        decision: "HIRE",
        improvementPlan: [
          {
            weakness: "Test boundaries",
            drill: "Name a boundary",
            successCriterion: "Cover four classes",
          },
        ],
      },
    },
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
    {
      report: {
        decision: "HIRE",
        improvementPlan: [
          {
            weakness: "Test boundaries",
            drill: "Build a test table",
            successCriterion: "Predict each output",
          },
          {
            weakness: "Test boundaries",
            drill: "Name a boundary",
            successCriterion: "Cover four classes",
          },
        ],
      },
    },
    {
      report: {
        decision: "NO_HIRE",
        improvementPlan: [
          {
            weakness: "Explain complexity",
            drill: "Narrate bounds",
            successCriterion: "Name time and space",
          },
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

test("incomplete, ungraded, and malformed reports cannot create a practice focus", () => {
  assert.equal(
    practiceFocus([
      {
        report: {
          incomplete: true,
          decision: "NO_HIRE",
          improvementPlan: [
            { weakness: "x", drill: "y", successCriterion: "z" },
          ],
        },
      },
      {
        report: {
          decision: null,
          improvementPlan: [
            { weakness: "x", drill: "y", successCriterion: "z" },
          ],
        },
      },
      { report: { decision: "HIRE", improvementPlan: [{ weakness: "x" }] } },
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

test("a shared focus is handed over once, in session storage, and unsharing clears it", () => {
  const store = new Map();
  const storage = {
    getItem: (key) => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => store.set(key, String(value)),
    removeItem: (key) => store.delete(key),
  };
  storeSharedFocus(storage, "Test boundaries");
  assert.equal(consumeSharedFocus(storage), "Test boundaries");
  assert.equal(
    consumeSharedFocus(storage),
    "",
    "a reload does not share it again",
  );

  storeSharedFocus(storage, "Test boundaries");
  storeSharedFocus(storage, null);
  assert.equal(
    consumeSharedFocus(storage),
    "",
    "choosing not to share clears an earlier one",
  );

  const broken = {
    getItem() {
      throw new Error("blocked");
    },
    setItem() {
      throw new Error("blocked");
    },
    removeItem() {
      throw new Error("blocked");
    },
  };
  storeSharedFocus(broken, "Test boundaries");
  assert.equal(
    consumeSharedFocus(broken),
    "",
    "blocked storage shares nothing",
  );
});

test("selected practice focus stays stored until explicitly cleared", () => {
  const store = new Map();
  const storage = {
    getItem: (key) => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => store.set(key, String(value)),
    removeItem: (key) => store.delete(key),
  };
  const focus = {
    phase: "Test",
    weakness: "Test boundaries",
    drill: "Build a test table",
    successCriterion: "Predict each output",
  };
  storeSelectedPracticeFocus(storage, "account:A", focus);
  assert.deepEqual(readSelectedPracticeFocus(storage, "account:A"), focus);
  assert.deepEqual(readSelectedPracticeFocus(storage, "account:A"), focus);
  clearSelectedPracticeFocus(storage);
  assert.equal(readSelectedPracticeFocus(storage, "account:A"), null);
});

test("selected practice focus keeps its source report id", () => {
  const store = new Map();
  const storage = {
    getItem: (key) => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => store.set(key, String(value)),
    removeItem: (key) => store.delete(key),
  };
  const focus = {
    phase: "Test",
    weakness: "Test boundaries",
    drill: "Build a test table",
    successCriterion: "Predict each output",
  };
  storeSelectedPracticeFocus(
    storage,
    "account:A",
    focus,
    "coding",
    "focus-source",
  );
  assert.deepEqual(readSelectedPracticeFocusSelection(storage, "account:A"), {
    focus,
    sourceReportId: "focus-source",
  });
  assert.deepEqual(readSelectedPracticeFocus(storage, "account:A"), focus);
});

test("practice focus account owners use one canonical key format", () => {
  assert.equal(practiceFocusAccountOwner("A"), "account:A");
  assert.equal(
    practiceFocusAccountOwner("opaque-owner"),
    "account:opaque-owner",
  );
  assert.equal(practiceFocusAccountOwner(""), null);
  assert.equal(practiceFocusAccountOwner(null), null);
  assert.equal(practiceFocusAccountOwner(null, true), "device");
});

test("selected practice focus reports whether storage succeeded", () => {
  const item = {
    phase: "Test",
    weakness: "Test boundaries",
    drill: "Build a test table",
    successCriterion: "Predict each output",
  };
  const storage = {
    setItem() {},
    removeItem() {},
  };
  assert.equal(
    storeSelectedPracticeFocus(
      storage,
      "account:A",
      item,
      "coding",
      "report-1",
    ),
    true,
  );
  const blockedStorage = {
    setItem() {
      throw new Error("blocked");
    },
    removeItem() {},
  };
  assert.equal(
    storeSelectedPracticeFocus(
      blockedStorage,
      "account:A",
      item,
      "coding",
      "report-1",
    ),
    false,
  );
  assert.equal(
    storeSelectedPracticeFocus(storage, null, item, "coding", "report-1"),
    false,
  );
});

test("clearing selected practice focus removes it from storage", () => {
  const store = new Map();
  const storage = {
    getItem: (key) => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => store.set(key, String(value)),
    removeItem: (key) => store.delete(key),
  };
  storeSelectedPracticeFocus(storage, "account:A", {
    phase: "Test",
    weakness: "Test boundaries",
    drill: "Build a test table",
    successCriterion: "Predict each output",
  });
  clearSelectedPracticeFocus(storage);
  assert.equal(readSelectedPracticeFocus(storage, "account:A"), null);
});

test("invalid selected practice focus is discarded", () => {
  const store = new Map();
  const storage = {
    getItem: (key) => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => store.set(key, String(value)),
    removeItem: (key) => store.delete(key),
  };
  const owner = "account:candidate";
  storage.setItem(
    "codetrial.selectedPracticeFocus",
    JSON.stringify({
      owner,
      focus: {
        phase: "Test",
        weakness: "",
        drill: "Build a test table",
        successCriterion: "Predict each output",
      },
    }),
  );
  assert.equal(readSelectedPracticeFocus(storage, owner), null);
  assert.equal(storage.getItem("codetrial.selectedPracticeFocus"), null);
});

test("selected practice focus rejects unknown phases", () => {
  assert.equal(
    practiceFocusFrom({
      phase: "Something else",
      weakness: "Test boundaries",
      drill: "Build a test table",
      successCriterion: "Predict each output",
    }),
    null,
  );
});

test("selected practice focus rebuilds only bounded fields", () => {
  const focus = practiceFocusFrom({
    phase: "Test",
    weakness: "x".repeat(450),
    drill: "Build a test table",
    successCriterion: "Predict each output",
    durationMin: 15,
    extra: "not carried",
  });

  assert.equal(Array.from(focus.weakness).length, 400);
  assert.equal("durationMin" in focus, false);
  assert.equal("extra" in focus, false);
});

test("the interview reads the focus from session storage, never from its address", async () => {
  const { readFile } = await import("node:fs/promises");
  const source = await readFile(
    new URL("../../web/interview.js", import.meta.url),
    "utf8",
  );
  assert.doesNotMatch(source, /params\.get\("focus"\)/);
  assert.match(source, /practiceFocus: consumeSharedFocus\(tabStorage\)/);
  assert.match(source, /const tabStorage = storageArea\("sessionStorage"\);/);
});

test("random draws respect narrowed practice while reviews keep their broader pool", () => {
  const reports = [completed("passed", 0)];
  const practice = bank.filter((problem) => problem.id === "fresh");
  const draw = (mode) =>
    pickProblem(
      practice,
      new Set(["Easy"]),
      reports,
      first,
      10 * day,
      undefined,
      bank,
      mode,
    );
  assert.equal(draw("recommend").picked.id, "passed");
  assert.equal(draw("random").picked.id, "fresh");
});

const series = [
  { id: "pair", difficulty: "Easy", sequel: "bundle" },
  { id: "bundle", difficulty: "Medium", sequel: "triple" },
  { id: "triple", difficulty: "Hard", sequel: null },
  { id: "alone", difficulty: "Easy", sequel: null },
];
const attempt = (problemId, at, decision) => ({
  problemId,
  at,
  report: decision ? { decision } : { incomplete: true },
});
const offered = (reports) => {
  const suggestion = sequelSuggestion(series, reports);
  return suggestion && [suggestion.from.id, suggestion.to.id];
};

test("a pass offers its sequel one level up, from Easy and from Medium", () => {
  assert.deepEqual(offered([attempt("pair", 2, "HIRE")]), ["pair", "bundle"]);
  assert.deepEqual(offered([attempt("bundle", 2, "HIRE")]), [
    "bundle",
    "triple",
  ]);
});

test("no sequel is offered after a fail, an unscored interview or a problem without one", () => {
  assert.equal(offered([]), null);
  assert.equal(offered([attempt("pair", 2, "NO_HIRE")]), null);
  assert.equal(offered([attempt("pair", 2)]), null);
  assert.equal(offered([attempt("alone", 2, "HIRE")]), null);
  assert.equal(offered([attempt("triple", 2, "HIRE")]), null);
});

test("a sequel already passed is not offered again", () => {
  assert.equal(
    offered([attempt("pair", 2, "HIRE"), attempt("bundle", 1, "HIRE")]),
    null,
  );
  // A failed attempt at the sequel is not a pass, so the offer stands.
  assert.deepEqual(
    offered([attempt("pair", 2, "HIRE"), attempt("bundle", 1, "NO_HIRE")]),
    ["pair", "bundle"],
  );
});

test("the latest interview decides the offer whatever order the reports arrive in", () => {
  // An older pass cannot speak for a newer interview that did not pass.
  assert.equal(
    offered([attempt("pair", 1, "HIRE"), attempt("alone", 2, "NO_HIRE")]),
    null,
  );
  assert.equal(
    offered([attempt("alone", 2, "NO_HIRE"), attempt("pair", 1, "HIRE")]),
    null,
  );
  assert.deepEqual(
    offered([attempt("alone", 1, "NO_HIRE"), attempt("pair", 2, "HIRE")]),
    ["pair", "bundle"],
  );
  // A row without a time cannot be placed, so it is not the latest.
  assert.deepEqual(
    offered([
      { problemId: "alone", report: { decision: "NO_HIRE" } },
      attempt("pair", 1, "HIRE"),
    ]),
    ["pair", "bundle"],
  );
});

test("a sequel the lobby has no card for is not offered", () => {
  const cards = [{ id: "pair", difficulty: "Easy", sequel: "retired" }];
  assert.equal(sequelSuggestion(cards, [attempt("pair", 2, "HIRE")]), null);
});

for (const session of [
  { signedIn: true, user: { id: "B", login: "same" } },
  { signedIn: false },
]) {
  test(`selected focus is cleared when owner changes to ${practiceFocusOwner(session)}`, () => {
    const values = new Map();
    const storage = {
      getItem: (key) => values.get(key) ?? null,
      setItem: (key, value) => values.set(key, value),
      removeItem: (key) => values.delete(key),
    };
    const owner = practiceFocusOwner({
      signedIn: true,
      user: { id: "A", login: "same" },
    });
    storeSelectedPracticeFocus(storage, owner, {
      phase: "Test",
      weakness: "Boundaries",
      drill: "Test edges",
      successCriterion: "All predicted",
    });
    assert.equal(readSelectedPracticeFocus(storage, null), null);
    assert.equal(values.size, 1);
    assert.equal(
      readSelectedPracticeFocus(storage, practiceFocusOwner(session)),
      null,
    );
    assert.equal(values.size, 0);
  });
}

test("selected focus tolerates blocked storage", () => {
  const storage = {
    getItem() {
      throw new Error("blocked");
    },
    setItem() {
      throw new Error("blocked");
    },
    removeItem() {
      throw new Error("blocked");
    },
  };
  storeSelectedPracticeFocus(storage, "account:A", {
    phase: "Test",
    weakness: "Boundaries",
    drill: "Test edges",
    successCriterion: "All predicted",
  });
  assert.equal(readSelectedPracticeFocus(storage, "account:A"), null);
  clearSelectedPracticeFocus(storage);
  assert.equal(
    practiceFocusOwner({ signedIn: true, user: { login: "same" } }),
    null,
  );
});
