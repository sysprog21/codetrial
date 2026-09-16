import { frameworkPhases, sanitizeReport } from "./lib.js";
import { LEVELS } from "./problem-picker.js";

export const progressPhases = frameworkPhases;

const allowedDifficulties = new Set(LEVELS);
const allowedLanguages = new Set(["python", "javascript", "c", "cpp", "java"]);

/// `/api/reports` wraps each entry in `payload` (accounts.rs list_reports) while
/// history.js stores the same entry flat. One rule for which of the two an entry
/// is, so the lobby's picker and its progress panel cannot disagree about it:
/// the panel reads entries through `normalizeProgressEntry` and the picker
/// through `pickerEntry`, and both unwrap here.
function unwrapEntry(raw) {
  const wrapper = raw && typeof raw === "object" ? raw : {};
  const entry = wrapper.payload && typeof wrapper.payload === "object" ? wrapper.payload : wrapper;
  // The wrapper is the stored row and the payload is what the browser saved
  // into it. Only the row is guaranteed to carry the problem id, so it is the
  // fallback: without it the picker saw every account report as unattributed
  // and recommended problems the candidate had already passed.
  return entry.problemId === undefined ? { ...entry, problemId: wrapper.problemId } : entry;
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
  const dateValue = unwrapEntry(raw).date ?? (Number.isFinite(wrapper.createdAt) ? wrapper.createdAt * 1000 : null);
  const at = dateValue == null ? NaN : new Date(dateValue).getTime();
  return Number.isFinite(at) ? at : null;
}

export function normalizeProgressEntry(raw) {
  const wrapper = raw && typeof raw === "object" ? raw : {};
  const entry = unwrapEntry(raw);
  const report = sanitizeReport(entry.report);
  const durationMin = Number.isInteger(entry.durationMin) && entry.durationMin >= 10 && entry.durationMin <= 90
    ? entry.durationMin : null;
  return {
    id: String(entry.id ?? wrapper.id ?? ""),
    at: entryTime(raw),
    problemId: typeof entry.problemId === "string" ? entry.problemId : String(wrapper.problemId ?? ""),
    problemTitle: typeof entry.problemTitle === "string" ? entry.problemTitle : "Past interview",
    difficulty: allowedDifficulties.has(entry.difficulty) ? entry.difficulty : null,
    language: allowedLanguages.has(entry.language) ? entry.language : null,
    durationMin,
    report,
  };
}

export function buildProgressModel(rawEntries, filters = {}) {
  const normalized = (Array.isArray(rawEntries) ? rawEntries : [])
    .map(normalizeProgressEntry)
    .filter((entry) => entry.at !== null)
    .sort((left, right) => left.at - right.at);
  const matches = (entry, key) => filters[key] == null || filters[key] === "all"
    || String(entry[key]) === String(filters[key]);
  const attempts = normalized.filter((entry) =>
    matches(entry, "difficulty") && matches(entry, "language")
    && matches(entry, "durationMin"));
  const series = Object.fromEntries(progressPhases.map((phase) => [phase, []]));
  const weaknessCounts = new Map();
  let activeVersion = null;
  let activeSegments = null;
  for (const [attemptIndex, attempt] of attempts.entries()) {
    const assessment = attempt.report.incomplete ? null : attempt.report.frameworkAssessment;
    const version = assessment?.rubricVersion ?? null;
    if (version !== activeVersion) {
      activeVersion = version;
      activeSegments = version === null ? null : Object.fromEntries(progressPhases.map((phase) => {
        const segment = { rubricVersion: version, points: [] };
        series[phase].push(segment);
        return [phase, segment];
      }));
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
      for (const tag of row.weaknessTags) {
        weaknessCounts.set(tag, (weaknessCounts.get(tag) || 0) + 1);
      }
    }
  }
  for (const phase of progressPhases) {
    series[phase] = series[phase].filter((segment) => segment.points.length > 0);
  }
  const weaknesses = [...weaknessCounts]
    .map(([tag, count]) => ({ tag, count }))
    .sort((left, right) => right.count - left.count || left.tag.localeCompare(right.tag));
  const topics = new Map();
  for (const attempt of attempts) {
    for (const topic of new Set(attempt.report.topics || [])) {
      const row = topics.get(topic) || { topic, attempts: 0, passes: 0, lastAttempt: null };
      row.attempts += 1;
      row.passes += Number(!attempt.report.incomplete && attempt.report.decision === "HIRE");
      row.lastAttempt = attempt.at;
      topics.set(topic, row);
    }
  }
  const topicProgress = [...topics.values()]
    .sort((left, right) => left.topic.localeCompare(right.topic));
  return { total: normalized.length, attempts, series, weaknesses, topics: topicProgress, options: progressOptions(normalized) };
}

function progressOptions(entries) {
  const values = (key, compare = undefined) => [...new Set(entries.map((entry) => entry[key]).filter((value) => value != null))]
    .sort(compare);
  return {
    difficulty: values("difficulty"),
    language: values("language"),
    durationMin: values("durationMin", (left, right) => left - right),
  };
}
