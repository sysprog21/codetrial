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
/// pause control is revealed by bindEvents after the starter buffer is seeded.
/// The textarea starts enabled in HTML, so it cannot signal readiness. Nothing
/// under test needs a live session, so this never waits on one.
async function editorPage(t, { params = {}, beforeLoad } = {}) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await browser.newPage();
  const errors = [];
  const parserRequests = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.startsWith("/vendor/tree-sitter"))
      parserRequests.push(request.url());
  });
  await beforeLoad?.(page);
  const query = new URLSearchParams({
    problem: "chargeback-pair-match",
    ...params,
  });
  await page.goto(`${base}/interview.html?${query}`, {
    waitUntil: "domcontentloaded",
  });
  await page.waitForSelector("#pause:not([hidden])");
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
  return { page, errors, parserRequests };
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

const setCode = (page, value, start = value.length, end = start) =>
  page.evaluate(
    async ({ value, start, end }) => {
      const editor = document.querySelector("#editor");
      editor.value = value;
      editor.setSelectionRange(start, end);
      editor.dispatchEvent(new Event("input", { bubbles: true }));
      editor.focus();
      await new Promise(requestAnimationFrame);
    },
    { value, start, end },
  );

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
    await page.locator("#language-select").selectOption("javascript");
    await page.evaluate(() => {
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
  const ctx = await editorPage(t, {
    beforeLoad: async (page) => {
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
    },
  });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await page.locator("#language-select").selectOption("javascript");
    await page.evaluate(async () => {
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
    // Selecting all clears the bracket marks in a queued paint. Count the
    // replacement only after that paint, whatever frame the keypress reached.
    await page
      .locator(".matching-bracket")
      .first()
      .waitFor({ state: "detached" });
    await page.evaluate(() => {
      window.overlayPaints = 0;
    });
    await page.keyboard.insertText("(other)");
    await page.locator(".matching-bracket").first().waitFor();
    await page.evaluate(() => new Promise(requestAnimationFrame));
    assert.equal(await page.evaluate(() => window.overlayPaints), 1);
    await page.locator("#language-select").selectOption("python");
    await page.evaluate(async () => {
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
  const ctx = await editorPage(t, {
    beforeLoad: async (page) => {
      await page.route("**/tree-sitter-javascript.wasm", async (route) => {
        await grammarReady;
        await route.continue();
      });
    },
  });
  if (!ctx) return;
  const { page, errors } = ctx;
  const code = "class A {} /[()]/.test(value);";
  try {
    await page.locator("#language-select").selectOption("javascript");
    await page.evaluate(() => {
      document.querySelector("#editor").focus();
    });
    await page.keyboard.press("ControlOrMeta+A");
    await page.keyboard.insertText(code);
    await page.evaluate(() => {
      const editor = document.querySelector("#editor");
      const position = editor.value.indexOf("[()") + 1;
      editor.setSelectionRange(position, position);
    });
    // The fallback treats the slash after a class body as division.
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

test("a parse cut off by its time budget is retried without an edit", async (t) => {
  const ctx = await editorPage(t, {
    beforeLoad: async (page) => {
      await page.route("**/syntax-parser.js", async (route) => {
        const response = await route.fetch();
        const source = await response.text();
        const guard = "    if (!parser) return null;\n";
        assert.ok(source.includes(guard));
        await route.fulfill({
          response,
          // Stand in for an overrun: the next parse returns no tree.
          body: source.replace(
            guard,
            guard +
              "    if (globalThis.cutParses > 0) {\n" +
              "      globalThis.cutParses -= 1;\n" +
              "      return null;\n" +
              "    }\n",
          ),
        });
      });
    },
  });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await page.locator("#language-select").selectOption("javascript");
    await page.evaluate(async () => {
      await (await import("/syntax-parser.js")).prepareLanguage("javascript");
      document.querySelector("#editor").focus();
    });
    // The scanner alone reads this regexp as division after a class body.
    const code = "class A {} /[()]/.test(value);";
    await page.evaluate(() => {
      window.cutParses = 1;
    });
    await setCode(page, code, code.indexOf("[()") + 1);
    await page.waitForFunction(
      () =>
        window.cutParses === 0 &&
        document.querySelectorAll(".matching-bracket").length === 0,
    );
    assert.equal((await caretState(page)).value, code);

    // An edit before the retry's frame abandons that retry, so cutting the
    // same buffer off again later still earns it one.
    const other = "class B {} /[()]/.test(other);";
    await page.evaluate(async (other) => {
      const editor = document.querySelector("#editor");
      const type = (value) => {
        editor.value = value;
        editor.setSelectionRange(
          other.indexOf("[()") + 1,
          other.indexOf("[()") + 1,
        );
        editor.dispatchEvent(new Event("input", { bubbles: true }));
      };
      window.cutParses = 1;
      type(other);
      // The paint cuts this parse off and queues its retry for the next frame.
      await new Promise(requestAnimationFrame);
      type("x");
      await new Promise(requestAnimationFrame);
      window.cutParses = 1;
      type(other);
    }, other);
    await page.waitForFunction(
      (other) =>
        window.cutParses === 0 &&
        document.querySelector("#editor-highlight code").textContent ===
          other &&
        document.querySelectorAll(".matching-bracket").length === 0,
      other,
    );
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("disabled auto indentation uses native Enter and retains manual indentation", async (t) => {
  const ctx = await editorPage(t, { params: { editorAutoIndent: "0" } });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    const code = "    if ready:";
    await setCode(page, code);
    await page.keyboard.press("Enter");
    assert.deepEqual(await caretState(page), {
      value: `${code}\n`,
      start: code.length + 1,
      end: code.length + 1,
    });
    await page.waitForFunction(
      () => document.querySelector("#editor-lines").textContent === "1\n2",
    );
    assert.equal(await page.locator("#editor-lines").textContent(), "1\n2");
    assert.ok(await page.locator("#editor-highlight .tok-keyword").count());
    await page.keyboard.press("ControlOrMeta+Z");
    assert.equal((await caretState(page)).value, code);
    await page.keyboard.press("ControlOrMeta+Shift+Z");
    assert.equal((await caretState(page)).value, `${code}\n`);
    await page.keyboard.press("Tab");
    assert.equal((await caretState(page)).value, `${code}\n    `);
    await page.keyboard.press("Shift+Tab");
    assert.equal((await caretState(page)).value, `${code}\n`);
    await page.keyboard.press("Escape");
    await page.keyboard.press("Tab");
    assert.equal(
      await page
        .locator("#editor")
        .evaluate((editor) => document.activeElement === editor),
      false,
    );
    await setCode(page, "aREMOVEb", 1, 7);
    await page.keyboard.press("Enter");
    assert.deepEqual(await caretState(page), {
      value: "a\nb",
      start: 2,
      end: 2,
    });
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("disabled auto indentation retains parsed bracket highlighting", async (t) => {
  const ctx = await editorPage(t, { params: { editorAutoIndent: "0" } });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await page.locator("#language-select").selectOption("javascript");
    await page.waitForFunction(async () => {
      const { parseSyntax } = await import("/syntax-parser.js");
      const tree = parseSyntax("", "javascript");
      tree?.delete();
      return tree !== null;
    });
    // The scanner alone reads this regexp as division after a class body.
    const code = "class A {} /[()]/.test(value);";
    const caret = code.indexOf("[()") + 1;
    await setCode(page, code, caret);
    // A cold runtime can overrun the parse budget on its first real buffer,
    // which paints the scanner's marks until the retry on the next frame.
    await page.waitForFunction(
      (code) =>
        document.querySelector("#editor-highlight code").textContent === code &&
        document.querySelectorAll(".matching-bracket").length === 0,
      code,
    );
    assert.equal(await page.locator(".matching-bracket").count(), 0);
    assert.deepEqual(await caretState(page), {
      value: code,
      start: caret,
      end: caret,
    });
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("Jim captions survive editor navigation and language switches", async (t) => {
  const ctx = await editorPage(t, {
    beforeLoad: (page) => page.clock.install(),
  });
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

    await page.locator("#language-select").selectOption("javascript");
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
    await page.clock.runFor(300 * Math.ceil(text.length / 8));
    await page.clock.runFor(13000);
    await assertJim("a completed Jim caption must outlive the idle timeout");

    await page.evaluate(() => {
      document
        .querySelector("#editor")
        .dispatchEvent(new Event("input", { bubbles: true }));
    });
    assert.equal(await editor.inputValue(), code);
    await assertJim("an input event without changed text must preserve Jim");

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

    const languageSelect = page.locator("#language-select");
    await languageSelect.selectOption("python");
    assert.equal(
      await languageSelect.inputValue(),
      "python",
      "the language switch must run",
    );
    assert.notEqual(
      await editor.inputValue(),
      code,
      "the language switch must restore a different buffer",
    );
    await assertJim("switching languages must preserve Jim");

    await page.locator("#language-select").selectOption("javascript");
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

test("disabled bracket auto-close leaves insertion and deletion to the browser", async (t) => {
  const ctx = await editorPage(t, { params: { editorAutoClose: "0" } });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    for (const [value, start, end, key, expected, caret] of [
      ["", 0, 0, "(", "(", 1],
      ["text", 0, 4, "[", "[", 1],
      ["()", 1, 1, ")", "())", 2],
      ["{}", 1, 1, "Backspace", "}", 0],
    ]) {
      await setCode(page, value, start, end);
      await page.keyboard.press(key);
      assert.deepEqual(
        await caretState(page),
        {
          value: expected,
          start: caret,
          end: caret,
        },
        key,
      );
    }
    await setCode(page, "if ready:");
    await page.keyboard.press("Enter");
    assert.equal((await caretState(page)).value, "if ready:\n    ");
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("disabled syntax-aware editor features invoke neither classifier nor parser", async (t) => {
  const ctx = await editorPage(t, {
    params: {
      editorHighlight: "0",
      editorAutoIndent: "0",
      editorAutoClose: "0",
    },
    beforeLoad: async (page) => {
      // Every name editor.js imports, or the page fails to link before any
      // of them could be called and the test reports that instead.
      for (const [module, names] of [
        ["highlight", ["highlight"]],
        ["tokenizer", ["tokenize", "lineCommentFor"]],
      ])
        await page.route(`**/${module}.js`, (route) =>
          route.fulfill({
            contentType: "text/javascript",
            body: names
              .map(
                (name) =>
                  `export function ${name}() { throw new Error("Disabled classifier called"); }`,
              )
              .join("\n"),
          }),
        );
    },
  });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    assert.equal(await page.locator("#editor-highlight").isVisible(), false);
    const color = await page
      .locator("#editor")
      .evaluate((editor) => getComputedStyle(editor).color);
    assert.notEqual(color, "rgba(0, 0, 0, 0)");
    assert.equal(await page.locator("#editor-highlight code").innerHTML(), "");
    await page.keyboard.type("(");
    assert.equal((await caretState(page)).value, "(");
    await setCode(page, "    if ready:");
    await page.keyboard.press("Enter");
    const python = (await caretState(page)).value;
    assert.equal(python, "    if ready:\n");
    await page.locator("#language-select").selectOption("javascript");
    await setCode(page, "if (ready) {");
    await page.keyboard.press("Enter");
    assert.equal((await caretState(page)).value, "if (ready) {\n");
    await page.locator("#language-select").selectOption("python");
    assert.equal((await caretState(page)).value, python);
    assert.equal(await page.locator("#editor-lines").textContent(), "1\n2");
    assert.deepEqual(ctx.parserRequests, []);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("disabled highlighting retains syntax-aware Enter", async (t) => {
  const ctx = await editorPage(t, { params: { editorHighlight: "0" } });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setCode(page, "    if ready:");
    await page.keyboard.press("Enter");
    assert.equal((await caretState(page)).value, "    if ready:\n        ");
    assert.equal(await page.locator("#editor-highlight").isVisible(), false);
    assert.equal(await page.locator("#editor-highlight code").innerHTML(), "");
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("plain editor text follows the highlight layer's color setting", async (t) => {
  const ctx = await editorPage(t, {
    params: { editorHighlight: "0" },
    beforeLoad: (page) => page.emulateMedia({ colorScheme: "dark" }),
  });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    const textColors = () =>
      page.evaluate(() =>
        ["#editor", "#editor-highlight"].map(
          (selector) =>
            getComputedStyle(document.querySelector(selector)).color,
        ),
      );
    assert.deepEqual(await textColors(), [
      "rgb(212, 212, 212)",
      "rgb(212, 212, 212)",
    ]);
    await page.evaluate(() =>
      document.querySelector("#editor-theme-toggle").click(),
    );
    assert.deepEqual(await textColors(), [
      "rgb(31, 31, 31)",
      "rgb(31, 31, 31)",
    ]);
    await page.evaluate(() =>
      document.querySelector("#editor-theme-toggle").click(),
    );
    assert.deepEqual(await textColors(), [
      "rgb(212, 212, 212)",
      "rgb(212, 212, 212)",
    ]);
    const colors = await page.evaluate(() => {
      document
        .querySelector(".editor-stack")
        .style.setProperty("--code-text", "#1f1f1f");
      return ["#editor", "#editor-highlight"].map(
        (selector) => getComputedStyle(document.querySelector(selector)).color,
      );
    });
    assert.deepEqual(colors, ["rgb(31, 31, 31)", "rgb(31, 31, 31)"]);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a new Jim turn starts at the top of the caption bar", async (t) => {
  const ctx = await editorPage(t, {
    beforeLoad: (page) => page.clock.install(),
  });
  if (!ctx) return;
  const { page, errors } = ctx;
  const bar = page.locator("#captions-bar");
  const captionText = page.locator("#captions-text");
  const firstText =
    "Start by checking how often each value appears in the input. ".repeat(40);
  const secondText =
    "Then scan the array and check the boundary cases. ".repeat(40);

  const showJim = async (text, id) => {
    await page.evaluate(
      async ({ text, id }) => {
        const captions = await import("/captions.js");
        captions.updateCaptions("interviewer", text, id, { final: true });
        await new Promise((resolve) => requestAnimationFrame(resolve));
      },
      { text, id },
    );
    await page.clock.runFor(300 * Math.ceil(text.length / 8));
  };

  const scrollDown = async () => {
    const position = await bar.evaluate((node) => {
      node.scrollTop = node.scrollHeight;
      return node.scrollTop;
    });
    assert.ok(position > 0, "the test must actually scroll the caption bar");
    return position;
  };

  try {
    await page.evaluate(async () => {
      await import("/interview.js");
      document.querySelector("#audio-check").hidden = true;
      (await import("/avatar/stage.js")).setStageCovered(false);
    });

    await showJim(firstText, "jim-scroll-1");
    assert.equal(await bar.isVisible(), true);
    assert.equal(await captionText.textContent(), `[Jim]: ${firstText}`);

    const firstPosition = await scrollDown();
    await showJim(firstText, "jim-scroll-1");
    assert.equal(
      await bar.evaluate((node) => node.scrollTop),
      firstPosition,
      "an update to the same turn must preserve the reading position",
    );

    await showJim(secondText, "jim-scroll-2");
    assert.equal(await captionText.textContent(), `[Jim]: ${secondText}`);
    assert.equal(
      await bar.evaluate((node) => node.scrollTop),
      0,
      "a new Jim turn must start at the top",
    );

    const secondPosition = await scrollDown();
    await showJim(firstText, "jim-scroll-1");
    assert.equal(await captionText.textContent(), `[Jim]: ${secondText}`);
    assert.equal(
      await bar.evaluate((node) => node.scrollTop),
      secondPosition,
      "a retired turn must not move the current reading position",
    );

    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a proportional font keeps the textarea and highlight metrics aligned", async (t) => {
  for (const editorHighlight of ["0", "1"]) {
    const ctx = await editorPage(t, {
      params: { editorMonospace: "0", editorHighlight },
    });
    if (!ctx) return;
    const { page, errors } = ctx;
    try {
      await page.evaluate(() =>
        document.querySelector("#editor-font-larger").click(),
      );
      const [editor, overlay] = await page.evaluate(() =>
        ["#editor", "#editor-highlight code"].map((selector) => {
          const style = getComputedStyle(document.querySelector(selector));
          return {
            fontFamily: style.fontFamily,
            fontSize: style.fontSize,
            lineHeight: style.lineHeight,
            tabSize: style.tabSize,
          };
        }),
      );
      const bodyFont = await page.evaluate(
        () => getComputedStyle(document.body).fontFamily,
      );
      assert.equal(editor.fontFamily, bodyFont);
      assert.deepEqual(overlay, editor);
      await setCode(page, "a\nb");
      assert.equal(await page.locator("#editor-lines").textContent(), "1\n2");
      assert.deepEqual(errors, []);
    } finally {
      await page.close();
    }
  }
});

test("editor setup waits for delayed problem initialization before clearing the starter", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  let releaseProblem;
  const problemReady = new Promise((resolve) => {
    releaseProblem = resolve;
  });
  let problemRequested;
  const requested = new Promise((resolve) => {
    problemRequested = resolve;
  });
  let loadingPage;
  let setupFinished = false;
  const setup = editorPage(t, {
    beforeLoad: async (page) => {
      loadingPage = page;
      await page.route(
        "**/problems/chargeback-pair-match.json",
        async (route) => {
          problemRequested();
          await problemReady;
          await route.continue();
        },
      );
    },
  }).then((ctx) => {
    setupFinished = true;
    return ctx;
  });
  try {
    await requested;
    await loadingPage.waitForSelector("#editor:not([disabled])");
    assert.equal(await loadingPage.locator("#pause").isVisible(), false);
    // Recreate the old helper's sequence before the module can initialize.
    await loadingPage.evaluate(() => document.querySelector("#editor").focus());
    await loadingPage.keyboard.press("ControlOrMeta+A");
    await loadingPage.keyboard.press("Delete");
    assert.equal((await caretState(loadingPage)).value, "");
    assert.equal(
      setupFinished,
      false,
      "setup must wait for the starter buffer",
    );
    releaseProblem();
    const { page, errors } = await setup;
    await page.waitForSelector("#pause:not([hidden])");
    assert.equal((await caretState(page)).value, "");
    assert.deepEqual(errors, []);
  } finally {
    releaseProblem();
    await setup.catch(() => {});
    await loadingPage?.close();
  }
});

const cppLiteralBracketCases = [
  { name: "line comment", before: "// note " },
  { name: "block comment", before: "/*\n * note \n */", start: 11 },
  {
    name: "string literal",
    before: 'const char* s = "left ";',
    start: 21,
  },
];

for (const { name, before, start = before.length } of cppLiteralBracketCases) {
  for (const opener of ["(", "[", "{"]) {
    test(`typing ${opener} inside a C++ ${name} inserts only the opener`, async (t) => {
      // Disabling execution keeps C++ available without Compiler Explorer.
      const ctx = await editorPage(t, { params: { editorExecution: "0" } });
      if (!ctx) return;
      const { page, errors } = ctx;
      try {
        await page.locator("#language-select").selectOption("cpp");
        await page.waitForFunction(
          () => document.querySelector("#language-select").value === "cpp",
        );
        await setCode(page, before, start);
        await page.keyboard.type(opener);
        assert.deepEqual(await caretState(page), {
          value: before.slice(0, start) + opener + before.slice(start),
          start: start + 1,
          end: start + 1,
        });
        assert.deepEqual(errors, []);
      } finally {
        await page.close();
      }
    });
  }
}

async function waitForParser(page, language) {
  const syntax = await page.evaluateHandle(() => import("/syntax-parser.js"));
  try {
    await page.waitForFunction(
      ({ syntax, language }) => {
        const tree = syntax.parseSyntax("", language);
        if (!tree) return false;
        tree.delete();
        return true;
      },
      { syntax, language },
    );
  } finally {
    await syntax.dispose();
  }
}

for (const parserReady of [false, true]) {
  test(`context-aware pairing works with highlighting off and parser ${parserReady ? "ready" : "unavailable"}`, async (t) => {
    const ctx = await editorPage(t, {
      params: {
        editorHighlight: "0",
        editorAutoIndent: "0",
        editorExecution: "0",
      },
      beforeLoad: async (page) => {
        if (!parserReady)
          await page.route(/\/vendor\/tree-sitter[^/]*\//, (route) =>
            route.abort(),
          );
      },
    });
    if (!ctx) return;
    const { page, errors } = ctx;
    try {
      // The same text is a comment in C++ and JS, but division in Python.
      // Replacing the buffer after switching also catches stale token ranges.
      for (const language of ["cpp", "python", "javascript"]) {
        await page.locator("#language-select").selectOption(language);
        await page.waitForFunction(
          (language) =>
            document.querySelector("#language-select").value === language,
          language,
        );
        if (parserReady) await waitForParser(page, language);
        const before = "// note ";
        await setCode(page, before);
        await page.keyboard.type("(");
        const typed = before + (language === "python" ? "()" : "(");
        assert.deepEqual(await caretState(page), {
          value: typed,
          start: before.length + 1,
          end: before.length + 1,
        });
        await page.keyboard.press("ControlOrMeta+Z");
        assert.equal((await caretState(page)).value, before);
        await page.keyboard.press("ControlOrMeta+Shift+Z");
        assert.equal((await caretState(page)).value, typed);
      }
      const before = 'const text = "left ";';
      const start = before.indexOf("left") + 4;
      await setCode(page, before, start);
      await page.keyboard.type("[");
      assert.deepEqual(await caretState(page), {
        value: 'const text = "left[ ";',
        start: start + 1,
        end: start + 1,
      });
      await setCode(page, "// text", 3, 7);
      await page.keyboard.type("(");
      assert.deepEqual(await caretState(page), {
        value: "// (text)",
        start: 4,
        end: 8,
      });
      assert.equal(await page.locator("#editor-highlight").isVisible(), false);
      assert.deepEqual(errors, []);
    } finally {
      await page.close();
    }
  });
}

for (const parserReady of [false, true]) {
  test(`literal bracket edits preserve existing closers with parser ${parserReady ? "ready" : "unavailable"}`, async (t) => {
    const ctx = await editorPage(t, {
      beforeLoad: async (page) => {
        if (!parserReady)
          await page.route(/\/vendor\/tree-sitter[^/]*\//, (route) =>
            route.abort(),
          );
      },
    });
    if (!ctx) return;
    const { page, errors } = ctx;
    try {
      if (parserReady) await waitForParser(page, "python");
      for (const [before, start] of [
        ['"a)"', 2],
        ["# a)", 3],
      ]) {
        await setCode(page, before, start);
        await page.keyboard.type("(");
        assert.deepEqual(await caretState(page), {
          value: before.slice(0, start) + "(" + before.slice(start),
          start: start + 1,
          end: start + 1,
        });
        await page.keyboard.press("Backspace");
        assert.deepEqual(await caretState(page), {
          value: before,
          start,
          end: start,
        });
        await page.keyboard.press("ControlOrMeta+Z");
        assert.equal(
          (await caretState(page)).value,
          before.slice(0, start) + "(" + before.slice(start),
        );
        await setCode(page, before, start);
        await page.keyboard.type("(");
        await page.keyboard.type(")");
        assert.deepEqual(await caretState(page), {
          value: before.slice(0, start) + "()" + before.slice(start),
          start: start + 2,
          end: start + 2,
        });
      }
      const value = 'f"{value }"';
      const start = value.indexOf(" ");
      await setCode(page, value, start);
      await page.keyboard.type("(");
      assert.deepEqual(await caretState(page), {
        value: 'f"{value() }"',
        start: start + 1,
        end: start + 1,
      });
      await page.keyboard.press("Backspace");
      assert.equal((await caretState(page)).value, value);
      await page.keyboard.type("(");
      await page.keyboard.type(")");
      assert.equal((await caretState(page)).value, 'f"{value() }"');
      assert.deepEqual(errors, []);
    } finally {
      await page.close();
    }
  });
}

test("unfinished f-string expressions retain bracket editing with a loaded parser", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await waitForParser(page, "python");
    const value = 'f"{value';
    await setCode(page, value);
    await page.keyboard.type("(");
    assert.deepEqual(await caretState(page), {
      value: value + "()",
      start: value.length + 1,
      end: value.length + 1,
    });
    await page.keyboard.press("Backspace");
    assert.equal((await caretState(page)).value, value);
    await page.keyboard.type("(");
    await page.keyboard.type(")");
    assert.deepEqual(await caretState(page), {
      value: value + "()",
      start: value.length + 2,
      end: value.length + 2,
    });
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});
