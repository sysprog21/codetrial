/// The replay feed: what the recording shows beside the video.
///
/// Split out of `interview.js` because it is one queue with one lifetime. Every
/// producer in the page goes through `recordReplay`, so there is one answer to
/// "is this server recording", one place the envelope is written, and one place
/// the feed stops when the server says it has heard enough. Scattered across
/// the caller that happened to need it, that invariant was one edit from being
/// three answers.
///
/// The page's own bindings arrive once through `initReplay` and keep their
/// names, so the queue reads the same state object the rest of the interview
/// does rather than a copy that drifts from it.

let state = null;
let nodes = null;
let problem = null;
let recordingEnabled = false;
let consentVersion = "";
let replayVersion = 1;

export function initReplay(deps) {
  ({ state, nodes, problem, recordingEnabled, consentVersion, replayVersion } = deps);
}

const REPLAY_FLUSH_MS = 1000;
const REPLAY_MAX_BATCH = 32;

/// How often the clock and the problem heading are restated.
///
/// Every second would be twenty-seven hundred events in a forty-five minute
/// interview, most of the per-interview budget spent on a number the viewer
/// can read off the video anyway. Fifteen seconds is a clock that is never
/// more than fifteen seconds stale in a replay nobody scrubs to the second.
const REPLAY_STAGE_MS = 15000;

let replayQueue = [];
let replayTimer = null;
let replayClosed = false;
let replayStageAt = 0;
let replayAvatarState = "";

/// One event onto the queue.
///
/// Every producer goes through here, so there is one answer to "is this server
/// recording", one place the envelope is written, and one place the replay
/// stops when the server says it has heard enough.
export function recordReplay(kind, payload) {
  if (!recordingEnabled || !state.interviewId || replayClosed) return;
  replayQueue.push({ v: replayVersion, kind, at: Date.now(), payload });
  if (replayQueue.length >= REPLAY_MAX_BATCH) {
    void flushReplay();
    return;
  }
  replayTimer ||= setTimeout(() => void flushReplay(), REPLAY_FLUSH_MS);
}

/// Stop producing, and forget what has not gone yet.
///
/// Called when the candidate withdraws consent and when the server says it has
/// heard enough. The queue is dropped rather than flushed: these are events
/// from before a decision that says they should not be stored.
export function closeReplay() {
  replayClosed = true;
  replayQueue = [];
  clearTimeout(replayTimer);
  replayTimer = null;
}

/// Send what is queued.
///
/// Dropped rather than retried on a failure. The events are a description of an
/// interview that is still happening, and a queue that grew through an outage
/// would deliver a burst of stale state after it, on top of the newer state
/// that had already arrived.
export async function flushReplay() {
  clearTimeout(replayTimer);
  replayTimer = null;
  if (!replayQueue.length || replayClosed) return;
  const batch = replayQueue.splice(0, REPLAY_MAX_BATCH);
  try {
    const response = await fetch(`/api/interviews/${encodeURIComponent(state.interviewId)}/events`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ events: batch }),
    });
    // Over quota, or an interview whose consent has been withdrawn. Both mean
    // the server will refuse everything after this, and a producer that kept
    // posting would spend the rest of the interview being told so.
    if (response.status === 413 || response.status === 404) closeReplay();
  } catch {
    // Offline. The interview is what matters and it is still running.
  }
  if (replayQueue.length) replayTimer ||= setTimeout(() => void flushReplay(), REPLAY_FLUSH_MS);
}

/// The problem heading, as the recording shows it.
export function recordStage() {
  recordReplay("stage", { title: problem.title, meta: nodes.meta.textContent });
}

/// Writes down what the candidate agreed to, and returns the interview id the
/// token request then has to carry.
///
/// `null` where the server records nothing: there is no consent to take, and
/// the token endpoint ignores the field.
///
/// A failure throws into `connect`'s catch, which is the right place: an
/// interview that cannot record consent must not start, and the candidate gets
/// the server's own sentence plus the offline practice editor rather than a
/// recording nobody agreed to.
export async function recordConsent() {
  if (!recordingEnabled) return null;
  const response = await fetch("/api/interviews", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ consentVersion }),
  });
  if (!response.ok) {
    throw new Error((await response.json())?.error || "Could not record your recording consent.");
  }
  return (await response.json()).interviewId;
}


/// The clock and the problem heading, restated on a throttle.
///
/// The guard lives with the queue rather than in `tickTimer`, so the one place
/// that decides how often the stage repeats is the one place that owns
/// `REPLAY_STAGE_MS`.
export function recordStageTick(remainingSeconds) {
  if (Date.now() - replayStageAt < REPLAY_STAGE_MS) return;
  replayStageAt = Date.now();
  recordReplay("stage", { remainingSeconds });
}

/// On the change, not on the tick. `updateAgentState` runs for every
/// participant event, and an interviewer who stays in one state for a minute is
/// one event, not sixty.
export function recordAvatarState(value) {
  if (value === replayAvatarState) return;
  replayAvatarState = value;
  recordReplay("avatar", { state: value });
}
