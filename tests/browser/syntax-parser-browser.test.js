import { test, before, after } from "node:test";
import assert from "node:assert/strict";

import {
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  startStaticServer,
} from "./source.js";

let browser;
let server;
let base;

before(async () => {
  browser = await launchChromium();
  if (browser)
    ({ server, base } = await startStaticServer({
      runtimeConfig:
        DEFAULT_RUNTIME_CONFIG +
        `
globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = true;
globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = "https://compiler.invalid";
`,
    }));
});

after(async () => {
  await browser?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
});

async function waitForPaint(page) {
  await page.evaluate(() => new Promise(requestAnimationFrame));
}

test("an interview loads only selected grammars and Enter works during a switch", async (t) => {
  if (!browser) return t.skip("Chromium is not installed");
  const page = await browser.newPage();
  const requests = [];
  const errors = [];
  let release;
  const download = new Promise((resolve) => {
    release = resolve;
  });
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (path.includes("/tree-sitter") && path.endsWith(".wasm"))
      requests.push(path.split("/").at(-1));
  });
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/tree-sitter-javascript.wasm", async (route) => {
    await download;
    await route.continue();
  });
  const select = async (language) => {
    await page.locator("#language-select").selectOption(language);
    await page.locator("#editor").focus();
  };
  const setCode = (value) =>
    page.evaluate(async (value) => {
      const editor = document.querySelector("#editor");
      editor.value = value;
      editor.setSelectionRange(value.length, value.length);
      editor.dispatchEvent(new Event("input", { bubbles: true }));
      editor.focus();
      await new Promise(requestAnimationFrame);
    }, value);
  const snapshot = () =>
    page.evaluate(() => {
      const editor = document.querySelector("#editor");
      return {
        value: editor.value,
        start: editor.selectionStart,
        end: editor.selectionEnd,
      };
    });
  try {
    await page.goto(base + "/interview.html?problem=chargeback-pair-match", {
      waitUntil: "domcontentloaded",
    });
    // C becomes selectable only after the judge callback has run.
    await page.waitForFunction(() => {
      const option = document.querySelector(
        '#language-select [data-language="c"]',
      );
      return option && !option.disabled;
    });
    await page.waitForSelector("#editor:not([disabled])");
    await page.evaluate(async () => {
      const { prepareLanguage } = await import("/syntax-parser.js");
      await prepareLanguage("python");
    });
    assert.deepEqual(requests.toSorted(), [
      "tree-sitter-python.wasm",
      "tree-sitter.wasm",
    ]);

    const python = "    if ready:";
    await setCode(python);
    await select("javascript");
    const code = "const text = `\n/*\n`;\n    if (ready) {";
    await setCode(code);
    await page.keyboard.press("Enter");
    await waitForPaint(page);
    const expected = code + "\n        ";
    assert.deepEqual(await snapshot(), {
      value: expected,
      start: expected.length,
      end: expected.length,
    });
    assert.equal(
      await page.evaluate(async () => {
        const { parseSyntax } = await import("/syntax-parser.js");
        return parseSyntax("const value = 1;", "javascript");
      }),
      null,
    );
    assert.equal(
      await page.locator("#editor-highlight .tok-comment").count(),
      0,
    );
    assert.equal(
      await page.locator("#editor-highlight .tok-string").textContent(),
      "`\n/*\n`",
    );
    assert.equal(
      await page.locator("#editor-highlight code").textContent(),
      expected,
    );

    await select("python");
    assert.equal((await snapshot()).value, python);
    await select("javascript");
    assert.equal((await snapshot()).value, expected);
    const parsedCode = "class A {}\n/\\/*/.test(text);\n    if (ready) {";
    await setCode(parsedCode);
    assert.equal(
      await page.locator("#editor-highlight .tok-string").count(),
      0,
    );
    assert.ok(
      (await page.locator("#editor-highlight .tok-comment").count()) > 0,
    );
    release();
    await page.evaluate(async () => {
      const { prepareLanguage } = await import("/syntax-parser.js");
      await prepareLanguage("javascript");
    });
    await page.waitForFunction(
      () =>
        document.querySelector("#editor-highlight .tok-string")?.textContent ===
        "/\\/*/",
      null,
      { timeout: 5000 },
    );
    assert.equal(
      await page.locator("#editor-highlight .tok-comment").count(),
      0,
    );
    assert.equal(
      await page.locator("#editor-highlight code").textContent(),
      parsedCode,
    );
    assert.deepEqual(await snapshot(), {
      value: parsedCode,
      start: parsedCode.length,
      end: parsedCode.length,
    });
    await page.keyboard.press("Enter");
    await waitForPaint(page);
    const parsedExpected = parsedCode + "\n        ";
    assert.deepEqual(await snapshot(), {
      value: parsedExpected,
      start: parsedExpected.length,
      end: parsedExpected.length,
    });
    assert.equal(
      await page.locator("#editor-highlight code").textContent(),
      parsedExpected,
    );
    assert.equal(
      await page.locator("#editor-lines").textContent(),
      "1\n2\n3\n4",
    );
    await page.keyboard.press("ControlOrMeta+z");
    await waitForPaint(page);
    assert.deepEqual(await snapshot(), {
      value: parsedCode,
      start: parsedCode.length,
      end: parsedCode.length,
    });
    await page.keyboard.press("ControlOrMeta+Shift+z");
    await waitForPaint(page);
    assert.deepEqual(await snapshot(), {
      value: parsedExpected,
      start: parsedExpected.length,
      end: parsedExpected.length,
    });
    assert.deepEqual(requests.toSorted(), [
      "tree-sitter-javascript.wasm",
      "tree-sitter-python.wasm",
      "tree-sitter.wasm",
    ]);
    assert.deepEqual(errors, []);
  } finally {
    release();
    await page.close();
  }
});

test("a grammar completing after a switch does not repaint the active language", async (t) => {
  if (!browser) return t.skip("Chromium is not installed");
  const page = await browser.newPage();
  let release;
  const download = new Promise((resolve) => {
    release = resolve;
  });
  await page.route("**/tree-sitter-javascript.wasm", async (route) => {
    await download;
    await route.continue();
  });
  try {
    await page.goto(base + "/interview.html?problem=chargeback-pair-match", {
      waitUntil: "domcontentloaded",
    });
    await page.waitForSelector("#editor:not([disabled])");
    const code = 'text = """\n/*\n"""\nif True:';
    await page.locator("#language-select").selectOption("javascript");
    await page.locator("#language-select").selectOption("python");
    await page.evaluate(async (code) => {
      const { prepareLanguage } = await import("/syntax-parser.js");
      await prepareLanguage("python");
      await prepareLanguage("python");
      const editor = document.querySelector("#editor");
      editor.value = code;
      editor.setSelectionRange(code.length, code.length);
      editor.dispatchEvent(new Event("input", { bubbles: true }));
      await new Promise(requestAnimationFrame);
      globalThis.highlightChanges = 0;
      globalThis.highlightObserver = new MutationObserver(() => {
        globalThis.highlightChanges += 1;
      });
      globalThis.highlightObserver.observe(
        document.querySelector("#editor-highlight code"),
        { childList: true },
      );
    }, code);
    release();
    const changes = await page.evaluate(async () => {
      const { prepareLanguage } = await import("/syntax-parser.js");
      await prepareLanguage("javascript");
      await new Promise(requestAnimationFrame);
      globalThis.highlightObserver.disconnect();
      return globalThis.highlightChanges;
    });
    assert.equal(
      changes,
      0,
      "an inactive grammar should not trigger another paint",
    );
    assert.equal(await page.locator("#editor").inputValue(), code);
    assert.equal(
      await page.locator("#editor-highlight code").textContent(),
      code,
    );
    assert.equal(
      await page.locator("#editor-highlight .tok-string").textContent(),
      '"""\n/*\n"""',
    );
    assert.equal(await page.locator("#language-select").inputValue(), "python");
  } finally {
    release();
    await page.close();
  }
});

test("a failed grammar download leaves highlighting, Enter and undo usable", async (t) => {
  if (!browser) return t.skip("Chromium is not installed");
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/tree-sitter-javascript.wasm", (route) => route.abort());
  try {
    await page.goto(base + "/interview.html?problem=chargeback-pair-match", {
      waitUntil: "domcontentloaded",
    });
    await page.waitForSelector("#editor:not([disabled])");
    const code = "const text = `\n/*\n`;\n    if (ready) {";
    await page.locator("#language-select").selectOption("javascript");
    await page.evaluate(async (code) => {
      const { prepareLanguage } = await import("/syntax-parser.js");
      await prepareLanguage("javascript").catch(() => {});
      const editor = document.querySelector("#editor");
      editor.value = code;
      editor.setSelectionRange(code.length, code.length);
      editor.dispatchEvent(new Event("input", { bubbles: true }));
      editor.focus();
      await new Promise(requestAnimationFrame);
    }, code);
    assert.equal(
      await page.locator("#editor-highlight .tok-comment").count(),
      0,
    );
    assert.equal(
      await page.locator("#editor-highlight .tok-string").textContent(),
      "`\n/*\n`",
    );
    await page.keyboard.press("Enter");
    await waitForPaint(page);
    const expected = code + "\n        ";
    assert.equal(await page.locator("#editor").inputValue(), expected);
    assert.equal(
      await page.locator("#editor-highlight code").textContent(),
      expected,
    );
    assert.equal(
      await page.locator("#editor-lines").textContent(),
      "1\n2\n3\n4\n5",
    );
    await page.keyboard.press("ControlOrMeta+z");
    await waitForPaint(page);
    assert.equal(await page.locator("#editor").inputValue(), code);
    assert.equal(
      await page.locator("#editor-highlight code").textContent(),
      code,
    );
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("highlighting and Enter share comment and string regions in every language", async (t) => {
  if (!browser) return t.skip("Chromium is not installed");
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  try {
    await page.goto(base + "/interview.html?problem=chargeback-pair-match", {
      waitUntil: "domcontentloaded",
    });
    await page.waitForFunction(() => {
      const option = document.querySelector(
        '#language-select [data-language="c"]',
      );
      return option && !option.disabled;
    });
    for (const [language, declaration, literal, comment] of [
      ["python", "text = ", '"""\n/*\n"""', "# if ready:"],
      ["javascript", "const text = ", "`\n/*\n`", "/*\n    if (ready) {"],
      ["c", "const char *text = ", '"/*"', "/*\n    if (ready) {"],
      ["cpp", "auto text = ", 'R"tag(\n/*\n)tag"', "/*\n    if (ready) {"],
      ["java", "String text = ", '"""\n/*\n"""', "/*\n    if (ready) {"],
    ]) {
      const code = declaration + literal + ";\n    " + comment;
      await page.locator("#language-select").selectOption(language);
      await page.evaluate(
        async ({ code, language }) => {
          const { prepareLanguage } = await import("/syntax-parser.js");
          await prepareLanguage(language);
          const editor = document.querySelector("#editor");
          editor.value = code;
          editor.setSelectionRange(code.length, code.length);
          editor.dispatchEvent(new Event("input", { bubbles: true }));
          editor.focus();
          await new Promise(requestAnimationFrame);
        },
        { code, language },
      );
      assert.deepEqual(
        await page.locator("#editor-highlight .tok-string").allTextContents(),
        [literal],
        language,
      );
      assert.deepEqual(
        await page.locator("#editor-highlight .tok-comment").allTextContents(),
        [comment],
        language,
      );
      assert.equal(
        await page.locator("#editor-highlight code").textContent(),
        code,
        language,
      );
      await page.keyboard.press("Enter");
      await waitForPaint(page);
      const expected = code + "\n    ";
      assert.equal(
        await page.locator("#editor").inputValue(),
        expected,
        language,
      );
      assert.equal(
        await page.locator("#editor-highlight code").textContent(),
        expected,
        language,
      );
    }
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("the browser loads all grammars locally and reuses prepared languages", async (t) => {
  if (!browser) return t.skip("Chromium is not installed");
  const page = await browser.newPage();
  const requests = [];
  const errors = [];
  page.on("request", (request) => {
    requests.push(request.url());
  });
  page.on("pageerror", (error) => {
    errors.push(error.message);
  });
  await page.route("**/syntax-parser-probe", (route) =>
    route.fulfill({
      contentType: "text/html",
      headers: {
        "Content-Security-Policy":
          "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'",
      },
      body: "<!doctype html><title>Syntax parser</title>",
    }),
  );
  try {
    await page.goto(base + "/syntax-parser-probe");
    const cases = [
      ["c", "int value = 1;", "translation_unit"],
      ["cpp", 'auto text = R"tag(/*)tag";', "translation_unit"],
      ["java", "class A {}", "program"],
      [
        "javascript",
        "class A { #return=4; f() { return this.#return / 2; } }",
        "program",
      ],
      ["python", "value = 1\n", "module"],
    ];
    const parsed = await page.evaluate(async (cases) => {
      const { prepareLanguage, parseSyntax } =
        await import("/syntax-parser.js");
      await Promise.all(cases.map(([language]) => prepareLanguage(language)));
      await prepareLanguage("javascript");
      return cases.map(([language, code]) => {
        const tree = parseSyntax(code, language);
        try {
          return [
            language,
            tree.rootNode.type,
            tree.rootNode.hasError,
            tree.rootNode.endIndex,
          ];
        } finally {
          tree?.delete();
        }
      });
    }, cases);
    assert.deepEqual(
      parsed,
      cases.map(([language, code, root]) => [
        language,
        root,
        false,
        code.length,
      ]),
    );
    assert.deepEqual(errors, []);
    assert.ok(requests.every((url) => url.startsWith(base + "/")));
    const assets = requests.filter((url) => url.includes("/vendor/"));
    assert.equal(assets.length, 7);
    assert.equal(new Set(assets).size, 7);
  } finally {
    await page.close();
  }
});
