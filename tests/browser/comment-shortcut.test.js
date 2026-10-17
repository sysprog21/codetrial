// Run with: node --test tests/browser/comment-shortcut.test.js
//
// A browser-level pin for the comment toggle in web/interview.js. What the
// toggle writes is covered in editor.test.js; what can break here is the
// wiring: the chord reaching it, the marker following the language tab rather
// than the language the page loaded with, and the edit landing where one undo
// takes it back.

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

/// The interview page with the editor reachable. Waits on the placeholder
/// bindEvents sets, because the textarea looks ready before any listener
/// exists and a press sent then would only show that nothing listened.
///
/// A window listener records whether the last slash had its default taken. It
/// runs after the editor's own, so it sees the answer.
async function interviewPage(t) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
    waitUntil: "domcontentloaded",
  });
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-input").placeholder !== "",
  );
  await page.evaluate(() => {
    document.querySelector("#audio-check").hidden = true;
    window.addEventListener("keydown", (event) => {
      if (event.key === "/")
        globalThis.slashDefaultPrevented = event.defaultPrevented;
    });
  });
  return { page, errors };
}

/// Replaces the buffer and selects `[start, end)`. One input event rather
/// than typing, so auto-indent and bracket pairing have no say in the fixture.
const setBuffer = (page, value, start, end = start) =>
  page.evaluate(
    ([value, start, end]) => {
      const editor = document.querySelector("#editor");
      editor.value = value;
      editor.dispatchEvent(new Event("input", { bubbles: true }));
      editor.focus();
      editor.setSelectionRange(start, end);
    },
    [value, start, end],
  );

const buffer = (page) => page.locator("#editor").inputValue();
const selection = (page) =>
  page.evaluate(() => {
    const editor = document.querySelector("#editor");
    return [editor.selectionStart, editor.selectionEnd];
  });
const slashDefaultPrevented = (page) =>
  page.evaluate(() => globalThis.slashDefaultPrevented);

test("Ctrl+/ and Cmd+/ toggle a comment on the selected lines", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    const code = "a = 1\nb = 2\nc = 3";
    for (const chord of ["Control+/", "Meta+/"]) {
      await setBuffer(page, code, 0, 11);
      await page.keyboard.press(chord);
      assert.equal(await buffer(page), "# a = 1\n# b = 2\nc = 3", chord);
      assert.equal(await slashDefaultPrevented(page), true, chord);
      assert.deepEqual(await selection(page), [2, 15], chord);
      await page.keyboard.press(chord);
      assert.equal(await buffer(page), code, chord);
      assert.deepEqual(await selection(page), [0, 11], chord);
    }
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

// The page loads on Python. A toggle that read the language once would go on
// writing `#` into every other tab.
test("the comment marker follows the language tab", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    for (const [language, marker] of [
      ["javascript", "//"],
      ["python", "#"],
    ]) {
      await page.locator("#language-select").selectOption(language);
      await setBuffer(page, "x", 0);
      await page.keyboard.press("Control+/");
      assert.equal(await buffer(page), `${marker} x`, language);
    }
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

// One toggle over several lines is one edit to take back, not one per line.
// Asked of the browser's own undo stack, which is where a toggle written by
// assigning the textarea's value would have left nothing at all.
test("one undo takes a toggled block back", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    const code = "a = 1\nb = 2\nc = 3";
    await setBuffer(page, code, 0, code.length);
    await page.keyboard.press("Control+/");
    assert.equal(await buffer(page), "# a = 1\n# b = 2\n# c = 3");
    await page.evaluate(() => document.execCommand("undo"));
    assert.equal(await buffer(page), code);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

// Holding the chord sends the keydown again with `repeat` set. Toggling on
// each one flips the block back and forth for as long as the key is down, so
// where it ends up depends on when the candidate lets go.
test("a held chord toggles once", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, "a = 1", 0);
    await page.keyboard.down("Control");
    await page.keyboard.down("/");
    // A second down with no up between is how Playwright sends a repeat.
    await page.keyboard.down("/");
    assert.equal(await slashDefaultPrevented(page), true);
    await page.keyboard.up("/");
    await page.keyboard.up("Control");
    assert.equal(await buffer(page), "# a = 1");
    await page.evaluate(() => document.execCommand("undo"));
    assert.equal(await buffer(page), "a = 1");
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a slash with no modifier is still typed", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, "a  b", 2);
    await page.keyboard.press("/");
    assert.equal(await buffer(page), "a / b");
    assert.equal(await slashDefaultPrevented(page), false);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});
