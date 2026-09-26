import { test } from "node:test";
import assert from "node:assert/strict";

import {
  indentNewline,
  indentSelection,
  insertBracketPair,
  isBracketOpenerKeystroke,
  isBracketCloserKeystroke,
  typeOverCloser,
  isBackspaceKeystroke,
  deleteEmptyPair,
} from "../../web/editor.js";

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

test("Enter carries block comments across lines in C-like languages", () => {
  for (const language of ["javascript", "c", "cpp", "java"]) {
    for (const before of [
      "/*\n * setup {",
      "/*\n\n    [",
      "call(); /* note\n    (",
      "/*\n    ' \" ` // /* {",
      "/*\r\n\tsetup {",
    ]) {
      const value = `${before}\n */`;
      const indentation = before.slice(before.lastIndexOf("\n") + 1).match(/^[ \t]*/)[0];
      const insertion = `\n${indentation}`;
      assert.deepEqual(indentNewline(value, before.length, before.length, language), {
        value: `${before}${insertion}\n */`,
        start: before.length + insertion.length,
        end: before.length + insertion.length,
      }, `${language}: ${before}`);
    }
  }
});

test("Enter resumes code scanning after a cross-line block comment closes", () => {
  for (const language of ["javascript", "c", "cpp", "java"]) {
    for (const [line, nested] of [
      ["*/", false],
      ["*/ if (ready) {", true],
      ["*/ call();", false],
      ["*/ if (ready) { /* trailing */", true],
      ["*/ // {", false],
      ["*/ /* another\n    setup {", false],
      ["*/ /* another */ if (ready) {", true],
      ["*/\n    if (ready) {", true],
    ]) {
      const value = `/* ignored {\n    ${line}`;
      const expected = `${value}\n${nested ? "        " : "    "}`;
      assert.deepEqual(indentNewline(value, value.length, value.length, language),
        { value: expected, start: expected.length, end: expected.length }, `${language}: ${line}`);
    }
  }
});

test("Enter carries only an open block comment into the next line", () => {
  for (const [name, prefix, language] of [
    ["code opener", "if (ready) {", "javascript"],
    ["block marker in a line comment", "// /*", "c"],
    ["block marker in a quoted string", 'const char *text = "/*";', "cpp"],
    ["block marker in a multiline template string", "const text = `\n/*\n`;", "javascript"],
    ["block marker in a JavaScript regex", String.raw`const re = /\/*/;`, "javascript"],
    ["block marker in a C++ raw string", 'auto text = R"tag(\n/*\n)tag";', "cpp"],
    ["block marker in a Java text block", 'String text = """\n/*\n""";', "java"],
    ["block marker in a Python triple-quoted string", 'text = """\n/*\n"""', "python"],
    ["closed block comment", "/* closed */", "java"],
    ["Python block marker", "/*", "python"],
  ]) {
    const value = `${prefix}\n    `;
    const unchanged = `${value}\n    `;
    assert.deepEqual(indentNewline(value, value.length, value.length, language),
      { value: unchanged, start: unchanged.length, end: unchanged.length }, name);
    const code = `${value}${language === "python" ? "if ready:" : "if (ready) {"}`;
    const nested = `${code}\n        `;
    assert.deepEqual(indentNewline(code, code.length, code.length, language),
      { value: nested, start: nested.length, end: nested.length }, `${name}: current line`);
  }
});

test("Enter resumes indentation after division, regex, and numeric separators", () => {
  for (const [language, before] of [
    ["javascript", "const n = object.return / 2;"],
    ["javascript", "const n = object.if(ready) / 2;"],
    ["javascript", "const n = π / 2;"],
    ["javascript", String.raw`if (ready) {} /\/*/.test(text);`],
    ["javascript", "const object = {} / 2;"],
    ["cpp", "auto n = 1'000;"],
  ]) {
    const value = `${before}\n    if (ready) {`;
    const expected = `${value}\n        `;
    assert.deepEqual(indentNewline(value, value.length, value.length, language),
      { value: expected, start: expected.length, end: expected.length }, value);
  }
  const continued = 'const s = "a\\\r\n/*\\\r\n";\r\n    if (ready) {';
  const expected = `${continued}\n        `;
  assert.deepEqual(indentNewline(continued, continued.length, continued.length, "javascript"),
    { value: expected, start: expected.length, end: expected.length });
});

test("Enter does not add indentation when a string or regex follows an opening parenthesis", () => {
  for (const [language, value] of [
    ["javascript", 'call("text"'],
    ["javascript", String.raw`call(/\/*/`],
    ["python", 'call("text"'],
  ]) {
    const expected = `${value}\n`;
    assert.deepEqual(indentNewline(value, value.length, value.length, language),
      { value: expected, start: expected.length, end: expected.length }, value);
  }
});

test("Enter uses only the prefix before a selection for comment state", () => {
  const value = "/*\n    setup {}\n*/";
  const start = value.indexOf("}");
  assert.deepEqual(indentNewline(value, start, start, "javascript"),
    { value: "/*\n    setup {\n    }\n*/", start: start + 5, end: start + 5 });
  assert.deepEqual(indentNewline(value, start, value.length, "javascript"),
    { value: "/*\n    setup {\n    ", start: start + 5, end: start + 5 });
  const code = "/*\n */ if (ready) {}";
  const caret = code.indexOf("}");
  assert.deepEqual(indentNewline(code, caret, caret, "cpp"),
    { value: "/*\n */ if (ready) {\n     \n }", start: caret + 6, end: caret + 6 });
});

test("Enter derives block comment state from the current value and language", () => {
  for (const [name, prefix, language, spaces] of [
    ["open JavaScript block comment", "/*", "javascript", 4],
    ["JavaScript line comment", "//", "javascript", 8],
    ["Python block marker", "/*", "python", 8],
    ["open C++ block comment", "/*", "cpp", 4],
    ["closed C++ block comment", "/* */", "cpp", 8],
  ]) {
    const value = `${prefix}\n    setup {`;
    const expected = `${value}\n${" ".repeat(spaces)}`;
    assert.deepEqual(indentNewline(value, value.length, value.length, language),
      { value: expected, start: expected.length, end: expected.length }, name);
  }
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

test("typing an opening bracket at a bare caret inserts the matching closer and steps inside", () => {
  for (const [open, close] of [["(", ")"], ["{", "}"], ["[", "]"]]) {
    assert.deepEqual(
      insertBracketPair("call", 4, 4, open),
      { value: `call${open}${close}`, start: 5, end: 5 }
    );
    assert.deepEqual(
      insertBracketPair(`before${open}${close}after`, 7, 7, open),
      { value: `before${open}${open}${close}${close}after`, start: 8, end: 8 }
    );
  }
});

test("typing an opening bracket around a selection wraps it and leaves the wrapped text selected", () => {
  for (const [open, close] of [["(", ")"], ["{", "}"], ["[", "]"]]) {
    assert.deepEqual(
      insertBracketPair(`return value;`, 7, 12, open),
      { value: `return ${open}value${close};`, start: 8, end: 13 }
    );
  }
});

test("typing an opening bracket in the middle of existing text only touches the selection", () => {
  assert.deepEqual(
    insertBracketPair("foobar", 3, 3, "("),
    { value: "foo()bar", start: 4, end: 4 }
  );
  assert.deepEqual(
    insertBracketPair("one, two", 5, 8, "["),
    { value: "one, [two]", start: 6, end: 9 }
  );
});

test("an opening-bracket beforeinput is a bracket keystroke", () => {
  for (const opener of ["(", "{", "["]) {
    assert.equal(isBracketOpenerKeystroke({ inputType: "insertText", data: opener }), true);
  }
});

// applyEditorEdit hands execCommand the changed span -- "()" at a bare caret,
// "(value)" around a selection -- never a bare opener, so this excludes
// execCommand's own synthetic beforeinput on content alone.
test("a beforeinput that is not plain bracket text is not a bracket keystroke", () => {
  assert.equal(isBracketOpenerKeystroke({ inputType: "insertLineBreak", data: null }), false);
  assert.equal(isBracketOpenerKeystroke({ inputType: "insertText", data: "a" }), false);
  assert.equal(isBracketOpenerKeystroke({ inputType: "insertText", data: "(x" }), false);
  assert.equal(isBracketOpenerKeystroke({ inputType: "insertText", data: "()" }), false);
  assert.equal(isBracketOpenerKeystroke({ inputType: "insertText", data: "(value)" }), false);
});

test("a closing bracket is a bracket closer keystroke, anything else is not", () => {
  for (const closer of [")", "]", "}"]) {
    assert.equal(isBracketCloserKeystroke({ inputType: "insertText", data: closer }), true);
  }
  assert.equal(isBracketCloserKeystroke({ inputType: "insertText", data: "a" }), false);
  assert.equal(isBracketCloserKeystroke({ inputType: "deleteContentBackward", data: ")" }), false);
});

test("typing a closer right before the same character steps over it instead of inserting", () => {
  for (const [open, close] of [["(", ")"], ["{", "}"], ["[", "]"]]) {
    assert.deepEqual(
      typeOverCloser(`foo${open}bar${close}`, 7, 7, close),
      { value: `foo${open}bar${close}`, start: 8, end: 8 }
    );
  }
});

test("typing a closer with no matching character ahead falls through to native insertion", () => {
  assert.equal(typeOverCloser("foo(bar", 4, 4, ")"), null);
  assert.equal(typeOverCloser("foo)bar", 3, 3, "]"), null);
  // A selection is never a caret sitting before the closer, so it also falls
  // through rather than swallowing whatever was selected.
  assert.equal(typeOverCloser("foo()bar", 4, 5, ")"), null);
});

test("Backspace is a backspace keystroke, other deletions are not", () => {
  assert.equal(isBackspaceKeystroke({ inputType: "deleteContentBackward" }), true);
  assert.equal(isBackspaceKeystroke({ inputType: "deleteContentForward" }), false);
});

test("Backspace between a matching empty pair removes both characters", () => {
  for (const [open, close] of [["(", ")"], ["{", "}"], ["[", "]"]]) {
    assert.deepEqual(
      deleteEmptyPair(`foo${open}${close}bar`, 4, 4, close),
      { value: "foobar", start: 3, end: 3 }
    );
  }
});

test("Backspace elsewhere, or between a mismatched pair, falls through to native deletion", () => {
  assert.equal(deleteEmptyPair("foobar", 3, 3), null);
  assert.equal(deleteEmptyPair("foo(]bar", 4, 4), null);
  assert.equal(deleteEmptyPair("foo(x)bar", 4, 4), null);
  assert.equal(deleteEmptyPair("(", 1, 1), null);
  // A selection is not a collapsed caret between the pair, so it also falls
  // through rather than deleting past what was selected.
  assert.equal(deleteEmptyPair("foo()bar", 3, 4), null);
});
