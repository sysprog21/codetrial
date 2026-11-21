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
