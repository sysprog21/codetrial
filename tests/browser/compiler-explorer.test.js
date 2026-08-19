import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  compilerType,
  generateHarness,
  mapCompilerResponse,
  nativeLiteral,
  parseCompilerResults,
  stripAnsi,
} from "../../web/compiler-explorer.js";
import { judges } from "../../web/judges.js";

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
  assert.equal(compilerType("list<list<string>>", "cpp"), "vector<vector<string>>");
  assert.equal(compilerType("list<list<string>>", "java"), "List<List<String>>");

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
  assert.equal(nativeLiteral([["a"]], "character[][]", "java"), "new char[][]{new char[]{'a'}}");
  assert.equal(nativeLiteral([1, null, 2], "TreeNode", "java"), "treeNode(new Integer[]{1, null, 2})");

  for (const type of bankFunctionTypes()) {
    if (type === "void" || type === "Node") continue;
    const value = sampleValues[type];
    assert.notEqual(value, undefined, `missing sample for ${type}`);
    assert.doesNotThrow(() => nativeLiteral(value, type, "c"), `C literal ${type}`);
    assert.doesNotThrow(() => nativeLiteral(value, type, "cpp"), `C++ literal ${type}`);
    assert.doesNotThrow(() => nativeLiteral(value, type, "java"), `Java literal ${type}`);
  }
  assert.throws(() => nativeLiteral([[2], [1]], "Node", "cpp"), /argTypes preparation/);
});

test("harness generator builds one batched function harness", () => {
  const spec = judges["two-sum"];
  const c = generateHarness("c", spec, `int* twoSum(int* nums, int numsSize, int target, int* returnSize) {
    return NULL;
  }`);
  const cpp = generateHarness("cpp", spec, "class Solution { public: vector<int> twoSum(vector<int>& nums, int target) { return {0, 1}; } };");
  const java = generateHarness("java", spec, "class Solution { public int[] twoSum(int[] nums, int target) { return new int[]{0, 1}; } }");

  assert.match(c, /#include <stdbool\.h>/);
  assert.match(c, /int\* twoSum\(int\* nums, int numsSize, int target, int\* returnSize\)/);
  assert.match(c, /json_int_array\(stdout, actual, returnSize\)/);
  assert.match(c, /results/);
  assert.match(cpp, /class Solution/);
  assert.match(cpp, /#include <unordered_map>/);
  assert.match(cpp, /set<ListNode\*> seen/);
  assert.match(cpp, /solution\.twoSum\(nums, target\)/);
  assert.match(cpp, /chrono::steady_clock/);
  assert.match(cpp, /results/);
  assert.match(java, /class Main/);
  assert.match(java, /ListNode\(int val, ListNode next\)/);
  assert.match(java, /TreeNode\(int val, TreeNode left, TreeNode right\)/);
  assert.match(java, /Node\(int val, Node left, Node right, Node next\)/);
  assert.match(java, /solution\.twoSum\(nums, target\)/);
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
  assert.match(cpp, /setprecision\(17\)/, "6 significant digits mis-compares doubles");
  assert.match(cpp, /#include <iomanip>/);
  assert.match(java, /c < 0x20/, "Java must escape control characters");
  assert.match(java, /Double\.isFinite\(value\) \? String\.valueOf\(value\) : "null"/);
});

test("harness generator covers every function problem without network access", () => {
  for (const [id, spec] of Object.entries(judges)) {
    if (spec.kind !== "function") continue;
    assert.doesNotThrow(() => generateHarness("c", spec, "int placeholder(void) { return 0; }"), `C harness for ${id}`);
    assert.doesNotThrow(() => generateHarness("cpp", spec, "class Solution {};"), `C++ harness for ${id}`);
    assert.doesNotThrow(() => generateHarness("java", spec, "class Solution {}"), `Java harness for ${id}`);
  }
});

test("harness generator covers every class problem without network access", () => {
  for (const [id, spec] of Object.entries(judges)) {
    if (spec.kind !== "class") continue;
    assert.doesNotThrow(() => generateHarness("cpp", spec, `class ${spec.className} {};`), `C++ class harness for ${id}`);
    assert.doesNotThrow(() => generateHarness("java", spec, `class ${spec.className} {}`), `Java class harness for ${id}`);
    assert.throws(() => generateHarness("c", spec, ""), /Only function judges are supported for C/);
  }
});

test("C Two Sum harness can produce passing Compiler Explorer JSON", (t) => {
  try {
    execFileSync("cc", ["--version"], { stdio: "ignore" });
  } catch {
    t.skip("cc is not available");
    return;
  }
  const dir = mkdtempSync(join(tmpdir(), "codetrial-c-"));
  try {
    const source = join(dir, "twosum.c");
    const binary = join(dir, "twosum");
    writeFileSync(source, generateHarness("c", judges["two-sum"], `int* twoSum(int* nums, int numsSize, int target, int* returnSize) {
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
}`));
    execFileSync("cc", ["-std=c17", source, "-o", binary]);
    assert.deepEqual(parseCompilerResults(execFileSync(binary, { encoding: "utf8" })).results.map((result) => result.actual), [
      [0, 1],
      [1, 2],
      [0, 1],
      [0, 2],
    ]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("C Merge k Sorted Lists harness can produce passing Compiler Explorer JSON", (t) => {
  try {
    execFileSync("cc", ["--version"], { stdio: "ignore" });
  } catch {
    t.skip("cc is not available");
  }
  const dir = mkdtempSync(join(tmpdir(), "codetrial-c-"));
  try {
    const source = join(dir, "mergek.c");
    const binary = join(dir, "mergek");
    writeFileSync(source, generateHarness("c", judges["merge-k-sorted-lists"], `struct ListNode* mergeKLists(struct ListNode** lists, int listsSize) {
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
}`));
    execFileSync("cc", ["-std=c17", source, "-o", binary]);
    assert.deepEqual(parseCompilerResults(execFileSync(binary, { encoding: "utf8" })).results.map((result) => result.actual), judges["merge-k-sorted-lists"].cases.map((testCase) => testCase.expected));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("C Construct Quad Tree harness can produce passing Compiler Explorer JSON", (t) => {
  try {
    execFileSync("cc", ["--version"], { stdio: "ignore" });
  } catch {
    t.skip("cc is not available");
    return;
  }
  const dir = mkdtempSync(join(tmpdir(), "codetrial-c-quad-"));
  try {
    const source = join(dir, "quad.c");
    const binary = join(dir, "quad");
    writeFileSync(source, generateHarness("c", judges["construct-quad-tree"], `static bool same(int** grid, int row, int col, int size) {
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

struct Node* construct(int** grid, int gridSize, int* gridColSize) {
    (void)gridColSize;
    return build(grid, 0, 0, gridSize);
}`));
    execFileSync("cc", ["-std=c17", source, "-o", binary]);
    assert.deepEqual(parseCompilerResults(execFileSync(binary, { encoding: "utf8" })).results.map((result) => result.actual), judges["construct-quad-tree"].cases.map((testCase) => testCase.expected));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("C++ MinStack class harness can produce passing Compiler Explorer JSON", (t) => {
  try {
    execFileSync("g++", ["--version"], { stdio: "ignore" });
  } catch {
    t.skip("g++ is not available");
    return;
  }
  const dir = mkdtempSync(join(tmpdir(), "codetrial-cpp-class-"));
  try {
    const source = join(dir, "minstack.cpp");
    const binary = join(dir, "minstack");
    writeFileSync(source, generateHarness("cpp", judges["min-stack"], `class MinStack {
    vector<int> values;
    vector<int> minimums;
public:
    MinStack() {}
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
};`));
    execFileSync("g++", ["-std=c++20", source, "-o", binary]);
    assert.deepEqual(parseCompilerResults(execFileSync(binary, { encoding: "utf8" })).results.map((result) => result.actual), judges["min-stack"].cases.map((testCase) => testCase.expected));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("Java BSTIterator class harness can produce passing Compiler Explorer JSON", (t) => {
  try {
    // The generated harness uses `instanceof String s` pattern matching, so a
    // present-but-older javac fails to compile it and reports a syntax error,
    // which reads as a harness bug rather than a toolchain gap.
    const version = execFileSync("javac", ["--version"], { encoding: "utf8" });
    const major = Number.parseInt(version.replace(/^javac\s+/, ""), 10);
    if (!Number.isFinite(major) || major < 16) {
      t.skip(`javac ${major} is older than 16, which the harness needs`);
      return;
    }
    execFileSync("java", ["--version"], { stdio: "ignore" });
  } catch {
    t.skip("javac/java are not available");
    return;
  }
  const dir = mkdtempSync(join(tmpdir(), "codetrial-java-class-"));
  try {
    const source = join(dir, "Main.java");
    writeFileSync(source, generateHarness("java", judges["binary-search-tree-iterator"], `class BSTIterator {
    private final ArrayDeque<TreeNode> stack = new ArrayDeque<>();
    public BSTIterator(TreeNode root) { pushLeft(root); }
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
}`));
    execFileSync("javac", [source]);
    assert.deepEqual(parseCompilerResults(execFileSync("java", ["-cp", dir, "Main"], { encoding: "utf8" })).results.map((result) => result.actual), judges["binary-search-tree-iterator"].cases.map((testCase) => testCase.expected));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("C++ Construct Quad Tree harness can produce passing Compiler Explorer JSON", (t) => {
  try {
    execFileSync("g++", ["--version"], { stdio: "ignore" });
  } catch {
    t.skip("g++ is not available");
    return;
  }
  const dir = mkdtempSync(join(tmpdir(), "codetrial-cpp-quad-"));
  try {
    const source = join(dir, "quad.cpp");
    const binary = join(dir, "quad");
    writeFileSync(source, generateHarness("cpp", judges["construct-quad-tree"], `class Solution {
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
    Node* construct(vector<vector<int>>& grid) {
        return build(grid, 0, 0, grid.size());
    }
};`));
    execFileSync("g++", ["-std=c++20", source, "-o", binary]);
    assert.deepEqual(parseCompilerResults(execFileSync(binary, { encoding: "utf8" })).results.map((result) => result.actual), judges["construct-quad-tree"].cases.map((testCase) => testCase.expected));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("C++ LRUCache class harness supports standard list-based solutions", (t) => {
  try {
    execFileSync("g++", ["--version"], { stdio: "ignore" });
  } catch {
    t.skip("g++ is not available");
    return;
  }
  const dir = mkdtempSync(join(tmpdir(), "codetrial-cpp-lru-"));
  try {
    const source = join(dir, "lru.cpp");
    const binary = join(dir, "lru");
    writeFileSync(source, generateHarness("cpp", judges["lru-cache"], `class LRUCache {
    int capacity;
    list<pair<int, int>> order;
    unordered_map<int, list<pair<int, int>>::iterator> byKey;
public:
    LRUCache(int capacity) : capacity(capacity) {}
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
};`));
    execFileSync("g++", ["-std=c++20", source, "-o", binary]);
    assert.deepEqual(parseCompilerResults(execFileSync(binary, { encoding: "utf8" })).results.map((result) => result.actual), judges["lru-cache"].cases.map((testCase) => testCase.expected));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("harness generator handles output parameter and output prefix specs", () => {
  const merge = generateHarness("cpp", judges["merge-sorted-array"], "class Solution { public: void merge(vector<int>& nums1, int m, vector<int>& nums2, int n) {} };");
  const remove = generateHarness("java", judges["remove-element"], "class Solution { public int removeElement(int[] nums, int val) { return 0; } }");

  assert.match(merge, /solution\.merge\(nums1, m, nums2, n\)/);
  assert.match(merge, /auto actual = nums1/);
  assert.match(remove, /outputSize < 0 \|\| outputSize > nums\.length/);
  assert.match(remove, /int outputSize = solution\.removeElement\(nums, val\)/);
  assert.match(remove, /Arrays\.copyOf\(nums, outputSize\)/);
});

test("class harness generator handles constructor, mutator, query, and tree constructor calls", () => {
  const minStack = generateHarness("cpp", judges["min-stack"], "class MinStack { public: MinStack() {} void push(int) {} void pop() {} int top() { return 0; } int getMin() { return 0; } };");
  const iterator = generateHarness("java", judges["binary-search-tree-iterator"], "class BSTIterator { BSTIterator(TreeNode root) {} int next() { return 0; } boolean hasNext() { return false; } }");

  assert.match(minStack, /MinStack instance\{\}/);
  assert.match(minStack, /instance\.push\(-2\)/);
  assert.match(minStack, /instance\.pop\(\);\n    actual\.push_back\("null"\)/);
  assert.match(minStack, /actual\.push_back\(toJson\(instance\.getMin\(\)\)\)/);
  assert.match(iterator, /new BSTIterator\(treeNode\(new Integer\[\]\{7, 3, 15, null, null, 9, 20\}\)\)/);
  assert.match(iterator, /actual\.add\(jsonAny\(instance\.hasNext\(\)\)\)/);
});

test("harness generator prepares adapter arguments and mutable char matrices", () => {
  const cList = generateHarness("c", judges["merge-two-sorted-lists"], "struct ListNode* mergeTwoLists(struct ListNode* list1, struct ListNode* list2) { return list1; }");
  const cTree = generateHarness("c", judges["lowest-common-ancestor-of-a-binary-tree"], "struct TreeNode* lowestCommonAncestor(struct TreeNode* root, struct TreeNode* p, struct TreeNode* q) { return p; }");
  const cSurrounded = generateHarness("c", judges["surrounded-regions"], "void solve(char** board, int boardSize, int* boardColSize) {}");
  const cycle = generateHarness("cpp", judges["linked-list-cycle"], "class Solution { public: bool hasCycle(ListNode* head) { return head != nullptr; } };");
  const lca = generateHarness("java", judges["lowest-common-ancestor-of-a-binary-tree"], "class Solution { public TreeNode lowestCommonAncestor(TreeNode root, TreeNode p, TreeNode q) { return p; } }");
  const surrounded = generateHarness("java", judges["surrounded-regions"], "class Solution { public void solve(char[][] board) {} }");
  const randomList = generateHarness("cpp", judges["copy-list-with-random-pointer"], "class Solution { public: Node* copyRandomList(Node* head) { return head; } };");
  const cRandomList = generateHarness("c", judges["copy-list-with-random-pointer"], "struct Node* copyRandomList(struct Node* head) { return head; }");
  const graph = generateHarness("java", judges["clone-graph"], "class Solution { public Node cloneGraph(Node node) { return node; } }");
  const cGraph = generateHarness("c", judges["clone-graph"], "struct Node* cloneGraph(struct Node* s) { return s; }");
  const nextTree = generateHarness("cpp", judges["populating-next-right-pointers-in-each-node-ii"], "class Solution { public: Node* connect(Node* root) { return root; } };");
  const quadTree = generateHarness("cpp", judges["construct-quad-tree"], "class Solution { public: Node* construct(vector<vector<int>>& grid) { return new Node(grid[0][0], true); } };");
  const cQuadTree = generateHarness("c", judges["construct-quad-tree"], "struct Node* construct(int** grid, int gridSize, int* gridColSize) { return NULL; }");
  const javaQuadTree = generateHarness("java", judges["construct-quad-tree"], "class Solution { public Node construct(int[][] grid) { return new Node(grid[0][0], true); } }");

  assert.match(cList, /mergeTwoLists\(list1, list2\)/);
  assert.doesNotMatch(cList, /mergeTwoLists\(list1, list1Size/);
  assert.match(cTree, /lowestCommonAncestor\(root, p, q\)/);
  assert.match(cTree, /findTreeNode\(root, 5\)/);
  assert.match(cSurrounded, /solve\(board, boardSize, boardColSize\)/);
  assert.match(cSurrounded, /json_char_matrix\(stdout, board, boardSize, boardColSize\)/);
  assert.match(cycle, /ListNode\* head = listNode\(\{3, 2, 0, -4\}, 1\)/);
  assert.match(cycle, /solution\.hasCycle\(head\)/);
  assert.doesNotMatch(cycle, /solution\.hasCycle\(head, pos\)/);
  assert.match(lca, /TreeNode p = findTreeNode\(root, 5\)/);
  assert.match(lca, /solution\.lowestCommonAncestor\(root, p, q\)/);
  assert.match(surrounded, /char\[\]\[\] board = new char\[\]\[\]\{new char\[\]\{'X', 'X', 'X', 'X'\}/);
  assert.match(surrounded, /jsonAny\(actual\)/);
  assert.match(randomList, /\{13, 0\}, \{11, 4\}, \{10, 2\}, \{1, 0\}/);
  assert.match(randomList, /\{3, nullopt\}/);
  assert.match(randomList, /Returned list must be a deep copy/);
  assert.match(randomList, /Returned random pointer must stay inside the copied list/);
  assert.match(randomList, /set<Node\*> seen/);
  assert.match(cRandomList, /Returned list must be a deep copy/);
  assert.match(cRandomList, /Returned random pointer must stay inside the copied list/);
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
  assert.deepEqual(parseCompilerResults([{ text: "noise " }, { text: "{\"results\":[{\"actual\":[0,1],\"timeMs\":1.2}]}" }]), {
    results: [{ actual: [0, 1], timeMs: 1.2 }],
  });
  assert.deepEqual(parseCompilerResults("debug {\"foo\":1}\n{\"results\":[]}"), {
    results: [],
  });
  assert.deepEqual(parseCompilerResults("{bad"), {
    setupError: "The run produced no JSON results.",
  });
  assert.deepEqual(mapCompilerResponse({ timedOut: true }), {
    setupError: "Compiler Explorer timed out before the run completed.",
  });
  assert.deepEqual(mapCompilerResponse({ didExecute: false, buildResult: { stderr: [{ text: "\u001b[31mbuild failed\u001b[0m" }] } }), {
    setupError: "build failed",
  });
  assert.deepEqual(mapCompilerResponse({ stdout: "not json" }), {
    setupError: "The run produced no JSON results.",
  });
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
