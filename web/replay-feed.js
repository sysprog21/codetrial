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
let recordingEnabled = false;
let consentVersion = "";
let replayVersion = 1;

export function initReplay(deps) {
  ({ state, nodes, recordingEnabled, consentVersion, replayVersion } = deps);
}

const REPLAY_FLUSH_MS = 1000;
const REPLAY_MAX_BATCH = 32;
const REPLAY_RETRY_MS = 60_000;
const REPLAY_RETRY_MAX_MS = 120_000;
const REPLAY_KEEPALIVE_MAX_BYTES = 64 * 1024;

/// `ReplayClass::Restates` in `src/recording/replay.rs`, named again here
/// because the browser cannot ask the server how it classifies a kind.
const REPLAY_RESTATES_KINDS = new Set(["editor", "stage"]);

const encoder = new TextEncoder();

/// The body an event goes in, and the event alone. The separators are counted
/// where the batch is, because there is one fewer of them than there are
/// events and a batch of exactly the budget must not be split.
const REPLAY_ENVELOPE_BYTES = encoder.encode(
  JSON.stringify({ events: [] }),
).length;

function eventBytes(event) {
  return encoder.encode(JSON.stringify(event)).length;
}

/// How often the clock and the problem heading are restated.
///
/// Every second would be twenty-seven hundred events in a forty-five minute
/// interview, most of the per-interview budget spent on a number the viewer
/// can read off the video anyway. Fifteen seconds is a clock that is never
/// more than fifteen seconds stale in a replay nobody scrubs to the second.
const REPLAY_STAGE_MS = 15000;

let replayQueue = [];
/// What `replayQueue` would encode to, kept in step with every push, batch and
/// put-back so the size of the next post is known without encoding it.
let queuedBytes = 0;
/// When the server's rate limit stops applying, as a wall-clock instant.
let retryAfter = 0;
let replayTimer = null;
let replayClosed = false;
let replayStageAt = 0;
let replayAvatarState = "";
let replayWindow = -1;
let replayWindowOpen = false;

/// One event onto the queue.
///
/// Every producer goes through here, so there is one answer to "is this server
/// recording", one place the envelope is written, and one place the replay
/// stops when the server says it has heard enough.
export function recordReplay(kind, payload) {
  if (!recordingEnabled || !state.interviewId || replayClosed) return;
  const event = { v: replayVersion, kind, at: Date.now(), payload };
  replayQueue.push(event);
  queuedBytes += eventBytes(event);
  const full =
    replayQueue.length >= REPLAY_MAX_BATCH ||
    queuedBodyBytes() > REPLAY_KEEPALIVE_MAX_BYTES;
  if (full && Date.now() >= retryAfter) {
    void flushReplay();
    return;
  }
  scheduleFlush();
}

function queuedBodyBytes() {
  return (
    REPLAY_ENVELOPE_BYTES + queuedBytes + Math.max(0, replayQueue.length - 1)
  );
}

function scheduleFlush() {
  if (replayTimer) return;
  const delay = Math.max(REPLAY_FLUSH_MS, retryAfter - Date.now());
  replayTimer = setTimeout(() => void flushReplay(), delay);
}

/// How long to wait after the server says it has heard enough for now.
///
/// Its own `Retry-After` when it sends one, because the server knows the width
/// of its window and the browser is guessing. Bounded either way: a header
/// asking for an hour would hold the rest of the interview in memory.
function retryDelayMs(header) {
  const seconds = Number(header);
  const wanted =
    Number.isFinite(seconds) && seconds > 0 ? seconds * 1000 : REPLAY_RETRY_MS;
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
  queuedBytes = 0;
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
/// `recordReplay` starts one whenever the queue fills, and the interview's end
/// asks for one without waiting on it. Inside a Retry-After window a flush
/// resolves without posting and leaves the queue to the timer that window
/// armed. Left unserialized, a batch posted while another was still
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
  if (Date.now() < retryAfter) {
    scheduleFlush();
    return;
  }
  compactSupersededRestates();
  const batch = takeBatch();
  const body = JSON.stringify({ events: batch });
  try {
    // Kept alive so a batch already on its way survives the tab closing. The
    // interview's last events are flushed as the candidate reaches the report,
    // which is the moment a candidate is most likely to leave, and a plain
    // fetch is cancelled with the page. Every batch rather than only the last,
    // because the end may be the one inside a Retry-After window and so go
    // out on the timer rather than from the call that asked for it.
    //
    // The flag is still decided from the body rather than assumed: `takeBatch`
    // keeps a batch inside the budget, except for a single event that is over
    // it on its own, which the server still accepts and a keepalive fetch would
    // reject outright.
    const response = await fetch(
      `/api/interviews/${encodeURIComponent(state.interviewId)}/events`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body,
        keepalive:
          new TextEncoder().encode(body).length <= REPLAY_KEEPALIVE_MAX_BYTES,
      },
    );
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
      queuedBytes += batch.reduce(
        (total, event) => total + eventBytes(event),
        0,
      );
      retryAfter =
        Date.now() + retryDelayMs(response.headers.get("Retry-After"));
    }
  } catch {
    // Offline. The interview is what matters and it is still running.
  }
  if (replayQueue.length) scheduleFlush();
}

/// The longest run from the front of the queue that one post may carry.
///
/// Bounded by the server's count limit and by the keepalive budget, because a
/// batch that clears the first can fail the second: `REPLAY_MAX_BATCH` editor
/// snapshots are several hundred kilobytes, and a post that size is sent with
/// no flag and dies with the page.
///
/// The first event goes whatever its size. The server's own body limit is four
/// times this budget, so an oversized single event is still storable, while a
/// batch of none would hand the queue back to the timer that would take none
/// again.
function takeBatch() {
  let bytes = REPLAY_ENVELOPE_BYTES;
  let taken = 0;
  let count = 0;
  while (count < REPLAY_MAX_BATCH && count < replayQueue.length) {
    const next = eventBytes(replayQueue[count]);
    // The comma this event brings with it, which the first one does not.
    const cost = count ? next + 1 : next;
    if (count && bytes + cost > REPLAY_KEEPALIVE_MAX_BYTES) break;
    bytes += cost;
    taken += next;
    count += 1;
  }
  queuedBytes -= taken;
  return replayQueue.splice(0, count);
}

/// Drop the queued `editor` and `stage` frames that a later queued frame of the
/// same kind already restates, and nothing else.
///
/// Only when the lifecycle events are queued behind more than one post's worth
/// of replay. `ended` and `rounds_final` are flushed as the candidate reaches
/// the report, which is when the tab goes, so they have to travel in a post
/// small enough to be sent with `keepalive`; a second batch behind them waits
/// on the first batch's response and never leaves at all. Sending it without
/// that wait is not the fix: `seq` is allocated inside the insert and every
/// read orders by it, so a small post that wins the race would hide the events
/// of the larger one behind an `ended` row that `responseWindows` stops at.
///
/// Narrow on purpose, and not a way to make any queue fit. These two kinds
/// restate a whole value and a review read keeps only the newest of each, so
/// what goes here is what no reader of the finished replay would have been
/// shown; the one reader that loses anything is tailing the interview live as
/// it ends. Every transcript line, test run, avatar transition and lifecycle
/// row is kept, in order, and a queue that still does not fit is split into
/// posts rather than compacted any further.
function compactSupersededRestates() {
  if (queuedBodyBytes() <= REPLAY_KEEPALIVE_MAX_BYTES) return;
  if (!replayQueue.some((event) => event.kind === "lifecycle")) return;
  // Backwards, because the frame that supersedes is the later one: the first
  // of a kind met on the way back is the newest, and everything of that kind
  // before it has been restated by it.
  const newest = new Set();
  const kept = [];
  for (let index = replayQueue.length - 1; index >= 0; index -= 1) {
    const event = replayQueue[index];
    if (REPLAY_RESTATES_KINDS.has(event.kind)) {
      if (newest.has(event.kind)) {
        queuedBytes -= eventBytes(event);
        continue;
      }
      newest.add(event.kind);
    }
    kept.push(event);
  }
  kept.reverse();
  replayQueue = kept;
}

/// The problem heading, as the recording shows it.
///
/// Every stage event carries it, not just the first. `Stage` is a snapshot kind
/// on the server, so the newest one supersedes every earlier one when a replay
/// is assembled: a clock tick carrying only the seconds would replace the
/// heading with nothing, and a reader who joined late would find the problem
/// title blank.
function stagePayload(extra) {
  return {
    title: nodes.title.textContent,
    meta: nodes.meta.textContent,
    avatarId: state.interviewerAvatar,
    ...extra,
  };
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
    throw new Error(
      (await response.json())?.error ||
        "Could not record your recording consent.",
    );
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
  const opensWindow = value === "listening" && replayAvatarState === "speaking";
  if (opensWindow) replayWindow += 1;
  replayWindowOpen = opensWindow;
  replayAvatarState = value;
  recordReplay("avatar", {
    state: value,
    responseWindow: opensWindow ? replayWindow : null,
  });
}

export function responseWindowIndex() {
  return replayWindowOpen ? replayWindow : null;
}
