// Run with: node --test tests/browser/whiteboard-toolbar.test.js
//
// The board toolbar as assistive technology reads it. A pen swatch is a dot of
// color with no text, so its name and its pressed state are all a screen
// reader or Voice Control has to go on. Both are read through getByRole, which
// resolves the accessible name and state the way the browser exposes them,
// rather than through the attributes interview.js happens to set.

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

const PENS = ["Black pen", "Red pen", "Blue pen", "Green pen"];

/// Which toolbar buttons report themselves pressed, by accessible name.
const pressed = (page) =>
  page
    .getByRole("toolbar", { name: "Whiteboard tools" })
    .getByRole("button", { pressed: true })
    .evaluateAll((buttons) =>
      buttons.map((b) => b.getAttribute("aria-label") ?? b.textContent.trim()),
    );

/// Which pens are drawn with the selection ring. The ring is styled from
/// aria-pressed, so a stylesheet still keyed to the old class would leave the
/// state announced and nothing drawn.
const ringed = (page) =>
  page
    .locator("#board-pens button")
    .evaluateAll((pens) =>
      pens
        .filter((pen) => getComputedStyle(pen).boxShadow !== "none")
        .map((pen) => pen.getAttribute("aria-label")),
    );

test("each pen is named by its color and says whether it is the one in hand", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(
      `${base}/interview.html?problem=chargeback-pair-match&mode=whiteboard`,
      { waitUntil: "domcontentloaded" },
    );
    await page.locator("#board-panel").waitFor({ state: "visible" });
    await page.evaluate(() => {
      document.querySelector("#audio-check").hidden = true;
    });

    for (const name of PENS) {
      const pen = page.getByRole("button", { name, exact: true });
      assert.equal(await pen.count(), 1, name);
      // The hover tip is the name for a sighted candidate who cannot tell the
      // red swatch from the green one.
      assert.equal(await pen.getAttribute("title"), `${name} (P)`);
      assert.equal(await pen.getAttribute("aria-keyshortcuts"), "P");
    }
    assert.deepEqual(await pressed(page), ["Black pen"]);
    assert.deepEqual(await ringed(page), ["Black pen"]);

    await page.getByRole("button", { name: "Red pen", exact: true }).click();
    assert.deepEqual(await pressed(page), ["Red pen"]);
    assert.deepEqual(await ringed(page), ["Red pen"]);

    // The eraser takes the board from every pen, so none of them may still
    // claim to be in hand while it is; the size it erases at is pressed with it.
    await page.getByRole("button", { name: "Eraser", exact: true }).click();
    assert.deepEqual(await pressed(page), ["Eraser", "Medium eraser"]);
    assert.deepEqual(await ringed(page), []);
  } finally {
    await page.close();
  }
});
