/// Starting a recording, saying whether it is running, and stopping it when the
/// candidate withdraws.
///
/// Split out of `interview.js` because the candidate agreed to be recorded and
/// is owed the answer to "is it". That answer is a request, a poll, a label and
/// a withdrawal button that must never claim more than it did, and keeping them
/// together is what stops the label and the truth from drifting apart.
///
/// `addTranscript` and `setBanner` arrive as functions rather than being
/// imported: they write into the page's transcript view and banner state, which
/// `interview.js` owns, and importing them back would make the two files
/// circular.

import { closeReplay } from "./replay-feed.js";

let state = null;
let nodes = null;
let recordingEnabled = false;
let addTranscript = () => {};
let setBanner = () => {};

export function initRecording(deps) {
  ({ state, nodes, recordingEnabled, addTranscript, setBanner } = deps);
}

let recordingPoll = null;

/// Slower than the timer on purpose: the states a recording moves through are
/// minutes apart, and a poll per second would be a request per second for a
/// word that does not change.
const RECORDING_POLL_MS = 15000;

/// Whether the recording notice has been agreed to, or does not apply.
export function consentGiven() {
  return !recordingEnabled || nodes.recordingConsent.checked;
}

/// Asks the server to start recording, and then to keep saying whether it is.
///
/// The candidate agreed to be recorded and is owed the answer to "is it". A
/// silent failure here is the worst outcome available: they believe the
/// interview is being kept and it is not.
export async function startRecording() {
  try {
    const response = await fetch(`/api/interviews/${encodeURIComponent(state.interviewId)}/recording`, {
      method: "POST",
    });
    if (!response.ok) throw new Error((await response.json())?.error || "The recording could not be started.");
    showRecordingState(await response.json());
  } catch (error) {
    // Still polls. A refused start and a start that succeeded and could not be
    // read back look the same from here, and the status route is the thing that
    // can tell them apart: it answers 404 when there is nothing, and polling
    // stops on that.
    // Neutral, not a verdict. This call cannot tell a refused start from one
    // that worked and could not be read back, and the status route can.
    console.warn("codetrial recording_start_failed", error);
    nodes.recordingState.hidden = false;
    nodes.recordingState.textContent = "Checking whether the recording started.";
  }
  // One timer, cleared first. `connect` runs again on a reconnect, and an
  // interval per attempt is a request per attempt per period forever. Armed
  // only when the first answer was not already terminal, because
  // `showRecordingState` clears it and an interval created afterwards would
  // survive one pointless request.
  clearInterval(recordingPoll);
  if (!nodes.recordingState.dataset.settled) {
    recordingPoll = setInterval(pollRecordingState, RECORDING_POLL_MS);
  }
}

export async function pollRecordingState() {
  if (!state.interviewId) return;
  try {
    const response = await fetch(`/api/interviews/${encodeURIComponent(state.interviewId)}/recording`);
    if (response.ok) {
      showRecordingState(await response.json());
      return;
    }
    // Gone, not ours, or signed out. None of these change by asking again, and
    // a session that expired would otherwise be a request every fifteen seconds
    // for the rest of the page's life.
    if ([401, 403, 404].includes(response.status)) {
      stopRecordingPoll();
      nodes.recordingState.hidden = false;
      // Every one of these is terminal, so every one of them replaces the
      // label. Stopping the poll and leaving "Checking whether the recording
      // started." on screen is the worst reading available: the candidate
      // agreed to be recorded and is left believing the answer is still
      // coming, forever, when the server has already said there is nothing.
      nodes.recordingState.textContent =
        response.status === 401
          ? "Sign in again to see the recording status."
          : "The recording did not start. The interview is not being recorded.";
      nodes.recordingState.dataset.settled = "1";
    }
  } catch (error) {
    // The interview is the thing that matters; a status that cannot be read is
    // left showing whatever it last said rather than blanked.
    console.warn("codetrial recording_status_failed", error);
  }
}

/// The state in words a candidate can act on. "failed" is not an instruction,
/// so the recovery the server returns is what picks the sentence.
export function showRecordingState(status) {
  const words = {
    starting: "Recording is starting.",
    recording: "Recording.",
    finalizing: "Recording is finishing.",
    transferring: "Recording is being saved.",
    ready: "Recording saved. The link goes to your verified GitHub email.",
    deleted: "Recording deleted.",
  };
  const recovery = {
    start_again: "Recording stopped and was not kept.",
    retry_delivery: "Recording was kept and has not been delivered yet.",
    none: "Recording stopped at your request.",
    wait_for_operator: "Recording is turned off on this server.",
    // Neither of these means the recording is gone, and saying it was not kept
    // would be the opposite of true: the media may still exist.
    delete_by_hand: "Recording deletion needs an operator. It has not been deleted yet.",
    clear_the_row_by_hand: "Recording was deleted. Its record needs an operator.",
  };
  nodes.recordingState.hidden = false;
  // The recovery wins where there is one. A recording that is `transferring`
  // with `drive_failed` is being saved and is also not delivered, and the
  // second half is the half worth saying.
  nodes.recordingState.textContent =
    recovery[status?.recovery] || words[status?.state] || "Recording stopped and was not kept.";

  // Nothing changes after a terminal state, so nothing keeps asking. Ending the
  // interview is not the end of the recording: `transferring` and `ready` both
  // happen afterwards, which is exactly when a candidate wants to know.
  // A `failed` recording whose recovery is another delivery attempt is not
  // finished: the transfer queue can still deliver it, and the candidate is the
  // person who wants to know when it does.
  const settled = ["ready", "deleted", "cleanup_failed"].includes(status?.state)
    || (status?.state === "failed" && status?.recovery !== "retry_delivery");
  // Recorded on the element rather than in a variable, because `startRecording`
  // reads it after this runs and the two are not in the same call.
  if (settled) {
    nodes.recordingState.dataset.settled = "1";
    stopRecordingPoll();
  } else {
    delete nodes.recordingState.dataset.settled;
  }
}

/// Takes consent back, mid-interview.
///
/// No confirmation step. Stopping a recording is the safe direction of a
/// misclick, because the other one cannot be undone, and a candidate reaching
/// for this button is not in a mood to be asked twice.
///
/// The button does not come back. Consent that was withdrawn stays withdrawn:
/// re-agreeing would be a new interview, and this page is already in one.
export async function withdrawRecordingConsent() {
  if (!state.interviewId) return;
  nodes.withdrawConsent.disabled = true;
  try {
    const response = await fetch(`/api/interviews/${encodeURIComponent(state.interviewId)}/consent`, {
      method: "DELETE",
    });
    if (!response.ok) throw new Error("The server did not accept the request.");
    // The replay stops here, not at the next refusal. The server will refuse
    // it, so this changes nothing it can see; what it changes is that a
    // candidate who has just said stop does not keep sending their editor and
    // their words for another second while the answer comes back.
    closeReplay();
    // "Requested", not "stopped". This route records the withdrawal; stopping
    // the provider and scheduling the deletion is the recording lifecycle's
    // work, and a button that reports a completed action it did not perform is
    // the worst possible place to be optimistic.
    nodes.withdrawConsent.textContent = "Recording stop requested";
    addTranscript("interviewer", "Your withdrawal is recorded. Recording will stop and the file is scheduled for deletion. Copies anyone already downloaded cannot be recalled.", true);
  } catch (error) {
    console.warn("codetrial withdraw_consent_failed", error);
    nodes.withdrawConsent.disabled = false;
    setBanner("connection", "Could not stop the recording. Try again, or end the interview.");
  }
}

/// Stops asking about a recording once the interview is over.
///
/// The interval outlives the room otherwise: an inert timer at best, and a
/// request every fifteen seconds for a word that will not change at worst.
export function stopRecordingPoll() {
  clearInterval(recordingPoll);
  recordingPoll = null;
}
