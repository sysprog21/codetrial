import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  buildProgressModel,
  normalizeProgressEntries,
  normalizeProgressEntry,
  pickerEntry,
  progressModelFrom,
  progressPhases,
} from "../../web/progress.js";
import { pickProblem, suggestDifficulty } from "../../web/problem-picker.js";

const web = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "web");
const assessment = (version, algorithm, action = null, tags = []) => ({
  rubricVersion: version,
  phases: progressPhases.map((phase) => ({
    phase,
    score:
      phase === "Algorithm" ? algorithm : phase === "Action" ? action : null,
    weaknessTags: phase === "Algorithm" ? tags : [],
  })),
});
const plan = (weakness) => [
  {
    phase: "Algorithm",
    weakness,
    impact: "high",
    frequency: 1,
    drill: "Narrate",
    durationMin: 5,
    successCriterion: "Justify bounds",
    selfReview: ["time"],
  },
];
const entry = (id, date, score, overrides = {}) => ({
  id,
  date,
  problemId: "two-sum",
  problemTitle: "Two Sum",
  difficulty: "Easy",
  language: "python",
  durationMin: 45,
  mode: "scored",
  report: {
    codingFeedback: {
      improvements: overrides.weakness ? [overrides.weakness] : [],
    },
    communicationFeedback: { improvements: [] },
    improvementPlan: overrides.weakness ? plan(overrides.weakness) : [],
    frameworkAssessment: assessment(
      1,
      score,
      null,
      overrides.weakness ? [overrides.weakness] : [],
    ),
  },
  ...overrides,
});

/// A January 2026 day as epoch milliseconds. Module level because two tests
/// below want the same clock and a copy each is a copy to keep in step.
const at = (day) => new Date(Date.UTC(2026, 0, day)).getTime();

test("progress normalizes local and account wrappers without rewriting legacy history", () => {
  const local = entry("local", "2026-01-02T00:00:00Z", 70);
  const account = {
    id: "db",
    createdAt: 1_767_225_600,
    payload: entry("payload", null, 80),
  };
  delete account.payload.date;
  assert.equal(normalizeProgressEntry(local).id, "local");
  assert.equal(normalizeProgressEntry(account).id, "payload");
  const legacy = normalizeProgressEntry({
    id: "old",
    date: "2025-01-01",
    report: { decision: "HIRE" },
  });
  assert.equal(legacy.report.frameworkAssessment, null);
  const hostile = normalizeProgressEntry({
    date: "not-a-date",
    difficulty: "Expert",
    language: "brainfuck",
    durationMin: 1,
    mode: "hire",
    report: {},
  });
  assert.equal(hostile.at, null);
  assert.deepEqual(
    [hostile.difficulty, hostile.language, hostile.durationMin],
    [null, null, null],
  );
});

test("progress filters metadata, orders attempts, ranks tags, and keeps null as a gap", () => {
  const rows = [
    entry("later", "2026-02-01", 80, { weakness: "Explain complexity" }),
    entry("earlier", "2026-01-01", 60, { weakness: "Explain complexity" }),
    entry("other", "2026-03-01", 99, {
      difficulty: "Hard",
      language: "java",
      durationMin: 60,
    }),
  ];
  const model = buildProgressModel(rows, {
    difficulty: "Easy",
    language: "python",
    durationMin: "45",
  });
  assert.deepEqual(
    model.attempts.map((item) => item.id),
    ["earlier", "later"],
  );
  assert.deepEqual(
    model.series.Algorithm[0].points.map((point) => point.score),
    [60, 80],
  );
  assert.deepEqual(
    model.series.Action,
    [],
    "null STAR values are gaps, not zeroes",
  );
  assert.deepEqual(model.weaknesses, [
    {
      phase: "Algorithm",
      framework: "REACTO",
      count: 2,
      assessed: 2,
      tags: ["Explain complexity"],
    },
  ]);
});

/// One attempt whose Algorithm row carries `weaknesses`, in the order given,
/// each grounded by a plan item so sanitizing keeps it.
const flaggedAttempt = (id, date, weaknesses, score = 60) => {
  const attempt = entry(id, date, score);
  attempt.report.codingFeedback.improvements = [...weaknesses];
  attempt.report.improvementPlan = weaknesses.map(
    (weakness) => plan(weakness)[0],
  );
  attempt.report.frameworkAssessment.phases.find(
    (row) => row.phase === "Algorithm",
  ).weaknessTags = [...weaknesses];
  return attempt;
};

// Each report words its weaknesses afresh, so grouping by the exact sentence
// listed the same gap once per attempt and nothing ever recurred.
test("weaknesses group reworded tags by phase and count attempts", () => {
  const rows = [
    entry("first", "2026-01-01", 50, {
      weakness:
        "Develop a concrete algorithmic plan before attempting to write code.",
    }),
    entry("second", "2026-01-02", 55, {
      weakness:
        "Develop a systematic approach to problem-solving before writing code.",
    }),
    entry("third", "2026-01-03", 60, {
      weakness:
        "Practice articulating a step-by-step algorithm before attempting to write code.",
    }),
    entry("repeat", "2026-01-04", 65, {
      weakness:
        "Develop a concrete algorithmic plan before attempting to write code.",
    }),
  ];
  // Tags survive sanitizing only when the plan grounds them in that phase.
  const star = entry("star", "2026-01-05", null);
  for (const [phase, weakness] of [
    ["Result", "Quantify the outcome"],
    ["Action", "Say what you did"],
  ]) {
    star.report.communicationFeedback.improvements.push(weakness);
    star.report.improvementPlan.push({ ...plan(weakness)[0], phase });
    star.report.frameworkAssessment.phases.find(
      (row) => row.phase === phase,
    ).weaknessTags = [weakness];
  }
  const model = buildProgressModel([...rows, star]);
  assert.deepEqual(
    model.weaknesses,
    [
      {
        phase: "Algorithm",
        framework: "REACTO",
        count: 4,
        assessed: 4,
        tags: [
          "Develop a concrete algorithmic plan before attempting to write code.",
          "Practice articulating a step-by-step algorithm before attempting to write code.",
          "Develop a systematic approach to problem-solving before writing code.",
        ],
      },
      {
        phase: "Action",
        framework: "STAR",
        count: 1,
        assessed: 1,
        tags: ["Say what you did"],
      },
      {
        phase: "Result",
        framework: "STAR",
        count: 1,
        assessed: 1,
        tags: ["Quantify the outcome"],
      },
    ],
    "one row per phase, newest first, ties in framework order, and the unscored Algorithm row of the STAR round is not an assessment",
  );
});

test("one report's weaknesses keep their priority order and count as one attempt", () => {
  const ranked = [
    "Explain the algorithm before coding",
    "Justify time complexity",
    "Justify space complexity",
    "Name an alternative",
  ];
  const older = flaggedAttempt("older", "2026-01-01", [
    "Trace an example first",
  ]);
  const newer = flaggedAttempt("newer", "2026-01-02", ranked);
  const [algorithm] = buildProgressModel([newer, older]).weaknesses;
  assert.equal(
    algorithm.count,
    2,
    "four tags in one report are one flagged attempt",
  );
  assert.deepEqual(
    algorithm.tags,
    [...ranked, "Trace an example first"],
    "the report's own order leads, so the cap never hides its highest-ranked weakness",
  );
});

test("a wording that differs only in case, spacing or a full stop is listed once, as last written", () => {
  const model = buildProgressModel([
    flaggedAttempt("first", "2026-01-01", ["Explain complexity."]),
    flaggedAttempt("second", "2026-01-02", ["explain  complexity"]),
    flaggedAttempt("third", "2026-01-03", ["Explain complexity bounds"]),
  ]);
  assert.deepEqual(model.weaknesses[0].tags, [
    "Explain complexity bounds",
    "explain  complexity",
  ]);
  assert.equal(model.weaknesses[0].count, 3);
});

// The model is rebuilt on every filter change, over as many as five hundred
// reports, so deduplicating wordings has to stay linear. Counted rather than
// timed, so a slow CI host cannot fail it and a fast one cannot hide it: each
// wording is normalized, and so lowercased, once.
test("a long history normalizes each weakness wording once", () => {
  const history = Array.from({ length: 500 }, (_, index) =>
    flaggedAttempt(
      String(index),
      new Date(Date.UTC(2026, 0, 1) + index * 3_600_000).toISOString(),
      [0, 1, 2, 3].map((rank) => `Attempt ${index} weakness ${rank}`),
    ),
  );
  const normalized = normalizeProgressEntries(history);
  const lower = String.prototype.toLowerCase;
  let calls = 0;
  String.prototype.toLowerCase = function toLowerCase() {
    calls += 1;
    return lower.call(this);
  };
  let model;
  try {
    model = progressModelFrom(normalized);
  } finally {
    String.prototype.toLowerCase = lower;
  }
  assert.equal(model.weaknesses[0].tags.length, 2000);
  assert.equal(model.weaknesses[0].tags[0], "Attempt 499 weakness 0");
  assert.ok(calls <= 2000, `normalized ${calls} times for 2000 wordings`);
});

test("the denominator counts assessed attempts, and filters narrow both counts", () => {
  const counts = (model) =>
    model.weaknesses.map(({ count, assessed }) => ({ count, assessed }));
  const clean = entry("clean", "2026-01-02", 90, { language: "java" });
  const unscored = entry("unscored", "2026-01-03", null);
  const model = buildProgressModel([
    flaggedAttempt("flagged", "2026-01-01", ["Explain complexity"]),
    clean,
    unscored,
  ]);
  assert.deepEqual(
    counts(model),
    [{ count: 1, assessed: 2 }],
    "a scored phase without a weakness is assessed, an unscored one is not",
  );
  const history = [
    flaggedAttempt("flagged", "2026-01-01", ["Explain complexity"]),
    clean,
    entry("scored", "2026-01-04", 70),
  ];
  assert.deepEqual(counts(buildProgressModel(history)), [
    { count: 1, assessed: 3 },
  ]);
  assert.deepEqual(
    counts(buildProgressModel(history, { language: "python" })),
    [{ count: 1, assessed: 2 }],
    "the filter drops the java attempt from the denominator",
  );
  assert.deepEqual(
    buildProgressModel(history, { language: "java" }).weaknesses,
    [],
    "a phase flagged only outside the filter has no row",
  );
});

test("reports without a usable phase assessment add neither a weakness nor an assessment", () => {
  const incomplete = flaggedAttempt("incomplete", "2026-01-02", [
    "Explain complexity",
  ]);
  incomplete.report.incomplete = true;
  const legacy = {
    id: "legacy",
    date: "2026-01-03",
    report: { decision: "NO_HIRE" },
  };
  const ungrounded = flaggedAttempt("ungrounded", "2026-01-04", [
    "Trace an example",
  ]);
  ungrounded.report.improvementPlan = [];
  const counted = flaggedAttempt("counted", "2026-01-01", [
    "Name the invariant",
  ]);
  assert.deepEqual(
    buildProgressModel([incomplete, legacy, ungrounded, counted]).weaknesses,
    [
      {
        phase: "Algorithm",
        framework: "REACTO",
        count: 1,
        assessed: 2,
        tags: ["Name the invariant"],
      },
    ],
    "only the counted attempt flags; the ungrounded one still scored the phase, so it is assessed",
  );
});

test("progress groups attempts by topic", () => {
  const first = entry("first", "2026-01-01", 70);
  first.report = {
    ...first.report,
    topics: ["arrays", "hash tables"],
    decision: "HIRE",
  };
  const second = entry("second", "2026-01-03", 70);
  second.report = {
    ...second.report,
    topics: ["arrays", "arrays"],
    decision: "NO_HIRE",
  };
  const incomplete = entry("incomplete", "2026-01-04", 70);
  incomplete.report = {
    ...incomplete.report,
    topics: ["hash tables"],
    decision: "HIRE",
    incomplete: true,
  };
  const model = buildProgressModel([incomplete, second, first]);
  assert.deepEqual(model.topics, [
    { topic: "arrays", attempts: 2, passes: 1, lastAttempt: at(3) },
    { topic: "hash tables", attempts: 2, passes: 1, lastAttempt: at(4) },
  ]);
});

test("history without topics still builds", () => {
  const model = buildProgressModel([entry("old", "2026-01-01", 70)]);
  assert.deepEqual(model.topics, []);
  assert.equal(model.attempts.length, 1);
});

test("rubric changes and legacy attempts split otherwise comparable series", () => {
  const one = entry("one", "2026-01-01", 50);
  const legacy = { id: "legacy", date: "2026-02-01", report: {} };
  const two = entry("two", "2026-03-01", 70);
  const three = entry("three", "2026-04-01", 90);
  three.report.frameworkAssessment.rubricVersion = 2;
  const model = buildProgressModel([three, legacy, two, one]);
  assert.deepEqual(
    model.series.Algorithm.map((segment) => ({
      version: segment.rubricVersion,
      scores: segment.points.map((point) => point.score),
    })),
    [
      { version: 1, scores: [50] },
      { version: 1, scores: [70] },
      { version: 2, scores: [90] },
    ],
    "legacy data and a rubric change create explicit discontinuities",
  );
});

test("lobby progress surface is accessible and separates the two frameworks", () => {
  const page = readFileSync(join(web, "index.html"), "utf8");
  const app = readFileSync(join(web, "app.js"), "utf8");
  for (const id of [
    "progress-difficulty",
    "progress-language",
    "progress-duration",
    "progress-summary",
    "progress-trends",
    "progress-weaknesses",
    "progress-topics",
  ]) {
    assert.match(page, new RegExp(`id="${id}"`));
  }
  assert.match(page, /aria-labelledby="progress-title"/);
  assert.match(
    page,
    /<fieldset class="progress-filters">\s*<legend>Filter attempts<\/legend>/,
  );
  assert.match(app, /Rubric v\$\{segment\.rubricVersion\}/);
  assert.match(app, /Not assessed in these attempts/);
  assert.match(app, /no zeroes are plotted/);
  assert.match(app, /formative phase scores, not calibrated hiring evidence/);
  assert.match(app, /No topic labels are available for these attempts/);
  assert.match(app, /nodes\.progressTopics\.append\(item\)/);
  // A table each, with its own caption naming the exercise it scores. One
  // table with the two as groups inside it still read as a single ten-step
  // scale, and neither half is evidence about the other.
  assert.match(app, /for \(const framework of Object\.values\(FRAMEWORKS\)\)/);
  assert.match(
    app,
    /caption\.textContent = `\$\{framework\.name\} · \$\{framework\.scenario\}`/,
  );
  assert.match(app, /nodes\.progressTrends\.append\(table\);\s*\}/);
  assert.doesNotMatch(app, /colgroup/);
  assert.doesNotMatch(app, /Formative phase trends/);
});

// The lobby's picker and its progress panel both unwrap entries the same way
// (`pickerEntry` and `normalizeProgressEntry`), and the unwrap fallback is a
// named past regression:
// `/api/reports` carries the problem id on the stored row while the browser's
// own payload may not, and without the fallback every account report read as
// unattributed, so the picker recommended problems the candidate had already
// passed.
test("an account row lends its problem id to a payload that has none", () => {
  // The row carries it, the payload does not: the fallback is the whole point.
  const lent = normalizeProgressEntry({
    problemId: "two-sum",
    payload: { id: "a", report: { decision: "HIRE" } },
  });
  assert.equal(lent.problemId, "two-sum");
  assert.equal(lent.id, "a");
  // The payload's own id wins when it has one, so a row cannot relabel a report.
  assert.equal(
    normalizeProgressEntry({
      problemId: "two-sum",
      payload: { problemId: "lru-cache" },
    }).problemId,
    "lru-cache",
  );
  // A flat entry, the shape history.js stores, is read as itself.
  assert.equal(
    normalizeProgressEntry({ problemId: "valid-anagram" }).problemId,
    "valid-anagram",
  );
  // Neither has one: the result says so rather than inventing an id.
  assert.equal(normalizeProgressEntry({ payload: {} }).problemId, "");
  // A wrapper that is not an object, and a payload that is not one, are both
  // read as no wrapper at all rather than throwing on a property access.
  assert.equal(normalizeProgressEntry(null).problemId, "");
  assert.equal(
    normalizeProgressEntry({ payload: "not an object", problemId: "x" })
      .problemId,
    "x",
  );
});

// The filter menus the lobby renders come from here, and nothing looked at
// them at all: `model.options` was asserted by no test in the suite.
//
// Not asserted here: that the duration list sorts numerically rather than
// lexicographically. `normalizeProgressEntry` above admits only integers from
// 10 to 90, so every value is two digits and the two orderings agree on all of
// them. The comparator is unreachable as a difference through this surface, so
// a test claiming to pin it would be pinning nothing.
test("filter options are deduplicated, ordered, and free of unreadable values", () => {
  const entry = (day, difficulty, language, durationMin) => ({
    id: `r${day}`,
    date: at(day),
    problemId: "two-sum",
    difficulty,
    language,
    durationMin,
  });
  const model = buildProgressModel([
    entry(1, "Medium", "python", 45),
    entry(2, "Easy", "c", 10),
    entry(3, "Medium", "python", 90),
    // Rejected by normalizeProgressEntry, so they must not reach the menus.
    entry(4, "Impossible", "cobol", 7),
  ]);
  assert.deepEqual(
    model.options.durationMin,
    [10, 45, 90],
    "ascending, and 45 appears once",
  );
  assert.deepEqual(model.options.difficulty, ["Easy", "Medium"]);
  assert.deepEqual(model.options.language, ["c", "python"]);
});

// "all" is the sentinel the menus send when nothing is selected, and it has to
// mean "no filter" rather than being compared against as a value.
test("the all sentinel and a missing filter both select every attempt", () => {
  const entries = [
    {
      id: "a",
      date: at(1),
      problemId: "two-sum",
      difficulty: "Easy",
      language: "c",
      durationMin: 30,
    },
    {
      id: "b",
      date: at(2),
      problemId: "two-sum",
      difficulty: "Hard",
      language: "python",
      durationMin: 60,
    },
  ];
  assert.equal(buildProgressModel(entries, {}).attempts.length, 2);
  assert.equal(
    buildProgressModel(entries, { difficulty: "all", language: "all" }).attempts
      .length,
    2,
  );
  assert.equal(
    buildProgressModel(entries, { difficulty: null }).attempts.length,
    2,
  );
  assert.equal(
    buildProgressModel(entries, { difficulty: "Easy" }).attempts.length,
    1,
  );
  // A filter naming something nobody attempted empties the panel rather than
  // falling back to showing everything.
  assert.equal(
    buildProgressModel(entries, { difficulty: "Medium" }).attempts.length,
    0,
  );
});

// A body that is not a list is what a failed or half-written fetch produces.
test("a history that is not a list is an empty model, not a throw", () => {
  for (const nonsense of [undefined, null, "entries", 7, {}]) {
    const model = buildProgressModel(nonsense);
    assert.equal(model.total, 0, JSON.stringify(nonsense));
    assert.deepEqual(model.attempts, []);
    assert.deepEqual(model.options.difficulty, []);
  }
});

// The picker reads verdicts as they were saved. Through the sanitized report a
// pass from an older contract bundle lost its decision and an ungraded session
// became NO_HIRE, so the lobby recommended passed problems again and moved a
// candidate down a level for interviews nobody graded.
test("the picker keeps saved verdicts that the progress panel cannot score", () => {
  const bundle3 = {
    bundleVersion: 3,
    livePromptVersion: 1,
    reportPromptVersion: 3,
    reportSchemaVersion: 1,
    rubricVersion: 1,
  };
  const cards = [
    { id: "a", difficulty: "Medium" },
    { id: "b", difficulty: "Medium" },
  ];
  const oldPass = {
    problemId: "a",
    payload: {
      date: "2026-08-01T00:00:00Z",
      report: { decision: "HIRE", interviewContract: bundle3 },
    },
  };
  const entry = pickerEntry(oldPass);
  assert.equal(entry.report.decision, "HIRE");
  assert.equal(entry.at, Date.parse("2026-08-01T00:00:00Z"));
  assert.equal(
    normalizeProgressEntry(oldPass).report.incomplete,
    true,
    "the panel still refuses to score it",
  );
  assert.equal(
    pickProblem(
      cards,
      new Set(["Medium"]),
      [entry],
      () => 0,
      Date.parse("2026-08-01T12:00:00Z"),
    ).picked.id,
    "b",
  );

  const levels = [
    { id: "a", difficulty: "Medium" },
    { id: "b", difficulty: "Medium" },
    { id: "e", difficulty: "Easy" },
  ];
  const ungraded = [{ problemId: "a", report: null }, { problemId: "b" }].map(
    pickerEntry,
  );
  assert.equal(
    suggestDifficulty(levels, ungraded),
    null,
    "ungraded sessions read as failures",
  );
});

test("progress retains whether a verdict was actually stored for export", () => {
  for (const decision of ["HIRE", "NO_HIRE", null, undefined, "MAYBE"]) {
    for (const account of [false, true]) {
      const row = { date: "2026-01-02", report: { decision } };
      const normalized = normalizeProgressEntry(
        account ? { payload: row } : row,
      );
      assert.equal(
        normalized.recordedDecision,
        decision === "HIRE" || decision === "NO_HIRE" ? decision : null,
      );
    }
  }
});

for (const account of [false, true]) {
  test(`retry settings survive ${account ? "account" : "device"} history normalization`, () => {
    const saved = {
      date: "2026-01-01",
      durationMin: 60,
      interviewLoop: "coding_only",
      interviewMode: "whiteboard",
      report: {},
    };
    const attempt = normalizeProgressEntry(
      account ? { payload: saved } : saved,
    );
    assert.equal(attempt.durationMin, 60);
    assert.equal(attempt.interviewLoop, "coding_only");
    assert.equal(attempt.interviewMode, "whiteboard");
    assert.equal(saved.interviewMode, "whiteboard");
  });
}

test("retry mode metadata falls back to the original report and defaults safely", () => {
  const older = normalizeProgressEntry({
    report: { interviewLoop: "coding_only", interviewMode: "whiteboard" },
  });
  assert.equal(older.interviewLoop, "coding_only");
  assert.equal(older.interviewMode, "whiteboard");
  const legacy = normalizeProgressEntry({ report: {} });
  assert.equal(legacy.durationMin, null);
  assert.equal(legacy.interviewLoop, "coding_behavioral");
  assert.equal(legacy.interviewMode, "coding");
  const invalid = normalizeProgressEntry({
    interviewLoop: "invalid",
    interviewMode: "invalid",
    durationMin: "60",
    report: {},
  });
  assert.equal(invalid.durationMin, null);
  assert.equal(invalid.interviewLoop, "coding_behavioral");
  assert.equal(invalid.interviewMode, "coding");
});
