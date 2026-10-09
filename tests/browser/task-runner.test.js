import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  launchChromium,
  startStaticServer,
  DEFAULT_RUNTIME_CONFIG,
} from "./source.js";

let browser, server, base;
before(async () => {
  browser = await launchChromium();
  ({ server, base } = await startStaticServer({
    runtimeConfig: DEFAULT_RUNTIME_CONFIG,
  }));
});
after(async () => {
  await browser?.close();
  await new Promise((resolve) => server.close(resolve));
});

test("a custom task executes Python practice cases without loading a catalog judge", async (t) => {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage();
  const catalogRequests = [];
  page.on("request", (request) => {
    if (
      request.url().includes("/judges/") ||
      request.url().includes("/problems/")
    )
      catalogRequests.push(request.url());
  });
  await page.route("**/task-runner-test", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: "<!doctype html><title>Task runner test</title>",
    }),
  );
  try {
    await page.goto(`${base}/task-runner-test`);
    const result = await page.evaluate(async () => {
      const { runBrowserTests } = await import("/runners.js");
      return runBrowserTests(
        "custom-unit-conversion",
        "def convert_units(value):\n    return value * 2\n",
        "python",
        null,
        [],
        {
          kind: "function",
          entry: "convert_units",
          checker: "exact",
          cases: [
            { label: "zero", input: [0], expected: 0 },
            { label: "positive", input: [3], expected: 6 },
          ],
        },
      );
    });
    assert.equal(result.total, 2, JSON.stringify(result));
    assert.equal(result.passed, 2, JSON.stringify(result));
    assert.deepEqual(catalogRequests, []);
  } finally {
    await page.close();
  }
});
