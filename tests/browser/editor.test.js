import { test } from "node:test";
import assert from "node:assert/strict";

import { indentSelection } from "../../web/editor.js";

test("Tab indents the current line or selected block", () => {
  assert.deepEqual(indentSelection("abc", 0, 0), { value: "\tabc", start: 1, end: 1 });
  assert.deepEqual(indentSelection("abc", 1, 1), { value: "a\tbc", start: 2, end: 2 });
  assert.deepEqual(indentSelection("one\ntwo", 1, 7), { value: "\tone\n\ttwo", start: 2, end: 9 });
  assert.deepEqual(indentSelection("abc", 1, 2), { value: "\tabc", start: 2, end: 3 });
});

test("Shift+Tab removes one indentation level", () => {
  assert.deepEqual(indentSelection("\tone\n    two", 1, 12, true), { value: "one\ntwo", start: 0, end: 7 });
  assert.deepEqual(indentSelection("    foo", 7, 7, true), { value: "foo", start: 3, end: 3 });
  assert.deepEqual(indentSelection("    foo", 2, 2, true), { value: "foo", start: 0, end: 0 });
});

// The mapped position of a caret inside removed indentation has to carry the
// earlier lines' deltas with it, or the selection drifts by whatever was
// stripped above it.
test("Shift+Tab maps a position that lands inside removed indentation", () => {
  assert.deepEqual(indentSelection("\tone\n    two", 0, 7, true), { value: "one\ntwo", start: 0, end: 4 });
  assert.deepEqual(indentSelection("\tone\n\ttwo\n    three", 1, 12, true),
    { value: "one\ntwo\nthree", start: 0, end: 8 });
});

// `lastIndexOf("\n", -1)` clamps its start to 0 and still matches there, so a
// document opening with a blank line used to resolve position 0 onto the second
// line: Shift+Tab at the top stripped the indent off a line nobody selected.
test("a document that opens with a blank line indents from the first line", () => {
  assert.deepEqual(indentSelection("\n    two", 0, 0, true), { value: "\n    two", start: 0, end: 0 });
  assert.deepEqual(indentSelection("\none\ntwo", 0, 8, false),
    { value: "\t\n\tone\n\ttwo", start: 1, end: 11 });
});
