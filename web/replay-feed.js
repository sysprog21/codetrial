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
const REPLAY_RETRY_MS = 60_000;
const REPLAY_RETRY_MAX_MS = 120_000;

/// How often the clock and the problem heading are restated.
///
/// Every second would be twenty-seven hundred events in a forty-five minute
/// interview, most of the per-interview budget spent on a number the viewer
/// can read off the video anyway. Fifteen seconds is a clock that is never
/// more than fifteen seconds stale in a replay nobody scrubs to the second.
const REPLAY_STAGE_MS = 15000;

let replayQueue = [];
/// When the server's rate limit stops applying, as a wall-clock instant.
let retryAfter = 0;
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
  if (replayQueue.length >= REPLAY_MAX_BATCH && Date.now() >= retryAfter) {
    void flushReplay();
    return;
  }
  replayTimer ||= setTimeout(() => void flushReplay(), REPLAY_FLUSH_MS);
}

/// How long to wait after the server says it has heard enough for now.
///
/// Its own `Retry-After` when it sends one, because the server knows the width
/// of its window and the browser is guessing. Bounded either way: a header
/// asking for an hour would hold the rest of the interview in memory.
function retryDelayMs(header) {
  const seconds = Number(header);
  const wanted = Number.isFinite(seconds) && seconds > 0 ? seconds * 1000 : REPLAY_RETRY_MS;
  return Math.min(wanted, REPLAY_RETRY_MAX_MS);
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
/// Dropped rather than retried when the network fails. The events describe an
/// interview that is still happening, and a queue that grew through an outage
/// would deliver a burst of stale state after it, on top of the newer state
/// that had already arrived. A refusal to serve for a minute is not an outage:
/// that batch is kept and sent when the minute is up.
/// One flush at a time, in the order they were asked for.
///
/// `recordReplay` starts one whenever the queue fills, and the interview's last
/// act awaits one. Left unserialized, a batch posted while another was still
/// awaiting `fetch` could commit first, and the replay would be ordered by
/// whichever request the server happened to finish rather than by what the
/// candidate did. Chained rather than skipped, because a caller that is told
/// "already flushing" and returns has silently dropped its own batch.
let flushChain = Promise.resolve();

export function flushReplay() {
  clearTimeout(replayTimer);
  replayTimer = null;
  flushChain = flushChain.then(sendQueuedBatch);
  return flushChain;
}

async function sendQueuedBatch() {
  if (!replayQueue.length || replayClosed) return;
  const batch = replayQueue.splice(0, REPLAY_MAX_BATCH);
  try {
    const response = await fetch(`/api/interviews/${encodeURIComponent(state.interviewId)}/events`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ events: batch }),
    });
    // 404 is an interview whose consent has been withdrawn, and quota is an
    // interview that has recorded all it may. Both mean the server will refuse
    // everything after this, and a producer that kept posting would spend the
    // rest of the interview being told so.
    //
    // The other 413 is `replay_batch_too_large`, which is about this batch and
    // not about the interview: one oversized editor payload used to close the
    // feed for good, so a candidate who pasted a large file lost the rest of
    // their replay. That batch is already gone from the queue, and the smaller
    // events behind it are still worth sending.
    if (response.status === 404) {
      closeReplay();
      return;
    }
    if (response.status === 413) {
      const code = await response
        .json()
        .then((body) => body?.code)
        .catch(() => null);
      if (code === "replay_quota_exceeded") closeReplay();
    }
    // Rate limiting says "not now", not "never", and the batch has already
    // been taken off the queue. Dropping it lost that stretch of the interview
    // for good, which is the one outcome the limit is not meant to have: it
    // bounds how often a browser may ask, it does not decide which evidence
    // survives. Put back at the front, because the feed is ordered by what the
    // candidate did.
    //
    // And held until the window the server named has passed. Putting the batch
    // back leaves the queue at the size that makes `recordReplay` flush on
    // sight, so without this the next event asks again, and so does every
    // event after it: a limit answered by asking harder.
    if (response.status === 429) {
      replayQueue.unshift(...batch);
      retryAfter = Date.now() + retryDelayMs(response.headers.get("Retry-After"));
    }
  } catch {
    // Offline. The interview is what matters and it is still running.
  }
  if (replayQueue.length) replayTimer ||= setTimeout(() => void flushReplay(), REPLAY_FLUSH_MS);
}

/// The problem heading, as the recording shows it.
///
/// Every stage event carries it, not just the first. `Stage` is a snapshot kind
/// on the server, so the newest one supersedes every earlier one when a replay
/// is assembled: a clock tick carrying only the seconds would replace the
/// heading with nothing, and a reader who joined late would find the problem
/// title blank.
function stagePayload(extra) {
  return { title: problem.title, meta: nodes.meta.textContent, ...extra };
}

export function recordStage() {
  recordReplay("stage", stagePayload());
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
  recordReplay("stage", stagePayload({ remainingSeconds }));
}

/// On the change, not on the tick. `updateAgentState` runs for every
/// participant event, and an interviewer who stays in one state for a minute is
/// one event, not sixty.
export function recordAvatarState(value) {
  if (value === replayAvatarState) return;
  replayAvatarState = value;
  recordReplay("avatar", { state: value });
}
