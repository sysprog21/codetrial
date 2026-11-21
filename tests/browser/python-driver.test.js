// The Python driver is plain Python that the page runs in Pyodide. These tests
// run it on CPython instead, python3 or the interpreter PYTHON names, which
// needs no Pyodide download.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { pythonDriver } from "../../web/runners.js";
import { read } from "./source.js";

const python = process.env.PYTHON || "python3";
// Only a missing interpreter skips. One that is there but does not start, or
// exits nonzero, fails every test with what the probe saw, rather than reading
// as absent.
const probe = spawnSync(python, ["--version"], {
  encoding: "utf8",
  timeout: 10000,
});
const skip =
  probe.error?.code === "ENOENT" ? `${python} is not installed` : false;
let startFailure;
if (!skip && probe.error) {
  startFailure = `${python} did not start: ${probe.error.message}`;
} else if (!skip && probe.status !== 0) {
  const detail = probe.stderr.trim();
  startFailure = `${python} --version exited with ${probe.status ?? probe.signal}${detail && `: ${detail}`}`;
}

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
  assert.equal(startFailure, undefined, startFailure);
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

const countToThree = {
  kind: "function",
  entry: "count",
  cases: [{ input: [1] }, { input: [2] }, { input: [3] }],
};

// A raised exception has to reach the page as the case's error, the one field
// that fails a case whatever it expected, not as the value it returned.
const outcome = (result) =>
  "error" in result ? { error: result.error } : { actual: result.actual };

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
    // Each case reports how many times __init__ has run, counted on a class
    // attribute that carries over between cases, whether self is the newest
    // object it ran on and was never run on before, and the attributes self
    // already had. A driver that shares, copies, re-initializes or prebuilds
    // an instance gets at least one of the three wrong.
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
      countToThree,
    );
    assert.deepEqual(reply.results.map(outcome), [
      { actual: [1, true, []] },
      { error: "ValueError: two (line 12, in count)" },
      { actual: [3, true, []] },
    ]);
  },
);

test(
  "an exception from __init__ fails only the case it was raised in",
  { skip },
  () => {
    // Only the second construction raises, so a driver that repeats the
    // first constructor error, or stops at it, gets the third case wrong.
    const reply = runDriver(
      `class Solution:
    built = 0

    def __init__(self):
        Solution.built += 1
        if Solution.built == 2:
            self.memo = {}[0]

    def count(self, value):
        return value
`,
      countToThree,
    );
    assert.equal(reply.setupError, undefined);
    assert.deepEqual(reply.results.map(outcome), [
      { actual: 1 },
      { error: "KeyError: 0 (line 7, in __init__)" },
      { actual: 3 },
    ]);
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

// A runtime error names the candidate's editor line: the driver compiles the
// code as <candidate> and reports its innermost frame there.
const indexPair = {
  kind: "function",
  entry: "matchDisputedCharge",
  checker: "indexPair",
  paramNames: ["nums", "target"],
  paramTypes: ["integer[]", "integer"],
  returnType: "integer[]",
  cases: [{ label: "first", input: [[2, 7, 11, 15], 9], expected: [0, 1] }],
};

const errorLines = [
  {
    name: "a Python error names the candidate's innermost line and function",
    code: `def helper(nums, i):
    return nums[i]

class Solution:
    def matchDisputedCharge(self, nums, target):
        a = helper(nums, 0)
        b = helper(nums, 99)
        return [a, b]
`,
    error: "IndexError: list index out of range (line 2, in helper)",
  },
  {
    name: "a Python error at the top level names its line",
    code: `limit = undefined_name

class Solution:
    def matchDisputedCharge(self, nums, target):
        return []
`,
    setupError: "NameError: name 'undefined_name' is not defined (line 1)",
  },
  {
    name: "a Python syntax error names its line without the code's file name",
    code: `class Solution:
    def matchDisputedCharge(self, nums, target):
        for i in range(len(nums)):
            if nums[i] == target
                return [i]
        return []
`,
    setupError: "SyntaxError: expected ':' (line 4)",
  },
  {
    name: "a Python syntax error raised by eval names the line that ran it",
    code: `class Solution:
    def matchDisputedCharge(self, nums, target):
        return eval("1 +")
`,
    error: "SyntaxError: invalid syntax (line 3, in matchDisputedCharge)",
  },
  {
    name: "a long Python error in a case keeps its line within the bound",
    code: `class Solution:
    def matchDisputedCharge(self, nums, target):
        raise ValueError("${"x".repeat(500)}")
`,
    error:
      `ValueError: ${"x".repeat(500)}`.slice(
        0,
        400 - " (line 3, in matchDisputedCharge)".length,
      ) + " (line 3, in matchDisputedCharge)",
  },
];

for (const { name, code, error, setupError } of errorLines) {
  test(name, { skip }, () => {
    const reply = runDriver(code, indexPair);
    assert.equal(reply.setupError, setupError);
    if (error !== undefined) assert.equal(reply.results[0].error, error);
  });
}
