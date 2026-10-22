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

// A promisified execFile, and one cached probe per tool.
//
// Do not "simplify" the seven tests below back to `execFileSync`. They compile
// and run real C, C++ and Java, which costs 1-3 seconds each, and they are the
// only slow thing in the browser lane. Two facts decide the shape:
//
//   * `node:test` runs top-level `test()` calls one after another even when
//     they are async, so the seven have to be `it()` children of a suite
//     declared `{ concurrency: true }` to be scheduled together at all.
//   * A concurrent scheduler cannot overlap anything that blocks the event
//     loop. `execFileSync` does exactly that, so with it the seven would still
//     run end to end no matter how the suite is declared.
//
// Both changes are needed; either one alone buys nothing. Each test compiles
// into its own `mkdtempSync` directory and removes it in a `finally`, so they
// share no path and can overlap safely.
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

describe("toolchain harnesses", { concurrency: true }, () => {
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
        assert.deepEqual(
          parseCompilerResults(
            (await run(binary, [], { encoding: "utf8" })).stdout,
          ).results.map((result) => result.actual),
          cloneGraphSpec.cases.map((testCase) =>
            testCase.input[0].length
              ? "Returned graph must be a deep copy."
              : [],
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
            ).results.map((result) => result.actual),
            cloneGraphSpec.cases.map((testCase) =>
              testCase.input[0][nodeValue - 1]?.length
                ? "Returned graph has a NULL neighbor."
                : testCase.expected,
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

  // tail starts at 1 and grows by two per node, so the 4096-slot C queue is
  // full after the 2047th node and the 2048th has nowhere to put its children.
  // The chain replaces the tree on every case with a root, cycle or not, and
  // the empty case returns nothing to replace. The 2047-node chain pins the
  // limit from the other side, so a check that gives up a node early fails.
  for (const [nodes, cyclic] of [
    [2047, false],
    [2048, false],
    [2048, true],
  ]) {
    const shape = `${nodes}-node ${cyclic ? "cycle" : "chain"}`;
    it(
      `c harness ${nodes > 2047 ? "refuses" : "prints"} a returned ${shape}`,
      { skip: noCc },
      async () => {
        const dir = mkdtempSync(join(tmpdir(), "codetrial-c-bound-"));
        const judge = judges["flatten-binary-tree-to-linked-list"];
        try {
          const source = join(dir, "chain.c");
          const binary = join(dir, "chain");
          writeFileSync(
            source,
            generateHarness(
              "c",
              judge,
              `void linearizeMenu(struct TreeNode* root) {
    if (!root) return;
    struct TreeNode* last = root;
    root->left = NULL;
    for (int i = 1; i < ${nodes}; i++) {
        last->right = calloc(1, sizeof(struct TreeNode));
        last = last->right;
    }
    last->right = ${cyclic ? "root" : "NULL"};
}`,
            ),
          );
          await run("cc", ["-std=c17", source, "-o", binary]);
          const { results } = parseCompilerResults(
            (await run(binary, [], { encoding: "utf8", timeout: 10000 }))
              .stdout,
          );
          assert.equal(results.length, judge.cases.length);
          judge.cases.forEach((testCase, index) => {
            const result = results[index];
            if (testCase.input[0].length === 0)
              assert.deepEqual(result.actual, [], testCase.label);
            else if (nodes > 2047)
              assert.match(
                result.error ?? "",
                /Returned tree must not have more than 2047 nodes\./,
                testCase.label,
              );
            else
              assert.equal(
                result.actual.filter((value) => value !== null).length,
                nodes,
                testCase.label,
              );
          });
        } finally {
          rmSync(dir, { recursive: true, force: true });
        }
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
