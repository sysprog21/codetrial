import { test } from "node:test";
import assert from "node:assert/strict";

import { highlight } from "../../web/highlight.js";
import {
  bufferIndent,
  detectWidth,
  indentGuides,
} from "../../web/indent-guides.js";
import { tokenize } from "../../web/tokenizer.js";

const textOf = (html) =>
  html
    .replace(/<[^>]+>/g, "")
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&quot;", '"')
    .replaceAll("&#39;", "'")
    .replaceAll("&amp;", "&");

/// The overlay the editor paints: highlighted code with its guides in place.
const paint = (code, language = "javascript") => {
  const { tokens } = tokenize(code, language);
  return highlight(code, language, [], tokens, indentGuides(code, tokens));
};

/// Guides per line, so a test reads as a picture of the code.
const depths = (code, language) =>
  paint(code, language)
    .split("\n")
    .map((line) => line.split('class="indent-guide"').length - 1);

const widthOf = (code, language = "javascript") => {
  const { tokens } = tokenize(code, language);
  let offset = 0;
  const lines = code.split("\n").map((line) => {
    const whitespace = line.match(/^[ \t]*/)[0];
    const start = offset;
    offset += line.length + 1;
    return { start, whitespace, blank: whitespace.length === line.length };
  });
  return detectWidth(lines, tokens);
};

test("each full level of indentation gets one guide", () => {
  assert.deepEqual(
    depths("def f(n):\n    if n:\n        return 1\n    return 0", "python"),
    [0, 1, 2, 1],
  );
});

// The case review found: at a fixed width of four the `if` body went
// unmarked and `x();` got the function's guide instead of its own.
test("two-space code gets a guide per two columns", () => {
  assert.deepEqual(
    depths("function f() {\n  if (a) {\n    x();\n  }\n}"),
    [0, 1, 2, 1, 0],
  );
});

// A doc comment's ` * ` lines sit one column in from `/**`, and nearly every
// starter has one; counting them would measure a width of 1.
test("a doc comment does not vote on the width", () => {
  const starter = [
    "/**",
    " * @param {number[]} nums",
    " * @return {number}",
    " */",
    "var f = function (nums) {",
    "  for (const n of nums) {",
    "    if (n) {",
    "      return n;",
    "    }",
    "  }",
    "};",
  ].join("\n");
  assert.equal(widthOf(starter), 2);
  assert.deepEqual(depths(starter), [0, 0, 0, 0, 0, 1, 2, 3, 2, 1, 0]);
});

// Most JavaScript starters, whose only indented line is a comment.
test("a line that opens a comment is indented like code", () => {
  const starter =
    "/**\n * @param {string} s\n * @return {boolean}\n */\n" +
    "function isValid(s) {\n  // Think out loud as you go!\n}\n";
  assert.equal(widthOf(starter), 2);
});

test("a step of one is alignment, not a level", () => {
  assert.equal(widthOf("a(b,\n c);\nif (x) {\n    y;\n}"), 4);
});

test("a buffer with nothing indented measures the editor's own width", () => {
  assert.equal(widthOf("x;\ny;"), 4);
  assert.equal(widthOf(""), 4);
});

test("a tie takes the narrower width", () => {
  assert.equal(widthOf("a {\n  b {\n      c;\n  }\n}"), 2);
});

test("a tab on a blank line in a two-space block does not break the line", () => {
  const code = "a {\n  b {\n    c {\n      x;\n\t\n      y;\n    }\n  }\n}";
  assert.equal(depths(code)[4], 3);
});

test("a tab and four spaces land on the same guide", () => {
  assert.deepEqual(depths("{\n\tx;\n  \ty;\n    z;\n}"), [0, 1, 1, 1, 0]);
});

test("a partial level still gets its guide", () => {
  assert.deepEqual(
    depths("a:\n    b\n    c:\n        d\n      e", "python"),
    [0, 1, 1, 2, 2],
  );
});

test("a code line seven columns in gets guides at 0 and 4", () => {
  const code = "a {\n    b {\n        x;\n    }\n       c;\n}";
  assert.equal(widthOf(code), 4);
  assert.equal(depths(code)[4], 2);
  assert.equal(textOf(paint(code)), code);
});

/// Line 3 of a block whose lines 2 and 4 are two levels deep, at width 4.
const deepBlankRow = (blank) => {
  const code = `a {\n    b {\n        x;\n${blank}\n        y;\n    }\n}`;
  return { depth: depths(code)[3], text: textOf(paint(code)).split("\n")[3] };
};

// Each reaches the second guide's column differently: not at all, partway
// through the first level, and partway through the second.
test("a blank line between two deep lines gets both guides at any length", () => {
  for (const blank of ["", "   ", "       "]) {
    assert.equal(deepBlankRow(blank).depth, 2, JSON.stringify(blank));
  }
  // Seven columns already reach the second guide, so nothing is added; three
  // do not, so the line is padded out to it.
  assert.equal(deepBlankRow("       ").text, "       ");
  assert.equal(deepBlankRow("   ").text.length, 8);
});

test("a blank line takes the shallower of its neighbours", () => {
  assert.deepEqual(
    depths("def f():\n    a = 1\n\n    return a\n\nprint(f())", "python"),
    [0, 1, 1, 1, 0, 0],
  );
  assert.deepEqual(depths("\n\n    x\n\ny"), [0, 0, 1, 0, 0]);
});

/// The guides on line 3 of a two-level block whose line 3 is `blank` and whose
/// line 4 is `next`, so each case below reads as the one row it is about.
const blankRow = (blank, next) =>
  depths(`function f() {\n  if (a) {\n    x();\n${blank}\n${next}\n}`)[3];

test("an empty line inside a block keeps the block's guides", () => {
  assert.equal(blankRow("", "    y();\n  }"), 2);
});

test("an empty line after a block stops the inner guide", () => {
  assert.equal(blankRow("", "  }"), 1);
});

// Enter at the end of a block auto-indents the new line to the block's depth,
// with the shallower closer below it; until something is typed on it, it is
// blank, and it used to take the closer's depth instead of the caret's.
test("an auto-indented line at the end of a block keeps its own depth", () => {
  assert.equal(blankRow("    ", "  }"), 2);
});

test("stray whitespace draws no guide deeper than the code around it", () => {
  assert.deepEqual(depths("x;\n            \ny;"), [0, 0, 0]);
});

// The overlay has to put every character where the textarea does, so the
// guides wrap a code line's own whitespace and add nothing to it.
test("code lines keep their text, escaped, around the guides", () => {
  const code = 'if (a < b) {\n\tlog("<b>&</b>");\n  \t}\n  // <c>\n    d;';
  assert.equal(textOf(paint(code)), code);
  assert.doesNotMatch(paint(code), /<b>|<c>/);
});

test("guides inside a string or comment do not break its color", () => {
  const html = paint('def f():\n    s = """\n        x\n    """', "python");
  assert.match(
    html,
    /<span class="tok-string">(&quot;){3}\n<span class="indent-guide"> {4}<\/span>/,
  );
});

test("a blank line is never narrower than its own whitespace", () => {
  const html = paint("{\n    x\n          \n    y\n}");
  assert.equal(textOf(html).split("\n")[2].length, 10);
});

test("a trailing newline keeps the final empty row", () => {
  assert.ok(paint("x\n").endsWith("\n\n"));
});

const indentOf = (code, language = "javascript") =>
  bufferIndent(code, tokenize(code, language).tokens);

// The editor takes a starter's space width from this, and the candidate's
// choice where it measures nothing: a width of four must not fall back the
// way the guides' default does, or a starter with no block could never take
// the choice.
test("a starter's space width is one level as it writes it, or none", () => {
  assert.equal(indentOf("", "python"), null);
  assert.equal(indentOf("x = 1\ny = 2", "python"), null);
  assert.equal(
    indentOf("class Solution:\n    def f(self):\n        pass", "python"),
    "    ",
  );
  assert.equal(indentOf("function f() {\n  return 1;\n}"), "  ");
  // Tab-indented lines do not vote: the editor gives them tabs line by line.
  assert.equal(indentOf("function f() {\n\treturn 1;\n}"), null);
  assert.equal(
    indentOf("function f() {\n  if (a) {\n    x();\n  }\n\ty();\n  z();\n}"),
    "  ",
  );
  // A doc comment's ` * ` lines do not make a JavaScript starter
  // one-column-indented.
  assert.equal(
    indentOf("/**\n * @param {number} n\n */\nfunction f(n) {\n  return n;\n}"),
    "  ",
  );
  // A mostly tabbed file can still hold a two-space function, and that
  // function's own width is what is measured.
  assert.equal(
    indentOf(
      "def a():\n\tif x:\n\t\tpass\n\tif y:\n\t\tpass\n\ndef b():\n  if z:\n    w()\n",
      "python",
    ),
    "  ",
  );
});
