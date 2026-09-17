import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { existsSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

import { launchChromium, root } from "./source.js";

const web = join(root, "web");
let browser = null;
let server = null;
let base = "";

before(async () => {
  browser = await launchChromium();
  if (!browser) return;
  server = createServer((request, response) => {
    const path = new URL(request.url, "http://candidate.invalid").pathname;
    const file = join(web, path === "/" ? "index.html" : path);
    if (!file.startsWith(web) || !existsSync(file) || statSync(file).isDirectory()) {
      response.statusCode = 404;
      response.end("not found");
      return;
    }
    response.setHeader("content-type", file.endsWith(".js") ? "text/javascript" : file.endsWith(".wasm") ? "application/wasm" : "application/json");
    response.end(readFileSync(file));
  });
  await new Promise((listening) => server.listen(0, "127.0.0.1", listening));
  base = `http://127.0.0.1:${server.address().port}`;
});

after(async () => {
  await browser?.close();
  await new Promise((closed) => (server ? server.close(closed) : closed()));
});

async function runCandidate(language, code) {
  const page = await browser.newPage();
  try {
    await page.goto(base);
    return await page.evaluate(async ({ language, code }) => {
      const { runBrowserTests } = await import("/runners.js");
      return runBrowserTests("chargeback-pair-match", code, language, null, [{ input: [[2, 7, 11, 15], 9] }]);
    }, { language, code });
  } finally {
    await page.close();
  }
}

test("candidate case output is shown by the JavaScript runner", async () => {
  if (!browser) return;
  const summary = await runCandidate("javascript", "function matchDisputedCharge(nums, target) { return [0, 1]; }");
  assert.equal(summary.total, summary.cases.length - 1);
  assert.equal(summary.cases.at(-1).candidate, true);
  assert.equal(summary.cases.at(-1).pass, null);
  assert.equal(summary.cases.at(-1).got, "[0,1]");
});

test("candidate case output is shown by the Python runner", async () => {
  if (!browser) return;
  const summary = await runCandidate("python", "def matchDisputedCharge(nums, target):\n    return [0, 1]\n");
  assert.equal(summary.total, summary.cases.length - 1);
  assert.equal(summary.cases.at(-1).candidate, true);
  assert.equal(summary.cases.at(-1).pass, null);
  assert.equal(summary.cases.at(-1).got, "[0,1]");
});
