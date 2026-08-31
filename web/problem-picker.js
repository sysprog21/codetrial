export const LEVELS = ["Easy", "Medium", "Hard"];

/// How many results in a row at one level move a candidate off it. One is a
/// single lucky or unlucky problem. Three means somebody practising once a week
/// never moves at all.
const STREAK = 2;

/// Which level to practise next, read off what the candidate has already done.
///
/// `reports` is newest first, which both sources guarantee: `list_reports`
/// orders by `updated_at DESC` and `history.js` prepends. Reading it the other
/// way round would answer with the oldest interview on record.
///
/// Null is "no opinion", and the lobby leaves its checkboxes as they are. The
/// judgment used is `decision`, not `codingScore`: the agent already folds the
/// scores into that verdict, and a score threshold here would be a number
/// invented in the browser to second-guess it.
///
/// `from` comes back alongside `difficulty` because the sentence the lobby
/// writes is about the level just finished, which is not the level being
/// suggested whenever the streak moves the candidate.
export function suggestDifficulty(problems, reports) {
  const levelOf = new Map(problems.map((problem) => [problem.id, problem.difficulty]));
  const attempts = reports
    .map((entry) => ({
      level: levelOf.get(entry?.problemId),
      decision: entry?.report?.decision,
    }))
    // A problem since dropped from the bank has no level to count towards one,
    // and an entry with no verdict is not a result: an interview that ended
    // without a report writes one anyway. Reading anything that is not "HIRE"
    // as a failure marched those candidates down a level for interviews nobody
    // ever graded.
    .filter(
      (attempt) =>
        attempt.level && (attempt.decision === "HIRE" || attempt.decision === "NO_HIRE"),
    );
  if (!attempts.length) return null;

  const from = attempts[0].level;
  const recent = attempts.filter((attempt) => attempt.level === from).slice(0, STREAK);
  if (recent.length < STREAK) return null;
  if (recent.every((attempt) => attempt.decision === "HIRE")) {
    return { difficulty: step(from, 1), from, reason: "passed" };
  }
  if (recent.every((attempt) => attempt.decision === "NO_HIRE")) {
    return { difficulty: step(from, -1), from, reason: "struggled" };
  }
  return null;
}

/// Clamped at both ends: passing two Hard problems leaves the candidate on
/// Hard, which is still the right answer, and the lobby still says why.
function step(level, by) {
  const index = LEVELS.indexOf(level) + by;
  return LEVELS[Math.min(Math.max(index, 0), LEVELS.length - 1)];
}

/// Pick what to interview on next: a problem at one of the selected levels that
/// the candidate has not already been hired on. `reports` is the flat entry
/// shape `history.js` writes; the caller flattens `/api/reports` into it.
///
/// Returns `repeat` rather than hiding it, because a candidate who has passed
/// everything at a level is owed the sentence saying so instead of a
/// recommendation that looks new.
export function pickProblem(problems, difficulties, reports, random = Math.random) {
  const eligible = problems.filter((problem) => difficulties.has(problem.difficulty));
  const passed = new Set(
    reports.filter((entry) => entry?.report?.decision === "HIRE").map((entry) => entry.problemId),
  );
  const fresh = eligible.filter((problem) => !passed.has(problem.id));
  const choices = fresh.length ? fresh : eligible;
  const picked = choices[Math.floor(random() * choices.length)];
  return picked ? { picked, repeat: !fresh.length } : null;
}
