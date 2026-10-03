export function availableTopics(problems) {
  return [...new Set(problems.flatMap((problem) => problem.topics ?? []))].sort(
    (left, right) => left.localeCompare(right),
  );
}

export function filterProblems(
  problems,
  { difficulties = new Set(), topic = "" } = {},
) {
  return problems.filter(
    (problem) =>
      (!difficulties.size || difficulties.has(problem.difficulty)) &&
      (!topic || problem.topics.includes(topic)),
  );
}

const ASSESSED = new Set(["HIRE", "NO_HIRE"]);

export function recentPerformance(reports, limit = 8) {
  const attempts = reports
    .filter(
      (entry) =>
        Number.isFinite(entry.at) &&
        !entry.report?.incomplete &&
        ASSESSED.has(entry.report?.decision),
    )
    .sort((left, right) => right.at - left.at)
    .slice(0, limit);
  if (!attempts.length) return null;

  const passes = attempts.filter(
    (entry) => entry.report.decision === "HIRE",
  ).length;
  const topicResults = new Map();
  for (const entry of attempts) {
    for (const topic of entry.report.topics ?? []) {
      const result = topicResults.get(topic) ?? { passes: 0, misses: 0 };
      result[entry.report.decision === "HIRE" ? "passes" : "misses"] += 1;
      topicResults.set(topic, result);
    }
  }
  const weakTopics = [...topicResults]
    .filter(([, result]) => result.misses > result.passes)
    .sort(
      ([leftTopic, left], [rightTopic, right]) =>
        right.misses - right.passes - (left.misses - left.passes) ||
        leftTopic.localeCompare(rightTopic),
    )
    .map(([topic]) => topic);
  const latestDecision = attempts[0].report.decision;
  const streak = attempts.findIndex(
    (entry) => entry.report.decision !== latestDecision,
  );

  return {
    attempts: attempts.length,
    passes,
    misses: attempts.length - passes,
    passRate: Math.round((passes / attempts.length) * 100),
    streak: streak === -1 ? attempts.length : streak,
    latestDecision,
    weakTopics,
  };
}
