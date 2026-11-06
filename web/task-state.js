// The task page's mutable state, in one object so that code which moves out of
// task.js can read and write it. Sealed: a misspelled name throws on a write
// instead of creating a field nobody reads. The fields set from the URL or the
// browser are assigned in task.js, where they are computed.

export const st = Object.seal({
  setId: undefined,
  taskId: undefined,
  site: undefined,
  version: undefined,
  // The default task, for a learner with nothing to sign in to: set by the
  // session where task mode is on.
  builtIn: null,
  // Whether the learner asked for their instructor's tasks while not signed in,
  // so the sign-in that follows ends at the server, not at a PIN.
  wantsInstructor: false,
  task: undefined,
  state: undefined,
  room: undefined,
  review: undefined,
  // The server clock against `performance.now()` and `Date.now()`; null until
  // a snapshot arrives.
  serverClockOffset: null,
  // The whiteboard, mounted once the page is loaded; `lock` repaints it.
  board: undefined,
  // The editor's syntax colours and line numbers, behind the textarea.
  editorView: undefined,
  // The cube a task may offer to turn, once one has.
  cubeMounted: undefined,
  revisionId: "initial",
  // The language chosen for this attempt: the task's first until the learner
  // picks another, and fixed once a room is joined for it.
  language: "python",
  admissionKey: undefined,
  pendingRevision: undefined,
  // Where the attempt stands; every guard asks this rather than a flag of its
  // own:
  //   "preparing"  before Start is sent;
  //   "starting"   Start is on its way and not yet acknowledged: the server may
  //                already be running the clock;
  //   "running"    started, with the rules armed;
  //   "ending"     an ending is under way, reported by the page or the server,
  //                so CodeTrial leaving fullscreen itself, or a second rule
  //                firing, is not reported again;
  //   "lost"       started, or perhaps started, but the room is gone: the
  //                server finishes the attempt and the page only collects the
  //                result;
  //   "done"       the result is shown.
  attempt: "preparing",
  runBusy: false,
  // A Discuss or Finish between its capture and its action: an edit sent in
  // between would move the server past the revision the action names.
  actionPending: false,
  editTimer: undefined,
  activeRoomName: undefined,
  reviewPollUntil: undefined,
  // The room was lost before Start was acknowledged, so the server may hold an
  // attempt or nothing at all; nothing retained then means it never began.
  startUnconfirmed: false,
  // A rule broken while Start was being acknowledged, before the rules were
  // armed; it still ends the attempt once it runs.
  startViolation: null,
  reviewPolling: false,
  recoveryTimer: undefined,
  acknowledgedAt: null,
  media: null,
  detector: null,
  // Whether the detector has answered once, which is the model loaded and
  // running and not only downloaded.
  faceReady: false,
  calibration: null,
  calibrating: false,
  monitorTimer: null,
  wakeLock: null,
  beepContext: null,
  enteringFullscreen: false,
  // A Start sent but not yet confirmed: its room, polling deadline and storage
  // key, kept so a reload in between still finds its way back to the result.
  startInFlight: null,
  // This tab was reloaded into an attempt it can only collect the result of.
  restored: false,
  loadGeneration: 0,
  // The Start in progress; a newer one makes an older one's continuation moot.
  currentStart: null,
  // F2 lets the next Tab leave the editor, so the keyboard is not trapped in
  // it. Not Escape, as in interviews: the attempt runs in fullscreen, which the
  // browser leaves on Escape, and leaving it ends the attempt.
  tabLeavesEditor: false,
  // The whiteboard: the interview's example drawing, so Jim can read what a
  // learner sketched while explaining. The picture goes to the room as a byte
  // stream once the drawing settles; Jim looks at it only when the learner
  // refers to it, and it is never evidence.
  boardSequence: 0,
  // The clock and the result poll run from Start, or a restored attempt, until
  // the result is shown; before and after there is nothing for them to do.
  attemptTimers: null,
});

/// Whether Start has been sent: every state of the attempt after "preparing".
export const hasStarted = () => st.attempt !== "preparing";

/// Whether the attempt is over for good: its result is shown, or its room is
/// lost. A send still in flight then fails on the room that is gone, which is
/// no news to the learner looking at the result.
export const attemptFinished = () =>
  st.attempt === "done" || st.attempt === "lost";

/// Whether the rules are being watched: the attempt is running and the server
/// has not yet moved on to feedback.
export function monitoring() {
  return st.attempt === "running" && st.state?.phase !== "feedback";
}

// What task.js still owns and the modules split out of it reach back for, by
// name. task.js lends each one near its top, as a getter so that the name is
// read at the call and not when it is lent; an entry goes from that list when
// the function it names moves into a module of its own. Because a split-out
// module imports this file and not task.js, it can be imported without the page.
export const host = {};

export function lend(getters) {
  for (const [name, get] of Object.entries(getters))
    Object.defineProperty(host, name, { get, configurable: true });
}
