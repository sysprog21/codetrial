/// The live subtitle over the editor.
///
/// A subtitle, not a log: the transcript tab already keeps every turn, so this
/// shows what is being said now and retires when nobody is speaking. Split out
/// of `interview.js` because the pacing state is three variables and one timer
/// that nothing else may touch, and the reveal is the only thing in the page
/// that repaints on a 300ms tick.

import { captionWindow } from "./lib.js";

let nodes = null;

export function initCaptions(deps) {
  ({ nodes } = deps);
}

const CAPTION_TICK_MS = 300;
const CAPTION_CHARS_PER_TICK = 8;

// One uninterrupted spoken turn is unbounded, and the caption line sits in the
// header: rendered whole, a long answer pushes the timer and the controls off
// the screen. The tail is the part being spoken.
const CAPTION_MAX_CHARS = 160;
// Long enough to finish reading a sentence that just landed, short enough that
// a silent pause clears the editor's corner.
const CAPTION_IDLE_HIDE_MS = 12000;
let captionIdleTimer = null;
let interviewerCaption = { id: null, text: "", shown: 0, timer: null };

export function updateCaptions(speaker, text, id = null) {
  if (!nodes.captionsText) return;
  if (speaker === "interviewer") {
    if (interviewerCaption.id !== id) {
      clearTimeout(interviewerCaption.timer);
      interviewerCaption = { id, text: "", shown: 0, timer: null };
    }
    // The longest, not the latest. Every interim publish carries the whole turn
    // so far on a stream of its own, so one that lands out of order is a
    // shorter copy of what is already on screen, and taking it would rewind the
    // reveal to a few characters and replay the line.
    if (text.length > interviewerCaption.text.length) interviewerCaption.text = text;
    if (!interviewerCaption.timer) paceInterviewerCaption();
    return;
  }
  // Stop pacing Jim: the next tick would otherwise repaint his line over the
  // candidate's. A later chunk of the same segment restarts it through the
  // `!interviewerCaption.timer` check below.
  clearTimeout(interviewerCaption.timer);
  interviewerCaption.timer = null;
  const clipped = captionWindow(text, CAPTION_MAX_CHARS);
  nodes.captionsText.textContent = `[${speaker === "you" ? "You" : "Jim"}]: ${clipped}`;
  showCaptions();
}

export function paceInterviewerCaption() {
  const caption = interviewerCaption;
  caption.shown = Math.min(caption.text.length, caption.shown + CAPTION_CHARS_PER_TICK);
  nodes.captionsText.textContent = `[Jim]: ${captionWindow(caption.text.slice(0, caption.shown), CAPTION_MAX_CHARS)}`;
  showCaptions();
  if (caption.shown < caption.text.length) {
    caption.timer = setTimeout(paceInterviewerCaption, CAPTION_TICK_MS);
  } else {
    caption.timer = null;
  }
}

/// Captions are a live subtitle, not a log: the transcript tab already keeps
/// every turn. Leaving the last thing anyone said pinned over the editor for
/// the rest of the interview is just an obstruction, so the bar retires once
/// nobody has spoken for a while and comes back on the next word.
export function showCaptions() {
  if (!nodes.captionsBar) return;
  nodes.captionsBar.hidden = false;
  clearTimeout(captionIdleTimer);
  captionIdleTimer = setTimeout(() => {
    nodes.captionsBar.hidden = true;
  }, CAPTION_IDLE_HIDE_MS);
}
