// Pure browser state: progress and deadlines come only from server snapshots.
import { formatTime } from "./lib.js";

export function acceptTaskState(previous, incoming) {
  if (
    incoming?.version !== 1 ||
    !Number.isSafeInteger(incoming.seq) ||
    incoming.seq < 0 ||
    (previous && incoming.seq <= previous.seq)
  )
    return previous;
  if (
    !["ready", "work", "wrap-up", "feedback"].includes(incoming.phase) ||
    !Array.isArray(incoming.checks)
  )
    return previous;
  return incoming;
}

export function taskLocked(state) {
  return (
    !state ||
    !["work", "wrap-up"].includes(state.phase) ||
    Boolean(state.finalRevisionId)
  );
}

export function taskRecovery(code) {
  const actions = {
    credentials_missing: "Reload after restarting CodeTrial",
    loopback_required: "Read the setup notes",
    authentication_required: "Sign in",
    csrf_rejected: "Reload",
    site_invalid: "Check the assignment link",
    invalid_pin: "Retry PIN",
    unlock_required: "Enter PIN",
    package_unavailable: "Retry load",
    package_invalid: "Ask the instructor",
    set_closed: "Return to task list",
    task_not_found: "Return to task list",
    rules_unacknowledged: "Read and accept the rules",
    devices_required: "Allow microphone and camera",
    calibration_required: "Run calibration",
    fullscreen_required: "Retry Start",
    client_override: "Reload",
    language_unsupported: "Select Python",
    platform_unsupported: "Open a supported desktop browser",
    request_key_expired: "Start again",
    setup_failed: "Retry",
    setup_pending: "Retry",
    admission_throttled: "Wait, then retry Start",
    request_too_large: "Reload",
    active_task_session: "Finish in original open tab",
    result_unavailable: "Start a new attempt",
    action_rejected: "Retry action",
  };
  return actions[code] ?? "Reload";
}

/// The set's rules as the learner reads them, before the PIN on the
/// instructor's page and again before Start here. The packaging tool's
/// `rules_in_words` says the same in Python; keep the two together.
export function rulesInWords(rules) {
  const lines = [
    `You have ${rules.durationMin} minutes once you press Start.`,
    "Stay in fullscreen for the whole attempt: leaving it ends the attempt at once and marks it invalid.",
    "Keep this page visible: switching to another tab or window, minimizing it or locking the screen ends the attempt at once and marks it invalid.",
    `Keep your face in front of the camera. Looking away or down, or leaving the frame, for more than ${rules.lookAwaySeconds} seconds in a row ends the attempt and marks it invalid; a glance at the keyboard is fine. A countdown and a sound warn you first.`,
    "Use the calculator inside the workspace instead of another app.",
    "Turn off notifications and keep your machine plugged in before you start.",
  ];
  if (rules.closesAt)
    lines.push(`Attempts must start before ${rules.closesAt}.`);
  if (rules.rulesNote) lines.push(rules.rulesNote);
  return lines;
}

/// What is not detected, said plainly so nobody mistakes the rules for more
/// than a deterrent.
export const NOT_DETECTED =
  "A second monitor, a phone held at screen height or an overlay under the camera are not detected; the rules deter rather than prevent.";

const CAUSES = {
  finish: "You finished the task.",
  timeout: "Time ran out.",
  fullscreen: "The workspace left fullscreen",
  visibility: "The page was hidden",
  look_away: "Your face was out of frame or turned down",
  camera: "The camera stopped",
  detector: "The face detector stopped answering",
  reload: "The page was reloaded or closed",
  room_loss: "The connection to the task room was lost",
};

/// One factual sentence about how an attempt ended. It names the rule or
/// failure and when, and never says or implies why the learner did it.
export function outcomeSentence(outcome) {
  if (!outcome) return "";
  const cause = CAUSES[outcome.cause] ?? "The attempt ended";
  if (outcome.outcome === "completed") return cause;
  const at = formatTime(outcome.at);
  if (outcome.outcome === "invalid")
    return `${cause} at ${at}; the attempt ended under the announced rules and is invalid.`;
  return `${cause} at ${at}; the attempt was interrupted and can be started again.`;
}

export function taskReviewForDisplay(value) {
  const unavailable = { assessmentMode: "task", feedbackUnavailable: true };
  const versions = ["package", "prompt", "wire", "reportSchema", "rubric"];
  if (
    value?.assessmentMode !== "task" ||
    !value.taskContract ||
    Object.keys(value.taskContract).length !== versions.length ||
    !versions.every((key) => value.taskContract[key] === 1)
  )
    return unavailable;
  if (value.feedbackUnavailable) return value;
  const assessment = value.taskAssessment;
  // An invalid or interrupted attempt is a record of how it ended, with no
  // ratings to check.
  if (["invalid", "interrupted"].includes(assessment?.outcome?.outcome))
    return value;
  const dimensions = [
    "reasoningParticipation",
    "implementationOwnership",
    "testingAndDiagnosis",
    "revisionAndImprovement",
  ];
  if (
    !assessment?.dimensions ||
    Object.keys(assessment.dimensions).length !== dimensions.length ||
    !dimensions.every((name) => {
      const dimension = assessment.dimensions[name];
      return (
        dimension &&
        typeof dimension.reason === "string" &&
        (dimension.rating === null ||
          (Number.isInteger(dimension.rating) &&
            dimension.rating >= 0 &&
            dimension.rating <= 3))
      );
    }) ||
    !["gaps", "nextActions"].every(
      (key) =>
        Array.isArray(assessment[key]) &&
        assessment[key].every((text) => typeof text === "string"),
    )
  )
    return unavailable;
  return value;
}
