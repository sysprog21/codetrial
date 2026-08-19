// The three ways candidate code gets executed, kept out of the page controller
// because none of them touch the DOM: a Worker for JavaScript, a Worker holding
// Pyodide for Python, and Compiler Explorer over the network for C, C++ and Java.
// Each returns the same summary shape, and each is interruptible: candidate code
// that never returns costs one test run, not the interview.

import { loadJudge } from "./problem-data.js";
import { generateHarness, mapCompilerResponse } from "./compiler-explorer.js";
import { checkAnswer, renderValue } from "./lib.js";

const pendingTestLanguages = new Set();
// C, C++ and Java runs leave the machine, which the editor toolbar says out loud.
// Setting this to an empty string turns those runs off instead of pointing them
// somewhere else; the editor still works, only the runner is withdrawn.
//
// /runtime-config.js always sets this, and the server builds its CSP from the
// same value, so the literal below is only reached by tests that import this
// module directly.
const compilerExplorerBaseUrl = globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL ?? "https://godbolt.org";
if (!compilerExplorerBaseUrl) {
  pendingTestLanguages.add("c").add("cpp").add("java");
}
/// How long candidate code may run locally, in either Worker. This bounds
/// execution, so it is the one number an infinite loop is measured against.
const testTimeoutMs = 5000;
/// A remote compile is not a local execution and needs its own, much larger
/// number: Compiler Explorer takes about six seconds to compile a harness it has
/// not seen (a few hundred milliseconds once it caches it), so sharing the 5s
/// execution budget failed the first compiled run of every interview and told
/// the candidate to try again later.
const compilerExplorerTimeoutMs = 20_000;
/// Starting Pyodide is not a test run, so it gets its own, much longer budget.
/// It exists only so a stalled or truncated download eventually gives up.
const pythonBootTimeoutMs = 60_000;
const compilerExplorer = {
  c: { compiler: "cclang1910", userArguments: "-O2 -std=c17" },
  cpp: { compiler: "g162", userArguments: "-O2 -std=c++20" },
  java: { compiler: "java2501", userArguments: "" },
};
// Served from this origin, not a CDN. All five files are pinned by SHA-256 in
// web/vendor/pyodide/SHA256SUMS and fetched by scripts/fetch-vendor.sh, which
// refuses a hash it does not recognise. Pinning the loader in the browser and
// letting it pull the wasm from a CDN unchecked, which is what this used to do,
// verified the one file that does not execute candidate code.
//
// Absolute, not "/vendor/pyodide/". `loadPyodide` runs inside a Worker built
// from a blob, whose base URL is the blob: URL rather than the page, so a
// root-relative path there has no origin to resolve against. Resolved lazily
// because the browser tests import this module under node, where there is no
// `location`.
const pyodideBaseUrl = () => new URL("/vendor/pyodide/", globalThis.location?.href ?? "http://localhost/").href;
let pythonWorkerPromise = null;

export async function runBrowserTests(problemId, code, language, onStatus = null) {
  const empty = { problemId, language, passed: 0, total: 0, cases: [], at: Date.now() };
  // A judge that cannot be fetched is not a problem without tests. Reporting
  // both the same way told a candidate on a flaky connection that their problem
  // had no test cases, which is the one reading that makes them stop trying.
  let spec;
  try {
    spec = await loadJudge(problemId);
  } catch {
    return { ...empty, setupError: "The test cases could not be loaded. Check your connection and run again." };
  }
  if (!spec) return { ...empty, setupError: "No test cases are defined for this problem." };
  const base = { ...empty, total: spec.cases.length };
  if (pendingTestLanguages.has(language)) {
    return { ...base, setupError: `${languageLabel(language)} tests are not wired up yet. Keep using the editor; Jim can still review this code.` };
  }
  const reportStatus = (status) => onStatus?.(status);
  try {
    const raw = language === "python"
      ? await runPython(code, spec, reportStatus)
      : compilerExplorer[language]
        ? await runCompilerExplorer(language, code, spec, reportStatus)
        : (reportStatus("running"), await runWorker(code, spec));
    if (raw.setupError || !raw.results) return { ...base, setupError: raw.setupError || "The run produced no results." };
    const cases = spec.cases.map((testCase, index) => {
      const result = raw.results[index];
      if (!result || result.error) {
        return { label: testCase.label, pass: false, got: "-", expected: renderValue(testCase.expected), error: result?.error || "No result produced.", timeMs: Math.round(result?.timeMs || 0) };
      }
      const pass = checkAnswer(spec, testCase, result.actual);
      return { label: testCase.label, pass, got: renderValue(result.actual), expected: renderValue(testCase.expected), timeMs: Math.round(result.timeMs) };
    });
    return { ...base, cases, passed: cases.filter((item) => item.pass).length };
  } catch (error) {
    return { ...base, setupError: String(error.message || error).slice(0, 400) };
  }
}

function languageLabel(language) {
  return { c: "C", cpp: "C++", java: "Java" }[language] || language;
}

async function runCompilerExplorer(language, code, spec, reportStatus = null) {
  if (spec.kind !== "function" && !(spec.kind === "class" && language !== "c")) {
    return { setupError: `${languageLabel(language)} class-style tests are not wired up yet. Keep using the editor; Jim can still review this code.` };
  }
  reportStatus?.("compiling");
  const config = compilerExplorer[language];
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), compilerExplorerTimeoutMs);
  try {
    const response = await fetch(`${compilerExplorerBaseUrl}/api/compiler/${config.compiler}/compile`, {
      method: "POST",
      headers: {
        "Accept": "application/json",
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        source: generateHarness(language, spec, code),
        options: {
          userArguments: config.userArguments,
          compilerOptions: { executorRequest: true },
          filters: { execute: true },
          executeParameters: { args: [], stdin: "" },
        },
      }),
      signal: controller.signal,
    });
    if (!response.ok) {
      return { setupError: `Compiler Explorer returned HTTP ${response.status}. Try again later.` };
    }
    return mapCompilerResponse(await response.json());
  } catch (error) {
    if (error?.name === "AbortError") {
      return { setupError: `Compiler Explorer did not respond within ${compilerExplorerTimeoutMs / 1000} seconds. Try again later.` };
    }
    return { setupError: `Compiler Explorer run failed: ${String(error?.message || error).slice(0, 300)}` };
  } finally {
    clearTimeout(timer);
  }
}

// The JavaScript runner's worker, as source rather than a file: it becomes a
// blob URL below, which `script-src 'self' blob:` allows and which keeps the
// runner from being fetchable on its own. Hoisted to module scope because it
// interpolates nothing and never has; leaving it inside runWorker made a
// 23-line function read as a 278-line one.
const JS_RUNNER_SOURCE = `
    self.onmessage = (event) => {
      const { code, spec } = event.data;
      const sanitize = (value) => {
        const normalized = value === undefined ? null : value;
        try { return JSON.parse(JSON.stringify(normalized)); } catch { return String(normalized); }
      };
      function ListNode(val, next) {
        this.val = val === undefined ? 0 : val;
        this.next = next === undefined ? null : next;
      }
      self.ListNode = ListNode;
      function TreeNode(val, left, right) {
        this.val = val === undefined ? 0 : val;
        this.left = left === undefined ? null : left;
        this.right = right === undefined ? null : right;
      }
      self.TreeNode = TreeNode;
      function RandomNode(val, next, random) {
        this.val = val === undefined ? 0 : val;
        this.next = next === undefined ? null : next;
        this.random = random === undefined ? null : random;
      }
      function NextNode(val, left, right, next) {
        this.val = val === undefined ? 0 : val;
        this.left = left === undefined ? null : left;
        this.right = right === undefined ? null : right;
        this.next = next === undefined ? null : next;
      }
      function GraphNode(val, neighbors) {
        this.val = val === undefined ? 0 : val;
        this.neighbors = neighbors === undefined ? [] : neighbors;
      }
      function QuadNode(val, isLeaf, topLeft, topRight, bottomLeft, bottomRight) {
        this.val = val === undefined ? false : val;
        this.isLeaf = isLeaf === undefined ? false : isLeaf;
        this.topLeft = topLeft === undefined ? null : topLeft;
        this.topRight = topRight === undefined ? null : topRight;
        this.bottomLeft = bottomLeft === undefined ? null : bottomLeft;
        this.bottomRight = bottomRight === undefined ? null : bottomRight;
      }
      const usesNextTree = spec.argTypes?.includes("nextTree") || spec.outputType === "nextTree";
      const usesGraph = spec.argTypes?.includes("graphNode") || spec.outputType === "graphNode";
      const usesQuadTree = spec.outputType === "quadTree";
      self._Node = usesQuadTree ? QuadNode : (usesGraph ? GraphNode : (usesNextTree ? NextNode : RandomNode));
      self.Node = self._Node;
      let originalRandomNodes = new Set();
      let originalGraphNodes = new Set();
      const listFromArray = (values, pos = -1) => {
        let head = null, tail = null, cycle = null;
        values.forEach((value, index) => {
          const node = new ListNode(value);
          if (!head) head = node;
          else tail.next = node;
          tail = node;
          if (index === pos) cycle = node;
        });
        if (tail && cycle) tail.next = cycle;
        return head;
      };
      const randomListFromArray = (items) => {
        const nodes = items.map(([value]) => new RandomNode(value));
        originalRandomNodes = new Set(nodes);
        nodes.forEach((node, index) => {
          node.next = nodes[index + 1] || null;
          const random = items[index][1];
          node.random = random === null ? null : nodes[random];
        });
        return nodes[0] || null;
      };
      const graphFromAdjacency = (adjacency) => {
        if (!adjacency.length) {
          originalGraphNodes = new Set();
          return null;
        }
        const nodes = adjacency.map((_, index) => new GraphNode(index + 1));
        originalGraphNodes = new Set(nodes);
        adjacency.forEach((neighbors, index) => {
          nodes[index].neighbors = neighbors.map((value) => nodes[value - 1]);
        });
        return nodes[0];
      };
      const listToArray = (node) => {
        const values = [], seen = new Set();
        while (node && !seen.has(node)) {
          seen.add(node);
          values.push(node.val);
          node = node.next;
        }
        return values;
      };
      // Level-order build, parameterised by node type: the plain tree and the
      // next-pointer tree differ only in what they allocate.
      const treeBuilder = (Node) => (values) => {
        if (!values.length || values[0] === null) return null;
        const root = new Node(values[0]);
        const queue = [root];
        let index = 1;
        for (const node of queue) {
          if (index < values.length && values[index] !== null) {
            node.left = new Node(values[index]);
            queue.push(node.left);
          }
          index++;
          if (index < values.length && values[index] !== null) {
            node.right = new Node(values[index]);
            queue.push(node.right);
          }
          index++;
        }
        return root;
      };
      const treeFromArray = treeBuilder(TreeNode);
      const nextTreeFromArray = treeBuilder(NextNode);
      const treeToArray = (root) => {
        const values = [], queue = [root];
        while (queue.length) {
          const node = queue.shift();
          if (!node) {
            values.push(null);
            continue;
          }
          values.push(node.val);
          queue.push(node.left || null, node.right || null);
        }
        while (values.at(-1) === null) values.pop();
        return values;
      };
      const nextTreeToLevels = (root) => {
        const levels = [], queue = root ? [root] : [];
        while (queue.length) {
          const levelNodes = queue.splice(0);
          levels.push(levelNodes.map((node) => node.val));
          for (let i = 0; i < levelNodes.length; i++) {
            if (levelNodes[i].next !== (levelNodes[i + 1] || null)) throw new TypeError("next pointers do not match the tree level order.");
            if (levelNodes[i].left) queue.push(levelNodes[i].left);
            if (levelNodes[i].right) queue.push(levelNodes[i].right);
          }
        }
        return levels;
      };
      const quadTreeToPreorder = (root) => {
        const rows = [];
        const visit = (node) => {
          if (!node) return;
          const isLeaf = Boolean(node.isLeaf);
          rows.push([isLeaf ? 1 : 0, isLeaf && node.val ? 1 : (isLeaf ? 0 : 1)]);
          if (!isLeaf) {
            visit(node.topLeft);
            visit(node.topRight);
            visit(node.bottomLeft);
            visit(node.bottomRight);
          }
        };
        visit(root);
        return rows;
      };
      const randomListToArray = (node) => {
        const nodes = [], seen = new Set();
        while (node && !seen.has(node)) {
          if (originalRandomNodes.has(node)) throw new TypeError("Returned list must be a deep copy.");
          seen.add(node);
          nodes.push(node);
          node = node.next;
        }
        return nodes.map((item) => [item.val, item.random === null ? null : nodes.indexOf(item.random)]);
      };
      const graphToAdjacency = (node) => {
        if (!node) return [];
        const byValue = new Map(), queue = [node];
        for (const current of queue) {
          if (!current) continue;
          if (originalGraphNodes.has(current)) throw new TypeError("Returned graph must be a deep copy.");
          if (byValue.has(current.val)) continue;
          byValue.set(current.val, current);
          for (const neighbor of current.neighbors || []) queue.push(neighbor);
        }
        return Array.from(byValue.keys()).sort((a, b) => a - b).map((value) =>
          (byValue.get(value).neighbors || []).map((neighbor) => neighbor.val).sort((a, b) => a - b)
        );
      };
      const prepareArgs = (testCase, argTypes = spec.argTypes) => {
        const raw = JSON.parse(JSON.stringify(testCase.input));
        const converted = [];
        const findTreeNode = (root, value) => {
          const queue = root ? [root] : [];
          for (const node of queue) {
            if (node.val === value) return node;
            if (node.left) queue.push(node.left);
            if (node.right) queue.push(node.right);
          }
          throw new TypeError("Could not find tree node value " + value + ".");
        };
        raw.forEach((value, index) => {
          const type = argTypes?.[index];
          if (type === "cyclePos") return;
          if (type === "linkedList") converted.push(listFromArray(value, raw[spec.cyclePosParam] ?? -1));
          else if (type === "linkedListArray") converted.push(value.map((items) => listFromArray(items)));
          else if (type === "randomList") converted.push(randomListFromArray(value));
          else if (type === "graphNode") converted.push(graphFromAdjacency(value));
          else if (type === "binaryTree") converted.push(treeFromArray(value));
          else if (type === "nextTree") converted.push(nextTreeFromArray(value));
          else if (type === "treeNodeValue") converted.push(findTreeNode(converted[spec.nodeRefRootParam ?? 0], value));
          else converted.push(value);
        });
        return converted;
      };
      const name = spec.kind === "class" ? spec.className : spec.entry;
      let entry;
      try {
        entry = (0, eval)(code + "\\n;(typeof " + name + " !== 'undefined' ? " + name + " : undefined);");
      } catch (error) {
        self.postMessage({ setupError: String((error && error.message) || error).slice(0, 400) });
        return;
      }
      if (typeof entry !== "function") {
        self.postMessage({ setupError: "Could not find " + name + " in your code - keep the starter signature." });
        return;
      }
      const results = [];
      for (const testCase of spec.cases) {
        const started = performance.now();
        try {
          let actual;
          if (spec.kind === "class") {
            const [methods, argsList] = testCase.input;
            const instance = new entry(...prepareArgs({ input: argsList[0] }, spec.constructorArgTypes));
            actual = [null];
            for (let i = 1; i < methods.length; i++) actual.push(instance[methods[i]](...argsList[i]) ?? null);
          } else {
            const args = prepareArgs(testCase);
            const returned = entry(...args);
            if (Number.isInteger(spec.outputPrefixParam)) {
              if (!Number.isInteger(returned) || returned < 0 || returned > args[spec.outputPrefixParam].length) {
                throw new TypeError("Return value must be an integer length for the kept prefix.");
              }
              actual = args[spec.outputPrefixParam].slice(0, returned);
            } else {
              actual = Number.isInteger(spec.outputParam) ? args[spec.outputParam] : returned;
              if (spec.outputType === "linkedList") actual = listToArray(actual);
              if (spec.outputType === "randomList") actual = randomListToArray(actual);
              if (spec.outputType === "graphNode") actual = graphToAdjacency(actual);
              if (spec.outputType === "binaryTree") actual = treeToArray(actual);
              if (spec.outputType === "nextTree") actual = nextTreeToLevels(actual);
              if (spec.outputType === "quadTree") actual = quadTreeToPreorder(actual);
            }
          }
          results.push({ actual: sanitize(actual), timeMs: performance.now() - started });
        } catch (error) {
          results.push({ error: String((error && error.message) || error).slice(0, 400), timeMs: performance.now() - started });
        }
      }
      self.postMessage({ results });
    };
  `;

function runWorker(code, spec) {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(new Blob([JS_RUNNER_SOURCE], { type: "application/javascript" }));
    const worker = new Worker(url);
    const timer = setTimeout(() => {
      worker.terminate();
      URL.revokeObjectURL(url);
      reject(new Error(`Execution timed out after ${testTimeoutMs / 1000}s - check for an infinite loop.`));
    }, testTimeoutMs);
    worker.onmessage = (event) => {
      clearTimeout(timer);
      worker.terminate();
      URL.revokeObjectURL(url);
      resolve(event.data);
    };
    worker.onerror = (event) => {
      clearTimeout(timer);
      worker.terminate();
      URL.revokeObjectURL(url);
      reject(new Error(event.message || "Worker crashed while running your code."));
    };
    worker.postMessage({ code, spec });
  });
}

const pythonDriver = `
import json, time
from typing import List, Optional

class ListNode:
    def __init__(self, val=0, next=None):
        self.val = val
        self.next = next

class TreeNode:
    def __init__(self, val=0, left=None, right=None):
        self.val = val
        self.left = left
        self.right = right

class Node:
    def __init__(self, x: int, next=None, random=None):
        self.val = int(x)
        self.next = next
        self.random = random

class NextNode:
    def __init__(self, val=0, left=None, right=None, next=None):
        self.val = val
        self.left = left
        self.right = right
        self.next = next

class GraphNode:
    def __init__(self, val=0, neighbors=None):
        self.val = val
        self.neighbors = [] if neighbors is None else neighbors

class QuadNode:
    def __init__(self, val=False, isLeaf=False, topLeft=None, topRight=None, bottomLeft=None, bottomRight=None):
        self.val = val
        self.isLeaf = isLeaf
        self.topLeft = topLeft
        self.topRight = topRight
        self.bottomLeft = bottomLeft
        self.bottomRight = bottomRight

def _list_from_array(values, pos=-1):
    head = tail = cycle = None
    for index, value in enumerate(values):
        node = ListNode(value)
        if head is None:
            head = node
        else:
            tail.next = node
        tail = node
        if index == pos:
            cycle = node
    if tail is not None and cycle is not None:
        tail.next = cycle
    return head

_original_random_nodes = set()

def _random_list_from_array(items):
    global _original_random_nodes
    nodes = [Node(value) for value, _ in items]
    _original_random_nodes = {id(node) for node in nodes}
    for index, node in enumerate(nodes):
        node.next = nodes[index + 1] if index + 1 < len(nodes) else None
        random = items[index][1]
        node.random = None if random is None else nodes[random]
    return nodes[0] if nodes else None

_original_graph_nodes = set()

def _graph_from_adjacency(adjacency):
    global _original_graph_nodes
    if not adjacency:
        _original_graph_nodes = set()
        return None
    nodes = [GraphNode(index + 1) for index, _ in enumerate(adjacency)]
    _original_graph_nodes = {id(node) for node in nodes}
    for index, neighbors in enumerate(adjacency):
        nodes[index].neighbors = [nodes[value - 1] for value in neighbors]
    return nodes[0]

def _list_to_array(node):
    values = []
    seen = set()
    while node is not None and id(node) not in seen:
        seen.add(id(node))
        values.append(node.val)
        node = node.next
    return values

# Level-order build, parameterised by node type: the plain tree and the
# next-pointer tree differ only in what they allocate.
def _tree_from_array(values, node_type=TreeNode):
    if not values or values[0] is None:
        return None
    root = node_type(values[0])
    queue = [root]
    index = 1
    for node in queue:
        if index < len(values) and values[index] is not None:
            node.left = node_type(values[index])
            queue.append(node.left)
        index += 1
        if index < len(values) and values[index] is not None:
            node.right = node_type(values[index])
            queue.append(node.right)
        index += 1
    return root

def _tree_to_array(root):
    values = []
    queue = [root]
    for node in queue:
        if node is None:
            values.append(None)
            continue
        values.append(node.val)
        queue.extend([node.left, node.right])
    while values and values[-1] is None:
        values.pop()
    return values

def _next_tree_to_levels(root):
    levels = []
    queue = [root] if root is not None else []
    while queue:
        level_nodes = queue
        queue = []
        levels.append([node.val for node in level_nodes])
        for index, node in enumerate(level_nodes):
            expected_next = level_nodes[index + 1] if index + 1 < len(level_nodes) else None
            if node.next is not expected_next:
                raise TypeError("next pointers do not match the tree level order.")
            if node.left is not None:
                queue.append(node.left)
            if node.right is not None:
                queue.append(node.right)
    return levels

def _quad_tree_to_preorder(root):
    rows = []
    def visit(node):
        if node is None:
            return
        is_leaf = bool(getattr(node, "isLeaf", False))
        rows.append([1 if is_leaf else 0, 1 if (not is_leaf or getattr(node, "val", False)) else 0])
        if not is_leaf:
            visit(getattr(node, "topLeft", None))
            visit(getattr(node, "topRight", None))
            visit(getattr(node, "bottomLeft", None))
            visit(getattr(node, "bottomRight", None))
    visit(root)
    return rows

def _random_list_to_array(node):
    nodes = []
    seen = set()
    while node is not None and id(node) not in seen:
        if id(node) in _original_random_nodes:
            raise TypeError("Returned list must be a deep copy.")
        seen.add(id(node))
        nodes.append(node)
        node = node.next
    return [[node.val, None if node.random is None else nodes.index(node.random)] for node in nodes]

def _graph_to_adjacency(node):
    if node is None:
        return []
    by_value = {}
    queue = [node]
    for current in queue:
        if current is None:
            continue
        if id(current) in _original_graph_nodes:
            raise TypeError("Returned graph must be a deep copy.")
        if current.val in by_value:
            continue
        by_value[current.val] = current
        queue.extend(getattr(current, "neighbors", []) or [])
    return [sorted(neighbor.val for neighbor in by_value[value].neighbors) for value in sorted(by_value)]

def _prepare_args(case, spec, arg_types=None):
    raw = json.loads(json.dumps(case["input"]))
    arg_types = arg_types if arg_types is not None else spec.get("argTypes")
    args = []
    def find_tree_node(root, value):
        queue = [root] if root is not None else []
        for node in queue:
            if node.val == value:
                return node
            if node.left is not None:
                queue.append(node.left)
            if node.right is not None:
                queue.append(node.right)
        raise TypeError(f"Could not find tree node value {value}.")
    for index, value in enumerate(raw):
        arg_type = (arg_types or [None] * len(raw))[index]
        if arg_type == "cyclePos":
            continue
        if arg_type == "linkedList":
            args.append(_list_from_array(value, raw[spec["cyclePosParam"]] if "cyclePosParam" in spec else -1))
        elif arg_type == "linkedListArray":
            args.append([_list_from_array(items) for items in value])
        elif arg_type == "randomList":
            args.append(_random_list_from_array(value))
        elif arg_type == "graphNode":
            args.append(_graph_from_adjacency(value))
        elif arg_type == "binaryTree":
            args.append(_tree_from_array(value))
        elif arg_type == "nextTree":
            args.append(_tree_from_array(value, NextNode))
        elif arg_type == "treeNodeValue":
            args.append(find_tree_node(args[spec.get("nodeRefRootParam", 0)], value))
        else:
            args.append(value)
    return args

def _codetrial_run(user_code, spec_json):
    spec = json.loads(spec_json)
    node_class = QuadNode if spec.get("outputType") == "quadTree" else (GraphNode if "graphNode" in (spec.get("argTypes") or []) or spec.get("outputType") == "graphNode" else (NextNode if "nextTree" in (spec.get("argTypes") or []) or spec.get("outputType") == "nextTree" else Node))
    ns = {"ListNode": ListNode, "TreeNode": TreeNode, "Node": node_class, "List": List, "Optional": Optional}
    try:
        exec(user_code, ns)
    except Exception as exc:
        return json.dumps({"setupError": f"{type(exc).__name__}: {exc}"})

    try:
        if spec["kind"] == "class":
            target = ns[spec["className"]]
        else:
            entry = spec["entry"]
            if "Solution" in ns and hasattr(ns["Solution"], entry):
                target = getattr(ns["Solution"](), entry)
            elif entry in ns and callable(ns[entry]):
                target = ns[entry]
            else:
                raise KeyError(entry)
    except Exception:
        name = spec.get("className") or spec["entry"]
        return json.dumps({"setupError": f"Could not find {name} in your code - keep the starter signature."})

    results = []
    for case in spec["cases"]:
        t0 = time.perf_counter()
        try:
            if spec["kind"] == "class":
                methods, args_list = case["input"]
                instance = target(*_prepare_args({"input": args_list[0]}, spec, spec.get("constructorArgTypes")))
                actual = [None]
                for method, args in zip(methods[1:], args_list[1:]):
                    actual.append(getattr(instance, method)(*args))
            else:
                args = _prepare_args(case, spec)
                returned = target(*args)
                output_prefix_param = spec.get("outputPrefixParam")
                output_param = spec.get("outputParam")
                if isinstance(output_prefix_param, int):
                    if type(returned) is not int or returned < 0 or returned > len(args[output_prefix_param]):
                        raise TypeError("Return value must be an integer length for the kept prefix.")
                    actual = args[output_prefix_param][:returned]
                else:
                    actual = args[output_param] if isinstance(output_param, int) else returned
                    if spec.get("outputType") == "linkedList":
                        actual = _list_to_array(actual)
                    if spec.get("outputType") == "randomList":
                        actual = _random_list_to_array(actual)
                    if spec.get("outputType") == "graphNode":
                        actual = _graph_to_adjacency(actual)
                    if spec.get("outputType") == "binaryTree":
                        actual = _tree_to_array(actual)
                    if spec.get("outputType") == "nextTree":
                        actual = _next_tree_to_levels(actual)
                    if spec.get("outputType") == "quadTree":
                        actual = _quad_tree_to_preorder(actual)
            if isinstance(actual, tuple):
                actual = list(actual)
            try:
                json.dumps(actual)
            except (TypeError, ValueError):
                actual = repr(actual)
            results.append({"actual": actual, "timeMs": (time.perf_counter() - t0) * 1000})
        except Exception as exc:
            results.append({"error": f"{type(exc).__name__}: {exc}"[:400], "timeMs": (time.perf_counter() - t0) * 1000})
    return json.dumps({"results": results})

_codetrial_run(_CODETRIAL_CODE, _CODETRIAL_SPEC)
`;

/// Python runs in a Worker for the same reason JavaScript does: `while True:
/// pass` on the main thread would freeze the timer, the transcript and the
/// interviewer's audio with no way back short of a reload. The worker outlives
/// a single run so the multi-megabyte runtime is downloaded once, and is only
/// torn down when a run overruns.
async function runPython(code, spec, reportStatus = null) {
  reportStatus?.("booting");
  const worker = await pythonWorkerOnce();
  reportStatus?.("running");
  return new Promise((resolve) => {
    let timer = null;
    // A worker stuck in a loop can only be stopped by terminating it, and a
    // terminated worker cannot be reused: drop the cached one so the next run
    // starts a fresh runtime. Only a stuck or crashed worker is unusable - a
    // syntax error in the candidate's code comes back as a setupError from a
    // perfectly healthy runtime, and rebooting for that costs them the whole
    // multi-megabyte download again on their next run.
    const finish = (value, fatal = false) => {
      clearTimeout(timer);
      worker.onmessage = null;
      worker.onerror = null;
      if (fatal) {
        worker.terminate();
        pythonWorkerPromise = null;
      }
      resolve(value);
    };
    timer = setTimeout(() => {
      finish({ setupError: `Execution timed out after ${testTimeoutMs / 1000}s - check for an infinite loop.` }, true);
    }, testTimeoutMs);
    worker.onmessage = (event) => finish(event.data);
    worker.onerror = (event) => finish({ setupError: event.message || "The Python runtime crashed." }, true);
    worker.postMessage({ code, spec: JSON.stringify(spec) });
  });
}

function pythonWorkerOnce() {
  if (!pythonWorkerPromise) {
    pythonWorkerPromise = startPythonWorker();
    pythonWorkerPromise.catch(() => {
      pythonWorkerPromise = null;
    });
  }
  return pythonWorkerPromise;
}

/// Resolves only once the runtime is loaded, so the multi-megabyte first
/// download happens on this budget rather than eating a run's 5 seconds and
/// getting reported as an infinite loop.
async function startPythonWorker() {
  const indexURL = pyodideBaseUrl();
  const loader = await pyodideLoader(`${indexURL}pyodide.js`);
  const source = `${loader}
    const ready = loadPyodide({ indexURL: ${JSON.stringify(indexURL)} });
    ready.then(
      () => self.postMessage({ ready: true }),
      (error) => self.postMessage({ setupError: String((error && error.message) || error).slice(0, 400) }),
    );
    self.onmessage = async (event) => {
      try {
        const pyodide = await ready;
        pyodide.globals.set("_CODETRIAL_CODE", event.data.code);
        pyodide.globals.set("_CODETRIAL_SPEC", event.data.spec);
        self.postMessage(JSON.parse(pyodide.runPython(${JSON.stringify(pythonDriver)})));
      } catch (error) {
        self.postMessage({ setupError: String((error && error.message) || error).slice(0, 400) });
      }
    };
  `;
  const url = URL.createObjectURL(new Blob([source], { type: "application/javascript" }));
  const worker = new Worker(url);
  const booted = await new Promise((resolve) => {
    const timer = setTimeout(
      () => resolve({ setupError: "The Python runtime did not start in time." }),
      pythonBootTimeoutMs,
    );
    worker.onmessage = (event) => {
      clearTimeout(timer);
      resolve(event.data);
    };
    worker.onerror = (event) => {
      clearTimeout(timer);
      resolve({ setupError: event.message || "The Python runtime crashed while starting." });
    };
  });
  worker.onmessage = null;
  worker.onerror = null;
  // Revoked only once the worker has answered, so the script fetch cannot race
  // the revocation the way it can when this runs right after the constructor.
  URL.revokeObjectURL(url);
  if (!booted.ready) {
    worker.terminate();
    throw new Error(booted.setupError || "The Python runtime failed to start.");
  }
  return worker;
}

/// `importScripts` cannot carry subresource integrity, so the pin is checked
/// here and the verified text is baked into the worker instead.
/// Read as text rather than `importScripts`d, because the Worker is built from
/// a blob and the loader has to sit in the same source as the driver below.
///
/// No hash check here any more. The bytes are ours, `fetch-vendor` verified
/// them against SHA256SUMS before they reached the disk, and `connect-src` no
/// longer permits an origin that could substitute anything else.
async function pyodideLoader(url) {
  // Without a deadline a stalled response leaves the Run button disabled
  // forever.
  const response = await fetch(url, { signal: AbortSignal.timeout(pythonBootTimeoutMs) });
  if (!response.ok) throw new Error("Could not load the Python runtime.");
  return response.text();
}
