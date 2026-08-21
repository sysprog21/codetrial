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
import { sanitizeReport } from "/lib.js";

const nodes = {
  list: document.querySelector("#replay-list"),
  more: document.querySelector("#replay-more"),
  empty: document.querySelector("#replay-empty"),
  title: document.querySelector("#replay-title"),
  status: document.querySelector("#replay-status"),
  media: document.querySelector("#replay-media"),
  transcript: document.querySelector("#replay-transcript"),
  timeline: document.querySelector("#replay-timeline"),
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
  nodes.momentLabel.textContent = "Code";
  nodes.status.textContent = "";
  nodes.media.textContent = "";
  nodes.transcript.replaceChildren();
  nodes.timeline.replaceChildren();
  nodes.code.textContent = "";
  nodes.tests.textContent = "";
  nodes.report.replaceChildren();
}

async function loadEvents(recordingId) {
  const response = await fetch(`/api/recordings/${encodeURIComponent(recordingId)}/events`);
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
  const moments = [];
  for (const event of events) {
    if (event.kind === "transcript") {
      const line = document.createElement("li");
      line.className = "replay-line";
      line.textContent = `${momentTime(event.at)} ${event.payload?.speaker || "candidate"}: ${event.payload?.text || ""}`;
      nodes.transcript.append(line);
      continue;
    }
    if (event.kind === "editor" || event.kind === "tests") moments.push(event);
  }

  for (const [index, moment] of moments.entries()) {
    const item = document.createElement("li");
    const button = document.createElement("button");
    button.type = "button";
    button.className = "replay-moment";
    button.dataset.moment = String(index);
    button.textContent = `${momentTime(moment.at)} · ${moment.kind}`;
    button.addEventListener("click", () => showMoment(moments, index));
    item.append(button);
    nodes.timeline.append(item);
  }
  if (moments.length) showMoment(moments, moments.length - 1);
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
  for (const button of nodes.timeline.querySelectorAll("[data-moment]")) {
    button.classList.toggle("current", Number(button.dataset.moment) === index);
  }
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
