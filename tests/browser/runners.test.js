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
import { functionBody, read } from "./source.js";
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

test("runBrowserTests reports the output of a candidate case", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    if (String(url).startsWith("/judges/")) {
      return new Response(JSON.stringify({
        kind: "function", entry: "sum", paramNames: ["value"], paramTypes: ["integer"],
        returnType: "integer", checker: "exact", cases: [{ label: "judge", input: [1], expected: 1 }],
      }));
    }
    return new Response(JSON.stringify({ stdout: [{ text: '{"results":[{"actual":1,"timeMs":1},{"actual":7,"timeMs":2}]}' }] }));
  };
  try {
    const summary = await runBrowserTests("candidate-case-runner", "", "cpp", null, [{ input: [7] }]);
    assert.equal(summary.passed, 1);
    assert.equal(summary.total, 1);
    assert.deepEqual(summary.cases.at(-1), {
      label: "Your case 1", pass: null, got: "7", expected: undefined, timeMs: 2, candidate: true,
    });
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("each worker is built from its own source", () => {
  const blobs = [...runners.matchAll(/new Blob\(\[(\w+)\]/g)].map((match) => match[1]);
  assert.equal(blobs.length, 2, "a third worker needs its own assertion here");

  assert.match(
    functionBody(runners, "runWorker"),
    /new Blob\(\[JS_RUNNER_SOURCE\]/,
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
