import test from "node:test";
import assert from "node:assert/strict";
import {
  CALIBRATION_LINE,
  faceFilesProblem,
  lineTyped,
  typedEnough,
} from "../../web/task-calibration.js";

// These pin what web/task.js did before the checks moved out of it: the cases
// were worked out from that code, and the calibration step types the line
// below into a real page in task-workspace.test.js.

test("the calibration line is the exact text the page asks for", () => {
  assert.equal(CALIBRATION_LINE, "for item in items: total += item");
});

test("nothing typed counts as neither started nor finished", () => {
  assert.equal(typedEnough(""), false);
  assert.equal(lineTyped(""), false);
  assert.equal(typedEnough(" \n\t "), false, "whitespace is not typing");
});

test("typing is seen once a third of the line is in, ignoring spacing", () => {
  // The line is 26 characters without spaces, so a third is 8.67.
  assert.equal(typedEnough("x".repeat(8)), false);
  assert.equal(typedEnough("x".repeat(9)), true);
});

test("the line is typed only when every character is in", () => {
  const almost = "for item in items: total += ite";
  assert.equal(typedEnough(almost), true);
  assert.equal(lineTyped(almost), false, "25 of 26");
  assert.equal(lineTyped(CALIBRATION_LINE), true);
  assert.equal(lineTyped("for  item\n in items:\ttotal+=item"), true);
});

test("missing detector files are named before unreachable ones", () => {
  assert.equal(
    faceFilesProblem({
      missing: [{ file: "a.wasm" }, { file: "b.task" }],
      unreachable: ["c"],
    }),
    "This server is missing face detector files (a.wasm, b.task). Ask whoever runs it to run scripts/fetch-vendor.sh, then reload the page.",
  );
});

test("an unreachable model asks for the connection to be checked", () => {
  assert.equal(
    faceFilesProblem({ missing: [], unreachable: ["a.wasm", "b.task"] }),
    "The face model could not be downloaded (a.wasm, b.task). Check the connection, then reload the page.",
  );
});

test("no problem is an empty string, which the page treats as success", () => {
  assert.equal(faceFilesProblem({ missing: [], unreachable: [] }), "");
});
