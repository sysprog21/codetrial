import { test } from "node:test";
import assert from "node:assert/strict";

import { indentNewline, indentSelection } from "../../web/editor.js";

test("Enter preserves the current line's spaces and tabs", () => {
  for (const indentation of ["", "  ", "    ", "\t", "\t  "]) {
    const value = `previous\n${indentation}call();`;
    const expected = `${value}\n${indentation}`;
    assert.deepEqual(
      indentNewline(value, value.length, value.length, "javascript"),
      { value: expected, start: expected.length, end: expected.length }
    );
  }
});

test("Enter adds a level after opening delimiters and Python colons", () => {
  for (const [value, language] of [
    ["    if (ready) {", "javascript"],
    ["    items = [", "python"],
    ["    call(", "java"],
    ["    if ready:", "python"],
    ["    if (ready) {  ", "cpp"],
  ]) {
    const expected = `${value}\n        `;
    assert.deepEqual(
      indentNewline(value, value.length, value.length, language),
      { value: expected, start: expected.length, end: expected.length }
    );
  }
  assert.deepEqual(
    indentNewline("    case 1:", 11, 11, "javascript"),
    { value: "    case 1:\n    ", start: 16, end: 16 }
  );
});

test("Enter does not add a level after comment-only lines", () => {
  for (const [value, language] of [
    ["    # Steps:", "python"],
    ["    // setup {", "javascript"],
    ["    /* setup {", "cpp"],
  ]) {
    const expected = `${value}\n    `;
    assert.deepEqual(
      indentNewline(value, value.length, value.length, language),
      { value: expected, start: expected.length, end: expected.length }
    );
  }
});

test("Enter does not treat C++ preprocessor directives as comments", () => {
  const value = "    #define BLOCK {";
  const expected = `${value}\n        `;

  assert.deepEqual(
    indentNewline(value, value.length, value.length, "cpp"),
    { value: expected, start: expected.length, end: expected.length }
  );
});

test("Enter ignores comment delimiters but keeps the code before and after comments", () => {
  for (const [line, language, nested] of [
    ["work(); // {", "javascript", false],
    ["value = 1 # note:", "python", false],
    ["if ready: # explanation", "python", true],
    ["if total // count: # integer division", "python", true],
    ["if (ready) { // explanation", "javascript", true],
    ["/* seed */ if (ready) {", "cpp", true],
    ["if (ready) { /* explanation */", "java", true],
    ["call(); /* { */", "c", false],
    ["/* first */ /* second */", "cpp", false],
    ["/* first */ // {", "cpp", false],
    ["/* first */ /* second {", "cpp", false],
    ['text = "# :"', "python", false],
    ['const text = "// {"', "javascript", false],
    ["const text = `/* {`", "javascript", false],
  ]) {
    const value = `    ${line}`;
    const expected = `${value}\n${nested ? "        " : "    "}`;
    assert.deepEqual(
      indentNewline(value, value.length, value.length, language),
      { value: expected, start: expected.length, end: expected.length }, line
    );
  }
});

test("Enter keeps comment markers inside quoted strings", () => {
  for (const [line, language] of [
    ['if url == "https://example.com": # note', "python"],
    ["if tag == '#': # note", "python"],
    ['if (url === "https://example.com") { // note', "javascript"],
    ["if (tag === '/*') { /* note */", "javascript"],
    ['if (tag === `//`) { // note', "javascript"],
    [String.raw`if tag == 'can\'t #': # note`, "python"],
    [String.raw`if (tag === "\"//") { // note`, "javascript"],
    [String.raw`if path == "C:\\": # note`, "python"],
  ]) {
    const value = `    ${line}`;
    const expected = `${value}\n        `;
    assert.deepEqual(
      indentNewline(value, value.length, value.length, language),
      { value: expected, start: expected.length, end: expected.length }, line
    );
  }
});

test("Enter ignores a matching delimiter inside a trailing comment", () => {
  const value = "    work(); // {}";
  const start = value.length - 1;
  assert.deepEqual(indentNewline(value, start, start, "javascript"),
    { value: "    work(); // {\n    }", start: start + 5, end: start + 5 });
});

test("Enter between matching delimiters leaves the caret on the inner line", () => {
  for (const [open, close] of [["{", "}"], ["[", "]"], ["(", ")"]]) {
    assert.deepEqual(
      indentNewline(`    ${open}  ${close}`, 5, 5, "javascript"),
      { value: `    ${open}\n        \n    ${close}`, start: 14, end: 14 }
    );
  }
  assert.deepEqual(
    indentNewline("{\n}", 1, 1, "javascript"),
    { value: "{\n    \n}", start: 6, end: 6 }
  );
  assert.deepEqual(
    indentNewline("{]", 1, 1, "javascript"),
    { value: "{\n    ]", start: 6, end: 6 }
  );
});

test("Enter replaces a selection and preserves text on both sides", () => {
  assert.deepEqual(
    indentNewline("    beforeREMOVEafter", 10, 16, "javascript"),
    { value: "    before\n    after", start: 15, end: 15 }
  );
  assert.deepEqual(
    indentNewline("    a\n  b", 5, 9, "javascript"),
    { value: "    a\n    ", start: 10, end: 10 }
  );
  assert.deepEqual(
    indentNewline("    abc", 2, 2, "javascript"),
    { value: "  \n    abc", start: 5, end: 5 }
  );
  assert.deepEqual(
    indentNewline("\n    abc", 0, 0, "javascript"),
    { value: "\n\n    abc", start: 1, end: 1 }
  );
  assert.deepEqual(
    indentNewline("", 0, 0, "python"),
    { value: "\n", start: 1, end: 1 }
  );
});

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
