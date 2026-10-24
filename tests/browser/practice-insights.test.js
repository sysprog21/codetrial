import { test } from "node:test";
import assert from "node:assert/strict";

import {
  availableTopics,
  compareCategoryAttempt,
  filterProblems,
  recentPerformance,
} from "../../web/practice-insights.js";

import { ACTIVE_CONTRACT, reportContractStatus } from "../../web/lib.js";

const problems = [
  { id: "a", difficulty: "Easy", topics: ["Array", "Hash Table"] },
  { id: "b", difficulty: "Medium", topics: ["Array", "Sorting"] },
  { id: "c", difficulty: "Hard", topics: ["Graph"] },
];

const report = (problemId, at, decision) => ({
  problemId,
  at,
  report: {
    decision,
    topics: [
      ...(problems.find((problem) => problem.id === problemId)?.topics ?? []),
    ],
  },
});

test("topics are unique and sorted", () => {
  assert.deepEqual(availableTopics(problems), [
    "Array",
    "Graph",
    "Hash Table",
    "Sorting",
  ]);
});

test("difficulty and topic filters combine", () => {
  assert.deepEqual(
    filterProblems(problems, {
      difficulties: new Set(["Medium", "Hard"]),
      topic: "Array",
    }).map((problem) => problem.id),
    ["b"],
  );
});

test("title search is case-insensitive and combines with other filters", () => {
  const named = [
    {
      id: "a",
      title: "Chargeback Pair Match",
      difficulty: "Easy",
      topics: ["Array"],
    },
    {
      id: "b",
      title: "Packet Pair Match",
      difficulty: "Medium",
      topics: ["Graph"],
    },
    {
      id: "c",
      title: "Chargeback Audit",
      difficulty: "Medium",
      topics: ["Array"],
    },
  ];

  assert.deepEqual(
    filterProblems(named, { query: "  CHARGEBACK " }).map(
      (problem) => problem.id,
    ),
    ["a", "c"],
  );
  assert.deepEqual(
    filterProblems(named, {
      difficulties: new Set(["Medium"]),
      topic: "Array",
      query: "chargeback",
    }).map((problem) => problem.id),
    ["c"],
  );
  assert.deepEqual(filterProblems(named, { query: "no such title" }), []);
  assert.equal(filterProblems(named, { query: " " }).length, named.length);
});

test("title search matches English uppercase titles with lowercase queries", () => {
  const named = [
    { id: "a", title: "Integer to Roman", difficulty: "Easy", topics: [] },
  ];
  assert.deepEqual(
    filterProblems(named, { query: "int" }).map((problem) => problem.id),
    ["a"],
  );
});

test("search mode selects scenario or published title without changing difficulty filters", () => {
  const named = [
    {
      id: "a",
      title: "Chargeback Pair Match",
      leetcodeTitle: "Two Sum",
      difficulty: "Easy",
      topics: ["Array"],
    },
    {
      id: "b",
      title: "Symmetric Lottery Tickets",
      leetcodeTitle: "Symmetric Tree",
      difficulty: "Easy",
      topics: ["Tree"],
    },
    {
      id: "c",
      title: "Two Sum Scenario",
      leetcodeTitle: "Graph Paths",
      difficulty: "Medium",
      topics: ["Graph"],
    },
  ];
  assert.deepEqual(
    filterProblems(named, { query: "two sum" }).map((p) => p.id),
    ["c"],
  );
  assert.deepEqual(
    filterProblems(named, { query: " TWO sum ", searchBy: "leetcode" }).map(
      (p) => p.id,
    ),
    ["a"],
  );
  assert.deepEqual(
    filterProblems(named, {
      query: "two sum",
      searchBy: "leetcode",
      difficulties: new Set(["Medium"]),
    }),
    [],
  );
});

test("recent performance summarizes assessed reports newest first", () => {
  const reports = [
    report("a", 10, "NO_HIRE"),
    report("b", 30, "HIRE"),
    report("a", 20, "NO_HIRE"),
    report("c", 40, "HIRE"),
  ];

  assert.deepEqual(recentPerformance(reports, 3), {
    attempts: 3,
    legacyAttempts: 3,
    passes: 2,
    misses: 1,
    passRate: 67,
    streak: 2,
    latestDecision: "HIRE",
    weakTopics: ["Hash Table"],
    weakTopicDetails: [
      { topic: "Hash Table", misses: 1, passes: 0, lastAt: 20 },
    ],
  });
});

test("recent performance ignores unscored and undated entries", () => {
  assert.equal(
    recentPerformance([
      { problemId: "a", at: null, report: { decision: "NO_HIRE" } },
      { problemId: "b", at: 20, report: { decision: "PENDING" } },
    ]),
    null,
  );
});

test("recent performance ignores incomplete reports with a decision", () => {
  assert.equal(
    recentPerformance([
      {
        problemId: "a",
        at: 20,
        report: { incomplete: true, decision: "HIRE", topics: ["Array"] },
      },
    ]),
    null,
  );
});

test("topics with more misses than passes are highlighted", () => {
  const snapshot = recentPerformance([
    report("a", 30, "NO_HIRE"),
    report("a", 20, "NO_HIRE"),
    report("b", 10, "HIRE"),
  ]);

  assert.deepEqual(snapshot.weakTopics, ["Hash Table", "Array"]);
});

test("recent performance ignores malformed report topics", () => {
  const malformed = report("a", 20, "NO_HIRE");
  malformed.report.topics = 5;

  assert.deepEqual(recentPerformance([malformed]).weakTopics, []);
});

test("recent performance counts each normalized topic once per report", () => {
  const first = report("a", 20, "NO_HIRE");
  first.report.topics = ["Array", "Array", 5, "Hash Table"];
  const second = report("b", 10, "NO_HIRE");
  second.report.topics = ["Array"];

  assert.deepEqual(recentPerformance([first, second]).weakTopics, [
    "Array",
    "Hash Table",
  ]);
});

const versionedReport = (at, decision, contract = ACTIVE_CONTRACT) => {
  const entry = report("a", at, decision);
  entry.report.interviewContract = { ...contract };
  return entry;
};

test("recent performance excludes future and malformed contracts before limiting", () => {
  const future = versionedReport(40, "HIRE", {
    ...ACTIVE_CONTRACT,
    bundleVersion: ACTIVE_CONTRACT.bundleVersion + 1,
  });
  const malformed = versionedReport(30, "HIRE", {
    ...ACTIVE_CONTRACT,
    extra: 1,
  });
  const supported = versionedReport(20, "NO_HIRE");
  const snapshot = recentPerformance([future, malformed, supported], 1);
  assert.equal(snapshot.attempts, 1);
  assert.equal(snapshot.passes, 0);
  assert.equal(snapshot.legacyAttempts, 0);
  assert.equal(snapshot.latestDecision, "NO_HIRE");
  assert.equal(recentPerformance([future, malformed]), null);
});

test("recent performance counts legacy provenance only inside the window", () => {
  const legacy = report("a", 20, "HIRE");
  const supported = versionedReport(30, "NO_HIRE");
  assert.equal(recentPerformance([legacy]).legacyAttempts, 1);
  assert.equal(recentPerformance([legacy, supported], 1).legacyAttempts, 0);
  assert.equal(recentPerformance([legacy, supported], 2).legacyAttempts, 1);
});

test("contract classification shares the report compatibility policy", () => {
  assert.equal(reportContractStatus({}), "legacy");
  assert.equal(
    reportContractStatus({ interviewContract: ACTIVE_CONTRACT }),
    "supported",
  );
  for (const contract of [
    null,
    [],
    { ...ACTIVE_CONTRACT, extra: 1 },
    { ...ACTIVE_CONTRACT, rubricVersion: ACTIVE_CONTRACT.rubricVersion + 1 },
    { ...ACTIVE_CONTRACT, reportSchemaVersion: 999 },
    { ...ACTIVE_CONTRACT, bundleVersion: 3 },
    {
      ...ACTIVE_CONTRACT,
      livePromptVersion: ACTIVE_CONTRACT.livePromptVersion + 1,
    },
    {
      ...ACTIVE_CONTRACT,
      reportPromptVersion: ACTIVE_CONTRACT.reportPromptVersion + 1,
    },
  ]) {
    assert.equal(
      reportContractStatus({ interviewContract: contract }),
      "unsupported",
    );
  }
});

test("weak topic details retain ranking, per-topic counts and latest dates in the window", () => {
  const duplicate = report("a", 30, "NO_HIRE");
  duplicate.report.topics.push("Hash Table");
  const snapshot = recentPerformance(
    [
      report("a", 1, "NO_HIRE"),
      report("b", 20, "HIRE"),
      duplicate,
      report("a", 10, "NO_HIRE"),
    ],
    3,
  );
  assert.deepEqual(snapshot.weakTopicDetails, [
    { topic: "Hash Table", misses: 2, passes: 0, lastAt: 30 },
    { topic: "Array", misses: 2, passes: 1, lastAt: 30 },
  ]);
  assert.deepEqual(
    snapshot.weakTopicDetails.map(({ topic }) => topic),
    snapshot.weakTopics,
  );
});

test("weak topic details exclude incompatible, incomplete and undated evidence", () => {
  const future = versionedReport(40, "HIRE", {
    ...ACTIVE_CONTRACT,
    bundleVersion: ACTIVE_CONTRACT.bundleVersion + 1,
  });
  const incomplete = report("a", 50, "HIRE");
  incomplete.report.incomplete = true;
  const undated = report("a", null, "HIRE");
  assert.deepEqual(
    recentPerformance(
      [future, incomplete, undated, report("a", 20, "NO_HIRE")],
      1,
    ).weakTopicDetails,
    [
      { topic: "Array", misses: 1, passes: 0, lastAt: 20 },
      { topic: "Hash Table", misses: 1, passes: 0, lastAt: 20 },
    ],
  );
});

const categories = {
  "step-ways": "1D DP",
  "vault-sweep": "1D DP",
  "coin-payout": "1D DP",
  "window-bounds": "Sliding Window",
};

const assessedAttempt = ({
  id = "",
  problemId,
  problemTitle = problemId,
  at,
  decision = "HIRE",
  codingScore = 75,
  communicationScore = 80,
  hintsUsed = 1,
  rubricVersion = 1,
  phases = [],
  improvementPlan = [],
  incomplete = false,
}) => ({
  ...(id ? { id } : {}),
  problemId,
  problemTitle,
  at,
  report: {
    incomplete,
    decision,
    codingScore,
    communicationScore,
    hintsUsed,
    frameworkAssessment:
      rubricVersion === null
        ? null
        : {
            rubricVersion,
            phases,
          },
    improvementPlan,
  },
});

test("category comparison pairs a different problem in the same category and computes deltas", () => {
  const previous = assessedAttempt({
    id: "prev-1",
    problemId: "step-ways",
    problemTitle: "Step Ways",
    at: 100,
    decision: "NO_HIRE",
    codingScore: 60,
    communicationScore: 85,
    hintsUsed: 2,
  });
  const current = assessedAttempt({
    id: "curr-1",
    problemId: "vault-sweep",
    problemTitle: "Vault Sweep",
    at: 200,
    decision: "HIRE",
    codingScore: 82,
    communicationScore: 78,
    hintsUsed: 0,
  });

  const comparison = compareCategoryAttempt(
    [previous, current],
    current,
    categories,
  );
  assert.equal(comparison.category, "1D DP");
  assert.equal(comparison.sameProblem, false);
  assert.equal(comparison.previous.problemId, "step-ways");
  assert.equal(comparison.previous.problemTitle, "Step Ways");
  assert.equal(comparison.previous.decision, "NO_HIRE");
  assert.equal(comparison.codingScoreDelta, 22);
  assert.equal(comparison.communicationScoreDelta, -7);
  assert.equal(comparison.hintsUsedDelta, -2);
});

test("category comparison selects the most recent prior attempt in that category", () => {
  const oldest = assessedAttempt({
    id: "a1",
    problemId: "step-ways",
    at: 10,
    codingScore: 50,
  });
  const middle = assessedAttempt({
    id: "a2",
    problemId: "coin-payout",
    at: 30,
    codingScore: 68,
  });
  const newestPrior = assessedAttempt({
    id: "a3",
    problemId: "step-ways",
    at: 20,
    codingScore: 61,
  });
  const laterAttempt = assessedAttempt({
    id: "a5",
    problemId: "coin-payout",
    at: 50,
    codingScore: 95,
  });
  const current = assessedAttempt({
    id: "a4",
    problemId: "vault-sweep",
    at: 40,
    codingScore: 80,
  });

  const comparison = compareCategoryAttempt(
    [oldest, middle, newestPrior, laterAttempt, current],
    current,
    categories,
  );
  assert.equal(comparison.previous.id, "a2");
  assert.equal(comparison.previous.at, 30);
  assert.equal(comparison.codingScoreDelta, 12);
});

test("category comparison skips incomplete, unscored, and undated prior attempts", () => {
  const validPrior = assessedAttempt({
    id: "valid",
    problemId: "step-ways",
    at: 10,
    decision: "NO_HIRE",
    codingScore: 55,
  });
  const incompletePrior = assessedAttempt({
    id: "inc",
    problemId: "coin-payout",
    at: 20,
    decision: "HIRE",
    codingScore: 90,
    incomplete: true,
  });
  const unscoredPrior = assessedAttempt({
    id: "unscored",
    problemId: "coin-payout",
    at: 25,
    decision: "PENDING",
    codingScore: 90,
  });
  const undatedPrior = assessedAttempt({
    id: "undated",
    problemId: "coin-payout",
    at: null,
    decision: "HIRE",
    codingScore: 90,
  });
  // A raw stored entry with no decision gets coerced to NO_HIRE + 0 by
  // sanitizeReport inside normalizeProgressEntry, with recordedDecision: null.
  const sanitizedUnscoredPrior = {
    id: "sanitized-unscored",
    problemId: "coin-payout",
    problemTitle: "Coin Payout",
    date: "1970-01-01T00:00:00.028Z",
    report: { summary: "Legacy unscored session" },
  };
  const current = assessedAttempt({
    id: "curr",
    problemId: "vault-sweep",
    at: 30,
    decision: "HIRE",
    codingScore: 75,
  });

  const comparison = compareCategoryAttempt(
    [
      validPrior,
      incompletePrior,
      unscoredPrior,
      undatedPrior,
      sanitizedUnscoredPrior,
    ],
    current,
    categories,
  );
  assert.equal(comparison.previous.id, "valid");
  assert.equal(comparison.codingScoreDelta, 20);
  assert.equal(
    compareCategoryAttempt([validPrior], sanitizedUnscoredPrior, categories),
    null,
  );
  assert.equal(
    compareCategoryAttempt(
      [validPrior],
      { ...current, recordedDecision: null },
      categories,
    ),
    null,
  );
});

test("category comparison returns null on a first category attempt or incomplete current report", () => {
  const otherCategory = assessedAttempt({
    id: "other",
    problemId: "window-bounds",
    at: 10,
  });
  const current = assessedAttempt({
    id: "curr",
    problemId: "vault-sweep",
    at: 20,
  });
  const incompleteCurrent = assessedAttempt({
    id: "inc-curr",
    problemId: "vault-sweep",
    at: 30,
    incomplete: true,
  });
  const prototypeProblem = assessedAttempt({
    id: "proto",
    problemId: "__proto__",
    at: 30,
  });

  assert.equal(
    compareCategoryAttempt([otherCategory], current, categories),
    null,
  );
  assert.equal(
    compareCategoryAttempt([current], incompleteCurrent, categories),
    null,
  );
  assert.equal(
    compareCategoryAttempt([otherCategory], prototypeProblem, categories),
    null,
  );
});

test("category comparison computes per-phase score deltas when rubricVersion matches and skips null phases", () => {
  const previous = assessedAttempt({
    id: "prev",
    problemId: "step-ways",
    at: 10,
    rubricVersion: 1,
    phases: [
      { phase: "Repeat", score: 70 },
      { phase: "Algorithm", score: 55 },
      { phase: "Test", score: 60 },
      { phase: "Situation", score: null },
    ],
  });
  const current = assessedAttempt({
    id: "curr",
    problemId: "vault-sweep",
    at: 20,
    rubricVersion: 1,
    phases: [
      { phase: "Repeat", score: 70 },
      { phase: "Algorithm", score: 80 },
      { phase: "Test", score: 50 },
      { phase: "Situation", score: null },
    ],
  });

  const comparison = compareCategoryAttempt([previous], current, categories);
  assert.equal(comparison.rubricComparable, true);
  assert.equal(comparison.prevRubricVersion, 1);
  assert.equal(comparison.currRubricVersion, 1);
  assert.deepEqual(comparison.phaseDeltas, [
    { phase: "Repeat", previousScore: 70, currentScore: 70, delta: 0 },
    { phase: "Algorithm", previousScore: 55, currentScore: 80, delta: 25 },
    { phase: "Test", previousScore: 60, currentScore: 50, delta: -10 },
  ]);
});

test("category comparison omits per-phase score deltas across a rubricVersion boundary", () => {
  const previous = assessedAttempt({
    id: "prev",
    problemId: "step-ways",
    at: 10,
    codingScore: 60,
    rubricVersion: 1,
    phases: [{ phase: "Algorithm", score: 55 }],
    improvementPlan: [
      {
        phase: "Algorithm",
        weakness: "Jumped to code before recurrence",
        drill: "Write the state transition first",
        impact: "high",
      },
    ],
  });
  const current = assessedAttempt({
    id: "curr",
    problemId: "vault-sweep",
    at: 20,
    codingScore: 85,
    rubricVersion: 2,
    phases: [{ phase: "Algorithm", score: 80 }],
    improvementPlan: [],
  });

  const comparison = compareCategoryAttempt([previous], current, categories);
  assert.equal(comparison.rubricComparable, false);
  assert.equal(comparison.prevRubricVersion, 1);
  assert.equal(comparison.currRubricVersion, 2);
  assert.deepEqual(comparison.phaseDeltas, []);
  assert.equal(comparison.codingScoreDelta, 25);
  assert.equal(comparison.phaseComparisons.length, 1);
  assert.equal(comparison.phaseComparisons[0].status, "no_longer_flagged");
});

test("category comparison groups improvementPlan items by canonical phase without matching prose", () => {
  const prevAlgorithmItem = {
    phase: "Algorithm",
    weakness: "Did not justify overlapping subproblems",
    drill: "State the recurrence and memo table dimensions before coding",
    impact: "high",
  };
  const prevTestItem = {
    phase: "Test",
    weakness: "Skipped boundary input n = 1",
    drill: "Trace the smallest valid input by hand",
    impact: "medium",
  };
  const currAlgorithmItem = {
    phase: "Algorithm",
    weakness: "Recurrence was stated without space complexity",
    drill: "Compare O(n) table space against two rolling variables",
    impact: "medium",
  };
  const currOptimizationsItem = {
    phase: "Optimizations",
    weakness: "Left the rolling-array reduction unmentioned",
    drill: "Check which prior states the transition actually reads",
    impact: "low",
  };

  const previous = assessedAttempt({
    id: "prev",
    problemId: "step-ways",
    at: 10,
    improvementPlan: [prevTestItem, prevAlgorithmItem],
  });
  const current = assessedAttempt({
    id: "curr",
    problemId: "vault-sweep",
    at: 20,
    improvementPlan: [currOptimizationsItem, currAlgorithmItem],
  });

  const comparison = compareCategoryAttempt([previous], current, categories);
  assert.deepEqual(comparison.phaseComparisons, [
    {
      phase: "Algorithm",
      previousItems: [prevAlgorithmItem],
      currentItems: [currAlgorithmItem],
      status: "still_flagged",
    },
    {
      phase: "Test",
      previousItems: [prevTestItem],
      currentItems: [],
      status: "no_longer_flagged",
    },
    {
      phase: "Optimizations",
      previousItems: [],
      currentItems: [currOptimizationsItem],
      status: "newly_flagged",
    },
  ]);
});
