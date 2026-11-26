// The Python driver is plain Python that the page runs in Pyodide. These tests
// run it on CPython instead, python3 or the interpreter PYTHON names, which
// needs no Pyodide download.

import assert from "node:assert/strict";
import test from "node:test";
import {
  assertVoidTable,
  candidateSource,
  finalVoidJudge,
  operationsOf,
  read,
  SENTINEL,
  pythonSkip,
  runPythonDriver as runDriver,
} from "./source.js";

const skip = pythonSkip();

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
  "Python driver preserves exact add-binary sums beyond 64 bits",
  { skip },
  () => {
    const spec = JSON.parse(read("web/judges/wide-register-arithmetic.json"));
    assert.ok(spec.cases.some(({ expected }) => expected.length > 64));
    const reply = runDriver(
      candidateSource("python-driver/add-binary.py"),
      spec,
    );
    assert.equal(reply.setupError, undefined);
    assert.equal(reply.results.length, spec.cases.length);
    spec.cases.forEach(({ label, expected }, index) => {
      assert.equal(reply.results[index].error, undefined, label);
      assert.equal(reply.results[index].actual, expected, label);
    });
  },
);

test("add-binary cases reject per-character Python recursion", { skip }, () => {
  const spec = JSON.parse(read("web/judges/wide-register-arithmetic.json"));
  const label = "maximum length all ones plus one";
  const row = spec.cases.find((testCase) => testCase.label === label);
  assert.ok(row, `the built-in cases must keep "${label}"`);
  assert.equal(row.input[0].length, 10000);
  const reply = runDriver(
    candidateSource("python-driver/add-binary.per-character-recursion.py"),
    spec,
  );
  assert.equal(reply.setupError, undefined);
  assert.equal(reply.results.length, spec.cases.length);
  const index = spec.cases.indexOf(row);
  assert.match(reply.results[index].error ?? "", /RecursionError/, label);
});

test(
  "candidate diagnostics do not corrupt the Python host protocol",
  { skip },
  () => {
    const reply = runDriver(
      `print('module diagnostic')
class Solution:
    def count(self, value):
        print('{"results": []}')
        return value
`,
      countToThree,
    );
    assert.deepEqual(reply.results.map(outcome), [
      { actual: 1 },
      { actual: 2 },
      { actual: 3 },
    ]);
  },
);

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
      candidateSource("python-driver/shared-self.py"),
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
      candidateSource("python-driver/counts-init.py"),
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
      candidateSource("python-driver/init-raises.py"),
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
  const reply = runDriver(candidateSource("python-driver/counter-class.py"), {
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
  });
  assert.deepEqual(
    reply.results.map((result) => result.actual),
    [
      [null, 1, 2],
      [null, 1],
    ],
  );
});

// The Python counterpart of the JavaScript candidate in runners.test.js: every
// operation returns the sentinel with its call number on the instance.
const sentinelClass = (spec) =>
  [
    `class ${spec.className}:`,
    "    def __init__(self, *args):",
    "        self.calls = 0",
    ...operationsOf(spec).map(
      (method) =>
        `    def ${method}(self, *args):\n        self.calls += 1\n        return ${JSON.stringify(SENTINEL)} + " " + str(self.calls)`,
    ),
  ].join("\n") + "\n";

test(
  "a void class operation records null whatever it returns",
  { skip },
  () => {
    assertVoidTable((spec) => runDriver(sentinelClass(spec), spec).results);
  },
);

test("a final void call still reports its error", { skip }, () => {
  const reply = runDriver(
    `class Counter:
    def finish(self):
        raise ValueError("final void ran")
`,
    finalVoidJudge,
  );
  assert.match(reply.results[0].error, /final void ran/);
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
