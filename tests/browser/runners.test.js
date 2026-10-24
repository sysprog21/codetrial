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
import { testPayload } from "../../web/lib.js";
import { parseCandidateCase, runBrowserTests } from "../../web/runners.js";

const runners = read("web/runners.js");

test("a malformed candidate case is refused", () => {
  const spec = {
    kind: "function",
    paramNames: ["count"],
    paramTypes: ["integer"],
  };
  assert.throws(
    () => parseCandidateCase(spec, "{ nope"),
    /JSON argument array/,
  );
  assert.throws(
    () => parseCandidateCase(spec, "[1.5]"),
    /Parameter 1 \(count\) must match integer/,
  );
});

test("a candidate integer outside the 32-bit range is refused", () => {
  const spec = {
    kind: "function",
    paramNames: [
      "count",
      "nums",
      "grid",
      "rows",
      "head",
      "lists",
      "root",
      "next",
      "random",
    ],
    paramTypes: [
      "integer",
      "integer[]",
      "integer[][]",
      "list<list<integer>>",
      "ListNode",
      "ListNode[]",
      "TreeNode",
      "Node",
      "Node",
    ],
    argTypes: [
      null,
      null,
      null,
      null,
      "linkedList",
      "linkedListArray",
      "binaryTree",
      "nextTree",
      "randomList",
    ],
  };
  const place = (value) => [
    value,
    [value],
    [[value]],
    [[value]],
    [value],
    [[value]],
    [value],
    [value],
    [[value, null]],
  ];
  for (const bound of [2147483647, -2147483648]) {
    const input = place(bound);
    assert.deepEqual(parseCandidateCase(spec, JSON.stringify(input)), input);
  }
  for (const outside of [2147483648, -2147483649]) {
    for (const [index, name] of spec.paramNames.entries()) {
      const input = place(0);
      input[index] = place(outside)[index];
      assert.throws(
        () => parseCandidateCase(spec, JSON.stringify(input)),
        new RegExp(
          `Parameter ${index + 1} \\(${name}\\) holds an integer outside -2147483648 to 2147483647\\.`,
        ),
      );
    }
  }
  const queue = {
    kind: "class",
    className: "EventQueue",
    cases: [
      {
        input: [
          ["EventQueue", "push"],
          [[2], [7]],
        ],
      },
    ],
  };
  const cursor = {
    kind: "class",
    className: "OrderedCursor",
    constructorArgTypes: ["binaryTree"],
    cases: [
      {
        input: [
          ["OrderedCursor", "next"],
          [[[7]], []],
        ],
      },
    ],
  };
  for (const [judge, operation, input] of [
    [
      queue,
      "push",
      (value) => [
        ["EventQueue", "push"],
        [[2], [value]],
      ],
    ],
    [
      queue,
      "EventQueue",
      (value) => [
        ["EventQueue", "push"],
        [[value], [7]],
      ],
    ],
    [cursor, "OrderedCursor", (value) => [["OrderedCursor"], [[[value]]]]],
  ]) {
    for (const bound of [2147483647, -2147483648]) {
      assert.deepEqual(
        parseCandidateCase(judge, JSON.stringify(input(bound))),
        input(bound),
      );
    }
    for (const outside of [2147483648, -2147483649]) {
      assert.throws(
        () => parseCandidateCase(judge, JSON.stringify(input(outside))),
        new RegExp(
          `Arguments for operation ${operation} hold an integer outside -2147483648 to 2147483647\\.`,
        ),
      );
    }
  }
  // The range is named only when it is the one fault: a number given for a
  // string, or a string for an integer, is still a type mismatch.
  assert.throws(
    () => parseCandidateCase(queue, '[["EventQueue", "push"], [[2], ["x"]]]'),
    /Arguments for operation push do not match its parameter types\./,
  );
  assert.throws(
    () =>
      parseCandidateCase(
        { kind: "function", paramNames: ["s"], paramTypes: ["string"] },
        "[3000000000]",
      ),
    /Parameter 1 \(s\) must match string\./,
  );
});

test("a cycle position, node reference and graph neighbor keep their checks", () => {
  const spec = {
    kind: "function",
    paramNames: ["head", "pos", "root", "node", "graph"],
    paramTypes: ["ListNode", "integer", "TreeNode", "TreeNode", "Node"],
    argTypes: [
      "linkedList",
      "cyclePos",
      "binaryTree",
      "treeNodeValue",
      "graphNode",
    ],
    cyclePosParam: 1,
    nodeRefRootParam: 2,
  };
  for (const [input, refusal] of [
    [
      [[1], 2147483648, [1], 1, [[]]],
      /Parameter 2 must name a list position or -1\./,
    ],
    [
      [[1], -1, [1], 2147483648, [[]]],
      /Parameter 4 must name a value in the tree\./,
    ],
    // Membership bounds a node reference only because the tree's own values
    // are bounded now: before, both this tree and the reference into it passed.
    [
      [[1], -1, [2147483648], 2147483648, [[]]],
      /Parameter 3 \(root\) holds an integer outside -2147483648 to 2147483647\./,
    ],
    [
      [[1], -1, [1], 1, [[2147483648]]],
      /Parameter 5 \(graph\) must match graphNode\./,
    ],
  ]) {
    assert.throws(
      () => parseCandidateCase(spec, JSON.stringify(input)),
      refusal,
    );
  }
});

test("a candidate case is typed by paramTypes", () => {
  const spec = {
    kind: "function",
    paramNames: ["grid", "name"],
    paramTypes: ["character[][]", "string"],
  };
  assert.deepEqual(parseCandidateCase(spec, '[[["a", "b"]], "edge"]'), [
    [["a", "b"]],
    "edge",
  ]);
  assert.throws(
    () => parseCandidateCase(spec, '[[["ab"]], "edge"]'),
    /Parameter 1 \(grid\)/,
  );
});

// Every arm here is the last thing between a candidate case and a harness that
// does not bound-check it: C indexes `nodes[adjacency[i][j] - 1]` and C++
// `nodes[*items[i].second]` with no guard of their own. An arm that goes
// missing instead refuses every case for the scenarios that use it, so the
// accepting rows matter as much as the refusing ones.
test("node candidate cases are typed by argTypes", () => {
  const spec = {
    kind: "function",
    paramNames: ["head", "tree", "random", "graph"],
    paramTypes: ["ListNode", "TreeNode", "Node", "Node"],
    argTypes: ["linkedList", "binaryTree", "randomList", "graphNode"],
  };
  assert.deepEqual(
    parseCandidateCase(spec, "[[1,2],[1,null,2],[[7,null],[9,0]],[[2],[1]]]"),
    [
      [1, 2],
      [1, null, 2],
      [
        [7, null],
        [9, 0],
      ],
      [[2], [1]],
    ],
  );
  assert.throws(
    () =>
      parseCandidateCase(
        spec,
        '[[1,"two"],[1,null,2],[[7,null],[9,0]],[[2],[1]]]',
      ),
    /Parameter 1 \(head\)/,
  );
  assert.throws(
    () => parseCandidateCase(spec, "[[1,2],[1,{}],[[7,null],[9,0]],[[2],[1]]]"),
    /Parameter 2 \(tree\)/,
  );
  assert.throws(
    () => parseCandidateCase(spec, "[[1,2],[1,null,2],[[7],[9,0]],[[2],[1]]]"),
    /Parameter 3 \(random\)/,
  );
  assert.throws(
    () =>
      parseCandidateCase(
        spec,
        '[[1,2],[1,null,2],[[7,null],[9,0]],[["two"],[1]]]',
      ),
    /Parameter 4 \(graph\)/,
  );
  assert.throws(
    () =>
      parseCandidateCase(spec, "[[1,2],[1,null,2],[[7,2],[9,0]],[[2],[1]]]"),
    /Parameter 3 \(random\)/,
  );
  assert.throws(
    () =>
      parseCandidateCase(spec, "[[1,2],[1,null,2],[[7,null],[9,0]],[[3],[1]]]"),
    /Parameter 4 \(graph\)/,
  );
  // A zero neighbour reaches the C harness as nodes[-1], and a negative random
  // index reaches the C++ one as nodes[negative]; neither bounds-checks.
  assert.throws(
    () =>
      parseCandidateCase(spec, "[[1,2],[1,null,2],[[7,null],[9,0]],[[0],[1]]]"),
    /Parameter 4 \(graph\)/,
  );
  assert.throws(
    () =>
      parseCandidateCase(spec, "[[1,2],[1,null,2],[[7,-1],[9,0]],[[2],[1]]]"),
    /Parameter 3 \(random\)/,
  );
});

// Both are reachable (org-chart-row-navigation, shard-result-consolidation),
// and an arm that drops out falls through to `return false`, refusing every
// candidate case for that scenario rather than failing loudly.
test("next-pointer trees and linked list arrays are typed too", () => {
  const spec = {
    kind: "function",
    paramNames: ["rows", "shards"],
    paramTypes: ["Node", "ListNode[]"],
    argTypes: ["nextTree", "linkedListArray"],
  };
  assert.deepEqual(parseCandidateCase(spec, "[[1,2,3,null,4],[[1,4],[1,3]]]"), [
    [1, 2, 3, null, 4],
    [
      [1, 4],
      [1, 3],
    ],
  ]);
  assert.throws(
    () => parseCandidateCase(spec, '[[1,"two"],[[1,4]]]'),
    /Parameter 1 \(rows\) must match nextTree/,
  );
  assert.throws(
    () => parseCandidateCase(spec, '[[1,2],[[1,"four"]]]'),
    /Parameter 2 \(shards\) must match linkedListArray/,
  );
});

// The one arm that answers for a judge carrying no `argTypes` override. Every
// `Node` in the bank has one today, so without this the arm is dead code that
// still decides what a future judge accepts.
test("a node parameter with no argTypes override still admits an array", () => {
  const spec = { kind: "function", paramNames: ["node"], paramTypes: ["Node"] };
  assert.deepEqual(parseCandidateCase(spec, "[[1,2,3]]"), [[1, 2, 3]]);
  assert.throws(
    () => parseCandidateCase(spec, "[7]"),
    /Parameter 1 \(node\) must match Node/,
  );
});

// Both parameters below are "Node" in the language signature, so a message
// naming `paramTypes` would describe neither constraint.
test("the refusal names the node shape that was checked", () => {
  const spec = {
    kind: "function",
    paramNames: ["random", "graph"],
    paramTypes: ["Node", "Node"],
    argTypes: ["randomList", "graphNode"],
  };
  assert.throws(
    () => parseCandidateCase(spec, "[[[7,2]],[[1]]]"),
    /Parameter 1 \(random\) must match randomList/,
  );
  assert.throws(
    () => parseCandidateCase(spec, "[[[7,null]],[[9]]]"),
    /Parameter 2 \(graph\) must match graphNode/,
  );
});

test("a cycle position names a list slot or -1", () => {
  const spec = {
    kind: "function",
    paramNames: ["head", "pos"],
    paramTypes: ["ListNode", "integer"],
    argTypes: ["linkedList", "cyclePos"],
    cyclePosParam: 1,
  };
  assert.deepEqual(parseCandidateCase(spec, "[[3,2,0,-4],1]"), [
    [3, 2, 0, -4],
    1,
  ]);
  assert.deepEqual(parseCandidateCase(spec, "[[3,2,0,-4],-1]"), [
    [3, 2, 0, -4],
    -1,
  ]);
  assert.deepEqual(parseCandidateCase(spec, "[[3,2,0,-4],3]"), [
    [3, 2, 0, -4],
    3,
  ]);
  assert.throws(
    () => parseCandidateCase(spec, "[[3,2,0,-4],4]"),
    /Parameter 2 must name a list position or -1/,
  );
  assert.throws(
    () => parseCandidateCase(spec, "[[3,2,0,-4],-2]"),
    /Parameter 2 must name a list position or -1/,
  );
  assert.throws(
    () => parseCandidateCase(spec, "[[],0]"),
    /Parameter 2 must name a list position or -1/,
  );
});

// The runner looks the value up in the tree it was given. Letting an absent
// one through is a thrown TypeError in JavaScript and Python, and a null node
// in C, so the five languages disagree about a case the parser accepted.
test("a tree node reference names a value the tree holds", () => {
  const spec = {
    kind: "function",
    paramNames: ["root", "p", "q"],
    paramTypes: ["TreeNode", "integer", "integer"],
    argTypes: ["binaryTree", "treeNodeValue", "treeNodeValue"],
  };
  assert.deepEqual(
    parseCandidateCase(spec, "[[3,5,1,6,2,0,8,null,null,7,4],5,4]"),
    [[3, 5, 1, 6, 2, 0, 8, null, null, 7, 4], 5, 4],
  );
  assert.throws(
    () => parseCandidateCase(spec, "[[3,5,1],5,99]"),
    /Parameter 3 must name a value in the tree/,
  );
  assert.throws(
    () => parseCandidateCase(spec, "[[3,5,1],99,1]"),
    /Parameter 2 must name a value in the tree/,
  );
});

test("a class candidate case matches the judge operation signatures", () => {
  const spec = {
    kind: "class",
    className: "EventQueue",
    cases: [
      {
        input: [
          ["EventQueue", "push", "pop", "size"],
          [[2], [7], [], []],
        ],
      },
    ],
  };
  assert.deepEqual(
    parseCandidateCase(
      spec,
      '[["EventQueue", "push", "size"], [[2], [7], []]]',
    ),
    [
      ["EventQueue", "push", "size"],
      [[2], [7], []],
    ],
  );
  assert.throws(
    () => parseCandidateCase(spec, '[["EventQueue", "push"], [[2], []]]'),
    /Operation push does not accept 0 arguments/,
  );
  assert.throws(
    () =>
      parseCandidateCase(spec, '[["EventQueue", "push"], [[2], ["seven"]]]'),
    /Arguments for operation push do not match its parameter types/,
  );
  assert.throws(
    () => parseCandidateCase(spec, '[["EventQueue"], [["two"]]]'),
    /Arguments for operation EventQueue do not match its parameter types/,
  );
  assert.throws(
    () =>
      parseCandidateCase(spec, '[["EventQueue", "EventQueue"], [[2], [2]]]'),
    /Only the first operation may be EventQueue/,
  );
});

test("runBrowserTests keeps inputs on every non-passing case", async () => {
  const restoreFetch = failFetchWith(async (url) => {
    if (String(url).startsWith("/judges/")) {
      return new Response(
        JSON.stringify({
          kind: "function",
          entry: "pair",
          paramNames: ["values", "target"],
          paramTypes: ["integer[]", "integer"],
          returnType: "integer[]",
          checker: "indexPair",
          cases: [
            { label: "judge pass", input: [[2, 7], 9], expected: [0, 1] },
            {
              label: "judge mismatch",
              input: [[3, 4], 99],
              expected: [0, 1],
            },
            {
              label: "judge exception",
              input: [[5, 6], 11],
              expected: [0, 1],
            },
            {
              label: "judge missing",
              input: [[7, 8], 15],
              expected: [0, 1],
            },
            {
              label: "judge checker exception",
              input: [null, 0],
              expected: [0, 1],
            },
          ],
        }),
      );
    }
    return new Response(
      JSON.stringify({
        stdout: [
          {
            text: JSON.stringify({
              results: [
                { actual: [0, 1], timeMs: 1 },
                { actual: [0, 1], timeMs: 2 },
                { error: "built-in boom", timeMs: 3 },
                null,
                { actual: [0, 1], timeMs: 4 },
                { actual: [1, 1], timeMs: 5 },
                { error: "candidate boom", timeMs: 6 },
                { actual: [9, 8], timeMs: 7 },
                { actual: "<output>", timeMs: 8 },
              ],
            }),
          },
        ],
      }),
    );
  });
  try {
    const summary = await runBrowserTests("failed-inputs", "", "cpp", null, [
      {
        label: "candidate mismatch",
        input: [[5, 5], 10],
        expected: [0, 1],
      },
      { label: "candidate exception", input: [[1], 1] },
      { label: "candidate output one", input: [[9, 8], 17] },
      { label: "candidate output two", input: [[4], 4] },
    ]);
    assert.equal(summary.passed, 1);
    assert.equal(summary.total, 5);
    assert.equal(
      summary.code,
      "",
      "the summary carries the code this run executed",
    );
    assert.equal(
      summary.runnerUnavailable,
      undefined,
      "a run that reached the runner never says it is missing",
    );
    assert.deepEqual(summary.cases, [
      {
        label: "judge pass",
        pass: true,
        got: "[0,1]",
        expected: "[0,1]",
        timeMs: 1,
        candidate: false,
      },
      {
        label: "judge mismatch",
        pass: false,
        got: "[0,1]",
        expected: "[0,1]",
        input: "[[3,4],99]",
        timeMs: 2,
        candidate: false,
      },
      {
        label: "judge exception",
        pass: false,
        got: "-",
        expected: "[0,1]",
        input: "[[5,6],11]",
        error: "built-in boom",
        timeMs: 3,
        candidate: false,
      },
      {
        label: "judge missing",
        pass: false,
        got: "-",
        expected: "[0,1]",
        input: "[[7,8],15]",
        error: "No result produced.",
        timeMs: 0,
        candidate: false,
      },
      {
        label: "judge checker exception",
        pass: false,
        got: "[0,1]",
        expected: "[0,1]",
        input: "[null,0]",
        error: "Cannot read properties of null (reading 'length')",
        timeMs: 4,
        candidate: false,
      },
      {
        label: "candidate mismatch",
        pass: false,
        got: "[1,1]",
        expected: "[0,1]",
        input: "[[5,5],10]",
        timeMs: 5,
        candidate: true,
      },
      {
        label: "candidate exception",
        pass: false,
        got: "-",
        expected: undefined,
        input: "[[1],1]",
        error: "candidate boom",
        timeMs: 6,
        candidate: true,
      },
      {
        label: "candidate output one",
        pass: null,
        got: "[9,8]",
        expected: undefined,
        input: "[[9,8],17]",
        timeMs: 7,
        candidate: true,
      },
      {
        label: "candidate output two",
        pass: null,
        got: '"<output>"',
        expected: undefined,
        input: "[[4],4]",
        timeMs: 8,
        candidate: true,
      },
    ]);
  } finally {
    restoreFetch();
  }
});

test("each worker is built from its own source", () => {
  const blobs = [...runners.matchAll(/new Blob\(\[[^\]]*?(\w+)\]/g)].map(
    (match) => match[1],
  );
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
  assert.match(
    python,
    /loadPyodide\(/,
    "the Python worker source has to load Pyodide",
  );
});

test("both runners refuse a returned list that loops back into itself", () => {
  // Stopping at the first revisited node prints a cycle as the acyclic list it
  // loops over, which the exact checker then accepts as a correct answer.
  const javascript = runners.slice(
    runners.indexOf("const listToArray"),
    runners.indexOf("};", runners.indexOf("const listToArray")),
  );
  assert.match(
    javascript,
    /if \(seen\.has\(node\)\)\s*throw new TypeError\("Returned list must not reach a node twice\."\)/,
  );
  assert.doesNotMatch(javascript, /while \(node && !seen\.has\(node\)\)/);

  const python = runners.slice(
    runners.indexOf("def _list_to_array"),
    runners.indexOf("return values", runners.indexOf("def _list_to_array")),
  );
  assert.match(
    python,
    /if id\(node\) in seen:\s*raise TypeError\("Returned list must not reach a node twice\."\)/,
  );
  assert.doesNotMatch(python, /id\(node\) not in seen/);
});

test("both runners refuse a copied random list that loops back into itself", () => {
  // A copy whose last node links back to the first holds every value and
  // random pointer, so stopping at the revisit would print a correct answer.
  const javascript = runners.slice(
    runners.indexOf("const randomListToArray"),
    runners.indexOf("};", runners.indexOf("const randomListToArray")),
  );
  assert.match(
    javascript,
    /if \(seen\.has\(node\)\)\s*throw new TypeError\("Returned list must not reach a node twice\."\)/,
  );
  assert.doesNotMatch(javascript, /while \(node && !seen\.has\(node\)\)/);

  const python = runners.slice(
    runners.indexOf("def _random_list_to_array"),
    runners.indexOf("def _graph_to_adjacency"),
  );
  assert.match(
    python,
    /if id\(node\) in seen:\s*raise TypeError\("Returned list must not reach a node twice\."\)/,
  );
  assert.doesNotMatch(python, /id\(node\) not in seen/);
});

test("the runner evaluates no string, so the page needs no unsafe-eval", () => {
  // The candidate's code is the first statements of the worker, not an
  // argument to eval inside it. This is what lets the page's
  // Content-Security-Policy withhold `unsafe-eval`: a blob worker inherits the
  // document's policy, so an eval here would have to be permitted for every
  // script on the interview page, which is where the candidate's own text is
  // already rendered.
  assert.doesNotMatch(
    runners,
    /\beval\s*\(/,
    "web/runners.js evaluates a string",
  );
  assert.doesNotMatch(
    runners,
    /new Function\s*\(/,
    "web/runners.js builds a function from text",
  );
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
  assert.doesNotMatch(
    source,
    /\$\{/,
    "an interpolation here would read from a scope it no longer has",
  );
});

test("execution infrastructure failures allow tracing but candidate failures do not", async (t) => {
  const spec = {
    kind: "function",
    entry: "sum",
    paramNames: ["value"],
    paramTypes: ["integer"],
    returnType: "integer",
    checker: "exact",
    cases: [{ label: "judge", input: [1], expected: 1 }],
  };
  const scenarios = [
    {
      name: "HTTP outage",
      response: () => new Response("", { status: 503 }),
      unavailable: true,
    },
    {
      name: "rate limited",
      response: () => new Response("", { status: 429 }),
      unavailable: true,
    },
    // Refused requests and unbuildable harnesses are this run's own, and an
    // edit can fix them: a hand trace must not stand in for Test on them.
    {
      name: "request rejected",
      response: () => new Response("", { status: 413 }),
      unavailable: false,
    },
    {
      name: "harness cannot be built",
      spec: { paramTypes: ["Unrepresentable"] },
      unavailable: false,
    },
    {
      name: "network failure",
      response: () => {
        throw new TypeError("Failed to fetch");
      },
      unavailable: true,
    },
    {
      name: "request timeout",
      response: () => {
        throw new DOMException("Timed out", "AbortError");
      },
      unavailable: true,
    },
    {
      name: "invalid response",
      response: () => new Response("<html>down</html>"),
      unavailable: true,
    },
    {
      name: "compilation error",
      response: () =>
        Response.json({
          didExecute: false,
          stderr: [{ text: "syntax error" }],
        }),
      unavailable: false,
    },
    {
      name: "execution timeout",
      response: () => Response.json({ timedOut: true }),
      unavailable: false,
    },
    {
      name: "runtime error",
      response: () =>
        Response.json({ code: 1, stderr: [{ text: "runtime error" }] }),
      unavailable: false,
    },
    {
      name: "Python loader failure",
      language: "python",
      response: () => new Response("", { status: 503 }),
      unavailable: true,
    },
    {
      name: "Python boot failure",
      language: "python",
      boot: { setupError: "Wasm failed to initialize" },
      unavailable: true,
    },
    {
      name: "Python syntax error",
      language: "python",
      boot: { ready: true },
      unavailable: false,
    },
  ];
  for (const scenario of scenarios) {
    await t.test(scenario.name, async (t) => {
      const restoreFetch = failFetchWith(async (url) =>
        String(url).startsWith("/judges/")
          ? Response.json({ ...spec, ...scenario.spec })
          : (scenario.response?.() ?? new Response("")),
      );
      if (scenario.boot) {
        t.mock.method(globalThis.URL, "createObjectURL", () => "blob:test");
        t.mock.method(globalThis.URL, "revokeObjectURL", () => {});
        const previous = globalThis.Worker;
        globalThis.Worker = class {
          constructor() {
            queueMicrotask(() => this.onmessage({ data: scenario.boot }));
          }
          postMessage() {
            queueMicrotask(() =>
              this.onmessage({
                data: { setupError: "SyntaxError: invalid syntax" },
              }),
            );
          }
          terminate() {}
        };
        t.after(() => {
          if (previous === undefined) delete globalThis.Worker;
          else globalThis.Worker = previous;
        });
      }
      try {
        const summary = await runBrowserTests(
          "infrastructure-" + scenario.name,
          "candidate code",
          scenario.language ?? "cpp",
        );
        assert.ok(summary.setupError);
        assert.equal(Boolean(summary.runnerUnavailable), scenario.unavailable);
        assert.equal(
          testPayload(summary).runnerUnavailable,
          scenario.unavailable,
        );
        assert.equal(summary.passed, 0);
        assert.deepEqual(summary.cases, []);
      } finally {
        restoreFetch();
      }
    });
  }
});

test("a compile error reaches the summary on the candidate's line", async () => {
  const restoreFetch = failFetchWith(async (url, request) => {
    if (String(url).startsWith("/judges/")) {
      return Response.json({
        kind: "function",
        entry: "sum",
        paramNames: ["value"],
        paramTypes: ["integer"],
        returnType: "integer",
        checker: "exact",
        cases: [{ label: "judge", input: [1], expected: 1 }],
      });
    }
    const { source } = JSON.parse(request.body);
    const line = source.split("\n").indexOf("  return value") + 1;
    return Response.json({
      didExecute: false,
      stderr: [{ text: `<source>:${line}:15: error: expected ';'` }],
    });
  });
  try {
    const summary = await runBrowserTests(
      "compile-error-line",
      "int sum(int value) {\n  return value\n}",
      "cpp",
    );
    assert.equal(summary.setupError, "<source>:2:15: error: expected ';'");
  } finally {
    restoreFetch();
  }
});

const sumJudge = {
  kind: "function",
  entry: "sum",
  paramNames: ["value"],
  paramTypes: ["integer"],
  returnType: "integer",
  checker: "exact",
  cases: [{ label: "judge", input: [1], expected: 1 }],
};

// A dropped connection or a stalled compile says nothing about the code, and
// the next run usually succeeds, so one more request goes out before the
// candidate and Jim hear about it. Any answer, even an error, is final: a
// compile error comes back inside a 200, and sending it twice only doubles the
// wait for the same message.
test("a compiled run is sent once more only when no answer came back", async (t) => {
  const dropped = () => {
    throw new TypeError("Failed to fetch");
  };
  const stalled = () => {
    throw new DOMException("Timed out", "AbortError");
  };
  const passed = () =>
    Response.json({
      stdout: [
        { text: JSON.stringify({ results: [{ actual: 1, timeMs: 1 }] }) },
      ],
    });
  const scenarios = [
    { name: "dropped, then answered", answers: [dropped, passed], sent: 2 },
    { name: "stalled, then answered", answers: [stalled, passed], sent: 2 },
    // However the two unanswered requests failed, the message counts them
    // without naming a wait, and only one that stalled is called a timeout.
    {
      name: "dropped twice",
      answers: [dropped, dropped, passed],
      sent: 2,
      unavailable: true,
      says: /^Compiler Explorer did not respond to 2 requests\. /,
      timeout: false,
    },
    {
      name: "stalled twice",
      answers: [stalled, stalled, passed],
      sent: 2,
      unavailable: true,
      says: /^Compiler Explorer did not respond to 2 requests\. /,
      timeout: true,
    },
    {
      name: "dropped, then stalled",
      answers: [dropped, stalled, passed],
      sent: 2,
      unavailable: true,
      says: /^Compiler Explorer did not respond to 2 requests\. /,
      timeout: true,
    },
    {
      name: "stalled, then dropped",
      answers: [stalled, dropped, passed],
      sent: 2,
      unavailable: true,
      says: /^Compiler Explorer did not respond to 2 requests\. /,
      timeout: true,
    },
    {
      name: "server error",
      answers: [() => new Response("", { status: 503 }), passed],
      sent: 1,
      unavailable: true,
    },
    {
      name: "request rejected",
      answers: [() => new Response("", { status: 413 }), passed],
      sent: 1,
      unavailable: false,
    },
    {
      name: "compilation error",
      answers: [
        () =>
          Response.json({
            didExecute: false,
            stderr: [{ text: "syntax error" }],
          }),
        passed,
      ],
      sent: 1,
      unavailable: false,
    },
    {
      name: "unreadable answer",
      answers: [() => new Response("<html>down</html>"), passed],
      sent: 1,
      unavailable: true,
    },
    {
      name: "answered, then stalled",
      answers: [
        () =>
          new Response(
            new ReadableStream({
              pull(controller) {
                controller.error(new DOMException("Timed out", "AbortError"));
              },
            }),
          ),
        passed,
      ],
      sent: 1,
      unavailable: true,
      says: /did not finish its answer within 20 seconds/,
      timeout: true,
    },
  ];
  for (const scenario of scenarios) {
    await t.test(scenario.name, async () => {
      let sent = 0;
      const restoreFetch = failFetchWith(async (url) => {
        if (String(url).startsWith("/judges/")) return Response.json(sumJudge);
        sent += 1;
        return scenario.answers[sent - 1]();
      });
      try {
        const summary = await runBrowserTests(
          "retry-" + scenario.name,
          "int sum(int value) { return value; }",
          "cpp",
        );
        assert.equal(sent, scenario.sent);
        if (scenario.says) assert.match(summary.setupError, scenario.says);
        if (scenario.timeout !== undefined) {
          assert.equal(
            summary.diagnostic?.category === "timeout",
            scenario.timeout,
          );
        }
        if (scenario.unavailable === undefined) {
          assert.equal(summary.setupError, undefined);
          assert.equal(summary.passed, 1);
        } else {
          assert.ok(summary.setupError);
          assert.equal(
            Boolean(summary.runnerUnavailable),
            scenario.unavailable,
          );
        }
      } finally {
        restoreFetch();
      }
    });
  }
});

// The scenarios above throw the abort themselves, so they would still pass
// with the timer removed or shared between attempts. Here the real timer has
// to fire: each request is cut off by its own, and the second only after its
// full budget.
test("each compiled request is cut off by a timer of its own", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const signals = [];
  const restoreFetch = failFetchWith((url, request) => {
    if (String(url).startsWith("/judges/")) {
      return Promise.resolve(Response.json(sumJudge));
    }
    signals.push(request.signal);
    return new Promise((_, reject) =>
      request.signal.addEventListener("abort", () =>
        reject(new DOMException("Timed out", "AbortError")),
      ),
    );
  });
  const requests = async (count) => {
    for (let turn = 0; turn < 100 && signals.length < count; turn += 1) {
      await new Promise(setImmediate);
    }
    assert.equal(signals.length, count);
  };
  try {
    const summary = runBrowserTests(
      "retry-own-timer",
      "int sum(int value) { return value; }",
      "cpp",
    );
    await requests(1);
    t.mock.timers.tick(19_999);
    assert.equal(signals[0].aborted, false);
    t.mock.timers.tick(1);
    assert.equal(signals[0].aborted, true);

    await requests(2);
    assert.equal(signals[1].aborted, false, "the retry is not sent aborted");
    t.mock.timers.tick(19_999);
    assert.equal(signals[1].aborted, false, "the retry gets its full budget");
    t.mock.timers.tick(1);

    const result = await summary;
    assert.equal(signals.length, 2);
    assert.equal(result.runnerUnavailable, true);
    assert.equal(result.diagnostic?.category, "timeout");
    assert.match(
      result.setupError,
      /^Compiler Explorer did not respond to 2 requests\. /,
      "the message counts the requests without naming a wait",
    );
  } finally {
    restoreFetch();
  }
});

test("task-mode JavaScript candidate errors are distinct from worker infrastructure outages", async (t) => {
  const spec = {
    kind: "function",
    entry: "sum",
    paramNames: ["value"],
    paramTypes: ["integer"],
    returnType: "integer",
    checker: "exact",
    cases: [{ label: "ordinary", input: [1], expected: 1 }],
  };
  for (const scenario of ["syntax", "timeout", "platform"]) {
    await t.test(scenario, async (t) => {
      const restoreFetch = failFetchWith(async () => Response.json(spec));
      t.mock.method(URL, "createObjectURL", () => "blob:test");
      t.mock.method(URL, "revokeObjectURL", () => {});
      const originalWorker = globalThis.Worker;
      globalThis.Worker = class {
        constructor() {
          if (scenario === "platform") throw new Error("Worker unavailable");
        }
        postMessage() {
          if (scenario === "syntax")
            queueMicrotask(() =>
              this.onerror({ message: "SyntaxError: invalid code" }),
            );
        }
        terminate() {}
      };
      if (scenario === "timeout") {
        const originalTimeout = globalThis.setTimeout;
        t.mock.method(globalThis, "setTimeout", (callback, delay, ...args) => {
          if (delay === 5000) {
            queueMicrotask(callback);
            return originalTimeout(() => {}, 0);
          }
          return originalTimeout(callback, delay, ...args);
        });
      }
      try {
        const summary = await runBrowserTests(
          `worker-${scenario}`,
          "candidate code",
          "javascript",
          null,
          [],
          spec,
        );
        assert.equal(
          Boolean(summary.runnerUnavailable),
          scenario === "platform",
        );
        assert.ok(summary.setupError);
        assert.equal(summary.total, 1);
        // An interview keeps its contract: a runner that threw is a setup
        // error, never an outage that would let a hand trace stand for Test.
        const interview = await runBrowserTests(
          `worker-${scenario}-interview`,
          "candidate code",
          "javascript",
        );
        assert.equal(interview.runnerUnavailable, undefined);
        assert.ok(interview.setupError);
      } finally {
        restoreFetch();
        if (originalWorker === undefined) delete globalThis.Worker;
        else globalThis.Worker = originalWorker;
      }
    });
  }
});
