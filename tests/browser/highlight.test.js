// Run with: node --test tests/browser/*.test.js
//
// The highlighter turns candidate code into HTML, so escaping is the property
// that matters most: the textarea is the source of truth and the overlay must
// never be able to introduce markup or lose a character.

import { test } from "node:test";
import assert from "node:assert/strict";

import { highlight } from "../../web/highlight.js";

/// The overlay must render exactly the characters the textarea holds, or the
/// painted text drifts out of line with the caret.
const textOf = (html) =>
  html
    .replace(/<[^>]+>/g, "")
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&quot;", '"')
    .replaceAll("&#39;", "'")
    .replaceAll("&amp;", "&");

/// Trailing newlines are compared loosely: highlight() deliberately appends one
/// so the overlay renders the same final empty line the textarea shows.
const roundTrips = (code, language) =>
  assert.equal(
    textOf(highlight(code, language)).replace(/\n+$/, ""),
    code.replace(/\n+$/, ""),
    "highlighting changed the text",
  );

test("python keywords, strings, comments, and numbers are marked", () => {
  const html = highlight('def f(n):\n    # add\n    return n + 1 if n else "x"', "python");

  assert.match(html, /<span class="tok-keyword">def<\/span>/);
  assert.match(html, /<span class="tok-keyword">return<\/span>/);
  assert.match(html, /<span class="tok-comment"># add<\/span>/);
  assert.match(html, /<span class="tok-number">1<\/span>/);
  assert.match(html, /<span class="tok-string">&quot;x&quot;<\/span>/);
  assert.doesNotMatch(html, /<span class="tok-keyword">f<\/span>/, "a plain name is not a keyword");
});

test("python literals and triple-quoted strings", () => {
  const html = highlight('x = True\ns = """a\nb"""\n', "python");

  assert.match(html, /<span class="tok-literal">True<\/span>/);
  assert.match(html, /<span class="tok-string">(?:&quot;){3}a\nb(?:&quot;){3}<\/span>/, "triple quotes span lines");
});

test("javascript keywords, template strings, and both comment forms", () => {
  const html = highlight("const a = `t${x}`; // note\n/* block */ let b = 0x1f;", "javascript");

  assert.match(html, /<span class="tok-keyword">const<\/span>/);
  assert.match(html, /<span class="tok-keyword">let<\/span>/);
  assert.match(html, /<span class="tok-comment">\/\/ note<\/span>/);
  assert.match(html, /<span class="tok-comment">\/\* block \*\/<\/span>/);
  assert.match(html, /<span class="tok-number">0x1f<\/span>/);
});

test("compiled language keywords, literals, strings, and comments are marked", () => {
  const c = highlight('int f(char* s) { // return\n  return NULL;\n}', "c");
  assert.match(c, /<span class="tok-keyword">int<\/span>/);
  assert.match(c, /<span class="tok-keyword">return<\/span>/);
  assert.match(c, /<span class="tok-literal">NULL<\/span>/);
  assert.match(c, /<span class="tok-comment">\/\/ return<\/span>/);

  const cpp = highlight('class Solution { int f() { string s = "return"; return 1; } };', "cpp");
  assert.match(cpp, /<span class="tok-keyword">class<\/span>/);
  assert.match(cpp, /<span class="tok-keyword">return<\/span>/);
  assert.match(cpp, /<span class="tok-string">&quot;return&quot;<\/span>/);

  const java = highlight("public class Solution { boolean ok = true; }", "java");
  assert.match(java, /<span class="tok-keyword">public<\/span>/);
  assert.match(java, /<span class="tok-keyword">boolean<\/span>/);
  assert.match(java, /<span class="tok-literal">true<\/span>/);
});

test("a keyword inside a string or comment stays part of it", () => {
  const html = highlight('s = "return 1"  # def g', "python");

  assert.match(html, /<span class="tok-string">&quot;return 1&quot;<\/span>/);
  assert.match(html, /<span class="tok-comment"># def g<\/span>/);
  assert.doesNotMatch(html, /tok-keyword/, "no keyword escapes its enclosing token");

  for (const language of ["c", "cpp", "java"]) {
    const compiled = highlight('value = "return"; // class', language);
    assert.match(compiled, /<span class="tok-string">&quot;return&quot;<\/span>/);
    assert.match(compiled, /<span class="tok-comment">\/\/ class<\/span>/);
    assert.doesNotMatch(compiled, /tok-keyword/, `${language} keyword escaped its enclosing token`);
  }
});

test("markup in code is escaped, never emitted", () => {
  const html = highlight('x = "<img src=q onerror=alert(1)>"', "python");

  assert.doesNotMatch(html, /<img/, "candidate code must not become live markup");
  assert.match(html, /&lt;img src=q onerror=alert\(1\)&gt;/);
});

test("ampersands and angle brackets outside strings are escaped too", () => {
  const html = highlight("a && b < c > d", "javascript");

  assert.match(html, /&amp;&amp;/);
  assert.match(html, /&lt;/);
  assert.match(html, /&gt;/);
});

test("every character survives highlighting", () => {
  roundTrips("def two_sum(nums, target):\n    seen = {}\n    return []\n", "python");
  roundTrips("const f = (a) => a ?? `x${a}`;\n\n// tail\n", "javascript");
  roundTrips("int f(char* s) {\n    return 0;\n}\n", "c");
  roundTrips("vector<int> f(vector<int>& nums) { return {}; }\n", "cpp");
  roundTrips("class Solution { public int f() { return 0; } }\n", "java");
  roundTrips("", "python");
  roundTrips("   \n\n\t", "python");
  roundTrips("no tokens here", "python");
});

test("a trailing newline gains the extra line the textarea shows", () => {
  // <pre> drops the final empty line, so the caret would sit one row below the
  // painted text without this.
  assert.equal(highlight("a", "python"), "a");
  assert.ok(highlight("a\n", "python").endsWith("a\n\n"));
  assert.ok(highlight("a\n\n", "python").endsWith("a\n\n\n"));
});

test("an unterminated string does not swallow the rest of the buffer", () => {
  const html = highlight('s = "oops\nreturn 1\n', "python");

  assert.match(html, /<span class="tok-keyword">return<\/span>/, "code after it still highlights");
  roundTrips('s = "oops\nreturn 1\n', "python");
});

test("an unknown language falls back instead of throwing", () => {
  const html = highlight("const x = 1;", "brainfuck");
  assert.match(html, /tok-keyword/);
  roundTrips("const x = 1;", "brainfuck");
});
