// A browser-level pin for the Transcript tab's follow in web/render.js: the
// real panel, stylesheet and tab buttons, at a desktop and a narrow viewport.
import { after, before, test } from "node:test";
import assert from "node:assert/strict";

import {
  closeSetupCheck,
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

// Adds turns through the page's own transcript: with no room joined, each
// run of the tests writes two rows ("I ran the tests." and Jim's answer).
// Two frames then let the scroll events the rows queued run.
async function addTurns(page, runs) {
  for (let run = 0; run < runs; run++) {
    const rows = await page.locator(".transcript-row").count();
    await page.click("#run-tests");
    await page.waitForFunction(
      (rows) => document.querySelectorAll(".transcript-row").length >= rows + 2,
      rows,
    );
  }
  await page.evaluate(async () => {
    for (let frame = 0; frame < 2; frame++)
      await new Promise(requestAnimationFrame);
  });
}

// Read after two frames, once the resize callbacks a change queued have run.
const panelState = (page) =>
  page.evaluate(async () => {
    for (let frame = 0; frame < 2; frame++)
      await new Promise(requestAnimationFrame);
    const panel = document.querySelector("#transcript-panel");
    return {
      top: panel.scrollTop,
      overflows: panel.scrollHeight > panel.clientHeight,
      atEnd: panel.scrollTop + panel.clientHeight >= panel.scrollHeight - 2,
      pageY: window.scrollY,
    };
  });

// The row at the panel's top edge, and how far above that edge a row starts.
const topRow = (page) =>
  page.evaluate(() => {
    const panel = document.querySelector("#transcript-panel");
    const edge = panel.getBoundingClientRect().top;
    return [...panel.querySelectorAll(".transcript-row")].findIndex(
      (row) => row.getBoundingClientRect().bottom > edge,
    );
  });
const rowOffset = (page, index) =>
  page.evaluate(async (index) => {
    for (let frame = 0; frame < 2; frame++)
      await new Promise(requestAnimationFrame);
    const panel = document.querySelector("#transcript-panel");
    const row = panel.querySelectorAll(".transcript-row")[index];
    return row.getBoundingClientRect().top - panel.getBoundingClientRect().top;
  }, index);

const scrollPanel = (page, top) =>
  page.evaluate(async (top) => {
    const panel = document.querySelector("#transcript-panel");
    panel.scrollTop = top === "end" ? panel.scrollHeight : top;
    for (let frame = 0; frame < 2; frame++)
      await new Promise(requestAnimationFrame);
  }, top);

for (const [width, height] of [
  [1280, 720],
  [900, 700],
]) {
  test(`the Transcript tab follows new turns at ${width}x${height}`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await browser.newPage({ viewport: { width, height } });
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    try {
      const query = new URLSearchParams({ problem: "chargeback-pair-match" });
      await page.goto(`${base}/interview.html?${query}`, {
        waitUntil: "domcontentloaded",
      });
      await page.waitForFunction(
        () => document.querySelector("#round-plan-summary").textContent !== "",
      );
      await closeSetupCheck(page);
      await page.locator("#language-select").selectOption("javascript");
      // A page taller than the window, so a follow that scrolled the page
      // instead of the panel would move window.scrollY.
      const pageY = await page.evaluate(() => {
        const spacer = document.createElement("div");
        spacer.style.height = "200vh";
        document.body.append(spacer);
        return window.scrollY;
      });
      await page.click("#transcript-tab");
      const empty = await panelState(page);
      assert.equal(empty.top, 0, "the empty state keeps its heading in view");

      await addTurns(page, 15);
      let state = await panelState(page);
      assert.ok(state.overflows, "the transcript is longer than the panel");
      assert.ok(state.atEnd, "a new turn is shown");
      assert.equal(state.pageY, pageY, "only the panel scrolls");

      await scrollPanel(page, 120);
      await addTurns(page, 2);
      state = await panelState(page);
      assert.ok(
        Math.abs(state.top - 120) <= 2,
        `rereading stops the follow, not at ${state.top}`,
      );

      await scrollPanel(page, "end");
      await addTurns(page, 2);
      assert.ok((await panelState(page)).atEnd, "the end resumes the follow");

      // Rows rewrap when the window crosses the 1024px breakpoint; the
      // browser moving the panel then is not the candidate scrolling.
      const across = width > 1024 ? 1000 : 1100;
      await page.setViewportSize({ width: across, height });
      assert.ok((await panelState(page)).atEnd, "a resize keeps the end");
      await addTurns(page, 2);
      assert.ok((await panelState(page)).atEnd, "a resize keeps the follow");
      await page.setViewportSize({ width, height });
      assert.ok((await panelState(page)).atEnd, "resizing back keeps the end");

      // Paused, the browser's anchoring is what keeps the line being reread
      // where it was while the rows around it rewrap.
      await scrollPanel(page, 300);
      const row = await topRow(page);
      const before = await rowOffset(page, row);
      await page.setViewportSize({ width: across, height });
      const after = await rowOffset(page, row);
      assert.ok(
        Math.abs(after - before) <= 2,
        `the reread line moved from ${before} to ${after}`,
      );
      await page.setViewportSize({ width, height });
      await scrollPanel(page, "end");

      await page.click("#problem-tab");
      await addTurns(page, 2);
      await page.click("#transcript-tab");
      assert.ok((await panelState(page)).atEnd, "the tab opens at the end");

      // Not 0, which is also where a panel that lost its offset would show.
      await scrollPanel(page, 120);
      await page.click("#problem-tab");
      await addTurns(page, 2);
      await page.click("#transcript-tab");
      state = await panelState(page);
      assert.ok(
        Math.abs(state.top - 120) <= 2,
        `the tab opens where the candidate left it, not at ${state.top}`,
      );
      assert.deepEqual(errors, []);
    } finally {
      await page.close();
    }
  });
}
