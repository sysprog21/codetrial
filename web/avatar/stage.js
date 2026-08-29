/// Jim's face and the analyser that drives its mouth.
///
/// Presentation only: no frame this renders is published, recorded, or sent
/// anywhere, and the analyser reads Jim's own track, never the candidate's
/// microphone.
///
/// Split out of `interview.js` because none of it touches interview state. It
/// needs the page's element handles and nothing else, so it takes them once in
/// `startAvatar` and keeps the rest to itself: the renderer, the frame handle,
/// the analyser and its buffers are private to this file and reachable only
/// through the functions below.

import { isAgent } from "../lib.js";
import { peakLevel } from "../audio-check.js";
import {
  ANALYSER_FFT_SIZE,
  ANALYSER_WINDOW,
  MODEL_URL,
  createAvatar,
  mouthFromAmplitude,
} from "./avatar.js";

/// The page handles this stage draws into. Held rather than passed to every
/// function because the render loop reads them sixty times a second and
/// threading them through `requestAnimationFrame` would mean a closure per
/// frame.
let nodes = null;

export function initAvatarStage(deps) {
  ({ nodes } = deps);
}

let avatar = null;
let avatarFrame = null;
let jimAnalyser = null;
let jimAnalyserSource = null;
let jimAnalyserSamples = null;
let jimAnalyserTrack = null;
let jimAnalyserContext = null;
const jimAnalyserPeaks = [];

export function startAvatar() {
  if (avatar) return;
  // Below the breakpoint the stage is hidden and nothing is built, but the
  // candidate can cross it at any time by widening the window or undocking
  // devtools. Without a retry, an interview that started narrow never gets an
  // avatar however wide it gets. Registered once and only while there is
  // nothing to render, so a session that starts wide adds no listener and a
  // session that starts narrow drops it as soon as one is built.
  watchForStageWidth();
  // Below the stylesheet's breakpoint the avatar is `display: none`, and a
  // hidden element is not worth 11 MB of model, 730 KB of renderer, one of the
  // page's ~16 WebGL contexts and a 60 Hz loop drawing into a 1x1 canvas. The
  // computed style is asked rather than the breakpoint restated, so the CSS
  // stays the only place that decides where the avatar is shown.
  if (!nodes.jimAvatar || window.getComputedStyle(nodes.jimAvatar).display === "none") return;
  const reducedMotion = window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches === true;
  stopWatchingStageWidth();
  avatar = createAvatar({
    mount: nodes.jimAvatar,
    reducedMotion,
    // The panel says which of the two things happened, because "Jim is here by
    // voice" under a blank box is indistinguishable from a broken page.
    onState: (next) => {
      if (next !== "unavailable") return;
      nodes.jimAvatarNote.textContent = "Jim is here by voice; his avatar is unavailable in this browser.";
    },
    loadModel: async () => {
      // Ask whether the model is published before importing the renderer.
      // Without this every candidate downloads 730 KB of Three.js only to
      // discover a 404, which is exactly the state of the repo today.
      const published = await fetch(MODEL_URL, { method: "HEAD" });
      if (!published.ok) throw new Error(`${MODEL_URL} is not published`);
      const renderer = await import("./vrm.js");
      return renderer.loadVrm({ mount: nodes.jimAvatar });
    },
  });
  // Started only once something can actually render. Pumping regardless meant
  // every session without a model, which is all of them today, ran a 60 Hz
  // loop that read the analyser and threw the result away.
  void avatar.ready.then((rendered) => {
    if (!rendered) return;
    // Read by the avatar browser check, which has no other way to tell a
    // running render loop from a canvas that was created once and abandoned.
    window.__codetrialAvatarFrames = () => avatar?.frames() ?? 0;
    // Through `resumeAvatar`, not straight into the loop. A tab that became
    // visible while the model was still loading has already started one, and
    // two loops is two analyser reads and two poses a frame.
    resumeAvatar();
  });
}

export function pumpAvatar(at) {
  if (!avatar) return;
  // A hidden document stops asking for frames rather than asking and returning
  // early. Browsers already throttle `requestAnimationFrame` in a background
  // tab, so what this buys is small and it is not nothing: the analyser read
  // and the humanoid update stop too, and `resumeAvatar` below is what starts
  // them again. Written as "stop scheduling" rather than "skip a frame"
  // because a loop that keeps scheduling is a loop that is still running.
  if (document.hidden) {
    avatarFrame = null;
    return;
  }
  avatarFrame = requestAnimationFrame(pumpAvatar);
  // Hidden by the CSS breakpoint, which the candidate can cross at any time by
  // narrowing the window or docking devtools. startAvatar only samples this
  // once, so without the check the humanoid rig, expressions and constraints
  // kept running 60 times a second to produce no pixels.
  //
  // The preflight overlay is the same argument by a different route: it is
  // opaque and covers the stage, so a model that finishes loading while the
  // candidate is still granting a camera would otherwise render behind it,
  // taking main-thread frames away from the level meter and the face detector.
  // The width check cannot see this, because an element under an overlay still
  // has its width. Loading early is the point; drawing early is not.
  if (!nodes.jimAvatar.clientWidth || !nodes.audioCheck.hidden) return;
  avatar.setMouth(mouthFromAmplitude(jimAmplitude()));
  // The rAF timestamp is the frame's target time and is identical across every
  // callback in that frame; performance.now() drifts by however long the loop
  // took to reach us.
  avatar.frame(at ?? performance.now());
}

/// Starts the render loop again after the tab comes back.
///
/// One listener for the life of the page, added beside the loop rather than
/// inside it: a listener added per frame is sixty listeners a second.
export function resumeAvatar() {
  // `state()` and not just `avatar`: `createAvatar` returns before the model
  // has loaded, so a visibility change during the load would otherwise start a
  // loop that poses nothing sixty times a second.
  if (!avatar || avatar.state() !== "ready" || document.hidden || avatarFrame !== null) return;
  pumpAvatar();
}

document.addEventListener("visibilitychange", resumeAvatar);

/// Retries `startAvatar` when the stage becomes visible.
///
/// `resize` rather than a media query, because the stylesheet owns the
/// breakpoint and `startAvatar` already asks the computed style rather than
/// restating it. One listener at a time.
let stageWidthWatch = null;

function watchForStageWidth() {
  if (avatar || stageWidthWatch) return;
  stageWidthWatch = () => startAvatar();
  window.addEventListener("resize", stageWidthWatch);
}

/// Dropped as soon as there is something to render, not on the next resize
/// after that. A session that never resizes again would otherwise hold the
/// handler, and the scope it closes over, for the length of the interview.
function stopWatchingStageWidth() {
  if (!stageWidthWatch) return;
  window.removeEventListener("resize", stageWidthWatch);
  stageWidthWatch = null;
}

/// Jim's own track, never the candidate's. `createMediaElementSource` would be
/// the obvious call and is the wrong one: it returns silence for a
/// MediaStream-backed element, so the mouth would never open.
export function attachAvatarAnalyser(track, participant) {
  // Jim only. playRemoteAudio fires for every remote audio track, and this used
  // to take whichever arrived first: a second participant, or a stray hosted
  // agent of the kind scripts/browser-check.cjs already has to isolate, would
  // have driven the mouth. Never the candidate, who is never subscribed here.
  if (!isAgent(participant)) return;
  if (!track?.mediaStreamTrack || track === jimAnalyserTrack) return;
  // A new publication replaces the old analyser rather than being ignored.
  // Bailing out on `jimAnalyser` alone left the analyser bound to the track
  // that had just been replaced, and LiveKit does not promise the unsubscribe
  // for the old publication arrives before the subscribe for the new one: when
  // it arrived after, `dropRemoteAudio` tore down the only analyser there was
  // and nothing rebuilt it, so lip sync died for the rest of the session.
  if (jimAnalyser) releaseAvatarAnalyser();
  try {
    jimAnalyserContext ||= new (window.AudioContext || window.webkitAudioContext)();
    // TrackSubscribed is not a user gesture, so a context first built here can
    // arrive suspended and then read silence forever. The mouth would simply
    // never open, with nothing anywhere saying why.
    if (jimAnalyserContext.state === "suspended") void jimAnalyserContext.resume().catch(() => {});
    jimAnalyserSource = jimAnalyserContext.createMediaStreamSource(new MediaStream([track.mediaStreamTrack]));
    jimAnalyser = jimAnalyserContext.createAnalyser();
    jimAnalyser.fftSize = ANALYSER_FFT_SIZE;
    // One buffer for the session. Allocating it per frame produced 512 bytes of
    // garbage 60 times a second for the whole interview.
    jimAnalyserSamples = new Uint8Array(jimAnalyser.fftSize);
    jimAnalyserTrack = track;
    // Not connected to the destination: the audio element is already playing
    // this track, and a second path would play Jim twice.
    jimAnalyserSource.connect(jimAnalyser);
    resumeAnalyserOnGesture();
  } catch {
    // No analyser means no lip-sync. Everything else about the avatar, and all
    // of the audio, still works.
    releaseAvatarAnalyser();
  }
}

/// LiveKit re-subscribes Jim on every reconnect, so this runs more than once
/// per session. Without disconnecting the source node, each reconnect left a
/// live MediaStreamAudioSourceNode attached to the same context.
export function releaseAvatarAnalyser() {
  jimAnalyserSource?.disconnect();
  jimAnalyserSource = null;
  jimAnalyser = null;
  jimAnalyserSamples = null;
  jimAnalyserTrack = null;
  jimAnalyserPeaks.length = 0;
}

/// Safari refuses `resume()` unless the call is inside a user gesture, and
/// `TrackSubscribed` is not one, so a context first built there stays suspended
/// for the rest of the interview. The analyser then reads silence forever: the
/// avatar's mouth never opens, and the code above already predicted exactly
/// that without being able to do anything about it. Chrome resumes from
/// anywhere, which is why this survived.
///
/// The listeners are capturing and one-shot. A candidate who never touches the
/// page again keeps a suspended context, which is the state it was already in.
let gestureResumePending = false;

export function resumeAnalyserOnGesture() {
  if (!jimAnalyserContext || jimAnalyserContext.state !== "suspended") return;
  // LiveKit re-subscribes Jim on every reconnect, so this runs more than once.
  // Without the guard a context that stays suspended across two subscribes
  // collects two pairs of listeners; they do unregister themselves on the first
  // gesture, but registering them at all was pointless.
  if (gestureResumePending) return;
  gestureResumePending = true;
  function stop() {
    gestureResumePending = false;
    document.removeEventListener("pointerdown", resume, true);
    document.removeEventListener("keydown", resume, true);
  }
  function resume() {
    if (!jimAnalyserContext || jimAnalyserContext.state !== "suspended") {
      stop();
      return;
    }
    void jimAnalyserContext.resume().then(stop).catch(() => {});
  }
  document.addEventListener("pointerdown", resume, true);
  document.addEventListener("keydown", resume, true);
}

export function jimAmplitude() {
  if (!jimAnalyser || !jimAnalyserSamples) return 0;
  jimAnalyser.getByteTimeDomainData(jimAnalyserSamples);
  jimAnalyserPeaks.push(peakLevel(jimAnalyserSamples));
  if (jimAnalyserPeaks.length > ANALYSER_WINDOW) jimAnalyserPeaks.shift();
  return jimAnalyserPeaks.reduce((total, peak) => total + peak, 0) / jimAnalyserPeaks.length;
}

export function stopAvatar() {
  stopWatchingStageWidth();
  if (avatarFrame !== null) cancelAnimationFrame(avatarFrame);
  avatarFrame = null;
  avatar?.destroy();
  avatar = null;
  releaseAvatarAnalyser();
  void jimAnalyserContext?.close?.().catch(() => {});
  jimAnalyserContext = null;
}

/// The three questions `interview.js` still asks about the avatar, as functions
/// rather than as exported state. A `let avatar` reachable from another module
/// is a second owner of the renderer's lifetime, and the teardown order here is
/// the whole reason this file exists.
export function isAvatarAnalyserTrack(track) {
  return track === jimAnalyserTrack;
}

export function setAvatarSpeaking(speaking) {
  avatar?.setSpeaking(speaking);
}

export function setAvatarExpression(value) {
  avatar?.setExpression(value);
}
