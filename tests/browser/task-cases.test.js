import test from "node:test";
import assert from "node:assert/strict";
import { caseLabel, caseLine } from "../../web/task-cases.js";

// The strings below are what the task workspace shows for a run. Required cases
// are given by index, because a variant rename can make two cases' shown labels
// equal.

test("a case is named by its label, or by its place when it has none", () => {
  assert.equal(caseLabel({ label: "Empty input" }, 0, []), "Empty input");
  assert.equal(caseLabel({}, 0, []), "Case 1");
  assert.equal(caseLabel({}, 2, []), "Case 3");
  assert.equal(
    caseLabel({ label: "" }, 0, []),
    "",
    "an empty label is a label",
  );
});

test("a required case is marked by its index and not by its label", () => {
  const rows = [{ label: "Same" }, { label: "Same" }];
  assert.equal(caseLabel(rows[0], 0, [1]), "Same");
  assert.equal(caseLabel(rows[1], 1, [1]), "Same (required)");
  assert.equal(caseLabel({}, 1, [1]), "Case 2 (required)");
});

test("a task with no list of required cases marks none", () => {
  assert.equal(caseLabel({ label: "A" }, 0, undefined), "A");
  assert.equal(caseLabel({ label: "A" }, 0, null), "A");
});

test("a passing case is one short line", () => {
  assert.equal(caseLine({ label: "A", pass: true }, 0, []), "OK: A");
  assert.equal(caseLine({ pass: true }, 1, [1]), "OK: Case 2 (required)");
});

test("a failure says what went in, what was expected and what came back", () => {
  const row = { label: "A", input: "3", expected: "5", got: "4" };
  assert.equal(caseLine(row, 0, []), "FAIL: A (input 3; expected 5; got 4)");
});

test("an error replaces what came back", () => {
  const row = {
    label: "A",
    input: "3",
    expected: "5",
    error: "boom",
    got: "4",
  };
  assert.equal(
    caseLine(row, 0, []),
    "FAIL: A (input 3; expected 5; error boom)",
  );
});

test("a value that is zero or empty is still reported, an absent one is not", () => {
  assert.equal(
    caseLine({ label: "A", input: 0, expected: "", got: false }, 0, []),
    "FAIL: A (input 0; expected ; got false)",
  );
  assert.equal(caseLine({ label: "A", got: "x" }, 0, []), "FAIL: A (got x)");
  assert.equal(caseLine({ label: "A" }, 0, []), "FAIL: A");
});

test("an empty error does not hide what came back", () => {
  assert.equal(
    caseLine({ label: "A", error: "", got: "4" }, 0, []),
    "FAIL: A (got 4)",
  );
});

test("a required case that fails keeps its mark", () => {
  assert.equal(caseLine({ label: "A" }, 1, [1]), "FAIL: A (required)");
});
