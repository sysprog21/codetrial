// Run with: node --test tests/browser/whiteboard-tool-shortcuts.test.js
// Bare P / E / T on the whiteboard: switch, cycle, restore from text, and ignore.

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
  await new Promise((done) => (server ? server.close(done) : done()));
});

async function openBoard(page) {
  await page.goto(
    `${base}/interview.html?problem=chargeback-pair-match&mode=whiteboard`,
    { waitUntil: "domcontentloaded" },
  );
  await page.locator("#board-panel").waitFor({ state: "visible" });
  await page.evaluate(() => {
    document.querySelector("#audio-check").hidden = true;
  });
  await page.locator("#board").focus();
}

function pressed(page) {
  return page
    .getByRole("toolbar", { name: "Whiteboard tools" })
    .getByRole("button", { pressed: true })
    .evaluateAll((buttons) =>
      buttons.map((b) => b.getAttribute("aria-label") ?? b.textContent.trim()),
    );
}

function pressToolKey(page, key, extras = {}) {
  return page.evaluate(
    ({ key, extras }) => {
      document.activeElement.dispatchEvent(
        new KeyboardEvent("keydown", {
          key,
          code: `Key${key.toUpperCase()}`,
          bubbles: true,
          cancelable: true,
          ...extras,
        }),
      );
    },
    { key, extras },
  );
}

test("P E T switch, cycle, restore, and ignore unsafe cases", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await openBoard(page);
    assert.deepEqual(
      await page
        .locator(".board-toolbar .board-key")
        .evaluateAll((nodes) => nodes.map((n) => n.textContent.trim())),
      ["P", "E", "T"],
    );
    assert.deepEqual(await pressed(page), ["Black pen"]);
    await pressToolKey(page, "p");
    assert.deepEqual(await pressed(page), ["Red pen"]);
    await pressToolKey(page, "e");
    assert.deepEqual(await pressed(page), ["Eraser", "Medium eraser"]);
    await pressToolKey(page, "e");
    assert.deepEqual(await pressed(page), ["Eraser", "Large eraser"]);
    await pressToolKey(page, "t");
    assert.deepEqual(await pressed(page), ["Text"]);
    await pressToolKey(page, "t");
    assert.deepEqual(await pressed(page), ["Eraser", "Large eraser"]);

    await pressToolKey(page, "p", { repeat: true });
    assert.deepEqual(await pressed(page), ["Eraser", "Large eraser"]);
    await page.locator("#problem-tab").focus();
    await pressToolKey(page, "p");
    assert.deepEqual(await pressed(page), ["Eraser", "Large eraser"]);
    await page.evaluate(() => {
      const field = document.createElement("input");
      field.id = "board-shortcut-trap";
      document.querySelector(".board-toolbar").append(field);
      field.focus();
    });
    await pressToolKey(page, "p");
    assert.deepEqual(await pressed(page), ["Eraser", "Large eraser"]);
    await page.evaluate(() => {
      document.querySelector("#board-shortcut-trap").remove();
    });
    await page.locator("#board").focus();
    await pressToolKey(page, "p");
    assert.deepEqual(await pressed(page), ["Red pen"]);
  } finally {
    await page.close();
  }
});
