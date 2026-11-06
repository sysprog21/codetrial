// Split out of task.js so a node test can import it: task.js reads the LiveKit
// global and the DOM when it loads, and this does neither. It holds the line
// the learner types in calibration, whether they have typed enough of it, and
// what to tell them when the face model's files are missing.

/// The line the learner types during calibration.
export const CALIBRATION_LINE = "for item in items: total += item";

const compact = (value) => value.replace(/\s+/g, "");

/// Whether the typing phase saw typing: a third of the line, ignoring spacing.
export function typedEnough(text) {
  return compact(text).length >= compact(CALIBRATION_LINE).length / 3;
}

/// Whether the whole line is in, which ends the typing phase.
export function lineTyped(text) {
  return compact(text).length >= compact(CALIBRATION_LINE).length;
}

/// What to tell a learner when the server lacks the detector's files, which no
/// amount of retrying fixes and which "not answering" would send them to retry.
export function faceFilesProblem({ missing, unreachable }) {
  if (missing.length)
    return `This server is missing face detector files (${missing.map(({ file }) => file).join(", ")}). Ask whoever runs it to run scripts/fetch-vendor.sh, then reload the page.`;
  return unreachable.length
    ? `The face model could not be downloaded (${unreachable.join(", ")}). Check the connection, then reload the page.`
    : "";
}
