import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { fileURLToPath } from "node:url";

import counts from "../../scripts/judge-case-counts.cjs";

test("every scenario resolves to a judge with cases", () => {
  const checked = counts.validateJudgeCaseCounts();
  assert.equal(checked, Object.keys(counts.problemPages).length);
  assert.ok(checked > 0);
});

function replaceJudge(t, fixture) {
  const file = fileURLToPath(
    new URL(
      `../../web/judges/${counts.problemPages["two-sum"].page}.json`,
      import.meta.url,
    ),
  );
  const readFile = fs.readFileSync;
  t.mock.method(fs, "readFileSync", (path, ...args) =>
    path === file ? JSON.stringify(fixture()) : readFile(path, ...args),
  );
}

test("browser case counts follow changes to their judge fixture", (t) => {
  const cases = [
    { input: [], expected: [] },
    { input: [], expected: [] },
  ];
  replaceJudge(t, () => ({ cases }));
  assert.equal(counts.judgeCaseCount("two-sum"), 2);
  cases.push({ input: [], expected: [] });
  assert.equal(counts.judgeCaseCount("two-sum"), 3);
  assert.doesNotThrow(() =>
    counts.validateReportCaseCount("Test results · 0/3", "two-sum"),
  );
  assert.throws(
    () => counts.validateReportCaseCount("Test results · 0/2", "two-sum"),
    /does not match 3 judge cases/,
  );
});

test("an unknown or omitted problem has no default case count", () => {
  for (const problemId of ["unknown", undefined])
    assert.throws(
      () => counts.judgeCaseCount(problemId),
      /no scenario page for/,
    );
});

test("empty or malformed judge cases fail validation", (t) => {
  let fixture;
  replaceJudge(t, () => fixture);
  for (fixture of [{}, { cases: [] }, { cases: {} }]) {
    assert.throws(
      () => counts.judgeCaseCount("two-sum"),
      /two-sum: judge has no cases/,
    );
    assert.throws(
      () => counts.validateJudgeCaseCounts(),
      /two-sum: judge has no cases/,
    );
  }
});

test("the report reference checks its total against the two-sum judge", () => {
  const reference = JSON.parse(
    fs.readFileSync(
      new URL("../golden/report-python.json", import.meta.url),
      "utf8",
    ),
  );
  assert.doesNotThrow(() =>
    counts.validateReportCaseCount(reference.testResultText, "two-sum"),
  );
});

test("a stale or missing report result fails validation", () => {
  const total = counts.judgeCaseCount("two-sum");
  for (const result of [
    `Test results · 0/${total + 1}`,
    "Couldn't run your code",
    "",
    undefined,
  ])
    assert.throws(
      () => counts.validateReportCaseCount(result, "two-sum"),
      /two-sum: report result.*does not match/,
    );
});
