export const LEVELS = ["Easy", "Medium", "Hard"];

/// How many results in a row at one level move a candidate off it. One is a
/// single lucky or unlucky problem. Three means somebody practising once a week
/// never moves at all.
const STREAK = 2;

/// Each successful recall earns a longer break before the same problem returns.
/// This is intentionally a small, explainable schedule: reports record a
/// verdict and a timestamp, not a confidence rating, so guessing a more precise
/// retention model would promise accuracy the interview never measured.
const REVIEW_INTERVAL_DAYS = [1, 3, 7, 14, 30];
const DAY_MS = 24 * 60 * 60 * 1000;

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

const sharedFocusKey = "codetrial.sharedPracticeFocus";

/// The focus a candidate chose to share, handed from the lobby to the interview
/// in this tab's session storage rather than the address bar. A URL carrying
/// it is one anybody can craft, putting their text into the interviewer's
/// instructions with no consent given, and it lands in history and access
/// logs. Read once: a reload does not share it again unasked.
export function storeSharedFocus(storage, focus) {
  try {
    if (focus) storage.setItem(sharedFocusKey, focus);
    else storage.removeItem(sharedFocusKey);
  } catch { /* an unshared focus is the safe outcome */ }
}

export function consumeSharedFocus(storage) {
  try {
    const focus = storage.getItem(sharedFocusKey);
    storage.removeItem(sharedFocusKey);
    return typeof focus === "string" ? focus : "";
  } catch {
    return "";
  }
}

/// Carry one concrete action from the reports into the lobby. A focus repeated
/// across reports wins; a tie goes to the newest report and its already-ranked
/// first item. This does not alter the next problem or become new assessment
/// evidence: it is the candidate's own reminder until they explicitly share it
/// with the interviewer.
export function practiceFocus(reports) {
  const text = (value) => typeof value === "string" && value.trim() !== "";
  // Insertion order is newest report first and plan order within it, and the
  // sort below is stable, so ties keep exactly that order without a key.
  const focuses = new Map();
  for (const { report } of reports) {
    if (
      report?.incomplete
      || (report?.decision !== "HIRE" && report?.decision !== "NO_HIRE")
      || !Array.isArray(report?.improvementPlan)
    ) continue;
    const reported = new Set();
    for (const item of report.improvementPlan) {
      if (!text(item?.weakness) || !text(item?.drill) || !text(item?.successCriterion)) continue;
      const weakness = item.weakness.trim();
      if (reported.has(weakness)) continue;
      reported.add(weakness);
      const existing = focuses.get(weakness);
      if (existing) existing.occurrences += 1;
      else focuses.set(weakness, { weakness, drill: item.drill.trim(), successCriterion: item.successCriterion.trim(), occurrences: 1 });
    }
  }
  return [...focuses.values()].sort((left, right) => right.occurrences - left.occurrences)[0] ?? null;
}

/// Clamped at both ends: passing two Hard problems leaves the candidate on
/// Hard, which is still the right answer, and the lobby still says why.
function step(level, by) {
  const index = LEVELS.indexOf(level) + by;
  return LEVELS[Math.min(Math.max(index, 0), LEVELS.length - 1)];
}

/// Pick what to interview on next. A completed problem returns when its review
/// is due; otherwise choose an unseen problem before repeating one early.
/// `reports` is the `pickerEntry` shape from `progress.js`: the entry as saved,
/// keyed by page name, with a normalized `at`.
///
/// The optional clock keeps the scheduling rule deterministic in its tests.
export function pickProblem(problems, difficulties, reports, random = Math.random, now = Date.now()) {
  const eligible = problems.filter((problem) => difficulties.has(problem.difficulty));
  const reportList = Array.isArray(reports) ? reports : [];
  const reviews = reviewStatus(reportList, now);
  const passed = new Set(
    reportList.filter((entry) => entry?.report?.decision === "HIRE").map((entry) => entry.problemId),
  );
  const due = problems.filter((problem) => reviews.get(problem.id)?.due);
  const fresh = eligible.filter((problem) => !passed.has(problem.id));
  const choices = due.length ? due : fresh.length ? fresh : eligible;
  const picked = choices[Math.floor(random() * choices.length)];
  if (!picked) return null;
  const review = reviews.get(picked.id);
  return { picked, repeat: !fresh.length, review: due.length ? review : null };
}

function reviewStatus(reports, now) {
    const outcomes = new Map();
    for (const entry of reports) {
        const decision = entry?.report?.decision;
        if ((decision !== "HIRE" && decision !== "NO_HIRE") || !Number.isFinite(entry.at)) continue;
        const entries = outcomes.get(entry.problemId) ?? [];
        entries.push({ at: entry.at, decision });
        outcomes.set(entry.problemId, entries);
    }
    return new Map([...outcomes].map(([problemId, entries]) => {
        // History is persisted newest first, while a failed result resets only
        // the streak that follows it. Ordering explicitly keeps an older miss
        // from erasing newer successful reviews and leaving no interval.
        const ordered = [...entries].sort((left, right) => left.at - right.at);
        let successes = 0;
        for (const entry of ordered) successes = entry.decision === "HIRE" ? successes + 1 : 0;
        const latest = ordered.at(-1);
        const intervalDays = latest.decision === "NO_HIRE"
            ? REVIEW_INTERVAL_DAYS[0]
            : REVIEW_INTERVAL_DAYS[Math.min(successes, REVIEW_INTERVAL_DAYS.length) - 1];
    return [problemId, { due: latest.at + intervalDays * DAY_MS <= now, intervalDays }];
  }));
}
