import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  closeSetupCheck,
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  startStaticServer,
} from "./source.js";

let browser;
let server;
let base;
before(async () => {
  browser = await launchChromium();
  if (!browser) return;
  ({ server, base } = await startStaticServer({
    runtimeConfig: DEFAULT_RUNTIME_CONFIG,
  }));
});
after(async () => {
  await browser?.close();
  await new Promise((done) => (server ? server.close(done) : done()));
});

async function open(t) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.clock.install();
  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`);
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-input").placeholder !== "",
  );
  await closeSetupCheck(page);
  await page.locator("#language-select").selectOption("javascript");
  return page;
}
async function run(
  page,
  code = "function matchDisputedCharge(nums, target) { return [0, 1]; }",
) {
  await page.locator("#editor").fill(code);
  await page.locator("#run-tests").click();
  await page.waitForFunction(
    () => document.querySelector("#run-tests").textContent === "Run tests",
  );
}
const visibleText = (page) => page.locator("#results-content").innerText();

test("Copy results copies visible plain text and respects the existing filter", async (t) => {
  const page = await open(t);
  if (!page) return;
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await run(page);
  const original = await page.locator("#results-content").innerHTML();
  const code = await page.locator("#editor").inputValue();
  const text = (await visibleText(page)).trim();
  assert.match(text, /OK/);
  assert.match(text, /FAIL/);
  assert.match(text, /input .*\nexpected .*\ngot /);
  const button = page.getByRole("button", {
    name: "Copy results",
    exact: true,
  });
  await button.focus();
  await page.keyboard.press("Enter");
  await page.waitForFunction(
    () =>
      document.querySelector("[data-copy-results-status]").textContent ===
      "Copied!",
  );
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), text);
  assert.equal(
    await button.evaluate((node) => node === document.activeElement),
    true,
  );
  assert.equal(await page.locator("#results-content").innerHTML(), original);
  assert.equal(await page.locator("#editor").inputValue(), code);
  await page.clock.fastForward(2100);
  assert.equal(
    await page.locator("[data-copy-results-status]").textContent(),
    "",
  );
  await page.getByRole("checkbox", { name: "Hide passed tests" }).check();
  const filtered = (await visibleText(page)).trim();
  assert.doesNotMatch(filtered, /\bOK\b/);
  await button.click();
  await page.waitForFunction(
    () =>
      document.querySelector("[data-copy-results-status]").textContent ===
      "Copied!",
  );
  assert.equal(
    await page.evaluate(() => navigator.clipboard.readText()),
    filtered,
  );
});

test("clipboard rejection and missing API preserve results and allow retry", async (t) => {
  const page = await open(t);
  if (!page) return;
  await run(
    page,
    "function matchDisputedCharge() { throw new Error('diagnostic <tag> & detail\\n    second line'); }",
  );
  const text = (await visibleText(page)).trim();
  assert.match(text, /diagnostic <tag> & detail\n    second line/);
  const original = await page.locator("#results-content").innerHTML();
  const button = page.getByRole("button", {
    name: "Copy results",
    exact: true,
  });
  for (const unavailable of [false, true]) {
    await page.evaluate((missing) => {
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: missing
          ? undefined
          : {
              writeText: async () => {
                throw new DOMException("Denied", "NotAllowedError");
              },
            },
      });
    }, unavailable);
    await button.click();
    await page.waitForFunction(() =>
      document
        .querySelector("[data-copy-results-status]")
        .textContent.includes("Could not copy results"),
    );
    assert.equal(
      await page.locator("[data-copy-results-status]").getAttribute("role"),
      "status",
    );
    assert.equal(await page.locator("#results-content").innerHTML(), original);
    assert.notEqual(await button.getAttribute("aria-disabled"), "true");
  }
  await page.evaluate(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (text) => {
          globalThis.copiedResults = text;
        },
      },
    });
  });
  await button.click();
  await page.waitForFunction(
    () =>
      document.querySelector("[data-copy-results-status]").textContent ===
      "Copied!",
  );
  assert.equal(await page.evaluate(() => globalThis.copiedResults), text);
});

test("Copy results is unavailable before a run, during a run and after invalidation", async (t) => {
  const page = await open(t);
  if (!page) return;
  const button = page.locator("[data-copy-results]");
  assert.equal(await button.count(), 1);
  assert.equal(await button.isVisible(), false);
  await page.locator("#candidate-case > summary").click();
  await page.locator("#candidate-case-input").fill("[[2,7],9]");
  await page.locator("#candidate-case-add").click();
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-list").children.length === 1,
  );
  await run(page);
  assert.equal(await button.isVisible(), true);
  await page.locator("[data-remove-case]").click();
  assert.equal(await button.isVisible(), false);
  await run(page);
  assert.equal(await button.isVisible(), true);
  await page.evaluate(() => {
    const post = Worker.prototype.postMessage;
    Worker.prototype.postMessage = function (...args) {
      if (args[0]?.name === "matchDisputedCharge" && args[0]?.spec) return;
      return post.apply(this, args);
    };
  });
  await page.locator("#run-tests").click();
  assert.equal(await button.isVisible(), false);
});
