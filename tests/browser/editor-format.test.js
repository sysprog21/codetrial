// Run with: node --test tests/browser/editor-format.test.js
//
// The parts of Shift+Alt+F formatting that need no page: what the formatter
// answers for a buffer and which keystrokes are the chord.
// format-shortcut.test.js drives the same code, through the Format JS button
// and the chord, in a browser.

import { test } from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";

import { createJavaScriptFormatter } from "../../web/format.js";
import { isFormatKeystroke, shortcutKey } from "../../web/editor.js";

const FOUR = "    ";
const format = createJavaScriptFormatter();

async function formatted(code, caret = 0, indent = FOUR) {
  const result = await format(code, caret, indent);
  assert.equal(result.ok, true, result.message);
  return result;
}

// Before and after, byte for byte, under the default four-space indentation.
// Each "after" is Prettier 3.10.1's answer; a version bump that moves one is
// a change the pull request has to show.
const FIXTURES = [
  {
    name: "a one-line function",
    before: "function add(a,b){return a+b}",
    after: "function add(a, b) {\n    return a + b;\n}\n",
  },
  {
    name: "comments, line and block, kept where they were",
    before:
      "// head\nfunction f(x){ /* inner */ return x // tail\n}\n/* end */\n",
    after:
      "// head\nfunction f(x) {\n    /* inner */ return x; // tail\n}\n/* end */\n",
  },
  {
    name: "a template literal's text untouched, its expressions spaced",
    before: "const s=`a  b\n   c ${ 1+2 }`\n",
    after: "const s = `a  b\n   c ${1 + 2}`;\n",
  },
  {
    name: "a directive stays first and keeps its quotes",
    before: "'use strict'\nlet x=1\n",
    after: '"use strict";\nlet x = 1;\n',
  },
  {
    name: "a long line broken at eighty columns",
    before:
      "const total = values.map((value) => value * factor).filter((value) => value > threshold).reduce((a, b) => a + b, 0);\n",
    after:
      "const total = values\n    .map((value) => value * factor)\n    .filter((value) => value > threshold)\n    .reduce((a, b) => a + b, 0);\n",
  },
  // Automatic semicolon insertion does not apply before a line that opens
  // with "[", so these two lines are one expression. Prettier prints the
  // expression the parser saw, which shows the hazard rather than hiding it.
  {
    name: "a missing semicolon before '[' printed as the one expression it is",
    before: "let a = 1\n[1,2].forEach(x=>x)\n",
    after: "let a = (1)[(1, 2)].forEach((x) => x);\n",
  },
];

for (const { name, before, after } of FIXTURES) {
  test(`formats ${name}`, async () => {
    assert.equal((await formatted(before)).value, after);
  });

  test(`formatting ${name} a second time changes nothing`, async () => {
    assert.equal((await formatted(after)).value, after);
  });
}

test("an empty or blank buffer formats to empty, with the caret kept in range", async () => {
  assert.deepEqual(await format("", 0, FOUR), {
    ok: true,
    value: "",
    cursor: 0,
  });
  assert.deepEqual(await format("  \n\n ", 4, FOUR), {
    ok: true,
    value: "",
    cursor: 0,
  });
});

test("the caret stays on the token it was on", async () => {
  const code = "function f(a,b){return a+b}";
  const caret = code.indexOf("return") + 2;
  const { value, cursor } = await formatted(code, caret);
  assert.equal(value.slice(cursor - 2, cursor + 4), "return");
});

test("the lobby's indentation is the formatter's", async () => {
  const code = "if(a){b()}";
  assert.equal((await formatted(code, 0, "  ")).value, "if (a) {\n  b();\n}\n");
  assert.equal(
    (await formatted(code, 0, FOUR)).value,
    "if (a) {\n    b();\n}\n",
  );
  assert.equal((await formatted(code, 0, "\t")).value, "if (a) {\n\tb();\n}\n");
});

// A formatter that guessed at incomplete code would hand the candidate a
// program they did not write. Acorn refuses it, and the refusal names where.
for (const [code, message] of [
  ["function f(", "Unexpected token at line 1, column 12"],
  ["let x = ;", "Unexpected token at line 1, column 9"],
  ["if (a) {\n  b()\n", "Unexpected token at line 3, column 1"],
  ["const s = `open", "Unterminated template at line 1, column 12"],
  ["a + b = 1", "Assigning to rvalue at line 1, column 1"],
]) {
  test(`incomplete code is refused, not repaired: ${JSON.stringify(code)}`, async () => {
    assert.deepEqual(await format(code, 0, FOUR), { ok: false, message });
  });
}

// The known exceptions to "only what the runner runs": Prettier's acorn
// parser carries JSX and allows a top-level return. Each is formatted, and
// stays as unrunnable after as it was before, so nothing invalid becomes valid.
test("JSX and a top-level return are formatted rather than refused", async () => {
  assert.equal(
    (await formatted("const a=<div/>")).value,
    "const a = <div />;\n",
  );
  assert.equal((await formatted("return  1")).value, "return 1;\n");
});

// Parsing is not proof: each program prints the same thing before and after,
// run in a fresh context each time.
for (const program of [
  "let a = [3,1,2]\nlet b = a.sort((x,y)=>x-y)\n;[a.length].forEach(n=>out.push(n))\nout.push(b.join(','))",
  "function f(){return (\n1+1)}\nout.push(f())",
  "const o={a:1,'b-c':2}; out.push(JSON.stringify(o))",
  "let i=0\ni++\n++i\nout.push(i)",
  "const t=`x${ [1,2].map(n=>n*2) }y`;out.push(t)",
  "label: for(const x of [1,2,3]){ if(x===2) continue label; out.push(x) }",
]) {
  test(`formatting keeps what a program does: ${JSON.stringify(program.slice(0, 30))}`, async () => {
    const run = (code) => {
      const out = [];
      vm.runInNewContext(code, { out });
      return out;
    };
    const after = (await formatted(program)).value;
    assert.notEqual(after, program);
    assert.deepEqual(run(after), run(program));
  });
}

test("a large buffer formats within the budget", async (t) => {
  const body = Array.from(
    { length: 400 },
    (_, i) =>
      `function f${i}(a,b){const c=[a,b,${i}].map(x=>x*2).filter(x=>x>1);return c.reduce((s,x)=>s+x,0)}`,
  ).join("\n");
  await formatted("let warm=1"); // the import is the first call's cost, not this one's
  const started = performance.now();
  const { value } = await formatted(body);
  const elapsed = performance.now() - started;
  t.diagnostic(
    `${body.length} characters, ${value.split("\n").length} lines formatted in ${Math.round(elapsed)} ms`,
  );
  // Measured at 245 ms on one machine. The ceiling is twenty times that, so a
  // loaded CI shard does not fail it; it catches a formatter that went
  // quadratic, not a slow runner.
  assert.ok(elapsed < 5000, `${Math.round(elapsed)} ms`);
});

test("a formatter that cannot load says so, and a retry asks again only for the module that failed", async () => {
  const seen = [];
  let online = false;
  const formatter = createJavaScriptFormatter((url) => {
    seen.push(url.pathname.split("/").pop() + url.search);
    return url.pathname.endsWith("estree.mjs") && !online
      ? Promise.reject(new TypeError("Failed to fetch"))
      : import(url.href);
  });
  assert.deepEqual(await formatter("let a=1", 0, FOUR), {
    ok: false,
    message: "The formatter could not be loaded",
  });
  online = true;
  assert.deepEqual(await formatter("let a=1", 0, FOUR), {
    ok: true,
    value: "let a = 1;\n",
    cursor: 0,
  });
  assert.deepEqual(seen, [
    "standalone.mjs",
    "estree.mjs",
    "acorn.mjs",
    "estree.mjs?retry=1",
  ]);
});

// A Prettier error with no parser position still says what went wrong.
test("a formatter error without a position is reported as it is", async () => {
  const broken = createJavaScriptFormatter(async (url) =>
    url.pathname.endsWith("standalone.mjs")
      ? {
          formatWithCursor() {
            throw new Error("Comment was not printed\nmore detail");
          },
        }
      : {},
  );
  assert.deepEqual(await broken("let a=1", 0, FOUR), {
    ok: false,
    message: "Comment was not printed",
  });
});

const key = (init) => ({
  key: "F",
  code: "KeyF",
  shiftKey: true,
  altKey: true,
  ctrlKey: false,
  metaKey: false,
  isComposing: false,
  keyCode: 70,
  ...init,
});

test("Shift+Alt+F is the chord, on every platform's spelling of it", () => {
  assert.equal(isFormatKeystroke(key()), true);
  assert.equal(isFormatKeystroke(key({ key: "f" })), true);
  // Option+Shift+F on a Mac composes a character; the key's position says F.
  assert.equal(isFormatKeystroke(key({ key: "Ï" })), true);
});

test("nothing else is the chord", () => {
  for (const [why, init] of [
    ["no Shift", { shiftKey: false, key: "f" }],
    ["no Alt", { altKey: false }],
    ["AltGr, which is Ctrl+Alt on Windows", { ctrlKey: true }],
    ["Cmd", { metaKey: true }],
    ["another letter", { key: "G", code: "KeyG" }],
    ["a composition", { isComposing: true }],
    ["an engine that cleared isComposing early", { keyCode: 229 }],
  ])
    assert.equal(isFormatKeystroke(key(init)), false, why);
});

test("shortcutKey lowercases ASCII, reads a composed character by key position, and keeps named keys", () => {
  assert.equal(shortcutKey({ key: "Z", code: "KeyZ" }), "z");
  assert.equal(shortcutKey({ key: "Ω", code: "KeyZ" }), "z");
  assert.equal(shortcutKey({ key: "Enter", code: "Enter" }), "Enter");
});
