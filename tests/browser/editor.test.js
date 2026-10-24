import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

import {
  matchingBrackets,
  moveSelectedText,
  indentNewline,
  indentSelection,
  insertBracketPair,
  isBracketOpenerKeystroke,
  isBracketCloserKeystroke,
  typeOverCloser,
  isBackspaceKeystroke,
  deleteEmptyPair,
  isToggleCommentKeystroke,
  toggleLineComment,
} from "../../web/editor.js";
import { createSyntaxParser } from "../../web/syntax-parser.js";
import { tokenize } from "../../web/tokenizer.js";

test("Enter keeps working for an unsupported language", () => {
  for (const language of ["unknown", "constructor", "toString", ""]) {
    for (const [value, start, end, expected, caret] of [
      ["", 0, 0, "\n", 1],
      ["    text", 8, 8, "    text\n    ", 13],
      ["    {}", 5, 5, "    {\n        \n    }", 14],
      ["    beforeREMOVEafter", 10, 16, "    before\n    after", 15],
    ]) {
      assert.deepEqual(
        indentNewline(value, start, end, language),
        {
          value: expected,
          start: caret,
          end: caret,
        },
        language,
      );
    }
  }
});

test("Enter preserves the current line's spaces and tabs", () => {
  for (const indentation of ["", "  ", "    ", "\t", "\t  "]) {
    const value = `previous\n${indentation}call();`;
    const expected = `${value}\n${indentation}`;
    assert.deepEqual(
      indentNewline(value, value.length, value.length, "javascript"),
      { value: expected, start: expected.length, end: expected.length },
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
      { value: expected, start: expected.length, end: expected.length },
    );
  }
  assert.deepEqual(indentNewline("    case 1:", 11, 11, "javascript"), {
    value: "    case 1:\n    ",
    start: 16,
    end: 16,
  });
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
      { value: expected, start: expected.length, end: expected.length },
    );
  }
});

test("Enter does not treat C++ preprocessor directives as comments", () => {
  const value = "    #define BLOCK {";
  const expected = `${value}\n        `;

  assert.deepEqual(indentNewline(value, value.length, value.length, "cpp"), {
    value: expected,
    start: expected.length,
    end: expected.length,
  });
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
      { value: expected, start: expected.length, end: expected.length },
      line,
    );
  }
});

test("Enter keeps comment markers inside quoted strings", () => {
  for (const [line, language] of [
    ['if url == "https://example.com": # note', "python"],
    ["if tag == '#': # note", "python"],
    ['if (url === "https://example.com") { // note', "javascript"],
    ["if (tag === '/*') { /* note */", "javascript"],
    ["if (tag === `//`) { // note", "javascript"],
    [String.raw`if tag == 'can\'t #': # note`, "python"],
    [String.raw`if (tag === "\"//") { // note`, "javascript"],
    [String.raw`if path == "C:\\": # note`, "python"],
  ]) {
    const value = `    ${line}`;
    const expected = `${value}\n        `;
    assert.deepEqual(
      indentNewline(value, value.length, value.length, language),
      { value: expected, start: expected.length, end: expected.length },
      line,
    );
  }
});

test("Enter ignores a matching delimiter inside a trailing comment", () => {
  const value = "    work(); // {}";
  const start = value.length - 1;
  assert.deepEqual(indentNewline(value, start, start, "javascript"), {
    value: "    work(); // {\n    }",
    start: start + 5,
    end: start + 5,
  });
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
      const indentation = before
        .slice(before.lastIndexOf("\n") + 1)
        .match(/^[ \t]*/)[0];
      const insertion = `\n${indentation}`;
      assert.deepEqual(
        indentNewline(value, before.length, before.length, language),
        {
          value: `${before}${insertion}\n */`,
          start: before.length + insertion.length,
          end: before.length + insertion.length,
        },
        `${language}: ${before}`,
      );
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
      assert.deepEqual(
        indentNewline(value, value.length, value.length, language),
        { value: expected, start: expected.length, end: expected.length },
        `${language}: ${line}`,
      );
    }
  }
});

test("Enter carries only an open block comment into the next line", () => {
  for (const [name, prefix, language] of [
    ["code opener", "if (ready) {", "javascript"],
    ["block marker in a line comment", "// /*", "c"],
    ["block marker in a quoted string", 'const char *text = "/*";', "cpp"],
    [
      "block marker in a multiline template string",
      "const text = `\n/*\n`;",
      "javascript",
    ],
    [
      "block marker in a JavaScript regex",
      String.raw`const re = /\/*/;`,
      "javascript",
    ],
    [
      "block marker in a C++ raw string",
      'auto text = R"tag(\n/*\n)tag";',
      "cpp",
    ],
    [
      "block marker in a Java text block",
      'String text = """\n/*\n""";',
      "java",
    ],
    [
      "block marker in a Python triple-quoted string",
      'text = """\n/*\n"""',
      "python",
    ],
    ["closed block comment", "/* closed */", "java"],
    ["Python block marker", "/*", "python"],
  ]) {
    const value = `${prefix}\n    `;
    const unchanged = `${value}\n    `;
    assert.deepEqual(
      indentNewline(value, value.length, value.length, language),
      { value: unchanged, start: unchanged.length, end: unchanged.length },
      name,
    );
    const code = `${value}${language === "python" ? "if ready:" : "if (ready) {"}`;
    const nested = `${code}\n        `;
    assert.deepEqual(
      indentNewline(code, code.length, code.length, language),
      { value: nested, start: nested.length, end: nested.length },
      `${name}: current line`,
    );
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
    assert.deepEqual(
      indentNewline(value, value.length, value.length, language),
      { value: expected, start: expected.length, end: expected.length },
      value,
    );
  }
  const continued = 'const s = "a\\\r\n/*\\\r\n";\r\n    if (ready) {';
  const expected = `${continued}\n        `;
  assert.deepEqual(
    indentNewline(continued, continued.length, continued.length, "javascript"),
    { value: expected, start: expected.length, end: expected.length },
  );
});

test("Enter uses parsed JavaScript regions before an unfinished block", async () => {
  const syntax = createSyntaxParser({
    loadBytes: async (url) => new Uint8Array(await readFile(url)),
  });
  await syntax.prepareLanguage("javascript");
  for (const prefix of [
    "class A { #return = 4; f() { return this.#return / 2; } }",
    String.raw`function f() {} /\/*/.test(value);`,
    String.raw`class A {} /\/*/.test(value);`,
    String.raw`f(.../\/*/.test(value));`,
  ]) {
    const value = `${prefix}\n    if (ready) {`;
    const expected = `${value}\n        `;
    assert.deepEqual(
      indentNewline(
        value,
        value.length,
        value.length,
        "javascript",
        syntax.parseSyntax,
      ),
      { value: expected, start: expected.length, end: expected.length },
      prefix,
    );
  }
});

test("Enter preserves lexical state in unfinished code with a ready parser", async () => {
  const syntax = createSyntaxParser({
    loadBytes: async (url) => new Uint8Array(await readFile(url)),
  });
  await Promise.all(
    ["c", "cpp", "java", "javascript", "python"].map((language) =>
      syntax.prepareLanguage(language),
    ),
  );
  for (const [language, value, indentation] of [
    ["javascript", "/*\n * setup {", " "],
    ["javascript", "const text = `\n/*\n  {", "  "],
    ["javascript", "const re = /[/*\n  if (ready) {", "      "],
    ["python", 'text = """\n#:\n  if ready:', "  "],
    ["c", "/*\n * setup {", " "],
    ["cpp", 'auto text = R"tag(\n/*\n  {', "  "],
    ["java", 'String text = """\n/*\n  {', "  "],
  ]) {
    const tree = syntax.parseSyntax(value, language);
    try {
      assert.equal(tree.rootNode.hasError, true, `${language}: ${value}`);
    } finally {
      tree.delete();
    }
    const expected = `${value}\n${indentation}`;
    assert.deepEqual(
      indentNewline(
        value,
        value.length,
        value.length,
        language,
        syntax.parseSyntax,
      ),
      { value: expected, start: expected.length, end: expected.length },
      `${language}: ${value}`,
    );
  }
});

test("Enter does not add indentation when a string or regex follows an opening parenthesis", () => {
  for (const [language, value] of [
    ["javascript", 'call("text"'],
    ["javascript", String.raw`call(/\/*/`],
    ["python", 'call("text"'],
  ]) {
    const expected = `${value}\n`;
    assert.deepEqual(
      indentNewline(value, value.length, value.length, language),
      { value: expected, start: expected.length, end: expected.length },
      value,
    );
  }
});

test("Enter uses only the prefix before a selection for comment state", () => {
  const value = "/*\n    setup {}\n*/";
  const start = value.indexOf("}");
  assert.deepEqual(indentNewline(value, start, start, "javascript"), {
    value: "/*\n    setup {\n    }\n*/",
    start: start + 5,
    end: start + 5,
  });
  assert.deepEqual(indentNewline(value, start, value.length, "javascript"), {
    value: "/*\n    setup {\n    ",
    start: start + 5,
    end: start + 5,
  });
  const code = "/*\n */ if (ready) {}";
  const caret = code.indexOf("}");
  assert.deepEqual(indentNewline(code, caret, caret, "cpp"), {
    value: "/*\n */ if (ready) {\n     \n }",
    start: caret + 6,
    end: caret + 6,
  });
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
    assert.deepEqual(
      indentNewline(value, value.length, value.length, language),
      { value: expected, start: expected.length, end: expected.length },
      name,
    );
  }
});

test("Enter between matching delimiters leaves the caret on the inner line", () => {
  for (const [open, close] of [
    ["{", "}"],
    ["[", "]"],
    ["(", ")"],
  ]) {
    assert.deepEqual(
      indentNewline(`    ${open}  ${close}`, 5, 5, "javascript"),
      { value: `    ${open}\n        \n    ${close}`, start: 14, end: 14 },
    );
  }
  assert.deepEqual(indentNewline("{\n}", 1, 1, "javascript"), {
    value: "{\n    \n}",
    start: 6,
    end: 6,
  });
  assert.deepEqual(indentNewline("{]", 1, 1, "javascript"), {
    value: "{\n    ]",
    start: 6,
    end: 6,
  });
});

test("Enter replaces a selection and preserves text on both sides", () => {
  assert.deepEqual(
    indentNewline("    beforeREMOVEafter", 10, 16, "javascript"),
    { value: "    before\n    after", start: 15, end: 15 },
  );
  assert.deepEqual(indentNewline("    a\n  b", 5, 9, "javascript"), {
    value: "    a\n    ",
    start: 10,
    end: 10,
  });
  assert.deepEqual(indentNewline("    abc", 2, 2, "javascript"), {
    value: "  \n    abc",
    start: 5,
    end: 5,
  });
  assert.deepEqual(indentNewline("\n    abc", 0, 0, "javascript"), {
    value: "\n\n    abc",
    start: 1,
    end: 1,
  });
  assert.deepEqual(indentNewline("", 0, 0, "python"), {
    value: "\n",
    start: 1,
    end: 1,
  });
});

test("Tab inserts four spaces on the current line or selected block", () => {
  assert.deepEqual(indentSelection("abc", 0, 0), {
    value: "    abc",
    start: 4,
    end: 4,
  });
  assert.deepEqual(indentSelection("abc", 1, 1), {
    value: "a    bc",
    start: 5,
    end: 5,
  });
  assert.deepEqual(indentSelection("one\ntwo", 1, 7), {
    value: "    one\n    two",
    start: 5,
    end: 15,
  });
  assert.deepEqual(indentSelection("abc", 1, 2), {
    value: "    abc",
    start: 5,
    end: 6,
  });
});

test("Shift+Tab removes one indentation level", () => {
  assert.deepEqual(indentSelection("\tone\n    two", 1, 12, true), {
    value: "one\ntwo",
    start: 0,
    end: 7,
  });
  assert.deepEqual(indentSelection("    foo", 7, 7, true), {
    value: "foo",
    start: 3,
    end: 3,
  });
  assert.deepEqual(indentSelection("    foo", 2, 2, true), {
    value: "foo",
    start: 0,
    end: 0,
  });
});

// The mapped position of a caret inside removed indentation has to carry the
// earlier lines' deltas with it, or the selection drifts by whatever was
// stripped above it.
test("Shift+Tab maps a position that lands inside removed indentation", () => {
  assert.deepEqual(indentSelection("\tone\n    two", 0, 7, true), {
    value: "one\ntwo",
    start: 0,
    end: 4,
  });
  assert.deepEqual(indentSelection("\tone\n\ttwo\n    three", 1, 12, true), {
    value: "one\ntwo\nthree",
    start: 0,
    end: 8,
  });
});

// `lastIndexOf("\n", -1)` clamps its start to 0 and still matches there, so a
// document opening with a blank line used to resolve position 0 onto the second
// line: Shift+Tab at the top stripped the indent off a line nobody selected.
test("a document that opens with a blank line indents from the first line", () => {
  assert.deepEqual(indentSelection("\n    two", 0, 0, true), {
    value: "\n    two",
    start: 0,
    end: 0,
  });
  assert.deepEqual(indentSelection("\none\ntwo", 0, 8, false), {
    value: "    \n    one\n    two",
    start: 4,
    end: 20,
  });
});

// A blank line's only character is its own newline, so a bound derived by
// backing off a trailing newline could not tell "the selection stops before
// the next line" apart from "the last selected line is blank", and dropped it.
test("a selected blank line is indented like any other", () => {
  assert.deepEqual(indentSelection("one\n\ntwo", 4, 5), {
    value: "one\n    \ntwo",
    start: 8,
    end: 9,
  });
  assert.deepEqual(indentSelection("a\n\nb", 0, 3), {
    value: "    a\n    \nb",
    start: 4,
    end: 11,
  });
});

// Tab past the indentation types an indent where the caret sits. Tab still
// inside it is asking for another level on the line, so it lands at column 0:
// anywhere else and the level is off by whatever column the caret held, and on
// a line pasted with tabs the spaces would land in among them.
test("Tab inside a line's indentation indents the line, not the caret", () => {
  assert.deepEqual(indentSelection("    foo", 2, 2), {
    value: "        foo",
    start: 6,
    end: 6,
  });
  assert.deepEqual(indentSelection("\tfoo", 1, 1), {
    value: "    \tfoo",
    start: 5,
    end: 5,
  });
  // The contrast the rule is stated against, and the reason this line is also
  // asserted in the first test: a caret past the indentation types where it
  // sits rather than at column 0.
  assert.deepEqual(indentSelection("abc", 1, 1), {
    value: "a    bc",
    start: 5,
    end: 5,
  });
});

// Tab then Shift+Tab must land back where it started, or a candidate correcting
// an over-indent silently loses a level. A caret past the indentation is left
// out: there Tab types a character, and Shift+Tab is not the inverse of typing.
test("Tab then Shift+Tab round-trips every indentation style", () => {
  for (const value of [
    "foo",
    "    foo",
    "\tfoo",
    "  foo",
    "\t  foo",
    "one\n\ttwo",
    "\n  one",
  ]) {
    for (const [start, end] of [
      [0, 0],
      [0, value.length],
    ]) {
      const indented = indentSelection(value, start, end, false);
      assert.deepEqual(
        indentSelection(indented.value, indented.start, indented.end, true),
        { value, start, end },
        `round trip failed for ${JSON.stringify(value)} at ${start}..${end}`,
      );
    }
  }
});

test("typing an opening bracket at a bare caret inserts the matching closer and steps inside", () => {
  for (const [open, close] of [
    ["(", ")"],
    ["{", "}"],
    ["[", "]"],
  ]) {
    assert.deepEqual(insertBracketPair("call", 4, 4, open), {
      value: `call${open}${close}`,
      start: 5,
      end: 5,
    });
    assert.deepEqual(
      insertBracketPair(`before${open}${close}after`, 7, 7, open),
      { value: `before${open}${open}${close}${close}after`, start: 8, end: 8 },
    );
  }
});

test("typing an opening bracket around a selection wraps it and leaves the wrapped text selected", () => {
  for (const [open, close] of [
    ["(", ")"],
    ["{", "}"],
    ["[", "]"],
  ]) {
    assert.deepEqual(insertBracketPair(`return value;`, 7, 12, open), {
      value: `return ${open}value${close};`,
      start: 8,
      end: 13,
    });
  }
});

test("typing an opening bracket right before other text leaves the closer to the candidate", () => {
  // The reproduction from issue 150: an opener typed in front of `a+b)` must
  // not strand the `)` the candidate already typed behind a second one.
  for (const open of ["(", "{", "["]) {
    assert.equal(insertBracketPair("a+b)", 0, 0, open), null);
    assert.equal(insertBracketPair("foobar", 3, 3, open), null);
  }
  // Whitespace, a closer, or punctuation that ends an expression still gets
  // the pair, at the end of a line as well as the end of the buffer.
  for (const [value, at] of [
    ["x = ;", 4],
    ["f(a, b)", 6],
    ["call\nnext", 4],
    ["call next", 4],
    ["if (x) ", 7],
  ]) {
    assert.deepEqual(insertBracketPair(value, at, at, "("), {
      value: `${value.slice(0, at)}()${value.slice(at)}`,
      start: at + 1,
      end: at + 1,
    });
  }
});

test("typing an opening bracket in the middle of existing text only touches the selection", () => {
  assert.deepEqual(insertBracketPair("one, two", 5, 8, "["), {
    value: "one, [two]",
    start: 6,
    end: 9,
  });
});

test("an opening-bracket beforeinput is a bracket keystroke", () => {
  for (const opener of ["(", "{", "["]) {
    assert.equal(
      isBracketOpenerKeystroke({ inputType: "insertText", data: opener }),
      true,
    );
  }
});

// Only a single typed opener starts bracket pairing. Other input types and
// multi-character inserts must pass through unchanged.
test("a beforeinput that is not plain bracket text is not a bracket keystroke", () => {
  assert.equal(
    isBracketOpenerKeystroke({ inputType: "insertLineBreak", data: null }),
    false,
  );
  assert.equal(
    isBracketOpenerKeystroke({ inputType: "insertText", data: "a" }),
    false,
  );
  assert.equal(
    isBracketOpenerKeystroke({ inputType: "insertText", data: "(x" }),
    false,
  );
  assert.equal(
    isBracketOpenerKeystroke({ inputType: "insertText", data: "()" }),
    false,
  );
  assert.equal(
    isBracketOpenerKeystroke({ inputType: "insertText", data: "(value)" }),
    false,
  );
});

test("a closing bracket is a bracket closer keystroke, anything else is not", () => {
  for (const closer of [")", "]", "}"]) {
    assert.equal(
      isBracketCloserKeystroke({ inputType: "insertText", data: closer }),
      true,
    );
  }
  assert.equal(
    isBracketCloserKeystroke({ inputType: "insertText", data: "a" }),
    false,
  );
  assert.equal(
    isBracketCloserKeystroke({ inputType: "deleteContentBackward", data: ")" }),
    false,
  );
});

test("typing a closer right before the same character steps over it instead of inserting", () => {
  for (const [open, close] of [
    ["(", ")"],
    ["{", "}"],
    ["[", "]"],
  ]) {
    assert.deepEqual(typeOverCloser(`foo${open}bar${close}`, 7, 7, close), {
      value: `foo${open}bar${close}`,
      start: 8,
      end: 8,
    });
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
  assert.equal(
    isBackspaceKeystroke({ inputType: "deleteContentBackward" }),
    true,
  );
  assert.equal(
    isBackspaceKeystroke({ inputType: "deleteContentForward" }),
    false,
  );
});

test("Backspace between a matching empty pair removes both characters", () => {
  for (const [open, close] of [
    ["(", ")"],
    ["{", "}"],
    ["[", "]"],
  ]) {
    assert.deepEqual(deleteEmptyPair(`foo${open}${close}bar`, 4, 4, close), {
      value: "foobar",
      start: 3,
      end: 3,
    });
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

test("matching brackets finds either side and nested mixed pairs", () => {
  const code = "{call(items[0])}";
  for (const [caret, pair] of [
    [0, [0, 15]],
    [1, [0, 15]],
    [15, [0, 15]],
    [16, [0, 15]],
    [5, [5, 14]],
    [6, [5, 14]],
    [14, [5, 14]],
    [11, [11, 13]],
    [12, [11, 13]],
    [13, [11, 13]],
  ])
    assert.deepEqual(matchingBrackets(code, caret, caret, "cpp"), pair);
  assert.deepEqual(matchingBrackets(")(x)", 1, 1, "cpp"), [1, 3]);
});

test("matching brackets refuses selections, missing pairs, and crossing pairs", () => {
  for (const [code, start, end] of [
    ["(x)", 0, 2],
    ["text", 2, 2],
    ["", 0, 0],
    ["(x", 0, 0],
    ["x)", 2, 2],
    ["([)]", 0, 0],
    ["([)]", 1, 1],
  ])
    assert.equal(matchingBrackets(code, start, end, "cpp"), null);
});

test("matching brackets falls back to the left when the right has no pair", () => {
  for (const code of ["f(x)]", "f(x)(", "f(x)/*[*/", 'f(x)"["']) {
    let calls = 0;
    assert.deepEqual(
      matchingBrackets(code, 4, 4, "cpp", (value, language) => {
        calls += 1;
        return tokenize(value, language);
      }),
      [1, 3],
      code,
    );
    assert.equal(calls, 1);
  }
  assert.deepEqual(matchingBrackets("(x)[y]", 3, 3, "cpp"), [3, 5]);
});

test("stray closers preserve enclosing pairs without matching themselves", () => {
  const code = "class A { void a() { f(x)); } void b() {} }";
  const caret = code.length - 1;
  assert.deepEqual(matchingBrackets(code, caret, caret, "java"), [
    code.indexOf("{"),
    caret,
  ]);
  const stray = code.indexOf("));") + 1;
  assert.equal(matchingBrackets(code, stray + 1, stray + 1, "java"), null);
  assert.deepEqual(matchingBrackets("{]}", 2, 2, "cpp"), [0, 2]);
});

test("unrelated closers do not repeatedly scan all enclosing openers", () => {
  const code = "(".repeat(12000) + "]".repeat(12000) + ")".repeat(12000);
  let reads = 0;
  // Count character reads rather than wall time, so slower CI machines use
  // the same linear-work bound. Tokenization is outside this check.
  const value = new Proxy(Object(code), {
    get(target, key) {
      if (typeof key === "string" && /^\d+$/.test(key)) {
        reads += 1;
        assert.ok(reads <= code.length * 4, "bracket scan must stay linear");
      }
      return Reflect.get(target, key);
    },
  });
  const caret = code.length - 1;
  assert.deepEqual(
    matchingBrackets(value, caret, caret, "cpp", () => ({
      tokens: [{ kind: "code", start: 0, end: code.length }],
    })),
    [0, caret],
  );
});

test("matching brackets ignores comments, strings, and regex literals", () => {
  for (const [code, language] of [
    ['("[)]" /* } */ value)', "cpp"],
    ['(R"tag([)])tag" + value)', "cpp"],
    ["(value // )\n)", "java"],
    ["(value # )\n)", "python"],
    ['("""[)]""" + value)', "python"],
    ["(/[(]/.test(value))", "javascript"],
  ]) {
    assert.deepEqual(
      matchingBrackets(code, 0, 0, language),
      [0, code.length - 1],
      code,
    );
  }
  const code = '"()" /* [] */';
  for (const caret of [1, 2, 8, 9]) {
    assert.equal(matchingBrackets(code, caret, caret, "cpp"), null);
  }
});

test("matching brackets use parsed regions in complete and unfinished code", async () => {
  const syntax = createSyntaxParser({
    loadBytes: async (url) => new Uint8Array(await readFile(url)),
  });
  await syntax.prepareLanguage("javascript");
  for (const code of [
    "const s = `${(value)}`;",
    "const s = `${(value)}`; if (ready) {",
  ]) {
    const start = code.indexOf("(");
    assert.deepEqual(
      matchingBrackets(code, start, start, "javascript", (value, language) =>
        tokenize(value, language, syntax.parseSyntax),
      ),
      [start, code.indexOf(")")],
    );
  }
});

test("parsed Python f-strings match expressions but exclude escaped braces and text", async () => {
  const syntax = createSyntaxParser({
    loadBytes: async (url) => new Uint8Array(await readFile(url)),
  });
  await syntax.prepareLanguage("python");
  const getTokens = (value, language) =>
    tokenize(value, language, syntax.parseSyntax);
  const adjacent = 'f"{x}("';
  assert.deepEqual(
    matchingBrackets(adjacent, 5, 5, "python", getTokens),
    [2, 4],
  );
  for (const code of [
    'f"{items[0]}"',
    'f"{{{items[0]}}}"',
    'f"{items[0]:{width}}"',
    'f"""text\n{items[0]}\n"""',
  ]) {
    const start = code.indexOf("[");
    assert.deepEqual(
      matchingBrackets(code, start, start, "python", getTokens),
      [start, code.indexOf("]")],
      code,
    );
  }
  const code = 'f"{{ }} {items[0]} literal []"';
  const start = code.indexOf("{items");
  assert.deepEqual(matchingBrackets(code, start, start, "python", getTokens), [
    start,
    code.indexOf("}", start),
  ]);
  for (const caret of [2, 3, 5, 6, code.indexOf("[]")]) {
    assert.equal(
      matchingBrackets(code, caret, caret, "python", getTokens),
      null,
    );
  }
  const formatted = 'f"{value:literal[] {width}}"';
  const literal = formatted.indexOf("[]");
  assert.equal(
    matchingBrackets(formatted, literal, literal, "python", getTokens),
    null,
  );
  const width = formatted.indexOf("{width");
  assert.deepEqual(
    matchingBrackets(formatted, width, width, "python", getTokens),
    [width, formatted.indexOf("}", width)],
  );
});

test("the comment toggle writes each language's own marker", () => {
  for (const [language, marker] of [
    ["python", "#"],
    ["javascript", "//"],
    ["c", "//"],
    ["cpp", "//"],
    ["java", "//"],
  ]) {
    const width = marker.length + 1;
    assert.deepEqual(
      toggleLineComment("x = 1", 2, 2, language),
      { value: `${marker} x = 1`, start: 2 + width, end: 2 + width },
      language,
    );
  }
});

// Null is what lets the keystroke through untouched, so an inherited property
// name must not be read as a language that has a marker.
test("the comment toggle leaves an unsupported language alone", () => {
  for (const language of ["unknown", "constructor", "toString", ""])
    assert.equal(toggleLineComment("x", 0, 0, language), null, language);
});

test("a second toggle restores the line and the caret", () => {
  const commented = toggleLineComment("    return n", 8, 8, "python");
  assert.deepEqual(commented, { value: "    # return n", start: 10, end: 10 });
  assert.deepEqual(
    toggleLineComment(
      commented.value,
      commented.start,
      commented.end,
      "python",
    ),
    { value: "    return n", start: 8, end: 8 },
  );
});

// The caret cases a removal has to get right: inside the marker it collapses
// onto where the marker began, and before it nothing moves.
test("uncommenting keeps the caret off the removed marker", () => {
  assert.deepEqual(toggleLineComment("  // a", 3, 3, "c"), {
    value: "  a",
    start: 2,
    end: 2,
  });
  assert.deepEqual(toggleLineComment("  // a", 1, 1, "c"), {
    value: "  a",
    start: 1,
    end: 1,
  });
});

test("a comment typed without the space is still removed whole", () => {
  assert.deepEqual(toggleLineComment("#x\n# y", 0, 6, "python"), {
    value: "x\ny",
    start: 0,
    end: 3,
  });
});

// One column for the block, so nested code keeps its shape under the markers,
// and a blank line gets nothing: a marker there is trailing whitespace the
// candidate did not type.
test("a block is commented at its shallowest indentation", () => {
  const value = "def f():\n    a = 1\n\n        b = 2\n";
  const commented = toggleLineComment(value, 9, value.length - 1, "python");
  assert.deepEqual(commented, {
    value: "def f():\n    # a = 1\n\n    #     b = 2\n",
    start: 9,
    end: 37,
  });
  assert.deepEqual(
    toggleLineComment(
      commented.value,
      commented.start,
      commented.end,
      "python",
    ),
    { value, start: 9, end: value.length - 1 },
  );
});

// Removing the one marker here would uncomment a line the candidate had
// commented on purpose and leave the rest as they were.
test("a block with one commented line gains a level", () => {
  assert.deepEqual(toggleLineComment("  // a\n  b", 0, 10, "javascript"), {
    value: "  // // a\n  // b",
    start: 0,
    end: 16,
  });
});

// `end` is exclusive, as it is for Tab: a selection dragged down to the start
// of a line has not selected that line.
test("a line the selection only reaches the start of is not toggled", () => {
  assert.deepEqual(toggleLineComment("a\nb\nc", 0, 2, "cpp"), {
    value: "// a\nb\nc",
    start: 3,
    end: 5,
  });
});

// The selection covered that line's indentation and none of its text, so the
// marker written there is not part of it either.
test("a selection ending where a marker goes in ends before it", () => {
  assert.deepEqual(toggleLineComment("  a\n  b", 0, 6, "c"), {
    value: "  // a\n  // b",
    start: 0,
    end: 9,
  });
});

test("a blank line is commented when it is all there is", () => {
  assert.deepEqual(toggleLineComment("", 0, 0, "java"), {
    value: "// ",
    start: 3,
    end: 3,
  });
  assert.deepEqual(toggleLineComment("a\n\nb", 2, 2, "python"), {
    value: "a\n# \nb",
    start: 4,
    end: 4,
  });
});

test("the comment toggle answers to Ctrl or Cmd with a typed slash", () => {
  const press = (event) =>
    isToggleCommentKeystroke({
      key: "/",
      ctrlKey: false,
      metaKey: false,
      altKey: false,
      shiftKey: false,
      isComposing: false,
      ...event,
    });
  assert.equal(press({ ctrlKey: true }), true);
  assert.equal(press({ metaKey: true }), true);
  // A layout that types the slash with Shift held still means the slash.
  assert.equal(press({ ctrlKey: true, shiftKey: true }), true);
  assert.equal(press({}), false);
  // AltGr on Windows, which is typing a character.
  assert.equal(press({ ctrlKey: true, altKey: true }), false);
  assert.equal(press({ ctrlKey: true, isComposing: true }), false);
  // The key in the US slash position on a layout where it types a hyphen:
  // Ctrl with it is the browser's zoom.
  assert.equal(press({ ctrlKey: true, key: "-", code: "Slash" }), false);
});

const literalPairingCases = [
  ["python", "# note |"],
  ["python", "# note |\npass"],
  ["python", 'text = "left| "'],
  ["python", 'text = "unfinished|'],
  ["python", 'text = """first\nsecond| """'],
  ["python", String.raw`text = "escaped \" quote| "`],
  ["javascript", "// note |"],
  ["javascript", "/* first\nsecond| */"],
  ["javascript", 'const text = "left| ";'],
  ["javascript", 'const text = "unfinished|'],
  ["javascript", "const text = `first\nsecond| `;"],
  ["javascript", "const text = `${value} text| `;"],
  ["c", "// note |"],
  ["c", "/* unfinished|"],
  ["c", 'char *text = "left| ";'],
  ["c", "char c = '| ';"],
  ["cpp", "// note |"],
  ["cpp", "/* first\nsecond| */"],
  ["cpp", 'auto text = "left| ";'],
  ["cpp", 'auto text = R"tag(first\nsecond| )tag";'],
  ["cpp", 'auto text = R"tag(first\nunfinished|'],
  ["java", "// note |"],
  ["java", "/* first\nsecond| */"],
  ["java", 'String text = "left| ";'],
  ["java", 'String text = """\nsecond| \n""";'],
];

const codePairingCases = [
  ["python", "|"],
  ["python", "# note\n|"],
  ["python", '"closed"|'],
  ["python", '"""closed"""|'],
  ["javascript", "/* closed */|"],
  ["javascript", "`closed`|"],
  ["javascript", "`${value| }`"],
  ["javascript", '"closed"| '],
  ["javascript", "/[| ]/"],
  ["c", "'x'|"],
  ["cpp", 'R"tag(closed)tag"|'],
  ["cpp", "#define VALUE |"],
  ["java", '"""\nclosed\n"""|'],
];

function checkContextPairing(parse) {
  for (const [cases, suppressed] of [
    [literalPairingCases, true],
    [codePairingCases, false],
  ]) {
    for (const [language, marked] of cases) {
      const at = marked.indexOf("|");
      const value = marked.replace("|", "");
      for (const [opener, closer] of [
        ["(", ")"],
        ["[", "]"],
        ["{", "}"],
      ]) {
        assert.deepEqual(
          insertBracketPair(value, at, at, opener, language, parse),
          suppressed
            ? null
            : {
                value: value.slice(0, at) + opener + closer + value.slice(at),
                start: at + 1,
                end: at + 1,
              },
          `${language}: ${marked}, typing ${opener}`,
        );
      }
    }
  }
}

test("bracket pairing distinguishes comments, strings and boundaries without a parser", () => {
  checkContextPairing(() => null);
  const value = 'f"{value }"';
  const at = value.indexOf(" ");
  assert.deepEqual(
    insertBracketPair(value, at, at, "(", "python", () => null),
    {
      value: 'f"{value() }"',
      start: at + 1,
      end: at + 1,
    },
  );
});

test("bracket pairing uses ready grammars and keeps interpolation expressions editable", async () => {
  const syntax = createSyntaxParser({
    loadBytes: async (url) => new Uint8Array(await readFile(url)),
  });
  await Promise.all(
    ["c", "cpp", "java", "javascript", "python"].map((language) =>
      syntax.prepareLanguage(language),
    ),
  );
  checkContextPairing(syntax.parseSyntax);
  const value = 'f"{value }"';
  const at = value.indexOf(" ");
  assert.deepEqual(
    insertBracketPair(value, at, at, "(", "python", syntax.parseSyntax),
    {
      value: 'f"{value() }"',
      start: at + 1,
      end: at + 1,
    },
  );
});

test("bracket pairing still wraps selected text inside comments and strings", () => {
  for (const value of ['"text"', "// text"]) {
    const start = value.indexOf("text");
    assert.deepEqual(
      insertBracketPair(value, start, start + 4, "(", "cpp", () => {
        throw new Error("Selection wrapping should not need lexical context");
      }),
      {
        value: value.slice(0, start) + "(text)" + value.slice(start + 4),
        start: start + 1,
        end: start + 5,
      },
    );
  }
});

test("bracket pairing preserves existing behavior for unknown languages", () => {
  for (const language of [undefined, "unknown", "constructor", "toString"]) {
    const value = "// note ";
    assert.deepEqual(
      insertBracketPair(value, value.length, value.length, "(", language),
      {
        value: value + "()",
        start: value.length + 1,
        end: value.length + 1,
      },
    );
  }
});

test("closer and Backspace handlers leave literal text to native editing", async () => {
  const syntax = createSyntaxParser({
    loadBytes: async (url) => new Uint8Array(await readFile(url)),
  });
  await Promise.all(
    ["python", "javascript", "c", "cpp", "java"].map((language) =>
      syntax.prepareLanguage(language),
    ),
  );
  for (const parse of [() => null, syntax.parseSyntax]) {
    for (const language of ["python", "javascript", "c", "cpp", "java"]) {
      for (const value of [
        '"a()"',
        `${language === "python" ? "#" : "//"} a()`,
      ]) {
        const at = value.indexOf(")");
        assert.equal(
          typeOverCloser(value, at, at, ")", language, parse),
          null,
          value,
        );
        assert.equal(
          deleteEmptyPair(value, at, at, language, parse),
          null,
          value,
        );
      }
      for (const [value, at] of [
        ['"a"()', 4],
        ["call()", 5],
      ]) {
        assert.deepEqual(typeOverCloser(value, at, at, ")", language, parse), {
          value,
          start: at + 1,
          end: at + 1,
        });
        assert.deepEqual(deleteEmptyPair(value, at, at, language, parse), {
          value: value.slice(0, at - 1) + value.slice(at + 1),
          start: at - 1,
          end: at - 1,
        });
      }
    }
  }
});

test("unparsed Python f-strings retain pairing while ordinary strings stay literal", () => {
  for (const prefix of ["f", "F", "fr", "Fr", "rf", "RF"]) {
    for (const quote of ['"', "'", '"""', "\'\'\'"]) {
      for (const content of ["{value }", "text "]) {
        const value = prefix + quote + content + quote;
        const at = value.indexOf(" ");
        const paired = insertBracketPair(
          value,
          at,
          at,
          "(",
          "python",
          () => null,
        );
        assert.deepEqual(
          paired,
          {
            value: value.slice(0, at) + "()" + value.slice(at),
            start: at + 1,
            end: at + 1,
          },
          value,
        );
        assert.deepEqual(
          deleteEmptyPair(paired.value, at + 1, at + 1, "python", () => null),
          {
            value,
            start: at,
            end: at,
          },
          value,
        );
        assert.deepEqual(
          typeOverCloser(
            paired.value,
            at + 1,
            at + 1,
            ")",
            "python",
            () => null,
          ),
          {
            value: paired.value,
            start: at + 2,
            end: at + 2,
          },
          value,
        );
      }
    }
  }
  for (const value of ['r"text "', 'b"text "', '"f text "']) {
    const at = value.lastIndexOf(" ");
    assert.equal(
      insertBracketPair(value, at, at, "(", "python", () => null),
      null,
      value,
    );
  }
});

test("error trees preserve pairing in unfinished Python f-string expressions", async () => {
  const syntax = createSyntaxParser({
    loadBytes: async (url) => new Uint8Array(await readFile(url)),
  });
  await syntax.prepareLanguage("python");
  for (const value of [
    'f"{value',
    'f"{value ',
    'f"{value }',
    'RF"{value ',
    'f"""text\n{value ',
  ]) {
    const at = value.indexOf("value") + 5;
    for (const [opener, closer] of [
      ["(", ")"],
      ["[", "]"],
      ["{", "}"],
    ]) {
      const typed = value.slice(0, at) + opener + value.slice(at);
      const tree = syntax.parseSyntax(typed, "python");
      assert.ok(
        tree,
        "The regression needs a returned tree, not an unloaded parser",
      );
      try {
        assert.equal(tree.rootNode.hasError, true, typed);
      } finally {
        tree.delete();
      }
      const paired = insertBracketPair(
        value,
        at,
        at,
        opener,
        "python",
        syntax.parseSyntax,
      );
      assert.deepEqual(
        paired,
        {
          value: value.slice(0, at) + opener + closer + value.slice(at),
          start: at + 1,
          end: at + 1,
        },
        typed,
      );
      assert.deepEqual(
        deleteEmptyPair(
          paired.value,
          at + 1,
          at + 1,
          "python",
          syntax.parseSyntax,
        ),
        {
          value,
          start: at,
          end: at,
        },
        typed,
      );
      assert.deepEqual(
        typeOverCloser(
          paired.value,
          at + 1,
          at + 1,
          closer,
          "python",
          syntax.parseSyntax,
        ),
        {
          value: paired.value,
          start: at + 2,
          end: at + 2,
        },
        typed,
      );
    }
  }
  for (const marked of [
    '"unfinished| ',
    'f"literal| "\nif (',
    'f"""literal| \n"""\nif (',
  ]) {
    const at = marked.indexOf("|");
    const value = marked.replace("|", "");
    assert.equal(
      insertBracketPair(value, at, at, "(", "python", syntax.parseSyntax),
      null,
      marked,
    );
  }
});

test("moving text preserves every unselected newline", () => {
  for (const [value, start, end, target, expected, selection] of [
    ["DRAG\n\n\n\n", 0, 4, 6, "\n\nDRAG\n\n", [2, 6]],
    ["DRAG\n", 0, 4, 5, "\nDRAG", [1, 5]],
    ["\n\nDRAG\n", 2, 6, 0, "DRAG\n\n\n", [0, 4]],
    ["A\nB\nC\n", 0, 4, 6, "C\nA\nB\n", [2, 6]],
    ["甲😀\n乙\n", 1, 3, 6, "甲\n乙\n😀", [4, 6]],
  ]) {
    const result = moveSelectedText(value, start, end, target);
    assert.deepEqual(result, {
      value: expected,
      start: selection[0],
      end: selection[1],
    });
    assert.equal(result.value.length, value.length);
    assert.equal(result.value.split("\n").length, value.split("\n").length);
  }
});

test("dropping in the original selection or moving no text leaves it unchanged", () => {
  for (const target of [1, 2, 3])
    assert.deepEqual(moveSelectedText("abcd", 1, 3, target), {
      value: "abcd",
      start: 1,
      end: 3,
    });
  assert.deepEqual(moveSelectedText("abcd", 2, 2, 4), {
    value: "abcd",
    start: 2,
    end: 2,
  });
});
