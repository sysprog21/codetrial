import { after, before, test } from "node:test";
import assert from "node:assert/strict";

import {
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  startStaticServer,
} from "./source.js";

let browser = null;
let server = null;
let base = "";

before(async () => {
  browser = await launchChromium();
  if (!browser) return;
  ({ server, base } = await startStaticServer({
    runtimeConfig: DEFAULT_RUNTIME_CONFIG,
  }));
});

after(async () => {
  await browser?.close();
  await new Promise((closed) => (server ? server.close(closed) : closed()));
});

const partial = "function matchDisputedCharge(nums, target) { return [0, 1]; }";
const correct = `function matchDisputedCharge(nums, target) {
  for (let i = 0; i < nums.length; i++) {
    for (let j = i + 1; j < nums.length; j++) {
      if (nums[i] + nums[j] === target) return [i, j];
    }
  }
}`;

async function openInterview(t) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`);
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-input").placeholder !== "",
  );
  await page.locator("#language-select").selectOption("javascript");
  await page.evaluate(() => {
    document.querySelector("#audio-check").hidden = true;
    const post = Worker.prototype.postMessage;
    globalThis.testRuns = 0;
    Worker.prototype.postMessage = function (...args) {
      if (args[0]?.name === "matchDisputedCharge" && args[0]?.spec) {
        globalThis.testRuns++;
        if (globalThis.holdTestRun) {
          globalThis.releaseTestRun = () => post.apply(this, args);
          return;
        }
      }
      return post.apply(this, args);
    };
  });
  return page;
}

async function waitForRun(page) {
  await page.waitForFunction(
    () => document.querySelector("#run-tests").textContent === "Run tests",
  );
}

async function run(page, code) {
  await page.locator("#editor").fill(code);
  await page.locator("#run-tests").click();
  await waitForRun(page);
}

test("the checkbox hides only passes without running tests or losing focus", async (t) => {
  const page = await openInterview(t);
  if (!page) return;
  const toggle = page.getByRole("checkbox", { name: "Hide passed tests" });
  assert.equal(await page.locator("#hide-passed-tests").isChecked(), false);
  assert.equal(await toggle.isVisible(), false);
  await page.locator("#candidate-case > summary").click();
  await page.locator("#candidate-case-input").fill("[[2,7],9]");
  await page.locator("#candidate-case-add").click();
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-list").children.length === 1,
  );
  await run(page, partial);
  const content = page.locator("#results-content");
  const full = await content.innerHTML();
  assert.match(full, />OK</);
  assert.match(full, />FAIL</);
  assert.match(full, />OUTPUT</);
  const count = await content.locator("strong").textContent();
  const runs = await page.evaluate(() => globalThis.testRuns);
  assert.equal(runs, 1);
  const warning = "Test results (not sent to the interviewer; run again)";
  await page.locator("#results-label").evaluate((node, text) => {
    node.textContent = text;
  }, warning);
  await toggle.focus();
  await page.keyboard.press("Space");
  assert.equal(await toggle.isChecked(), true);
  assert.equal(
    await toggle.evaluate((node) => node === document.activeElement),
    true,
  );
  const filtered = await content.innerHTML();
  assert.doesNotMatch(filtered, />OK</);
  assert.match(filtered, />FAIL</);
  assert.match(filtered, />OUTPUT</);
  assert.equal(await content.locator("strong").textContent(), count);
  assert.equal(await page.locator("#results-label").textContent(), warning);
  await page.locator("#results-toggle").click();
  assert.equal(await toggle.isVisible(), false);
  await page.locator("#results-toggle").click();
  assert.equal(await toggle.isChecked(), true);
  await toggle.uncheck();
  assert.equal(await content.innerHTML(), full);
  assert.equal(await page.evaluate(() => globalThis.testRuns), runs);
  await page.locator("[data-remove-case]").click();
  assert.equal(await toggle.isVisible(), false);
  assert.match(await content.textContent(), /Run the test cases any time/);
});

test("both run shortcuts work after toggling the filter with the keyboard", async (t) => {
  const page = await openInterview(t);
  if (!page) return;
  await run(page, partial);
  const toggle = page.getByRole("checkbox", { name: "Hide passed tests" });
  for (const chord of ["Control+Enter", "Meta+Enter"]) {
    await toggle.focus();
    await page.keyboard.press("Space");
    assert.equal(
      await toggle.evaluate((node) => node === document.activeElement),
      true,
    );
    const runs = await page.evaluate(() => globalThis.testRuns);
    await page.keyboard.press(chord);
    await page.waitForFunction(
      (expected) => globalThis.testRuns === expected,
      runs + 1,
      { timeout: 5000 },
    );
    await waitForRun(page);
    assert.match(
      await page.locator("#results-content").textContent(),
      /Run finished/,
    );
  }
});

test("new runs hide stale results and retain the filter through success and errors", async (t) => {
  const page = await openInterview(t);
  if (!page) return;
  const toggle = page.getByRole("checkbox", { name: "Hide passed tests" });
  const content = page.locator("#results-content");
  await run(page, partial);
  await toggle.check();
  await page.evaluate(() => {
    globalThis.holdTestRun = true;
  });
  await page.locator("#editor").fill(correct);
  await page.locator("#run-tests").click();
  await page.waitForFunction(
    () => typeof globalThis.releaseTestRun === "function",
  );
  assert.equal(await toggle.isVisible(), false);
  assert.match(await content.textContent(), /Running test cases/);
  assert.equal(await content.locator("li").count(), 0);
  await page.evaluate(() => {
    globalThis.holdTestRun = false;
    globalThis.releaseTestRun();
  });
  await waitForRun(page);
  assert.equal(await toggle.isChecked(), true);
  assert.equal(await content.locator("li").count(), 0);
  const count = await content.locator("strong").textContent();
  const totals = count.match(/(\d+)\/(\d+) test cases passed/);
  assert.ok(totals);
  assert.ok(Number(totals[1]) > 0);
  assert.equal(totals[1], totals[2]);
  assert.equal(await content.locator("ul").count(), 0);
  assert.ok(
    (await content.textContent()).includes(`${totals[1]} passed tests hidden`),
  );
  await toggle.uncheck();
  assert.equal(await content.locator("li").count(), Number(totals[2]));
  assert.doesNotMatch(await content.textContent(), /passed tests? hidden/);
  await toggle.check();
  await run(page, "function {");
  assert.equal(await toggle.isVisible(), false);
  assert.match(await content.textContent(), /Couldn't run your code/);
  assert.match(await content.textContent(), /SyntaxError/);
  await run(page, partial);
  assert.equal(await toggle.isChecked(), true);
  assert.match(await content.innerHTML(), />FAIL</);
  assert.doesNotMatch(await content.innerHTML(), />OK</);
});
