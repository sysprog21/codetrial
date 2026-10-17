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
  AVATAR_FRAME_MS,
  createAvatar,
  createPlayoutWatch,
  createRenderBudget,
  mouthFromAmplitude,
} from "./avatar.js";

/// The page handles this stage draws into. Held rather than passed to every
/// function because the render loop reads them sixty times a second and
/// threading them through `requestAnimationFrame` would mean a closure per
/// frame.
let nodes = null;

/// The model loader. `deps.loadModel` replaces it for the stage test only,
/// which has no WebGL; the page passes nothing.
let loadModel = async (mount, signal) => {
  const { loadAvatarModel } = await import("./model.js");
  return loadAvatarModel(mount, signal);
};

let onShowAvatar = null;

/// Once per page. A second call would add a second click listener and could
/// swap the loader under a model already in flight.
export function initAvatarStage(deps) {
  if (nodes) throw new Error("the avatar stage is initialized once");
  ({ nodes } = deps);
  onShowAvatar = deps.onShowAvatar ?? null;
  if (deps.loadModel) loadModel = deps.loadModel;
  nodes.hideAvatar.addEventListener("click", () =>
    avatarHidden ? showAvatar() : disableAvatar(null),
  );
  // Read once here and observed after, never per frame: reading clientWidth
  // flushes pending layout, which vrm.js already had to take out of the typing
  // path. The observer is also how a stage that was narrow comes back, so it
  // replaces a window resize listener. One observer for the page; the fallback
  // is the listener it replaces, for an engine without ResizeObserver.
  stageWidth = nodes.jimAvatar.clientWidth;
  if (window.ResizeObserver)
    new window.ResizeObserver((entries) =>
      onStageWidth(entries[entries.length - 1].contentRect.width),
    ).observe(nodes.jimAvatar);
  else
    window.addEventListener("resize", () =>
      onStageWidth(nodes.jimAvatar.clientWidth),
    );
}

/// Whether a model is wanted: between the page's `startAvatar` and a stop. A
/// stage that widens outside that window builds nothing.
let avatarWanted = false;
/// The mount's width as last observed. Zero below the CSS breakpoint.
let stageWidth = 0;

function onStageWidth(width) {
  // The observer also reports height-only changes, every frame of a drag.
  if (width === stageWidth) return;
  stageWidth = width;
  if (width === 0 || !avatarWanted) return;
  // Before a model exists this builds one; after, it restarts a loop that
  // paused because the stage went narrow.
  if (avatar) resumeAvatar();
  else startAvatar();
}

/// The page covers the stage with its preflight and uncovers it after. Told
/// rather than read, so the stage does not depend on another component's DOM.
/// Covered, the stage is inert, so nothing on it can be reached by keyboard
/// unseen, and nothing is drawn behind the overlay: its level meter and face
/// detector need the main thread more than a face nobody can see does.
export function setStageCovered(covered) {
  nodes.jimStage.inert = covered;
  if (!covered) resumeAvatar();
}

/// Automatic retirement lasts for the rest of the page. A candidate's Hide is
/// different: it releases the renderer, but the same button can start it again.
let avatarRetired = false;
let avatarHidden = false;
const renderBudget = createRenderBudget();
const playout = createPlayoutWatch({
  currentTrack: () => jimTrack,
  onLate: (ms) => disableAvatar("playout_delayed", `${ms}ms`),
});
let avatar = null;
let avatarFrame = null;
/// When the next frame is due, for the 30 fps cap. Null while paused, so the
/// first frame after a pause draws at once.
let nextDraw = null;
let jimAnalyser = null;
let jimAnalyserSource = null;
let jimAnalyserSamples = null;
/// Jim's current audio track, kept whether or not an analyser could be built
/// on it. The playout watch follows this and not the analyser: a browser that
/// throws on `AudioContext` loses lip sync, and must not also lose the one
/// signal that sees his voice arriving late.
let jimTrack = null;
let jimParticipant = null;
let jimAnalyserContext = null;
const jimAnalyserPeaks = [];

export function startAvatar() {
  if (avatar || avatarRetired || avatarHidden) return;
  // Below the breakpoint the stage is hidden and nothing is built, but the
  // candidate can cross it at any time by widening the window or undocking
  // devtools. The width observer retries while this is wanted, so an interview
  // that started narrow still gets an avatar once it is wide.
  avatarWanted = true;
  // Below the stylesheet's breakpoint the avatar is `display: none`, and a
  // hidden element is not worth 11 MB of model, 730 KB of renderer, one of the
  // page's ~16 WebGL contexts and a 60 Hz loop drawing into a 1x1 canvas. The
  // computed style is asked rather than the breakpoint restated, so the CSS
  // stays the only place that decides where the avatar is shown.
  if (
    !nodes.jimAvatar ||
    window.getComputedStyle(nodes.jimAvatar).display === "none"
  )
    return;
  const reducedMotion =
    window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches === true;
  avatar = createAvatar({
    mount: nodes.jimAvatar,
    reducedMotion,
    // Hide is offered while there is something to hide, and Show after the
    // candidate's own Hide. This decides the button on every change of state;
    // the Hide itself and a final `stopAvatar` write it too, because after
    // either one there is no model left to report a state. The panel says
    // which of the two things happened, because "Jim is here by voice" under a
    // blank box is indistinguishable from a broken page.
    onState: (next) => {
      const offered =
        next === "loading" ||
        next === "ready" ||
        (next === "stopped" && avatarHidden);
      nodes.hideAvatar.hidden = !offered;
      if (next === "loading" || next === "ready")
        nodes.hideAvatar.textContent = "Hide avatar";
      else if (avatarHidden) nodes.hideAvatar.textContent = "Show avatar";
      if (next !== "unavailable") return;
      nodes.jimAvatarNote.textContent =
        "Jim is here by voice; his avatar is unavailable in this browser.";
    },
    loadModel: (signal) => loadModel(nodes.jimAvatar, signal),
  });
  if (avatarExpression !== null) avatar.setExpression(avatarExpression);
  if (avatarSpeaking !== null) avatar.setSpeaking(avatarSpeaking);
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

/// Whether a drawn frame would be seen. Below the CSS breakpoint the mount has
/// no width, and the preflight overlay is opaque over the stage while it is up.
function stageVisible() {
  return stageWidth > 0 && !nodes.jimStage.inert;
}

export function pumpAvatar(at) {
  if (!avatar) return;
  // A hidden document stops asking for frames rather than asking and returning
  // early. Browsers already throttle `requestAnimationFrame` in a background
  // tab, so what this buys is small and it is not nothing: the analyser read
  // and the humanoid update stop too, and `resumeAvatar` below is what starts
  // them again. Written as "stop scheduling" rather than "skip a frame"
  // because a loop that keeps scheduling is a loop that is still running.
  //
  // The same holds for a stage nobody can see. Hidden by the CSS breakpoint,
  // which the candidate can cross at any time by narrowing the window or
  // docking devtools, or covered by the preflight overlay, whose level meter
  // and face detector need the main thread more than a face behind it does.
  // The width cannot see the overlay, because an element under it still has
  // its width. Loading early is the point; drawing early is not. The width
  // observer brings the loop back, and so does the end of the preflight.
  if (document.hidden) {
    pauseAvatar();
    return;
  }
  avatarFrame = requestAnimationFrame(pumpAvatar);
  // The rAF timestamp is the frame's target time and is identical across every
  // callback in that frame; performance.now() drifts by however long the loop
  // took to reach us.
  const clock = at ?? performance.now();
  // Thirty frames a second is enough for a face, and half the main-thread and
  // GPU work of sixty. A deadline that advances by the frame time, rather than
  // a gap since the last draw, holds the average to 30 on any refresh rate: a
  // minimum gap drew every third vsync at 100 Hz, 33 fps. The 1 ms slack is for
  // timestamps that land a hair early; a deadline missed by a whole frame
  // starts over instead of bursting to catch up.
  if (nextDraw !== null && clock < nextDraw - 1) return;
  if (!stageVisible()) {
    pauseAvatar();
    return;
  }
  nextDraw =
    nextDraw === null || clock - nextDraw >= AVATAR_FRAME_MS
      ? clock + AVATAR_FRAME_MS
      : nextDraw + AVATAR_FRAME_MS;
  const started = performance.now();
  avatar.setMouth(mouthFromAmplitude(jimAmplitude()));
  avatar.frame(clock);
  playout.noteDraw();
  // Cadence on the frame clock, cost on the wall clock: the rAF timestamp is
  // vsync-aligned, and performance.now() here adds callback jitter to the gap.
  if (renderBudget.sample(clock, performance.now() - started)) {
    const { stalledPct, expensivePct } = renderBudget.last();
    disableAvatar(
      "budget_exceeded",
      `stalled=${stalledPct}% expensive=${expensivePct}%`,
    );
  }
}

/// Stops the loop without tearing anything down. The budget and the playout
/// watch forget their windows, so the pause is never charged to the avatar:
/// not as one long frame, and not as delay accrued while nothing was drawn.
function pauseAvatar() {
  if (avatarFrame !== null) cancelAnimationFrame(avatarFrame);
  avatarFrame = null;
  nextDraw = null;
  renderBudget.reset();
  playout.pause();
}

/// Starts the render loop again after the tab comes back.
///
/// One listener for the life of the page, added beside the loop rather than
/// inside it: a listener added per frame is sixty listeners a second.
function resumeAvatar() {
  if (!nodes) return;
  // Paused here and not left to the next frame: a browser may suspend the
  // frame callback as the tab hides, and the stats poll keeps running, so
  // without this a hidden tab kept its playout count and could be retired
  // on delay accrued while nobody could see it.
  if (document.hidden) {
    pauseAvatar();
    return;
  }
  // No budget reset here. This also runs while the loop is going, on every
  // width change, and a reset then cleared an overloaded streak; a loop that
  // is not going stopped through `pauseAvatar`, which already reset it.
  //
  // `state()` and not just `avatar`: `createAvatar` returns before the model
  // has loaded, so a visibility change during the load would otherwise start a
  // loop that poses nothing sixty times a second.
  if (!avatar || avatar.state() !== "ready" || avatarFrame !== null) return;
  pumpAvatar();
}

document.addEventListener("visibilitychange", resumeAvatar);

/// `reason` is null for the candidate's own Hide, or what retired it, and
/// `measured` is the number that decided, logged so a real session can say
/// how close to the line it was.
function disableAvatar(reason, measured) {
  if (avatarRetired || (avatarHidden && !reason)) return;
  // Asked before the teardown, which for an automatic retirement hides the
  // button and would drop focus to the body for a keyboard user on it.
  const hadFocus = document.activeElement === nodes.hideAvatar;
  if (reason) avatarRetired = true;
  else avatarHidden = true;
  stopAvatar(reason ? "degraded" : "stopped", !reason);
  const note = reason
    ? "Jim is here by voice; his avatar was turned off to keep the interview responsive."
    : "Jim is here by voice; his avatar is turned off.";
  nodes.jimAvatarNote.textContent = note;
  if (reason) console.warn(`codetrial avatar_${reason} ${measured}`);
  // The candidate's own Hide leaves the button where it was, now as Show, so
  // focus stays on it and the same key undoes it. Moving focus to the note made
  // a keyboard user tab back to find the control they had just pressed, and
  // with focus unmoved the status region is what says the avatar went away.
  if (!reason) {
    nodes.hideAvatar.hidden = false;
    nodes.hideAvatar.textContent = "Show avatar";
    nodes.jimAvatarStatus.textContent = note;
    return;
  }
  // Focus says it once. The status region says it only when focus did not
  // move, since a note both focused and announced is read twice.
  if (hadFocus) nodes.jimAvatarNote.focus({ preventScroll: true });
  else nodes.jimAvatarStatus.textContent = note;
}

function showAvatar() {
  if (!avatarHidden || avatarRetired) return;
  const track = jimTrack;
  const participant = jimParticipant;
  jimTrack = null;
  avatarHidden = false;
  nodes.jimAvatarNote.textContent = "Jim is here by voice.";
  nodes.jimAvatarStatus.textContent = "";
  startAvatar();
  if (track && onShowAvatar) onShowAvatar(track, participant);
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
  if (avatarRetired || !track?.mediaStreamTrack) return;
  if (avatarHidden) {
    jimTrack = track;
    jimParticipant = participant;
    return;
  }
  if (track === jimTrack) return;
  // A new publication replaces the old analyser rather than being ignored.
  // Bailing out on `jimAnalyser` alone left the analyser bound to the track
  // that had just been replaced, and LiveKit does not promise the unsubscribe
  // for the old publication arrives before the subscribe for the new one: when
  // it arrived after, `dropRemoteAudio` tore down the only analyser there was
  // and nothing rebuilt it, so lip sync died for the rest of the session.
  releaseAnalyserNodes();
  jimTrack = track;
  jimParticipant = participant;
  try {
    jimAnalyserContext ||= new (
      window.AudioContext || window.webkitAudioContext
    )();
    // TrackSubscribed is not a user gesture, so a context first built here can
    // arrive suspended and then read silence forever. The mouth would simply
    // never open, with nothing anywhere saying why.
    if (jimAnalyserContext.state === "suspended")
      void jimAnalyserContext.resume().catch(() => {});
    jimAnalyserSource = jimAnalyserContext.createMediaStreamSource(
      new MediaStream([track.mediaStreamTrack]),
    );
    jimAnalyser = jimAnalyserContext.createAnalyser();
    jimAnalyser.fftSize = ANALYSER_FFT_SIZE;
    // One buffer for the session. Allocating it per frame produced 512 bytes of
    // garbage 60 times a second for the whole interview.
    jimAnalyserSamples = new Uint8Array(jimAnalyser.fftSize);
    // Not connected to the destination: the audio element is already playing
    // this track, and a second path would play Jim twice.
    jimAnalyserSource.connect(jimAnalyser);
    resumeAnalyserOnGesture();
  } catch {
    // No analyser means no lip-sync. Everything else about the avatar, and all
    // of the audio, still works, and Jim's track is still followed.
    releaseAnalyserNodes();
  }
}

/// LiveKit re-subscribes Jim on every reconnect, so this runs more than once
/// per session. Without disconnecting the source node, each reconnect left a
/// live MediaStreamAudioSourceNode attached to the same context.
export function releaseAvatarAnalyser() {
  releaseAnalyserNodes();
  jimTrack = null;
  jimParticipant = null;
}

function releaseAnalyserNodes() {
  jimAnalyserSource?.disconnect();
  jimAnalyserSource = null;
  jimAnalyser = null;
  jimAnalyserSamples = null;
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
    void jimAnalyserContext
      .resume()
      .then(stop)
      .catch(() => {});
  }
  document.addEventListener("pointerdown", resume, true);
  document.addEventListener("keydown", resume, true);
}

export function jimAmplitude() {
  if (!jimAnalyser || !jimAnalyserSamples) return 0;
  jimAnalyser.getByteTimeDomainData(jimAnalyserSamples);
  jimAnalyserPeaks.push(peakLevel(jimAnalyserSamples));
  if (jimAnalyserPeaks.length > ANALYSER_WINDOW) jimAnalyserPeaks.shift();
  return (
    jimAnalyserPeaks.reduce((total, peak) => total + peak, 0) /
    jimAnalyserPeaks.length
  );
}

/// `final` is the state the mount is left in; `createAvatar` owns the write.
/// A load still running is aborted there, before the renderer is imported or a
/// context is built: hiding is how a candidate under load asks for their CPU
/// back, and a load that ran to the end only to be disposed spent it anyway.
export function stopAvatar(final, preserveTrack = false) {
  avatarWanted = false;
  pauseAvatar();
  avatar?.destroy(final);
  avatar = null;
  if (!preserveTrack) {
    avatarHidden = false;
    nodes.hideAvatar.hidden = true;
  }
  if (preserveTrack) releaseAnalyserNodes();
  else releaseAvatarAnalyser();
  void jimAnalyserContext?.close?.().catch(() => {});
  jimAnalyserContext = null;
}

/// The three questions `interview.js` still asks about the avatar, as functions
/// rather than as exported state. A `let avatar` reachable from another module
/// is a second owner of the renderer's lifetime, and the teardown order here is
/// the whole reason this file exists.
export function isAvatarAnalyserTrack(track) {
  return track === jimTrack;
}

/// The page reports Jim's state on change, not on a tick, so a model built
/// after the last report would never hear it: one rebuilt by Show, or one first
/// built when a stage that started narrow widens. Kept here and handed to each
/// new model in `startAvatar`. Null until the page says anything, so a model
/// built before then keeps `createAvatar`'s own open-mouth default.
let avatarSpeaking = null;
let avatarExpression = null;

export function setAvatarSpeaking(speaking) {
  avatarSpeaking = speaking;
  avatar?.setSpeaking(speaking);
}

export function setAvatarExpression(value) {
  avatarExpression = value;
  avatar?.setExpression(value);
}
