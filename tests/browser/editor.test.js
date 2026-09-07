import { test } from "node:test";
import assert from "node:assert/strict";

import { indentSelection } from "../../web/editor.js";

test("Tab inserts four spaces on the current line or selected block", () => {
  assert.deepEqual(indentSelection("abc", 0, 0), { value: "    abc", start: 4, end: 4 });
  assert.deepEqual(indentSelection("abc", 1, 1), { value: "a    bc", start: 5, end: 5 });
  assert.deepEqual(indentSelection("one\ntwo", 1, 7), { value: "    one\n    two", start: 5, end: 15 });
  assert.deepEqual(indentSelection("abc", 1, 2), { value: "    abc", start: 5, end: 6 });
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
    { value: "    \n    one\n    two", start: 4, end: 20 });
});

// A blank line's only character is its own newline, so a bound derived by
// backing off a trailing newline could not tell "the selection stops before
// the next line" apart from "the last selected line is blank", and dropped it.
test("a selected blank line is indented like any other", () => {
  assert.deepEqual(indentSelection("one\n\ntwo", 4, 5), { value: "one\n    \ntwo", start: 8, end: 9 });
  assert.deepEqual(indentSelection("a\n\nb", 0, 3), { value: "    a\n    \nb", start: 4, end: 11 });
});

// Tab past the indentation types an indent where the caret sits. Tab still
// inside it is asking for another level on the line, so it lands at column 0:
// anywhere else and the level is off by whatever column the caret held, and on
// a line pasted with tabs the spaces would land in among them.
test("Tab inside a line's indentation indents the line, not the caret", () => {
  assert.deepEqual(indentSelection("    foo", 2, 2), { value: "        foo", start: 6, end: 6 });
  assert.deepEqual(indentSelection("\tfoo", 1, 1), { value: "    \tfoo", start: 5, end: 5 });
  // The contrast the rule is stated against, and the reason this line is also
  // asserted in the first test: a caret past the indentation types where it
  // sits rather than at column 0.
  assert.deepEqual(indentSelection("abc", 1, 1), { value: "a    bc", start: 5, end: 5 });
});

// Tab then Shift+Tab must land back where it started, or a candidate correcting
// an over-indent silently loses a level. A caret past the indentation is left
// out: there Tab types a character, and Shift+Tab is not the inverse of typing.
test("Tab then Shift+Tab round-trips every indentation style", () => {
  for (const value of ["foo", "    foo", "\tfoo", "  foo", "\t  foo", "one\n\ttwo", "\n  one"]) {
    for (const [start, end] of [[0, 0], [0, value.length]]) {
      const indented = indentSelection(value, start, end, false);
      assert.deepEqual(indentSelection(indented.value, indented.start, indented.end, true),
        { value, start, end }, `round trip failed for ${JSON.stringify(value)} at ${start}..${end}`);
    }
  }
});
