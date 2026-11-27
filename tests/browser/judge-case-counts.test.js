import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

import counts from "../../scripts/judge-case-counts.cjs";
import { checkAnswer } from "../../web/lib.js";

test("every scenario resolves to a judge with cases", () => {
  const checked = counts.validateJudgeCaseCounts();
  assert.equal(checked, Object.keys(counts.problemPages).length);
  assert.ok(checked > 0);
});

test("every judge case expects an answer its own checker accepts", () => {
  // The checkers that judge an answer against the input (palindrome,
  // indexPair and friends) read little or none of `expected`, so a wrong
  // expected value can sit in a judge unnoticed while the results panel
  // shows it as the answer. Membership only: for the palindrome judge, the
  // run of centre expansion over every case in compiler-explorer.test.js
  // is what pins how long an answer must be.
  for (const [problemId, { page }] of Object.entries(counts.problemPages)) {
    const spec = JSON.parse(
      fs.readFileSync(
        new URL(`../../web/judges/${page}.json`, import.meta.url),
        "utf8",
      ),
    );
    for (const testCase of spec.cases) {
      assert.ok(
        checkAnswer(spec, testCase, structuredClone(testCase.expected)),
        `"${problemId}" case "${testCase.label}" must expect an answer its checker accepts`,
      );
    }
  }
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

function browserCheck() {
  const file = new URL("../../scripts/browser-check.cjs", import.meta.url);
  const require = createRequire(file);
  let launches = 0;
  const context = vm.createContext({
    require: (name) =>
      name === "test-playwright"
        ? {
            chromium: {
              launch: () => {
                launches += 1;
                // Stop at the browser boundary without starting a real interview.
                return new Promise(() => {});
              },
            },
          }
        : require(name),
    process: {
      env: { ...process.env, PLAYWRIGHT_PATH: "test-playwright" },
    },
    console,
  });
  const script = new vm.Script(fs.readFileSync(file, "utf8"), {
    filename: fileURLToPath(file),
  });
  return {
    run: () => script.runInContext(context),
    evaluate: (code) => vm.runInContext(code, context),
    launches: () => launches,
  };
}

test("browser flows read their judges once before launch and reuse the counts", (t) => {
  const cases = [
    { input: [], expected: [] },
    { input: [], expected: [] },
  ];
  const readFile = fs.readFileSync;
  const judgeRoot = fileURLToPath(
    new URL("../../web/judges/", import.meta.url),
  );
  const judge = fileURLToPath(
    new URL(
      `../../web/judges/${counts.problemPages["two-sum"].page}.json`,
      import.meta.url,
    ),
  );
  const reads = new Map();
  t.mock.method(fs, "readFileSync", (file, ...args) => {
    if (typeof file === "string" && file.startsWith(judgeRoot))
      reads.set(file, (reads.get(file) ?? 0) + 1);
    return file === judge ? JSON.stringify({ cases }) : readFile(file, ...args);
  });
  const check = browserCheck();
  check.run();
  assert.equal(check.launches(), 1);
  assert.equal(reads.size, check.evaluate("Object.values(PROBLEM_IDS).length"));
  assert.ok([...reads.values()].every((count) => count === 1));
  cases.push({ input: [], expected: [] });
  for (let i = 0; i < 2; i++)
    assert.equal(check.evaluate("scenario(PROBLEM_IDS.twoSum).caseCount"), 2);
  assert.ok([...reads.values()].every((count) => count === 1));
  assert.throws(
    () => check.evaluate("scenario(undefined)"),
    /no scenario page for undefined/,
  );
});

test("a missing flow problem fails before the browser launches", (t) => {
  const original = counts.problemPages["two-sum"];
  delete counts.problemPages["two-sum"];
  t.after(() => {
    counts.problemPages["two-sum"] = original;
  });
  const check = browserCheck();
  assert.throws(() => check.run(), /no scenario page for two-sum/);
  assert.equal(check.launches(), 0);
});

test("a missing flow judge fails before the browser launches", (t) => {
  const judge = fileURLToPath(
    new URL(
      `../../web/judges/${counts.problemPages["two-sum"].page}.json`,
      import.meta.url,
    ),
  );
  const readFile = fs.readFileSync;
  t.mock.method(fs, "readFileSync", (file, ...args) =>
    readFile(file === judge ? file + ".missing-test-fixture" : file, ...args),
  );
  const check = browserCheck();
  assert.throws(() => check.run(), { code: "ENOENT" });
  assert.equal(check.launches(), 0);
});

test("an empty flow judge fails before the browser launches", (t) => {
  replaceJudge(t, () => ({ cases: [] }));
  const check = browserCheck();
  assert.throws(() => check.run(), /two-sum: judge has no cases/);
  assert.equal(check.launches(), 0);
});

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

const reportReference = JSON.parse(
  fs.readFileSync(
    new URL("../golden/report-python.json", import.meta.url),
    "utf8",
  ),
);

function reportCheck(reference, live) {
  const file = new URL("../../scripts/report-parity-check.sh", import.meta.url);
  const blocks = [
    ...fs
      .readFileSync(file, "utf8")
      .matchAll(/node << 'NODE'\n([\s\S]*?)\nNODE/g),
  ];
  assert.equal(
    blocks.length,
    2,
    "both report validation paths must be exercised",
  );
  const require = createRequire(import.meta.url);
  const errors = [];
  const output = [];
  const captureFiles = {
    "saved-capture": reference,
    "live-capture": live,
  };
  vm.runInNewContext(blocks[live ? 1 : 0][1], {
    require: (name) =>
      name === "fs"
        ? {
            readFileSync: (path, ...args) =>
              Object.hasOwn(captureFiles, path)
                ? JSON.stringify(captureFiles[path])
                : fs.readFileSync(path, ...args),
          }
        : require(name),
    process: {
      env: {
        PY_CAPTURE: "saved-capture",
        RUST_CAPTURE: "live-capture",
        WEB_ROOT: fileURLToPath(new URL("../../web", import.meta.url)),
        CASE_COUNTS_MODULE: fileURLToPath(
          new URL("../../scripts/judge-case-counts.cjs", import.meta.url),
        ),
      },
      exit: (code) => {
        throw new Error(`report check exited ${code}: ${errors.join("\n")}`);
      },
    },
    console: {
      error: (message) => errors.push(message),
      log: (message) => output.push(message),
    },
  });
  return output;
}

function liveReport(testResultText) {
  return {
    problemTitle: counts.problemPages["two-sum"].title,
    testResultText,
    report: {
      codingScore: 0,
      communicationScore: 50,
      decision: "NO_HIRE",
      summary: "Incomplete solution",
      codingFeedback: { strengths: [], improvements: [] },
      communicationFeedback: { strengths: [], improvements: [] },
      hintsUsed: 0,
    },
  };
}

test("report results separate passing counts from historical totals", () => {
  assert.deepEqual(counts.reportCaseCounts("Test results · 0/4"), {
    passed: 0,
    total: 4,
  });
  assert.deepEqual(counts.reportCaseCounts("Test results · 2/5"), {
    passed: 2,
    total: 5,
  });
  assert.equal(
    counts.reportCaseCounts(reportReference.testResultText).passed,
    0,
  );
});

test("malformed or impossible report counts fail validation", () => {
  for (const result of [
    "Couldn't run your code",
    "",
    undefined,
    null,
    ["Test results · 0/5"],
    "Test results · 6/5",
    "Test results · 0/0",
    "Test results · -1/5",
    "Test results · 0/9007199254740992",
  ]) {
    assert.throws(
      () => counts.reportCaseCounts(result),
      /invalid report result/,
    );
    assert.throws(
      () => counts.validateReportCaseCount(result, "two-sum"),
      /invalid report result/,
    );
  }
});

test("a stale live report total fails validation", () => {
  const total = counts.judgeCaseCount("two-sum");
  assert.deepEqual(
    counts.validateReportCaseCount(`Test results · 0/${total}`, "two-sum"),
    {
      passed: 0,
      total,
    },
  );
  assert.throws(
    () =>
      counts.validateReportCaseCount(
        `Test results · 0/${total + 1}`,
        "two-sum",
      ),
    /two-sum: report result.*does not match/,
  );
});

test("fixture-only report validation accepts historical totals after judge cases change", (t) => {
  replaceJudge(t, () => ({ cases: Array.from({ length: 6 }, () => ({})) }));
  assert.doesNotThrow(() => reportCheck(reportReference));
});

test("fixture-only report validation rejects invalid results and nonzero passing counts", () => {
  for (const testResultText of [
    undefined,
    "Couldn't run your code",
    "Test results · 5/4",
  ])
    assert.throws(
      () => reportCheck({ ...reportReference, testResultText }),
      /invalid report result/,
    );
  assert.throws(
    () =>
      reportCheck({ ...reportReference, testResultText: "Test results · 1/4" }),
    /zero passing cases/,
  );
});

test("live report validation follows the current judge instead of the saved total", (t) => {
  replaceJudge(t, () => ({ cases: Array.from({ length: 6 }, () => ({})) }));
  const output = reportCheck(reportReference, liveReport("Test results · 0/6"));
  assert.equal(output.length, 1);
  assert.match(output[0], /tests=Test results · 0\/6/);
  assert.throws(
    () => reportCheck(reportReference, liveReport("Test results · 0/5")),
    /does not match 6 judge cases/,
  );
  for (const result of ["Test results · 1/6", "Test results · 6/6"])
    assert.throws(
      () => reportCheck(reportReference, liveReport(result)),
      /passing test count mismatch/,
    );
  for (const result of [undefined, "Couldn't run your code"])
    assert.throws(
      () => reportCheck(reportReference, liveReport(result)),
      /invalid report result/,
    );
});

test("report result changes retain the report shape and problem checks", () => {
  const result = `Test results · 0/${counts.judgeCaseCount("two-sum")}`;
  const live = liveReport(result);
  assert.throws(
    () =>
      reportCheck(reportReference, { ...live, problemTitle: "Other problem" }),
    /rust problem mismatch/,
  );
  live.report.error = true;
  assert.throws(() => reportCheck(reportReference, live), /fallback report/);
});
