// Run with: node --test tests/browser/editor-font-size.test.js
//
// The code font size is a candidate's choice kept between visits. What can
// break is the agreement between the textarea, the highlight overlay and the
// gutter: one layer at a different size or offset and the caret drifts away
// from the glyphs it edits, so every offered size is checked in a real browser.

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

/// Out from behind the media preflight, which covers the toolbar until a device
/// check clears it. Hidden rather than reached through with a scripted click,
/// so the clicks below still have to land on the buttons a candidate sees.
/// Waits on the placeholder `init()` sets, not on static markup, so the stored
/// size has been applied and the buttons have their handlers.
async function openEditor(page) {
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-input").placeholder !== "",
  );
  await page.evaluate(() => {
    document.querySelector("#audio-check").hidden = true;
  });
}

/// Four-digit line numbers, which the gutter is tightest for at the smallest
/// size. Refilled after every reload, because the buffer is not kept. Set with
/// one input event rather than `page.fill`, which takes half a minute to type
/// this many lines.
const fillLongBuffer = (page) =>
  page.evaluate(() => {
    const editor = document.querySelector("#editor");
    editor.value = Array.from({ length: 1200 }, (_, i) => `x = ${i}`).join(
      "\n",
    );
    editor.dispatchEvent(new Event("input", { bubbles: true }));
  });

const editorMetrics = (page) =>
  page.evaluate(() => {
    const layers = ["#editor", "#editor-highlight", "#editor-lines"].map((id) =>
      document.querySelector(id),
    );
    const [editor, , gutter] = layers;
    return {
      sizes: layers.map((node) => getComputedStyle(node).fontSize),
      lineHeights: layers.map((node) => getComputedStyle(node).lineHeight),
      gutterRight: gutter.getBoundingClientRect().right,
      editorLeft: editor.getBoundingClientRect().left,
      highlightLeft: document
        .querySelector("#editor-highlight")
        .getBoundingClientRect().left,
      gutterOverflows: gutter.scrollWidth > gutter.clientWidth,
      smallerDisabled:
        document
          .querySelector("#editor-font-smaller")
          .getAttribute("aria-disabled") === "true",
      largerDisabled:
        document
          .querySelector("#editor-font-larger")
          .getAttribute("aria-disabled") === "true",
      focused: document.activeElement?.id ?? "",
      stored: localStorage.getItem("codetrial:editorFontSize"),
    };
  });

function assertLayersAgree(metrics, label) {
  assert.equal(new Set(metrics.sizes).size, 1, `${label}: ${metrics.sizes}`);
  assert.equal(
    new Set(metrics.lineHeights).size,
    1,
    `${label}: ${metrics.lineHeights}`,
  );
  assert.equal(metrics.editorLeft, metrics.highlightLeft, label);
  assert.equal(metrics.editorLeft, metrics.gutterRight, label);
  assert.equal(metrics.gutterOverflows, false, `${label}: gutter overflows`);
}

/// Clicks once per expected size rather than until the button disables, so a
/// button that stops stepping or never disables fails here instead of
/// clicking until CI times out.
async function stepThrough(page, direction, sizes) {
  const disabled = `${direction}Disabled`;
  for (const [index, size] of sizes.entries()) {
    await page.click(`#editor-font-${direction}`);
    const metrics = await editorMetrics(page);
    assert.equal(metrics.sizes[0], size, `${direction} to ${size}`);
    assertLayersAgree(metrics, `${direction} ${size}`);
    assert.equal(
      metrics[disabled],
      index === sizes.length - 1,
      `${direction} at ${size}: disabled only at the end of the ladder`,
    );
    assert.equal(
      metrics.focused,
      `editor-font-${direction}`,
      `${direction} at ${size}: focus stays on the button`,
    );
  }
}

test("every offered code font size keeps the editor layers in line, and is remembered", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html`, {
      waitUntil: "domcontentloaded",
    });
    await openEditor(page);
    await fillLongBuffer(page);

    const initial = await editorMetrics(page);
    assert.equal(initial.sizes[0], "14px", "0.875rem unless changed");
    assert.equal(initial.smallerDisabled, false);
    assert.equal(initial.largerDisabled, false);
    assertLayersAgree(initial, "default");

    await stepThrough(page, "larger", ["16px", "18px", "20px", "24px"]);

    await page.reload({ waitUntil: "domcontentloaded" });
    await openEditor(page);
    await fillLongBuffer(page);
    const returning = await editorMetrics(page);
    assert.equal(returning.sizes[0], "24px", "the choice is remembered");
    assert.equal(returning.largerDisabled, true);

    await stepThrough(page, "smaller", [
      "20px",
      "18px",
      "16px",
      "14px",
      "12px",
    ]);

    await page.click("#editor-font-larger");
    const back = await editorMetrics(page);
    assert.equal(back.sizes[0], "14px");
    assert.equal(back.stored, null, "the default is stored as no preference");
  } finally {
    await page.close();
  }
});

test("a stored size the page does not offer falls back to the default", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.addInitScript(() =>
      localStorage.setItem("codetrial:editorFontSize", "9"),
    );
    await page.goto(`${base}/interview.html`, {
      waitUntil: "domcontentloaded",
    });
    await openEditor(page);
    const metrics = await editorMetrics(page);
    assert.equal(metrics.sizes[0], "14px");
    assertLayersAgree(metrics, "fallback");
  } finally {
    await page.close();
  }
});

// The repaint waits for the next frame and a scroll event does not, so Enter
// at the bottom scrolls the textarea while the overlay still holds the old
// text: its scrollTop clamps to the old height, and every row would sit one
// above its code until something scrolled again. Headless Chromium does not
// scroll a textarea to its caret, so the scroll a real browser makes is made
// here, between the input and the frame that paints it.
test("a scroll before the repaint leaves the overlay on its rows", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html`, {
      waitUntil: "domcontentloaded",
    });
    await openEditor(page);
    await fillLongBuffer(page);
    for (let line = 0; line < 3; line += 1) {
      const scroll = await page.evaluate(
        () =>
          new Promise((settled) => {
            const editor = document.querySelector("#editor");
            editor.value += "\n";
            editor.dispatchEvent(new Event("input", { bubbles: true }));
            editor.scrollTop = editor.scrollHeight;
            requestAnimationFrame(() =>
              requestAnimationFrame(() =>
                settled(
                  ["#editor", "#editor-highlight", "#editor-lines"].map(
                    (id) => document.querySelector(id).scrollTop,
                  ),
                ),
              ),
            );
          }),
      );
      assert.deepEqual(scroll.slice(1), [scroll[0], scroll[0]], `line ${line}`);
    }
  } finally {
    await page.close();
  }
});

test("the active line highlight tracks the caret and clears on blur or selection", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html`, {
      waitUntil: "domcontentloaded",
    });
    await openEditor(page);
    await fillLongBuffer(page);

    const activeLine = () =>
      page.evaluate(() =>
        document
          .querySelector("#editor")
          .parentElement.style.getPropertyValue("--active-line"),
      );

    const expectActiveLine = async (expected, message) => {
      await page.waitForFunction(
        (exp) =>
          document
            .querySelector("#editor")
            .parentElement.style.getPropertyValue("--active-line") === exp,
        expected,
      );

      assert.equal(await activeLine(), expected, message);
    };

    await page.focus("#editor");
    await page.evaluate(() => {
      const editor = document.querySelector("#editor");
      editor.setSelectionRange(0, 0);
      document.dispatchEvent(new Event("selectionchange"));
    });
    await expectActiveLine("0", "highlight initializes at line 0");

    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await expectActiveLine(
      "2",
      "highlight tracks a collapsed caret moved by the keyboard",
    );

    await page.evaluate(() => {
      document.querySelector("#editor").scrollBy(0, 200);
    });
    await expectActiveLine(
      "2",
      "highlight stays anchored to its row when scrolled natively",
    );

    await page.keyboard.down("Shift");
    await page.keyboard.press("ArrowRight");
    await page.keyboard.up("Shift");
    await expectActiveLine("", "highlight clears on a range selection");

    await page.keyboard.press("ArrowRight");
    await expectActiveLine("2", "highlight restores when selection collapses");

    await page.evaluate(() => {
      document.querySelector("#editor").blur();
      document.dispatchEvent(new Event("selectionchange"));
    });
    await expectActiveLine("", "highlight clears when the editor loses focus");
  } finally {
    await page.close();
  }
});
