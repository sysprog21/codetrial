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
      const jump = page.getByRole("button", {
        name: "Jump to latest",
        exact: true,
        includeHidden: true,
      });
      assert.equal(await jump.count(), 1);
      assert.equal(await jump.isVisible(), false, "empty transcript");
      await addTurns(page, 1);
      assert.equal((await panelState(page)).overflows, false);
      assert.equal(await jump.isVisible(), false, "short transcript");

      await addTurns(page, 15);
      let state = await panelState(page);
      assert.ok(state.overflows, "the transcript is longer than the panel");
      assert.ok(state.atEnd, "a new turn is shown");
      assert.equal(state.pageY, pageY, "only the panel scrolls");
      const panelHeight = (
        await page.locator("#transcript-panel").boundingBox()
      ).height;

      await page.locator("#transcript-tab").focus();
      await page.keyboard.press("Tab");
      assert.equal(
        await page.evaluate(() => document.activeElement.id),
        "transcript-panel",
        "Tab enters the transcript before the return button",
      );
      await page.keyboard.press("PageUp");
      await page.waitForFunction(() => {
        const panel = document.querySelector("#transcript-panel");
        return panel.scrollTop + panel.clientHeight < panel.scrollHeight - 2;
      });
      assert.equal(await jump.isVisible(), true);
      await page.keyboard.press("Home");
      await page.waitForFunction(
        () => document.querySelector("#transcript-panel").scrollTop === 0,
      );
      await scrollPanel(page, 120);
      await addTurns(page, 2);
      state = await panelState(page);
      assert.ok(
        Math.abs(state.top - 120) <= 2,
        `rereading stops the follow, not at ${state.top}`,
      );
      assert.equal(await jump.isVisible(), true);

      await scrollPanel(page, "end");
      await addTurns(page, 2);
      assert.ok((await panelState(page)).atEnd, "the end resumes the follow");
      assert.equal(await jump.isVisible(), false);

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
      assert.equal(await jump.isVisible(), false);
      await addTurns(page, 2);
      await page.click("#transcript-tab");
      assert.ok((await panelState(page)).atEnd, "the tab opens at the end");
      assert.equal(await jump.isVisible(), false);

      // Not 0, which is also where a panel that lost its offset would show.
      await scrollPanel(page, 120);
      await page.click("#problem-tab");
      assert.equal(await jump.isVisible(), false);
      await addTurns(page, 2);
      await page.click("#transcript-tab");
      state = await panelState(page);
      assert.ok(
        Math.abs(state.top - 120) <= 2,
        `the tab opens where the candidate left it, not at ${state.top}`,
      );
      assert.equal(await jump.isVisible(), true);

      for (const key of ["Space", "Enter", "click"]) {
        const reducedMotion = key === "Enter";
        await page.emulateMedia({
          reducedMotion: reducedMotion ? "reduce" : "no-preference",
        });
        await scrollPanel(page, 120);
        assert.equal(await jump.isVisible(), true);
        await page.locator("#transcript-tab").focus();
        await page.keyboard.press("Tab");
        await page.keyboard.press("Tab");
        const layout = await jump.evaluate((button) => {
          const box = button.getBoundingClientRect();
          const panel = document
            .querySelector("#transcript-panel")
            .getBoundingClientRect();
          const view = button.parentElement.getBoundingClientRect();
          return {
            height: panel.height,
            inView:
              box.left >= 0 &&
              box.top >= Math.max(0, view.top) &&
              box.right <= innerWidth &&
              box.bottom <= Math.min(innerHeight, view.bottom),
            textAreaFillsView: panel.height === view.height,
            hit:
              document.elementFromPoint(
                box.x + box.width / 2,
                box.y + box.height / 2,
              ) === button,
            focused: document.activeElement === button,
          };
        });
        assert.deepEqual(layout, {
          height: panelHeight,
          inView: true,
          textAreaFillsView: true,
          hit: true,
          focused: true,
        });
        await jump.evaluate((button) => {
          button.addEventListener(
            "click",
            () => {
              const panel = document.querySelector("#transcript-panel");
              const trace = {
                from: panel.scrollTop,
                positions: [],
                heights: [panel.scrollHeight],
                complete: false,
              };
              globalThis.transcriptJumpTrace = trace;
              let settled = 0;
              function sample() {
                trace.positions.push(panel.scrollTop);
                trace.heights.push(panel.scrollHeight);
                if (button.hidden) settled++;
                if (settled < 3) requestAnimationFrame(sample);
                else trace.complete = true;
              }
              requestAnimationFrame(sample);
            },
            { once: true, capture: true },
          );
        });
        const pageY = (await panelState(page)).pageY;
        if (key === "click") await jump.click();
        else await page.keyboard.press(key);
        await page.waitForFunction(
          () => globalThis.transcriptJumpTrace.complete,
        );
        const state = await panelState(page);
        assert.ok(state.atEnd);
        assert.equal(state.pageY, pageY, "only the transcript scrolls");
        assert.equal(await jump.isVisible(), false);
        assert.equal(
          await page.evaluate(() => document.activeElement.id),
          "transcript-panel",
          "focus stays in the transcript when the button hides",
        );
        const trace = await page.evaluate(() => globalThis.transcriptJumpTrace);
        assert.equal(
          new Set(trace.heights).size,
          1,
          "hiding the button must not change the scroll range",
        );
        const positions = [trace.from, ...trace.positions, state.top];
        assert.ok(
          positions.every((top, i) => i === 0 || top >= positions[i - 1] - 2),
          `the downward return must not bounce back: ${positions.join(", ")}`,
        );
        assert.equal(
          trace.positions.some(
            (top) => top > trace.from && top < state.top - 2,
          ),
          !reducedMotion,
          reducedMotion
            ? "reduced motion returns immediately"
            : "scroll animates",
        );
        await addTurns(page, 1);
        assert.ok((await panelState(page)).atEnd);
        assert.equal(await jump.isVisible(), false);
      }
      await scrollPanel(page, 120);
      // Activate before the preceding scroll event is delivered. That event
      // belongs to the old reading position, not an interrupted return.
      await page.evaluate(() => {
        document.querySelector("#transcript-panel").scrollTop = 300;
        document.querySelector("#transcript-latest").click();
      });
      await page.waitForFunction(
        () => document.querySelector("#transcript-latest").hidden,
        null,
        { timeout: 2000 },
      );
      assert.ok(
        (await panelState(page)).atEnd,
        "a queued scroll cannot cancel return",
      );
      await addTurns(page, 1);
      assert.ok((await panelState(page)).atEnd, "the return resumes following");

      await scrollPanel(page, 120);
      await jump.click();
      await scrollPanel(page, 0);
      await addTurns(page, 1);
      assert.equal(
        (await panelState(page)).top,
        0,
        "a reader interrupts the return",
      );
      assert.equal(await jump.isVisible(), true);
      assert.deepEqual(errors, []);
    } finally {
      await page.close();
    }
  });
}
