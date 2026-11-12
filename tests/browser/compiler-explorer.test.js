import { describe, it, test } from "node:test";
import assert from "node:assert/strict";
import { execFile, execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

import {
  ALL_LANGUAGES,
  COMPILED_LANGUAGES,
  compilerDiagnosticLocations,
  compilerType,
  generateHarness,
  HARNESS_SIGNATURE_NOTE,
  harnessAvailability,
  harnessGap,
  harnessLineOffset,
  languagesFor,
  mapCompilerResponse,
  nativeLiteral,
  parseCompilerResults,
  stripAnsi,
} from "../../web/compiler-explorer.js";

// The per-problem files the browser fetches, not the bank they are generated
// from: the generator puts each variant's entry-point name in place of the
// published one, and a harness is built against what the candidate's starter
// code defines. The files are named by page; these tests speak in ids, which
// the generated map translates.
const webFile = (path) => new URL(`../../web/${path}`, import.meta.url);
const pageMap = JSON.parse(readFileSync(webFile("problem-pages.json"), "utf8"));
const judges = Object.fromEntries(
  Object.entries(pageMap).map(([id, { page }]) => [
    id,
    JSON.parse(readFileSync(webFile(`judges/${page}.json`), "utf8")),
  ]),
);

const sampleValues = {
  boolean: true,
  "character[][]": [["a", "b"], ["c"]],
  double: 1.5,
  "double[]": [1.5, 2.25],
  integer: 42,
  "integer[]": [1, 2, 3],
  "integer[][]": [[1, 2], [3]],
  "list<double>": [1.5, 2.25],
  "list<integer>": [1, 2, 3],
  "list<list<integer>>": [[1, 2], [3]],
  "list<list<string>>": [["ab"], ["cd", "ef"]],
  "list<string>": ["ab", "cd"],
  ListNode: [1, 2, 3],
  "ListNode[]": [[1, 2], [3]],
  string: "abc",
  "string[]": ["ab", "cd"],
  TreeNode: [1, null, 2],
};

test("type mapper covers bank function types for C, C++ and Java", () => {
  assert.equal(compilerType("integer[]", "c"), "int*");
  assert.equal(compilerType("string[]", "c"), "char**");
  assert.equal(compilerType("integer[]", "cpp"), "vector<int>");
  assert.equal(compilerType("integer[]", "java"), "int[]");
  assert.equal(
    compilerType("list<list<string>>", "cpp"),
    "vector<vector<string>>",
  );
  assert.equal(
    compilerType("list<list<string>>", "java"),
    "List<List<String>>",
  );

  for (const type of bankFunctionTypes()) {
    if (type === "void" || type === "Node") continue;
    assert.doesNotThrow(() => compilerType(type, "c"), `C type ${type}`);
    assert.doesNotThrow(() => compilerType(type, "cpp"), `C++ type ${type}`);
    assert.doesNotThrow(() => compilerType(type, "java"), `Java type ${type}`);
  }
});

test("literal emitter covers every non-void function type in the bank", () => {
  assert.equal(nativeLiteral([1, 2], "integer[]", "cpp"), "{1, 2}");
  assert.equal(nativeLiteral([1, 2], "integer[]", "c"), "{1, 2}");
  assert.equal(nativeLiteral([1, 2], "integer[]", "java"), "new int[]{1, 2}");
  assert.equal(
    nativeLiteral([["a"]], "character[][]", "java"),
    "new char[][]{new char[]{'a'}}",
  );
  assert.equal(
    nativeLiteral([1, null, 2], "TreeNode", "java"),
    "treeNode(new Integer[]{1, null, 2})",
  );
  assert.equal(nativeLiteral(2147483648, "double", "java"), "2147483648.0");
  assert.equal(nativeLiteral(1.5, "double", "java"), "1.5");
  assert.equal(nativeLiteral(1e21, "double", "java"), "1e+21");
  assert.equal(
    nativeLiteral([2, 0.5], "double[]", "java"),
    "new double[]{2.0, 0.5}",
  );

  for (const type of bankFunctionTypes()) {
    if (type === "void" || type === "Node") continue;
    const value = sampleValues[type];
    assert.notEqual(value, undefined, `missing sample for ${type}`);
    assert.doesNotThrow(
      () => nativeLiteral(value, type, "c"),
      `C literal ${type}`,
    );
    assert.doesNotThrow(
      () => nativeLiteral(value, type, "cpp"),
      `C++ literal ${type}`,
    );
    assert.doesNotThrow(
      () => nativeLiteral(value, type, "java"),
      `Java literal ${type}`,
    );
  }
  assert.throws(
    () => nativeLiteral([[2], [1]], "Node", "cpp"),
    /argTypes preparation/,
  );
});

test("C string literals hand the candidate's text over unchanged", () => {
  // `-std=c17` keeps trigraphs: "??!" would compile to "|". C++ and Java read
  // the JSON form as typed, so only C is escaped.
  assert.equal(nativeLiteral("abc??!def", "string", "c"), '"abc\\?\\?!def"');
  assert.equal(nativeLiteral(["ab??=cd"], "string[]", "c"), '{"ab\\?\\?=cd"}');
  assert.equal(nativeLiteral("abc??!def", "string", "cpp"), '"abc??!def"');
  assert.equal(nativeLiteral("abc??!def", "string", "java"), '"abc??!def"');
  // The escapes C shares with JSON keep their spelling.
  assert.equal(
    nativeLiteral('tab\tline\n"q"\\', "string", "c"),
    '"tab\\tline\\n\\"q\\"\\\\"',
  );
  // C17 refuses JSON's `\u0001` as a universal character name; octal stops at
  // three digits, so the "7" after `\u001f` stays its own character, and a
  // typed backslash before "u0001" is not read as an escape.
  assert.equal(nativeLiteral("a\u0001b", "string", "c"), '"a\\001b"');
  assert.equal(nativeLiteral("\u001f7", "string", "c"), '"\\0377"');
  assert.equal(nativeLiteral("\\u0001", "string", "c"), '"\\\\u0001"');
});

test("C harness refuses a string case holding NUL rather than truncating it", () => {
  // `\000` compiles, but the solution's string ends there, so the case would
  // run on "a" alone. The refusal names the case, as the workspace does.
  const spec = {
    ...judges["longest-substring-without-repeating-characters"],
    cases: [
      { input: ["ab"], expected: 2 },
      { input: ["a\u0000b"], expected: 3, label: "Your case 1" },
    ],
  };
  assert.throws(
    () => generateHarness("c", spec, ""),
    /^Error: Your case 1 holds a NUL character/,
  );
  // A typed backslash before "u0000" is text, not NUL.
  assert.doesNotThrow(() =>
    generateHarness("c", { ...spec, cases: [{ input: ["\\u0000"] }] }, ""),
  );
  // Word Search II takes a `character[][]` board and a `string[]` of words.
  // A NUL item in the words truncates; a NUL cell is a raw byte between
  // quotes, which compiles and reaches the solution as 0.
  const grid = (board, words) => ({
    ...judges["word-search-ii"],
    cases: [{ input: [board, words], expected: [] }],
  });
  assert.throws(
    () => generateHarness("c", grid([["a"]], ["x", "a\u0000"]), ""),
    /^Error: Case 1 holds a NUL character/,
  );
  assert.doesNotThrow(() =>
    generateHarness("c", grid([["a", "\u0000"]], ["ab"]), ""),
  );
  // Evaluate Division nests its strings two lists deep.
  assert.throws(
    () =>
      generateHarness(
        "c",
        {
          ...judges["evaluate-division"],
          cases: [
            {
              input: [[["a", "b\u0000"]], [2], [["a", "b"]]],
              expected: [2],
            },
          ],
        },
        "",
      ),
    /^Error: Case 1 holds a NUL character/,
  );
});

test("harness generator builds one batched function harness", () => {
  const spec = judges["two-sum"];
  const c = generateHarness(
    "c",
    spec,
    `int* matchDisputedCharge(int* nums, int numsSize, int target, int* returnSize) {
    return NULL;
  }`,
  );
  const cpp = generateHarness(
    "cpp",
    spec,
    "class Solution { public: vector<int> matchDisputedCharge(vector<int>& nums, int target) { return {0, 1}; } };",
  );
  const java = generateHarness(
    "java",
    spec,
    "class Solution { public int[] matchDisputedCharge(int[] nums, int target) { return new int[]{0, 1}; } }",
  );

  assert.match(c, /#include <stdbool\.h>/);
  assert.match(
    c,
    /int\* matchDisputedCharge\(int\* nums, int numsSize, int target, int\* returnSize\)/,
  );
  assert.match(c, /json_int_array\(stdout, actual, returnSize\)/);
  assert.match(c, /results/);
  assert.match(cpp, /class Solution/);
  assert.match(cpp, /#include <unordered_map>/);
  assert.match(cpp, /set<ListNode\*> seen/);
  assert.match(cpp, /solution\.matchDisputedCharge\(nums, target\)/);
  assert.match(cpp, /chrono::steady_clock/);
  assert.match(cpp, /results/);
  assert.match(java, /class Main/);
  assert.match(java, /ListNode\(int val, ListNode next\)/);
  assert.match(java, /TreeNode\(int val, TreeNode left, TreeNode right\)/);
  assert.match(java, /Node\(int val, Node left, Node right, Node next\)/);
  assert.match(java, /solution\.matchDisputedCharge\(nums, target\)/);
  assert.match(java, /System\.nanoTime/);
  assert.match(java, /results/);
});

// A string answer holding a newline or a tab used to emit invalid JSON, so the
// whole run came back as "no JSON results" instead of a test failure. Same for
// a non-finite double, which has no JSON spelling at all.
test("generated harnesses emit JSON for control characters and non-finite doubles", () => {
  const spec = judges["two-sum"];
  const cpp = generateHarness("cpp", spec, "class Solution {};");
  const java = generateHarness("java", spec, "class Solution {}");

  assert.match(cpp, /c < 0x20/, "C++ must escape control characters");
  assert.match(cpp, /if \(!isfinite\(value\)\) return "null"/);
  assert.match(
    cpp,
    /setprecision\(17\)/,
    "6 significant digits mis-compares doubles",
  );
  assert.match(cpp, /#include <iomanip>/);
  assert.match(java, /c < 0x20/, "Java must escape control characters");
  assert.match(
    java,
    /Double\.isFinite\(value\) \? String\.valueOf\(value\) : "null"/,
  );
});

test("harness generator covers every function problem without network access", () => {
  for (const [id, spec] of Object.entries(judges)) {
    if (spec.kind !== "function") continue;
    assert.doesNotThrow(
      () => generateHarness("c", spec, "int placeholder(void) { return 0; }"),
      `C harness for ${id}`,
    );
    assert.doesNotThrow(
      () => generateHarness("cpp", spec, "class Solution {};"),
      `C++ harness for ${id}`,
    );
    assert.doesNotThrow(
      () => generateHarness("java", spec, "class Solution {}"),
      `Java harness for ${id}`,
    );
  }
});

// The C starters describe struct Node in a comment, and the harness declares
// the struct the code is compiled against. Nothing ties the two copies
// together, so a field named in one and not the other only shows up as a
// compile error for a candidate who wrote what the comment said, as the
// graph clone starter's numNeighbors once did against a harness that
// named the field neighborsSize.
test("every struct Node field a C starter documents is one the harness declares", () => {
  const fieldsOf = (body) =>
    body
      .split(";")
      .map((declaration) => declaration.trim())
      .filter(Boolean)
      .map((declaration) => {
        const name = declaration.match(/^\w+(?:\s+\w+)?[\s*]+(\w+)$/)?.[1];
        assert.ok(
          name,
          `cannot read a struct Node field from "${declaration}"`,
        );
        return name;
      });
  const harness = generateHarness("c", judges["clone-graph"], "").match(
    /struct\s+Node\s*\{([^}]*)\}/,
  );
  assert.ok(harness, "the C harness no longer declares struct Node");
  const declared = new Set(fieldsOf(harness[1]));

  for (const [id, { page }] of Object.entries(pageMap)) {
    const starter = JSON.parse(
      readFileSync(webFile(`problems/${page}.json`), "utf8"),
    ).starterCode.c;
    const usesNode = [
      ...(judges[id].paramTypes ?? []),
      judges[id].returnType,
    ].includes("Node");
    if (!usesNode) continue;
    // Drop the comment's leading asterisks first, so a brace on its own line
    // still reads as part of the struct.
    const comment = starter
      ?.replace(/^\s*\*/gm, "")
      .match(/struct\s+Node\s*\{([^}]*)\}/);
    assert.ok(comment, `${page}: the C starter does not document struct Node`);
    for (const field of fieldsOf(comment[1])) {
      assert.ok(
        declared.has(field),
        `${page}: the C starter documents struct Node field ${field}, which the harness does not declare`,
      );
    }
  }
});

// The C++ counterpart of the test above: starters describe ListNode, TreeNode
// and Node in a comment, and the harness declares what the code compiles
// against. A constructor drifts too: three tree starters once documented
// `TreeNode(int x) : val(x), left(nullptr) {}`, leaving `right` indeterminate.
const cppNodeClasses = ["ListNode", "TreeNode", "Node"];

function closingIndex(text, open) {
  const [left, right] = text[open] === "(" ? ["(", ")"] : ["{", "}"];
  let depth = 0;
  for (let index = open; index < text.length; index += 1) {
    if (text[index] === left) depth += 1;
    else if (text[index] === right && --depth === 0) return index;
  }
  return -1;
}

// The pieces of `text` between top-level separators, untrimmed, so a caller
// can tell where the first one ends.
function splitTopLevel(text, separator, opens = "({[", closes = ")}]") {
  const parts = [];
  let depth = 0;
  let start = 0;
  for (let index = 0; index < text.length; index += 1) {
    if (opens.includes(text[index])) depth += 1;
    else if (closes.includes(text[index])) depth -= 1;
    else if (text[index] === separator && depth === 0) {
      parts.push(text.slice(start, index));
      start = index + 1;
    }
  }
  return [...parts, text.slice(start)];
}

// A declaration's type and name. The last word is the name only when a type
// precedes it, so `TreeNode(int)` has an unnamed int. Spaces stay only between
// words: `TreeNode *left` reads as `TreeNode*`, `unsigned int` keeps its space.
const cppBuiltins = new Set([
  "bool",
  "char",
  "double",
  "float",
  "int",
  "long",
  "short",
  "signed",
  "unsigned",
]);
function cppDeclaration(text) {
  const tokens = text.match(/\w+|[^\w\s]/g) ?? [];
  const last = tokens.at(-1);
  const named =
    tokens.length > 1 && /^\w+$/.test(last) && !cppBuiltins.has(last);
  const type = (named ? tokens.slice(0, -1) : tokens).reduce(
    (out, token) =>
      out + (/\w$/.test(out) && /^\w/.test(token) ? " " : "") + token,
    "",
  );
  return { type, name: named ? last : null };
}

// The members and constructors of class `name` as `source` declares it, or
// null when it does not. A constructor records the parameter types it takes,
// how many of them have defaults, and the members it sets: each top-level
// initializer, and each body statement that assigns a member rather than a
// local. One whose only initializer is another constructor counts as setting
// every member, since that constructor is checked on its own.
function cppClass(source, name, where) {
  const text = source.replace(/\/\/[^\n]*/g, " ");
  const head = text.match(new RegExp(`\\b(?:struct|class)\\s+${name}\\s*\\{`));
  if (!head) return null;
  const open = head.index + head[0].length - 1;
  const close = closingIndex(text, open);
  assert.ok(close > open, `${where}: ${name} has no closing brace`);
  const body = text
    .slice(open + 1, close)
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/\b(?:public|private|protected)\s*:/g, " ");
  const shape = `${where}: only \`${name}(params) : member(value), ... { member = value; }\` constructors and \`type member;\` members of ${name} can be read`;

  const fields = new Map();
  const defaulted = new Set();
  const constructors = [];
  const constructor = new RegExp(`^${name}\\s*\\(`);
  let at = 0;
  const skip = () => {
    while (/\s/.test(body[at] ?? "")) at += 1;
  };
  for (skip(); at < body.length; skip()) {
    const found = body.slice(at).match(constructor);
    if (!found) {
      const [declaration] = splitTopLevel(body.slice(at), ";");
      assert.ok(at + declaration.length < body.length, shape);
      at += declaration.length + 1;
      // A stray `;`, as after `TreeNode() {};`, declares nothing.
      if (!declaration.trim()) continue;
      const bare = declaration.replace(/\s*(?:=[\s\S]*|\{[\s\S]*\})$/, "");
      const { type, name: field } = cppDeclaration(bare);
      assert.ok(field && type, `${shape}, not "${declaration.trim()}"`);
      fields.set(field, type);
      if (bare !== declaration) defaulted.add(field);
      continue;
    }
    const paramsOpen = at + found[0].length - 1;
    const paramsClose = closingIndex(body, paramsOpen);
    assert.ok(paramsClose > 0, shape);
    const params = splitTopLevel(
      body.slice(paramsOpen + 1, paramsClose),
      ",",
      "(<{[",
      ")>}]",
    )
      .filter((param) => param.trim())
      .map((param) => {
        const [declaration, fallback] = param.split("=");
        return {
          ...cppDeclaration(declaration),
          optional: fallback !== undefined,
        };
      });
    at = paramsClose + 1;
    skip();
    const initialized = [];
    if (body[at] === ":") {
      do {
        at += 1;
        skip();
        const member = body.slice(at).match(/^\w+/)?.[0] ?? "";
        at += member.length;
        skip();
        const argsClose = "({".includes(body[at]) ? closingIndex(body, at) : -1;
        assert.ok(member && argsClose > 0, shape);
        initialized.push(member);
        at = argsClose + 1;
        skip();
      } while (body[at] === ",");
    }
    const bodyClose = body[at] === "{" ? closingIndex(body, at) : -1;
    assert.ok(bodyClose > 0, shape);
    const assigned = new Set();
    // A parameter shadows a member of its name, so `right = right` sets
    // nothing; only `this->right` reaches the member.
    const locals = new Set(params.map((param) => param.name).filter(Boolean));
    for (const statement of splitTopLevel(body.slice(at + 1, bodyClose), ";")) {
      // `left = right = nullptr` assigns every name before the last `=`; a
      // statement with none, such as `TreeNode *left;`, only declares.
      const parts = statement.split(/(?<![=!<>])=(?!=)/);
      const targets = parts.length > 1 ? parts.slice(0, -1) : [];
      if (!targets.length) {
        const local = cppDeclaration(statement).name;
        if (local) locals.add(local);
      }
      for (const target of targets.map((part) => part.trim())) {
        const member = target.match(/^this\s*->\s*(\w+)$/)?.[1];
        if (member) assigned.add(member);
        else if (/^\w+$/.test(target)) {
          if (!locals.has(target)) assigned.add(target);
        } else {
          const local = cppDeclaration(target).name;
          if (local) locals.add(local);
        }
      }
    }
    at = bodyClose + 1;
    const delegates = initialized.length === 1 && initialized[0] === name;
    constructors.push({
      types: params.map((param) => param.type),
      required: params.filter((param) => !param.optional).length,
      sets: (field) =>
        delegates || initialized.includes(field) || assigned.has(field),
    });
  }
  return { fields, defaulted, constructors };
}

// What every constructor has to do, in the harness and in a comment alike:
// start each pointer member somewhere, which the harness does, so a comment
// that leaves one out describes code the candidate is not compiled against.
function assertPointersSet(where, name, { fields, defaulted, constructors }) {
  const pointers = [...fields]
    .filter(([field, type]) => type.endsWith("*") && !defaulted.has(field))
    .map(([field]) => field);
  assert.ok(
    constructors.length || !pointers.length,
    `${where}: ${name} declares no constructor, so ${pointers.join(", ")} start indeterminate`,
  );
  for (const { types, sets } of constructors) {
    for (const pointer of pointers) {
      assert.ok(
        sets(pointer),
        `${where}: ${name}(${types.join(", ")}) leaves ${name}::${pointer} uninitialized`,
      );
    }
  }
}

// A documented member has the harness's type, and every call a documented
// constructor accepts, from its required parameters up to all of them,
// compiles against some harness constructor.
function assertDocumentedAsDeclared(where, name, documented, declared) {
  for (const [field, type] of documented.fields) {
    assert.equal(
      declared.fields.get(field),
      type,
      `${where}: the C++ starter documents ${name}::${field} as ${type}, which the harness does not declare`,
    );
  }
  for (const { types, required } of documented.constructors) {
    for (let arity = required; arity <= types.length; arity += 1) {
      const call = types.slice(0, arity);
      assert.ok(
        declared.constructors.some(
          (candidate) =>
            arity >= candidate.required &&
            arity <= candidate.types.length &&
            call.every((type, index) => candidate.types[index] === type),
        ),
        `${where}: the C++ starter documents ${name}(${call.join(", ")}), which no harness constructor accepts`,
      );
    }
  }
  assertPointersSet(where, name, documented);
}

test("the C++ node class reader catches the drift it compares", () => {
  const tree = (constructors) =>
    cppClass(
      `struct TreeNode { int val; TreeNode *left; TreeNode *right; ${constructors} };`,
      "TreeNode",
      "inline",
    );
  const check = (constructors) => () =>
    assertPointersSet("inline", "TreeNode", tree(constructors));
  const unset = /TreeNode::right uninitialized/;
  assert.throws(check("TreeNode(int x) : val(x), left(nullptr) {}"), unset);
  assert.throws(
    check("TreeNode(int x) : val(x), left(new TreeNode(0)) {}"),
    unset,
  );
  assert.throws(
    check("TreeNode(int x) { left = nullptr; TreeNode *right = nullptr; }"),
    unset,
  );
  assert.doesNotThrow(
    check("TreeNode(int x) : val(x) { left = right = nullptr; }"),
  );
  assert.doesNotThrow(
    check("TreeNode() : val(0) { this->left = this->right = nullptr; }"),
  );
  assert.throws(
    check("TreeNode(int x) : val(x) { left; right; }"),
    /TreeNode::left/,
  );
  assert.throws(
    check("TreeNode() : val(0) { TreeNode *left; left = right = nullptr; }"),
    /TreeNode::left uninitialized/,
  );
  assert.throws(check(""), /TreeNode declares no constructor, so left, right/);
  assert.throws(
    check(
      "TreeNode(TreeNode *left, TreeNode *right) { left = left; right = right; }",
    ),
    /TreeNode::left uninitialized/,
  );
  assert.doesNotThrow(
    check(
      "TreeNode() : val(0), left(nullptr) { this->right = nullptr; } TreeNode(int x) : TreeNode() {}",
    ),
  );

  const braced = cppClass(
    `struct TreeNode {
      unsigned int val; // the key
      TreeNode *left{nullptr};
      TreeNode *right;
      TreeNode(unsigned int, TreeNode *) : val{0}, right{nullptr} {};
    };`,
    "TreeNode",
    "inline",
  );
  assert.deepEqual(Object.fromEntries(braced.fields), {
    val: "unsigned int",
    left: "TreeNode*",
    right: "TreeNode*",
  });
  assert.deepEqual(braced.constructors[0].types, ["unsigned int", "TreeNode*"]);
  assertPointersSet("inline", "TreeNode", braced);

  const list = (constructor) =>
    cppClass(
      `struct ListNode { int val; ListNode *next; ${constructor} };`,
      "ListNode",
      "inline",
    );
  assert.throws(
    () =>
      assertDocumentedAsDeclared(
        "inline",
        "ListNode",
        list("ListNode(int x = 0, ListNode *n = nullptr) : val(x), next(n) {}"),
        list("ListNode(int x, ListNode *n) : val(x), next(n) {}"),
      ),
    /documents ListNode\(\), which no harness/,
  );
});

test("every C++ starter documents ListNode, TreeNode and Node as the harness declares them", () => {
  const harnessSource = generateHarness(
    "cpp",
    judges["clone-graph"],
    "class Solution {};",
  );
  const harness = Object.fromEntries(
    cppNodeClasses.map((name) => {
      const declared = cppClass(harnessSource, name, "the C++ harness");
      assert.ok(declared, `the C++ harness no longer declares ${name}`);
      assertPointersSet("the C++ harness", name, declared);
      return [name, declared];
    }),
  );

  let documented = 0;
  for (const [id, { page }] of Object.entries(pageMap)) {
    const starter =
      JSON.parse(readFileSync(webFile(`problems/${page}.json`), "utf8"))
        .starterCode.cpp ?? "";
    // Drop the comment's leading asterisks, as the C test does.
    const comment = starter.replace(/^\s*\*(?!\/)/gm, "");
    // What the judge passes or returns; `ListNode[]` counts as ListNode, and a
    // class judge's `binaryTree` constructor argument as TreeNode.
    const used = new Set(
      [
        ...(judges[id].paramTypes ?? []),
        judges[id].returnType,
        ...(judges[id].constructorArgTypes ?? []),
      ].map((type) =>
        type === "binaryTree" ? "TreeNode" : type?.replace(/\[\]$/, ""),
      ),
    );
    for (const name of cppNodeClasses) {
      const starterClass = cppClass(comment, name, page);
      if (!starterClass) {
        assert.ok(
          !used.has(name),
          `${page}: the C++ judge uses ${name}, which the starter does not document`,
        );
        continue;
      }
      documented += 1;
      assertDocumentedAsDeclared(page, name, starterClass, harness[name]);
    }
  }
  assert.ok(documented > 0, "no C++ starter documents a node class");
});

test("harness generator covers every class problem without network access", () => {
  for (const [id, spec] of Object.entries(judges)) {
    if (spec.kind !== "class") continue;
    assert.doesNotThrow(
      () => generateHarness("cpp", spec, `class ${spec.className} {};`),
      `C++ class harness for ${id}`,
    );
    assert.doesNotThrow(
      () => generateHarness("java", spec, `class ${spec.className} {}`),
      `Java class harness for ${id}`,
    );
    assert.throws(
      () => generateHarness("c", spec, ""),
      /Only function judges are supported for C/,
    );
  }
});

test("a candidate case without an expected value builds the C++ and Java class harnesses", () => {
  const spec = judges["min-stack"];
  const candidate = {
    label: "Your case 1",
    input: [
      [spec.className, "push", "getMin"],
      [[], [3], []],
    ],
  };
  // Asserted on the generated code, not on the absence of a JS expression that
  // was never going to appear in C++ or Java. What proves the contract was read
  // is that a value-returning method is serialized while a void one is not: the
  // fallback this replaces reads the expected value, which a candidate case
  // does not have, and would call every method a value.
  const serialized = {
    cpp: ["toJson(instance.getMin())", "instance.push(3);"],
    java: ["jsonAny(instance.getMin())", "instance.push(3);"],
  };
  for (const language of ["cpp", "java"]) {
    const harness = generateHarness(
      language,
      { ...spec, cases: [...spec.cases, candidate] },
      `class ${spec.className} {}`,
    );
    for (const fragment of serialized[language]) {
      assert.ok(
        harness.includes(fragment),
        `${language} must emit ${fragment}, which is the return type coming from the judge contract`,
      );
    }
    assert.ok(
      !harness.includes("jsonAny(instance.push(") &&
        !harness.includes("toJson(instance.push("),
      `${language} must not serialize a void method`,
    );
  }
});

// Async subprocesses keep independent fixtures concurrent; a blocking
// `execFileSync` would serialize them. The suite bounds compiler jobs so
// candidates can finish before their execution timeout.
const run = promisify(execFile);

/// Whether a tool answers at all. Probed once per tool rather than once per
/// test, and synchronously, which is safe here because a blocking call cannot
/// interleave with the concurrent bodies below: this all runs before they do.
function present(command, versionArg) {
  try {
    execFileSync(command, [versionArg], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

// Declared, not performed. `node:test` takes `skip` as a reason string in the
// options, so a missing compiler is stated where the test is named and the body
// never runs. The imperative form this replaces called `t.skip()` inside the
// body, which only marks the result: one guard was written without the `return`
// that has to follow it, and that test reported itself skipped and then failed
// on the missing compiler. There is nothing left here to forget.
const noCc = present("cc", "--version") ? false : "cc is not available";
const noGpp = present("g++", "--version") ? false : "g++ is not available";
const noJava =
  present("javac", "-version") && present("java", "-version")
    ? false
    : "javac/java are not available";

describe("toolchain harnesses", { concurrency: 4 }, () => {
  for (const [id, solution] of [
    [
      "reverse-words-in-a-string",
      `static void reverseRange(char* s, int left, int right) {
    while (left < right) {
        char temp = s[left];
        s[left++] = s[right];
        s[right--] = temp;
    }
}
char* restoreCaptionOrder(char* s) {
    int read = 0, write = 0;
    while (s[read]) {
        while (s[read] == ' ') read++;
        if (!s[read]) break;
        if (write) s[write++] = ' ';
        while (s[read] && s[read] != ' ') s[write++] = s[read++];
    }
    s[write] = '\\0';
    reverseRange(s, 0, write - 1);
    int start = 0;
    for (int i = 0; i <= write; i++) {
        if (s[i] == ' ' || s[i] == '\\0') {
            reverseRange(s, start, i - 1);
            start = i + 1;
        }
    }
    return s;
}`,
    ],
    [
      "valid-anagram",
      `static int compareCharacters(const void* a, const void* b) {
    return *(const unsigned char*)a - *(const unsigned char*)b;
}
bool isRearrangedCode(char* s, char* t) {
    size_t length = strlen(s);
    if (length != strlen(t)) return false;
    qsort(s, length, sizeof(*s), compareCharacters);
    qsort(t, length, sizeof(*t), compareCharacters);
    return strcmp(s, t) == 0;
}`,
    ],
  ]) {
    it(
      `C ${id} harness accepts an in-place string solution`,
      { skip: noCc },
      async () => {
        const dir = mkdtempSync(join(tmpdir(), "codetrial-c-string-"));
        try {
          const source = join(dir, "solution.c");
          const binary = join(dir, "solution");
          writeFileSync(source, generateHarness("c", judges[id], solution));
          await run("cc", ["-std=c17", source, "-o", binary]);
          const parsed = parseCompilerResults(
            (await run(binary, [], { encoding: "utf8" })).stdout,
          );
          assert.equal(parsed.setupError, undefined);
          assert.deepEqual(
            parsed.results.map((result) => result.actual),
            judges[id].cases.map((testCase) => testCase.expected),
          );
        } finally {
          rmSync(dir, { recursive: true, force: true });
        }
      },
    );
  }

  for (const value of ["INFINITY", "-INFINITY", "NAN"]) {
    it(
      `C double harness preserves all cases when returning ${value}`,
      { skip: noCc },
      async () => {
        const dir = mkdtempSync(join(tmpdir(), "codetrial-c-double-"));
        try {
          const source = join(dir, "growth.c");
          const binary = join(dir, "growth");
          writeFileSync(
            source,
            generateHarness(
              "c",
              judges["powx-n"],
              `double growthOverPeriods(double x, int n) {
    if (n < 0) return ${value};
    double result = 1.0;
    for (int i = 0; i < n; i++) result *= x;
    return result;
}`,
            ),
          );
          await run("cc", ["-std=c17", source, "-o", binary]);
          const parsed = parseCompilerResults((await run(binary)).stdout);
          assert.equal(parsed.setupError, undefined);
          assert.deepEqual(
            parsed.results.map((result) => result.actual),
            [1024, 9.261000000000001, null, 1, null, null],
          );
        } finally {
          rmSync(dir, { recursive: true, force: true });
        }
      },
    );
  }

  it(
    "C double array harness preserves finite elements beside non-finite values",
    { skip: noCc },
    async () => {
      const dir = mkdtempSync(join(tmpdir(), "codetrial-c-double-array-"));
      try {
        const source = join(dir, "array.c");
        const binary = join(dir, "array");
        const spec = {
          kind: "function",
          entry: "numbers",
          paramNames: [],
          paramTypes: [],
          returnType: "double[]",
          cases: [{ input: [], expected: [1.25, null, null, null, 2.5] }],
        };
        writeFileSync(
          source,
          generateHarness(
            "c",
            spec,
            `double* numbers(int* returnSize) {
    static double values[] = {1.25, INFINITY, -INFINITY, NAN, 2.5};
    *returnSize = 5;
    return values;
}`,
          ),
        );
        await run("cc", ["-std=c17", source, "-o", binary]);
        const parsed = parseCompilerResults((await run(binary)).stdout);
        assert.equal(parsed.setupError, undefined);
        assert.deepEqual(
          parsed.results.map((result) => result.actual),
          [[1.25, null, null, null, 2.5]],
        );
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  it(
    "C Two Sum harness can produce passing Compiler Explorer JSON",
    { skip: noCc },
    async () => {
      const dir = mkdtempSync(join(tmpdir(), "codetrial-c-"));
      try {
        const source = join(dir, "twosum.c");
        const binary = join(dir, "twosum");
        writeFileSync(
          source,
          generateHarness(
            "c",
            judges["two-sum"],
            `int* matchDisputedCharge(int* nums, int numsSize, int target, int* returnSize) {
    int* out = malloc(sizeof(int) * 2);
    for (int i = 0; i < numsSize; i++) {
        for (int j = i + 1; j < numsSize; j++) {
            if (nums[i] + nums[j] == target) {
                out[0] = i;
                out[1] = j;
                *returnSize = 2;
                return out;
            }
        }
    }
    *returnSize = 0;
    return out;
}`,
          ),
        );
        await run("cc", ["-std=c17", source, "-o", binary]);
        assert.deepEqual(
          parseCompilerResults(
            (await run(binary, [], { encoding: "utf8" })).stdout,
          ).results.map((result) => result.actual),
          [
            [0, 1],
            [1, 2],
            [0, 1],
            [0, 2],
            [1, 2],
          ],
        );
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  it(
    "C harness keeps trigraph sequences and control characters in candidate strings",
    { skip: noCc },
    async () => {
      const dir = mkdtempSync(join(tmpdir(), "codetrial-c-"));
      // Symbols are in this problem's domain. Under -std=c17 "??!" read as
      // "|" and "??=" as "#", so the run was 7 and 5 rather than 5 and 4, and
      // a trailing "??/" read as a backslash that escaped the closing quote, so
      // the harness did not compile. Inside a string "??/" only escapes the
      // next character, which can leave the count unchanged. clang refuses
      // JSON's `\u0001` in C17; gcc accepts it.
      const counted = ["abc??!def", "ab??=cd", "ab??/", "a\u0001b"];
      // The echo also takes spellings no count would show: octal stopping at
      // three digits, and a typed backslash before "u0001" or a trigraph.
      const echoed = [...counted, "\u001f7", "\\u0001", "a\\??/b"];
      const results = async (name, spec, solution, texts) => {
        const source = join(dir, `${name}.c`);
        const binary = join(dir, name);
        const cases = texts.map((text) => ({ input: [text], expected: 0 }));
        writeFileSync(
          source,
          generateHarness("c", { ...spec, cases }, solution),
        );
        await run("cc", ["-std=c17", source, "-o", binary]);
        return parseCompilerResults(
          (await run(binary, [], { encoding: "utf8" })).stdout,
        ).results.map((result) => result.actual);
      };
      try {
        assert.deepEqual(
          await results(
            "distinct",
            judges["longest-substring-without-repeating-characters"],
            `int longestDistinctRun(char* s) {
    int last[256], best = 0, start = 0;
    for (int i = 0; i < 256; i++) last[i] = -1;
    for (int i = 0; s[i]; i++) {
        unsigned char c = (unsigned char)s[i];
        if (last[c] >= start) start = last[c] + 1;
        last[c] = i;
        if (i - start + 1 > best) best = i - start + 1;
    }
    return best;
}`,
            counted,
          ),
          [5, 4, 3, 3],
        );
        // A count cannot tell "abc??!def" from "abc??@def"; a solution that
        // returns its input shows every byte arrived as typed.
        assert.deepEqual(
          await results(
            "echo",
            {
              kind: "function",
              entry: "echo",
              checker: "exact",
              paramNames: ["s"],
              paramTypes: ["string"],
              returnType: "string",
            },
            "char* echo(char* s) { return s; }",
            echoed,
          ),
          echoed,
        );
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  // The list<T> spellings reach C through the same pointer-and-returnSize
  // shape as T[], so a correct solution must print what it returned.
  for (const [id, solution] of [
    [
      "average-of-levels-in-binary-tree",
      `double* meanReadingPerHop(struct TreeNode* root, int* returnSize) {
    struct TreeNode* queue[64];
    double* out = malloc(sizeof(double) * 64);
    int head = 0, tail = 0;
    *returnSize = 0;
    if (root) queue[tail++] = root;
    while (head < tail) {
        int count = tail - head;
        double sum = 0;
        for (int i = 0; i < count; i++) {
            struct TreeNode* node = queue[head++];
            sum += node->val;
            if (node->left) queue[tail++] = node->left;
            if (node->right) queue[tail++] = node->right;
        }
        out[(*returnSize)++] = sum / count;
    }
    return out;
}`,
    ],
    [
      "spiral-matrix",
      `int* clockwiseScanOrder(int** matrix, int matrixSize, int* matrixColSize, int* returnSize) {
    int rows = matrixSize, cols = matrixSize ? matrixColSize[0] : 0;
    int* out = malloc(sizeof(int) * (rows * cols + 1));
    int top = 0, bottom = rows - 1, left = 0, right = cols - 1;
    *returnSize = 0;
    while (top <= bottom && left <= right) {
        for (int j = left; j <= right; j++) out[(*returnSize)++] = matrix[top][j];
        for (int i = top + 1; i <= bottom; i++) out[(*returnSize)++] = matrix[i][right];
        if (top < bottom)
            for (int j = right - 1; j >= left; j--) out[(*returnSize)++] = matrix[bottom][j];
        if (left < right)
            for (int i = bottom - 1; i > top; i--) out[(*returnSize)++] = matrix[i][left];
        top++, bottom--, left++, right--;
    }
    return out;
}`,
    ],
    [
      "binary-tree-right-side-view",
      `int* sidebarLabels(struct TreeNode* root, int* returnSize) {
    struct TreeNode* queue[64];
    int* out = malloc(sizeof(int) * 64);
    int head = 0, tail = 0;
    *returnSize = 0;
    if (root) queue[tail++] = root;
    while (head < tail) {
        int count = tail - head;
        for (int i = 0; i < count; i++) {
            struct TreeNode* node = queue[head++];
            if (i == count - 1) out[(*returnSize)++] = node->val;
            if (node->left) queue[tail++] = node->left;
            if (node->right) queue[tail++] = node->right;
        }
    }
    return out;
}`,
    ],
    [
      "substring-with-concatenation-of-all-words",
      `int* findChunkBlockOffsets(char* s, char** words, int wordsSize, int* returnSize) {
    int length = strlen(s), width = strlen(words[0]);
    int span = width * wordsSize;
    int* out = malloc(sizeof(int) * (length + 1));
    *returnSize = 0;
    for (int start = 0; start + span <= length; start++) {
        bool used[64] = {false};
        int matched = 0;
        for (int chunk = 0; chunk < wordsSize; chunk++) {
            const char* piece = s + start + chunk * width;
            int found = -1;
            for (int w = 0; w < wordsSize && found < 0; w++)
                if (!used[w] && strncmp(piece, words[w], width) == 0) found = w;
            if (found < 0) break;
            used[found] = true;
            matched++;
        }
        if (matched == wordsSize) out[(*returnSize)++] = start;
    }
    return out;
}`,
    ],
  ]) {
    it(
      `C harness prints the ${judges[id].returnType} that ${id} returns`,
      { skip: noCc },
      async () => {
        const dir = mkdtempSync(join(tmpdir(), "codetrial-c-"));
        try {
          const source = join(dir, "list.c");
          const binary = join(dir, "list");
          writeFileSync(source, generateHarness("c", judges[id], solution));
          await run("cc", ["-std=c17", source, "-o", binary]);
          assert.deepEqual(
            parseCompilerResults(
              (await run(binary, [], { encoding: "utf8" })).stdout,
            ).results.map((result) => result.actual),
            judges[id].cases.map((testCase) => testCase.expected),
          );
        } finally {
          rmSync(dir, { recursive: true, force: true });
        }
      },
    );
  }

  // A degree-three node distinguishes rotation from simple reversal.
  const cloneGraphSpec = {
    ...judges["clone-graph"],
    cases: [
      ...judges["clone-graph"].cases,
      {
        label: "three-neighbor star",
        input: [[[2, 3, 4], [1], [1], [1]]],
        expected: [[2, 3, 4], [1], [1], [1]],
      },
    ],
  };

  for (const [order, neighborIndex] of [
    ["original neighbor order", "i"],
    ["reversed neighbors", "s->numNeighbors - 1 - i"],
    [
      "reversed neighbors on odd nodes",
      "s->val % 2 ? s->numNeighbors - 1 - i : i",
    ],
    ["rotated neighbors", "(i + 1) % s->numNeighbors"],
  ]) {
    it(
      `C Clone Graph harness accepts numNeighbors with ${order}`,
      { skip: noCc },
      async () => {
        const dir = mkdtempSync(join(tmpdir(), "codetrial-c-"));
        try {
          const source = join(dir, "clonegraph.c");
          const binary = join(dir, "clonegraph");
          writeFileSync(
            source,
            generateHarness(
              "c",
              cloneGraphSpec,
              `static struct Node* copies[101];
static struct Node* copyNode(struct Node* s) {
    if (!s) return NULL;
    if (copies[s->val]) return copies[s->val];
    struct Node* copy = calloc(1, sizeof(*copy));
    copy->val = s->val;
    copies[s->val] = copy;
    copy->numNeighbors = s->numNeighbors;
    copy->neighbors = calloc(s->numNeighbors + 1, sizeof(*copy->neighbors));
    for (int i = 0; i < s->numNeighbors; i++)
        copy->neighbors[i] = copyNode(s->neighbors[${neighborIndex}]);
    return copy;
}
struct Node* replicateTopology(struct Node* s) {
    memset(copies, 0, sizeof(copies));
    return copyNode(s);
}`,
            ),
          );
          await run("cc", ["-std=c17", source, "-o", binary]);
          assert.deepEqual(
            parseCompilerResults(
              (await run(binary, [], { encoding: "utf8" })).stdout,
            ).results.map((result) => result.actual),
            cloneGraphSpec.cases.map((testCase) => testCase.expected),
          );
        } finally {
          rmSync(dir, { recursive: true, force: true });
        }
      },
    );
  }

  it(
    "C Clone Graph harness rejects original nodes",
    { skip: noCc },
    async () => {
      const dir = mkdtempSync(join(tmpdir(), "codetrial-c-"));
      try {
        const source = join(dir, "clonegraph-original.c");
        const binary = join(dir, "clonegraph-original");
        writeFileSync(
          source,
          generateHarness(
            "c",
            cloneGraphSpec,
            "struct Node* replicateTopology(struct Node* s) { return s; }",
          ),
        );
        await run("cc", ["-std=c17", source, "-o", binary]);
        // A refusal is the case's error, not a string the candidate returned:
        // as an actual, a case without an expected value showed it as OUTPUT.
        assert.deepEqual(
          parseCompilerResults(
            (await run(binary, [], { encoding: "utf8" })).stdout,
          ).results.map(({ actual, error }) =>
            error === undefined ? { actual } : { error },
          ),
          cloneGraphSpec.cases.map((testCase) =>
            testCase.input[0].length
              ? { error: "Returned graph must be a deep copy." }
              : { actual: [] },
          ),
        );
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  // A non-root NULL must be rejected before any adjacency rows are emitted.
  for (const [position, nodeValue] of [
    ["the root node", 1],
    ["a reachable non-root node", 2],
  ]) {
    it(
      `C Clone Graph harness reports a NULL neighbor on ${position}`,
      { skip: noCc },
      async () => {
        const dir = mkdtempSync(join(tmpdir(), "codetrial-c-graph-null-"));
        try {
          const source = join(dir, "clonegraph-null.c");
          const binary = join(dir, "clonegraph-null");
          writeFileSync(
            source,
            generateHarness(
              "c",
              cloneGraphSpec,
              `static struct Node* copies[101];
static struct Node* copyNode(struct Node* s) {
    if (!s) return NULL;
    if (copies[s->val]) return copies[s->val];
    struct Node* copy = calloc(1, sizeof(*copy));
    copy->val = s->val;
    copies[s->val] = copy;
    copy->numNeighbors = s->numNeighbors;
    copy->neighbors = calloc(s->numNeighbors, sizeof(*copy->neighbors));
    for (int i = 0; i < s->numNeighbors; i++) {
        if (s->val == ${nodeValue} && i == s->numNeighbors - 1) continue;
        copy->neighbors[i] = copyNode(s->neighbors[i]);
    }
    return copy;
}
struct Node* replicateTopology(struct Node* s) {
    memset(copies, 0, sizeof(copies));
    return copyNode(s);
}`,
            ),
          );
          await run("cc", ["-std=c17", source, "-o", binary]);
          assert.deepEqual(
            parseCompilerResults(
              (await run(binary, [], { encoding: "utf8" })).stdout,
            ).results.map(({ actual, error }) =>
              error === undefined ? { actual } : { error },
            ),
            cloneGraphSpec.cases.map((testCase) =>
              testCase.input[0][nodeValue - 1]?.length
                ? { error: "Returned graph has a NULL neighbor." }
                : { actual: testCase.expected },
            ),
          );
        } finally {
          rmSync(dir, { recursive: true, force: true });
        }
      },
    );
  }

  for (const [behavior, nullNeighborValue] of [
    ["accepts reordered neighbors", null],
    ["reports a NULL neighbor on the root node", 1],
    ["reports a NULL neighbor on a reachable non-root node", 2],
  ]) {
    it(`C++ Clone Graph harness ${behavior}`, { skip: noGpp }, async () => {
      const dir = mkdtempSync(join(tmpdir(), "codetrial-cpp-graph-"));
      try {
        const source = join(dir, "clonegraph.cpp");
        const binary = join(dir, "clonegraph");
        writeFileSync(
          source,
          generateHarness(
            "cpp",
            cloneGraphSpec,
            `class Solution {
    map<Node*, Node*> copies;
    Node* copyNode(Node* node) {
        if (!node) return nullptr;
        if (copies.count(node)) return copies[node];
        auto* copy = new Node(node->val);
        copies[node] = copy;
        copy->neighbors.resize(node->neighbors.size(), nullptr);
        for (size_t i = 0; i < node->neighbors.size(); i++) {
            if (node->val == ${nullNeighborValue ?? 0} && i + 1 == node->neighbors.size()) continue;
            copy->neighbors[i] = copyNode(node->neighbors[node->neighbors.size() - 1 - i]);
        }
        return copy;
    }
public:
    Node* replicateTopology(Node* node) { return copyNode(node); }
};`,
          ),
        );
        await run("g++", ["-std=c++20", source, "-o", binary]);
        const { results } = parseCompilerResults(
          (await run(binary, [], { encoding: "utf8" })).stdout,
        );
        assert.equal(results.length, cloneGraphSpec.cases.length);
        cloneGraphSpec.cases.forEach((testCase, index) => {
          const result = results[index];
          if (
            nullNeighborValue !== null &&
            testCase.input[0][nullNeighborValue - 1]?.length
          ) {
            assert.equal(
              result.error,
              "Returned graph has a NULL neighbor.",
              testCase.label,
            );
            assert.equal(result.actual, undefined, testCase.label);
          } else {
            assert.equal(result.error, undefined, testCase.label);
            assert.deepEqual(result.actual, testCase.expected, testCase.label);
          }
        });
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    });
  }

  it(
    "C next-pointer tree harness checks every next pointer, up to 6000 nodes",
    { skip: noCc },
    async () => {
      const dir = mkdtempSync(join(tmpdir(), "codetrial-c-next-"));
      const judge = judges["populating-next-right-pointers-in-each-node-ii"];
      const completeTree = (label, size, value = (n) => n) => {
        const levels = [];
        for (let first = 1; first <= size; first *= 2) {
          const count = Math.min(first * 2, size + 1) - first;
          levels.push(
            Array.from({ length: count }, (_, i) => value(first + i)),
          );
        }
        const input = [Array.from({ length: size }, (_, i) => value(i + 1))];
        return { label, input, expected: levels };
      };
      // 64 nodes fill the harness's first queue exactly, so they never need
      // it to grow, and all of them hold the same value, so only a node's
      // address tells a repeat from a sibling. A candidate case can be as
      // large as the problem's 6000-node limit, past what a fixed 4096-entry
      // queue holds.
      const exactFit = completeTree("exact fit", 64, () => 1);
      const spec = {
        ...judge,
        cases: [...judge.cases, exactFit, completeTree("full-size tree", 6000)],
      };
      // The bottom row's last node can point back to that row's first node
      // instead of NULL, which printing the values alone hides, or take
      // itself as a left child, which passes every next check while the
      // tree never ends.
      const linkRows = (lastNext, lastLeft) => `static struct Node* rows[6000];
struct Node* linkRowNeighbors(struct Node* root) {
    int head = 0, tail = 0;
    if (root) rows[tail++] = root;
    while (head < tail) {
        int end = tail;
        for (int i = head; i < end; i++) {
            rows[i]->next = i + 1 < end ? rows[i + 1] : NULL;
            if (rows[i]->left) rows[tail++] = rows[i]->left;
            if (rows[i]->right) rows[tail++] = rows[i]->right;
        }
        if (tail == end) {
            rows[end - 1]->next = ${lastNext};
            rows[end - 1]->left = ${lastLeft};
        }
        head = end;
    }
    return root;
}`;
      const actuals = async (name, code, flags = []) => {
        const source = join(dir, `${name}.c`);
        const binary = join(dir, name);
        writeFileSync(source, generateHarness("c", spec, code));
        await run("cc", ["-std=c17", ...flags, source, "-o", binary]);
        return parseCompilerResults(
          (await run(binary, [], { encoding: "utf8", timeout: 10000 })).stdout,
        ).results.map((result) =>
          result.error === undefined ? result.actual : { error: result.error },
        );
      };
      try {
        assert.deepEqual(
          await actuals("correct", linkRows("NULL", "NULL")),
          spec.cases.map((testCase) => testCase.expected),
        );
        const mismatch = {
          error: "next pointers do not match the tree level order.",
        };
        assert.deepEqual(
          await actuals("ring", linkRows("rows[head]", "NULL")),
          [mismatch, [], mismatch, mismatch, mismatch, mismatch, mismatch],
        );
        // Only the root's left child is wrong: its next points back at the
        // root instead of across to its sibling. That is an upper level, a
        // pointer that is not NULL, and in the exact fit a node with the
        // sibling's value, so checking only the last pointer of each level,
        // only whether a pointer is NULL, or only values would all miss it.
        const linkAllRows = linkRows("NULL", "NULL").replace(
          "linkRowNeighbors",
          "linkAllRows",
        );
        const interior = `${linkAllRows}
struct Node* linkRowNeighbors(struct Node* root) {
    linkAllRows(root);
    if (root && root->left && root->right) root->left->next = root;
    return root;
}`;
        assert.deepEqual(
          await actuals("interior", interior),
          spec.cases.map((testCase) =>
            testCase.expected[1]?.length === 2 ? mismatch : testCase.expected,
          ),
        );
        const twice = { error: "Returned tree must not reach a node twice." };
        // The root's right child is its left child again, a repeat inside the
        // level being queued rather than one already walked.
        const sharedChild = `${linkAllRows}
struct Node* linkRowNeighbors(struct Node* root) {
    linkAllRows(root);
    if (root && root->left) root->right = root->left;
    return root;
}`;
        assert.deepEqual(
          await actuals("shared-child", sharedChild),
          spec.cases.map((testCase) =>
            testCase.expected.length > 1 ? twice : testCase.expected,
          ),
        );
        // Every realloc in the harness is the queue's, so renaming it to one
        // that always fails takes the full-size case down the grow failure,
        // while the exact fit never asks to grow.
        const failingRealloc = `void* failing_realloc(void* ptr, size_t size) {
    (void)ptr;
    (void)size;
    return NULL;
}
`;
        const noMemory = {
          error: "Out of memory while checking next pointers.",
        };
        assert.deepEqual(
          await actuals(
            "no-memory",
            failingRealloc + linkRows("NULL", "NULL"),
            ["-Drealloc=failing_realloc"],
          ),
          [
            ...judge.cases.map((testCase) => testCase.expected),
            exactFit.expected,
            noMemory,
          ],
        );
        // A repeated child is refused before the queue would grow for it, so
        // the exact fit reports the repeat rather than the failed grow.
        assert.deepEqual(
          await actuals(
            "no-memory-self-child",
            failingRealloc + linkRows("NULL", "rows[end - 1]"),
            ["-Drealloc=failing_realloc"],
          ),
          [twice, [], twice, twice, twice, twice, noMemory],
        );
        // The bottom row's last node takes the root as its left child, a
        // repeat of a level already walked. These repeats run with growth
        // failing first, so a scan that missed one would report memory at 64
        // nodes instead of growing the queue until the run is killed.
        assert.deepEqual(
          await actuals(
            "back-to-root",
            failingRealloc + linkRows("NULL", "root"),
            ["-Drealloc=failing_realloc"],
          ),
          [twice, [], twice, twice, twice, twice, noMemory],
        );
        // With growth working, the full-size tree finds its repeat as well.
        assert.deepEqual(
          await actuals("self-child", linkRows("NULL", "rows[end - 1]")),
          [twice, [], twice, twice, twice, twice, twice],
        );
        // The queue grows and then the level ends cannot, so the failure frees
        // the queue at its new address and the old level ends.
        const levelEndsFail = `#undef realloc
void* realloc(void* ptr, size_t size);
void* failing_realloc(void* ptr, size_t size) {
    static int calls;
    return ++calls % 2 ? realloc(ptr, size) : NULL;
}
`;
        assert.deepEqual(
          await actuals(
            "level-ends-fail",
            levelEndsFail + linkRows("NULL", "NULL"),
            ["-Drealloc=failing_realloc"],
          ),
          [
            ...judge.cases.map((testCase) => testCase.expected),
            exactFit.expected,
            noMemory,
          ],
        );
        // In this problem only json_next_tree calls malloc (nextTree uses
        // calloc), twice per non-empty case: the queue, then the level ends.
        // Failing every odd call loses only the queue and every even call
        // only the level ends, and each loss must be reported by itself; an
        // empty tree never allocates.
        for (const lost of [1, 0]) {
          const failingMalloc = `#undef malloc
void* malloc(size_t size);
void* failing_malloc(size_t size) {
    static int calls;
    return ++calls % 2 == ${lost} ? NULL : malloc(size);
}
`;
          assert.deepEqual(
            await actuals(
              `no-malloc-${lost}`,
              failingMalloc + linkRows("NULL", "NULL"),
              ["-Dmalloc=failing_malloc"],
            ),
            [noMemory, [], noMemory, noMemory, noMemory, noMemory, noMemory],
          );
        }
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  it(
    "C Merge k Sorted Lists harness can produce passing Compiler Explorer JSON",
    { skip: noCc },
    async () => {
      const dir = mkdtempSync(join(tmpdir(), "codetrial-c-"));
      try {
        const source = join(dir, "mergek.c");
        const binary = join(dir, "mergek");
        writeFileSync(
          source,
          generateHarness(
            "c",
            judges["merge-k-sorted-lists"],
            `struct ListNode* combineShardChains(struct ListNode** lists, int listsSize) {
    struct ListNode dummy = {0, NULL};
    struct ListNode* tail = &dummy;
    for (;;) {
        int best = -1;
        for (int i = 0; i < listsSize; i++) {
            if (lists[i] && (best < 0 || lists[i]->val < lists[best]->val)) best = i;
        }
        if (best < 0) break;
        tail->next = lists[best];
        tail = tail->next;
        lists[best] = lists[best]->next;
        tail->next = NULL;
    }
    return dummy.next;
}`,
          ),
        );
        await run("cc", ["-std=c17", source, "-o", binary]);
        assert.deepEqual(
          parseCompilerResults(
            (await run(binary, [], { encoding: "utf8" })).stdout,
          ).results.map((result) => result.actual),
          judges["merge-k-sorted-lists"].cases.map(
            (testCase) => testCase.expected,
          ),
        );
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  it(
    "C Construct Quad Tree harness can produce passing Compiler Explorer JSON",
    { skip: noCc },
    async () => {
      const dir = mkdtempSync(join(tmpdir(), "codetrial-c-quad-"));
      try {
        const source = join(dir, "quad.c");
        const binary = join(dir, "quad");
        writeFileSync(
          source,
          generateHarness(
            "c",
            judges["construct-quad-tree"],
            `static bool same(int** grid, int row, int col, int size) {
    int first = grid[row][col];
    for (int r = row; r < row + size; r++) {
        for (int c = col; c < col + size; c++) {
            if (grid[r][c] != first) return false;
        }
    }
    return true;
}

static struct Node* build(int** grid, int row, int col, int size) {
    struct Node* node = calloc(1, sizeof(*node));
    if (same(grid, row, col, size)) {
        node->val = grid[row][col];
        node->isLeaf = true;
        return node;
    }
    int half = size / 2;
    node->val = 1;
    node->isLeaf = false;
    node->topLeft = build(grid, row, col, half);
    node->topRight = build(grid, row, col + half, half);
    node->bottomLeft = build(grid, row + half, col, half);
    node->bottomRight = build(grid, row + half, col + half, half);
    return node;
}

struct Node* buildTileTree(int** grid, int gridSize, int* gridColSize) {
    (void)gridColSize;
    return build(grid, 0, 0, gridSize);
}`,
          ),
        );
        await run("cc", ["-std=c17", source, "-o", binary]);
        assert.deepEqual(
          parseCompilerResults(
            (await run(binary, [], { encoding: "utf8" })).stdout,
          ).results.map((result) => result.actual),
          judges["construct-quad-tree"].cases.map(
            (testCase) => testCase.expected,
          ),
        );
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  it(
    "C++ BidLedger class harness can produce passing Compiler Explorer JSON",
    { skip: noGpp },
    async () => {
      const dir = mkdtempSync(join(tmpdir(), "codetrial-cpp-class-"));
      try {
        const source = join(dir, "minstack.cpp");
        const binary = join(dir, "minstack");
        writeFileSync(
          source,
          generateHarness(
            "cpp",
            judges["min-stack"],
            `class BidLedger {
    vector<int> values;
    vector<int> minimums;
public:
    BidLedger() {}
    void push(int value) {
        values.push_back(value);
        minimums.push_back(minimums.empty() ? value : min(value, minimums.back()));
    }
    void pop() {
        values.pop_back();
        minimums.pop_back();
    }
    int top() { return values.back(); }
    int getMin() { return minimums.back(); }
};`,
          ),
        );
        await run("g++", ["-std=c++20", source, "-o", binary]);
        assert.deepEqual(
          parseCompilerResults(
            (await run(binary, [], { encoding: "utf8" })).stdout,
          ).results.map((result) => result.actual),
          judges["min-stack"].cases.map((testCase) => testCase.expected),
        );
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  it(
    "Java OrderedCursor class harness can produce passing Compiler Explorer JSON",
    { skip: noJava },
    async () => {
      const dir = mkdtempSync(join(tmpdir(), "codetrial-java-class-"));
      try {
        const source = join(dir, "Main.java");
        writeFileSync(
          source,
          generateHarness(
            "java",
            judges["binary-search-tree-iterator"],
            `class OrderedCursor {
    private final ArrayDeque<TreeNode> stack = new ArrayDeque<>();
    public OrderedCursor(TreeNode root) { pushLeft(root); }
    private void pushLeft(TreeNode node) {
        while (node != null) {
            stack.push(node);
            node = node.left;
        }
    }
    public int next() {
        TreeNode node = stack.pop();
        pushLeft(node.right);
        return node.val;
    }
    public boolean hasNext() { return !stack.isEmpty(); }
}`,
          ),
        );
        await run("javac", [source]);
        assert.deepEqual(
          parseCompilerResults(
            (await run("java", ["-cp", dir, "Main"], { encoding: "utf8" }))
              .stdout,
          ).results.map((result) => result.actual),
          judges["binary-search-tree-iterator"].cases.map(
            (testCase) => testCase.expected,
          ),
        );
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  // 2147483648 is a valid double but too large for an int, and javac refuses
  // it as an int literal even when the variable it initializes is a double.
  it(
    "Java harness passes a whole-number double past the int range",
    { skip: noJava },
    async () => {
      const dir = mkdtempSync(join(tmpdir(), "codetrial-java-double-"));
      try {
        const source = join(dir, "Main.java");
        const spec = judges["powx-n"];
        writeFileSync(
          source,
          generateHarness(
            "java",
            {
              ...spec,
              cases: [{ label: "Your case 1", input: [2147483648, 1] }],
            },
            `class Solution {
    public double ${spec.entry}(double x, int n) { return x; }
}`,
          ),
        );
        await run("javac", [source]);
        assert.deepEqual(
          parseCompilerResults(
            (await run("java", ["-cp", dir, "Main"], { encoding: "utf8" }))
              .stdout,
          ).results.map((result) => result.actual),
          [2147483648],
        );
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  it(
    "C++ Construct Quad Tree harness can produce passing Compiler Explorer JSON",
    { skip: noGpp },
    async () => {
      const dir = mkdtempSync(join(tmpdir(), "codetrial-cpp-quad-"));
      try {
        const source = join(dir, "quad.cpp");
        const binary = join(dir, "quad");
        writeFileSync(
          source,
          generateHarness(
            "cpp",
            judges["construct-quad-tree"],
            `class Solution {
    bool same(vector<vector<int>>& grid, int row, int col, int size) {
        int first = grid[row][col];
        for (int r = row; r < row + size; r++) {
            for (int c = col; c < col + size; c++) {
                if (grid[r][c] != first) return false;
            }
        }
        return true;
    }
    Node* build(vector<vector<int>>& grid, int row, int col, int size) {
        if (same(grid, row, col, size)) return new Node(grid[row][col], true);
        int half = size / 2;
        return new Node(1, false,
            build(grid, row, col, half),
            build(grid, row, col + half, half),
            build(grid, row + half, col, half),
            build(grid, row + half, col + half, half));
    }
public:
    Node* buildTileTree(vector<vector<int>>& grid) {
        return build(grid, 0, 0, grid.size());
    }
};`,
          ),
        );
        await run("g++", ["-std=c++20", source, "-o", binary]);
        assert.deepEqual(
          parseCompilerResults(
            (await run(binary, [], { encoding: "utf8" })).stdout,
          ).results.map((result) => result.actual),
          judges["construct-quad-tree"].cases.map(
            (testCase) => testCase.expected,
          ),
        );
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  it(
    "C++ ThumbnailStore class harness supports standard list-based solutions",
    { skip: noGpp },
    async () => {
      const dir = mkdtempSync(join(tmpdir(), "codetrial-cpp-lru-"));
      try {
        const source = join(dir, "lru.cpp");
        const binary = join(dir, "lru");
        writeFileSync(
          source,
          generateHarness(
            "cpp",
            judges["lru-cache"],
            `class ThumbnailStore {
    int capacity;
    list<pair<int, int>> order;
    unordered_map<int, list<pair<int, int>>::iterator> byKey;
public:
    ThumbnailStore(int capacity) : capacity(capacity) {}
    int get(int key) {
        auto found = byKey.find(key);
        if (found == byKey.end()) return -1;
        order.splice(order.begin(), order, found->second);
        return found->second->second;
    }
    void put(int key, int value) {
        auto found = byKey.find(key);
        if (found != byKey.end()) {
            found->second->second = value;
            order.splice(order.begin(), order, found->second);
            return;
        }
        order.push_front({key, value});
        byKey[key] = order.begin();
        if ((int)order.size() > capacity) {
            byKey.erase(order.back().first);
            order.pop_back();
        }
    }
};`,
          ),
        );
        await run("g++", ["-std=c++20", source, "-o", binary]);
        assert.deepEqual(
          parseCompilerResults(
            (await run(binary, [], { encoding: "utf8" })).stdout,
          ).results.map((result) => result.actual),
          judges["lru-cache"].cases.map((testCase) => testCase.expected),
        );
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  it(
    "C++ EventQueue original class judge runs end to end",
    { skip: noGpp },
    async () => {
      const dir = mkdtempSync(join(tmpdir(), "codetrial-cpp-event-queue-"));
      try {
        const source = join(dir, "event-queue.cpp");
        const binary = join(dir, "event-queue");
        writeFileSync(
          source,
          generateHarness(
            "cpp",
            judges["fixed-capacity-ring-buffer"],
            `class EventQueue {
    vector<int> values;
    int head = 0;
    int tail = 0;
    int count = 0;
public:
    EventQueue(int capacity) : values(capacity) {}
    bool push(int value) {
        if (count == (int)values.size()) return false;
        values[tail] = value;
        tail = (tail + 1) % values.size();
        count++;
        return true;
    }
    int pop() {
        if (count == 0) return -1;
        int value = values[head];
        head = (head + 1) % values.size();
        count--;
        return value;
    }
    int front() { return count == 0 ? -1 : values[head]; }
    int size() { return count; }
};`,
          ),
        );
        await run("g++", ["-std=c++20", source, "-o", binary]);
        assert.deepEqual(
          parseCompilerResults(
            (await run(binary, [], { encoding: "utf8" })).stdout,
          ).results.map((result) => result.actual),
          judges["fixed-capacity-ring-buffer"].cases.map(
            (testCase) => testCase.expected,
          ),
        );
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  // A Java solution returns null for an empty list or tree, and the harness
  // holds the result as an Object, so only the judge's output type can say
  // that null is an empty list rather than a missing answer. Both judges
  // carry a case whose expected answer is [].
  for (const [id, solution] of [
    [
      "rotate-list",
      `class Solution {
    public ListNode wrapPlaylistTail(ListNode head, int k) {
        if (head == null || head.next == null) return head;
        int n = 1;
        ListNode tail = head;
        while (tail.next != null) { tail = tail.next; n++; }
        k %= n;
        if (k == 0) return head;
        ListNode newTail = head;
        for (int i = 0; i < n - k - 1; i++) newTail = newTail.next;
        ListNode newHead = newTail.next;
        newTail.next = null;
        tail.next = head;
        return newHead;
    }
}`,
    ],
    [
      "invert-binary-tree",
      `class Solution {
    public TreeNode mirrorLayout(TreeNode root) {
        if (root == null) return null;
        TreeNode left = mirrorLayout(root.left);
        root.left = mirrorLayout(root.right);
        root.right = left;
        return root;
    }
}`,
    ],
  ]) {
    it(
      `Java ${id} harness reports an empty result as []`,
      { skip: noJava },
      async () => {
        const dir = mkdtempSync(join(tmpdir(), "codetrial-java-empty-"));
        try {
          const source = join(dir, "Main.java");
          writeFileSync(source, generateHarness("java", judges[id], solution));
          await run("javac", [source]);
          assert.deepEqual(
            parseCompilerResults(
              (await run("java", ["-cp", dir, "Main"], { encoding: "utf8" }))
                .stdout,
            ).results.map((result) => result.actual),
            judges[id].cases.map((testCase) => testCase.expected),
          );
        } finally {
          rmSync(dir, { recursive: true, force: true });
        }
      },
    );
  }

  // A merge that links its last node back to the head returns the right values
  // in the right order, so only a serializer that refuses a revisited node can
  // tell it from a correct answer. "both empty" returns no node to link back.
  const cyclicMerges = {
    c: `struct ListNode* interleaveEvents(struct ListNode* list1, struct ListNode* list2) {
    struct ListNode dummy = { 0, NULL };
    struct ListNode* tail = &dummy;
    while (list1 && list2) {
        if (list1->val <= list2->val) { tail->next = list1; list1 = list1->next; }
        else { tail->next = list2; list2 = list2->next; }
        tail = tail->next;
    }
    tail->next = list1 ? list1 : list2;
    struct ListNode* last = dummy.next;
    while (last && last->next) last = last->next;
    if (last) last->next = dummy.next;
    return dummy.next;
}`,
    cpp: `class Solution {
public:
    ListNode* interleaveEvents(ListNode* list1, ListNode* list2) {
        ListNode dummy;
        ListNode* tail = &dummy;
        while (list1 && list2) {
            if (list1->val <= list2->val) { tail->next = list1; list1 = list1->next; }
            else { tail->next = list2; list2 = list2->next; }
            tail = tail->next;
        }
        tail->next = list1 ? list1 : list2;
        ListNode* last = dummy.next;
        while (last && last->next) last = last->next;
        if (last) last->next = dummy.next;
        return dummy.next;
    }
};`,
    java: `class Solution {
    public ListNode interleaveEvents(ListNode list1, ListNode list2) {
        ListNode dummy = new ListNode(0);
        ListNode tail = dummy;
        while (list1 != null && list2 != null) {
            if (list1.val <= list2.val) { tail.next = list1; list1 = list1.next; }
            else { tail.next = list2; list2 = list2.next; }
            tail = tail.next;
        }
        tail.next = list1 != null ? list1 : list2;
        ListNode last = dummy.next;
        while (last != null && last.next != null) last = last.next;
        if (last != null) last.next = dummy.next;
        return dummy.next;
    }
}`,
  };
  const cyclicMergeRuns = {
    c: { skip: noCc, file: "merge.c" },
    cpp: { skip: noGpp, file: "merge.cpp" },
    java: { skip: noJava, file: "Main.java" },
  };
  for (const [language, { skip, file }] of Object.entries(cyclicMergeRuns)) {
    it(
      `${language} harness refuses a returned list that loops back into itself`,
      { skip },
      async () => {
        const dir = mkdtempSync(join(tmpdir(), `codetrial-${language}-cycle-`));
        try {
          const source = join(dir, file);
          const binary = join(dir, "merge");
          writeFileSync(
            source,
            generateHarness(
              language,
              judges["merge-two-sorted-lists"],
              cyclicMerges[language],
            ),
          );
          let command = binary;
          let args = [];
          if (language === "c")
            await run("cc", ["-std=c17", source, "-o", binary]);
          if (language === "cpp")
            await run("g++", ["-std=c++20", source, "-o", binary]);
          if (language === "java") {
            await run("javac", [source]);
            command = "java";
            args = ["-cp", dir, "Main"];
          }
          const { results } = parseCompilerResults(
            (await run(command, args, { encoding: "utf8" })).stdout,
          );
          judges["merge-two-sorted-lists"].cases.forEach((testCase, index) => {
            const result = results[index];
            if (testCase.expected.length === 0) {
              assert.deepEqual(result.actual, [], testCase.label);
              return;
            }
            assert.match(
              result.error ?? "",
              /Returned list must not reach a node twice\./,
              testCase.label,
            );
          });
        } finally {
          rmSync(dir, { recursive: true, force: true });
        }
      },
    );
  }

  // Compiles a candidate into the named judge's harness and returns its
  // results. The timeout keeps a serializer that walks forever from hanging
  // the lane, and the buffer holds the output of the largest tree the bank
  // allows.
  async function runJudge(language, id, candidate) {
    const dir = mkdtempSync(join(tmpdir(), `codetrial-${language}-${id}-`));
    try {
      const source = join(
        dir,
        language === "java" ? "Main.java" : `candidate.${language}`,
      );
      const binary = join(dir, "candidate");
      writeFileSync(source, generateHarness(language, judges[id], candidate));
      if (language === "java") await run("javac", [source]);
      else
        await run(language === "c" ? "cc" : "g++", [
          language === "c" ? "-std=c17" : "-std=c++20",
          source,
          "-o",
          binary,
        ]);
      const options = { encoding: "utf8", timeout: 10000, maxBuffer: 1 << 24 };
      const { stdout } = await (language === "java"
        ? run("java", ["-cp", dir, "Main"], options)
        : run(binary, [], options));
      return parseCompilerResults(stdout).results;
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }
  const flattenLanguages = [
    ["c", noCc],
    ["cpp", noGpp],
    ["java", noJava],
  ];

  // A Morris-style flatten that leaves its last thread in place links the
  // chain back to the root. The serializer used to follow that link forever,
  // so the run timed out or was killed and no case reported anything. The
  // single-node and empty cases have no thread to leave, so they still pass.
  const cyclicFlatten = {
    c: `void linearizeMenu(struct TreeNode* root) {
    struct TreeNode* last = NULL;
    for (struct TreeNode* cur = root; cur; cur = cur->right) {
        if (cur->left) {
            struct TreeNode* pre = cur->left;
            while (pre->right) pre = pre->right;
            pre->right = cur->right;
            cur->right = cur->left;
            cur->left = NULL;
        }
        last = cur;
    }
    if (last && last != root) last->right = root;
}`,
    cpp: `class Solution {
public:
    void linearizeMenu(TreeNode* root) {
        TreeNode* last = nullptr;
        for (TreeNode* cur = root; cur; cur = cur->right) {
            if (cur->left) {
                TreeNode* pre = cur->left;
                while (pre->right) pre = pre->right;
                pre->right = cur->right;
                cur->right = cur->left;
                cur->left = nullptr;
            }
            last = cur;
        }
        if (last && last != root) last->right = root;
    }
};`,
    java: `class Solution {
    public void linearizeMenu(TreeNode root) {
        TreeNode last = null;
        for (TreeNode cur = root; cur != null; cur = cur.right) {
            if (cur.left != null) {
                TreeNode pre = cur.left;
                while (pre.right != null) pre = pre.right;
                pre.right = cur.right;
                cur.right = cur.left;
                cur.left = null;
            }
            last = cur;
        }
        if (last != null && last != root) last.right = root;
    }
}`,
  };
  for (const [language, skip] of flattenLanguages) {
    it(
      `${language} harness refuses a returned tree that reaches a node twice`,
      { skip },
      async () => {
        const judge = judges["flatten-binary-tree-to-linked-list"];
        const results = await runJudge(
          language,
          "flatten-binary-tree-to-linked-list",
          cyclicFlatten[language],
        );
        assert.equal(results.length, judge.cases.length);
        judge.cases.forEach((testCase, index) => {
          const result = results[index];
          if (testCase.input[0].length > 1)
            assert.match(
              result.error ?? "",
              /Returned tree must not reach a node twice\./,
              testCase.label,
            );
          else
            assert.deepEqual(result.actual, testCase.expected, testCase.label);
        });
      },
    );
  }

  // A child on both sides of the root is a second parent rather than a cycle,
  // so a check that only watched the path back to the root would print it.
  // The same shape built from two distinct nodes holding the root's value must
  // still print, which keeps the check keyed on the node rather than on val.
  // Every case with a root gets the shape, and the empty case stays empty.
  const twinChildren = {
    c: (shared) => `void linearizeMenu(struct TreeNode* root) {
    if (!root) return;
    root->left = calloc(1, sizeof(struct TreeNode));
    root->left->val = root->val;
    root->right = ${shared ? "root->left" : "calloc(1, sizeof(struct TreeNode))"};
    root->right->val = root->val;
}`,
    cpp: (shared) => `class Solution {
public:
    void linearizeMenu(TreeNode* root) {
        if (!root) return;
        root->left = new TreeNode(root->val);
        root->right = ${shared ? "root->left" : "new TreeNode(root->val)"};
    }
};`,
    java: (shared) => `class Solution {
    public void linearizeMenu(TreeNode root) {
        if (root == null) return;
        root.left = new TreeNode(root.val);
        root.right = ${shared ? "root.left" : "new TreeNode(root.val)"};
    }
}`,
  };
  for (const [language, skip] of flattenLanguages) {
    for (const shared of [true, false]) {
      it(
        shared
          ? `${language} harness refuses a returned tree whose two children are one node`
          : `${language} harness prints a returned tree whose two children only share a value`,
        { skip },
        async () => {
          const judge = judges["flatten-binary-tree-to-linked-list"];
          const results = await runJudge(
            language,
            "flatten-binary-tree-to-linked-list",
            twinChildren[language](shared),
          );
          assert.equal(results.length, judge.cases.length);
          judge.cases.forEach((testCase, index) => {
            const result = results[index];
            const [value] = testCase.input[0];
            if (value === undefined)
              assert.deepEqual(result.actual, [], testCase.label);
            else if (shared)
              assert.match(
                result.error ?? "",
                /Returned tree must not reach a node twice\./,
                testCase.label,
              );
            else
              assert.deepEqual(
                result.actual,
                [value, value, value],
                testCase.label,
              );
          });
        },
      );
    }
  }

  // A deep copy whose last node links back to the first copies every value
  // and every random pointer, so only a serializer that refuses a revisited
  // node can tell it from a correct answer. "empty random list" has no node.
  const cyclicCopies = {
    c: `struct Node* snapshotOutline(struct Node* head) {
    int n = 0;
    for (struct Node* p = head; p; p = p->next) n++;
    if (!n) return NULL;
    struct Node** copies = calloc(n, sizeof(*copies));
    struct Node* p = head;
    for (int i = 0; i < n; i++, p = p->next) {
        copies[i] = calloc(1, sizeof(*copies[i]));
        copies[i]->val = p->val;
    }
    p = head;
    for (int i = 0; i < n; i++, p = p->next) {
        copies[i]->next = copies[(i + 1) % n];
        struct Node* q = head;
        for (int j = 0; j < n; j++, q = q->next)
            if (q == p->random) copies[i]->random = copies[j];
    }
    struct Node* copy = copies[0];
    free(copies);
    return copy;
}`,
    cpp: `class Solution {
public:
    Node* snapshotOutline(Node* head) {
        if (!head) return nullptr;
        unordered_map<Node*, Node*> copies;
        for (Node* p = head; p; p = p->next) copies[p] = new Node(p->val);
        for (Node* p = head; p; p = p->next) {
            copies[p]->next = p->next ? copies[p->next] : copies[head];
            copies[p]->random = p->random ? copies[p->random] : nullptr;
        }
        return copies[head];
    }
};`,
    java: `class Solution {
    public Node snapshotOutline(Node head) {
        if (head == null) return null;
        java.util.HashMap<Node, Node> copies = new java.util.HashMap<>();
        for (Node p = head; p != null; p = p.next) copies.put(p, new Node(p.val));
        for (Node p = head; p != null; p = p.next) {
            copies.get(p).next = p.next != null ? copies.get(p.next) : copies.get(head);
            copies.get(p).random = p.random != null ? copies.get(p.random) : null;
        }
        return copies.get(head);
    }
}`,
  };
  const cyclicCopyRuns = [
    ["c", noCc],
    ["cpp", noGpp],
    ["java", noJava],
  ];
  for (const [language, skip] of cyclicCopyRuns) {
    it(
      `${language} harness refuses a copied random list that loops back into itself`,
      { skip },
      async () => {
        const spec = judges["copy-list-with-random-pointer"];
        const results = await runJudge(
          language,
          "copy-list-with-random-pointer",
          cyclicCopies[language],
        );
        spec.cases.forEach((testCase, index) => {
          const result = results[index];
          if (testCase.expected.length === 0) {
            assert.deepEqual(result.actual, [], testCase.label);
            return;
          }
          assert.match(
            result.error ?? "",
            /Returned list must not reach a node twice\./,
            testCase.label,
          );
        });
      },
    );
  }

  // C used to print these two refusals where the answer goes, so the case
  // failed as a wrong answer with no reason. C++ and Java throw them, and C
  // reports them through the same error field. A shallow copy that also
  // loops is refused as a shallow copy, as the other four harnesses refuse
  // it, because the original head comes before any revisit.
  const deepCopy = "Returned list must be a deep copy.";
  const strayRandom =
    "Returned random pointer must stay inside the copied list.";
  const strayCopies = [
    {
      name: "a shallow copy",
      message: deepCopy,
      candidate: `struct Node* snapshotOutline(struct Node* head) { return head; }`,
    },
    {
      name: "a shallow copy that loops",
      message: deepCopy,
      candidate: `struct Node* snapshotOutline(struct Node* head) {
    struct Node* tail = head;
    while (tail && tail->next) tail = tail->next;
    if (tail) tail->next = head;
    return head;
}`,
    },
    {
      name: "a random pointer into the original",
      message: strayRandom,
      candidate: `struct Node* snapshotOutline(struct Node* head) {
    struct Node dummy = { 0 };
    struct Node* tail = &dummy;
    for (struct Node* p = head; p; p = p->next) {
        tail->next = calloc(1, sizeof(*tail->next));
        tail = tail->next;
        tail->val = p->val;
        tail->random = p->random;
    }
    return dummy.next;
}`,
    },
  ];
  for (const { name, message, candidate } of strayCopies) {
    it(`c harness reports ${name} as an error`, { skip: noCc }, async () => {
      const spec = judges["copy-list-with-random-pointer"];
      const results = await runJudge(
        "c",
        "copy-list-with-random-pointer",
        candidate,
      );
      spec.cases.forEach((testCase, index) => {
        const result = results[index];
        const nodes = testCase.input[0];
        const strays =
          message === deepCopy
            ? nodes.length > 0
            : nodes.some(([, random]) => random !== null);
        if (!strays) {
          assert.deepEqual(result.actual, testCase.expected, testCase.label);
          return;
        }
        assert.equal(result.actual, null, testCase.label);
        assert.equal(result.error, message, testCase.label);
      });
    });
  }

  // n nodes take 2n + 1 queue slots. The C queue and the visited set sized
  // with it start at 64 slots and double from there, so a 2048-node chain
  // outgrows them several times, and a chain linked back to its root or one
  // whose last node is also the root's left child repeats a node only after
  // that growth. The walk stops past 100000 nodes, the largest tree the bank
  // allows (the subtree lowest-common-ancestor-of-a-binary-tree returns), and
  // the 100000-node chain pins that bound from below inside a timeout that a
  // scan of every dequeued node would overrun. The chain replaces the tree on
  // every case with a root, and the empty case returns nothing to replace.
  for (const [nodes, link, shape] of [
    [2048, "", "chain"],
    [100000, "", "chain"],
    [100001, "", "chain"],
    [2048, "last->right = root;", "chain linked back to its root"],
    [
      2048,
      "root->left = last;",
      "chain whose last node is also the root's left child",
    ],
  ]) {
    const refusal = link
      ? /Returned tree must not reach a node twice\./
      : nodes > 100000
        ? /Returned tree must not have more than 100000 nodes\./
        : null;
    it(
      `c harness ${refusal ? "refuses" : "prints"} a returned ${nodes}-node ${shape}`,
      { skip: noCc },
      async () => {
        const judge = judges["flatten-binary-tree-to-linked-list"];
        const results = await runJudge(
          "c",
          "flatten-binary-tree-to-linked-list",
          `void linearizeMenu(struct TreeNode* root) {
    if (!root) return;
    struct TreeNode* last = root;
    root->left = NULL;
    for (int i = 1; i < ${nodes}; i++) {
        last->right = calloc(1, sizeof(struct TreeNode));
        last = last->right;
    }
    ${link}
}`,
        );
        assert.equal(results.length, judge.cases.length);
        judge.cases.forEach((testCase, index) => {
          const result = results[index];
          if (testCase.input[0].length === 0)
            assert.deepEqual(result.actual, [], testCase.label);
          else if (refusal)
            assert.match(result.error ?? "", refusal, testCase.label);
          else
            assert.equal(
              result.actual?.filter((value) => value !== null).length,
              nodes,
              testCase.label,
            );
        });
      },
    );
  }
});

test("harness generator handles output parameter and output prefix specs", () => {
  const merge = generateHarness(
    "cpp",
    judges["merge-sorted-array"],
    "class Solution { public: void spliceReadings(vector<int>& nums1, int m, vector<int>& nums2, int n) {} };",
  );
  const remove = generateHarness(
    "java",
    judges["remove-element"],
    "class Solution { public int dropMutedCode(int[] nums, int val) { return 0; } }",
  );
  const removeC = generateHarness(
    "c",
    judges["remove-element"],
    "int dropMutedCode(int* nums, int numsSize, int val) { return 0; }",
  );

  assert.match(merge, /solution\.spliceReadings\(nums1, m, nums2, n\)/);
  assert.match(merge, /auto actual = nums1/);
  assert.match(remove, /outputSize < 0 \|\| outputSize > nums\.length/);
  assert.match(remove, /int outputSize = solution\.dropMutedCode\(nums, val\)/);
  assert.match(remove, /Arrays\.copyOf\(nums, outputSize\)/);
  assert.match(
    removeC,
    /actualSize < 0 \|\| actualSize > numsSize[\s\S]*Return value must be an integer length for the kept prefix\./,
  );
});

test("class harness generator handles constructor, mutator, query, and tree constructor calls", () => {
  const minStack = generateHarness(
    "cpp",
    judges["min-stack"],
    "class BidLedger { public: BidLedger() {} void push(int) {} void pop() {} int top() { return 0; } int getMin() { return 0; } };",
  );
  const iterator = generateHarness(
    "java",
    judges["binary-search-tree-iterator"],
    "class OrderedCursor { OrderedCursor(TreeNode root) {} int next() { return 0; } boolean hasNext() { return false; } }",
  );

  assert.match(minStack, /BidLedger instance\{\}/);
  assert.match(minStack, /instance\.push\(-2\)/);
  assert.match(minStack, /instance\.pop\(\);\n    actual\.push_back\("null"\)/);
  assert.match(minStack, /actual\.push_back\(toJson\(instance\.getMin\(\)\)\)/);
  assert.match(
    iterator,
    /new OrderedCursor\(treeNode\(new Integer\[\]\{7, 3, 15, null, null, 9, 20\}\)\)/,
  );
  assert.match(iterator, /actual\.add\(jsonAny\(instance\.hasNext\(\)\)\)/);
});

test("harness generator prepares adapter arguments and mutable char matrices", () => {
  const cList = generateHarness(
    "c",
    judges["merge-two-sorted-lists"],
    "struct ListNode* interleaveEvents(struct ListNode* list1, struct ListNode* list2) { return list1; }",
  );
  const cTree = generateHarness(
    "c",
    judges["lowest-common-ancestor-of-a-binary-tree"],
    "struct TreeNode* nearestSharedApprover(struct TreeNode* root, struct TreeNode* p, struct TreeNode* q) { return p; }",
  );
  const cSurrounded = generateHarness(
    "c",
    judges["surrounded-regions"],
    "void backfillSealedPockets(char** board, int boardSize, int* boardColSize) {}",
  );
  const cycle = generateHarness(
    "cpp",
    judges["linked-list-cycle"],
    "class Solution { public: bool hasHandoffLoop(ListNode* head) { return head != nullptr; } };",
  );
  const lca = generateHarness(
    "java",
    judges["lowest-common-ancestor-of-a-binary-tree"],
    "class Solution { public TreeNode nearestSharedApprover(TreeNode root, TreeNode p, TreeNode q) { return p; } }",
  );
  const surrounded = generateHarness(
    "java",
    judges["surrounded-regions"],
    "class Solution { public void backfillSealedPockets(char[][] board) {} }",
  );
  const randomList = generateHarness(
    "cpp",
    judges["copy-list-with-random-pointer"],
    "class Solution { public: Node* snapshotOutline(Node* head) { return head; } };",
  );
  const cRandomList = generateHarness(
    "c",
    judges["copy-list-with-random-pointer"],
    "struct Node* snapshotOutline(struct Node* head) { return head; }",
  );
  const graph = generateHarness(
    "java",
    judges["clone-graph"],
    "class Solution { public Node replicateTopology(Node node) { return node; } }",
  );
  const cGraph = generateHarness(
    "c",
    judges["clone-graph"],
    "struct Node* replicateTopology(struct Node* s) { return s; }",
  );
  const nextTree = generateHarness(
    "cpp",
    judges["populating-next-right-pointers-in-each-node-ii"],
    "class Solution { public: Node* linkRowNeighbors(Node* root) { return root; } };",
  );
  const quadTree = generateHarness(
    "cpp",
    judges["construct-quad-tree"],
    "class Solution { public: Node* buildTileTree(vector<vector<int>>& grid) { return new Node(grid[0][0], true); } };",
  );
  const cQuadTree = generateHarness(
    "c",
    judges["construct-quad-tree"],
    "struct Node* buildTileTree(int** grid, int gridSize, int* gridColSize) { return NULL; }",
  );
  const javaQuadTree = generateHarness(
    "java",
    judges["construct-quad-tree"],
    "class Solution { public Node buildTileTree(int[][] grid) { return new Node(grid[0][0], true); } }",
  );

  assert.match(cList, /interleaveEvents\(list1, list2\)/);
  assert.doesNotMatch(cList, /interleaveEvents\(list1, list1Size/);
  assert.match(cTree, /nearestSharedApprover\(root, p, q\)/);
  assert.match(cTree, /findTreeNode\(root, 5\)/);
  assert.match(
    cSurrounded,
    /backfillSealedPockets\(board, boardSize, boardColSize\)/,
  );
  assert.match(
    cSurrounded,
    /json_char_matrix\(stdout, board, boardSize, boardColSize\)/,
  );
  assert.match(cycle, /ListNode\* head = listNode\(\{3, 2, 0, -4\}, 1\)/);
  assert.match(cycle, /solution\.hasHandoffLoop\(head\)/);
  assert.doesNotMatch(cycle, /solution\.hasHandoffLoop\(head, pos\)/);
  assert.match(lca, /TreeNode p = findTreeNode\(root, 5\)/);
  assert.match(lca, /solution\.nearestSharedApprover\(root, p, q\)/);
  assert.match(
    surrounded,
    /char\[\]\[\] board = new char\[\]\[\]\{new char\[\]\{'X', 'X', 'X', 'X'\}/,
  );
  assert.match(surrounded, /jsonAny\(actual\)/);
  assert.match(randomList, /\{13, 0\}, \{11, 4\}, \{10, 2\}, \{1, 0\}/);
  assert.match(randomList, /\{3, nullopt\}/);
  assert.match(randomList, /Returned list must be a deep copy/);
  assert.match(
    randomList,
    /Returned random pointer must stay inside the copied list/,
  );
  assert.match(randomList, /set<Node\*> seen/);
  assert.match(cRandomList, /Returned list must be a deep copy/);
  assert.match(
    cRandomList,
    /Returned random pointer must stay inside the copied list/,
  );
  // The browser runners serialize the same list, once in JavaScript and once
  // in Python, and have to refuse the same pointer before indexing it.
  const runners = readFileSync(webFile("runners.js"), "utf8");
  assert.match(
    runners,
    /item\.random != null && !seen\.has\(item\.random\)\) throw new TypeError\("Returned random pointer must stay inside the copied list\."\)/,
  );
  assert.match(
    runners,
    /node\.random is not None and id\(node\.random\) not in seen:\n\s+raise TypeError\("Returned random pointer must stay inside the copied list\."\)/,
  );
  assert.match(graph, /Returned graph must be a deep copy/);
  assert.match(cGraph, /Returned graph must be a deep copy/);
  assert.match(nextTree, /next pointers do not match the tree level order/);
  assert.match(quadTree, /quadTreeToJson\(actual\)/);
  assert.match(quadTree, /topLeft/);
  assert.match(cQuadTree, /json_quad_tree\(stdout, actual\)/);
  assert.match(javaQuadTree, /quadTreeJson\(\(Node\) actual\)/);
});

test("compiler output helpers parse results and map failures to setupError", () => {
  assert.equal(stripAnsi("\u001b[31merror\u001b[0m"), "error");
  assert.deepEqual(
    parseCompilerResults([
      { text: "noise " },
      { text: '{"results":[{"actual":[0,1],"timeMs":1.2}]}' },
    ]),
    {
      results: [{ actual: [0, 1], timeMs: 1.2 }],
    },
  );
  assert.deepEqual(parseCompilerResults('debug {"foo":1}\n{"results":[]}'), {
    results: [],
  });
  assert.deepEqual(parseCompilerResults("{bad"), {
    setupError: "The run produced no JSON results.",
  });
  assert.deepEqual(mapCompilerResponse({ timedOut: true }), {
    setupError: "Compiler Explorer timed out before the run completed.",
    diagnostic: { category: "timeout" },
  });
  assert.deepEqual(
    mapCompilerResponse({
      didExecute: false,
      buildResult: { stderr: [{ text: "\u001b[31mbuild failed\u001b[0m" }] },
    }),
    {
      setupError: "build failed",
      diagnostic: { category: "other" },
    },
  );
  assert.deepEqual(
    mapCompilerResponse({
      didExecute: false,
      buildResult: {
        stderr: [
          { text: "<source>:3: error: ';' expected" },
          { text: "        return 1" },
          { text: "                ^" },
        ],
      },
    }),
    {
      setupError:
        "<source>:3: error: ';' expected\n        return 1\n                ^",
      diagnostic: { category: "other" },
    },
  );
  assert.deepEqual(mapCompilerResponse({ stdout: "not json" }), {
    setupError: "The run produced no JSON results.",
  });
});

const compileError = (lines, editor) =>
  mapCompilerResponse(
    { didExecute: false, stderr: lines.map((text) => ({ text })) },
    editor,
  ).setupError;

test("compiler location parser ignores Java excerpts and keeps include chains", () => {
  const java = mapCompilerResponse(
    {
      didExecute: false,
      stderr: [
        { text: "<source>:2: error: cannot find symbol" },
        { text: "<source>:1:2: candidate source text" },
        { text: "        ^" },
      ],
    },
    { offset: 0, lineCount: 3 },
  );
  assert.equal(
    java.setupError,
    [
      "<source>:2: error: cannot find symbol",
      "<source>:1:2: candidate source text",
      "        ^",
    ].join("\n"),
  );
  assert.deepEqual(compilerDiagnosticLocations(java.setupError, 3), [
    { outputLine: 0, editorLine: 2 },
  ]);

  const cpp = mapCompilerResponse(
    {
      didExecute: false,
      stderr: [
        { text: "                 from <source>:5:7:" },
        { text: "<source>:7:11: required from here" },
      ],
    },
    { offset: 4, lineCount: 3 },
  );
  assert.deepEqual(compilerDiagnosticLocations(cpp.setupError, 3), [
    { outputLine: 0, editorLine: 1 },
    { outputLine: 1, editorLine: 3 },
  ]);

  const runtime = mapCompilerResponse(
    {
      didExecute: true,
      code: 1,
      stderr: [{ text: "<source>:1:2: candidate runtime text" }],
    },
    { offset: 0, lineCount: 3 },
  );
  assert.equal(runtime.setupError, "<source>:1:2: candidate runtime text");
  assert.equal(runtime.diagnostic.category, "runtime_signal");
});

// javac puts a binary-operator error at the operator, so a line holding only
// `^` echoes as an excerpt that looks like its own caret. The location above
// it is still a location: remapped for Jim and operable for the candidate.
test("a Java excerpt that is a bare caret leaves its location remapped", () => {
  const java = mapCompilerResponse(
    {
      didExecute: false,
      stderr: [
        {
          text: "<source>:9: error: bad operand types for binary operator '^'",
        },
        { text: "            ^" },
        { text: "            ^" },
        { text: "1 error" },
      ],
    },
    { offset: 4, lineCount: 10 },
  );
  assert.equal(
    java.setupError,
    [
      "<source>:5: error: bad operand types for binary operator '^'",
      "            ^",
      "            ^",
      "1 error",
    ].join("\n"),
  );
  assert.deepEqual(compilerDiagnosticLocations(java.setupError, 10), [
    { outputLine: 0, editorLine: 5 },
  ]);
});

test("compile errors cite the editor line in C, C++, Java and a class judge", () => {
  const code = ["public class Solution {", "  // first", "  BROKEN", "}"];
  for (const [language, id] of [
    ["c", "two-sum"],
    ["cpp", "two-sum"],
    ["java", "two-sum"],
    ["cpp", "lru-cache"],
    ["java", "lru-cache"],
  ]) {
    const spec = judges[id];
    const source = generateHarness(language, spec, code.join("\n"));
    const harnessLine = source.split("\n").indexOf("  BROKEN") + 1;
    assert.ok(harnessLine > 3, `${language} ${id} has no prelude to remap`);
    const position = language === "java" ? "" : "3:";
    assert.equal(
      compileError([`<source>:${harnessLine}:${position} error: x`], {
        offset: harnessLineOffset(language, spec),
        lineCount: code.length,
      }),
      `<source>:3:${position} error: x`,
      `${language} ${id}`,
    );
  }
});

test("a public Java class split across lines keeps its line count", () => {
  for (const id of ["two-sum", "lru-cache"]) {
    const spec = judges[id];
    const name = spec.kind === "class" ? spec.className : "Solution";
    const code = ["public", `class ${name} {`, "  // first", "  BROKEN", "}"];
    const source = generateHarness("java", spec, code.join("\n"));
    assert.doesNotMatch(source, new RegExp(`public\\s+class\\s+${name}\\b`));
    assert.equal(
      source.split("\n").indexOf("  BROKEN") +
        1 -
        harnessLineOffset("java", spec),
      4,
      id,
    );
  }
});

// The outputs below are what Compiler Explorer returned for a missing `;` in
// a 6-line C++, a 4-line C and a 6-line Java answer.
test("an error inside the candidate's code keeps its excerpt, renumbered", () => {
  assert.equal(
    compileError(
      [
        "<source>: In member function 'bool Solution::settlesToOne(int)':",
        "<source>:56:5: error: expected ';' before '}' token",
        "   56 |     }",
        "      |     ^",
      ],
      { offset: 51, lineCount: 6 },
    ),
    [
      "<source>: In member function 'bool Solution::settlesToOne(int)':",
      "<source>:5:5: error: expected ';' before '}' token",
      "    5 |     }",
      "      |     ^",
    ].join("\n"),
  );
  assert.equal(
    compileError(
      [
        "<source>:40:17: error: expected ';' after return statement",
        "   40 |     return false",
        "      |                 ^",
        "      |                 ;",
        "1 error generated.",
      ],
      { offset: 37, lineCount: 4 },
    ),
    [
      "<source>:3:17: error: expected ';' after return statement",
      "    3 |     return false",
      "      |                 ^",
      "      |                 ;",
      "1 error generated.",
    ].join("\n"),
  );
  assert.equal(
    compileError(
      [
        "<source>:8: error: ';' expected",
        "        return false",
        "                    ^",
        "1 error",
      ],
      { offset: 4, lineCount: 6 },
    ),
    [
      "<source>:4: error: ';' expected",
      "        return false",
      "                    ^",
      "1 error",
    ].join("\n"),
  );
});

test("an error in the harness names the harness and says why once", () => {
  assert.equal(
    compileError(
      [
        "<source>: In function 'int main()':",
        "<source>:65:28: error: 'class Solution' has no member named 'settlesToOne'",
        "   65 |     auto actual = solution.settlesToOne(n);",
        "      |                            ^~~~~~~~~~~~",
        "<source>:75:28: error: 'class Solution' has no member named 'settlesToOne'",
        "   75 |     auto actual = solution.settlesToOne(n);",
        "      |                            ^~~~~~~~~~~~",
      ],
      { offset: 51, lineCount: 6 },
    ),
    [
      "<test harness>: In function 'int main()':",
      "<test harness>: error: 'class Solution' has no member named 'settlesToOne'",
      "<test harness>: error: 'class Solution' has no member named 'settlesToOne'",
      HARNESS_SIGNATURE_NOTE,
    ].join("\n"),
  );
  assert.equal(
    compileError(
      [
        "<source>:45: error: cannot find symbol",
        "      Object actual = solution.settlesToOne(n);",
        "                              ^",
        "  symbol:   method settlesToOne(int)",
        "  location: variable solution of type Solution",
        "<source>:55: error: cannot find symbol",
        "      Object actual = solution.settlesToOne(n);",
        "                              ^",
        "  symbol:   method settlesToOne(int)",
        "  location: variable solution of type Solution",
        "5 errors",
      ],
      { offset: 4, lineCount: 6 },
    ),
    [
      "<test harness>: error: cannot find symbol",
      "  symbol:   method settlesToOne(int)",
      "  location: variable solution of type Solution",
      "<test harness>: error: cannot find symbol",
      "  symbol:   method settlesToOne(int)",
      "  location: variable solution of type Solution",
      "5 errors",
      HARNESS_SIGNATURE_NOTE,
    ].join("\n"),
  );
});

// clang's answer for a C function renamed to settlesTo, cut after the second
// of five errors: the "declared here" excerpt starts on the candidate's first
// line and runs down to the harness call.
test("an excerpt keeps the candidate's lines and drops the harness ones", () => {
  assert.equal(
    compileError(
      [
        "<source>:50:19: error: call to undeclared function 'settlesToOne'; ISO C99 and later do not support implicit function declarations [-Wimplicit-function-declaration]",
        "   50 |     bool actual = settlesToOne(n);",
        "      |                   ^",
        "<source>:50:19: note: did you mean 'settlesTo'?",
        "<source>:38:6: note: 'settlesTo' declared here",
        "   38 | bool settlesTo(int n) {",
        "      |      ^",
        "   39 |     // Think out loud as you go!",
        "   40 |     return false;",
        "   41 | }",
        "   42 | int main(void) {",
        '   43 |   printf("{\\"results\\":[");',
        "   44 |   ",
        "   45 |   {",
        "   46 |     int n = 19;",
        "   47 |     int returnSize = 0;",
        "   48 |     int* returnColumnSizes = NULL;",
        "   49 |     clock_t start = clock();",
        "   50 |     bool actual = settlesToOne(n);",
        "      |                   ~~~~~~~~~~~~",
        "      |                   settlesTo",
        "<source>:62:19: error: call to undeclared function 'settlesToOne'; ISO C99 and later do not support implicit function declarations [-Wimplicit-function-declaration]",
        "   62 |     bool actual = settlesToOne(n);",
        "      |                   ^",
        "5 errors generated.",
      ],
      { offset: 37, lineCount: 4 },
    ),
    [
      "<test harness>: error: call to undeclared function 'settlesToOne'; ISO C99 and later do not support implicit function declarations [-Wimplicit-function-declaration]",
      "<test harness>: note: did you mean 'settlesTo'?",
      "<source>:1:6: note: 'settlesTo' declared here",
      "    1 | bool settlesTo(int n) {",
      "      |      ^",
      "    2 |     // Think out loud as you go!",
      "    3 |     return false;",
      "    4 | }",
      "<test harness>: error: call to undeclared function 'settlesToOne'; ISO C99 and later do not support implicit function declarations [-Wimplicit-function-declaration]",
      "5 errors generated.",
      HARNESS_SIGNATURE_NOTE,
    ].join("\n"),
  );
});

// g++ 16's answer for a C++ function given a second parameter, cut after the
// first of five errors. The bullet g++ prints is U+2022, built here so the
// file stays ASCII.
test("a candidate g++ lists under a harness error keeps its editor line", () => {
  const bullet = String.fromCodePoint(0x2022);
  assert.equal(
    compileError(
      [
        "<source>: In function 'int main()':",
        "<source>:66:40: error: no matching function for call to 'Solution::settlesToOne(int&)'",
        "   66 |     auto actual = solution.settlesToOne(n);",
        "      |                   ~~~~~~~~~~~~~~~~~~~~~^~~",
        `  ${bullet} there is 1 candidate`,
        `    ${bullet} candidate 1: 'bool Solution::settlesToOne(int, int)'`,
        "      <source>:54:10:",
        "         54 |     bool settlesToOne(int n, int m) {",
        "            |          ^~~~~~~~~~~~",
        `      ${bullet} candidate expects 2 arguments, 1 provided`,
      ],
      { offset: 51, lineCount: 8 },
    ),
    [
      "<test harness>: In function 'int main()':",
      "<test harness>: error: no matching function for call to 'Solution::settlesToOne(int&)'",
      `  ${bullet} there is 1 candidate`,
      `    ${bullet} candidate 1: 'bool Solution::settlesToOne(int, int)'`,
      "      <source>:3:10:",
      "          3 |     bool settlesToOne(int n, int m) {",
      "            |          ^~~~~~~~~~~~",
      `      ${bullet} candidate expects 2 arguments, 1 provided`,
      HARNESS_SIGNATURE_NOTE,
    ].join("\n"),
  );
});

// g++ 16 on a 60-line C++ answer that hands std::sort a one-argument
// comparator, cut to the include chain, the candidate's call and one error in
// the library. Header line 91 less the offset is 40, inside the answer, so a
// header excerpt that took the offset would read as the candidate's line.
test("a library header keeps its own lines and the include chain is remapped", () => {
  const header =
    "/cefs/17/17a2c76db590a13a8a92c74c_consolidated/compilers_c++_x86_gcc_16.2.0/include/c++/16.2.0";
  const bullet = String.fromCodePoint(0x2022);
  const fromHeader = [
    `${header}/bits/stl_algo.h:91:17: error: no match for call to '(Solution::settlesToOne(int)::<lambda(int)>) (int&, int&)'`,
    "   91 |       if (__comp(*__a, *__b))",
    "      |           ~~~~~~^~~~~~~~~~~~",
    `  ${bullet} there are 2 candidates`,
  ];
  assert.equal(
    compileError(
      [
        `In file included from ${header}/algorithm:63,`,
        "                 from <source>:1:",
        "<source>:57:13:   required from here",
        "   57 |         sort(v.begin(), v.end(), [](int a) { return a; });",
        "      |         ~~~~^~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~",
        ...fromHeader,
        "    <source>:57:35:",
        "       57 |         sort(v.begin(), v.end(), [](int a) { return a; });",
        "          |                                   ^",
      ],
      { offset: 51, lineCount: 60 },
    ),
    [
      `In file included from ${header}/algorithm:63,`,
      "                 from <test harness>:",
      "<source>:6:13:   required from here",
      "    6 |         sort(v.begin(), v.end(), [](int a) { return a; });",
      "      |         ~~~~^~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~",
      ...fromHeader,
      "    <source>:6:35:",
      "        6 |         sort(v.begin(), v.end(), [](int a) { return a; });",
      "          |                                   ^",
    ].join("\n"),
  );
});

// clang's answer for strlen given an int in a 5-line C answer: the note points
// into the system header, whose excerpt stays as printed.
test("a clang note in a system header is left as printed", () => {
  const header = [
    "/usr/include/string.h:407:35: note: passing argument to parameter '__s' here",
    "  407 | extern size_t strlen (const char *__s)",
    "      |                                   ^",
  ];
  assert.equal(
    compileError(
      [
        "<source>:40:19: error: incompatible integer to pointer conversion passing 'int' to parameter of type 'const char *' [-Wint-conversion]",
        "   40 |     return strlen(n) > 0;",
        "      |                   ^",
        ...header,
        "1 error generated.",
      ],
      { offset: 37, lineCount: 5 },
    ),
    [
      "<source>:3:19: error: incompatible integer to pointer conversion passing 'int' to parameter of type 'const char *' [-Wint-conversion]",
      "    3 |     return strlen(n) > 0;",
      "      |                   ^",
      ...header,
      "1 error generated.",
    ].join("\n"),
  );
});

// What clang, g++ and javac returned when the candidate added their own
// `main` to an 8-line C and an 11-line C++ answer, and `class Main` to a
// 10-line Java one: the harness's definition comes second, so it is the error.
test("the candidate's own main earns no signature note", () => {
  assert.equal(
    compileError(
      [
        "<source>:46:5: error: redefinition of 'main'",
        "   46 | int main(void) {",
        "      |     ^",
        "<source>:42:5: note: previous definition is here",
        "   42 | int main(void) {",
        "      |     ^",
        "1 error generated.",
      ],
      { offset: 37, lineCount: 8 },
    ),
    [
      "<test harness>: error: redefinition of 'main'",
      "<source>:5:5: note: previous definition is here",
      "    5 | int main(void) {",
      "      |     ^",
      "1 error generated.",
    ].join("\n"),
  );
  assert.equal(
    compileError(
      [
        "<source>:63:5: error: redefinition of 'int main()'",
        "   63 | int main() {",
        "      |     ^~~~",
        "<source>:59:5: note: 'int main()' previously defined here",
        "   59 | int main() {",
        "      |     ^~~~",
      ],
      { offset: 51, lineCount: 11 },
    ),
    [
      "<test harness>: error: redefinition of 'int main()'",
      "<source>:8:5: note: 'int main()' previously defined here",
      "    8 | int main() {",
      "      |     ^~~~",
    ].join("\n"),
  );
  assert.equal(
    compileError(
      [
        "<source>:15: error: duplicate class: Main",
        "class Main {",
        "^",
        "1 error",
      ],
      { offset: 4, lineCount: 10 },
    ),
    "<test harness>: error: duplicate class: Main\n1 error",
  );
});

test("a harness note under the candidate's own error adds no signature note", () => {
  assert.equal(
    compileError(
      [
        "<source>:53:8: error: redefinition of 'struct ListNode'",
        "   53 | struct ListNode {",
        "      |        ^~~~~~~~",
        "<source>:20:8: note: previous definition of 'struct ListNode'",
        "   20 | struct ListNode {",
        "      |        ^~~~~~~~",
      ],
      { offset: 51, lineCount: 6 },
    ),
    [
      "<source>:2:8: error: redefinition of 'struct ListNode'",
      "    2 | struct ListNode {",
      "      |        ^~~~~~~~",
      "<test harness>: note: previous definition of 'struct ListNode'",
    ].join("\n"),
  );
});

// What clang, g++ and javac returned when the closing brace of settlesToOne
// was left out of a 4-line C, 7-line C++ and 6-line Java answer. The g++ one
// is cut after its second error.
test("an unclosed brace points at the candidate's last line, with no note", () => {
  assert.equal(
    compileError(
      [
        "<source>:42:16: error: function definition is not allowed here",
        "   42 | int main(void) {",
        "      |                ^",
        "<source>:106:2: error: expected '}'",
        "  106 | }",
        "      |  ^",
        "<source>:38:26: note: to match this '{'",
        "   38 | bool settlesToOne(int n) {",
        "      |                          ^",
        "2 errors generated.",
      ],
      { offset: 37, lineCount: 4 },
    ),
    [
      "<test harness>: error: function definition is not allowed here",
      "<source>:4: error: expected '}'",
      "<source>:1:26: note: to match this '{'",
      "    1 | bool settlesToOne(int n) {",
      "      |                          ^",
      "2 errors generated.",
    ].join("\n"),
  );
  assert.equal(
    compileError(
      [
        "<source>:112:2: error: expected '}' at end of input",
        "  112 | }",
        "      |  ^",
        "<source>:52:16: note: to match this '{'",
        "   52 | class Solution {",
        "      |                ^",
        "<source>: In member function 'int Solution::main()':",
        "<source>:112:1: warning: no return statement in function returning non-void [-Wreturn-type]",
        "  112 | }",
        "      | ^",
        "<source>: At global scope:",
        "<source>:112:2: error: expected unqualified-id at end of input",
        "  112 | }",
        "      |  ^",
      ],
      { offset: 51, lineCount: 7 },
    ),
    [
      "<source>:7: error: expected '}' at end of input",
      "<source>:1:16: note: to match this '{'",
      "    1 | class Solution {",
      "      |                ^",
      "<test harness>: In member function 'int Solution::main()':",
      "<test harness>: warning: no return statement in function returning non-void [-Wreturn-type]",
      "<source>: At global scope:",
      "<source>:7: error: expected unqualified-id at end of input",
    ].join("\n"),
  );
  assert.equal(
    compileError(
      [
        "<source>:93: error: reached end of file while parsing",
        "}",
        " ^",
        "1 error",
      ],
      { offset: 4, lineCount: 6 },
    ),
    "<source>:6: error: reached end of file while parsing\n1 error",
  );
});

// g++ on a 7-line C++ answer whose method is itself called main, cut after
// the first harness error: only the scope line above a harness location is
// relabelled.
test("a scope line names the harness only when its location is there", () => {
  assert.equal(
    compileError(
      [
        "<source>: In member function 'bool Solution::main(int)':",
        "<source>:56:16: error: 'missing' was not declared in this scope",
        "   56 |         return missing;",
        "      |                ^~~~~~~",
        "<source>: In function 'int main()':",
        "<source>:66:28: error: 'class Solution' has no member named 'settlesToOne'",
        "   66 |     auto actual = solution.settlesToOne(n);",
        "      |                            ^~~~~~~~~~~~",
      ],
      { offset: 51, lineCount: 8 },
    ),
    [
      "<source>: In member function 'bool Solution::main(int)':",
      "<source>:5:16: error: 'missing' was not declared in this scope",
      "    5 |         return missing;",
      "      |                ^~~~~~~",
      "<test harness>: In function 'int main()':",
      "<test harness>: error: 'class Solution' has no member named 'settlesToOne'",
      HARNESS_SIGNATURE_NOTE,
    ].join("\n"),
  );
});

test("only lines past either end of the candidate's code are the harness", () => {
  const editor = { offset: 10, lineCount: 3 };
  assert.equal(
    compileError(["<source>:11: error: x"], editor),
    "<source>:1: error: x",
  );
  assert.equal(
    compileError(["<source>:13: error: x"], editor),
    "<source>:3: error: x",
  );
  for (const line of [10, 14]) {
    assert.equal(
      compileError([`<source>:${line}: error: x`], editor),
      `<test harness>: error: x\n${HARNESS_SIGNATURE_NOTE}`,
    );
  }
});

function bankFunctionTypes() {
  const types = new Set();
  for (const spec of Object.values(judges)) {
    if (spec.kind !== "function") continue;
    for (const type of spec.paramTypes) types.add(type);
    types.add(spec.returnType);
  }
  return [...types].sort();
}

test("a class-style judge withholds C, and says why", () => {
  assert.equal(judges["lru-cache"].kind, "class");
  assert.deepEqual(
    languagesFor(judges["lru-cache"]),
    ALL_LANGUAGES.filter((one) => one !== "c"),
  );
  assert.deepEqual(harnessAvailability("c", judges["lru-cache"]), {
    message: harnessGap("c", judges["lru-cache"]),
    reason: "class problem",
  });
  assert.match(
    harnessGap("c", judges["lru-cache"]),
    /only builds function judges/,
  );
  assert.equal(harnessGap("cpp", judges["lru-cache"]), null);
});

test("C is offered only once a function-style judge is known", () => {
  assert.equal(judges["two-sum"].kind, "function");
  assert.deepEqual(languagesFor(judges["two-sum"]), ALL_LANGUAGES);
  assert.deepEqual(
    languagesFor(null),
    ALL_LANGUAGES.filter((one) => one !== "c"),
  );
  assert.deepEqual(harnessAvailability("c", null), {
    message: harnessGap("c", null),
    reason: "waiting for judge",
  });
  assert.match(harnessGap("c", null), /only after function-style tests load/);
});

test("disabled compiled runners withhold their languages for every judge", () => {
  // The module reads the runtime config on every call, as the page's own
  // /runtime-config.js would leave it on a server with the runs turned off.
  const previous = globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED;
  globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = false;
  try {
    for (const spec of [null, judges["two-sum"], judges["lru-cache"]]) {
      assert.deepEqual(languagesFor(spec), ["python", "javascript"]);
      for (const language of COMPILED_LANGUAGES) {
        assert.deepEqual(harnessAvailability(language, spec), {
          message: harnessGap(language, spec),
          reason: "disabled by server",
        });
        assert.match(harnessGap(language, spec), /disabled by this server/);
      }
      const available =
        spec?.kind === "function"
          ? ALL_LANGUAGES
          : ALL_LANGUAGES.filter((language) => language !== "c");
      assert.deepEqual(languagesFor(spec, false), available);
      for (const language of available) {
        assert.equal(harnessGap(language, spec, false), null);
      }
      if (!available.includes("c"))
        assert.match(harnessGap("c", spec, false), /function/);
    }
  } finally {
    if (previous === undefined)
      delete globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED;
    else globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = previous;
  }
});

test("execution-disabled languages have starter code for every problem", () => {
  for (const [id, spec] of Object.entries(judges)) {
    const problem = JSON.parse(
      readFileSync(webFile(`problems/${pageMap[id].page}.json`), "utf8"),
    );
    for (const language of languagesFor(spec, false))
      assert.equal(
        typeof problem.starterCode[language],
        "string",
        `${id}: ${language}`,
      );
  }
});
