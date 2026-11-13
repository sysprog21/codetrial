import {
  frameworkPhases,
  interviewMode,
  reportContractStatus,
  reportTopics,
} from "./lib.js";
import { normalizeProgressEntry } from "./progress.js";

export function availableTopics(problems) {
  return [...new Set(problems.flatMap((problem) => problem.topics ?? []))].sort(
    (left, right) => left.localeCompare(right),
  );
}

export function filterProblems(
  problems,
  {
    difficulties = new Set(),
    topic = "",
    query = "",
    searchBy = "scenario",
  } = {},
) {
  const trimmedQuery = query.trim();
  const needle = trimmedQuery.toLowerCase();

  // "20", "20.", "#20" and "020" all name problem 20.
  const number =
    searchBy === "leetcode"
      ? trimmedQuery.match(/^#?0*(\d+)\.?$/)?.[1]
      : undefined;

  return problems.filter((problem) => {
    if (difficulties.size && !difficulties.has(problem.difficulty))
      return false;
    if (topic && !problem.topics.includes(topic)) return false;
    if (!needle) return true;

    if (searchBy === "leetcode") {
      const leetcodeTitle = problem.leetcodeTitle ?? "";
      if (number) {
        const publishedTitle = leetcodeTitle.replace(/^\d+\. /, "");
        return (
          leetcodeTitle.startsWith(`${number}. `) ||
          publishedTitle.toLowerCase().includes(number)
        );
      }
      return leetcodeTitle.toLowerCase().includes(needle);
    } else {
      const title = problem.title ?? "";
      return title.toLowerCase().includes(needle);
    }
  });
}

const ASSESSED = new Set(["HIRE", "NO_HIRE"]);

export function recentPerformance(reports, limit = 8) {
  const attempts = reports
    .filter(
      (entry) =>
        Number.isFinite(entry.at) &&
        !entry.report?.incomplete &&
        ASSESSED.has(entry.report?.decision) &&
        reportContractStatus(entry.report) !== "unsupported",
    )
    .sort((left, right) => right.at - left.at)
    .slice(0, limit);
  if (!attempts.length) return null;

  const passes = attempts.filter(
    (entry) => entry.report.decision === "HIRE",
  ).length;
  const topicResults = new Map();
  // Newest first, so the attempt that creates a topic's row is its latest.
  for (const entry of attempts) {
    for (const topic of new Set(reportTopics(entry.report) ?? [])) {
      const result = topicResults.get(topic) ?? {
        passes: 0,
        misses: 0,
        lastAt: entry.at,
      };
      result[entry.report.decision === "HIRE" ? "passes" : "misses"] += 1;
      topicResults.set(topic, result);
    }
  }
  const weakTopicDetails = [...topicResults]
    .filter(([, result]) => result.misses > result.passes)
    .sort(
      ([leftTopic, left], [rightTopic, right]) =>
        right.misses - right.passes - (left.misses - left.passes) ||
        leftTopic.localeCompare(rightTopic),
    )
    .map(([topic, result]) => ({ topic, ...result }));
  const weakTopics = weakTopicDetails.map(({ topic }) => topic);
  const latestDecision = attempts[0].report.decision;
  const streak = attempts.findIndex(
    (entry) => entry.report.decision !== latestDecision,
  );

  return {
    attempts: attempts.length,
    legacyAttempts: attempts.filter(
      (entry) => reportContractStatus(entry.report) === "legacy",
    ).length,
    passes,
    misses: attempts.length - passes,
    passRate: Math.round((passes / attempts.length) * 100),
    streak: streak === -1 ? attempts.length : streak,
    latestDecision,
    weakTopics,
    weakTopicDetails,
  };
}

const canonicalPhase = new Map(
  frameworkPhases.flatMap((label) => [
    [label, label],
    [label.toLowerCase(), label],
  ]),
);

function categoryFor(categories, problemId) {
  return categories &&
    typeof categories === "object" &&
    typeof problemId === "string" &&
    Object.hasOwn(categories, problemId) &&
    typeof categories[problemId] === "string" &&
    categories[problemId]
    ? categories[problemId]
    : null;
}

/// Callers in the lobby pass entries `normalizeProgressEntry` already produced;
/// callers holding a raw local or `/api/reports` row normalize through it here
/// so both paths share one rule for timestamps, titles, and sanitized reports.
function unwrapAttempt(raw) {
  if (!raw || typeof raw !== "object") return null;
  if (
    !raw.payload &&
    raw.date === undefined &&
    raw.createdAt === undefined &&
    raw.report &&
    typeof raw.report === "object"
  ) {
    const id = typeof raw.id === "string" ? raw.id : "";
    const problemId = typeof raw.problemId === "string" ? raw.problemId : "";
    const problemTitle =
      typeof raw.problemTitle === "string" && raw.problemTitle.trim()
        ? raw.problemTitle
        : problemId || "Past interview";
    return {
      id,
      at: Number.isFinite(raw.at) ? raw.at : null,
      problemId,
      problemTitle,
      interviewMode: interviewMode(
        raw.interviewMode ?? raw.report?.interviewMode,
      ),
      recordedDecision: raw.recordedDecision,
      scored: raw.scored,
      recordedCodingScore: raw.recordedCodingScore,
      recordedCommunicationScore: raw.recordedCommunicationScore,
      recordedHintsUsed: raw.recordedHintsUsed,
      report: raw.report,
    };
  }
  return normalizeProgressEntry(raw);
}

function isAssessedAttempt(entry) {
  if (typeof entry?.scored === "boolean") return entry.scored;
  return (
    Boolean(entry?.report) &&
    !entry.report.incomplete &&
    (entry.recordedDecision !== undefined
      ? ASSESSED.has(entry.recordedDecision)
      : ASSESSED.has(entry.report.decision))
  );
}

function rubricVersionOf(report) {
  const version = report?.frameworkAssessment?.rubricVersion;
  return Number.isSafeInteger(version) && version >= 1 ? version : null;
}

function finiteMetric(entry, recordedKey, reportKey) {
  const value =
    entry?.[recordedKey] !== undefined
      ? entry[recordedKey]
      : entry?.report?.[reportKey];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function metricDelta(previous, current) {
  return previous !== null && current !== null ? current - previous : null;
}

function phaseScoreMap(assessment) {
  const scores = new Map();
  for (const row of Array.isArray(assessment?.phases)
    ? assessment.phases
    : []) {
    const phase = canonicalPhase.get(row?.phase);
    if (
      phase &&
      !scores.has(phase) &&
      typeof row.score === "number" &&
      Number.isInteger(row.score)
    ) {
      scores.set(phase, row.score);
    }
  }
  return scores;
}

function planByPhase(plan) {
  const grouped = new Map();
  for (const item of Array.isArray(plan) ? plan : []) {
    const phase = canonicalPhase.get(item?.phase);
    if (!phase) continue;
    const items = grouped.get(phase) ?? [];
    items.push(item);
    grouped.set(phase, items);
  }
  return grouped;
}

/// Compares `currentAttempt` against the most recent prior assessed attempt in
/// the same study-plan category.
///
/// Phase scores are subtracted only within the same `rubricVersion`, for the
/// reason `progress.js` breaks chart segments at version boundaries: a score
/// under a revised rubric is not on the old scale. `improvementPlan` items are
/// paired by their closed `phase` id rather than by `weakness` wording, which
/// the report model writes afresh each session.
export function compareCategoryAttempt(entries, currentAttempt, categories) {
  if (!Array.isArray(entries)) return null;
  const current = unwrapAttempt(currentAttempt);
  if (!isAssessedAttempt(current)) {
    return null;
  }
  const category = categoryFor(categories, current.problemId);
  if (!category) return null;

  const previous = entries
    .filter((raw) => raw !== currentAttempt)
    .map(unwrapAttempt)
    .filter(
      (entry) =>
        entry !== null &&
        !(current.id && entry.id && entry.id === current.id) &&
        entry.at !== null &&
        (current.at === null || entry.at < current.at) &&
        isAssessedAttempt(entry) &&
        entry.interviewMode === current.interviewMode &&
        categoryFor(categories, entry.problemId) === category,
    )
    .sort((left, right) => right.at - left.at)[0];
  if (!previous) return null;

  const previousCodingScore = finiteMetric(
    previous,
    "recordedCodingScore",
    "codingScore",
  );
  const currentCodingScore = finiteMetric(
    current,
    "recordedCodingScore",
    "codingScore",
  );
  const previousCommunicationScore = finiteMetric(
    previous,
    "recordedCommunicationScore",
    "communicationScore",
  );
  const currentCommunicationScore = finiteMetric(
    current,
    "recordedCommunicationScore",
    "communicationScore",
  );
  const previousHintsUsed = finiteMetric(
    previous,
    "recordedHintsUsed",
    "hintsUsed",
  );
  const currentHintsUsed = finiteMetric(
    current,
    "recordedHintsUsed",
    "hintsUsed",
  );

  const prevRubricVersion = rubricVersionOf(previous.report);
  const currRubricVersion = rubricVersionOf(current.report);
  const rubricComparable =
    prevRubricVersion !== null && prevRubricVersion === currRubricVersion;
  const prevScores = phaseScoreMap(previous.report.frameworkAssessment);
  const currScores = phaseScoreMap(current.report.frameworkAssessment);
  const phaseDeltas = rubricComparable
    ? frameworkPhases
        .filter((phase) => prevScores.has(phase) && currScores.has(phase))
        .map((phase) => {
          const previousScore = prevScores.get(phase);
          const currentScore = currScores.get(phase);
          return {
            phase,
            previousScore,
            currentScore,
            delta: currentScore - previousScore,
          };
        })
    : [];

  const prevByPhase = planByPhase(previous.report.improvementPlan);
  const currByPhase = planByPhase(current.report.improvementPlan);
  const phaseComparisons = frameworkPhases
    .filter((phase) => prevByPhase.has(phase))
    .map((phase) => {
      const previousItems = prevByPhase.get(phase);
      const currentItems = currByPhase.get(phase) ?? [];
      return {
        phase,
        previousItems,
        currentItems,
        status:
          currentItems.length === 0 ? "no_longer_flagged" : "still_flagged",
      };
    });

  return {
    category,
    sameProblem: previous.problemId === current.problemId,
    previous: {
      id: previous.id,
      at: previous.at,
      problemId: previous.problemId,
      problemTitle: previous.problemTitle,
      decision: previous.report.decision,
      codingScore: previousCodingScore,
      communicationScore: previousCommunicationScore,
      hintsUsed: previousHintsUsed,
    },
    current: {
      id: current.id,
      at: current.at,
      problemId: current.problemId,
      problemTitle: current.problemTitle,
      decision: current.report.decision,
      codingScore: currentCodingScore,
      communicationScore: currentCommunicationScore,
      hintsUsed: currentHintsUsed,
    },
    codingScoreDelta: metricDelta(previousCodingScore, currentCodingScore),
    communicationScoreDelta: metricDelta(
      previousCommunicationScore,
      currentCommunicationScore,
    ),
    hintsUsedDelta: metricDelta(previousHintsUsed, currentHintsUsed),
    rubricComparable,
    prevRubricVersion,
    currRubricVersion,
    phaseDeltas,
    phaseComparisons,
  };
}
