/// Captions over the editor.
///
/// Keep completed interviewer turns readable while the candidate thinks.
/// Interviewer captions reveal gradually, including final copies, and never
/// drop the start of the turn. Candidate captions retain their bounded window
/// and idle timeout.

import { captionWindow, MAX_UNCLEAR_LINES } from "./lib.js";
import { atScrollEnd, scrolledByCandidate } from "./scroll-follow.js";

let nodes = null;

export function initCaptions(deps) {
  ({ nodes } = deps);
  nodes.captionUnclear?.addEventListener("click", toggleUnclear);
  nodes.captionsBar?.addEventListener?.("scroll", trackCandidateScroll);
}

const CAPTION_TICK_MS = 300;
const CAPTION_CHARS_PER_TICK = 8;

// Only the candidate's caption is windowed. Jim's turn is what the candidate
// has to answer, so the bar scrolls rather than cutting its opening.
const CAPTION_MAX_CHARS = 160;
// Candidate captions and the listening placeholder should clear when idle.
const CAPTION_IDLE_HIDE_MS = 12000;
let captionIdleTimer = null;
// Whose words the bar holds, since only a Jim caption has a follow to change.
let jimOnBar = false;
// Where the bar was last seen resting, by a tick or by the scroll listener,
// so a move since then that its geometry cannot explain is the candidate's.
let seenTop = 0;
let interviewerCaption = {
  id: null,
  text: "",
  shown: 0,
  timer: null,
  final: false,
  pendingDismissal: false,
  following: true,
};

// Each publish opens a separate stream, so replaced turns can arrive late.
const retiredInterviewerIds = new Set();
const retiredCandidateIds = new Set();
// Marks are the candidate's study notes: kept here, never sent to Jim.
const unclearIds = new Set();
let candidateCaption = { id: null, text: "", replaced: false };

export function updateCaptions(
  speaker,
  text,
  id = null,
  { final = false, finalStream = final } = {},
) {
  if (!nodes.captionsText) return;
  if (speaker === "interviewer") {
    if (retiredInterviewerIds.has(id)) return;
    if (candidateCaption.id != null) candidateCaption.replaced = true;
    if (interviewerCaption.id !== id) {
      retireInterviewerCaption();
      // Wherever the candidate left the last turn, this one follows from its
      // first tick, which also scrolls a short opening back to the top. A
      // scroll of the last turn still queued must not decide this one's
      // follow, so its first tick skips the check for a candidate's move.
      jimOnBar = false;
      interviewerCaption = {
        id,
        text: "",
        shown: 0,
        timer: null,
        final: false,
        pendingDismissal: false,
        following: true,
      };
    }
    // The longest, not the latest. Every interim publish carries the whole turn
    // so far on a stream of its own, so one that lands out of order is a
    // shorter copy of what is already on screen, and taking it would rewind the
    // reveal to a few characters and replay the line.
    if (text.length > interviewerCaption.text.length)
      interviewerCaption.text = text;
    if (final) interviewerCaption.final = true;

    // Final text can arrive while audio is still queued.
    if (interviewerCaption.final) retireCandidateCaption();
    if (!interviewerCaption.timer) paceInterviewerCaption();
    return;
  }

  if (retiredCandidateIds.has(id)) return;
  const newCandidateSegment = candidateCaption.id !== id;
  if (newCandidateSegment) {
    retireCandidateCaption();
    candidateCaption = { id, text: "", replaced: false };
  }

  // A closing copy is not a new response to the interviewer.
  if (
    candidateCaption.replaced &&
    (finalStream || text.length <= candidateCaption.text.length)
  ) {
    return;
  }

  if (text.length > candidateCaption.text.length) candidateCaption.text = text;
  candidateCaption.replaced = false;

  // A new candidate segment supersedes Jim. Otherwise, only a final Jim
  // turn is retired; a live turn can resume after late candidate speech.
  if (newCandidateSegment || interviewerCaption.final) {
    retireInterviewerCaption();
  } else {
    clearTimeout(interviewerCaption.timer);
    interviewerCaption.timer = null;
  }
  const clipped = captionWindow(text, CAPTION_MAX_CHARS);
  showUnclearButton(false);
  jimOnBar = false;
  nodes.captionsText.textContent = `[${speaker === "you" ? "You" : "Jim"}]: ${clipped}`;
  showCaptions();
}

export function paceInterviewerCaption() {
  const caption = interviewerCaption;
  const bar = nodes.captionsBar;
  // A scroll the candidate made since the last tick can still be queued, and
  // this tick's follow would write over it; the bar has already moved, though.
  if (bar && jimOnBar && scrolledByCandidate(bar, seenTop)) {
    caption.following = atScrollEnd(bar);
  }
  caption.shown = Math.min(
    caption.text.length,
    caption.shown + CAPTION_CHARS_PER_TICK,
  );
  const retained = caption.final && caption.shown === caption.text.length;
  jimOnBar = true;
  nodes.captionsText.textContent = `[Jim]: ${caption.text.slice(0, caption.shown)}`;
  showUnclearButton(true);
  if (retained && caption.pendingDismissal) {
    dismissCaptions();
    return;
  }
  showCaptions({ persistent: retained });
  // After showCaptions: a hidden bar has no scroll position to set.
  if (bar) {
    if (caption.following) bar.scrollTop = bar.scrollHeight;
    // The new text can clamp the bar as well as the follow can move it, and
    // either queues an event that is not the candidate's.
    seenTop = bar.scrollTop;
  }
  if (caption.shown < caption.text.length) {
    caption.timer = setTimeout(paceInterviewerCaption, CAPTION_TICK_MS);
  } else {
    caption.timer = null;
  }
}

/// Only the candidate decides whether Jim's reveal follows, but a scroll event
/// also comes from our own follow and from the browser clamping scrollTop when
/// Jim's text is replaced or the bar's size changes. While the bar holds the
/// candidate's words, nothing done to it is about Jim.
function trackCandidateScroll() {
  const bar = nodes.captionsBar;
  if (!jimOnBar) return;
  if (scrolledByCandidate(bar, seenTop)) {
    interviewerCaption.following = atScrollEnd(bar);
  }
  seenTop = bar.scrollTop;
}

/// Completed interviewer turns remain available while the candidate thinks.
/// Other captions expire so an idle subtitle does not cover the editor.
export function showCaptions({ persistent = false } = {}) {
  if (!nodes.captionsBar) return;
  nodes.captionsBar.hidden = false;
  clearTimeout(captionIdleTimer);
  captionIdleTimer = null;

  if (persistent) return;

  captionIdleTimer = setTimeout(() => {
    releaseUnclearFocus();
    nodes.captionsBar.hidden = true;
  }, CAPTION_IDLE_HIDE_MS);
}

/// Remember a code response until Jim's final text finishes revealing.
export function dismissCaptions() {
  if (
    !interviewerCaption.text ||
    retiredInterviewerIds.has(interviewerCaption.id)
  ) {
    return;
  }

  interviewerCaption.pendingDismissal = true;
  if (
    !interviewerCaption.final ||
    interviewerCaption.shown < interviewerCaption.text.length
  ) {
    return;
  }

  retireInterviewerCaption();
  clearTimeout(captionIdleTimer);
  captionIdleTimer = null;
  releaseUnclearFocus();
  if (nodes.captionsBar) nodes.captionsBar.hidden = true;
}

function retireInterviewerCaption() {
  clearTimeout(interviewerCaption.timer);
  interviewerCaption.timer = null;
  interviewerCaption.final = false;
  interviewerCaption.pendingDismissal = false;
  if (interviewerCaption.id != null) {
    retiredInterviewerIds.add(interviewerCaption.id);
  }
}

function retireCandidateCaption() {
  if (candidateCaption.id != null) {
    retiredCandidateIds.add(candidateCaption.id);
  }
}

/// The marked interviewer turn ids, for the report to resolve into text.
export function unclearLineIds() {
  return [...unclearIds];
}

function refreshUnclearButton() {
  const button = nodes.captionUnclear;
  if (!button) return;
  const marked = unclearIds.has(interviewerCaption.id);
  const full = !marked && unclearIds.size >= MAX_UNCLEAR_LINES;
  button.setAttribute("aria-pressed", String(marked));
  // aria-disabled rather than disabled: disabling the focused button would
  // drop keyboard focus to <body>, and toggleUnclear already refuses the cap.
  button.setAttribute("aria-disabled", String(full));
  button.title = full ? `Up to ${MAX_UNCLEAR_LINES} lines` : "";
}

function toggleUnclear(event) {
  const id = interviewerCaption.id;
  if (id == null) return;
  if (unclearIds.has(id)) {
    unclearIds.delete(id);
  } else if (unclearIds.size < MAX_UNCLEAR_LINES) {
    unclearIds.add(id);
  }
  refreshUnclearButton();
  // A click leaves focus on the button, where the candidate's next keystrokes
  // would land and the first space would undo the mark. Space or Enter
  // (detail 0) means they came here by keyboard, so focus stays.
  if (event?.detail > 0) focusWorkspace();
}

function showUnclearButton(visible) {
  if (!nodes.captionUnclear) return;
  if (!visible) releaseUnclearFocus();
  nodes.captionUnclear.hidden = !visible;
  if (visible) refreshUnclearButton();
}
/// Hiding the focused button, or the bar that holds it, would drop keyboard
/// focus to the page body, so it moves to the workspace first.
function releaseUnclearFocus() {
  const button = nodes.captionUnclear;
  if (button?.ownerDocument?.activeElement !== button) return;
  focusWorkspace();
}

/// The surface in use: the editor unless a whiteboard removed it or a pause or
/// the behavioral round disabled it, then the board, then Jim's stage.
function focusWorkspace() {
  const document = nodes.captionUnclear?.ownerDocument;
  for (const target of [nodes.editor, nodes.board, nodes.jimStage]) {
    if (!target?.isConnected || target.disabled) continue;
    target.focus();
    if (document?.activeElement === target) return;
  }
}
