// The page LiveKit Egress loads and records.
//
// It is not the candidate's page and it is not signed in. Egress opens
// `custom_base_url` with `url`, `token` and `layout` appended, so the room and
// the credential arrive in the query string and this file asserts nothing about
// their lifetime: the token is minted by Egress, not by CodeTrial.

import { isAgent } from "/lib.js";
import {
  ANALYSER_FFT_SIZE,
  ANALYSER_WINDOW,
  MODEL_URL,
  createAvatar,
  mouthFromAmplitude,
} from "/avatar/avatar.js";

// Every panel is written from the replay and from nothing else, so what the
// video shows and what the replay page shows cannot disagree.
const nodes = {
  stage: document.querySelector("#stage"),
  problemTitle: document.querySelector("#problem-title"),
  problemMeta: document.querySelector("#problem-meta"),
  code: document.querySelector("#code"),
  tests: document.querySelector("#tests"),
  timer: document.querySelector("#timer"),
  candidateVideo: document.querySelector("#candidate-video"),
  jimAvatar: document.querySelector("#jim-avatar"),
  jimState: document.querySelector("#jim-state"),
  ready: document.querySelector("#recording-ready"),
};

/// How often the replay is asked what happened next.
///
/// The replay is the only source these panels have: the recorder is not the
/// candidate's page and cannot read its editor. Half a second is under a
/// keystroke's worth of lag in a video, and two requests a second per recording
/// is nothing beside the encode.
const REPLAY_POLL_MS = 500;

/// How long one replay read may take before it is abandoned.
///
/// A request that hangs neither answers nor fails, and the first read is
/// awaited before the recording starts: without a bound, one stalled connection
/// holds the whole interview at an empty layout forever.
export const REPLAY_TIMEOUT_MS = 4000;

const params = new URLSearchParams(window.location.search);
const serverUrl = params.get("url");
const token = params.get("token");
// Carried through to the layout attribute rather than branched on. One layout
// exists today, and a page that silently rendered a different one than Egress
// asked for would be a difference nobody could see until the file was watched.
const layout = params.get("layout") || "default";

/// The recorder's own participant, and anything else hidden.
///
/// Room composite Egress joins hidden, so it is usually not in the participant
/// list at all. "Usually" is not a rule to build the candidate selector on: a
/// recorder that did show up would publish no camera, but it would be the first
/// participant with no camera yet, and the selector would have to be right for
/// the wrong reason.
export function isRecorder(participant) {
  return Boolean(
    participant?.permissions?.hidden ||
      participant?.identity?.startsWith("EG_") ||
      participant?.identity?.startsWith("egress-"),
  );
}

/// Whether this participant is publishing a live camera.
///
/// Source, not kind: a candidate presenting their screen publishes a second
/// video track, and a selector that took "any video" would put the screen share
/// where the face belongs.
export function publishesCamera(participant) {
  const publications = participant?.videoTrackPublications;
  if (!publications) return false;
  for (const publication of publications.values()) {
    if (publication.source === window.LivekitClient.Track.Source.Camera) return true;
  }
  return false;
}

/// The candidate, by elimination.
///
/// Whoever is neither the interviewer nor the recorder and publishes a camera.
/// Identity strings are not a rule here: the agent is recognised by the same
/// `isAgent` the interview page uses, and everyone else is told apart by what
/// they publish rather than by what they called themselves.
export function findCandidate(participants) {
  return participants.find(
    (participant) =>
      !isAgent(participant) && !isRecorder(participant) && publishesCamera(participant),
  );
}

let started = false;
let cameraAttached = false;
let bootstrapped = false;
let avatar = null;
let audioContext = null;
let analyser = null;
let analyserSource = null;
let samples = new Uint8Array(0);
const amplitudes = [];

/// Answers that will not change. Anything else is worth asking again: a 500 or
/// a dropped connection says the server had a bad moment, not that this
/// recording has no replay.
const RETRY_NEVER = [401, 403, 404, 410];

let replayClosed = false;
/// How long to keep trying for a replay before recording without one.
///
/// A transient failure must not let the recorder paint an empty layout, which
/// is why a retry does not count as bootstrapped. But a replay that never
/// answers must not hold the recording either. Measured on the clock rather
/// than counted in attempts: with a four second abort behind each one, twenty
/// attempts is a minute and a half of an interview nobody is recording.
export const REPLAY_BOOTSTRAP_MS = 10000;

const openedAt = Date.now();

/// The last event this page has applied. `-1` asks for the snapshot, which is
/// the replay with superseded frames dropped; anything else asks for what came
/// after it. One loop and one call, whichever it is.
let lastSeq = -1;

/// The readiness marker, and the only place it moves.
///
/// Flipped once the room is joined and the candidate's camera is on screen. A
/// frame recorded before that is a frame of an empty layout, and Egress has no
/// way to know it recorded one.
function markReady() {
  // Both, not either. A camera with no panels is a recording of a face beside
  // an empty editor; panels with no camera is a recording of an interview with
  // nobody in it. An empty snapshot is fine, and is what a recording that
  // started with the interview looks like; what matters is that the replay was
  // asked and answered.
  if (started || !cameraAttached || !bootstrapped) return;
  started = true;
  nodes.ready.dataset.ready = "true";
  nodes.ready.hidden = false;
  // The console, not a callback. Egress watches Chrome's console output for
  // these two exact strings; there is no injected function to call, and a page
  // that waited for one would never start the recording it was loaded for.
  console.log("START_RECORDING");
}

function attachCandidate(track) {
  track.attach(nodes.candidateVideo);
  cameraAttached = true;
  markReady();
}

/// One replay event onto the page.
///
/// An unknown kind is ignored rather than refused: a producer from a later
/// deploy is not a reason to stop rendering the interview.
export function applyReplayEvent(event) {
  const payload = event?.payload || {};
  switch (event?.kind) {
    case "stage":
      if (typeof payload.title === "string") nodes.problemTitle.textContent = payload.title;
      if (typeof payload.meta === "string") nodes.problemMeta.textContent = payload.meta;
      if (typeof payload.remainingSeconds === "number") {
        nodes.timer.textContent = clockText(payload.remainingSeconds);
      }
      break;
    case "editor":
      // `textContent`, never `innerHTML`. This is the candidate's own code
      // rendered into a page a recorder screenshots sixty times a second, and
      // markup in it would be markup in the recording.
      if (typeof payload.code === "string") nodes.code.textContent = payload.code;
      break;
    case "tests":
      nodes.tests.replaceChildren(testsLine(payload));
      break;
    case "avatar":
      if (typeof payload.state === "string") {
        avatar?.setExpression(payload.state);
        nodes.jimState.textContent = jimLabel(payload.state);
      }
      break;
    default:
      // `transcript` and `lifecycle` have no panel in this layout. They are in
      // the replay for the replay page to render, and dropping them here is a
      // decision rather than an omission.
      break;
  }
}

function clockText(remainingSeconds) {
  const whole = Math.max(0, Math.floor(remainingSeconds));
  const minutes = String(Math.floor(whole / 60)).padStart(2, "0");
  const seconds = String(whole % 60).padStart(2, "0");
  return `${minutes}:${seconds}`;
}

function jimLabel(state) {
  if (state === "speaking") return "Interviewer speaking";
  if (state === "thinking") return "Interviewer thinking";
  return "Interviewer listening";
}

function testsLine(payload) {
  const line = document.createElement("p");
  const passed = Number(payload.passed) || 0;
  const failed = Number(payload.failed) || 0;
  const total = Number(payload.total) || passed + failed;
  line.className = failed > 0 ? "fail" : "pass";
  line.textContent = `${passed}/${total} passing`;
  return line;
}

/// Ask the replay what happened, and apply it in the order it happened.
///
/// The server orders by sequence and this walks that order, so the snapshot is
/// complete before a single later event lands. A reader that applied a tail
/// before its snapshot would show an editor frame under a problem that had not
/// appeared yet.
export async function pumpReplay() {
  const query = lastSeq < 0 ? "" : `?after=${lastSeq}`;
  let response = null;
  let body = null;
  try {
    response = await fetch(`/api/recording/replay${query}`, {
      headers: { authorization: token || "" },
      signal: AbortSignal.timeout(REPLAY_TIMEOUT_MS),
    });
    // Read inside the same guard as the request. A truncated body rejects here,
    // and a rejection that escaped would take the whole poll with it: the first
    // read would land in `connect`'s catch and never schedule a second, and a
    // later one would break the chain that schedules the next.
    if (response.ok) body = await response.json();
  } catch {
    // A dropped request or a half-read answer is worth retrying; the room is
    // still up and so is the interview. Only an answer decides whether to stop
    // asking.
    giveUpEventually();
    return;
  }
  if (!response.ok) {
    // A replay that is refused or gone is not a reason to stop recording the
    // room. The camera and the interviewer's voice are still worth the file.
    //
    // Nor is it worth asking again: an expired replay does not un-expire and a
    // credential this route refused will be refused for the life of the job.
    // Polling it for the rest of the interview would be two requests a second
    // producing the same 410.
    if (!RETRY_NEVER.includes(response.status)) {
      giveUpEventually();
      return;
    }
    replayClosed = true;
    bootstrapped = true;
    markReady();
    return;
  }
  // Applying is inside a guard of its own. A `200` with a body this page did
  // not expect is still an answer, and an exception thrown while rendering it
  // would reject the first read: `connect` would fall into its own catch and
  // never schedule a second poll, so one malformed event would end the replay
  // for the rest of the interview.
  // A `200` from this route always carries both fields, so a body without them
  // came from something other than this route: an error page from a proxy, or
  // a truncation that still parsed. An empty replay is not that; it answers
  // with an empty list and a `seq` of -1, and is a perfectly good snapshot of
  // an interview that has not produced anything yet.
  if (!Array.isArray(body?.events) || typeof body?.seq !== "number") {
    giveUpEventually();
    return;
  }
  try {
    for (const event of body.events) applyReplayEvent(event);
    lastSeq = Math.max(lastSeq, body.seq);
  } catch {
    giveUpEventually();
    return;
  }
  bootstrapped = true;
  markReady();
}

function candidateTrack(participant) {
  const publications = participant?.videoTrackPublications;
  if (!publications) return null;
  for (const publication of publications.values()) {
    if (publication.source !== window.LivekitClient.Track.Source.Camera) continue;
    if (publication.track) return publication.track;
  }
  return null;
}

function refreshCandidate(room) {
  const candidate = findCandidate([...room.remoteParticipants.values()]);
  if (!candidate) return;
  const track = candidateTrack(candidate);
  if (track) attachCandidate(track);
}

async function connect() {
  if (!serverUrl || !token) {
    nodes.jimState.textContent = "This template was opened without a room.";
    return;
  }
  nodes.stage.dataset.layout = layout;

  // `adaptiveStream` off: it sizes a subscription to the element it is painted
  // into, which is the right answer for a person on a laptop and the wrong one
  // for a recorder, where the output resolution is fixed and a downgraded
  // stream is a permanently blurry file.
  const room = new window.LivekitClient.Room({ adaptiveStream: false, dynacast: false });
  const events = window.LivekitClient.RoomEvent;

  room.on(events.TrackSubscribed, (track, publication, participant) => {
    // Every voice in the room, not only the interviewer's. Egress records what
    // the page plays, so audio that is subscribed but never attached to an
    // element is audio the file does not have: an interview where the candidate
    // is seen answering questions nobody can hear them answer. The camera
    // element stays muted so their voice arrives once rather than twice.
    if (track.kind === "audio" && !isRecorder(participant)) {
      const audio = track.attach();
      audio.autoplay = true;
      document.body.append(audio);
      void audio.play().catch(() => {});
      if (isAgent(participant)) {
        nodes.jimState.textContent = "Interviewer connected";
        listenToAgent(track);
      }
      return;
    }
    if (isAgent(participant)) return;
    // Through the selector, never straight to the element. Attaching whatever
    // camera arrived would make "the candidate" mean "whoever published video
    // first", and an observer with a camera on would take the panel and the
    // readiness marker with it.
    refreshCandidate(room);
  });
  room.on(events.ParticipantConnected, () => refreshCandidate(room));
  room.on(events.TrackPublished, () => refreshCandidate(room));
  room.on(events.Disconnected, () => {
    // The recorder stops when the room does. Without this the job runs until
    // the provider's timeout and pays for a black tail.
    console.log("END_RECORDING");
  });
  room.on(events.TrackUnsubscribed, (track) => {
    // Detaching clears the element, which is what keeps a candidate who left
    // from being recorded as a frozen last frame for the rest of the
    // interview. The elements `attach()` minted are removed with it, because a
    // rejoin mints new ones and the old ones would sit in the page playing the
    // same voice twice. The camera element is the layout's, not this handler's:
    // removing it would leave a rejoining candidate with nothing to attach to.
    for (const element of track.detach()) {
      if (element === nodes.candidateVideo) {
        // The camera is gone, so the page is no longer worth recording on its
        // account. This matters before readiness: a camera that arrived and
        // left while the replay was still loading would otherwise start the
        // recording on an empty video panel.
        cameraAttached = false;
        // And then ask again, because the departing track may not be the one
        // that was on screen: a candidate who republished has already attached
        // a new camera to this element, and the old track's detach clears it.
        refreshCandidate(room);
        continue;
      }
      element.remove();
    }
  });

  await room.connect(serverUrl, token);
  refreshCandidate(room);
  startAvatar();

  // The snapshot first, and only then the loop. Awaiting it here is what makes
  // "one complete snapshot, then ordered events" true rather than hoped for: a
  // poll started alongside it could apply a tail while the snapshot was still
  // in flight.
  await pumpReplay();
  schedulePoll();
}

/// A read that failed in a way worth retrying.
///
/// Readiness is deliberately not granted here until the attempts run out: a
/// recording that started on the first dropped request would open with an empty
/// layout, which is the thing the snapshot exists to prevent.
function giveUpEventually() {
  if (Date.now() - openedAt < REPLAY_BOOTSTRAP_MS) return;
  bootstrapped = true;
  markReady();
}

/// The next read, scheduled only once the last one has landed.
///
/// A timer rather than an interval, because an interval with a busy flag is the
/// same thing with a way to get it wrong: a slow answer cannot stack up behind
/// a request that is still out, and a closed replay simply stops scheduling.
function schedulePoll() {
  if (replayClosed) return;
  setTimeout(() => void pumpReplay().then(schedulePoll), REPLAY_POLL_MS);
}

/// Jim, rendered here rather than subscribed.
///
/// The interviewer publishes audio and no video, so the face in the recording
/// is this page's own render driven by that audio. The mouth follows amplitude,
/// which is the one signal that cannot lie about whether he is speaking; the
/// expression follows the replay's avatar events.
function startAvatar() {
  avatar = createAvatar({
    mount: nodes.jimAvatar,
    loadModel: async () => {
      // The model is a 404 in this repo today, and asking first is what keeps
      // every recording from downloading a renderer to discover that.
      const published = await fetch(MODEL_URL, { method: "HEAD" });
      if (!published.ok) throw new Error(`${MODEL_URL} is not published`);
      const renderer = await import("/avatar/vrm.js");
      return renderer.loadVrm({ mount: nodes.jimAvatar });
    },
  });
  void avatar.ready.then((rendered) => {
    if (rendered) pumpAvatar();
  });
}

function pumpAvatar(at) {
  if (!avatar) return;
  requestAnimationFrame(pumpAvatar);
  if (analyser) {
    analyser.getByteTimeDomainData(samples);
    let peak = 0;
    for (const sample of samples) peak = Math.max(peak, Math.abs(sample - 128) / 128);
    amplitudes.push(peak);
    if (amplitudes.length > ANALYSER_WINDOW) amplitudes.shift();
    const average = amplitudes.reduce((total, each) => total + each, 0) / amplitudes.length;
    avatar.setMouth(mouthFromAmplitude(average));
  }
  // The rAF timestamp, not `performance.now()`: it is the frame's target time
  // and is identical across every callback in that frame, where `now()` drifts
  // by however long the loop took to get here.
  avatar.frame(at ?? performance.now());
}

/// The interviewer's voice, as numbers.
///
/// Read from the subscribed track rather than from the element Egress records,
/// because a graph node between the element and the speaker is a second place
/// for the volume to be wrong.
function listenToAgent(track) {
  if (!track?.mediaStreamTrack) return;
  audioContext ||= new (window.AudioContext || window.webkitAudioContext)();
  if (audioContext.state === "suspended") void audioContext.resume().catch(() => {});
  // The old graph first. LiveKit re-subscribes the interviewer on every
  // reconnect, and a source per subscription accumulates nodes that hold their
  // tracks and keep processing for the rest of the interview.
  analyserSource?.disconnect();
  analyser?.disconnect();
  const source = audioContext.createMediaStreamSource(new MediaStream([track.mediaStreamTrack]));
  analyserSource = source;
  analyser = audioContext.createAnalyser();
  analyser.fftSize = ANALYSER_FFT_SIZE;
  source.connect(analyser);
  samples = new Uint8Array(analyser.fftSize);
}

void connect().catch((error) => {
  nodes.jimState.textContent = error?.message || "Could not join the room.";
});
