// A fixture under tests/fixtures/candidates/ with no test reading it.
//
// A case deleted or renamed leaves its answer behind, and the folder goes on
// looking like code some test runs. The reverse is checked too: a name with no
// file fails when its test reads it, but not when that test is skipped, as the
// Python driver tests are on a host without an interpreter.

import { test } from "node:test";
import assert from "node:assert/strict";
import { statSync } from "node:fs";
import { join } from "node:path";

import { filesUnder, read, root } from "./source.js";

const FIXTURES = "tests/fixtures/candidates";

// This file's own test below holds sample calls, and source.js defines the
// function rather than calling it.
const SKIPPED = new Set(["candidate-fixtures.test.js", "source.js"]);

// Comments go first, so a case disabled by commenting out its call stops
// counting as a reader. A `/*` is stripped only after whitespace or
// punctuation: inside a "**/api/**" route glob it follows a letter or `*`, and
// stripping there would erase the code up to the next `*/`.
// Then every `candidateSource(` that is not part of a longer name or a member
// access is a call (a spread's `...` is neither), unless its line has `//`
// before it, which makes it a trailing comment. The rule fails closed: a call
// that names no plain literal fails, and so does one after an odd number of
// `'` or `` ` `` on its line, which sits inside a string; a real call after a
// URL on its line fails as unread. Each call has to name a fixture in the
// folder of the test file making it, so a shared helper reads none.
function namesIn(text, file) {
  const folder = file.replace(/\.test\.js$/, "");
  const names = [];
  const source = text
    .replace(/(^|[\s;{}()[\],=:!?&|])\/\*[\s\S]*?\*\//gm, "$1")
    .replace(/^[ \t]*\/\/.*$/gm, "");
  for (const match of source.matchAll(
    /(?<![\w$]|[^.]\.)candidateSource\(([^)]*)\)/g,
  )) {
    const lineStart = source.lastIndexOf("\n", match.index) + 1;
    const before = source.slice(lineStart, match.index);
    if (before.includes("//")) {
      continue;
    }
    const [call, argument] = match;
    assert.ok(
      file.endsWith(".test.js"),
      `${file}: ${call} is in a helper; only a .test.js file reads a fixture`,
    );
    assert.ok(
      before.split("'").length % 2 && before.split("`").length % 2,
      `${file}: ${call} sits inside a string`,
    );
    // A name built at run time cannot be read here, so it fails rather than
    // dropping out of the count.
    const literal = argument
      .trim()
      .replace(/,$/, "")
      .match(/^"([^"\\$]+)"$/);
    assert.ok(literal, `${file}: ${call} does not name its fixture literally`);
    assert.ok(
      literal[1].startsWith(`${folder}/`),
      `${file}: ${literal[1]} is outside ${FIXTURES}/${folder}/`,
    );
    names.push(literal[1]);
  }
  return names;
}

function readNames() {
  const names = new Set();
  for (const file of filesUnder("tests/browser", ".js")) {
    if (SKIPPED.has(file)) {
      continue;
    }
    for (const name of namesIn(read(`tests/browser/${file}`), file)) {
      names.add(name);
    }
  }
  return names;
}

test("a commented-out call is dropped and a call between globs is kept", () => {
  const file = "x.test.js";
  assert.deepEqual(namesIn('// candidateSource("x/a.c");\n', file), []);
  assert.deepEqual(
    namesIn('x();\n/*\ncandidateSource("x/a.c");\n*/\n', file),
    [],
  );
  assert.deepEqual(
    namesIn('x(); /*\ncandidateSource("x/a.c");\n*/\n', file),
    [],
  );
  for (const disabled of [
    'const code = /*\ncandidateSource("x/a.c");\n*/ null;',
    'const c = { code: /* candidateSource("x/a.c") */ null };',
    'run(/* candidateSource("x/a.c") */ null);',
    'const all = [/* candidateSource("x/a.c") */];',
    'if (!/* candidateSource("x/a.c") */ ok) skip();',
    'return /* candidateSource("x/a.c") */ null;',
  ]) {
    assert.deepEqual(namesIn(`${disabled}\n`, file), [], disabled);
  }
  for (const disabled of [
    'run(); // candidateSource("x/a.c");',
    'run(); // Disabled: candidateSource("x/a.c");',
    'run(); // see (candidateSource("x/a.c"))',
  ]) {
    assert.deepEqual(namesIn(`${disabled}\n`, file), [], disabled);
  }
  assert.deepEqual(
    namesIn(
      'route("**/api/**", h);\ncandidateSource("x/b.c");\n' +
        'route("**/judges/*.json", h);\n',
      file,
    ),
    ["x/b.c"],
  );
  for (const call of [
    'load(() => candidateSource("x/c.c"));',
    'return candidateSource("x/c.c");',
    'a ? candidateSource("x/c.c") : b;',
    'a || candidateSource("x/c.c");',
    'run(() => { candidateSource("x/c.c"); });',
    'setUp(); candidateSource("x/c.c");',
    'const code = await candidateSource("x/c.c");',
    'if (!candidateSource("x/c.c")) skip();',
    'const code = "int " + candidateSource("x/c.c");',
    'const all = [...candidateSource("x/c.c")];',
  ]) {
    assert.deepEqual(namesIn(`${call}\n`, file), ["x/c.c"], call);
  }
  assert.throws(
    () => namesIn('candidateSource("y/b.c");\n', file),
    /x\.test\.js: y\/b\.c is outside tests\/fixtures\/candidates\/x\//,
  );
  assert.throws(
    () =>
      namesIn(
        'candidateSource("interview-fixture/a.c");\n',
        "interview-fixture.js",
      ),
    /interview-fixture\.js: .* is in a helper; only a \.test\.js file reads a fixture/,
  );
  for (const quoted of [
    `log('candidateSource("x/a.c")');`,
    'const s = `candidateSource("x/a.c")`;',
  ]) {
    assert.throws(
      () => namesIn(`${quoted}\n`, file),
      /sits inside a string/,
      quoted,
    );
  }
  for (const unread of [
    "candidateSource(`x/${name}.c`);",
    'log("candidateSource(x/a.c)");',
  ]) {
    assert.throws(
      () => namesIn(`${unread}\n`, file),
      /does not name its fixture literally/,
      unread,
    );
  }
});

test("every candidate fixture is read by a browser test", () => {
  const names = readNames();
  // A dotfile, such as the .DS_Store Finder leaves, is no fixture.
  const fixtures = filesUnder(FIXTURES, "").filter(
    (name) =>
      statSync(join(root, FIXTURES, name)).isFile() && !/(^|\/)\./.test(name),
  );
  assert.ok(fixtures.length > 0, `${FIXTURES} has no fixtures`);
  assert.deepEqual(
    fixtures.filter((name) => !names.has(name)),
    [],
    "fixtures no candidateSource call reads",
  );
  assert.deepEqual(
    [...names].filter((name) => !fixtures.includes(name)),
    [],
    "candidateSource names with no fixture",
  );
});
