import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { buildProgressModel, normalizeProgressEntry, progressPhases, unwrapEntry } from "../../web/progress.js";

const web = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "web");
const assessment = (version, algorithm, action = null, tags = []) => ({
  rubricVersion: version,
  phases: progressPhases.map((phase) => ({
    phase,
    score: phase === "Algorithm" ? algorithm : phase === "Action" ? action : null,
    weaknessTags: phase === "Algorithm" ? tags : [],
  })),
});
const plan = (weakness) => [{
  phase: "Algorithm", weakness, impact: "high", frequency: 1, drill: "Narrate",
  durationMin: 5, successCriterion: "Justify bounds", selfReview: ["time"],
}];
const entry = (id, date, score, overrides = {}) => ({
  id, date, problemId: "two-sum", problemTitle: "Two Sum", difficulty: "Easy",
  language: "python", durationMin: 45, mode: "scored",
  report: {
    codingFeedback: { improvements: overrides.weakness ? [overrides.weakness] : [] },
    communicationFeedback: { improvements: [] },
    improvementPlan: overrides.weakness ? plan(overrides.weakness) : [],
    frameworkAssessment: assessment(1, score, null, overrides.weakness ? [overrides.weakness] : []),
  },
  ...overrides,
});

/// A January 2026 day as epoch milliseconds. Module level because two tests
/// below want the same clock and a copy each is a copy to keep in step.
const at = (day) => new Date(Date.UTC(2026, 0, day)).getTime();

test("progress normalizes local and account wrappers without rewriting legacy history", () => {
  const local = entry("local", "2026-01-02T00:00:00Z", 70);
  const account = { id: "db", createdAt: 1_767_225_600, payload: entry("payload", null, 80) };
  delete account.payload.date;
  assert.equal(normalizeProgressEntry(local).id, "local");
  assert.equal(normalizeProgressEntry(account).id, "payload");
  const legacy = normalizeProgressEntry({ id: "old", date: "2025-01-01", report: { decision: "HIRE" } });
  assert.equal(legacy.report.frameworkAssessment, null);
  const hostile = normalizeProgressEntry({ date: "not-a-date", difficulty: "Expert", language: "brainfuck", durationMin: 1, mode: "hire", report: {} });
  assert.equal(hostile.at, null);
  assert.deepEqual([hostile.difficulty, hostile.language, hostile.durationMin], [null, null, null]);
});

test("progress filters metadata, orders attempts, ranks tags, and keeps null as a gap", () => {
  const rows = [
    entry("later", "2026-02-01", 80, { weakness: "Explain complexity" }),
    entry("earlier", "2026-01-01", 60, { weakness: "Explain complexity" }),
    entry("other", "2026-03-01", 99, { difficulty: "Hard", language: "java", durationMin: 60 }),
  ];
  const model = buildProgressModel(rows, { difficulty: "Easy", language: "python", durationMin: "45" });
  assert.deepEqual(model.attempts.map((item) => item.id), ["earlier", "later"]);
  assert.deepEqual(model.series.Algorithm[0].points.map((point) => point.score), [60, 80]);
  assert.deepEqual(model.series.Action, [], "null STAR values are gaps, not zeroes");
  assert.deepEqual(model.weaknesses, [{ tag: "Explain complexity", count: 2 }]);
});

test("rubric changes and legacy attempts split otherwise comparable series", () => {
  const one = entry("one", "2026-01-01", 50);
  const legacy = { id: "legacy", date: "2026-02-01", report: {} };
  const two = entry("two", "2026-03-01", 70);
  const three = entry("three", "2026-04-01", 90);
  three.report.frameworkAssessment.rubricVersion = 2;
  const model = buildProgressModel([three, legacy, two, one]);
  assert.deepEqual(model.series.Algorithm.map((segment) => ({
    version: segment.rubricVersion,
    scores: segment.points.map((point) => point.score),
  })), [
    { version: 1, scores: [50] },
    { version: 1, scores: [70] },
    { version: 2, scores: [90] },
  ], "legacy data and a rubric change create explicit discontinuities");
});

test("lobby progress surface is accessible and separates the two frameworks", () => {
  const page = readFileSync(join(web, "index.html"), "utf8");
  const app = readFileSync(join(web, "app.js"), "utf8");
  for (const id of ["progress-difficulty", "progress-language", "progress-duration", "progress-summary", "progress-trends", "progress-weaknesses"]) {
    assert.match(page, new RegExp(`id="${id}"`));
  }
  assert.match(page, /aria-labelledby="progress-title"/);
  assert.match(page, /<fieldset class="progress-filters">\s*<legend>Filter attempts<\/legend>/);
  assert.match(app, /Rubric v\$\{segment\.rubricVersion\}/);
  assert.match(app, /Not assessed in these attempts/);
  assert.match(app, /no zeroes are plotted/);
  assert.match(app, /formative phase scores, not calibrated hiring evidence/);
  // A table each, with its own caption naming the exercise it scores. One
  // table with the two as groups inside it still read as a single ten-step
  // scale, and neither half is evidence about the other.
  assert.match(app, /for \(const framework of Object\.values\(FRAMEWORKS\)\)/);
  assert.match(app, /caption\.textContent = `\$\{framework\.name\} · \$\{framework\.scenario\}`/);
  assert.match(app, /nodes\.progressTrends\.append\(table\);\s*\}/);
  assert.doesNotMatch(app, /colgroup/);
  assert.doesNotMatch(app, /Formative phase trends/);
});

// `unwrapEntry` is exported and, until now, called by no test: the lobby's
// picker and its progress panel both read it, but only through a Playwright
// scenario that skips wherever Chromium is not installed. Its fallback is a
// named past regression -- `/api/reports` carries the problem id on the stored
// row while the browser's own payload may not, and without the fallback every
// account report read as unattributed, so the picker recommended problems the
// candidate had already passed.
test("an account row lends its problem id to a payload that has none", () => {
  // The row carries it, the payload does not: the fallback is the whole point.
  assert.deepEqual(
    unwrapEntry({ problemId: "two-sum", payload: { id: "a", report: { decision: "HIRE" } } }),
    { id: "a", report: { decision: "HIRE" }, problemId: "two-sum" },
  );
  // The payload's own id wins when it has one, so a row cannot relabel a report.
  assert.equal(
    unwrapEntry({ problemId: "two-sum", payload: { problemId: "lru-cache" } }).problemId,
    "lru-cache",
  );
  // A flat entry, the shape history.js stores, is returned as itself.
  assert.deepEqual(unwrapEntry({ problemId: "valid-anagram" }), { problemId: "valid-anagram" });
  // Neither has one: the result says so rather than inventing an id, and
  // `undefined` is what `normalizeProgressEntry` turns into "".
  assert.equal(unwrapEntry({ payload: {} }).problemId, undefined);
  // A wrapper that is not an object, and a payload that is not one, are both
  // read as no wrapper at all rather than throwing on a property access.
  assert.deepEqual(unwrapEntry(null), { problemId: undefined });
  assert.deepEqual(unwrapEntry({ payload: "not an object", problemId: "x" }),
    { payload: "not an object", problemId: "x" });
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
    id: `r${day}`, date: at(day), problemId: "two-sum", difficulty, language, durationMin,
  });
  const model = buildProgressModel([
    entry(1, "Medium", "python", 45),
    entry(2, "Easy", "c", 10),
    entry(3, "Medium", "python", 90),
    // Rejected by normalizeProgressEntry, so they must not reach the menus.
    entry(4, "Impossible", "cobol", 7),
  ]);
  assert.deepEqual(model.options.durationMin, [10, 45, 90], "ascending, and 45 appears once");
  assert.deepEqual(model.options.difficulty, ["Easy", "Medium"]);
  assert.deepEqual(model.options.language, ["c", "python"]);
});

// "all" is the sentinel the menus send when nothing is selected, and it has to
// mean "no filter" rather than being compared against as a value.
test("the all sentinel and a missing filter both select every attempt", () => {
  const entries = [
    { id: "a", date: at(1), problemId: "two-sum", difficulty: "Easy", language: "c", durationMin: 30 },
    { id: "b", date: at(2), problemId: "two-sum", difficulty: "Hard", language: "python", durationMin: 60 },
  ];
  assert.equal(buildProgressModel(entries, {}).attempts.length, 2);
  assert.equal(buildProgressModel(entries, { difficulty: "all", language: "all" }).attempts.length, 2);
  assert.equal(buildProgressModel(entries, { difficulty: null }).attempts.length, 2);
  assert.equal(buildProgressModel(entries, { difficulty: "Easy" }).attempts.length, 1);
  // A filter naming something nobody attempted empties the panel rather than
  // falling back to showing everything.
  assert.equal(buildProgressModel(entries, { difficulty: "Medium" }).attempts.length, 0);
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
