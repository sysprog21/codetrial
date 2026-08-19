#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TMP=$(mktemp -d)
cleanup() {
  rm -r "$TMP" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

if [ "${PARITY_CHECK_VALIDATE_FIXTURES_ONLY:-}" ]; then
  PY_CAPTURE="$ROOT/tests/golden/browser-python.json" node <<'NODE'
const fs = require("fs");
const capture = JSON.parse(fs.readFileSync(process.env.PY_CAPTURE, "utf8"));
function assert(condition, message) {
  if (!condition) {
    console.error(message);
    process.exit(1);
  }
}
assert(capture.problemTitle === "Two Sum", "browser reference problem mismatch");
// A recording of what the Python implementation did, so it keeps whatever
// speaker label that run produced. Do not update it to match the current agent.
assert(capture.firstTranscriptSpeaker === "alex", "browser reference speaker mismatch");
assert(Number.isInteger(capture.transcriptSegmentCount) && capture.transcriptSegmentCount > 0, "browser reference missing transcript");
NODE
  exit 0
fi

BROWSER_CHECK_AGENT=rust BROWSER_CHECK_CAPTURE="$TMP/rust.json" "$ROOT/scripts/browser-check.sh"

PY_CAPTURE="$ROOT/tests/golden/browser-python.json" RUST_CAPTURE="$TMP/rust.json" node <<'NODE'
const fs = require("fs");
const python = JSON.parse(fs.readFileSync(process.env.PY_CAPTURE, "utf8"));
const rust = JSON.parse(fs.readFileSync(process.env.RUST_CAPTURE, "utf8"));

function assert(condition, message) {
  if (!condition) {
    console.error(message);
    process.exit(1);
  }
}

// The interviewer's display name deliberately differs from the Python
// reference, so parity is that the interviewer opens the conversation, not that
// both implementations label that speaker identically.
const openedWithInterviewer = (capture) =>
  typeof capture.firstTranscriptSpeaker === "string"
  && capture.firstTranscriptSpeaker !== ""
  && capture.firstTranscriptSpeaker !== "you";

for (const capture of [python, rust]) {
  assert(capture.problemTitle === "Two Sum", `${capture.mode}: wrong problem`);
  assert(["Listening", "Thinking", "Speaking"].includes(capture.agentState), `${capture.mode}: missing assistant state`);
  assert(capture.transcriptSegmentCount > 0, `${capture.mode}: missing transcript`);
  assert(openedWithInterviewer(capture), `${capture.mode}: missing interviewer transcript`);
}
assert(python.problemTitle === rust.problemTitle, "problem mismatch");
assert(Array.isArray(rust.rustAgentParticipants) && rust.rustAgentParticipants.length === 1, "rust room was not isolated to one agent");
console.log(`browser parity comparison passed: problem=${python.problemTitle} rustState=${rust.agentState} firstSpeaker=${python.firstTranscriptSpeaker}/${rust.firstTranscriptSpeaker}`);
NODE
