import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  closeSetupCheck,
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  pauseClock,
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

async function open(t, { missingClipboard = false } = {}) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.clock.install();
  await page.addInitScript((missing) => {
    globalThis.copiedResults = [];
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: missing
        ? undefined
        : {
            writeText: async (text) => globalThis.copiedResults.push(text),
          },
    });
  }, missingClipboard);
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
const visibleText = async (page) =>
  (await page.locator("#results-content").innerText()).replace(
    /^Run finished\.\s*/,
    "",
  );

const copiedFeedback = (page) =>
  page.waitForFunction(
    () =>
      document.querySelector("[data-copy-results-status]").textContent ===
      "Copied!",
  );

test("a refused run keeps old results unavailable through filter changes", async (t) => {
  const page = await open(t);
  if (!page) return;
  await run(page);
  const button = page.locator("[data-copy-results]");
  const filter = page.locator("#results-filter");
  assert.equal(await button.isVisible(), true);
  assert.equal(await filter.isVisible(), true);
  await page.locator("#candidate-case").evaluate((node) => {
    node.open = true;
  });
  await page.locator("#candidate-case-input").fill("not json");
  await page.locator("#run-tests").click();
  await page.waitForFunction(
    () => document.querySelector("#run-tests").textContent === "Run tests",
  );
  const refusal = await page.locator("#candidate-case-status").textContent();
  assert.match(refusal, /JSON/);
  assert.equal(await page.locator("#results-content").textContent(), refusal);
  assert.equal(await filter.isVisible(), false);
  assert.equal(await button.isVisible(), false);
  await page.locator("#hide-passed-tests").evaluate((node) => {
    node.checked = !node.checked;
    node.dispatchEvent(new Event("change", { bubbles: true }));
  });
  assert.equal(await page.locator("#results-content").textContent(), refusal);
  assert.equal(await button.isVisible(), false);
  assert.deepEqual(await page.evaluate(() => globalThis.copiedResults), []);
});

/// The worker never answers a test run, so the page stays mid-run.
const holdWorkerRuns = (page) =>
  page.evaluate(() => {
    const post = Worker.prototype.postMessage;
    Worker.prototype.postMessage = function (...args) {
      if (args[0]?.name === "matchDisputedCharge" && args[0]?.spec) return;
      return post.apply(this, args);
    };
  });

test("Copy results copies visible plain text and respects the existing filter", async (t) => {
  const page = await open(t);
  if (!page) return;
  await run(page);
  await pauseClock(page);
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
  await copiedFeedback(page);
  const copied = await page.evaluate(() => globalThis.copiedResults.at(-1));
  assert.doesNotMatch(copied, /Run finished\./);
  assert.equal(copied, text);
  assert.equal(
    await button.evaluate((node) => node === document.activeElement),
    true,
  );
  assert.equal(await page.locator("#results-content").innerHTML(), original);
  assert.equal(await page.locator("#editor").inputValue(), code);
  await page.clock.fastForward(1000);
  await page.getByRole("checkbox", { name: "Hide passed tests" }).check();
  assert.equal(
    await page.locator("[data-copy-results-status]").textContent(),
    "Copied!",
  );
  await page.clock.fastForward(1100);
  assert.equal(
    await page.locator("[data-copy-results-status]").textContent(),
    "",
  );
  const filtered = (await visibleText(page)).trim();
  assert.doesNotMatch(filtered, /\bOK\b/);
  await button.click();
  await copiedFeedback(page);
  assert.equal(
    await page.evaluate(() => globalThis.copiedResults.at(-1)),
    filtered,
  );
});

for (const rejectCopy of [false, true]) {
  test(`filter changes preserve a pending clipboard ${rejectCopy ? "failure" : "success"}`, async (t) => {
    const page = await open(t);
    if (!page) return;
    await run(page);
    await pauseClock(page);
    const original = (await visibleText(page)).trim();
    await page.evaluate((rejectCopy) => {
      navigator.clipboard.writeText = (text) => {
        globalThis.copiedResults.push(text);
        return new Promise((resolve, reject) => {
          globalThis.finishCopy = () => {
            if (rejectCopy)
              reject(new DOMException("Denied", "NotAllowedError"));
            else resolve();
          };
        });
      };
    }, rejectCopy);
    const button = page.locator("[data-copy-results]");
    const status = page.locator("[data-copy-results-status]");
    const filter = page.getByRole("checkbox", { name: "Hide passed tests" });
    await button.click();
    assert.equal(await status.textContent(), "Copying...");
    await filter.check();
    assert.doesNotMatch(await visibleText(page), /\bOK\b/);
    assert.equal(await button.isVisible(), true);
    assert.equal(await button.getAttribute("aria-disabled"), "true");
    assert.equal(await status.textContent(), "Copying...");
    await page.evaluate(() => globalThis.finishCopy());
    const expected = rejectCopy
      ? "Could not copy results. Select the results and copy them manually."
      : "Copied!";
    await page.waitForFunction(
      (expected) =>
        document.querySelector("[data-copy-results-status]").textContent ===
        expected,
      expected,
    );
    assert.equal(await button.getAttribute("aria-disabled"), null);
    assert.deepEqual(await page.evaluate(() => globalThis.copiedResults), [
      original,
    ]);
    await filter.uncheck();
    assert.equal(await status.textContent(), expected);
  });
}

test("Enter presses while copying produce one clipboard write", async (t) => {
  const page = await open(t);
  if (!page) return;
  await run(page);
  await page.evaluate(() => {
    navigator.clipboard.writeText = (text) => {
      globalThis.copiedResults.push(text);
      return new Promise((resolve) => {
        globalThis.finishCopy = resolve;
      });
    };
  });
  const button = page.locator("[data-copy-results]");
  await button.focus();
  await page.keyboard.press("Enter");
  await page.keyboard.press("Enter");
  assert.equal(await page.evaluate(() => globalThis.copiedResults.length), 1);
  assert.equal(
    await page.locator("[data-copy-results-status]").textContent(),
    "Copying...",
  );
  await page.evaluate(() => globalThis.finishCopy());
  await copiedFeedback(page);
});

test("another results copy restarts the full success feedback interval", async (t) => {
  const page = await open(t);
  if (!page) return;
  await run(page);
  await pauseClock(page);
  const button = page.locator("[data-copy-results]");
  const status = page.locator("[data-copy-results-status]");
  await button.click();
  await copiedFeedback(page);
  await page.clock.fastForward(1000);
  await button.click();
  await page.waitForFunction(
    () =>
      globalThis.copiedResults.length === 2 &&
      document.querySelector("[data-copy-results-status]").textContent ===
        "Copied!",
  );
  await page.clock.fastForward(1100);
  assert.equal(await status.textContent(), "Copied!");
  await page.clock.fastForward(1000);
  assert.equal(await status.textContent(), "");
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
  await page.evaluate(() => {
    navigator.clipboard.writeText = async () => {
      throw new DOMException("Denied", "NotAllowedError");
    };
  });
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
  await page.evaluate(() => {
    navigator.clipboard.writeText = async (text) =>
      globalThis.copiedResults.push(text);
  });
  await button.click();
  await copiedFeedback(page);
  assert.equal(
    await page.evaluate(() => globalThis.copiedResults.at(-1)),
    text,
  );
  const missing = await open(t, { missingClipboard: true });
  await run(missing);
  await missing.locator("[data-copy-results]").click();
  await missing.waitForFunction(() =>
    document
      .querySelector("[data-copy-results-status]")
      .textContent.includes("Could not copy results"),
  );
  assert.notEqual(
    await missing.locator("[data-copy-results]").getAttribute("aria-disabled"),
    "true",
  );
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
  await holdWorkerRuns(page);
  await page.locator("#run-tests").click();
  assert.equal(await button.isVisible(), false);
});

test("a stale clipboard completion cannot announce newer results", async (t) => {
  for (const rejectCopy of [false, true]) {
    const page = await open(t);
    if (!page) return;
    await run(page);
    await page.evaluate((rejectCopy) => {
      globalThis.copyCalls = 0;
      navigator.clipboard.writeText = () => {
        globalThis.copyCalls++;
        return new Promise((resolve, reject) => {
          globalThis.finishCopy = () => {
            if (rejectCopy)
              reject(new DOMException("Denied", "NotAllowedError"));
            else resolve();
          };
        });
      };
    }, rejectCopy);
    const button = page.locator("[data-copy-results]");
    await button.click();
    assert.equal(
      await page.locator("[data-copy-results-status]").textContent(),
      "Copying...",
    );
    await button.evaluate((node) => node.click());
    assert.equal(await page.evaluate(() => globalThis.copyCalls), 1);
    await page.evaluate(() => {
      globalThis.finishOldCopy = globalThis.finishCopy;
    });
    await run(page);
    assert.equal(await button.isVisible(), true);
    assert.notEqual(await button.getAttribute("aria-disabled"), "true");
    await button.click();
    assert.equal(await page.evaluate(() => globalThis.copyCalls), 2);
    const status = page.locator("[data-copy-results-status]");
    assert.equal(await status.textContent(), "Copying...");
    await page.evaluate(async () => {
      globalThis.finishOldCopy();
      await Promise.resolve();
    });
    assert.equal(await status.textContent(), "Copying...");
    assert.equal(await button.getAttribute("aria-disabled"), "true");
    await button.evaluate((node) => node.click());
    assert.equal(await page.evaluate(() => globalThis.copyCalls), 2);
    await page.evaluate(() => globalThis.finishCopy());
    const expected = rejectCopy
      ? "Could not copy results. Select the results and copy them manually."
      : "Copied!";
    await page.waitForFunction(
      (expected) =>
        document.querySelector("[data-copy-results-status]").textContent ===
        expected,
      expected,
    );
    assert.equal(await button.getAttribute("aria-disabled"), null);
  }
});

test("invalidating a focused Copy results moves focus to Results", async (t) => {
  const page = await open(t);
  if (!page) return;
  await run(page);
  const button = page.locator("[data-copy-results]");
  await button.focus();
  await holdWorkerRuns(page);
  await page.locator("#run-tests").evaluate((node) => node.click());
  assert.equal(await button.isVisible(), false);
  assert.equal(
    await page
      .locator("#results-toggle")
      .evaluate((node) => node === document.activeElement),
    true,
  );
});

test("copy actions wrap with spacing and a pending copy stays visibly waiting", async (t) => {
  const page = await open(t);
  if (!page) return;
  await page.setViewportSize({ width: 390, height: 844 });
  await run(page);
  const actions = page.locator("[data-copy-results-actions]");
  const layout = await actions.evaluate((node) => {
    const style = getComputedStyle(node);
    return {
      display: style.display,
      wrap: style.flexWrap,
      gap: style.gap,
      margin: style.marginBottom,
    };
  });
  assert.deepEqual(layout, {
    display: "flex",
    wrap: "wrap",
    gap: "8px",
    margin: "8px",
  });
  await page.evaluate(() => {
    navigator.clipboard.writeText = () =>
      new Promise((resolve) => {
        globalThis.finishCopy = resolve;
      });
  });
  const button = page.locator("[data-copy-results]");
  const idle = await button.evaluate((node) => {
    const style = getComputedStyle(node);
    return {
      color: style.color,
      border: style.borderColor,
      background: style.backgroundColor,
    };
  });
  await button.click();
  await button.hover();
  const waiting = await button.evaluate((node) => {
    const style = getComputedStyle(node);
    return {
      opacity: style.opacity,
      cursor: style.cursor,
      color: style.color,
      border: style.borderColor,
      background: style.backgroundColor,
    };
  });
  assert.deepEqual(waiting, { opacity: "0.55", cursor: "progress", ...idle });
  await page.evaluate(() => globalThis.finishCopy());
});

test("Copy results accepts an injected clipboard and a page-independent focus target", async (t) => {
  const page = await open(t);
  if (!page) return;
  const result = await page.evaluate(async () => {
    const { mountResultsCopy } = await import("/render.js");
    const root = document.createElement("section");
    root.innerHTML =
      "<button data-fallback>Results</button><div data-copy-results-actions hidden><button data-copy-results>Copy results</button><span data-copy-results-status></span></div><div data-content>result &amp; diagnostic</div>";
    document.body.append(root);
    const writes = [];
    const fallback = root.querySelector("[data-fallback]");
    const button = root.querySelector("[data-copy-results]");
    const update = mountResultsCopy(
      root,
      root.querySelector("[data-content]"),
      fallback,
      {
        writeText: async (text) => writes.push(text),
      },
    );
    update(true);
    button.focus();
    button.click();
    await Promise.resolve();
    await Promise.resolve();
    const status = root.querySelector("[data-copy-results-status]").textContent;
    update(false);
    return { writes, status, focused: document.activeElement === fallback };
  });
  assert.deepEqual(result, {
    writes: ["result & diagnostic"],
    status: "Copied!",
    focused: true,
  });
});
