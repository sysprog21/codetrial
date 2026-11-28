import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { launchChromium, startStaticServer } from "./source.js";

const spec = {
  kind: "function",
  entry: "matchDisputedCharge",
  checker: "indexPair",
  paramNames: ["nums", "target"],
  paramTypes: ["integer[]", "integer"],
  returnType: "integer[]",
  cases: [{ label: "first", input: [[2, 7, 11, 15], 9], expected: [0, 1] }],
};

// Each answer fails on a known editor line, and the line is the one the
// browser reported rather than one a fake made up: JavaScript runs in a blob
// Worker, the only place its syntax and top-level errors reach
// `worker.onerror`. The Python cases are in python-driver.test.js.

const javascript = [
  {
    name: "a JavaScript error names its type and the candidate's line",
    code: `function matchDisputedCharge(nums, target) {
  const seen = new Map();
  return nums[100].x;
}
`,
    error:
      "TypeError: Cannot read properties of undefined (reading 'x') (line 3)",
  },
  {
    name: "a JavaScript error inside a helper names the helper's line",
    code: `function helper(nums, i) {
  return nums[i].x.y;
}
function matchDisputedCharge(nums, target) {
  const a = 1;
  return [helper(nums, 0), a];
}
`,
    error:
      "TypeError: Cannot read properties of undefined (reading 'y') (line 2)",
  },
  {
    name: "a JavaScript error at the top level names its line",
    code: `const limit = undefinedName;
function matchDisputedCharge(nums, target) {
  return [];
}
`,
    setupError: "ReferenceError: undefinedName is not defined (line 1)",
  },
  {
    name: "a JavaScript syntax error names its line",
    code: `function matchDisputedCharge(nums, target) {
  if (nums.length > 0 {
    return [0, 1];
  }
  return [];
}
`,
    setupError: "SyntaxError: Unexpected token '{' (line 2)",
  },
  {
    name: "a thrown JavaScript value that is not an Error is shown as thrown",
    code: `function matchDisputedCharge(nums, target) {
  throw "no pair";
}
`,
    error: "no pair",
  },
  {
    name: "a thrown value that cannot be shown fails its own case only",
    code: `class Odd extends Error {
  toString() {
    throw new Error("unprintable");
  }
}
function matchDisputedCharge(nums, target) {
  throw new Odd("odd");
}
`,
    error: "odd",
  },
  {
    name: "a thrown value with no text at all fails its own case only",
    code: `function matchDisputedCharge(nums, target) {
  throw Object.create(null);
}
`,
    error: "The code threw a value that cannot be shown.",
  },
  {
    name: "a long error in a case keeps its line within the bound",
    code: `function matchDisputedCharge(nums, target) {
  throw new Error("${"x".repeat(500)}");
}
`,
    error:
      `Error: ${"x".repeat(500)}`.slice(0, 400 - " (line 2)".length) +
      " (line 2)",
  },
  {
    name: "a long top-level error keeps its line within the bound",
    code: `const limit = ${"a".repeat(400)};
function matchDisputedCharge(nums, target) {
  return [];
}
`,
    setupError:
      `ReferenceError: ${"a".repeat(400)} is not defined`.slice(
        0,
        400 - " (line 1)".length,
      ) + " (line 1)",
  },
];

let browser, server, base;
before(async () => {
  browser = await launchChromium();
  if (!browser) return;
  ({ server, base } = await startStaticServer({
    handle: (_request, response, url) => {
      if (url.pathname !== "/judges/runtime-error-lines.json") return false;
      response.setHeader("content-type", "application/json");
      response.end(JSON.stringify(spec));
      return true;
    },
  }));
});
after(async () => {
  await browser?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
});

for (const { name, code, error, setupError } of javascript) {
  test(name, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await browser.newPage();
    try {
      await page.goto(`${base}/observer.html`);
      const summary = await page.evaluate(async (code) => {
        const { runBrowserTests } = await import("/runners.js");
        return runBrowserTests("runtime-error-lines", code, "javascript");
      }, code);
      assert.equal(summary.setupError, setupError);
      if (error !== undefined) assert.equal(summary.cases[0].error, error);
    } finally {
      await page.close();
    }
  });
}
