import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { buildProgressModel, normalizeProgressEntry, progressPhases } from "../../web/progress.js";

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

test("progress normalizes local and account wrappers without rewriting legacy history", () => {
  const local = entry("local", "2026-01-02T00:00:00Z", 70);
  const account = { id: "db", createdAt: 1_767_225_600, payload: entry("payload", null, 80) };
  delete account.payload.date;
  assert.equal(normalizeProgressEntry(local).id, "local");
  assert.equal(normalizeProgressEntry(account).id, "payload");
  const legacy = normalizeProgressEntry({ id: "old", date: "2025-01-01", report: { decision: "HIRE" } });
  assert.equal(legacy.report.frameworkAssessment, null);
  assert.equal(legacy.mode, null, "legacy mode is unknown, not silently scored");
  const hostile = normalizeProgressEntry({ date: "not-a-date", difficulty: "Expert", language: "brainfuck", durationMin: 1, mode: "hire", report: {} });
  assert.equal(hostile.at, null);
  assert.deepEqual([hostile.difficulty, hostile.language, hostile.durationMin, hostile.mode], [null, null, null, null]);
});

test("progress filters metadata, orders attempts, ranks tags, and keeps null as a gap", () => {
  const rows = [
    entry("later", "2026-02-01", 80, { weakness: "Explain complexity" }),
    entry("earlier", "2026-01-01", 60, { weakness: "Explain complexity" }),
    entry("other", "2026-03-01", 99, { difficulty: "Hard", language: "java", durationMin: 60, mode: "practice" }),
  ];
  const model = buildProgressModel(rows, { difficulty: "Easy", language: "python", durationMin: "45", mode: "scored" });
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

test("lobby progress surface is accessible and exposes all four filters", () => {
  const page = readFileSync(join(web, "index.html"), "utf8");
  const app = readFileSync(join(web, "app.js"), "utf8");
  for (const id of ["progress-difficulty", "progress-language", "progress-duration", "progress-mode", "progress-summary", "progress-trends", "progress-weaknesses"]) {
    assert.match(page, new RegExp(`id="${id}"`));
  }
  assert.match(page, /aria-labelledby="progress-title"/);
  assert.match(page, /<fieldset class="progress-filters">\s*<legend>Filter attempts<\/legend>/);
  assert.match(app, /Rubric v\$\{segment\.rubricVersion\}/);
  assert.match(app, /Not assessed in these attempts/);
  assert.match(app, /no zeroes are plotted/);
});
