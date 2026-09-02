#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TMP=$(mktemp -d)
cleanup()
{
    rm -r "$TMP" 2> /dev/null || true
}
trap cleanup EXIT INT TERM

if [ "${REPORT_PARITY_CHECK_VALIDATE_FIXTURES_ONLY:-}" ]; then
    PY_CAPTURE="$ROOT/tests/golden/report-python.json" node << 'NODE'
const fs = require("fs");
const capture = JSON.parse(fs.readFileSync(process.env.PY_CAPTURE, "utf8"));
function assert(condition, message) {
  if (!condition) {
    console.error(message);
    process.exit(1);
  }
}
assert(capture.problemTitle === "Two Sum", "report reference problem mismatch");
assert(capture.testResultText && capture.testResultText !== "Couldn't run your code", "report reference missing test result");
assert(Array.isArray(capture.reportKeys), "report reference missing keys");
for (const key of ["codingScore", "communicationScore", "decision", "summary", "codingFeedback", "communicationFeedback", "hintsUsed"]) {
  assert(capture.reportKeys.includes(key), `report reference missing ${key}`);
}
NODE
    exit 0
fi

run_capture()
{
    output=$1
    attempt=1
    while [ "$attempt" -le 2 ]; do

        # The `${...}` inside the single quotes is a JavaScript template literal
        # that node expands, not a shell expansion this script wants back.
        # shellcheck disable=SC2016
        if BROWSER_CHECK_AGENT=rust BROWSER_CHECK_FLOW=report BROWSER_CHECK_CAPTURE="$output" "$ROOT/scripts/browser-check.sh" \
            && CAPTURE="$output" node -e 'const c=JSON.parse(require("fs").readFileSync(process.env.CAPTURE,"utf8")); if (c.report?.error === true) { console.error(`rust report fallback: ${String(c.report.summary || "").slice(0, 240)}`); process.exit(1); }'; then
            return 0
        fi
        attempt=$((attempt + 1))
    done
    return 1
}

run_capture "$TMP/rust.json"

PY_CAPTURE="$ROOT/tests/golden/report-python.json" RUST_CAPTURE="$TMP/rust.json" node << 'NODE'
const fs = require("fs");
const python = JSON.parse(fs.readFileSync(process.env.PY_CAPTURE, "utf8"));
const rust = JSON.parse(fs.readFileSync(process.env.RUST_CAPTURE, "utf8"));

function assert(condition, message) {
  if (!condition) {
    console.error(message);
    process.exit(1);
  }
}

function shape(report, mode) {
  assert(report && typeof report === "object", `${mode}: missing report`);
  assert(report.error !== true, `${mode}: fallback report`);
  assert(Number.isInteger(report.codingScore) && report.codingScore >= 0 && report.codingScore <= 100, `${mode}: bad coding score`);
  assert(Number.isInteger(report.communicationScore) && report.communicationScore >= 0 && report.communicationScore <= 100, `${mode}: bad communication score`);
  assert(["HIRE", "NO_HIRE"].includes(report.decision), `${mode}: bad decision`);
  assert(typeof report.summary === "string", `${mode}: bad summary`);
  assert(Array.isArray(report.codingFeedback?.strengths), `${mode}: bad coding strengths`);
  assert(Array.isArray(report.codingFeedback?.improvements), `${mode}: bad coding improvements`);
  assert(Array.isArray(report.communicationFeedback?.strengths), `${mode}: bad communication strengths`);
  assert(Array.isArray(report.communicationFeedback?.improvements), `${mode}: bad communication improvements`);
  assert(Number.isInteger(report.hintsUsed) && report.hintsUsed >= 0, `${mode}: bad hints`);
  return Object.keys(report).sort().join(",");
}

assert(python.problemTitle === "Two Sum", "python problem mismatch");
assert(rust.problemTitle === "Two Sum", "rust problem mismatch");
assert(python.problemTitle === rust.problemTitle, "problem mismatch");
assert(python.testResultText === rust.testResultText, "test result mismatch");
assert(python.testResultText !== "Couldn't run your code", "test run setup failed");
assert(Array.isArray(python.reportKeys), "python reference missing report keys");
assert(python.reportKeys.join(",") === shape(rust.report, "rust"), "report key mismatch");
console.log(`report parity comparison passed: problem=${python.problemTitle} tests=${python.testResultText} rustDecision=${rust.report.decision} rustScores=${rust.report.codingScore}/${rust.report.communicationScore}`);
NODE
