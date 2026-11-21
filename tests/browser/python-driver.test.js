// The Python driver is plain Python that the page runs in Pyodide. These tests
// run it on CPython instead, which needs no Pyodide download, and skip when
// python3 is absent.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { pythonDriver } from "../../web/runners.js";
import { read } from "./source.js";

const python = process.env.PYTHON || "python3";
const skip =
  spawnSync(python, ["--version"]).status === 0
    ? false
    : `${python} is not installed`;

// The worker sets the code and the judge as two globals and reads back the
// value of the driver's last line, which pyodide.runPython returns. CPython's
// exec returns nothing, so the last line is split off and evaluated on its own.
const host = `
import ast, json, sys
request = json.load(sys.stdin)
module = ast.parse(request["driver"])
last = module.body.pop()
scope = {"_CODETRIAL_CODE": request["code"], "_CODETRIAL_SPEC": request["spec"]}
exec(compile(module, "<driver>", "exec"), scope)
print(eval(compile(ast.Expression(last.value), "<driver>", "eval"), scope))
`;

function runDriver(code, spec) {
  const run = spawnSync(python, ["-I", "-c", host], {
    input: JSON.stringify({
      driver: pythonDriver,
      code,
      spec: JSON.stringify(spec),
    }),
    encoding: "utf8",
    timeout: 10000,
  });
  assert.equal(run.status, 0, run.error?.message ?? run.stderr);
  return JSON.parse(run.stdout);
}

const maxPathSum = () =>
  JSON.parse(read("web/judges/grid-segment-net-yield.json"));

test(
  "state an answer keeps on self starts fresh in every case",
  { skip },
  () => {
    const spec = maxPathSum();
    const expected = spec.cases.map((testCase) => testCase.expected);
    // This answer only ever raises self.best, so a Solution shared across
    // cases shows only when some case's answer is below one before it.
    assert.ok(
      expected.some(
        (value, index) => value < Math.max(...expected.slice(0, index)),
      ),
      "the judge's cases no longer expose a Solution shared across cases",
    );
    const reply = runDriver(
      `class Solution:
    def __init__(self):
        self.best = float("-inf")

    def bestRouteYield(self, root: Optional[TreeNode]) -> int:
        def gain(node):
            if node is None:
                return 0
            left = max(gain(node.left), 0)
            right = max(gain(node.right), 0)
            self.best = max(self.best, node.val + left + right)
            return node.val + max(left, right)

        gain(root)
        return self.best
`,
      spec,
    );
    assert.deepEqual(
      reply.results.map((result) => result.actual),
      expected,
    );
  },
);

test(
  "each case runs on a new Solution that __init__ built for it",
  { skip },
  () => {
    // Each case reports how many times __init__ has run, whether self is the
    // newest object it ran on and was never run on before, and the attributes
    // self already had. A driver that shares, copies, re-initializes or
    // prebuilds an instance gets at least one of the three wrong.
    const reply = runDriver(
      `class Solution:
    built = []

    def __init__(self):
        Solution.built.append(self)

    def count(self, value):
        newest = Solution.built[-1] is self and Solution.built.count(self) == 1
        seen = [len(Solution.built), newest, list(vars(self))]
        self.value = value
        if value == 2:
            raise ValueError("two")
        return seen
`,
      {
        kind: "function",
        entry: "count",
        cases: [{ input: [1] }, { input: [2] }, { input: [3] }],
      },
    );
    assert.deepEqual(
      reply.results.map((result) => result.actual ?? result.error),
      [[1, true, []], "ValueError: two", [3, true, []]],
    );
  },
);

test(
  "an exception from __init__ fails each case with its own message",
  { skip },
  () => {
    const spec = maxPathSum();
    const reply = runDriver(
      `class Solution:
    def __init__(self):
        self.memo = {}[0]

    def bestRouteYield(self, root: Optional[TreeNode]) -> int:
        return 0
`,
      spec,
    );
    assert.equal(reply.setupError, undefined);
    assert.deepEqual(
      reply.results.map((result) => result.error),
      spec.cases.map(() => "KeyError: 0"),
    );
  },
);

test(
  "a Solution without the entry method is still a setup error",
  { skip },
  () => {
    const reply = runDriver(
      `class Solution:
    def maxPathSum(self, root: Optional[TreeNode]) -> int:
        return 0
`,
      maxPathSum(),
    );
    assert.deepEqual(reply, {
      setupError:
        "Could not find bestRouteYield in your code - keep the starter signature.",
    });
  },
);

test("a class judge calls a case's methods on one instance", { skip }, () => {
  const reply = runDriver(
    `class Counter:
    def __init__(self):
        self.count = 0

    def add(self):
        self.count += 1
        return self.count
`,
    {
      kind: "class",
      className: "Counter",
      cases: [
        {
          input: [
            ["Counter", "add", "add"],
            [[], [], []],
          ],
        },
        {
          input: [
            ["Counter", "add"],
            [[], []],
          ],
        },
      ],
    },
  );
  assert.deepEqual(
    reply.results.map((result) => result.actual),
    [
      [null, 1, 2],
      [null, 1],
    ],
  );
});

test("an answer written as a plain function still runs", { skip }, () => {
  const reply = runDriver(
    `def double(value):
    return value * 2
`,
    {
      kind: "function",
      entry: "double",
      cases: [{ input: [2] }, { input: [5] }],
    },
  );
  assert.deepEqual(
    reply.results.map((result) => result.actual),
    [4, 10],
  );
});
