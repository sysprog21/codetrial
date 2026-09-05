// The page a candidate reads their own interviews back on.
//
// Everything here comes from `/api/recordings`, which is scoped to the signed-in
// account and returns no handle to media: no object path, no Drive file id, no
// permission id. So this page has no link to a video and is not going to grow
// one. What it says about the file is what the server knows, which is whether it
// exists and until when.
//
// A new page rather than `web/history.js`: that file is the localStorage plus
// `/api/reports` sync module the lobby uses, and repurposing it would break the
// section it already serves.

import { reportMarkup } from "/render.js";
import { modeLabel, replayRows, replayTimeline, responseWindowLabel, sanitizeReport } from "/lib.js";

const nodes = {
  list: document.querySelector("#replay-list"),
  more: document.querySelector("#replay-more"),
  empty: document.querySelector("#replay-empty"),
  title: document.querySelector("#replay-title"),
  status: document.querySelector("#replay-status"),
  media: document.querySelector("#replay-media"),
  transcript: document.querySelector("#replay-transcript"),
  timeline: document.querySelector("#replay-timeline"),
  windowNote: document.querySelector("#replay-window-note"),
  momentLabel: document.querySelector("#replay-moment-label"),
  code: document.querySelector("#replay-code"),
  tests: document.querySelector("#replay-tests"),
  report: document.querySelector("#replay-report"),
};

/// What the page says about the media, per state.
///
/// One sentence each, and none of them a link. `ready` does not say "watch it
/// here", because the file is in a Shared Drive folder shared with the address
/// GitHub verified, and this page has never been told where it is.
const MEDIA_WORDS = {
  ready: "The video was shared with your verified email address. It is deleted 24 hours after the interview.",
  recording: "This interview is still being recorded.",
  starting: "This interview is still being recorded.",
  finalizing: "The recording is being finished.",
  transferring: "The recording is being delivered to your email address.",
  failed: "There is no video for this interview.",
  cleanup_failed: "There is no video for this interview.",
  deleted: "The video has been deleted.",
};

let cursor = null;
let selected = null;
/// The last editor snapshot rendered, which is also what the report card shows
/// as the candidate's code. Taken from the replay rather than stored a second
/// time in the report: the replay already has every version of it, and a copy
/// in two places is a copy that can disagree.
let latest = { code: "", language: "" };

/// A recording whose media is gone, in the words the server used.
///
/// `410` is two different things and they are not the same to a person: one
/// waited too long, the other was taken away.
const GONE_WORDS = {
  replay_expired: "This recording is past its 24 hours and has been deleted.",
  recording_deleted: "This recording has been deleted.",
};

/// Every word a response window may put on the timeline.
///
/// One object, because `tests/browser/history.test.js` holds it to an allowlist,
/// alongside every other string this file can say. "No severity language
/// anywhere" is a negative no test can observe; "these strings and no others" is
/// one it can, and it is what tells the next person who reaches for a label that
/// the wording is a decision rather than a blank.
///
/// That allowlist is the static half. The other half is
/// `tests/browser/replay-render.test.js`, which drives this page through a stub
/// document and reads back what it rendered, because a table of allowed strings
/// says nothing about where they are put or what is put beside them.
///
/// The sentence explaining what the number is worth is not here. It is the
/// static `#replay-window-note` in `web/replay.html`, written once, because a
/// paragraph that needs no state does not need a renderer.
const WINDOW_WORDS = {
  label: "response window",
  seconds: " s",
  separator: " · ",
  /// Both reasons `responseWindows` returns a null duration: a window the
  /// recording ended inside, and a clock that ran backwards between the two
  /// rows. One phrase for both, because neither is a measurement and the note
  /// above the list says what they are.
  unmeasured: "duration not recorded",
  /// A window a transcript row could have named and none did. The producer
  /// stamps each turn with the window its stream started in, so this is a fact
  /// about the interview rather than about the record: the candidate said
  /// nothing between the interviewer's two turns.
  noTurn: "no candidate transcript recorded",
  /// A window nothing could have named, which is a fact about the recording
  /// instead. Recordings made before the producer stamped its turns carry no
  /// index to match on, and past the first window there is no way to place a
  /// turn without guessing at timing.
  ///
  /// Its own sentence rather than `noTurn`, because that one is the only claim
  /// about the candidate this panel makes, and making it on a recording that
  /// cannot support it is the one thing here that would be false rather than
  /// merely withheld.
  unmatchedTurn: "transcript not matched to a window",
  /// The one cause of a long window the replay can name; why, in
  /// `responseWindows`.
  paused: "interview paused during this window",
};

export function momentTime(at) {
  // The producer's own clock, in milliseconds, which is what the replay is
  // played back against. Rendered as a wall clock rather than an offset,
  // because a candidate reading their own interview back knows what time they
  // sat down and does not know what `00:04:12` is relative to.
  const when = new Date(at);
  return Number.isNaN(when.getTime()) ? "" : when.toLocaleTimeString();
}

async function loadList() {
  const query = cursor ? `?before=${encodeURIComponent(cursor)}` : "";
  const response = await fetch(`/api/recordings${query}`);
  if (!response.ok) {
    nodes.empty.hidden = false;
    nodes.empty.textContent =
      response.status === 401
        ? "Sign in to see your recordings."
        : "Could not read your recordings.";
    return;
  }
  const body = await response.json();
  for (const recording of body.recordings || []) {
    const item = document.createElement("li");
    const button = document.createElement("button");
    button.type = "button";
    button.className = "replay-item";
    button.dataset.recording = recording.recordingId;
    button.textContent = `${new Date(recording.createdAt * 1000).toLocaleString()} · ${recording.state}`;
    button.addEventListener("click", () => void select(recording.recordingId));
    item.append(button);
    nodes.list.append(item);
  }
  cursor = body.nextCursor || null;
  nodes.more.hidden = !cursor;
  nodes.empty.hidden = nodes.list.childElementCount > 0;
}

/// One recording: what happened to it, and what it recorded.
async function select(recordingId) {
  selected = recordingId;
  nodes.title.textContent = "Recording";
  clearDetail();

  const response = await fetch(`/api/recordings/${encodeURIComponent(recordingId)}`);
  // A click while a fetch was in flight. Without this the older answer paints
  // over the newer one and the page shows one recording's state beside
  // another's transcript.
  if (selected !== recordingId) return;
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    // Again after the parse, not only before it. Reading a body is another
    // await, and a click during it is another chance for this answer to land on
    // somebody else's recording.
    if (selected !== recordingId) return;
    // A recording that is gone is not an error to apologise for. It is the
    // retention promise being kept, and the page says which of the two ways it
    // was kept.
    nodes.status.textContent = response.status === 410 ? "Deleted" : "Not available";
    nodes.media.textContent =
      GONE_WORDS[body.code] || "This recording is not available on this account.";
    return;
  }
  const recording = await response.json();
  if (selected !== recordingId) return;
  nodes.status.textContent = recording.recovery
    ? `${recording.state} · ${recording.error} · ${recording.recovery}`
    : recording.state;
  nodes.media.textContent =
    MEDIA_WORDS[recording.state] || "There is no video for this interview.";
  if (recording.quotaExceeded) {
    nodes.media.textContent += " The replay below stops before the end of the interview.";
  }
  // Events first, then the report, because the report card shows the code and
  // the code comes from the last editor snapshot the events carried.
  await loadEvents(recordingId);
  await loadReport(recording.interviewId, recordingId);
}

function clearDetail() {
  latest = { code: "", language: "" };
  // The paragraph explaining a response window is hidden until there is one to
  // explain. A replay with no `avatar` rows is a replay this feature has nothing
  // to say about, and a standing note about an empty list reads as a promise the
  // page did not keep.
  nodes.windowNote.hidden = true;
  nodes.momentLabel.textContent = "Code";
  nodes.status.textContent = "";
  nodes.media.textContent = "";
  nodes.transcript.replaceChildren();
  nodes.timeline.replaceChildren();
  nodes.code.textContent = "";
  nodes.tests.textContent = "";
  nodes.report.replaceChildren();
}

/// The events, with the `avatar` history rather than the newest `avatar` row.
///
/// A response window is computed from the `avatar` rows and marked from the
/// `lifecycle` ones, and the default read keeps one of each: both are snapshot
/// kinds on the server, so the newest row supersedes every earlier one. Three
/// ways to reach the history, and this is the one that was taken.
///
///  - `?after=0` alone, with no server change. The tail collapses nothing, so it
///    also returns every superseded `editor` snapshot, a whole code buffer per
///    debounce, bounded only by the per-interview replay ceiling, sent to a page
///    that wants one buffer and a list of states. And the tail floors its `after`
///    at 0 against a `seq > ?` bound, so `seq` 0 is unreachable through it.
///  - Dropping `Avatar` from `ReplayKind::is_snapshot`. The smallest diff and
///    the wrong one; `ReplayClass` in `src/recording/replay.rs` says why.
///  - A read that collapses only what restates a whole value, `editor` and
///    `stage`, so the `avatar` and `lifecycle` transitions survive. This one. It
///    costs a server change and a query parameter, and it is the only one of the
///    three where the page pays for what it reads.
///
/// `seq` 0 comes with it. This read binds `after` at `-1` as the snapshot does,
/// so the bound is `seq > -1` and the first event of the interview is in the
/// answer; nothing here has to argue that the first event does not matter,
/// because it is not missing.
async function loadEvents(recordingId) {
  const response = await fetch(
    `/api/recordings/${encodeURIComponent(recordingId)}/events?avatar=history`,
  );
  if (selected !== recordingId) return;
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    if (selected !== recordingId) return;
    if (response.status === 410) {
      nodes.media.textContent = GONE_WORDS[body.code] || nodes.media.textContent;
      return;
    }
    // Said rather than left blank. An empty transcript beside an interview that
    // certainly had one reads as "nothing was recorded", which is a different
    // and worse claim than "this did not load".
    nodes.tests.textContent = "Could not load the replay for this interview.";
    return;
  }
  const body = await response.json().catch(() => null);
  if (selected !== recordingId) return;
  if (!body) {
    nodes.tests.textContent = "Could not load the replay for this interview.";
    return;
  }
  render(body.events || []);
}

/// The replay, as a page rather than a playback.
///
/// The transcript accumulates and the snapshots supersede, which is the same
/// split the schema makes: a transcript line is a line and an editor snapshot
/// replaces the last one. Every snapshot keeps its time, so a moment can be
/// opened rather than scrubbed to.
export function render(events) {
  // Guarded here rather than only inside `replayTimeline`. The page reads
  // `body.events || []` off a parsed JSON body on the path where the response
  // was already `ok`, so a string or an object there is one bad deploy away, and
  // `findLast` on either throws before anything defensive downstream is reached.
  // A throw paints a blank panel where this page had a sentence ready.
  const rows = replayRows(events);
  const stage = rows.findLast((event) => event?.kind === "stage");
  // Only where the recording actually carried one. Reading it unconditionally
  // floored `undefined` to "Scored" and announced a distinction that no longer
  // exists on every replay made since the practice mode was removed.
  const mode = stage?.payload?.mode === undefined ? "" : modeLabel(stage.payload.mode);
  if (mode) {
    nodes.status.textContent = nodes.status.textContent
      ? `${nodes.status.textContent} · ${mode}`
      : mode;
  }
  for (const event of rows) {
    if (event?.kind !== "transcript") continue;
    const line = document.createElement("li");
    line.className = "replay-line";
    line.textContent = `${momentTime(event.at)} ${event.payload?.speaker || "candidate"}: ${event.payload?.text || ""}`;
    nodes.transcript.append(line);
  }

  // Two lists, not one: a window does not join `moments`, because `showMoment`
  // scans that list as editor and test snapshots. They are interleaved in one
  // timeline because that is where a reader looks for both.
  const { moments, windows, timeline } = replayTimeline(rows);

  nodes.windowNote.hidden = windows.length === 0;
  // Appended once. Interleaving the windows roughly quadruples this list, and a
  // node at a time is a layout pass at a time.
  nodes.timeline.append(
    ...timeline.map((entry) => {
      const item = document.createElement("li");
      item.append(
        entry.window === undefined
          ? momentButton(moments, entry.moment)
          : windowItem(windows[entry.window], entry.window),
      );
      return item;
    }),
  );
  if (moments.length) showMoment(moments, moments.length - 1);
}

function momentButton(moments, index) {
  const moment = moments[index];
  const button = document.createElement("button");
  button.type = "button";
  button.className = "replay-moment";
  button.dataset.moment = String(index);
  button.textContent = `${momentTime(moment.at)} · ${moment.kind}`;
  button.addEventListener("click", () => showMoment(moments, index));
  return button;
}

/// One response window, in the only words it is allowed.
///
/// Every string it can write comes from `WINDOW_WORDS`, and what is left here is
/// a time, a number and the separators between them. What holds that is
/// `tests/browser/replay-render.test.js`, which renders this row and reads
/// the result rather than reading this source: twelve ways to put a word on this
/// page were closed one at a time by reading source, and a thirteenth kept
/// arriving until the check started reading the output instead.
function windowItem(span, index) {
  const item = document.createElement("div");
  item.className = "replay-window";
  item.dataset.window = String(index);
  // The clock here and every word from `responseWindowLabel`, which is given
  // `WINDOW_WORDS` and can say nothing else. Empty parts dropped rather than
  // joined: `momentTime` answers a clock it cannot read with "", and joining
  // that left the label opening on a separator with nothing in front of it.
  item.textContent = [momentTime(span.at), ...responseWindowLabel(span, WINDOW_WORDS)]
    .filter(Boolean)
    .join(WINDOW_WORDS.separator);
  return item;
}

/// The editor or test moment currently shown in the panels.
function markCurrent(index) {
  for (const button of nodes.timeline.querySelectorAll("[data-moment]")) {
    button.classList.toggle("current", button.dataset.moment === String(index));
  }
}

/// The editor and the test results as they stood at one moment.
///
/// Walked from the start rather than read off the chosen event, because a
/// `tests` moment says nothing about the code and an `editor` moment says
/// nothing about the tests: the state at a moment is the last of each up to it.
function showMoment(moments, index) {
  let code = "";
  let language = "";
  let tests = null;
  for (const moment of moments.slice(0, index + 1)) {
    if (moment.kind === "editor") {
      code = moment.payload?.code || "";
      language = moment.payload?.language || "";
    }
    if (moment.kind === "tests") tests = moment.payload;
  }
  latest = { code, language };
  nodes.momentLabel.textContent = language ? `Code · ${language}` : "Code";
  // `textContent`, never `innerHTML`: this is the candidate's own code coming
  // back from a server that stored it verbatim.
  nodes.code.textContent = code;
  nodes.tests.textContent = tests
    ? `${tests.passed ?? 0}/${tests.total ?? 0} passing`
    : "No test run before this point.";
  markCurrent(index);
}

/// The report, which is `/api/reports` rather than anything recording owns.
async function loadReport(interviewId, recordingId) {
  const response = await fetch("/api/reports");
  if (selected !== recordingId) return;
  if (!response.ok) {
    // Distinct from "no report was saved", which is a fact about the interview
    // rather than about this page's luck with the network.
    nodes.report.textContent = "Could not load the report.";
    return;
  }
  const body = await response.json().catch(() => null);
  if (selected !== recordingId) return;
  if (!body) {
    nodes.report.textContent = "Could not load the report.";
    return;
  }
  // Matched on the interview id the report carries, which is why
  // `web/interview.js` puts it there. Reports are keyed by their own id and
  // recordings by theirs, and the clock is not a key.
  const saved = (body.reports || [])
    .map((row) => row.payload)
    .find((payload) => payload?.interviewId === interviewId);
  const report = saved && sanitizeReport(saved.report);
  if (!report) {
    nodes.report.textContent = "No report was saved for this interview.";
    return;
  }
  // `reportMarkup` escapes what it interpolates, which is what makes it safe to
  // hand a payload that came back from a server that stored it verbatim.
  nodes.report.innerHTML = reportMarkup({
    report,
    problemTitle: saved.problemTitle || "",
    language: latest.language,
    code: latest.code,
  });
}

nodes.more.addEventListener("click", () => void loadList());
void loadList();
