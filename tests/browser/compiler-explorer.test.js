import { before, describe, it, test } from "node:test";
import assert from "node:assert/strict";
import { execFile, execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

import {
  evaluate as calculate,
  MAX_EXPRESSION_LENGTH,
} from "../../web/calculator.js";
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
import { checkAnswer } from "../../web/lib.js";
import { parseCandidateCase } from "../../web/runners.js";
import { createSyntaxParser } from "../../web/syntax-parser.js";
import { candidateSource } from "./source.js";

const cppSyntax = createSyntaxParser({
  loadBytes: async (url) => new Uint8Array(readFileSync(url)),
  now: () => 0,
});

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

test("EventQueue cases reject incorrect queue implementations", () => {
  const replay = ({ input: [operations, args] }, fault = "correct") => {
    const capacity = args[0][0];
    const slots = Array(capacity);
    let head = 0;
    let tail = 0;
    let count = 0;
    const linear = fault === "linear tail resets only when empty";
    const rejected = fault === "rejected push advances tail";
    const queue = {
      push(value) {
        const slot = tail;
        if (rejected) tail = (tail + 1) % capacity;
        const full =
          fault === "full check wastes a slot"
            ? (tail + 1) % capacity === head
            : count === capacity;
        if (full || (linear && tail === capacity)) return false;
        slots[slot] = value;
        if (!rejected) tail = linear ? tail + 1 : (tail + 1) % capacity;
        count++;
        return true;
      },
      pop() {
        if (count === 0) return -1;
        const value = slots[head];
        head = fault === "head never wraps" ? head + 1 : (head + 1) % capacity;
        count--;
        if (linear && count === 0) head = tail = 0;
        return value;
      },
      front() {
        return count ? slots[head] : -1;
      },
      size() {
        return fault === "size is tail minus head" ? tail - head : count;
      },
    };
    return operations.map((operation, index) =>
      index === 0 ? null : queue[operation](...args[index]),
    );
  };
  const cases = judges["fixed-capacity-ring-buffer"].cases;
  for (const testCase of cases)
    assert.deepEqual(replay(testCase), testCase.expected, testCase.label);
  for (const [name, label] of [
    ["linear tail resets only when empty", "wrap after removing the oldest"],
    ["full check wastes a slot", "one slot can be reused repeatedly"],
    ["head never wraps", "one slot can be reused repeatedly"],
    ["size is tail minus head", "empty reads and rejected overflow"],
    ["rejected push advances tail", "rejected push keeps later order"],
  ]) {
    const testCase = cases.find((row) => row.label === label);
    assert.ok(testCase, `missing case for ${name}: ${label}`);
    assert.notDeepEqual(
      replay(testCase, name),
      testCase.expected,
      `${label} must reject ${name}`,
    );
  }
});

test("ThumbnailStore cases each reject one wrong store", () => {
  // A Map keeps insertion order, so its first key is the least recently used.
  // Map.set on a key already present keeps that key where it was, so with
  // "overwrite keeps its old place" the store overwrites in place and later
  // evicts the key it just wrote. With "evicts before checking the key" it
  // evicts on every put to a full store, so overwriting a key that is not the
  // oldest drops another key the store still had room for. With
  // "zero read as missing" a stored 0 reads as absent and is never refreshed,
  // and with "put reads zero as missing" overwriting a stored 0 is taken for an
  // insert, so a full store evicts another key and the 0 keeps its place.
  // With "a read moves one step" a read key passes only its newer neighbor,
  // and with "a read leaves recency alone" it does not move at all. With
  // "rewriting the same value is skipped" a put that changes nothing returns
  // before it refreshes the key.
  const replay = ({ input: [operations, args] }, fault = "correct") => {
    const capacity = args[0][0];
    const slots = new Map();
    const store = {
      get(key) {
        const present =
          fault === "zero read as missing"
            ? Boolean(slots.get(key))
            : slots.has(key);
        if (!present) return -1;
        const value = slots.get(key);
        if (fault === "a read leaves recency alone") return value;
        if (fault === "a read moves one step") {
          const entries = [...slots];
          const at = entries.findIndex(([stored]) => stored === key);
          if (at + 1 < entries.length)
            [entries[at], entries[at + 1]] = [entries[at + 1], entries[at]];
          slots.clear();
          for (const [stored, kept] of entries) slots.set(stored, kept);
          return value;
        }
        slots.delete(key);
        slots.set(key, value);
        return value;
      },
      put(key, value) {
        if (
          fault === "rewriting the same value is skipped" &&
          slots.get(key) === value
        )
          return null;
        if (fault === "evicts before checking the key") {
          if (slots.size === capacity) slots.delete(slots.keys().next().value);
          slots.delete(key);
        } else if (
          fault === "put reads zero as missing"
            ? Boolean(slots.get(key))
            : slots.has(key)
        ) {
          if (fault !== "overwrite keeps its old place") slots.delete(key);
        } else if (slots.size === capacity) {
          slots.delete(slots.keys().next().value);
        }
        slots.set(key, value);
        return null;
      },
    };
    return operations.map((operation, index) =>
      index === 0 ? null : store[operation](...args[index]),
    );
  };
  const cases = judges["lru-cache"].cases;
  for (const testCase of cases)
    assert.deepEqual(replay(testCase), testCase.expected, testCase.label);
  for (const [name, label] of [
    ["overwrite keeps its old place", "insert after an overwrite at capacity"],
    ["evicts before checking the key", "overwrite the newest key at capacity"],
    ["zero read as missing", "zero is a stored value"],
    ["a read moves one step", "refresh from the middle at capacity 3"],
    [
      "rewriting the same value is skipped",
      "rewrite with the same value at capacity",
    ],
    ["a read leaves recency alone", "capacity 2, classic eviction sequence"],
    ["put reads zero as missing", "overwrite a stored zero at capacity 3"],
  ]) {
    const testCase = pinnedCase("lru-cache", label);
    assert.notDeepEqual(
      replay(testCase, name),
      testCase.expected,
      `${label} must reject ${name}`,
    );
  }
});

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

test("character literals spell control characters in octal", () => {
  // A raw newline or carriage return between quotes ends the literal in all
  // three languages; the escapes C, C++ and Java share keep their spelling.
  for (const language of ["c", "cpp", "java"]) {
    assert.equal(nativeLiteral("\n", "character", language), "'\\012'");
    assert.equal(nativeLiteral("\r", "character", language), "'\\015'");
    assert.equal(nativeLiteral("\u0000", "character", language), "'\\000'");
    assert.equal(nativeLiteral("'", "character", language), "'\\''");
    assert.equal(nativeLiteral("\\", "character", language), "'\\\\'");
    assert.equal(nativeLiteral("7", "character", language), "'7'");
    assert.equal(nativeLiteral("\t", "character", language), "'\\011'");
    assert.equal(nativeLiteral("\u0001", "character", language), "'\\001'");
    assert.equal(nativeLiteral(" ", "character", language), "' '");
  }
  assert.equal(
    nativeLiteral([["X", "\n"]], "character[][]", "c"),
    "{{'X', '\\012'}}",
  );
});

// What adding a case says about a character no compiled harness can pass on
// as typed, and the judges whose parameters hold strings at each depth.
const addCase = (id, input) =>
  parseCandidateCase(judges[id], JSON.stringify(input));
const addTrieWord = (word) =>
  addCase("implement-trie-prefix-tree", [
    ["CommandIndex", "insert"],
    [[], [word]],
  ]);
const HOLDS_NUL = `"\\u0000" (U+0000), which C and C++ read as the end of a string`;
const HOLDS_HALF_PAIR = `"\\ud800" (U+D800), which is half of a surrogate pair and does not compile in C or C++`;
const LONGEST = "longest-substring-without-repeating-characters";

// `\000` compiles, but C and C++ hand the solution a string that ends there, so
// the case would run on "a" alone and fail a correct solution.
test("an added string holding NUL is refused and named", () => {
  assert.throws(() => addCase(LONGEST, ["a\u0000b"]), {
    message: `Parameter 1 (s) holds ${HOLDS_NUL}.`,
  });
  // A typed backslash before "u0000" is text, not NUL.
  assert.deepEqual(addCase(LONGEST, ["\\u0000"]), ["\\u0000"]);
  // Word Search II takes a `string[]` of words, and Evaluate Division nests its
  // strings two lists deep.
  assert.throws(() => addCase("word-search-ii", [[["a"]], ["x", "a\u0000"]]), {
    message: `Parameter 2 (words) holds ${HOLDS_NUL}.`,
  });
  assert.throws(
    () => addCase("evaluate-division", [[["a", "b\u0000"]], [2], [["a", "b"]]]),
    { message: `Parameter 1 (pairs) holds ${HOLDS_NUL}.` },
  );
});

// JSON writes a lone surrogate as `\ud800`, a universal character name clang
// and gcc refuse in C and C++, so the whole run stops compiling.
test("an added string holding a lone surrogate is refused and named", () => {
  assert.throws(() => addCase(LONGEST, ["a\ud800b"]), {
    message: `Parameter 1 (s) holds ${HOLDS_HALF_PAIR}.`,
  });
  assert.throws(() => addCase("word-search-ii", [[["a"]], ["x", "a\ud800"]]), {
    message: `Parameter 2 (words) holds ${HOLDS_HALF_PAIR}.`,
  });
  // A whole pair is one character outside the Basic Multilingual Plane.
  assert.deepEqual(addCase(LONGEST, ["a\u{1F600}b"]), ["a\u{1F600}b"]);
});

// A NUL cell reaches a C solution as 0, but Surrounded Regions returns its
// board in place, and C's `json_char_matrix` prints that cell as "", so a
// correct solution is graded wrong.
test("an added character holding NUL is refused and named", () => {
  assert.throws(() => addCase("surrounded-regions", [[["X", "\u0000"]]]), {
    message: `Parameter 1 (board) holds ${HOLDS_NUL}.`,
  });
  assert.throws(() => addCase("word-search-ii", [[["a", "\u0000"]], ["ab"]]), {
    message: `Parameter 1 (board) holds ${HOLDS_NUL}.`,
  });
});

// A class operation's strings reach C++ through the same string literal.
test("an added class operation holding NUL or a lone surrogate is refused and named", () => {
  assert.throws(() => addTrieWord("a\u0000b"), {
    message: `Arguments for operation insert hold ${HOLDS_NUL}.`,
  });
  assert.throws(() => addTrieWord("a\ud800b"), {
    message: `Arguments for operation insert hold ${HOLDS_HALF_PAIR}.`,
  });
  assert.deepEqual(addTrieWord("\\u0000")[1][1], ["\\u0000"]);
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

test("valid-anagram cases reject equal-length character-digest shortcuts", () => {
  const codes = (s) => [...s].map((c) => c.charCodeAt(0));
  const add = (values) => values.reduce((a, b) => a + b, 0);
  for (const [name, digest] of [
    ["sum", (s) => add(codes(s))],
    ["xor", (s) => codes(s).reduce((a, b) => a ^ b, 0)],
    [
      "sum and sum of squares",
      (s) => `${add(codes(s))} ${add(codes(s).map((c) => c * c))}`,
    ],
  ]) {
    assert.ok(
      judges["valid-anagram"].cases.some(
        ({ input: [s, t], expected }) =>
          expected === false &&
          s.length === t.length &&
          digest(s) === digest(t),
      ),
      `the built-in cases must reject the length-and-character-${name} shortcut`,
    );
  }
});

/// The built-in case `label` of `problem`, which a test pinning a wrong
/// solution to it must still find, so dropping the case fails by its name.
const pinnedCase = (problem, label) => {
  const row = judges[problem].cases.find((item) => item.label === label);
  assert.ok(row, `the built-in cases must keep "${label}"`);
  return row;
};

/// Asserts that the judge rejects `run` on the built-in case `label`.
const assertRejects = (problem, label, run, message) => {
  const row = pinnedCase(problem, label);
  assert.ok(
    !checkAnswer(judges[problem], row, run(...structuredClone(row.input))),
    message,
  );
};

test("longest-common-prefix cases reject incomplete comparisons", () => {
  const firstAndLast = (strs) => {
    const first = strs[0];
    const last = strs.at(-1);
    let at = 0;
    while (at < first.length && first[at] === last[at]) at += 1;
    return first.slice(0, at);
  };

  const containsPrefix = (strs) => {
    let prefix = strs[0];
    for (const str of strs.slice(1)) {
      while (!str.includes(prefix)) prefix = prefix.slice(0, -1);
    }
    return prefix;
  };

  const ignoresEndedString = (strs) => {
    const first = strs[0];
    for (let i = 0; i < first.length; i += 1) {
      for (const str of strs.slice(1)) {
        if (i < str.length && str[i] !== first[i]) return first.slice(0, i);
      }
    }
    return first;
  };

  const lengthSortedEndpoints = (strs) =>
    firstAndLast([...strs].sort((a, b) => a.length - b.length));

  const matchingColumns = (strs) => {
    let count = 0;
    const shortest = Math.min(...strs.map((str) => str.length));
    for (let i = 0; i < shortest; i += 1) {
      if (strs.every((str) => str[i] === strs[0][i])) count += 1;
    }
    return strs[0].slice(0, count);
  };

  for (const [label, shortcut, why] of [
    [
      "middle string breaks prefix",
      firstAndLast,
      "a middle string must be checked even when the endpoints share a prefix",
    ],
    [
      "same substring appears later",
      containsPrefix,
      "a prefix must start each string, not just appear in it",
    ],
    [
      "later string ends first",
      ignoresEndedString,
      "the prefix must end when any string ends",
    ],
    [
      "middle differs at second letter",
      lengthSortedEndpoints,
      "sorting by length does not make endpoint comparison sufficient",
    ],
    [
      "matching column after mismatch",
      matchingColumns,
      "matching columns after a mismatch cannot extend the prefix",
    ],
  ]) {
    assertRejects("longest-common-prefix", label, shortcut, why);
  }

  for (const {
    label,
    input: [strs],
    expected,
  } of judges["longest-common-prefix"].cases) {
    let prefix = strs[0];
    while (!strs.every((str) => str.startsWith(prefix))) {
      prefix = prefix.slice(0, -1);
    }
    assert.equal(
      prefix,
      expected,
      `"${label}" must expect the longest shared prefix`,
    );
  }
});

/// Asserts that the judge accepts `solve` on every built-in case of `problem`.
const assertSolves = (problem, solve, message) => {
  const judge = judges[problem];
  for (const row of judge.cases) {
    assert.ok(
      checkAnswer(judge, row, solve(...structuredClone(row.input))),
      message(row.label),
    );
  }
};

test("merge-sorted-array cases reject zero-as-padding shortcuts", async (t) => {
  function writeBack(nums1, values) {
    nums1.fill(0);
    values.forEach((value, index) => {
      nums1[index] = value;
    });
  }

  function dropZerosFromNums1(nums1, m, nums2, n) {
    const merged = [
      ...nums1.slice(0, m).filter((value) => value !== 0),
      ...nums2.slice(0, n),
    ].sort((a, b) => a - b);
    writeBack(nums1, merged);
  }

  function stripTrailingZerosFromNums1(nums1, m, nums2, n) {
    const prefix = nums1.slice(0, m);
    while (prefix.at(-1) === 0) prefix.pop();
    const merged = [...prefix, ...nums2.slice(0, n)].sort((a, b) => a - b);
    writeBack(nums1, merged);
  }

  function stripLeadingZerosFromNums1(nums1, m, nums2, n) {
    const prefix = nums1.slice(0, m);
    while (prefix[0] === 0) prefix.shift();
    const merged = [...prefix, ...nums2.slice(0, n)].sort((a, b) => a - b);
    writeBack(nums1, merged);
  }

  function skipLeadingZerosInNums1Merge(nums1, m, nums2, n) {
    let start = 0;
    while (start < m && nums1[start] === 0) start += 1;
    let left = m - 1;
    let right = n - 1;

    for (let write = m + n - 1; right >= 0; write -= 1) {
      nums1[write] =
        left >= start && nums1[left] > nums2[right]
          ? nums1[left--]
          : nums2[right--];
    }
  }

  function skipZerosInNums1Merge(nums1, m, nums2, n) {
    let left = m - 1;
    let right = n - 1;
    let write = m + n - 1;

    while (right >= 0) {
      if (left >= 0 && nums1[left] === 0) left -= 1;
      else if (left >= 0 && nums1[left] > nums2[right])
        nums1[write--] = nums1[left--];
      else nums1[write--] = nums2[right--];
    }
  }

  function skipZerosInNums2Merge(nums1, m, nums2, n) {
    let left = m - 1;
    let right = n - 1;
    let write = m + n - 1;

    while (right >= 0) {
      if (nums2[right] === 0) right -= 1;
      else if (left >= 0 && nums1[left] > nums2[right])
        nums1[write--] = nums1[left--];
      else nums1[write--] = nums2[right--];
    }
  }

  function filterZerosFromNums2(nums1, m, nums2, n) {
    const merged = [
      ...nums1.slice(0, m),
      ...nums2.slice(0, n).filter((value) => value !== 0),
    ].sort((a, b) => a - b);
    writeBack(nums1, merged);
  }

  function stripTrailingZerosFromNums2(nums1, m, nums2, n) {
    const prefix = nums2.slice(0, n);
    while (prefix.at(-1) === 0) prefix.pop();
    const merged = [...nums1.slice(0, m), ...prefix].sort((a, b) => a - b);
    writeBack(nums1, merged);
  }

  function stripLeadingZerosFromNums2(nums1, m, nums2, n) {
    const prefix = nums2.slice(0, n);
    while (prefix[0] === 0) prefix.shift();
    const merged = [...nums1.slice(0, m), ...prefix].sort((a, b) => a - b);
    writeBack(nums1, merged);
  }

  function skipLeadingZerosInNums2Merge(nums1, m, nums2, n) {
    let start = 0;
    while (start < n && nums2[start] === 0) start += 1;
    let left = m - 1;
    let right = n - 1;

    for (let write = m + n - 1; right >= start; write -= 1) {
      nums1[write] =
        left >= 0 && nums1[left] > nums2[right]
          ? nums1[left--]
          : nums2[right--];
    }
  }

  function keepFirstZero(values) {
    let keptZero = false;
    return values.filter((value) => {
      if (value !== 0) return true;
      if (keptZero) return false;
      keptZero = true;
      return true;
    });
  }

  function coalesceZerosAcrossInputs(nums1, m, nums2, n) {
    const merged = keepFirstZero([
      ...nums1.slice(0, m),
      ...nums2.slice(0, n),
    ]).sort((a, b) => a - b);
    writeBack(nums1, merged);
  }

  function coalesceZerosAcrossInputsInPlace(nums1, m, nums2, n) {
    let left = m - 1;
    let right = n - 1;
    let write = m + n - 1;

    while (right >= 0) {
      if (left >= 0 && nums1[left] === 0 && nums2[right] === 0) left -= 1;
      else if (left >= 0 && nums1[left] > nums2[right])
        nums1[write--] = nums1[left--];
      else nums1[write--] = nums2[right--];
    }
  }

  function coalesceZerosWithinInputs(nums1, m, nums2, n) {
    const merged = [
      ...keepFirstZero(nums1.slice(0, m)),
      ...keepFirstZero(nums2.slice(0, n)),
    ].sort((a, b) => a - b);
    writeBack(nums1, merged);
  }

  function merge(nums1, m, nums2, n) {
    let left = m - 1;
    let right = n - 1;

    for (let write = m + n - 1; right >= 0; write -= 1) {
      nums1[write] =
        left >= 0 && nums1[left] > nums2[right]
          ? nums1[left--]
          : nums2[right--];
    }

    return nums1;
  }

  for (const [label, shortcut] of [
    ["trailing zero in valid prefix", dropZerosFromNums1],
    ["trailing zero in valid prefix", stripTrailingZerosFromNums1],
    ["trailing zero in valid prefix", skipZerosInNums1Merge],
    ["leading zero in valid prefix", dropZerosFromNums1],
    ["leading zero in valid prefix", stripLeadingZerosFromNums1],
    ["leading zero changes position", skipLeadingZerosInNums1Merge],
    ["trailing zero in nums2", skipZerosInNums2Merge],
    ["trailing zero in nums2", filterZerosFromNums2],
    ["trailing zero in nums2", stripTrailingZerosFromNums2],
    ["leading zero in nums2", stripLeadingZerosFromNums2],
    ["leading zero in nums2", skipLeadingZerosInNums2Merge],
    ["zero in both inputs", coalesceZerosAcrossInputs],
    ["zero in both inputs", coalesceZerosAcrossInputsInPlace],
    ["repeated zeros in both inputs", coalesceZerosWithinInputs],
  ]) {
    await t.test(`${shortcut.name} on "${label}"`, () =>
      assertRejects(
        "merge-sorted-array",
        label,
        (nums1, m, nums2, n) => {
          shortcut(nums1, m, nums2, n);
          return nums1;
        },
        `"${label}" must reject ${shortcut.name}`,
      ),
    );
  }

  assertSolves(
    "merge-sorted-array",
    merge,
    (label) => `"${label}" must expect the merged array`,
  );
});

const addColumns = (a, b, digit = (sum) => sum % 2) => {
  let i = a.length - 1;
  let j = b.length - 1;
  let carry = 0;
  const bits = [];
  while (i >= 0 || j >= 0 || carry) {
    const sum = Number(a[i--] ?? 0) + Number(b[j--] ?? 0) + carry;
    bits.push(digit(sum));
    carry = Math.floor(sum / 2);
  }
  return bits.reverse().join("");
};

test("add-binary cases accept exact addition of wide values", () => {
  for (const {
    input: [a, b],
    expected,
    label,
  } of judges["add-binary"].cases) {
    assert.equal(addColumns(a, b), expected, label);
    assert.equal(
      (BigInt(`0b${a}`) + BigInt(`0b${b}`)).toString(2),
      expected,
      label,
    );
  }
});

for (const [name, label, add] of [
  [
    "parseInt addition through Number",
    "wide value plus one",
    (a, b) => (parseInt(a, 2) + parseInt(b, 2)).toString(2),
  ],
  [
    "BigInt converted from an already rounded Number",
    "wide odd value plus zero",
    (a, b) => (BigInt(parseInt(a, 2)) + BigInt(parseInt(b, 2))).toString(2),
  ],
  [
    "addition limited to 64 bits",
    "wide all ones plus one",
    (a, b) =>
      BigInt.asUintN(64, BigInt(`0b${a}`) + BigInt(`0b${b}`)).toString(2),
  ],
  [
    "addition limited to 128 bits",
    "wide all ones plus one",
    (a, b) =>
      BigInt.asUintN(128, BigInt(`0b${a}`) + BigInt(`0b${b}`)).toString(2),
  ],
]) {
  test(`add-binary cases reject ${name}`, () => {
    assertRejects("add-binary", label, add, `${label} must reject ${name}`);
  });
}

test("add-binary cases reject rounding only the shorter operand", () => {
  const add = (a, b) => {
    const [longer, shorter] = a.length > b.length ? [a, b] : [b, a];
    return (BigInt(`0b${longer}`) + BigInt(parseInt(shorter, 2))).toString(2);
  };
  const label = "two wide values with multi-bit carry";
  const { input } = pinnedCase("add-binary", label);
  assert.ok(input.every((bits) => bits.length > 53));
  assert.notEqual(input[0].length, input[1].length);
  assertRejects("add-binary", label, add, `${label} must reject rounding`);
});

test("add-binary cases reject writing a column of three as zero", () => {
  const add = (a, b) => addColumns(a, b, (sum) => (sum === 3 ? 0 : sum % 2));
  const label = "two wide values with multi-bit carry";
  assertRejects(
    "add-binary",
    label,
    add,
    `${label} must reject a column of three`,
  );
});

test("word-pattern cases reject shortcuts that skip a length or mapping check", () => {
  const distinct = (values) => new Set(values).size;
  const sameDistinct = (p, w) =>
    distinct([...p].slice(0, w.length).map((c, i) => `${c} ${w[i]}`)) ===
      distinct(p) && distinct(p) === distinct(w);
  const counts = (v) => v.map((x) => v.filter((y) => y === x).length).sort();
  // A letter keeps its first word; clash(i) can refuse a new letter's word.
  const oneWay = (p, w, clash) => {
    const wordOf = new Map();
    return [...p].every((c, i) =>
      wordOf.has(c) ? wordOf.get(c) === w[i] : !clash(i) && wordOf.set(c, w[i]),
    );
  };
  const sameLength = (answer) => (p, w) =>
    p.length === w.length && answer(p, w);
  // Each word is compared with every earlier one through same(later, earlier).
  const wordsCompared = (same) =>
    sameLength((p, w) =>
      [...p].every((c, i) =>
        w.slice(0, i).every((x, j) => (c === p[j]) === same(w[i], x)),
      ),
    );
  const letterSum = (word) =>
    [...word].reduce((sum, c) => sum + c.charCodeAt(0), 0);
  for (const [name, answer, label] of [
    [
      "distinct counts without a length check",
      sameDistinct,
      "fewer words than letters",
    ],
    [
      "distinct counts that refuse only a short message",
      (p, w) => w.length >= p.length && sameDistinct(p, w),
      "extra word repeats an earlier one",
    ],
    [
      "distinct counts that refuse only a long message",
      (p, w) => w.length <= p.length && sameDistinct(p, w),
      "fewer words than letters",
    ],
    [
      "letter and word counts",
      sameLength((p, w) => `${counts([...p])}` === `${counts(w)}`),
      "alternating words for a mirrored template",
    ],
    [
      "comparison with the first position",
      sameLength((p, w) =>
        [...p].every((c, i) => (c === p[0]) === (w[i] === w[0])),
      ),
      "new letter takes a word already used",
    ],
    [
      "one-way map with a neighbour check",
      sameLength(
        (p, w) =>
          oneWay(p, w, () => false) &&
          [...p].every(
            (c, i) => i === 0 || (c === p[i - 1]) === (w[i] === w[i - 1]),
          ),
      ),
      "new letter takes a word already used",
    ],
    [
      "one-way map checked against the last new word",
      sameLength((p, w) => {
        let last;
        return oneWay(p, w, (i) => {
          const clash = w[i] === last;
          last = w[i];
          return clash;
        });
      }),
      "third letter reuses the first word",
    ],
    [
      "one map for both directions",
      sameLength((p, w) => {
        const seen = new Map();
        const claim = (k, v) => (seen.get(k) ?? seen.set(k, v).get(k)) === v;
        return [...p].every((c, i) => claim(c, w[i]) && claim(w[i], c));
      }),
      "one-letter words named like template letters",
    ],
    [
      "boxed indices compared by reference",
      sameLength((p, w) => {
        // Integer.valueOf shares a box only from -128 to 127.
        const box = (i) => (i < 128 ? i : [i]);
        const letterAt = new Map();
        const wordAt = new Map();
        return [...p].every((c, i) => {
          const same = letterAt.get(c) === wordAt.get(w[i]);
          letterAt.set(c, box(i));
          wordAt.set(w[i], box(i));
          return same;
        });
      }),
      "one word repeated 130 times",
    ],
    [
      "words compared by the earlier word's prefix",
      wordsCompared((word, earlier) => word.startsWith(earlier)),
      "second word extends the first",
    ],
    [
      "words compared by the later word's prefix",
      wordsCompared((word, earlier) => earlier.startsWith(word)),
      "second word shortens the first",
    ],
    [
      "words compared by letter sum",
      wordsCompared((word, earlier) => letterSum(word) === letterSum(earlier)),
      "second word reorders the first",
    ],
    [
      "words compared by their first eight bytes",
      // strncmp(a, b, sizeof a) with a 64-bit pointer; four bytes fails here too.
      wordsCompared(
        (word, earlier) => word.slice(0, 8) === earlier.slice(0, 8),
      ),
      "words differ only at the ninth letter",
    ],
    [
      "words kept in a plain object",
      sameLength((p, w) => {
        const letterOf = {};
        const wordOf = {};
        return [...p].every((c, i) => {
          if (wordOf[c] && wordOf[c] !== w[i]) return false;
          if (letterOf[w[i]] && letterOf[w[i]] !== c) return false;
          wordOf[c] = w[i];
          letterOf[w[i]] = c;
          return true;
        });
      }),
      "second word is constructor",
    ],
    [
      "two-way map that refuses only a long message",
      // Past the last word w[i] is undefined, and no earlier word equals it.
      (p, w) =>
        w.length <= p.length &&
        oneWay(p, w, (i) => w.slice(0, i).includes(w[i])),
      "last letter has no word",
    ],
    [
      "first-seen indices joined into a string",
      // Once an index has two digits, 10 1 10 and 10 11 0 join alike.
      (p, w) =>
        [...p].map((c) => p.indexOf(c)).join("") ===
        w.map((x) => w.indexOf(x)).join(""),
      "twelfth word is new where its letter repeats",
    ],
  ]) {
    assertRejects(
      "word-pattern",
      label,
      (pattern, s) => answer(pattern, s.split(" ")),
      `"${label}" must reject the ${name} shortcut`,
    );
  }
  const correct = sameLength((p, w) => {
    const wordOf = new Map();
    const letterOf = new Map();
    return [...p].every((c, i) => {
      if (!wordOf.has(c) && !letterOf.has(w[i])) {
        wordOf.set(c, w[i]);
        letterOf.set(w[i], c);
      }
      return wordOf.get(c) === w[i] && letterOf.get(w[i]) === c;
    });
  });
  for (const {
    label,
    input: [pattern, s],
    expected,
  } of judges["word-pattern"].cases) {
    assert.equal(
      correct(pattern, s.split(" ")),
      expected,
      `"${label}" must expect the two-way mapping answer`,
    );
  }
});

test("rotated search cases reject each faulty bound, pivot or shortcut search", () => {
  const problem = "search-in-rotated-sorted-array";
  const inclusive = (a, b) => a <= b;
  const exclusive = (a, b) => a < b;
  const search =
    ({
      leftSorted = inclusive,
      leftBound = inclusive,
      rightBound = inclusive,
      rightLowerBound = exclusive,
      midpoint = Math.floor,
    } = {}) =>
    (nums, target) => {
      let lo = 0;
      let hi = nums.length - 1;
      while (lo <= hi) {
        const mid = lo + midpoint((hi - lo) / 2);
        if (nums[mid] === target) return mid;
        if (leftSorted(nums[lo], nums[mid], nums[hi])) {
          if (leftBound(nums[lo], target) && target < nums[mid]) hi = mid - 1;
          else lo = mid + 1;
        } else if (
          rightLowerBound(nums[mid], target) &&
          rightBound(target, nums[hi], nums[nums.length - 1])
        ) {
          lo = mid + 1;
        } else {
          hi = mid - 1;
        }
      }
      return -1;
    };

  const searchRightFirst =
    ({ leftBound = inclusive, useOuterBounds = false } = {}) =>
    (nums, target) => {
      let lo = 0;
      let hi = nums.length - 1;
      while (lo <= hi) {
        const mid =
          lo + (useOuterBounds ? Math.ceil : Math.floor)((hi - lo) / 2);
        if (nums[mid] === target) return mid;
        const first = nums[useOuterBounds ? 0 : lo];
        const last = nums[useOuterBounds ? nums.length - 1 : hi];
        const sorted = useOuterBounds ? exclusive : inclusive;
        if (sorted(nums[mid], last)) {
          if (nums[mid] < target && target <= last) lo = mid + 1;
          else hi = mid - 1;
        } else if (leftBound(first, target) && target < nums[mid]) {
          hi = mid - 1;
        } else {
          lo = mid + 1;
        }
      }
      return -1;
    };

  const rightFirstLeftWithoutMid = (nums, target) => {
    let lo = 0;
    let hi = nums.length - 1;
    while (lo <= hi) {
      const mid = lo + Math.floor((hi - lo) / 2);
      if (nums[mid] === target) return mid;
      if (nums[mid] <= nums[hi]) {
        if (nums[mid] < target && target <= nums[hi]) lo = mid + 1;
        else hi = mid - 1;
      } else if (nums[lo] <= target) {
        hi = mid - 1;
      } else {
        lo = mid + 1;
      }
    }
    return -1;
  };

  const pivotAgainstFirst = (nums, target) => {
    let lo = 0;
    let hi = nums.length - 1;
    while (lo < hi) {
      const mid = lo + Math.floor((hi - lo) / 2);
      if (nums[mid] >= nums[0]) lo = mid + 1;
      else hi = mid;
    }
    return target >= nums[0]
      ? searchRange(nums, target, 0, lo - 1)
      : searchRange(nums, target, lo, nums.length - 1);
  };

  const pivotAgainstFirstDiscardingMidpoint = (nums, target) => {
    const last = nums.length - 1;
    let lo = nums[0] > nums[last] ? 1 : 0;
    let hi = last;
    while (lo < hi) {
      const mid = lo + Math.floor((hi - lo) / 2);
      if (nums[mid] >= nums[0]) lo = mid + 1;
      else hi = mid - 1;
    }
    return target < nums[0]
      ? searchRange(nums, target, lo, last)
      : searchRange(nums, target, 0, lo);
  };

  const shiftedStartPivotAgainstHi = (nums, target) => {
    const last = nums.length - 1;
    let lo = nums[0] > nums[last] ? 1 : 0;
    let hi = last;
    while (lo < hi) {
      const mid = lo + Math.floor((hi - lo) / 2);
      if (nums[mid] > nums[hi]) lo = mid + 1;
      else hi = mid - 1;
    }
    let [from, to] = target <= nums[last] ? [lo, last] : [0, lo];
    while (from <= to) {
      const mid = from + Math.floor((to - from) / 2);
      if (nums[mid] === target) return mid;
      if (nums[mid] < target) from = mid + 1;
      else to = mid - 1;
    }
    return -1;
  };

  const pivotAgainstFirstIncludingPivot = (nums, target) => {
    let hi = nums.length - 1;
    // Index zero cannot be the pivot once a wrap is known to exist.
    let lo = nums[0] > nums[hi] ? 1 : 0;
    while (lo < hi) {
      const mid = lo + Math.floor((hi - lo) / 2);
      if (nums[mid] > nums[0]) lo = mid + 1;
      else hi = mid;
    }
    return target < nums[0]
      ? searchRange(nums, target, lo, nums.length - 1)
      : searchRange(nums, target, 0, lo);
  };

  const searchRange = (nums, target, from, to, requireMatch = true) => {
    if (from > to) return -1;
    while (from < to) {
      const mid = from + Math.floor((to - from) / 2);
      if (nums[mid] < target) from = mid + 1;
      else to = mid;
    }
    return !requireMatch || nums[from] === target ? from : -1;
  };

  const pivotWithDiscardedMidpoint =
    ({ closedLoop = false, includePivot = false, bound = "last" } = {}) =>
    (nums, target) => {
      const last = nums.length - 1;
      let lo = 0;
      let hi = last;
      while (lo < hi || (closedLoop && lo === hi)) {
        const mid = lo + Math.floor((hi - lo) / 2);
        if (nums[mid] > nums[bound === "hi" ? hi : last]) lo = mid + 1;
        else hi = mid - 1;
      }
      let [from, to] =
        target <= nums[last] ? [lo, last] : [0, includePivot ? lo : lo - 1];
      // The inclusive-pivot variants use an equality check at each midpoint.
      if (!includePivot) return searchRange(nums, target, from, to);
      while (from <= to) {
        const mid = from + Math.floor((to - from) / 2);
        if (nums[mid] === target) return mid;
        if (nums[mid] < target) from = mid + 1;
        else to = mid - 1;
      }
      return -1;
    };

  const uncheckedInsertionPoint = (nums, target, afterWrapOnly = false) => {
    const last = nums.length - 1;
    let lo = 0;
    let hi = last;
    while (lo < hi) {
      const mid = lo + Math.floor((hi - lo) / 2);
      if (nums[mid] > nums[hi]) lo = mid + 1;
      else hi = mid;
    }
    const afterWrap = target <= nums[last];
    if (afterWrap) hi = last;
    else [lo, hi] = [0, lo - 1];
    if (lo > hi || target < nums[lo] || target > nums[hi]) return -1;
    return searchRange(nums, target, lo, hi, afterWrapOnly && !afterWrap);
  };

  const afterWrapGap = "target between entries after a wrap";
  const uncheckedAfterWrap = (nums, target) =>
    uncheckedInsertionPoint(nums, target, true);
  assertRejects(problem, afterWrapGap, uncheckedAfterWrap, afterWrapGap);
  const { input: afterWrapInput } = pinnedCase(problem, afterWrapGap);
  assert.equal(uncheckedAfterWrap(...afterWrapInput), 3);

  const reference = search();
  const rightFirstReference = searchRightFirst();
  for (const {
    label,
    input: [nums, target],
    expected,
  } of judges[problem].cases) {
    assert.equal(nums.indexOf(target), expected, label);
    assert.equal(reference(nums, target), expected, label);
    assert.equal(rightFirstReference(nums, target), expected, label);
  }

  const left = "first entry before a wrap";
  const right = "last entry after a wrap";
  const pair = "wrapped pair with target last";
  const firstOfFour = "first entry in a wrapped quartet";
  const roundedUpLeftFirst = search({
    midpoint: Math.ceil,
    leftSorted: (_lo, mid, hi) => mid >= hi,
  });
  const roundedUpSearchWithGlobalLastBound = search({
    leftSorted: (_lo, mid, hi) => mid > hi,
    rightBound: (target, _hi, last) => target <= last,
    midpoint: Math.ceil,
  });
  const halfOpenSearchWithGlobalLastBound = (nums, target) => {
    let lo = 0;
    let hi = nums.length - 1;
    while (lo < hi) {
      const mid = lo + Math.floor((hi - lo) / 2);
      if (nums[mid] === target) return mid;
      if (nums[mid] < nums[hi]) {
        if (nums[mid] < target && target <= nums[nums.length - 1]) lo = mid + 1;
        else hi = mid;
      } else if (nums[lo] <= target && target < nums[mid]) {
        hi = mid;
      } else {
        lo = mid + 1;
      }
    }
    return nums[lo] === target ? lo : -1;
  };
  const halfOpenSearchWithGlobalFirstBound = (nums, target) => {
    let lo = 0;
    let hi = nums.length - 1;
    while (lo < hi) {
      const mid = lo + Math.ceil((hi - lo) / 2);
      if (nums[mid] === target) return mid;
      if (nums[lo] < nums[mid]) {
        if (nums[0] <= target && target < nums[mid]) hi = mid - 1;
        else lo = mid;
      } else if (nums[mid] < target && target <= nums[hi]) {
        lo = mid;
      } else {
        hi = mid - 1;
      }
    }
    return nums[lo] === target ? lo : -1;
  };
  for (const [label, fault, faultySearch] of [
    [
      "target before the midpoint of a dump wrapped at its end",
      "a rounded-up search that bounds the right half by the dump's last entry",
      roundedUpSearchWithGlobalLastBound,
    ],
    [
      "target before the midpoint of a seven-entry dump wrapped at its end",
      "a half-open search that rounds down and bounds the right half by the dump's last entry",
      halfOpenSearchWithGlobalLastBound,
    ],
    [
      "entry just past the midpoint of a seven-entry wrap",
      "a half-open search that rounds up and bounds the left half by the dump's first entry",
      halfOpenSearchWithGlobalFirstBound,
    ],
    [
      "target past the midpoint before the wrap",
      "a right-first search that omits the midpoint from its left-half bound",
      rightFirstLeftWithoutMid,
    ],
    [
      firstOfFour,
      "a closed pivot search compared with nums[hi] that discards its midpoint",
      pivotWithDiscardedMidpoint({ closedLoop: true, bound: "hi" }),
    ],
    [
      firstOfFour,
      "a rounded-up midpoint that takes nums[mid] >= nums[hi] as a sorted left half",
      roundedUpLeftFirst,
    ],
    [
      "last entry before a five-entry wrap",
      "a closed pivot search compared with nums[hi] that discards its midpoint and searches through the pivot",
      pivotWithDiscardedMidpoint({
        closedLoop: true,
        includePivot: true,
        bound: "hi",
      }),
    ],
    [
      "smallest entry in a six-entry wrap",
      "an open pivot search compared with nums[last] that discards its midpoint and searches through the pivot",
      pivotWithDiscardedMidpoint({ includePivot: true, bound: "last" }),
    ],
    [
      "smallest entry in a seven-entry wrap",
      "a pivot search against nums[0] that discards its midpoint",
      pivotAgainstFirstDiscardingMidpoint,
    ],
    [
      "last entry before a six-entry wrap",
      "a shifted-start pivot search compared with nums[hi] that discards its midpoint and searches through the pivot",
      shiftedStartPivotAgainstHi,
    ],
    [
      "wrapped pair with target first",
      "fixed outer bounds with a rounded-up midpoint, right half first",
      searchRightFirst({ useOuterBounds: true }),
    ],
    [
      "smallest entry after a wrap",
      "an open pivot search compared with nums[last] that discards its midpoint",
      pivotWithDiscardedMidpoint({ bound: "last" }),
    ],
    [
      "pair not wrapped with target last",
      "a pivot found against nums[0] and included in the left half",
      pivotAgainstFirstIncludingPivot,
    ],
    [
      "target between the wrap and the midpoint",
      "a sorted right half searched without checking its lower bound",
      search({ rightLowerBound: () => true }),
    ],
    [
      "target between entries before a wrap",
      "an insertion point returned without a match check",
      uncheckedInsertionPoint,
    ],
  ]) {
    assertRejects(
      problem,
      label,
      faultySearch,
      `"${label}" must reject ${fault}`,
    );
  }
  assertRejects(
    problem,
    "target in a single entry",
    (nums, target) => (nums.length === 1 ? -1 : reference(nums, target)),
    "The search must check a singleton for the target",
  );
  assertRejects(
    problem,
    "negative target after a wrap",
    (nums, target) => (target < 0 ? -1 : reference(nums, target)),
    "The search must look for negative targets",
  );
  assertRejects(
    problem,
    "last entry, not wrapped",
    pivotAgainstFirst,
    "The search must include the last entry when the dump does not wrap",
  );
  assertRejects(
    problem,
    left,
    searchRightFirst({ leftBound: exclusive }),
    `"${left}" must reject an exclusive left bound when testing the right half first`,
  );
  for (const [fault, labels] of [
    [{ leftBound: exclusive }, [left]],
    [{ rightBound: exclusive }, [right]],
    [{ leftSorted: exclusive }, [pair]],
    [{ leftBound: exclusive, rightBound: exclusive }, [left, right]],
    [{ leftBound: exclusive, leftSorted: exclusive }, [left, pair]],
    [{ rightBound: exclusive, leftSorted: exclusive }, [right, pair]],
    [
      { leftBound: exclusive, rightBound: exclusive, leftSorted: exclusive },
      [left, right, pair],
    ],
  ]) {
    const faultySearch = search(fault);
    const comparisons = Object.keys(fault).join(", ");
    for (const label of labels) {
      assertRejects(
        problem,
        label,
        faultySearch,
        `"${label}" must reject exclusive comparisons for ${comparisons}`,
      );
    }
  }
});

test("jump-game cases reject each hop-by-hop shortcut", () => {
  const travel = (choose) => (nums) => {
    let at = 0;
    while (at < nums.length - 1) {
      const next = nums[at] === 0 ? at : choose(nums, at);
      if (next === at) return false;
      at = next;
    }
    return true;
  };
  const last = (nums) => nums.length - 1;
  const largestRange = (pickLaterTie) => (nums, at) => {
    if (at + nums[at] >= last(nums)) return last(nums);
    const beats = pickLaterTie ? (a, b) => a >= b : (a, b) => a > b;
    let best = at + 1;
    for (let next = at + 2; next <= at + nums[at]; next += 1) {
      if (beats(nums[next], nums[best])) best = next;
    }
    return best;
  };
  for (const [name, choose, label] of [
    [
      "the full range",
      (nums, at) => Math.min(at + nums[at], last(nums)),
      "range grows one relay ahead",
    ],
    [
      "the farthest relay that is not a zero",
      (nums, at) => {
        let next = Math.min(at + nums[at], last(nums));
        while (next > at && next < last(nums) && nums[next] === 0) next -= 1;
        return next;
      },
      "second relay clears two zeros",
    ],
    [
      "the reachable relay with the largest range",
      largestRange(false),
      "far small relay beats a near big one",
    ],
    [
      "the reachable relay with the largest range, the later one on a tie",
      largestRange(true),
      "far small relay beats a near big one",
    ],
  ]) {
    assertRejects(
      "jump-game",
      label,
      travel(choose),
      `"${label}" must reject always hopping to ${name}`,
    );
  }
});

test("insert-interval cases reject reading the last block before checking for one", () => {
  const merge = (intervals, [start, end]) => {
    const before = intervals.filter(([, b]) => b < start);
    const after = intervals.filter(([a]) => a > end);
    const overlapping = intervals.filter(([a, b]) => b >= start && a <= end);
    return [
      ...before,
      [
        Math.min(start, ...overlapping.map(([a]) => a)),
        Math.max(end, ...overlapping.map(([, b]) => b)),
      ],
      ...after,
    ];
  };
  const lastBlockFirst = (intervals, block) =>
    block[0] > intervals[intervals.length - 1][1]
      ? [...intervals, block]
      : merge(intervals, block);
  const label = "no existing blocks";
  const { cases } = judges["insert-interval"];
  for (const { label: name, input, expected } of cases) {
    assert.deepEqual(
      merge(...structuredClone(input)),
      expected,
      `"${name}" must expect the merged blocks`,
    );
  }
  const pinned = cases.find(({ label: name }) => name === label);
  assert.ok(pinned, `the built-in cases must keep "${label}"`);
  assert.throws(
    () => lastBlockFirst(...structuredClone(pinned.input)),
    TypeError,
    `"${label}" must reject reading the last block before checking for one`,
  );
});

test("remove-element cases reject skipping a last unchecked target", () => {
  const kept = (nums, val) => nums.filter((code) => code !== val);
  // Swaps kept codes in from the right, but tests left === right only
  // after right moves, so a lone unchecked target is copied and counted.
  const skipsLastTarget = (nums, val) => {
    let left = 0;
    let right = nums.length - 1;
    while (left <= right) {
      if (nums[left] !== val) {
        left += 1;
      } else {
        while (left <= right && nums[right] === val) {
          right -= 1;
          if (left === right) return left;
        }
        nums[left] = nums[right];
        left += 1;
        right -= 1;
      }
    }
    return left;
  };
  const judge = judges["remove-element"];
  for (const testCase of judge.cases) {
    assert.ok(
      checkAnswer(judge, testCase, kept(...testCase.input)),
      `"${testCase.label}" must expect the codes that are not muted`,
    );
  }
  for (const label of [
    "last unchecked element is the target",
    "only element is the target",
  ]) {
    const pinned = pinnedCase("remove-element", label);
    const [nums, val] = structuredClone(pinned.input);
    const size = skipsLastTarget(nums, val);
    assert.equal(
      checkAnswer(judge, pinned, nums.slice(0, size)),
      false,
      `"${label}" must reject skipping a last unchecked target`,
    );
  }
});

test("longest-substring cases reject shortcuts in moving the stretch", () => {
  const problem = "longest-substring-without-repeating-characters";
  const longest = (s) => {
    const last = new Map();
    let best = 0;
    let left = 0;
    [...s].forEach((key, i) => {
      if (last.has(key) && last.get(key) >= left) left = last.get(key) + 1;
      last.set(key, i);
      best = Math.max(best, i - left + 1);
    });
    return best;
  };
  for (const { label, input, expected } of judges[problem].cases) {
    assert.equal(
      longest(...input),
      expected,
      `"${label}" must expect the longest stretch`,
    );
  }
  // Both keep only the repeated key, so a key between its two copies is lost.
  const restart = (s) => {
    const seen = new Set();
    let best = 0;
    for (const key of s) {
      if (seen.has(key)) seen.clear();
      seen.add(key);
      best = Math.max(best, seen.size);
    }
    return best;
  };
  const leftAtRepeat = (s) => {
    const last = new Map();
    let best = 0;
    let left = 0;
    [...s].forEach((key, i) => {
      if (last.has(key) && last.get(key) >= left) left = i;
      last.set(key, i);
      best = Math.max(best, i - left + 1);
    });
    return best;
  };
  // `if` where `while` belongs: the left edge drops one key per repeat.
  const oneStep = (s) => {
    const seen = new Set();
    let best = 0;
    let left = 0;
    for (const key of s) {
      if (seen.has(key)) seen.delete(s[left++]);
      seen.add(key);
      best = Math.max(best, seen.size);
    }
    return best;
  };
  // Measures only the stretch from the first key and the one to the last.
  const endsOnly = (s) => {
    const run = (keys) => {
      const seen = new Set();
      for (const key of keys) {
        if (seen.has(key)) break;
        seen.add(key);
      }
      return seen.size;
    };
    return Math.max(run(s), run([...s].reverse()));
  };
  // The edge starts at 0, not -1, so a stretch from the first key is one short.
  const edgeAtZero = (s) => {
    const last = new Map();
    let best = 0;
    let edge = 0;
    [...s].forEach((key, i) => {
      if (last.has(key)) edge = Math.max(edge, last.get(key));
      last.set(key, i);
      best = Math.max(best, i - edge);
    });
    return best;
  };
  // Measures a stretch only when a repeat ends it, so the last one is never counted.
  const bestAtRepeat = (s) => {
    const last = new Map();
    let best = 0;
    let left = 0;
    [...s].forEach((key, i) => {
      if (last.has(key) && last.get(key) >= left) {
        best = Math.max(best, i - left);
        left = last.get(key) + 1;
      }
      last.set(key, i);
    });
    return best;
  };
  // The contract compares case-sensitively and counts every character.
  const lowercased = (s) => longest(s.toLowerCase());
  const lettersOnly = (s) => longest(s.replace(/[^A-Za-z]/g, ""));
  const symbolsDropped = (s) => longest(s.replace(/[^A-Za-z0-9 ]/g, ""));
  const trimmed = (s) => longest(s.trim());
  const trimmedStart = (s) => longest(s.trimStart());
  const trimmedEnd = (s) => longest(s.trimEnd());
  // A last-seen table of 26 slots: a key outside a to z is never recorded.
  const slots26 = (s) => {
    const last = new Int32Array(26).fill(-1);
    let best = 0;
    let left = 0;
    [...s].forEach((key, i) => {
      const slot = key.charCodeAt(0) - 97;
      if (slot >= 0 && slot < 26) {
        if (last[slot] >= left) left = last[slot] + 1;
        last[slot] = i;
      }
      best = Math.max(best, i - left + 1);
    });
    return best;
  };
  const mid = "repeat in the middle of the stretch";
  for (const [name, shortcut, label] of [
    ["clearing the stretch and restarting at the repeat", restart, mid],
    ["moving the left edge to the repeat itself", leftAtRepeat, mid],
    [
      "moving the left edge one key per repeat",
      oneStep,
      "left edge jumps more than one key",
    ],
    [
      "measuring only the stretches at the ends",
      endsOnly,
      "longest stretch away from both ends",
    ],
    ["starting the edge at 0 instead of -1", edgeAtZero, "every key distinct"],
    [
      "measuring a stretch only when a repeat ends it",
      bestAtRepeat,
      "every key distinct",
    ],
    ["lowercasing the keys first", lowercased, "case and symbols count"],
    [
      "dropping keys that are not letters",
      lettersOnly,
      "case and symbols count",
    ],
    ["a 26-slot last-seen table", slots26, "space before and after the keys"],
    ["dropping symbols", symbolsDropped, "space before and after the keys"],
    [
      "trimming the spaces at the ends",
      trimmed,
      "space before and after the keys",
    ],
    ["trimming only the leading spaces", trimmedStart, "leading space only"],
    ["trimming only the trailing spaces", trimmedEnd, "trailing space only"],
  ]) {
    assertRejects(problem, label, shortcut, `"${label}" must reject ${name}`);
  }
});

test("merge-intervals cases reject shortcuts in consolidating windows", () => {
  const sorted = (intervals) =>
    structuredClone(intervals).sort(([a], [b]) => a - b);
  // The sorted sweep a right answer makes. Most shortcuts below change only
  // when a window joins the last kept one or how it extends it.
  const sweep =
    (
      joins = (last, start) => start <= last[1],
      extend = (last, end) => Math.max(last[1], end),
    ) =>
    (intervals) => {
      const merged = [];
      for (const [start, end] of sorted(intervals)) {
        const last = merged.at(-1);
        if (last && joins(last, start, merged)) {
          last[1] = extend(last, end, merged);
        } else {
          merged.push([start, end]);
        }
      }
      return merged;
    };
  const consolidate = sweep();
  // Absorbs at most `limit` following windows into each kept one, so a
  // longer chain is cut short.
  const absorbing = (limit) => (intervals) => {
    const windows = sorted(intervals);
    const merged = [];
    for (let i = 0; i < windows.length; i += 1) {
      let [start, end] = windows[i];
      for (let n = 0; n < limit; n += 1) {
        if (i + 1 < windows.length && windows[i + 1][0] <= end) {
          i += 1;
          end = Math.max(end, windows[i][1]);
        } else {
          break;
        }
      }
      merged.push([start, end]);
    }
    return merged;
  };
  // Merges a window with the next one, then skips both, so a merged window
  // is never compared with the window after it.
  const onePair = absorbing(1);
  // Compares each window with the previous input window, not the merged one,
  // so a window inside an earlier long one starts a window of its own.
  const previousInput = (intervals) => {
    const windows = sorted(intervals);
    const merged = [];
    windows.forEach(([start, end], i) => {
      if (i > 0 && start <= windows[i - 1][1]) {
        merged.at(-1)[1] = Math.max(merged.at(-1)[1], end);
      } else {
        merged.push([start, end]);
      }
    });
    return merged;
  };
  const twoNeighbours = absorbing(2);
  // Compares each window with the first merged window, not the last one.
  const firstMerged = (intervals) => {
    const merged = [];
    for (const [start, end] of sorted(intervals)) {
      if (merged.length > 0 && start <= merged[0][1]) {
        merged[0][1] = Math.max(merged[0][1], end);
      } else {
        merged.push([start, end]);
      }
    }
    return merged;
  };
  // Folds each window, in input order, into a kept window it overlaps among
  // those `candidates` offers, and sorts only the result.
  const foldUnsorted = (candidates) => (intervals) => {
    const merged = [];
    for (const [start, end] of structuredClone(intervals)) {
      const kept = candidates(merged).find(([a, b]) => start <= b && end >= a);
      if (kept) {
        kept[0] = Math.min(kept[0], start);
        kept[1] = Math.max(kept[1], end);
      } else {
        merged.push([start, end]);
      }
    }
    return merged.sort(([a], [b]) => a - b);
  };
  // Merges neighbours in input order and sorts only the result, so two
  // overlapping windows that are apart in the input stay apart.
  const inputOrder = foldUnsorted((merged) => merged.slice(-1));
  // Folds each window into the first kept window it overlaps and sorts only
  // the result, so a window that bridges two kept ones joins just the first.
  const firstOverlap = foldUnsorted((merged) => merged);
  // Takes the next window's end without max once a window was kept before
  // the one being extended, so a window inside the second one shortens it.
  const noMaxAfterFirst = sweep(undefined, (last, end, merged) =>
    merged.length > 1 ? end : Math.max(last[1], end),
  );
  // Merges touching windows only into the first kept window; later ones need
  // a real overlap.
  const touchOnlyFirst = sweep((last, start, merged) =>
    merged.length > 1 ? start < last[1] : start <= last[1],
  );
  // Skips a window whose start equals its end as if it were empty.
  const skipEmpty = (intervals) =>
    consolidate(intervals.filter(([start, end]) => start !== end));
  // Tests the last end for truthiness, so a window ending at 0 is never
  // extended.
  const truthyEnd = sweep((last, start) => last[1] && start <= last[1]);
  // Keeps one end per start and lets a later window overwrite it, so a
  // shorter window after a longer one at the same start replaces it.
  const endPerStart = (intervals) => {
    const endAt = new Map();
    for (const [start, end] of intervals) endAt.set(start, end);
    return consolidate([...endAt]);
  };
  // Buckets windows by start in a table one slot short of the 0..10^4 range,
  // so a window starting at 10000 is dropped.
  const shortTable = (intervals) => {
    const endAt = new Int32Array(10000).fill(-1);
    for (const [start, end] of intervals) {
      if (end > endAt[start]) endAt[start] = end;
    }
    const windows = [];
    endAt.forEach((end, start) => {
      if (end >= 0) windows.push([start, end]);
    });
    return consolidate(windows);
  };
  const judge = judges["merge-intervals"];
  for (const testCase of judge.cases) {
    assert.ok(
      checkAnswer(judge, testCase, consolidate(...testCase.input)),
      `"${testCase.label}" must expect the consolidated windows`,
    );
  }
  for (const [name, shortcut, label] of [
    [
      "merging only one neighbouring pair",
      onePair,
      "chained overlaps: [[1,3],[2,5],[4,8]]",
    ],
    [
      "comparing with the previous input window",
      previousInput,
      "later window inside the first: [[1,10],[2,3],[4,5]]",
    ],
    [
      "absorbing at most two following windows",
      twoNeighbours,
      "four windows in one chain: [[1,3],[2,5],[4,8],[7,10]]",
    ],
    [
      "comparing with the first merged window",
      firstMerged,
      "overlap after a separate first window: [[1,2],[4,6],[5,7]]",
    ],
    [
      "merging in input order and sorting only the result",
      inputOrder,
      "overlapping windows apart in the input: [[1,3],[5,6],[2,4]]",
    ],
    [
      "folding into the first kept window it overlaps",
      firstOverlap,
      "window joining two earlier ones: [[1,2],[5,6],[2,5]]",
    ],
    [
      "dropping max once a window was kept before",
      noMaxAfterFirst,
      "later window inside a second window: [[1,2],[4,10],[5,6]]",
    ],
    [
      "merging touching windows only into the first",
      touchOnlyFirst,
      "touching endpoints after a separate first window: [[1,2],[4,5],[5,6]]",
    ],
    [
      "skipping zero-length windows",
      skipEmpty,
      "zero-length and zero-end windows: [[0,0],[0,3],[5,5]]",
    ],
    [
      "testing the last end for truthiness",
      truthyEnd,
      "zero-length and zero-end windows: [[0,0],[0,3],[5,5]]",
    ],
    [
      "overwriting the end kept for a start",
      endPerStart,
      "longer window first at a shared start: [[1,4],[1,2]]",
    ],
    [
      "sizing the start table one short",
      shortTable,
      "window starting at the time limit: [[1,2],[10000,10000]]",
    ],
  ]) {
    const pinned = pinnedCase("merge-intervals", label);
    assert.equal(
      checkAnswer(judge, pinned, shortcut(...structuredClone(pinned.input))),
      false,
      `"${label}" must reject ${name}`,
    );
  }
});

test("basic-calculator cases reject parsers without a unary minus", () => {
  // One parser for the wrong ones, which differ only in when a "-" that starts
  // a term is read as a sign, and in what that sign negates. `sign` sees the
  // character right after the minus, a space included. Two of them instead put
  // a 0 before each minus they take for a sign, and read every minus as binary.
  // web/calculator.js has no such switch, so it only checks the values below.
  const digit = (c) => c >= "0" && c <= "9";
  const evaluate = (
    text,
    { sign = () => false, rest = () => false, one = false } = {},
  ) => {
    let at = 0;
    const skip = () => {
      while (text[at] === " ") at += 1;
    };
    const term = () => {
      skip();
      if (text[at] === "-" && sign(text[at + 1])) {
        at += 1;
        if (one && digit(text[at])) return -Number(text[at++]);
        return rest(text[at]) ? -expression() : -term();
      }
      if (text[at] === "(") {
        at += 1;
        const value = expression();
        skip();
        if (text[at] !== ")") throw new Error(`no ")" at ${at}`);
        at += 1;
        return value;
      }
      const start = at;
      while (digit(text[at])) at += 1;
      if (at === start) throw new Error(`no number at ${start}`);
      return Number(text.slice(start, at));
    };
    const expression = () => {
      let value = term();
      skip();
      while (text[at] === "+" || text[at] === "-") {
        const op = text[at];
        at += 1;
        value = op === "+" ? value + term() : value - term();
        skip();
      }
      return value;
    };
    const value = expression();
    if (at !== text.length) throw new Error(`unread input at ${at}`);
    return value;
  };
  const byLabel = Object.fromEntries(
    judges["basic-calculator"].cases.map(({ label, input: [s], expected }) => [
      label,
      { s, expected },
    ]),
  );
  const run = (parse, s) => {
    try {
      return parse(s);
    } catch (error) {
      return error.message;
    }
  };
  // Each wrong parser must fail the cases in its first list and still read
  // those in its second, so setting its options any other way that changes
  // which built-in cases it gets right fails the test, and it stays the parser
  // its name says. Values compare with ===, so -0 and 0 are the same integer.
  for (const [name, parse, fails, reads] of [
    [
      "the binary-minus-only parser",
      (s) => evaluate(s),
      [
        "minus before a group",
        "minus inside a group",
        "space after a leading minus",
        "space before a minus inside a group",
      ],
      ["nested subtraction"],
    ],
    [
      "the parser that prepends 0 for a leading minus",
      (s) => evaluate(s.trim().startsWith("-") ? `0${s.trim()}` : s),
      ["minus inside a group", "space before a minus inside a group"],
      ["minus on the first number only"],
    ],
    [
      "the parser that reads a minus as a sign from the character before it",
      (s) => evaluate(s.replace(/(?<=^|\()-/g, "0-")),
      ["space before a minus inside a group"],
      ["minus inside a group", "minus on the first number only"],
    ],
    [
      "the parser whose minus negates the rest of the expression",
      (s) => evaluate(s, { sign: () => true, rest: () => true }),
      ["minus on the first number only", "number added after a negated group"],
      [
        "minus before a group",
        "space after a leading minus",
        "minus before a two-digit number",
      ],
    ],
    [
      "the parser whose minus before a group negates the rest",
      (s) => evaluate(s, { sign: () => true, rest: (c) => c === "(" }),
      ["number added after a negated group"],
      [
        "minus before a group",
        "minus on the first number only",
        "space after a leading minus",
        "minus before a two-digit number",
      ],
    ],
    [
      "the parser whose unary minus only precedes a number",
      (s) => evaluate(s, { sign: digit }),
      ["minus before a group", "space after a leading minus"],
      ["minus on the first number only", "minus before a two-digit number"],
    ],
    [
      "the parser whose unary minus must touch its operand",
      (s) => evaluate(s, { sign: (c) => c === "(" || digit(c) }),
      ["space after a leading minus"],
      [
        "minus on the first number only",
        "minus before a two-digit number",
        "number added after a negated group",
      ],
    ],
    [
      "the parser whose unary minus reads one digit",
      (s) => evaluate(s, { sign: () => true, one: true }),
      ["minus before a two-digit number"],
      [
        "minus on the first number only",
        "space after a leading minus",
        "number added after a negated group",
      ],
    ],
  ]) {
    for (const label of [...fails, ...reads])
      assert.ok(
        byLabel[label],
        `the built-in cases must keep "${label}" for ${name}`,
      );
    for (const label of fails)
      assert.ok(
        run(parse, byLabel[label].s) !== byLabel[label].expected,
        `"${label}" must reject ${name}`,
      );
    for (const label of reads) {
      const { s, expected } = byLabel[label];
      const read = run(parse, s);
      assert.ok(
        read === expected,
        `${name} must still read "${label}" as ${expected}, not ${read}`,
      );
    }
  }
  // The task workspace's calculator checks every expected value within its
  // MAX_EXPRESSION_LENGTH characters, and refuses an unclosed group or input
  // left unread. It also reads `*`, functions and a plus sign of its own,
  // which the contract leaves out, so each input is held to the contract's
  // characters first. The shared parser, with its minus always a sign, checks
  // every value too, so breaking it cannot reject every wrong one.
  // The calculator gives -0 for -(1-1), so values compare with ===.
  for (const { input, expected, label } of judges["basic-calculator"].cases) {
    assert.match(input[0], /^[\d ()+-]+$/, label);
    assert.doesNotMatch(input[0], /(^|[(+-]) *\+/, label);
    if (input[0].length <= MAX_EXPRESSION_LENGTH)
      assert.ok(calculate(input[0]) === expected, label);
    assert.ok(evaluate(input[0], { sign: () => true }) === expected, label);
  }
});

test("word-break cases reject splits that go back no further than the last cut and prefix tables that ask about one term", () => {
  const flip = (text, fromEnd) =>
    fromEnd ? [...text].reverse().join("") : text;
  // Cuts one term at a time and never goes back. With `lookahead` it only
  // takes a term that the rest of the tag can start another term after.
  const split =
    (pick, fromEnd, lookahead = false) =>
    (s, vocabulary) => {
      const tag = flip(s, fromEnd);
      const terms = vocabulary.map((term) => flip(term, fromEnd));
      const followed = (at) =>
        at === tag.length || terms.some((term) => tag.startsWith(term, at));
      let at = 0;
      while (at < tag.length) {
        const fits = terms.filter(
          (term) =>
            tag.startsWith(term, at) &&
            (!lookahead || followed(at + term.length)),
        );
        if (fits.length === 0) return false;
        at += pick(fits).length;
      }
      return true;
    };
  const shortest = (fits) =>
    fits.reduce((best, term) => (term.length < best.length ? term : best));
  const longest = (fits) =>
    fits.reduce((best, term) => (term.length > best.length ? term : best));
  const first = (fits) => fits[0];
  const last = (fits) => fits.at(-1);
  // Goes back only as far as its last cut: at a dead end it cuts the next term
  // there, and gives up once that cut has no term left.
  const undoLast = (pick, fromEnd) => (s, vocabulary) => {
    const tag = flip(s, fromEnd);
    const terms = vocabulary.map((term) => flip(term, fromEnd));
    const cuts = [];
    let at = 0;
    while (at < tag.length) {
      let from = at;
      let left = terms.filter((term) => tag.startsWith(term, at));
      if (left.length === 0) {
        const cut = cuts.pop();
        if (!cut || cut.left.length === 0) return false;
        ({ from, left } = cut);
      }
      const term = pick(left);
      cuts.push({ from, left: left.filter((other) => other !== term) });
      at = from + term.length;
    }
    return true;
  };
  // Records whether each prefix splits, but asks only about the one term
  // `pick` takes among those that end the prefix, not about each of them.
  const oneTermDecides = (pick) => (s, vocabulary) => {
    const splits = [true];
    for (let end = 1; end <= s.length; end += 1) {
      const fits = vocabulary.filter((term) => s.endsWith(term, end));
      splits[end] = fits.length > 0 && splits[end - pick(fits).length];
    }
    return splits[s.length];
  };
  const picks = [
    ["the shortest term", shortest],
    ["the longest term", longest],
    ["the first listed term", first],
    ["the last listed term", last],
  ];
  const choices = picks.flatMap(([term, pick]) =>
    [false, true].flatMap((lookahead) =>
      [false, true].map((fromEnd) => [
        `${term}${lookahead ? " another term can follow" : ""}${fromEnd ? " from the end" : ""}`,
        split(pick, fromEnd, lookahead),
      ]),
    ),
  );
  const retries = picks.flatMap(([term, pick]) =>
    [false, true].map((fromEnd) => [
      `${term}${fromEnd ? " from the end" : ""} and going back only to the last cut`,
      undoLast(pick, fromEnd),
    ]),
  );
  const run = "one different letter in a run";
  for (const [name, cut] of [...choices, ...retries]) {
    assertRejects(
      "word-break",
      run,
      cut,
      `"${run}" must reject always cutting ${name}`,
    );
  }
  for (const [term, pick] of picks) {
    assertRejects(
      "word-break",
      run,
      oneTermDecides(pick),
      `"${run}" must reject asking each prefix only about ${term} that ends it`,
    );
  }
  const inside = "term inside a longer term";
  for (const lookahead of [false, true]) {
    assertRejects(
      "word-break",
      inside,
      split(shortest, false, lookahead),
      `"${inside}" must reject always cutting the shortest term${lookahead ? " another term can follow" : ""}`,
    );
  }

  // Both strings of the run case read the same reversed, so only a case that
  // does not shows the cuts from the end are made from the end. The lookahead
  // rows fail the new cases whether or not they look ahead, so only a case
  // where looking ahead changes the answer shows that they look. Every row
  // misses the run case whichever term it cuts, so the term each one picks is
  // pinned on overlapping words, which lists car before ca: from the front
  // the shortest and the last listed term split it, while the longest and
  // the first listed term stop at s. Going back to the last cut fails the run
  // case too, so overlapping words, which splits only once car is undone,
  // shows that it goes back, and catsdog, which it splits from the front only
  // after cats, pins its term and its direction. A prefix table fails the run
  // case whichever term it asks about, and no case tells those terms apart, so
  // ab over ab and b does: only the longest and the first listed term that end
  // it reach the start.
  const { input, expected } = pinnedCase("word-break", inside);
  assert.equal(
    split(shortest, true)(...input),
    expected,
    `"${inside}" must split when the shortest term is cut from the end`,
  );
  const cars = pinnedCase("word-break", "overlapping words");
  assert.equal(
    split(longest, false, true)(...cars.input),
    cars.expected,
    `"overlapping words" must split when the longest term another term can follow is cut`,
  );
  assert.equal(
    undoLast(longest, false)(...cars.input),
    cars.expected,
    `"overlapping words" must split when the longest term is cut and the last cut is gone back to`,
  );
  for (const [name, pick, splits] of [
    ["the shortest term", shortest, false],
    ["the first listed term", first, false],
    ["the longest term", longest, true],
    ["the last listed term", last, true],
  ]) {
    assert.equal(
      undoLast(pick, false)(...input) === expected,
      splits,
      `"${inside}" must ${splits ? "split" : "reject"} when ${name} is cut and the last cut is gone back to`,
    );
  }
  assert.equal(
    undoLast(shortest, true)(...input),
    expected,
    `"${inside}" must split when the shortest term is cut from the end and the last cut is gone back to`,
  );
  for (const [name, pick, splits] of [
    ["the shortest term", shortest, false],
    ["the last listed term", last, false],
    ["the longest term", longest, true],
    ["the first listed term", first, true],
  ]) {
    assert.equal(
      oneTermDecides(pick)("ab", ["ab", "b"]),
      splits,
      `"ab" over ab and b must ${splits ? "split" : "not split"} when each prefix asks only about ${name} that ends it`,
    );
  }
  for (const [name, pick, splits] of [
    ["the shortest term", shortest, true],
    ["the last listed term", last, true],
    ["the longest term", longest, false],
    ["the first listed term", first, false],
  ]) {
    assert.equal(
      split(pick, false)(...cars.input) === cars.expected,
      splits,
      `"overlapping words" must ${splits ? "split" : "reject"} when ${name} is cut`,
    );
  }

  // A wrong solution missing an answer says nothing if the answer is wrong, so
  // each new case carries a split that shows it is true.
  for (const [label, pieces] of [
    [inside, ["cats", "dog"]],
    [run, ["a", "aabaa", "a"]],
  ]) {
    const {
      input: [s, vocabulary],
      expected,
    } = pinnedCase("word-break", label);
    assert.equal(expected, true, `"${label}" must expect a split`);
    assert.ok(
      pieces.every((piece) => vocabulary.includes(piece)) &&
        pieces.join("") === s,
      `"${label}" must split as ${pieces.join(" + ")}`,
    );
  }
});

test("stock III cases reject the two-runs shortcut and its greedy join", () => {
  const best = (prices) => {
    let [buy1, sell1, buy2, sell2] = [-Infinity, 0, -Infinity, 0];
    for (const p of prices) {
      buy1 = Math.max(buy1, -p);
      sell1 = Math.max(sell1, buy1 + p);
      buy2 = Math.max(buy2, sell1 - p);
      sell2 = Math.max(sell2, buy2 + p);
    }
    return sell2;
  };
  // Each maximal non-falling stretch that ends higher than it starts.
  const runs = (prices) => {
    const found = [];
    for (let i = 0; i < prices.length - 1;) {
      let j = i;
      while (j + 1 < prices.length && prices[j + 1] >= prices[j]) j += 1;
      if (prices[j] > prices[i]) found.push([prices[i], prices[j]]);
      i = j + 1;
    }
    return found;
  };
  const twoLargest = (found) =>
    found
      .map(([low, high]) => high - low)
      .sort((a, b) => b - a)
      .slice(0, 2)
      .reduce((a, b) => a + b, 0);
  // The patch a candidate tries after the two-runs shortcut fails: fold two
  // neighbouring runs into one trade whenever that trade gains more than
  // either run (or at least as much, on a tie), rescanning from the left
  // after each fold, until two runs remain or no pair gains from a fold.
  const joined = (foldOnTie) => (prices) => {
    const found = runs(prices);
    for (let k = 0; found.length > 2 && k < found.length - 1;) {
      const [[a, b], [c, d]] = [found[k], found[k + 1]];
      const joint = d - a;
      const single = Math.max(b - a, d - c);
      if (foldOnTie ? joint >= single : joint > single) {
        found.splice(k, 2, [a, d]);
        k = 0;
      } else {
        k += 1;
      }
    }
    return twoLargest(found);
  };
  const { cases } = judges["best-time-to-buy-and-sell-stock-iii"];
  for (const {
    label,
    input: [prices],
    expected,
  } of cases) {
    assert.equal(
      best(prices),
      expected,
      `"${label}" must expect the best two trades`,
    );
  }
  for (const [name, solve, label] of [
    [
      "the two largest rising runs",
      (prices) => twoLargest(runs(prices)),
      "best trades span a dip",
    ],
    [
      "runs joined whenever the joined trade beats each run",
      joined(false),
      "small rise then two equal rises",
    ],
    [
      "runs joined whenever the joined trade ties or beats each run",
      joined(true),
      "small rise then two equal rises",
    ],
  ]) {
    assertRejects(
      "best-time-to-buy-and-sell-stock-iii",
      label,
      solve,
      `"${label}" must reject ${name}`,
    );
  }
  // Both joining rows miss the same case, so only an input where they part
  // shows that `foldOnTie` decides anything. On [0,1,0,3,1,4] the tie variant
  // folds the first two runs and keeps the best two trades, which is also
  // why that input is not a judge case.
  assert.equal(joined(false)([0, 1, 0, 3, 1, 4]), 5);
  assert.equal(joined(true)([0, 1, 0, 3, 1, 4]), 6);
});

const jumpGameIICorrect = (nums) => {
  let jumps = 0;
  let end = 0;
  let far = 0;
  for (let i = 0; i < nums.length - 1; i += 1) {
    far = Math.max(far, i + nums[i]);
    if (i === end) {
      jumps += 1;
      end = far;
    }
  }
  return jumps;
};
const jumpGameIIChoosePad = (score) => (nums) => {
  let pad = 0;
  let jumps = 0;
  while (pad < nums.length - 1) {
    const end = Math.min(pad + nums[pad], nums.length - 1);
    if (end === nums.length - 1) return jumps + 1;
    if (end <= pad) return -1;
    let next = pad + 1;
    for (let k = next + 1; k <= end; k += 1) {
      if (score(k, nums) >= score(next, nums)) next = k;
    }
    pad = next;
    jumps += 1;
  }
  return jumps;
};
const jumpGameIILargestReach = jumpGameIIChoosePad((k, nums) => k + nums[k]);
const jumpGameIIMinimumHops = (nums) => {
  const distance = Array(nums.length).fill(Infinity);
  distance[0] = 0;
  const queue = [0];
  for (let at = 0; at < queue.length; at += 1) {
    const pad = queue[at];
    for (
      let next = pad + 1;
      next < nums.length && next <= pad + nums[pad];
      next += 1
    ) {
      if (distance[next] === Infinity) {
        distance[next] = distance[pad] + 1;
        queue.push(next);
      }
    }
  }
  return distance.at(-1);
};

test("jump-game-ii cases reject hop-count and landing shortcuts", () => {
  const reported = (nums) => {
    let jumps = 0;
    let end = 0;
    let far = 0;
    for (let i = 0; i < nums.length; i += 1) {
      far = Math.max(far, i + nums[i]);
      if (i === end) {
        jumps += 1;
        end = far;
      }
    }
    return nums.length > 1 ? jumps - 1 : 0;
  };
  const rejectsZeroDestination = (nums) =>
    nums.length > 1 && nums.at(-1) === 0 ? -1 : jumpGameIICorrect(nums);
  const firstHopOnly = (nums) =>
    nums.length === 1 ? 0 : nums[0] >= nums.length - 1 ? 1 : reported(nums);
  const choosePastDestination = (preferLater) => (nums) => {
    let pad = 0;
    let jumps = 0;
    while (pad < nums.length - 1) {
      const end = Math.min(pad + nums[pad], nums.length - 1);
      if (end <= pad) return -1;
      let next = pad + 1;
      for (let k = next + 1; k <= end; k += 1) {
        const reach = k + nums[k];
        const best = next + nums[next];
        if (reach > best || (preferLater && reach === best)) next = k;
      }
      pad = next;
      jumps += 1;
    }
    return jumps;
  };
  const countReachGrowth = (nums, stopWhenReachable = false) => {
    let far = 0;
    let jumps = 0;
    for (let i = 0; i < nums.length - 1; i += 1) {
      if (stopWhenReachable && far >= nums.length - 1) break;
      if (i + nums[i] > far) {
        far = i + nums[i];
        jumps += 1;
      }
    }
    return jumps;
  };
  const countReachGrowthEarlyExit = (nums) => countReachGrowth(nums, true);
  const stopOnReachOvershoot = (nums) => {
    let jumps = 0;
    let end = 0;
    let far = 0;
    for (let i = 0; i < nums.length - 1; i += 1) {
      if (far > nums.length - 1) break;
      far = Math.max(far, i + nums[i]);
      if (i === end) {
        jumps += 1;
        end = far;
      }
    }
    return jumps;
  };
  const firstFlightOnly = (preferLater) => (nums) =>
    nums.length > 1 && nums[0] >= nums.length - 1
      ? 1
      : choosePastDestination(preferLater)(nums);
  const backwardLargestValue = (preferLater) => (nums) => {
    let pad = nums.length - 1;
    let jumps = 0;
    while (pad > 0) {
      let previous = -1;
      for (let k = 0; k < pad; k += 1) {
        if (
          k + nums[k] >= pad &&
          (previous < 0 ||
            nums[k] > nums[previous] ||
            (preferLater && nums[k] === nums[previous]))
        ) {
          previous = k;
        }
      }
      if (previous < 0) return -1;
      pad = previous;
      jumps += 1;
    }
    return jumps;
  };
  const positiveDirectFirst = (nums) =>
    nums[0] > 0 && nums[0] >= nums.length - 1 ? 1 : jumpGameIICorrect(nums);
  const rejectsExactFirst = (nums) =>
    nums.length > 1 && nums[0] === nums.length - 1
      ? 2
      : jumpGameIICorrect(nums);
  const rejectsConsecutiveZeros = (nums) =>
    nums.some((value, i) => i > 0 && value === 0 && nums[i - 1] === 0)
      ? -1
      : jumpGameIICorrect(nums);
  const largestValue = jumpGameIIChoosePad((k, nums) => nums[k]);
  // The judge can reject both variants even if they use the same tie policy.
  // These synthetic controls keep earlier and later ties distinct for the
  // destination, first-flight repair, and backward predecessor shortcuts.
  assert.equal(
    choosePastDestination(false)([2, 1, 0]),
    2,
    "an earlier reach tie keeps the intermediate pad",
  );
  assert.equal(
    choosePastDestination(true)([2, 1, 0]),
    1,
    "a later reach tie selects the destination",
  );
  const tied = pinnedCase("jump-game-ii", "three hops needed");
  assert.equal(
    largestValue(tied.input[0]),
    tied.expected,
    "equal jump values must prefer the later pad",
  );
  const growing = pinnedCase(
    "jump-game-ii",
    "two pads in range both reach farther",
  );
  assert.equal(
    countReachGrowth(growing.input[0]),
    3,
    "three reach extensions are not three flights",
  );
  assert.equal(
    countReachGrowthEarlyExit(growing.input[0]),
    2,
    "a reachable destination stops the reach-growth counter",
  );
  assert.equal(
    stopOnReachOvershoot(pinnedCase("jump-game-ii", "two hops").input[0]),
    2,
    "an exact reach does not trigger the overshoot exit",
  );
  for (const later of [false, true]) {
    for (const nums of [
      [2, 2, 0],
      [3, 3, 0],
    ]) {
      assert.equal(
        firstFlightOnly(later)(nums),
        1,
        "the first-flight repair must take exact and overshooting arrivals",
      );
    }
  }
  assert.equal(
    firstFlightOnly(false)([1, 2, 1, 0]),
    3,
    "the first-flight repair keeps earlier ties on later flights",
  );
  assert.equal(
    firstFlightOnly(true)([1, 2, 1, 0]),
    2,
    "the first-flight repair keeps later ties on later flights",
  );
  assert.equal(
    backwardLargestValue(false)([1, 2, 2, 0]),
    2,
    "the backward raw-value chooser must keep earlier ties",
  );
  assert.equal(
    backwardLargestValue(true)([1, 2, 2, 0]),
    3,
    "the backward raw-value chooser must keep later ties",
  );
  for (const [name, wrong, label] of [
    [
      "the reported final-hop subtraction",
      reported,
      "two pads with room to spare",
    ],
    [
      "the reported final-hop subtraction",
      reported,
      "three pads with room to spare",
    ],
    [
      "the reported final-hop subtraction",
      reported,
      "range grows after the first pad",
    ],
    [
      "choosing the largest raw predecessor with earlier ties",
      backwardLargestValue(false),
      "last pad shares the second flight's range",
    ],
    [
      "choosing the largest raw predecessor with later ties",
      backwardLargestValue(true),
      "last pad shares the second flight's range",
    ],
    [
      "counting reach extensions with a reachable-destination early exit",
      countReachGrowthEarlyExit,
      "three charged pads followed by two empty pads",
    ],
    [
      "counting reach extensions without an early exit",
      countReachGrowth,
      "three charged pads followed by two empty pads",
    ],
    [
      "stopping on an overshoot before counting the pending flight",
      stopOnReachOvershoot,
      "second pad has room beyond the destination",
    ],
    [
      "repairing direct arrivals only on the first flight with earlier ties",
      firstFlightOnly(false),
      "last pad shares the second flight's range",
    ],
    [
      "repairing direct arrivals only on the first flight with later ties",
      firstFlightOnly(true),
      "last pad shares the second flight's range",
    ],
    [
      "choosing past the destination with a later tie",
      choosePastDestination(true),
      "last pad in range beside a farther-reaching pad",
    ],
    [
      "choosing past the destination with an earlier tie",
      choosePastDestination(false),
      "last pad in range beside a farther-reaching pad",
    ],
    [
      "counting each reach extension as a flight",
      countReachGrowth,
      "two pads in range both reach farther",
    ],
    [
      "checking a direct flight before a singleton",
      positiveDirectFirst,
      "one pad with charge remaining",
    ],
    [
      "requiring the first range to pass the destination",
      rejectsExactFirst,
      "two pads with just enough range",
    ],
    [
      "rejecting consecutive empty pads",
      rejectsConsecutiveZeros,
      "empty pads between charging stops",
    ],
    [
      "treating a zero-valued destination as unreachable",
      rejectsZeroDestination,
      "two pads with room to spare",
    ],
    [
      "choosing the largest pad value with a later tie",
      largestValue,
      "weaker pad sits closer to the end",
    ],
    [
      "a first-hop-only repair",
      firstHopOnly,
      "range grows after the first pad",
    ],
  ]) {
    assertRejects(
      "jump-game-ii",
      label,
      wrong,
      `"${label}" must reject ${name}`,
    );
  }
});

test("jump-game-ii reference strategies match shortest paths", () => {
  for (const { label, input, expected } of judges["jump-game-ii"].cases) {
    assert.equal(
      jumpGameIICorrect(input[0]),
      expected,
      `"${label}" must accept the correct greedy solution`,
    );
    assert.equal(
      jumpGameIILargestReach(input[0]),
      expected,
      `"${label}" must accept choosing the largest reach`,
    );
    assert.equal(
      jumpGameIIMinimumHops(input[0]),
      expected,
      `"${label}" must match shortest paths`,
    );
  }
  let checked = 0;
  for (let size = 1; size <= 5; size += 1) {
    for (let encoded = 0; encoded < 4 ** size; encoded += 1) {
      let digits = encoded;
      const nums = Array.from({ length: size }, () => {
        const value = digits % 4;
        digits = Math.floor(digits / 4);
        return value;
      });
      const expected = jumpGameIIMinimumHops(nums);
      if (expected === Infinity) continue;
      checked += 1;
      assert.equal(jumpGameIICorrect(nums), expected, JSON.stringify(nums));
      assert.equal(
        jumpGameIILargestReach(nums),
        expected,
        JSON.stringify(nums),
      );
    }
  }
  assert.equal(checked, 828);
  for (const [nums, expected] of [
    [Array(10000).fill(1), 9999],
    [Array(10000).fill(1000), 10],
    [Array.from({ length: 10000 }, (_, i) => (i % 1000 === 0 ? 1000 : 0)), 10],
    [[1000, 0], 1],
  ]) {
    assert.equal(jumpGameIIMinimumHops(nums), expected);
    assert.equal(jumpGameIICorrect(nums), expected);
    assert.equal(jumpGameIILargestReach(nums), expected);
  }
});

test("valid-sudoku cases reject a check that misses a rule or a unit", () => {
  const all = ["row", "column", "box"];
  // Records each filled cell under the units its `rules` name and refuses a
  // digit a unit has already seen. A wrong check leaves out a rule, the rows
  // and columns outside `from` to `to`, or the boxes, numbered in reading
  // order, that `boxes` does not list.
  const consistent =
    ({ rules = all, from = 0, to = 9, boxes = [...Array(9).keys()] } = {}) =>
    (board) => {
      const seen = new Set();
      const inside = (index) => index >= from && index < to;
      for (let r = 0; r < 9; r += 1) {
        for (let c = 0; c < 9; c += 1) {
          const digit = board[r][c];
          if (digit === ".") continue;
          const box = 3 * Math.floor(r / 3) + Math.floor(c / 3);
          const units = {
            row: inside(r) ? `row ${r}` : "",
            column: inside(c) ? `column ${c}` : "",
            box: boxes.includes(box) ? `box ${box}` : "",
          };
          for (const rule of rules) {
            if (!units[rule]) continue;
            const key = `${units[rule]} ${digit}`;
            if (seen.has(key)) return false;
            seen.add(key);
          }
        }
      }
      return true;
    };
  const every = consistent();
  for (const { label, input, expected } of judges["valid-sudoku"].cases) {
    assert.equal(
      every(...structuredClone(input)),
      expected,
      `"${label}" must expect whether any row, column or box repeats a digit`,
    );
  }
  // Each of these cases repeats a digit in one unit only, so the check that
  // misses that unit is the one wrong check that accepts it. The repeats sit
  // in the first and last row and column, in the last box and in a box off
  // the diagonal and the corners, so loops that start or stop one short, or
  // step through the boxes by three, are refused too.
  for (const [label, wrong, misses] of [
    ["duplicate in box only", { rules: ["row", "column"] }, "the box rule"],
    ["duplicate in far box only", { rules: ["row", "column"] }, "the box rule"],
    ["duplicate in far box only", { boxes: [0] }, "all but the top-left box"],
    [
      "duplicate in edge box only",
      { rules: ["row", "column"] },
      "the box rule",
    ],
    [
      "duplicate in edge box only",
      { boxes: [0, 4, 8] },
      "the boxes off the diagonal",
    ],
    [
      "duplicate in edge box only",
      { boxes: [0, 2, 6, 8] },
      "all but the corner boxes",
    ],
    ["duplicate in last column only", { rules: ["row", "box"] }, "columns"],
    ["duplicate in last column only", { to: 8 }, "the last row and column"],
    ["duplicate in last row only", { rules: ["column", "box"] }, "rows"],
    ["duplicate in last row only", { to: 8 }, "the last row and column"],
    ["duplicate in first column only", { rules: ["row", "box"] }, "columns"],
    ["duplicate in first column only", { from: 1 }, "the first row and column"],
    ["duplicate in first row only", { rules: ["column", "box"] }, "rows"],
    ["duplicate in first row only", { from: 1 }, "the first row and column"],
  ]) {
    assertRejects(
      "valid-sudoku",
      label,
      consistent(wrong),
      `"${label}" must reject a check that skips ${misses}`,
    );
  }
  // No other true case is filled in, so a check that wants a blank cell
  // passed them all.
  assertRejects(
    "valid-sudoku",
    "fully filled board",
    (board) => board.some((row) => row.includes(".")) && every(board),
    '"fully filled board" must reject a check that requires a blank cell',
  );
  // Fills the first blank cell with each digit the board still allows and
  // backtracks, so it finds a completion if one exists.
  const solvable = (board) => {
    const r = board.findIndex((row) => row.includes("."));
    if (r < 0) return true;
    const c = board[r].indexOf(".");
    return [..."123456789"].some((digit) => {
      board[r][c] = digit;
      const found = every(board) && solvable(board);
      board[r][c] = ".";
      return found;
    });
  };
  // Every other true case can be completed, so a check that also asks for a
  // completion passed them all, and the solver must find each completion.
  for (const { label, input, expected } of judges["valid-sudoku"].cases) {
    if (!expected || label === "another partial board") continue;
    assert.ok(
      solvable(...structuredClone(input)),
      `"${label}" must have a completion the solver finds`,
    );
  }
  assertRejects(
    "valid-sudoku",
    "another partial board",
    (board) => every(board) && solvable(board),
    '"another partial board" must reject a check that requires a completion',
  );
});

test("mirrored tag cases reject each wrong scan by name", async (t) => {
  // The brief says the scanner reads each tag forwards and backwards, so a
  // candidate's first idea is the longest common substring of the read and
  // its reversed copy, never checking that the match itself is mirrored.
  // Two stretches can tie for longest, so both picks are run.
  const commonSubstring = (keepLater) => (s) => {
    const r = [...s].reverse().join("");
    let best = 0;
    let end = 0;
    let prev = new Array(s.length + 1).fill(0);
    for (let i = 1; i <= s.length; i += 1) {
      const cur = new Array(s.length + 1).fill(0);
      for (let j = 1; j <= s.length; j += 1) {
        if (s[i - 1] === r[j - 1]) {
          cur[j] = prev[j - 1] + 1;
          if (keepLater ? cur[j] >= best : cur[j] > best) {
            best = cur[j];
            end = i;
          }
        }
      }
      prev = cur;
    }
    return s.slice(end - best, end);
  };
  // Centre expansion, written once: each wrong scan built on it below differs
  // from it in one way, so it reads as the one mistake it makes.
  const scan =
    ({
      visits = () => true,
      same = (x, y) => x === y,
      keep = (best, found) => found.length > best.length,
      spans = [0, 1],
    } = {}) =>
    (s) => {
      let best = s[0];
      for (let centre = 0; centre < s.length; centre += 1) {
        for (const span of spans) {
          if (!visits(centre, span, s)) continue;
          let a = centre;
          let b = centre + span;
          while (a >= 0 && b < s.length && same(s[a], s[b])) {
            a -= 1;
            b += 1;
          }
          const found = s.slice(a + 1, b);
          if (keep(best, found)) best = found;
        }
      }
      return best;
    };
  const expand = scan();
  const spec = judges["longest-palindromic-substring"];
  // The longest palindrome in this read is 3 characters, so every common
  // substring of length 4 or more fails on length and palindromicity alike.
  // The two picks part on this read, which is what shows `keepLater` decides
  // anything, and each is 5 characters.
  for (const [name, solve, copied] of [
    ["the earlier stretch on a tie", commonSubstring(false), "abacd"],
    ["the later stretch on a tie", commonSubstring(true), "dcaba"],
  ]) {
    await t.test(`the reversed-copy shortcut keeping ${name}`, () => {
      const pinned = pinnedCase(
        "longest-palindromic-substring",
        'mirrored ends: s="abacdfgdcaba"',
      );
      const answer = solve(...structuredClone(pinned.input));
      assert.equal(
        answer,
        copied,
        `the shortcut keeping ${name} must copy "${copied}" from the read`,
      );
      assert.equal(
        checkAnswer(spec, pinned, answer),
        false,
        `"${pinned.label}" must reject the reversed-copy shortcut keeping ${name}`,
      );
    });
  }
  // The patch a candidate skips: an adjacent equal pair looks finished as it
  // stands, so even mirrors are never expanded while odd centres are. Only a
  // read whose longest mirror is even and longer than two tells them apart.
  const pairOnly = (s) => {
    let best = "";
    for (let centre = 0; centre < s.length; centre += 1) {
      let a = centre;
      let b = centre;
      while (a >= 0 && b < s.length && s[a] === s[b]) {
        a -= 1;
        b += 1;
      }
      if (b - a - 1 > best.length) best = s.slice(a + 1, b);
      if (
        best.length < 2 &&
        centre + 1 < s.length &&
        s[centre] === s[centre + 1]
      ) {
        best = s.slice(centre, centre + 2);
      }
    }
    return best;
  };
  // Each scan the table below runs is pinned to a read where its mistake
  // loses the longest stretch; all but pairOnly are built on centre expansion.
  const edgeSkipped = scan({
    visits: (centre, _, s) => centre > 0 && centre < s.length - 1,
  });
  const lastPairSkipped = scan({
    visits: (centre, span, s) => span === 0 || centre < s.length - 2,
  });
  // Choosing one kind of centre at each position from the characters beside
  // it can lose a stretch whose centre sits inside a run of three or more
  // equal characters, where those characters no longer tell the two apart.
  const nextDecides = scan({
    visits: (centre, span, s) => (s[centre] === s[centre + 1]) === (span === 1),
  });
  const neighboursDecide = scan({
    visits: (centre, span, s) =>
      (s[centre - 1] === s[centre + 1]) === (span === 0),
  });
  const caseFolded = scan({
    same: (x, y) => x.toLowerCase() === y.toLowerCase(),
  });
  const lettersOnly = (s) => expand(s.replace(/[^A-Za-z]/g, ""));
  const capitalsDropped = (s) => expand(s.replace(/[^a-z0-9]/g, ""));
  const shortRead = (s) => (s.length <= 2 ? s[0] : expand(s));
  const grown = (keepLast) =>
    scan({
      keep: (best, found) =>
        found.length > 1 && (keepLast || best.length === 1),
    });
  // Odd and even centres compete only where both grow past one character,
  // and a scan that settles one kind before trying the other can lose there.
  const kindFirst = (first, then) => (s) => {
    const found = scan({ spans: [first] })(s);
    return found.length > 1 ? found : scan({ spans: [then] })(s);
  };
  for (const [name, solve, label, returned] of [
    [
      "a pair that was never expanded",
      pairOnly,
      'whole string even: s="abba"',
      "bb",
    ],
    [
      "centres that skip the ends",
      edgeSkipped,
      'matching prefix: s="aab"',
      "a",
    ],
    [
      "even centres that stop before the last pair",
      lastPairSkipped,
      'matching suffix: s="baa"',
      "b",
    ],
    ["a case-folded comparison", caseFolded, 'mixed case: s="aA"', "aA"],
    [
      "a letters-only read",
      lettersOnly,
      'digit at the centre: s="ab1ba"',
      "abba",
    ],
    [
      "a read without capitals",
      capitalsDropped,
      'capital at the centre: s="abBba"',
      "abba",
    ],
    [
      "the first stretch past one character",
      grown(false),
      'middle mirror: s="abacdedcfgf"',
      "aba",
    ],
    [
      "the last stretch past one character",
      grown(true),
      'middle mirror: s="abacdedcfgf"',
      "fgf",
    ],
    [
      "odd centres settled first",
      kindFirst(0, 1),
      'even mirror beats odd mirror: s="abacca"',
      "aba",
    ],
    [
      "even centres settled first",
      kindFirst(1, 0),
      'odd mirror beats even mirror: s="aabcb"',
      "aa",
    ],
    [
      "one centre kind chosen from the next character",
      nextDecides,
      'run of three at the centre: s="baaab"',
      "aa",
    ],
    [
      "one centre kind chosen from the two neighbours",
      neighboursDecide,
      'run of four at the centre: s="baaaab"',
      "aaa",
    ],
    [
      "a read of two answered by its first character",
      shortRead,
      'two equal characters: s="aa"',
      "a",
    ],
  ]) {
    await t.test(`${name} on "${label}"`, () => {
      const row = pinnedCase("longest-palindromic-substring", label);
      const got = solve(...structuredClone(row.input));
      assert.equal(
        got,
        returned,
        `the scan with ${name} must return "${returned}" for "${row.input[0]}"`,
      );
      assert.equal(
        checkAnswer(spec, row, got),
        false,
        `"${row.label}" must reject the scan with ${name}`,
      );
    });
  }

  assertSolves(
    "longest-palindromic-substring",
    expand,
    (label) => `"${label}" must accept centre expansion`,
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

// Parse the extracted class: the starter may document it inside a comment.
// Only list boundaries and defaults come from syntax, not member-set policy.
// Syntax does not resolve names or overloads; targets are checked by arity.
function cppConstructorLists(body, name, where, parse = cppSyntax.parseSyntax) {
  const prefix = `struct ${name} {`;
  const source = prefix + body + "};";
  const tree = parse(source, "cpp");
  assert.ok(
    tree,
    `${where}: C++ parser unavailable or exceeded its parse budget for ${name}`,
  );
  try {
    assert.ok(
      !tree.rootNode.hasError,
      `${where}: invalid or incomplete C++ constructor syntax for ${name}`,
    );
    const lists = new Map();
    const classBody = tree.rootNode.namedChildren[0].childForFieldName("body");
    for (const definition of classBody.namedChildren) {
      if (definition.type !== "function_definition") continue;
      const declarator = definition.childForFieldName("declarator");
      if (declarator?.childForFieldName("declarator")?.text !== name) continue;
      const node = declarator.childForFieldName("parameters");
      assert.ok(
        !node.children.some((child) => child.type === "..."),
        `${where}: unsupported C++ parameter syntax for ${name}`,
      );
      const parameters = node.namedChildren.filter(
        (param) => param.type !== "comment",
      );
      const params = (
        parameters.length === 1 && parameters[0].text === "void"
          ? []
          : parameters
      ).map((param) => {
        assert.ok(
          ["parameter_declaration", "optional_parameter_declaration"].includes(
            param.type,
          ) && param.text !== "void",
          `${where}: unsupported C++ parameter syntax for ${name}`,
        );
        const equal = param.children.find((child) => child.type === "=");
        return {
          ...cppDeclaration(
            source.slice(param.startIndex, equal?.startIndex ?? param.endIndex),
          ),
          optional: param.childForFieldName("default_value") !== null,
        };
      });
      lists.set(node.startIndex - prefix.length, {
        end: node.endIndex - prefix.length - 1,
        params,
      });
      const initializers = definition.namedChildren.find(
        (child) => child.type === "field_initializer_list",
      );
      const entries = (initializers?.namedChildren ?? []).filter(
        (child) => child.type === "field_initializer",
      );
      for (const initializer of entries) {
        if (initializer.namedChildren[0]?.text !== name) continue;
        assert.ok(
          entries.length === 1,
          `${where}: ${name}(${params.map((param) => param.type).join(", ")}) delegates alongside other initializers`,
        );
        const args = initializer.namedChildren.find((child) =>
          ["argument_list", "initializer_list"].includes(child.type),
        );
        assert.ok(args, `${where}: incomplete C++ initializer for ${name}`);
        lists.set(args.startIndex - prefix.length, {
          end: args.endIndex - prefix.length - 1,
          arity: args.namedChildren.filter((child) => child.type !== "comment")
            .length,
        });
      }
    }
    return lists;
  } finally {
    tree.delete();
  }
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
  const lists = cppConstructorLists(body, name, where);

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
    const signature = lists.get(paramsOpen);
    assert.ok(signature?.params, shape);
    const params = signature.params;
    at = signature.end + 1;
    skip();
    const initialized = [];
    let delegationArity = null;
    if (body[at] === ":") {
      do {
        at += 1;
        skip();
        const member = body.slice(at).match(/^\w+/)?.[0] ?? "";
        at += member.length;
        skip();
        let argsClose = -1;
        if (member === name) {
          const args = lists.get(at);
          assert.ok(args && args.arity !== undefined, shape);
          argsClose = args.end;
          delegationArity = args.arity;
        } else if ("({".includes(body[at])) argsClose = closingIndex(body, at);
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
      delegationArity: delegates ? delegationArity : null,
      sets: (field) =>
        delegates || initialized.includes(field) || assigned.has(field),
    });
  }
  // Targets can appear after their delegates, so check the complete list.
  // Arity cannot identify the actual overload: TreeNode(int) may delegate to
  // itself even when a double overload accepts the same argument count.
  // Full C++ overload resolution is outside this checker.
  const accepts = (target, arity) =>
    arity >= target.required && arity <= target.types.length;
  // Detect cycles only along chains with a single arity-compatible target
  // at every step; ambiguous targets keep the existing initialization credit.
  for (const start of constructors) {
    const seen = new Set();
    for (let at = start; at.delegationArity !== null;) {
      seen.add(at);
      const targets = constructors.filter(
        (target) => target !== at && accepts(target, at.delegationArity),
      );
      assert.ok(
        targets.length,
        `${where}: ${name}(${at.types.join(", ")}) delegates with ${at.delegationArity} arguments, which no other constructor accepts`,
      );
      if (targets.length > 1) break;
      [at] = targets;
      assert.ok(
        !seen.has(at),
        `${where}: ${name}(${start.types.join(", ")}) delegates in a cycle`,
      );
    }
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

test("the C++ node class reader catches the drift it compares", async () => {
  await cppSyntax.prepareLanguage("cpp");
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

describe("C++ delegation target validation", () => {
  before(() => cppSyntax.prepareLanguage("cpp"));
  const direct = "TreeNode() : val(0), left(nullptr), right(nullptr) {}";
  const pair =
    "TreeNode(int x, int y) : val(x), left(nullptr), right(nullptr) {}";
  const defaults =
    "TreeNode(int x, TreeNode *l = nullptr, TreeNode *r = nullptr) : val(x), left(l), right(r) {}";
  const source = (constructors) =>
    `struct TreeNode { int val; TreeNode *left; TreeNode *right; ${constructors} };`;
  const read = (text) => cppClass(text, "TreeNode", "inline");
  const check = (constructors) =>
    assertPointersSet("inline", "TreeNode", read(source(constructors)));

  it("accepts a void parameter list as a zero-argument target", () => {
    assert.doesNotThrow(() =>
      check(
        "TreeNode(int x) : TreeNode() {} TreeNode(void) : val(0), left(nullptr), right(nullptr) {}",
      ),
    );
  });

  for (const [params, args] of [
    ["void, int x", "1"],
    ["void, void", ""],
    ["int x, void", "1"],
  ]) {
    it(`rejects mixed void parameters: ${params}`, () => {
      assert.throws(
        () =>
          check(
            `TreeNode() : TreeNode(${args}) {} TreeNode(${params}) : val(0), left(nullptr), right(nullptr) {}`,
          ),
        { message: "inline: unsupported C++ parameter syntax for TreeNode" },
      );
    });
  }

  for (const [params, required, total] of [
    ["/* before */ void /* after */", 0, 0],
    ["void *p", 1, 1],
    ["void (*callback)(void)", 1, 1],
    ["int x, int y", 2, 2],
    ["int x, int y = 2", 1, 2],
  ]) {
    it(`preserves parameter arity: ${params}`, () => {
      const declared = read(
        source(
          `TreeNode(${params}) : val(0), left(nullptr), right(nullptr) {}`,
        ),
      );
      assertPointersSet("inline", "TreeNode", declared);
      assert.equal(declared.constructors.length, 1);
      assert.equal(declared.constructors[0].required, required);
      assert.equal(declared.constructors[0].types.length, total);
    });
  }

  it("rejects unsupported variadic constructor parameters", () => {
    assert.throws(
      () =>
        check(
          "TreeNode(int x, ...) : val(x), left(nullptr), right(nullptr) {}",
        ),
      { message: "inline: unsupported C++ parameter syntax for TreeNode" },
    );
  });

  it("rejects a delegation cycle", () => {
    assert.throws(
      () =>
        check("TreeNode() : TreeNode(1) {} TreeNode(int x) : TreeNode() {}"),
      {
        message: "inline: TreeNode() delegates in a cycle",
      },
    );
  });

  for (const [label, constructors] of [
    [
      "a chain entering a cycle",
      "TreeNode() : TreeNode(1) {} TreeNode(int x) : TreeNode(1, 2) {} TreeNode(int x, int y) : TreeNode(1) {}",
    ],
    [
      "a cycle through a defaulted target",
      "TreeNode() : TreeNode(1) {} TreeNode(int x, int y = 2) : TreeNode() {}",
    ],
  ]) {
    it(`rejects ${label}`, () => {
      assert.throws(() => check(constructors), {
        message: "inline: TreeNode() delegates in a cycle",
      });
    });
  }

  for (const [label, target, initializers] of [
    [
      "delegation first with a missing target",
      "TreeNode(int x) : val(x), left(nullptr), right(nullptr) {}",
      "TreeNode(), left(nullptr), right(nullptr)",
    ],
    [
      "delegation in the middle with an existing target",
      direct,
      "left(nullptr), TreeNode(), right(nullptr)",
    ],
  ]) {
    it(`rejects mixed initializers: ${label}`, () => {
      assert.throws(
        () => check(`${target} TreeNode(int x, int y) : ${initializers} {}`),
        {
          message:
            "inline: TreeNode(int, int) delegates alongside other initializers",
        },
      );
    });
  }

  for (const [label, constructors, signature, arity] of [
    ["missing target", "TreeNode(int x) : TreeNode() {}", "int", 0],
    ["self-only target", "TreeNode(int x) : TreeNode(1) {}", "int", 1],
    ["too few arguments", `TreeNode() : TreeNode(1) {} ${pair}`, "", 1],
    [
      "too many arguments",
      `${direct} TreeNode(int x) : TreeNode(1, 2) {}`,
      "int",
      2,
    ],
    [
      "template comma is not two arguments",
      "TreeNode() : TreeNode(std::pair<int, int>{}) {} TreeNode(int x, int y) : val(x), left(nullptr), right(nullptr) {}",
      "",
      1,
    ],
    [
      "later invalid delegate",
      `${direct} TreeNode(int x) : TreeNode() {} TreeNode(int x, int y) : TreeNode(1, 2, 3) {}`,
      "int, int",
      3,
    ],
  ]) {
    it(`rejects ${label}`, () => {
      assert.throws(() => check(constructors), {
        message: `inline: TreeNode(${signature}) delegates with ${arity} arguments, which no other constructor accepts`,
      });
    });
  }

  for (const [label, constructors] of [
    ["later target", `TreeNode(int x) : TreeNode() {} ${direct}`],
    [
      "multiple valid delegates",
      `${direct} TreeNode(int x) : TreeNode() {} TreeNode(int x, int y) : TreeNode(1) {}`,
    ],
    [
      "a chain through a unique defaulted intermediate target",
      "TreeNode() : TreeNode(1, nullptr) {} TreeNode(int x, TreeNode *l, TreeNode *r = nullptr) : TreeNode(1) {} TreeNode(int x) : val(x), left(nullptr), right(nullptr) {}",
    ],
    [
      "ambiguous target sets without selecting overloads",
      "TreeNode() : TreeNode(1) {} TreeNode(int x) : TreeNode() {} TreeNode(double x) : TreeNode(1) {}",
    ],
    [
      "template-id argument",
      "TreeNode() : TreeNode(std::pair<int, int>{}) {} TreeNode(std::pair<int, int> x) : val(0), left(nullptr), right(nullptr) {}",
    ],
    [
      "member initializer containing a constructor call",
      `${direct} TreeNode(int x) : val(x), left(new TreeNode()), right(nullptr) {}`,
    ],
    ...["1", "1, nullptr", "1, nullptr, nullptr"].map((args) => [
      `default arity ${args.split(",").length}`,
      `TreeNode() : TreeNode(${args}) {} ${defaults}`,
    ]),
    [
      "ungrouped default comparison",
      "TreeNode(int x, TreeNode *l, TreeNode *r) : TreeNode(1, nullptr) {} TreeNode(int x = 1 < 2, TreeNode *l = nullptr) : val(x), left(l), right(nullptr) {}",
    ],
  ]) {
    it(`accepts ${label}`, () =>
      assert.doesNotThrow(() => check(constructors)));
  }

  it("checks pointer initialization at delegation targets", () => {
    assert.throws(
      () =>
        check(
          "TreeNode(int x) : TreeNode() {} TreeNode() : val(0), left(nullptr) {}",
        ),
      { message: "inline: TreeNode() leaves TreeNode::right uninitialized" },
    );
  });
});

test("every C++ starter documents ListNode, TreeNode and Node as the harness declares them", async () => {
  await cppSyntax.prepareLanguage("cpp");
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

/// Whether cc builds and runs a program under AddressSanitizer. A write one
/// slot past a global array lands on whatever the linker put next to it, so
/// without the sanitizer such a write can go unseen on one platform and crash
/// on another; under it the write itself fails the run.
function sanitizes() {
  const dir = mkdtempSync(join(tmpdir(), "codetrial-asan-"));
  try {
    const source = join(dir, "probe.c");
    const binary = join(dir, "probe");
    writeFileSync(source, "int main(void) { return 0; }\n");
    execFileSync("cc", ["-fsanitize=address", source, "-o", binary], {
      stdio: "ignore",
    });
    execFileSync(binary, [], {
      stdio: "ignore",
      env: { ...process.env, ASAN_OPTIONS: "detect_leaks=0" },
    });
    return true;
  } catch {
    return false;
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
const noAsan =
  noCc || (sanitizes() ? false : "cc cannot build with AddressSanitizer");

describe("toolchain harnesses", { concurrency: 4 }, () => {
  for (const [id, solution] of [
    [
      "reverse-words-in-a-string",
      candidateSource("compiler-explorer/reverse-words-in-a-string.c"),
    ],
    ["valid-anagram", candidateSource("compiler-explorer/valid-anagram.c")],
    [
      "longest-common-prefix",
      candidateSource("compiler-explorer/longest-common-prefix.c"),
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
            candidateSource("compiler-explorer/two-sum.c"),
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
            candidateSource(
              "compiler-explorer/longest-substring-without-repeating-characters.c",
            ),
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

  // A newline or carriage return in a cell sat raw between quotes and ended the
  // character literal, so no compiled language built the harness. Surrounded
  // Regions hands its board back, so a solution that leaves it alone shows
  // every cell arrived as typed.
  const untouchedBoard = {
    c: "void backfillSealedPockets(char** board, int boardSize, int* boardColSize) {}",
    cpp: "class Solution {\npublic:\n    void backfillSealedPockets(vector<vector<char>>& board) {}\n};",
    java: "class Solution {\n    public void backfillSealedPockets(char[][] board) {}\n}",
  };
  const controlBoard = [
    ["X", "\n", "X"],
    ["\r", "\t", "\u0001"],
  ];
  for (const [language, skip] of [
    ["c", noCc],
    ["cpp", noGpp],
    ["java", noJava],
  ]) {
    it(
      `${language} harness hands control characters in a board over as typed`,
      { skip },
      async () => {
        const results = await runJudge(
          language,
          "surrounded-regions",
          untouchedBoard[language],
          [{ input: [controlBoard], expected: controlBoard }],
        );
        assert.deepEqual(
          results.map((result) => result.actual),
          [controlBoard],
        );
      },
    );
  }

  // The list<T> spellings reach C through the same pointer-and-returnSize
  // shape as T[], so a correct solution must print what it returned.
  for (const [id, solution] of [
    [
      "average-of-levels-in-binary-tree",
      candidateSource("compiler-explorer/average-of-levels-in-binary-tree.c"),
    ],
    ["spiral-matrix", candidateSource("compiler-explorer/spiral-matrix.c")],
    [
      "binary-tree-right-side-view",
      candidateSource("compiler-explorer/binary-tree-right-side-view.c"),
    ],
    [
      "substring-with-concatenation-of-all-words",
      candidateSource(
        "compiler-explorer/substring-with-concatenation-of-all-words.c",
      ),
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
            candidateSource("compiler-explorer/merge-k-sorted-lists.c"),
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
            candidateSource("compiler-explorer/construct-quad-tree.c"),
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
            candidateSource("compiler-explorer/min-stack.cpp"),
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
            candidateSource(
              "compiler-explorer/binary-search-tree-iterator.java",
            ),
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
            candidateSource("compiler-explorer/construct-quad-tree.cpp"),
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
            candidateSource("compiler-explorer/lru-cache.cpp"),
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
            candidateSource("compiler-explorer/fixed-capacity-ring-buffer.cpp"),
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
    ["rotate-list", candidateSource("compiler-explorer/rotate-list.java")],
    [
      "invert-binary-tree",
      candidateSource("compiler-explorer/invert-binary-tree.java"),
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
    c: candidateSource("compiler-explorer/merge-two-sorted-lists.cyclic.c"),
    cpp: candidateSource("compiler-explorer/merge-two-sorted-lists.cyclic.cpp"),
    java: candidateSource(
      "compiler-explorer/merge-two-sorted-lists.cyclic.java",
    ),
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
  // allows. `cases` replaces the judge's own when a test needs inputs of its
  // own.
  async function runJudge(
    language,
    id,
    candidate,
    cases = judges[id].cases,
    flags = [],
  ) {
    const dir = mkdtempSync(join(tmpdir(), `codetrial-${language}-${id}-`));
    try {
      const source = join(
        dir,
        language === "java" ? "Main.java" : `candidate.${language}`,
      );
      const binary = join(dir, "candidate");
      writeFileSync(
        source,
        generateHarness(language, { ...judges[id], cases }, candidate),
      );
      if (language === "java") await run("javac", [source]);
      else
        await run(language === "c" ? "cc" : "g++", [
          language === "c" ? "-std=c17" : "-std=c++20",
          ...flags,
          source,
          "-o",
          binary,
        ]);
      // The harness never frees what a case built, so the leak report
      // AddressSanitizer prints at exit would fail every sanitized run; other
      // runs keep the caller's environment.
      const options = {
        encoding: "utf8",
        timeout: 10000,
        maxBuffer: 1 << 24,
        env: flags.some((flag) => flag.startsWith("-fsanitize"))
          ? { ...process.env, ASAN_OPTIONS: "detect_leaks=0" }
          : process.env,
      };
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

  const addBits = {
    c: candidateSource("compiler-explorer/add-binary.c"),
    cpp: candidateSource("compiler-explorer/add-binary.cpp"),
    java: candidateSource("compiler-explorer/add-binary.java"),
  };
  for (const [language, skip] of flattenLanguages) {
    it(
      `${language} harness preserves exact add-binary sums beyond 64 bits`,
      { skip },
      async () => {
        const { cases } = judges["add-binary"];
        assert.ok(cases.some(({ expected }) => expected.length > 64));
        const results = await runJudge(
          language,
          "add-binary",
          addBits[language],
        );
        assert.equal(results.length, cases.length);
        cases.forEach(({ label, expected }, index) => {
          assert.equal(results[index].error, undefined, label);
          assert.equal(results[index].actual, expected, label);
        });
      },
    );
  }

  // The empty block list reaches a compiled harness as a matrix with no rows,
  // which the JavaScript model in the insert-interval test never builds or
  // prints. A correct solution in each language must return the new block
  // there, and every other case unchanged.
  const insertBlock = {
    c: candidateSource("compiler-explorer/insert-interval.c"),
    cpp: candidateSource("compiler-explorer/insert-interval.cpp"),
    java: candidateSource("compiler-explorer/insert-interval.java"),
  };
  for (const [language, skip] of flattenLanguages) {
    it(
      `${language} harness passes a correct insert-interval solution, no existing blocks included`,
      { skip },
      async () => {
        const { cases } = judges["insert-interval"];
        assert.ok(
          cases.some(({ label }) => label === "no existing blocks"),
          'the built-in cases must keep "no existing blocks"',
        );
        const results = await runJudge(
          language,
          "insert-interval",
          insertBlock[language],
        );
        assert.equal(results.length, cases.length);
        cases.forEach(({ label, expected }, index) => {
          assert.equal(results[index].error, undefined, label);
          assert.deepEqual(results[index].actual, expected, label);
        });
      },
    );
  }

  // A Morris-style flatten that leaves its last thread in place links the
  // chain back to the root. The serializer used to follow that link forever,
  // so the run timed out or was killed and no case reported anything. The
  // single-node and empty cases have no thread to leave, so they still pass.
  const cyclicFlatten = {
    c: candidateSource(
      "compiler-explorer/flatten-binary-tree-to-linked-list.cyclic.c",
    ),
    cpp: candidateSource(
      "compiler-explorer/flatten-binary-tree-to-linked-list.cyclic.cpp",
    ),
    java: candidateSource(
      "compiler-explorer/flatten-binary-tree-to-linked-list.cyclic.java",
    ),
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
    c: candidateSource(
      "compiler-explorer/copy-list-with-random-pointer.cyclic.c",
    ),
    cpp: candidateSource(
      "compiler-explorer/copy-list-with-random-pointer.cyclic.cpp",
    ),
    java: candidateSource(
      "compiler-explorer/copy-list-with-random-pointer.cyclic.java",
    ),
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
      candidate: candidateSource(
        "compiler-explorer/copy-list-with-random-pointer.shallow-loop.c",
      ),
    },
    {
      name: "a full copy whose last node links back into the input",
      message: deepCopy,
      candidate: candidateSource(
        "compiler-explorer/copy-list-with-random-pointer.links-into-input.c",
      ),
    },
    {
      name: "a random pointer into the original",
      message: strayRandom,
      candidate: candidateSource(
        "compiler-explorer/copy-list-with-random-pointer.random-into-input.c",
      ),
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

  // The C harness kept the input's nodes in 10000-entry arrays, written once
  // per node with no bound, and read a returned list only that far. A
  // 10001-node case wrote past the array before the candidate ran, and a
  // correct copy came back one node short. The registries are now sized from
  // the input, as C++ and Java size theirs, and a returned list is read no
  // further than one node past the input it copies, so that a loop or an
  // overlong answer cannot grow it without bound. The short copy fails
  // on any build. A shallow copy of the last node does not: the slot past the
  // array may still hold the node written there, so the overflow shows only
  // under AddressSanitizer, and that case runs under it.
  const longList = Array.from({ length: 10001 }, (_, i) => [
    i,
    i ? i - 1 : null,
  ]);
  const longListCases = [
    { label: "10001 nodes", input: [longList], expected: longList },
  ];
  for (const [name, candidate, check, flags] of [
    [
      "copies a 10001-node random list",
      candidateSource("compiler-explorer/copy-list-with-random-pointer.c"),
      (result) => assert.deepEqual(result.actual, longList),
    ],
    [
      "refuses the last node of a 10001-node random list as a shallow copy",
      candidateSource(
        "compiler-explorer/copy-list-with-random-pointer.last-node.c",
      ),
      (result) => assert.equal(result.error, deepCopy),
      ["-fsanitize=address"],
    ],
  ]) {
    it(`c harness ${name}`, { skip: flags ? noAsan : noCc }, async () => {
      const [result] = await runJudge(
        "c",
        "copy-list-with-random-pointer",
        candidate,
        longListCases,
        flags,
      );
      check(result);
    });
  }

  // With the 10000-node cut gone, a returned list is read no further than
  // one node past its input, since a correct copy has exactly as many nodes
  // as its input. Each node read is compared with every input node, so the
  // bound is what keeps a long answer to a long input from running until the
  // timeout. The judge's cases are too small for that cost to show, so the
  // bound is pinned by what it never reaches: n + 1 fresh nodes in front of
  // an n-node input fill the read exactly, so they are refused on length
  // before the read gets to the input, at any case size, while one more node
  // read would reach it and be refused as a deep copy. A copy with one extra
  // node is read to that one node and refused only because it outgrew the
  // input.
  for (const [name, candidate] of [
    [
      "a 200000-node random list",
      candidateSource(
        "compiler-explorer/copy-list-with-random-pointer.fresh-200000.c",
      ),
    ],
    [
      "n + 1 fresh nodes in front of its input",
      candidateSource(
        "compiler-explorer/copy-list-with-random-pointer.fresh-prefix.c",
      ),
    ],
    [
      "a copy with one node more than its input",
      candidateSource(
        "compiler-explorer/copy-list-with-random-pointer.one-node-more.c",
      ),
    ],
  ]) {
    it(`c harness refuses ${name}`, { skip: noCc }, async () => {
      const spec = judges["copy-list-with-random-pointer"];
      const results = await runJudge(
        "c",
        "copy-list-with-random-pointer",
        candidate,
      );
      spec.cases.forEach((testCase, index) => {
        assert.equal(
          results[index].error,
          "Returned list must not have more nodes than the input.",
          testCase.label,
        );
      });
    });
  }

  // The graph registry had the same 10000-entry bound. Returning the input's
  // last node must be refused as a shallow copy, which the harness can say
  // only if that node was recorded, and as above only the sanitizer is sure
  // to see the write that recorded it past the array.
  it(
    "c harness refuses the last node of a 10001-node graph as a shallow copy",
    { skip: noAsan },
    async () => {
      const ring = Array.from({ length: 10001 }, (_, i) => [
        ((i + 1) % 10001) + 1,
        ((i + 10000) % 10001) + 1,
      ]);
      const [result] = await runJudge(
        "c",
        "clone-graph",
        `struct Node* replicateTopology(struct Node* node) {
    return node ? node->neighbors[1] : NULL;
}`,
        [{ label: "10001 nodes", input: [ring], expected: ring }],
        // The generated main declares one stack array per adjacency row, and
        // instrumenting 10001 of them for use-after-scope takes over a
        // minute to build under gcc; the build is clean with the check on,
        // and the overflow this test guards is in a global or heap array.
        ["-fsanitize=address", "-fno-sanitize-address-use-after-scope"],
      );
      assert.equal(result.error, "Returned graph must be a deep copy.");
    },
  );

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

  // The C harness finds p and q by value with its own walk over the input
  // tree. Its queue held 4096 nodes on the stack, and a complete tree puts
  // every node in it, so a larger input wrote past the end before the
  // solution ran. The bank allows 100000 nodes here. Where cc has
  // AddressSanitizer the harness is built with it, since a queue that grows
  // one slot too late writes past its heap block without crashing a plain
  // build. The solution refuses a missing node, so a walk that stops short
  // fails too.
  for (const nodes of [5000, 100000]) {
    it(
      `c harness finds a node by value in a ${nodes}-node input tree`,
      { skip: noCc },
      async () => {
        const id = "lowest-common-ancestor-of-a-binary-tree";
        const leaf = nodes - 1;
        const parent = (leaf - 1) >> 1;
        const results = await runJudge(
          "c",
          id,
          candidateSource(
            "compiler-explorer/lowest-common-ancestor-of-a-binary-tree.c",
          ),
          [
            {
              input: [
                Array.from({ length: nodes }, (_, value) => value),
                leaf,
                parent,
              ],
            },
          ],
          noAsan ? [] : ["-fsanitize=address"],
        );
        // The answer is two nodes. Only three values are compared, since a
        // wrong answer can be the whole tree, and the assertion's diff of
        // 100000 values takes more memory than a runner has.
        assert.deepEqual(
          results.map((result) => result.actual?.slice(0, 3)),
          [[parent, leaf]],
        );
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
