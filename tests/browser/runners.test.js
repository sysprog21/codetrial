// Both language runners build a Worker from a blob, and the two sources look
// identical at the call site: `new Blob([something])`. A search-and-replace
// across this file pointed the Python worker at the JavaScript runner's source,
// which boots a worker that never answers, so every Python run would have died
// on the "runtime did not start in time" timeout with nothing naming the cause.
//
// Nothing caught it. These files run in plain node with no DOM and no Worker,
// so no test here executes a runner; this one reads the source text instead,
// which is the same tripwire `meet-tab-audio.test.js` uses for `getUserMedia`.

import assert from "node:assert/strict";
import test from "node:test";
import { failFetchWith, functionBody, read } from "./source.js";
import { parseCandidateCase, runBrowserTests } from "../../web/runners.js";

const runners = read("web/runners.js");

test("a malformed candidate case is refused", () => {
  const spec = { kind: "function", paramNames: ["count"], paramTypes: ["integer"] };
  assert.throws(() => parseCandidateCase(spec, "{ nope"), /JSON argument array/);
  assert.throws(() => parseCandidateCase(spec, "[1.5]"), /Parameter 1 \(count\) must match integer/);
});

test("a candidate case is typed by paramTypes", () => {
  const spec = { kind: "function", paramNames: ["grid", "name"], paramTypes: ["character[][]", "string"] };
  assert.deepEqual(parseCandidateCase(spec, '[[["a", "b"]], "edge"]'), [[['a', 'b']], "edge"]);
  assert.throws(() => parseCandidateCase(spec, '[[["ab"]], "edge"]'), /Parameter 1 \(grid\)/);
});

// Every arm here is the last thing between a candidate case and a harness that
// does not bound-check it: C indexes `nodes[adjacency[i][j] - 1]` and C++
// `nodes[*items[i].second]` with no guard of their own. An arm that goes
// missing instead refuses every case for the scenarios that use it, so the
// accepting rows matter as much as the refusing ones.
test("node candidate cases are typed by argTypes", () => {
  const spec = {
    kind: "function", paramNames: ["head", "tree", "random", "graph"],
    paramTypes: ["ListNode", "TreeNode", "Node", "Node"],
    argTypes: ["linkedList", "binaryTree", "randomList", "graphNode"],
  };
  assert.deepEqual(
    parseCandidateCase(spec, '[[1,2],[1,null,2],[[7,null],[9,0]],[[2],[1]]]'),
    [[1, 2], [1, null, 2], [[7, null], [9, 0]], [[2], [1]]],
  );
  assert.throws(
    () => parseCandidateCase(spec, '[[1,"two"],[1,null,2],[[7,null],[9,0]],[[2],[1]]]'),
    /Parameter 1 \(head\)/,
  );
  assert.throws(
    () => parseCandidateCase(spec, '[[1,2],[1,{}],[[7,null],[9,0]],[[2],[1]]]'),
    /Parameter 2 \(tree\)/,
  );
  assert.throws(
    () => parseCandidateCase(spec, '[[1,2],[1,null,2],[[7],[9,0]],[[2],[1]]]'),
    /Parameter 3 \(random\)/,
  );
  assert.throws(
    () => parseCandidateCase(spec, '[[1,2],[1,null,2],[[7,null],[9,0]],[["two"],[1]]]'),
    /Parameter 4 \(graph\)/,
  );
  assert.throws(
    () => parseCandidateCase(spec, '[[1,2],[1,null,2],[[7,2],[9,0]],[[2],[1]]]'),
    /Parameter 3 \(random\)/,
  );
  assert.throws(
    () => parseCandidateCase(spec, '[[1,2],[1,null,2],[[7,null],[9,0]],[[3],[1]]]'),
    /Parameter 4 \(graph\)/,
  );
  // A zero neighbour reaches the C harness as nodes[-1], and a negative random
  // index reaches the C++ one as nodes[negative]; neither bounds-checks.
  assert.throws(
    () => parseCandidateCase(spec, '[[1,2],[1,null,2],[[7,null],[9,0]],[[0],[1]]]'),
    /Parameter 4 \(graph\)/,
  );
  assert.throws(
    () => parseCandidateCase(spec, '[[1,2],[1,null,2],[[7,-1],[9,0]],[[2],[1]]]'),
    /Parameter 3 \(random\)/,
  );
});

// Both are reachable (org-chart-row-navigation, shard-result-consolidation),
// and an arm that drops out falls through to `return false`, refusing every
// candidate case for that scenario rather than failing loudly.
test("next-pointer trees and linked list arrays are typed too", () => {
  const spec = {
    kind: "function", paramNames: ["rows", "shards"],
    paramTypes: ["Node", "ListNode[]"],
    argTypes: ["nextTree", "linkedListArray"],
  };
  assert.deepEqual(
    parseCandidateCase(spec, "[[1,2,3,null,4],[[1,4],[1,3]]]"),
    [[1, 2, 3, null, 4], [[1, 4], [1, 3]]],
  );
  assert.throws(() => parseCandidateCase(spec, '[[1,"two"],[[1,4]]]'), /Parameter 1 \(rows\) must match nextTree/);
  assert.throws(() => parseCandidateCase(spec, '[[1,2],[[1,"four"]]]'), /Parameter 2 \(shards\) must match linkedListArray/);
});

// The one arm that answers for a judge carrying no `argTypes` override. Every
// `Node` in the bank has one today, so without this the arm is dead code that
// still decides what a future judge accepts.
test("a node parameter with no argTypes override still admits an array", () => {
  const spec = { kind: "function", paramNames: ["node"], paramTypes: ["Node"] };
  assert.deepEqual(parseCandidateCase(spec, "[[1,2,3]]"), [[1, 2, 3]]);
  assert.throws(() => parseCandidateCase(spec, '[7]'), /Parameter 1 \(node\) must match Node/);
});

// Both parameters below are "Node" in the language signature, so a message
// naming `paramTypes` would describe neither constraint.
test("the refusal names the node shape that was checked", () => {
  const spec = {
    kind: "function", paramNames: ["random", "graph"],
    paramTypes: ["Node", "Node"], argTypes: ["randomList", "graphNode"],
  };
  assert.throws(
    () => parseCandidateCase(spec, '[[[7,2]],[[1]]]'),
    /Parameter 1 \(random\) must match randomList/,
  );
  assert.throws(
    () => parseCandidateCase(spec, '[[[7,null]],[[9]]]'),
    /Parameter 2 \(graph\) must match graphNode/,
  );
});

test("a cycle position names a list slot or -1", () => {
  const spec = {
    kind: "function", paramNames: ["head", "pos"],
    paramTypes: ["ListNode", "integer"], argTypes: ["linkedList", "cyclePos"],
    cyclePosParam: 1,
  };
  assert.deepEqual(parseCandidateCase(spec, "[[3,2,0,-4],1]"), [[3, 2, 0, -4], 1]);
  assert.deepEqual(parseCandidateCase(spec, "[[3,2,0,-4],-1]"), [[3, 2, 0, -4], -1]);
  assert.deepEqual(parseCandidateCase(spec, "[[3,2,0,-4],3]"), [[3, 2, 0, -4], 3]);
  assert.throws(() => parseCandidateCase(spec, "[[3,2,0,-4],4]"), /Parameter 2 must name a list position or -1/);
  assert.throws(() => parseCandidateCase(spec, "[[3,2,0,-4],-2]"), /Parameter 2 must name a list position or -1/);
  assert.throws(() => parseCandidateCase(spec, "[[],0]"), /Parameter 2 must name a list position or -1/);
});

// The runner looks the value up in the tree it was given. Letting an absent
// one through is a thrown TypeError in JavaScript and Python, and a null node
// in C, so the five languages disagree about a case the parser accepted.
test("a tree node reference names a value the tree holds", () => {
  const spec = {
    kind: "function", paramNames: ["root", "p", "q"],
    paramTypes: ["TreeNode", "integer", "integer"],
    argTypes: ["binaryTree", "treeNodeValue", "treeNodeValue"],
  };
  assert.deepEqual(
    parseCandidateCase(spec, "[[3,5,1,6,2,0,8,null,null,7,4],5,4]"),
    [[3, 5, 1, 6, 2, 0, 8, null, null, 7, 4], 5, 4],
  );
  assert.throws(() => parseCandidateCase(spec, "[[3,5,1],5,99]"), /Parameter 3 must name a value in the tree/);
  assert.throws(() => parseCandidateCase(spec, "[[3,5,1],99,1]"), /Parameter 2 must name a value in the tree/);
});

test("a class candidate case matches the judge operation signatures", () => {
  const spec = {
    kind: "class", className: "EventQueue", cases: [{
      input: [["EventQueue", "push", "pop", "size"], [[2], [7], [], []]],
    }],
  };
  assert.deepEqual(
    parseCandidateCase(spec, '[["EventQueue", "push", "size"], [[2], [7], []]]'),
    [["EventQueue", "push", "size"], [[2], [7], []]],
  );
  assert.throws(
    () => parseCandidateCase(spec, '[["EventQueue", "push"], [[2], []]]'),
    /Operation push does not accept 0 arguments/,
  );
  assert.throws(
    () => parseCandidateCase(spec, '[["EventQueue", "push"], [[2], ["seven"]]]'),
    /Arguments for operation push do not match its parameter types/,
  );
  assert.throws(
    () => parseCandidateCase(spec, '[["EventQueue"], [["two"]]]'),
    /Arguments for operation EventQueue do not match its parameter types/,
  );
  assert.throws(
    () => parseCandidateCase(spec, '[["EventQueue", "EventQueue"], [[2], [2]]]'),
    /Only the first operation may be EventQueue/,
  );
});

test("runBrowserTests reports the output of a candidate case", async () => {
  const restoreFetch = failFetchWith(async (url) => {
    if (String(url).startsWith("/judges/")) {
      return new Response(JSON.stringify({
        kind: "function", entry: "sum", paramNames: ["value"], paramTypes: ["integer"],
        returnType: "integer", checker: "exact", cases: [{ label: "judge", input: [1], expected: 1 }],
      }));
    }
    return new Response(JSON.stringify({ stdout: [{ text: '{"results":[{"actual":1,"timeMs":1},{"actual":7,"timeMs":2}]}' }] }));
  });
  try {
    const summary = await runBrowserTests("candidate-case-runner", "", "cpp", null, [{ input: [7] }]);
    assert.equal(summary.passed, 1);
    assert.equal(summary.total, 1);
    assert.deepEqual(summary.cases.at(-1), {
      label: "Your case 1", pass: null, got: "7", expected: undefined, input: "[7]", timeMs: 2, candidate: true,
    });
  } finally {
    restoreFetch();
  }
});

test("an observed candidate error keeps its expectation absent", async () => {
  const restoreFetch = failFetchWith(async (url) => {
    if (String(url).startsWith("/judges/")) {
      return new Response(JSON.stringify({
        kind: "function", entry: "sum", paramNames: ["value"], paramTypes: ["integer"],
        returnType: "integer", checker: "exact", cases: [{ label: "judge", input: [1], expected: 1 }],
      }));
    }
    return new Response(JSON.stringify({ stdout: [{ text: '{"results":[{"actual":1,"timeMs":1},{"error":"boom","timeMs":2}]}' }] }));
  });
  try {
    const summary = await runBrowserTests("candidate-case-error", "", "cpp", null, [{ input: [7] }]);
    assert.equal(summary.cases.at(-1).input, "[7]");
    assert.equal(summary.cases.at(-1).displayInput, "[7]");
    assert.equal(summary.cases.at(-1).expected, undefined);
    assert.equal(summary.cases.at(-1).error, "boom");
  } finally {
    restoreFetch();
  }
});

test("runBrowserTests retains display input for failures only", async () => {
  const restoreFetch = failFetchWith(async (url) => {
    if (String(url).startsWith("/judges/")) {
      return new Response(JSON.stringify({
        kind: "function", entry: "sum", paramNames: ["value"], paramTypes: ["integer"],
        returnType: "integer", checker: "exact", cases: [
          { label: "passing", input: [1], expected: 1 },
          { label: "failing", input: [2], expected: 2 },
          { label: "throws", input: [3], expected: 3 },
        ],
      }));
    }
    return new Response(JSON.stringify({ stdout: [{ text: JSON.stringify({ results: [
      { actual: 1, timeMs: 1 },
      { actual: 0, timeMs: 2 },
      { error: "boom", timeMs: 3 },
      { actual: 4, timeMs: 4 },
      { actual: 0, timeMs: 5 },
      { error: "timeout", timeMs: 6 },
    ] }) }] }));
  });
  try {
    const summary = await runBrowserTests("display-inputs", "", "cpp", null, [
      { input: [4], expected: 4 },
      { input: [5], expected: 5 },
      { input: [6], expected: 6 },
    ]);
    const { cases } = summary;

    assert.equal(summary.passed, 1);
    assert.equal(summary.total, 3);
    assert.equal(Object.hasOwn(cases[0], "displayInput"), false, "passing cases carry no display input");
    assert.equal(cases[1].displayInput, "[2]");
    assert.equal(cases[2].displayInput, "[3]");
    assert.equal(cases[3].label, "Your case 1");
    assert.equal(cases[3].input, "[4]", "the candidate payload keeps its input");
    assert.equal(Object.hasOwn(cases[3], "displayInput"), false, "passing candidate cases carry no display input");
    assert.equal(cases[4].label, "Your case 2");
    assert.equal(cases[4].input, "[5]");
    assert.equal(cases[4].displayInput, "[5]");
    assert.equal(cases[5].label, "Your case 3");
    assert.equal(cases[5].input, "[6]");
    assert.equal(cases[5].displayInput, "[6]");
  } finally {
    restoreFetch();
  }
});

test("each worker is built from its own source", () => {
  const blobs = [...runners.matchAll(/new Blob\(\[[^\]]*?(\w+)\]/g)].map((match) => match[1]);
  assert.equal(blobs.length, 2, "a third worker needs its own assertion here");

  assert.match(
    functionBody(runners, "runWorker"),
    /new Blob\(\[\.\.\.prelude, JS_RUNNER_SOURCE\]/,
    "the JavaScript runner must build its worker from the hoisted runner source",
  );

  const python = functionBody(runners, "startPythonWorker");
  assert.match(
    python,
    /new Blob\(\[source\]/,
    "the Python worker must build its worker from the Pyodide loader it just fetched, not from the JavaScript runner",
  );
  assert.match(python, /loadPyodide\(/, "the Python worker source has to load Pyodide");
});

test("the runner evaluates no string, so the page needs no unsafe-eval", () => {
  // The candidate's code is the first statements of the worker, not an
  // argument to eval inside it. This is what lets the page's
  // Content-Security-Policy withhold `unsafe-eval`: a blob worker inherits the
  // document's policy, so an eval here would have to be permitted for every
  // script on the interview page, which is where the candidate's own text is
  // already rendered.
  assert.doesNotMatch(runners, /\beval\s*\(/, "web/runners.js evaluates a string");
  assert.doesNotMatch(runners, /new Function\s*\(/, "web/runners.js builds a function from text");
  assert.match(
    functionBody(runners, "runWorker"),
    /__codetrialEntry/,
    "the entry point must be resolved by the worker prelude",
  );
});

// Hoisting the runner source to module scope is only correct while it reads
// nothing from a caller. The day it needs a value from runWorker, it has to go
// back inside or take a parameter, and this is what says so.
test("the hoisted runner source interpolates nothing", () => {
  const start = runners.indexOf("const JS_RUNNER_SOURCE = `");
  assert.notEqual(start, -1, "JS_RUNNER_SOURCE is gone from web/runners.js");
  const source = runners.slice(start, runners.indexOf("\n  `;\n", start));

  assert.match(source, /self\.onmessage/, "it has to still be a worker");
  assert.doesNotMatch(source, /\$\{/, "an interpolation here would read from a scope it no longer has");
});
