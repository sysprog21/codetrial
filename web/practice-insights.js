const ASSESSED = new Set(["HIRE", "NO_HIRE"]);
const PRACTICE_ATTEMPTS_KEY = "codetrial.practiceAttempts";
const PRACTICE_ATTEMPTS_LIMIT = 100;

export function practiceAttemptScope(session) {
  const login = session?.signedIn && typeof session?.user?.login === "string"
    ? session.user.login.trim().toLowerCase()
    : "";
  return login ? `github:${login}` : "anonymous";
}

function practiceAttemptsKey(scope) {
  const normalized = typeof scope === "string" && scope ? scope : "anonymous";
  return `${PRACTICE_ATTEMPTS_KEY}:${encodeURIComponent(normalized)}`;
}

function assessedEntries(reports) {
  return (Array.isArray(reports) ? reports : [])
    .filter((entry) => ASSESSED.has(entry?.report?.decision));
}

function topicsFor(entry, problemById) {
  const reported = Array.isArray(entry?.report?.topics) ? entry.report.topics : [];
  const fallback = problemById.get(entry?.problemId)?.topics ?? [];
  return [...new Set((reported.length ? reported : fallback)
    .filter((topic) => typeof topic === "string" && topic.trim())
    .map((topic) => topic.trim()))];
}

/// A compact, explainable reading of recent assessed interviews. "Weak" means
/// the recent window contains more misses than passes for that topic and at
/// least one miss; it is a practice signal, not a new score or hiring claim.
export function recentPerformance(problems, reports, limit = 8) {
  const problemById = new Map(problems.map((problem) => [problem.id, problem]));
  const attempts = assessedEntries(reports).slice(0, limit);
  const topicRows = new Map();
  for (const entry of attempts) {
    for (const topic of topicsFor(entry, problemById)) {
      const row = topicRows.get(topic) ?? { topic, attempts: 0, passes: 0, misses: 0 };
      row.attempts += 1;
      if (entry.report.decision === "HIRE") row.passes += 1;
      else row.misses += 1;
      topicRows.set(topic, row);
    }
  }
  const weakTopics = [...topicRows.values()]
    .filter((row) => row.misses > row.passes)
    .sort((left, right) =>
      right.misses - left.misses
      || right.attempts - left.attempts
      || left.topic.localeCompare(right.topic));
  const passes = attempts.filter((entry) => entry.report.decision === "HIRE").length;
  let currentStreak = 0;
  const latest = attempts[0]?.report?.decision;
  if (latest) {
    for (const entry of attempts) {
      if (entry.report.decision !== latest) break;
      currentStreak += 1;
    }
  }
  return {
    attempts: attempts.length,
    passes,
    misses: attempts.length - passes,
    passRate: attempts.length ? Math.round((passes / attempts.length) * 100) : null,
    latest,
    currentStreak,
    weakTopics,
  };
}

export function availableTopics(problems) {
  return [...new Set(problems.flatMap((problem) => problem.topics ?? []))]
    .sort((left, right) => left.localeCompare(right));
}

export function readPracticeAttempts({ storage, scope = "anonymous" } = {}) {
  try {
    storage ||= localStorage;
    const scopedKey = practiceAttemptsKey(scope);
    let stored = storage.getItem(scopedKey);
    // The old origin-wide key has no trustworthy account owner. Preserve it
    // under the anonymous scope instead of leaking it into whichever GitHub
    // account happens to sign in first after an upgrade.
    if (stored === null && scope === "anonymous") {
      stored = storage.getItem(PRACTICE_ATTEMPTS_KEY);
      if (stored !== null) {
        const migrated = validPracticeAttempts(JSON.parse(stored));
        storage.setItem(scopedKey, JSON.stringify(migrated));
        storage.removeItem(PRACTICE_ATTEMPTS_KEY);
        return migrated;
      }
    }
    return validPracticeAttempts(JSON.parse(stored));
  } catch {
    return [];
  }
}

function validPracticeAttempts(value) {
  return Array.isArray(value)
    ? value.filter((entry) => typeof entry?.problemId === "string" && Number.isFinite(entry?.at))
    : [];
}

/// Record when a candidate actually leaves the lobby for an interview.
/// Selecting a card is browsing, not practice. This write survives an
/// immediate tab close, before a report or provider connection can exist.
export function recordPracticeAttempt(problemId, {
  storage,
  now = Date.now(),
  scope = "anonymous",
} = {}) {
  try {
    storage ||= localStorage;
    const next = [{ problemId, at: now }, ...readPracticeAttempts({ storage, scope })]
      .slice(0, PRACTICE_ATTEMPTS_LIMIT);
    storage.setItem(practiceAttemptsKey(scope), JSON.stringify(next));
    return next;
  } catch {
    return [];
  }
}

export function clearPracticeAttempts({ storage, scope = "anonymous" } = {}) {
  try {
    (storage || localStorage).removeItem(practiceAttemptsKey(scope));
    return true;
  } catch {
    return false;
  }
}

/// Apply the candidate's local practice choices to both the visible bank and
/// the recommendation pool. Topic matching is OR within the selected topics;
/// difficulty remains the existing OR filter, and the two groups combine with
/// AND so each visible card satisfies every kind of choice shown above it.
export function filterProblems(problems, reports, {
  difficulties = new Set(),
  topics = new Set(),
  status = "all",
  attempts = [],
} = {}) {
  const attemptsByProblem = new Map();
  for (const entry of Array.isArray(reports) ? reports : []) {
    if (typeof entry?.problemId !== "string") continue;
    const row = attemptsByProblem.get(entry.problemId) ?? {
      attempted: false, failed: false, passed: false,
      latestReportAt: 0, latestReportAssessed: false, latestStartAt: 0,
    };
    row.attempted = true;
    row.failed ||= entry.report?.decision === "NO_HIRE";
    row.passed ||= entry.report?.decision === "HIRE";
    const at = Number.isFinite(entry.at) ? entry.at : 0;
    if (at >= row.latestReportAt) {
      row.latestReportAt = at;
      row.latestReportAssessed = ASSESSED.has(entry?.report?.decision);
    }
    attemptsByProblem.set(entry.problemId, row);
  }
  for (const entry of Array.isArray(attempts) ? attempts : []) {
    if (typeof entry?.problemId !== "string") continue;
    const row = attemptsByProblem.get(entry.problemId) ?? {
      attempted: false, failed: false, passed: false,
      latestReportAt: 0, latestReportAssessed: false, latestStartAt: 0,
    };
    row.attempted = true;
    row.latestStartAt = Math.max(row.latestStartAt, Number.isFinite(entry.at) ? entry.at : 0);
    attemptsByProblem.set(entry.problemId, row);
  }
  const weak = new Set(recentPerformance(problems, reports).weakTopics.map((row) => row.topic));
  return problems.filter((problem) => {
    if (difficulties.size && !difficulties.has(problem.difficulty)) return false;
    if (topics.size && !problem.topics.some((topic) => topics.has(topic))) return false;
    const history = attemptsByProblem.get(problem.id);
    if (status === "unseen" && history?.attempted) return false;
    if (status === "attempted" && !history?.attempted) return false;
    const incomplete = history?.attempted && (
      history.latestStartAt > history.latestReportAt
      || (!history.latestReportAssessed && history.latestReportAt >= history.latestStartAt)
    );
    if (status === "incomplete" && !incomplete) return false;
    if (status === "failed" && !history?.failed) return false;
    if (status === "passed" && !history?.passed) return false;
    if (status === "weak" && !problem.topics.some((topic) => weak.has(topic))) return false;
    return true;
  });
}
