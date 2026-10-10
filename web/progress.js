import {
  FRAMEWORKS,
  codingLoop,
  interviewMode,
  frameworkPhases,
  sanitizeReport,
} from "./lib.js";
import { LEVELS } from "./problem-picker.js";

export const progressPhases = frameworkPhases;

const phaseFramework = new Map(
  Object.values(FRAMEWORKS).flatMap((framework) =>
    framework.steps.map((step) => [step.label, framework.name]),
  ),
);

/// Which weakness wordings the progress panel lists once: they may differ in
/// case, spacing and a closing full stop, and in nothing that could change
/// what the sentence says. A display key only; no report is accepted or
/// refused on it.
const weaknessKey = (tag) =>
  tag.replace(/\s+/g, " ").trim().replace(/\.+$/, "").toLowerCase();

const allowedDifficulties = new Set(LEVELS);
const allowedLanguages = new Set(["python", "javascript", "c", "cpp", "java"]);

/// `/api/reports` wraps each entry in `payload` (accounts.rs list_reports) while
/// history.js stores the same entry flat. One rule for which of the two an entry
/// is, so the lobby's picker and its progress panel cannot disagree about it:
/// the panel reads entries through `normalizeProgressEntry` and the picker
/// through `pickerEntry`, and both unwrap here.
function unwrapEntry(raw) {
  const wrapper = raw && typeof raw === "object" ? raw : {};
  const entry =
    wrapper.payload && typeof wrapper.payload === "object"
      ? wrapper.payload
      : wrapper;
  // The wrapper is the stored row and the payload is what the browser saved
  // into it. Only the row is guaranteed to carry the problem id, so it is the
  // fallback: without it the picker saw every account report as unattributed
  // and recommended problems the candidate had already passed.
  return entry.problemId === undefined
    ? { ...entry, problemId: wrapper.problemId }
    : entry;
}

/// What the lobby's picker reads: the entry as it was saved, with only the
/// timestamp normalized. Not the sanitized report the progress panel draws,
/// because sanitizing a report from an older contract bundle drops its verdict
/// and turns a missing one into NO_HIRE, which forgot passes an earlier build
/// recorded and marched a candidate down a level for sessions nobody graded.
export function pickerEntry(raw) {
  return { ...unwrapEntry(raw), at: entryTime(raw) };
}

/// When an entry was saved, in milliseconds, or null. The local entry carries
/// its date; an account row carries `createdAt` in seconds beside the payload.
function entryTime(raw) {
  const wrapper = raw && typeof raw === "object" ? raw : {};
  const dateValue =
    unwrapEntry(raw).date ??
    (Number.isFinite(wrapper.createdAt) ? wrapper.createdAt * 1000 : null);
  const at = dateValue == null ? NaN : new Date(dateValue).getTime();
  return Number.isFinite(at) ? at : null;
}

export function normalizeProgressEntry(raw) {
  const wrapper = raw && typeof raw === "object" ? raw : {};
  const entry = unwrapEntry(raw);
  const report = sanitizeReport(entry.report);
  const durationMin =
    Number.isInteger(entry.durationMin) &&
    entry.durationMin >= 10 &&
    entry.durationMin <= 90
      ? entry.durationMin
      : null;
  return {
    id: String(entry.id ?? wrapper.id ?? ""),
    at: entryTime(raw),
    problemId:
      typeof entry.problemId === "string"
        ? entry.problemId
        : String(wrapper.problemId ?? ""),
    problemTitle:
      typeof entry.problemTitle === "string"
        ? entry.problemTitle
        : "Past interview",
    difficulty: allowedDifficulties.has(entry.difficulty)
      ? entry.difficulty
      : null,
    language: allowedLanguages.has(entry.language) ? entry.language : null,
    durationMin,
    interviewLoop: codingLoop(
      entry.interviewLoop ?? entry.report?.interviewLoop,
    ),
    interviewMode: interviewMode(
      entry.interviewMode ?? entry.report?.interviewMode,
    ),
    recordedDecision: ["HIRE", "NO_HIRE"].includes(entry.report?.decision)
      ? entry.report.decision
      : null,
    report,
  };
}

/// Every stored row read as an attempt, oldest first.
///
/// Separate from the model because a filter change rebuilds the model and does
/// not change this: `normalizeProgressEntry` sanitizes a whole report each
/// time, and the device history is up to twenty short rows plus five hundred
/// full ones, so re-running it on every dropdown change was five hundred
/// report sanitizations to re-sort rows that had not moved.
export function normalizeProgressEntries(rawEntries) {
  return (Array.isArray(rawEntries) ? rawEntries : [])
    .map(normalizeProgressEntry)
    .filter((entry) => entry.at !== null)
    .sort((left, right) => left.at - right.at);
}

export function buildProgressModel(rawEntries, filters = {}) {
  return progressModelFrom(normalizeProgressEntries(rawEntries), filters);
}

export function progressModelFrom(normalized, filters = {}) {
  const matches = (entry, key) =>
    filters[key] == null ||
    filters[key] === "all" ||
    String(entry[key]) === String(filters[key]);
  const attempts = normalized.filter(
    (entry) =>
      matches(entry, "difficulty") &&
      matches(entry, "language") &&
      matches(entry, "durationMin"),
  );
  const series = Object.fromEntries(progressPhases.map((phase) => [phase, []]));
  const weaknessByPhase = new Map();
  let activeVersion = null;
  let activeSegments = null;
  for (const [attemptIndex, attempt] of attempts.entries()) {
    const assessment = attempt.report.incomplete
      ? null
      : attempt.report.frameworkAssessment;
    const version = assessment?.rubricVersion ?? null;
    if (version !== activeVersion) {
      activeVersion = version;
      activeSegments =
        version === null
          ? null
          : Object.fromEntries(
              progressPhases.map((phase) => {
                const segment = { rubricVersion: version, points: [] };
                series[phase].push(segment);
                return [phase, segment];
              }),
            );
    }
    if (!assessment || !activeSegments) continue;
    for (const row of assessment.phases) {
      if (row.score !== null) {
        activeSegments[row.phase].points.push({
          attemptIndex,
          at: attempt.at,
          score: row.score,
          problemTitle: attempt.problemTitle,
        });
      }
      const flagged = row.weaknessTags.length > 0;
      if (row.score === null && !flagged) continue;
      const group = weaknessByPhase.get(row.phase) || {
        phase: row.phase,
        assessed: 0,
        reports: [],
      };
      group.assessed += 1;
      if (flagged) group.reports.push(row.weaknessTags);
      weaknessByPhase.set(row.phase, group);
    }
  }
  for (const phase of progressPhases) {
    series[phase] = series[phase].filter(
      (segment) => segment.points.length > 0,
    );
  }
  // Grouped by phase, not by wording: the model words every report afresh, so
  // the same gap never matched itself across attempts. The phase is a closed
  // set every stored report carries, old ones included. It is coarse, which is
  // why a row says the phase was flagged rather than that a weakness recurred.
  // `assessed` is the denominator because a coding round never scores a STAR
  // step and a behavioral round never scores a REACTO one.
  const weaknesses = [...weaknessByPhase.values()]
    .filter((group) => group.reports.length > 0)
    .map(({ phase, assessed, reports }) => ({
      phase,
      framework: phaseFramework.get(phase),
      count: reports.length,
      assessed,
      tags: newestWordings(reports),
    }))
    .sort(
      (left, right) =>
        right.count - left.count ||
        progressPhases.indexOf(left.phase) -
          progressPhases.indexOf(right.phase),
    );
  const topics = new Map();
  for (const attempt of attempts) {
    for (const topic of new Set(attempt.report.topics || [])) {
      const row = topics.get(topic) || { topic, attempts: 0, passes: 0 };
      row.attempts += 1;
      row.passes += Number(
        !attempt.report.incomplete && attempt.report.decision === "HIRE",
      );
      row.lastAttempt = attempt.at;
      topics.set(topic, row);
    }
  }
  const topicProgress = [...topics.values()].sort((left, right) =>
    left.topic.localeCompare(right.topic),
  );
  return {
    total: normalized.length,
    attempts,
    series,
    weaknesses,
    topics: topicProgress,
    options: progressOptions(normalized),
  };
}

/// The distinct wordings of a phase's weaknesses, newest report first, each
/// report's tags kept in the order the server wrote them, most important
/// first, so the display cap keeps a report's top weakness. One pass from the
/// newest report back, because this runs on every filter change.
function newestWordings(reports) {
  const seen = new Map();
  for (const tags of [...reports].reverse()) {
    for (const tag of tags) {
      const key = weaknessKey(tag);
      if (!seen.has(key)) seen.set(key, tag);
    }
  }
  return [...seen.values()];
}

function progressOptions(entries) {
  const values = (key, compare = undefined) =>
    [
      ...new Set(
        entries.map((entry) => entry[key]).filter((value) => value != null),
      ),
    ].sort(compare);
  return {
    difficulty: values("difficulty"),
    language: values("language"),
    durationMin: values("durationMin", (left, right) => left - right),
  };
}
