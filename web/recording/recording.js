// The page LiveKit Egress loads and records.
//
// It is not the candidate's page and it is not signed in. Egress opens
// `custom_base_url` with `url`, `token` and `layout` appended, so the room and
// the credential arrive in the query string and this file asserts nothing about
// their lifetime: the token is minted by Egress, not by CodeTrial.

import { isAgent } from "/lib.js";

// Only what this file writes to. The problem, code, test and timer panels are
// in the page and empty until the replay bootstrap fills them; a lookup here
// for a node nothing writes would be scaffolding that reads as wiring.
const nodes = {
  stage: document.querySelector("#stage"),
  candidateVideo: document.querySelector("#candidate-video"),
  jimState: document.querySelector("#jim-state"),
  ready: document.querySelector("#recording-ready"),
};

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

/// The readiness marker, and the only place it moves.
///
/// Flipped once the room is joined and the candidate's camera is on screen. A
/// frame recorded before that is a frame of an empty layout, and Egress has no
/// way to know it recorded one.
function markReady() {
  if (started) return;
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
      if (isAgent(participant)) nodes.jimState.textContent = "Interviewer connected";
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
      if (element !== nodes.candidateVideo) element.remove();
    }
  });

  await room.connect(serverUrl, token);
  refreshCandidate(room);
}

void connect().catch((error) => {
  nodes.jimState.textContent = error?.message || "Could not join the room.";
});
