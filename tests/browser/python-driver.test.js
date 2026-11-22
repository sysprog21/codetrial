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
      { error: "ValueError: two" },
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
      { error: "KeyError: 0" },
      { actual: 3 },
    ]);
  },
);
