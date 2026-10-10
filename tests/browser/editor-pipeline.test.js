// A browser-level pin for the editor pipeline in web/editor.js and
// web/interview.js: bracket auto-close, and the overlay that paints its text.
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

/// A page on the interview screen with an empty editor, no room joined. The
/// editor is enabled as soon as bindEvents runs; nothing under test needs a
/// live session, so this never waits on one.
async function editorPage(t, configure = async () => {}) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await browser.newPage();
  await configure(page);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
    waitUntil: "domcontentloaded",
  });
  await page.waitForSelector("#editor:not([disabled])");
  // The media preflight overlay sits over the editor until a candidate (or a
  // stubbed device) clears it, and it blocks Playwright's own click on
  // whatever is under it. A keystroke only needs the element focused, not
  // clicked, so this reaches through the overlay the same way the existing
  // candidate-cases.test.js does for its own buttons -- .click()/.focus() in
  // page.evaluate bypasses the hit-test a real pointer click would fail.
  await page.evaluate(() => document.querySelector("#editor").focus());
  // The starter code carries its own brackets; clearing it keeps every
  // assertion below about exactly the characters this test types.
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.press("Delete");
  await page.waitForFunction(
    () => document.querySelector("#editor").value === "",
  );
  return { page, errors };
}

/// The editor's value and caret, the way lobby.test.js's snapshot(page) reads
/// the lobby: one shape, so the two tests that read it assert against the
/// same fields instead of two literals that could quietly drift apart.
const caretState = (page) =>
  page.evaluate(() => {
    const editor = document.querySelector("#editor");
    return {
      value: editor.value,
      start: editor.selectionStart,
      end: editor.selectionEnd,
    };
  });

test("typing an opening bracket auto-closes it, in a real browser", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await page.keyboard.type("(");
    assert.deepEqual(await caretState(page), { value: "()", start: 1, end: 1 });
    assert.deepEqual(
      errors,
      [],
      `typing "(" threw in the browser: ${errors[0]}`,
    );
  } finally {
    await page.close();
  }
});

test("an opener typed before existing text is inserted alone, in a real browser", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await page.keyboard.type("a+b)");
    await page.keyboard.press("Home");
    await page.keyboard.type("(");
    assert.deepEqual(await caretState(page), {
      value: "(a+b)",
      start: 1,
      end: 1,
    });
    assert.deepEqual(
      errors,
      [],
      `typing "(" before text threw in the browser: ${errors[0]}`,
    );
  } finally {
    await page.close();
  }
});

test("closing a pair by hand types over the closer instead of doubling it, in a real browser", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await page.keyboard.type("(");
    await page.keyboard.type("value");
    await page.keyboard.type(")");
    assert.deepEqual(await caretState(page), {
      value: "(value)",
      start: 7,
      end: 7,
    });
    assert.deepEqual(
      errors,
      [],
      `closing the pair threw in the browser: ${errors[0]}`,
    );
  } finally {
    await page.close();
  }
});

test("Backspace right after an auto-closed pair removes both characters, in a real browser", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    // deleteEmptyPair changes the value, so this exercises the other
    // execCommand round trip (execCommand("delete")), not just the
    // insertText one the test above covers.
    await page.keyboard.type("(");
    await page.keyboard.press("Backspace");
    assert.equal(await page.locator("#editor").inputValue(), "");
    assert.deepEqual(
      errors,
      [],
      `Backspace on an empty pair threw in the browser: ${errors[0]}`,
    );
  } finally {
    await page.close();
  }
});

test("the highlight overlay resolves the textarea's font metrics, in a real browser", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    // The overlay paints the glyphs the textarea positions. Any metric of the
    // shared #editor, #editor-highlight rule that <code> resolves on its own
    // moves the painted text away from the caret, a little more every line.
    const [editor, overlay] = await page.evaluate(() =>
      ["#editor", "#editor-highlight code"].map((selector) => {
        const style = getComputedStyle(document.querySelector(selector));
        return {
          fontFamily: style.fontFamily,
          fontSize: style.fontSize,
          lineHeight: style.lineHeight,
          fontWeight: style.fontWeight,
          letterSpacing: style.letterSpacing,
        };
      }),
    );
    assert.deepEqual(overlay, editor);
  } finally {
    await page.close();
  }
});

test("matching brackets follow the caret without editing or losing undo", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  const marks = page.locator("#editor-highlight .matching-bracket");
  try {
    await page.keyboard.type("(");
    await marks.first().waitFor();
    assert.deepEqual(await marks.allTextContents(), ["(", ")"]);
    await page.keyboard.insertText("value");
    await page.keyboard.press("Home");
    await page.waitForFunction(
      () => document.querySelectorAll(".matching-bracket").length === 2,
    );
    assert.deepEqual(await caretState(page), {
      value: "(value)",
      start: 0,
      end: 0,
    });
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("ArrowRight");
    await page.waitForFunction(
      () => document.querySelectorAll(".matching-bracket").length === 0,
    );
    assert.deepEqual(await caretState(page), {
      value: "(value)",
      start: 2,
      end: 2,
    });
    await page.evaluate(() =>
      document.querySelector("#editor").setSelectionRange(0, 7),
    );
    await page.waitForFunction(
      () => document.querySelectorAll(".matching-bracket").length === 0,
    );
    await page.keyboard.press("ControlOrMeta+Z");
    assert.equal((await caretState(page)).value, "()");
    await page.evaluate(() =>
      document.querySelector("#editor").setSelectionRange(0, 0),
    );
    await page.waitForFunction(
      () => document.querySelectorAll(".matching-bracket").length === 2,
    );
    await page.evaluate(() => document.querySelector("#editor").blur());
    await page.waitForFunction(
      () => document.querySelectorAll(".matching-bracket").length === 0,
    );
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("nested matching ignores quoted brackets and tracks programmatic selection", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  const code = '{ call("}", items[0]); /* ] */ }';
  try {
    await page.evaluate(() => {
      document.querySelector('[data-language="javascript"]').click();
      document.querySelector("#editor").focus();
    });
    await page.keyboard.press("ControlOrMeta+A");
    await page.keyboard.press("Delete");
    await page.keyboard.insertText(code);
    await page.evaluate(() =>
      document.querySelector("#editor").setSelectionRange(0, 0),
    );
    await page.waitForFunction(
      () => document.querySelectorAll(".matching-bracket").length === 2,
    );
    assert.deepEqual(
      await page.locator(".matching-bracket").allTextContents(),
      ["{", "}"],
    );
    await page.evaluate(() => {
      const editor = document.querySelector("#editor");
      const position = editor.value.indexOf("[");
      editor.setSelectionRange(position, position);
    });
    await page.waitForFunction(
      () => document.querySelector(".matching-bracket")?.textContent === "[",
    );
    assert.deepEqual(
      await page.locator(".matching-bracket").allTextContents(),
      ["[", "]"],
    );
    await page.evaluate(() =>
      document.querySelector("#editor").setSelectionRange(8, 8),
    );
    await page.waitForFunction(
      () => document.querySelectorAll(".matching-bracket").length === 0,
    );
    assert.equal((await caretState(page)).value, code);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("blur clears matching brackets before activeElement changes", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await page.keyboard.type("(");
    await page.locator(".matching-bracket").first().waitFor();
    const state = await page.evaluate(() => {
      const editor = document.querySelector("#editor");
      // Reproduce browsers that dispatch blur before updating activeElement.
      editor.dispatchEvent(new FocusEvent("blur"));
      return {
        stillActive: document.activeElement === editor,
        marks: document.querySelectorAll(".matching-bracket").length,
      };
    });
    assert.deepEqual(state, { stillActive: true, marks: 0 });
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("caret and focus changes reuse ranges until code or language changes", async (t) => {
  const ctx = await editorPage(t, async (page) => {
    await page.route("**/tokenizer.js", async (route) => {
      const response = await route.fetch();
      const source = await response.text();
      const declaration =
        "export function tokenize(code, language, parse = parseSyntax) {";
      assert.ok(source.includes(declaration));
      await route.fulfill({
        response,
        body: source.replace(
          declaration,
          declaration +
            "\n(window.tokenizations ??= []).push({code, language});",
        ),
      });
    });
  });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await page.evaluate(async () => {
      document.querySelector('[data-language="javascript"]').click();
      await (await import("/syntax-parser.js")).prepareLanguage("javascript");
      document.querySelector("#editor").focus();
    });
    await page.keyboard.press("ControlOrMeta+A");
    await page.keyboard.insertText("(value)");
    await page.locator(".matching-bracket").first().waitFor();
    await page.evaluate(() => {
      window.overlayPaints = 0;
      new MutationObserver((records) => {
        window.overlayPaints += records.length;
      }).observe(document.querySelector("#editor-highlight code"), {
        childList: true,
      });
    });
    await page.keyboard.press("Home");
    await page.keyboard.press("End");
    await page.evaluate(() => new Promise(requestAnimationFrame));
    assert.equal(await page.evaluate(() => window.overlayPaints), 0);
    await page.evaluate(() => {
      const editor = document.querySelector("#editor");
      editor.blur();
      editor.focus();
    });
    await page.keyboard.press("ControlOrMeta+A");
    await page.evaluate(() => {
      window.overlayPaints = 0;
    });
    await page.keyboard.insertText("(other)");
    await page.locator(".matching-bracket").first().waitFor();
    await page.evaluate(() => new Promise(requestAnimationFrame));
    assert.equal(await page.evaluate(() => window.overlayPaints), 1);
    await page.evaluate(async () => {
      document.querySelector('[data-language="python"]').click();
      await (await import("/syntax-parser.js")).prepareLanguage("python");
      document.querySelector("#editor").focus();
    });
    await page.keyboard.press("ControlOrMeta+A");
    await page.keyboard.insertText("(other)");
    await page.locator(".matching-bracket").first().waitFor();
    assert.deepEqual(
      await page.evaluate(() =>
        window.tokenizations.filter(({ code }) =>
          ["(value)", "(other)"].includes(code),
        ),
      ),
      [
        { code: "(value)", language: "javascript" },
        { code: "(other)", language: "javascript" },
        { code: "(other)", language: "python" },
      ],
    );
    const burst = await page.evaluate(async () => {
      const editor = document.querySelector("#editor");
      window.tokenizations = [];
      window.overlayPaints = 0;
      for (const code of ["(first)", "(second)", "(latest)"]) {
        editor.value = code;
        editor.setSelectionRange(code.length - 1, code.length - 1);
        editor.dispatchEvent(new Event("input", { bubbles: true }));
        document.dispatchEvent(new Event("selectionchange"));
      }
      const synchronousParses = window.tokenizations.length;
      await new Promise(requestAnimationFrame);
      return {
        synchronousParses,
        tokenizations: window.tokenizations,
        paints: window.overlayPaints,
        text: document.querySelector("#editor-highlight code").textContent,
        marks: [...document.querySelectorAll(".matching-bracket")].map(
          (node) => node.textContent,
        ),
        value: editor.value,
        caret: editor.selectionStart,
      };
    });
    assert.deepEqual(burst, {
      synchronousParses: 0,
      tokenizations: [{ code: "(latest)", language: "python" }],
      paints: 1,
      text: "(latest)",
      marks: ["(", ")"],
      value: "(latest)",
      caret: 7,
    });
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a loaded grammar replaces cached fallback ranges without an edit", async (t) => {
  let releaseGrammar;
  const grammarReady = new Promise((resolve) => {
    releaseGrammar = resolve;
  });
  const ctx = await editorPage(t, async (page) => {
    await page.route("**/tree-sitter-javascript.wasm", async (route) => {
      await grammarReady;
      await route.continue();
    });
  });
  if (!ctx) return;
  const { page, errors } = ctx;
  const code = "function f() {} /[()]/.test(value);";
  try {
    await page.evaluate(() => {
      document.querySelector('[data-language="javascript"]').click();
      document.querySelector("#editor").focus();
    });
    await page.keyboard.press("ControlOrMeta+A");
    await page.keyboard.insertText(code);
    await page.evaluate(() => {
      const editor = document.querySelector("#editor");
      const position = editor.value.indexOf("[()") + 1;
      editor.setSelectionRange(position, position);
    });
    // The fallback treats the slash after a declaration as division.
    await page.locator(".matching-bracket").first().waitFor();
    releaseGrammar();
    await page.waitForFunction(
      () => document.querySelectorAll(".matching-bracket").length === 0,
    );
    assert.equal((await caretState(page)).value, code);
    assert.deepEqual(errors, []);
  } finally {
    releaseGrammar();
    await page.close();
  }
});

test("Jim captions survive editor navigation and language switches", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  const editor = page.locator("#editor");
  const bar = page.locator("#captions-bar");
  const code = "const answer = 42;";
  const text = "Use a hash map and count each value.";

  const assertJim = async (message) => {
    await page.evaluate(
      () => new Promise((resolve) => requestAnimationFrame(resolve)),
    );
    assert.equal(await bar.isVisible(), true, message);
    assert.equal(
      await page.locator("#captions-text").textContent(),
      `[Jim]: ${text}`,
      message,
    );
  };

  try {
    await page.evaluate(async () => {
      await import("/interview.js");
      document.querySelector("#audio-check").hidden = true;
      (await import("/avatar/stage.js")).setStageCovered(false);
    });

    await page.locator('[data-language="javascript"]').click();
    await editor.fill(code);
    await page.locator("#problem-tab").click();
    assert.equal(
      await editor.evaluate((node) => document.activeElement === node),
      false,
      "setup must move focus away from the editor",
    );

    await page.evaluate(async (captionText) => {
      const captions = await import("/captions.js");
      captions.updateCaptions(
        "interviewer",
        captionText,
        "jim-editor-navigation",
        { final: true },
      );
    }, text);
    await assertJim("a completed Jim caption must start visible");

    await editor.focus();
    assert.equal(
      await editor.evaluate((node) => document.activeElement === node),
      true,
    );
    await assertJim("focusing the editor must preserve Jim");

    await editor.click({ position: { x: 8, y: 8 } });
    await assertJim("clicking the editor must preserve Jim");

    await page.evaluate(() => {
      document.querySelector("#editor").setSelectionRange(0, 0);
    });
    await page.keyboard.press("ArrowRight");
    assert.deepEqual(await caretState(page), {
      value: code,
      start: 1,
      end: 1,
    });
    await assertJim("moving the caret must preserve Jim");

    const python = page.locator('[data-language="python"]');
    await python.click();
    assert.equal(
      await python.evaluate((button) => button.classList.contains("selected")),
      true,
      "the language switch must run",
    );
    assert.notEqual(
      await editor.inputValue(),
      code,
      "the language switch must restore a different buffer",
    );
    await assertJim("switching languages must preserve Jim");

    await page.locator('[data-language="javascript"]').click();
    assert.equal(await editor.inputValue(), code);
    await assertJim("returning to the saved buffer must preserve Jim");

    await editor.focus();
    await assertJim("refocusing after a switch must preserve Jim");

    await editor.fill(`${code} `);
    assert.equal(await editor.inputValue(), `${code} `);
    assert.equal(
      await bar.isVisible(),
      false,
      "changing the code text must dismiss Jim",
    );
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});
