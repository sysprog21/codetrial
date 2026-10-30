// Run with: node --test tests/browser/compiler-diagnostic-navigation.test.js

import { after, before, test } from "node:test";
import assert from "node:assert/strict";

import {
  closeSetupCheck,
  FAKE_SPEECH_CAPTURE,
  fakeLiveKit,
  launchChromium,
  startStaticServer,
} from "./source.js";

let browser = null;
let server = null;
let base = "";
let compilerReply = null;

const runtimeConfig = `globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = true;
globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = globalThis.location.origin;
globalThis.CODETRIAL_RECORDING_ENABLED = false;
globalThis.CODETRIAL_CONSENT_VERSION = "";
globalThis.CODETRIAL_REPLAY_VERSION = 1;
`;

const requestText = async (request) => {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
};

before(async () => {
  browser = await launchChromium({
    args: [
      "--use-fake-device-for-media-stream",
      "--use-fake-ui-for-media-stream",
      FAKE_SPEECH_CAPTURE,
    ],
  });
  if (!browser) return;
  ({ server, base } = await startStaticServer({
    runtimeConfig,
    handle: async (request, response, url) => {
      if (!url.pathname.startsWith("/api/compiler/")) return false;
      const payload = JSON.parse(await requestText(request));
      const reply = await compilerReply(payload.source);
      response.setHeader("content-type", "application/json");
      response.end(JSON.stringify(reply));
      return true;
    },
  }));
});

after(async () => {
  await browser?.close();
  await new Promise((closed) => (server ? server.close(closed) : closed()));
});

const compilationFailure = (lines) => ({
  didExecute: false,
  stderr: lines.map((text) => ({ text })),
});

function locationsIn(source, names, withColumns = true) {
  const lines = source.split("\n");
  const positions = names.map((name) => {
    const line = lines.findIndex((text) => text.includes(name)) + 1;
    assert.ok(line > 0, `${name} is present in the submitted source`);
    return `<source>:${line}${withColumns ? ":17" : ""}: error: ${name}`;
  });
  return compilationFailure([
    ...positions,
    `<source>:${lines.length + 1}:9: error: harness-only`,
    "/usr/include/string.h:2:17: note: external header",
  ]);
}

function javaLocationsIn(source, names) {
  const reply = locationsIn(source, names, false);
  reply.stderr.splice(
    1,
    0,
    { text: "<source>:1:2: candidate source text" },
    { text: "        ^" },
  );
  return reply;
}

async function interviewPage(
  t,
  language,
  problem = "marquee-letter-tiles",
  { connected = false } = {},
) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  if (connected) {
    await page.addInitScript(fakeLiveKit({ startupReady: true }));
    await page.route("**/vendor/livekit-client.js", (route) =>
      route.fulfill({ contentType: "text/javascript", body: "" }),
    );
    await page.route("**/api/token", (route) =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          serverUrl: "wss://fake.invalid",
          token: "token",
          durationMin: 10,
        }),
      }),
    );
  }
  await page.goto(
    `${base}/interview.html?problem=${problem}&language=${language}`,
    { waitUntil: "domcontentloaded" },
  );
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-input").placeholder !== "",
  );
  if (connected) {
    await page.getByRole("button", { name: "Play test tone" }).click();
    await page.getByRole("button", { name: "I heard it" }).click();
    await page.waitForFunction(
      () => !document.querySelector("#audio-check-join").disabled,
    );
    await page.click("#audio-check-join");
    await page.waitForFunction(() =>
      (window.__published || []).some((item) => item.topic === "code_update"),
    );
  } else {
    await closeSetupCheck(page);
  }
  assert.equal(await page.locator("#language-select").inputValue(), language);
  return { page, errors };
}

async function replaceCode(page, code) {
  return page.evaluate((value) => {
    const editor = document.querySelector("#editor");
    const before = editor.value;
    editor.value = value;
    editor.setSelectionRange(value.length, value.length);
    editor.dispatchEvent(new Event("input", { bubbles: true }));
    return before;
  }, code);
}

const editorState = (page) =>
  page.evaluate(() => {
    const editor = document.querySelector("#editor");
    return {
      value: editor.value,
      start: editor.selectionStart,
      end: editor.selectionEnd,
      focused: document.activeElement === editor,
      activeLine: document
        .querySelector(".editor-stack")
        .style.getPropertyValue("--active-line"),
      scrolls: [
        editor.scrollTop,
        document.querySelector("#editor-highlight").scrollTop,
        document.querySelector("#editor-lines").scrollTop,
      ],
      inputEvents: Number(editor.dataset.navigationInputEvents || 0),
    };
  });

const codePacketCount = (page) =>
  page.evaluate(
    () =>
      (window.__published || []).filter((item) => item.topic === "code_update")
        .length,
  );

async function waitForLocations(page, count) {
  await page.waitForFunction(
    (expected) =>
      document.querySelectorAll("[data-diagnostic-line]").length === expected,
    count,
  );
}

test("multiple C diagnostics navigate by keyboard without editing or publishing code", async (t) => {
  compilerReply = (source) =>
    locationsIn(source, ["missing_first", "missing_second"]);
  const context = await interviewPage(t, "c", "marquee-letter-tiles", {
    connected: true,
  });
  if (!context) return;
  const { page, errors } = context;
  try {
    const code = [
      "bool canSpellFromTiles(char* message, char* tiles) {",
      ...Array.from({ length: 60 }, (_, index) => `  // filler ${index}`),
      "  int first = missing_first;",
      ...Array.from({ length: 20 }, (_, index) => `  // more ${index}`),
      "  int second = missing_second;",
      "  return true;",
      "}",
    ].join("\n");
    const starter = await replaceCode(page, code);
    await page.evaluate(() => {
      const editor = document.querySelector("#editor");
      editor.dataset.navigationInputEvents = "0";
      editor.addEventListener("input", () => {
        editor.dataset.navigationInputEvents = String(
          Number(editor.dataset.navigationInputEvents) + 1,
        );
      });
    });

    await page.click("#run-tests");
    await waitForLocations(page, 2);
    const buttons = page.locator("[data-diagnostic-line]");
    assert.equal(
      await buttons.nth(0).getAttribute("aria-label"),
      "<source>:62:17, go to editor line 62",
    );
    assert.equal(
      await buttons.nth(1).getAttribute("aria-label"),
      "<source>:83:17, go to editor line 83",
    );
    const output = await page.locator("#results-content pre").innerText();
    assert.ok(
      output.indexOf("missing_first") < output.indexOf("missing_second"),
    );
    assert.match(output, /harness-only/);
    assert.match(output, /external header/);

    const packetsBeforeNavigation = await codePacketCount(page);
    await buttons.nth(1).focus();
    await page.keyboard.press("Enter");
    await page.evaluate(() => new Promise(requestAnimationFrame));
    const navigated = await editorState(page);
    const secondStart = code.indexOf("  int second = missing_second;");
    assert.equal(navigated.start, secondStart);
    assert.equal(navigated.end, secondStart);
    assert.equal(navigated.focused, true);
    assert.equal(navigated.activeLine, "82");
    assert.ok(navigated.scrolls[0] > 0);
    assert.deepEqual(navigated.scrolls.slice(1), [
      navigated.scrolls[0],
      navigated.scrolls[0],
    ]);
    assert.equal(navigated.value, code);
    assert.equal(navigated.inputEvents, 0);
    assert.equal(await codePacketCount(page), packetsBeforeNavigation);

    await buttons.nth(0).click();
    const firstNavigation = await editorState(page);
    assert.equal(
      firstNavigation.start,
      code.indexOf("  int first = missing_first;"),
    );
    assert.equal(firstNavigation.value, code);
    assert.equal(firstNavigation.inputEvents, 0);
    assert.equal(await codePacketCount(page), packetsBeforeNavigation);

    await page.keyboard.press("ControlOrMeta+Z");
    await page.waitForFunction(
      (expected) => document.querySelector("#editor").value === expected,
      starter,
    );
    await page.keyboard.press("ControlOrMeta+Shift+Z");
    await page.waitForFunction(
      (expected) => document.querySelector("#editor").value === expected,
      code,
    );

    compilerReply = () => ({
      didExecute: true,
      code: 1,
      stderr: [{ text: "<source>:1:2: candidate runtime text" }],
    });
    await page.click("#run-tests");
    await page.waitForFunction(() =>
      document
        .querySelector("#results-content pre")
        ?.textContent.includes("candidate runtime text"),
    );
    assert.equal(await page.locator("[data-diagnostic-line]").count(), 0);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("Java locations without columns use the pending run snapshot and newer runs replace it", async (t) => {
  let release;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  compilerReply = async (source) => {
    await held;
    return javaLocationsIn(source, ["missing_first", "missing_second"]);
  };
  const context = await interviewPage(t, "java");
  if (!context) return;
  const { page, errors } = context;
  try {
    const firstCode = [
      "class Solution {",
      "  boolean canSpellFromTiles(String message, String tiles) {",
      "    int first = missing_first;",
      "    int second = missing_second;",
      "    return true;",
      "  }",
      "}",
    ].join("\n");
    await replaceCode(page, firstCode);
    await Promise.all([
      page.waitForRequest((request) =>
        request.url().includes("/api/compiler/"),
      ),
      page.click("#run-tests"),
    ]);
    const changedCode = firstCode.replace(
      "    int first = missing_first;",
      "    // edited while compiling\n    int first = missing_first;",
    );
    await replaceCode(page, changedCode);
    const beforeStaleActivation = await editorState(page);
    release();
    await waitForLocations(page, 2);
    await page
      .getByRole("button", {
        name: "<source>:3, go to editor line 3",
      })
      .click();
    assert.equal(
      await page.locator("[data-diagnostic-navigation-status]").textContent(),
      "Code or language has changed. Run tests again to navigate to this location.",
    );
    const afterStaleActivation = await editorState(page);
    assert.equal(afterStaleActivation.value, changedCode);
    assert.equal(afterStaleActivation.start, beforeStaleActivation.start);
    assert.equal(afterStaleActivation.end, beforeStaleActivation.end);

    compilerReply = (source) =>
      javaLocationsIn(source, ["missing_first", "missing_second"]);
    await page.click("#run-tests");
    await page.waitForFunction(() =>
      document
        .querySelector('[data-diagnostic-line="4"]')
        ?.textContent.includes("<source>:4"),
    );
    assert.match(
      await page.locator("#results-content pre").innerText(),
      /<source>:1:2: candidate source text/,
    );
    await page
      .getByRole("button", {
        name: "<source>:4, go to editor line 4",
      })
      .press(" ");
    const freshNavigation = await editorState(page);
    assert.equal(
      freshNavigation.start,
      changedCode.indexOf("    int first = missing_first;"),
    );
    assert.equal(freshNavigation.focused, true);

    await page.selectOption("#language-select", "cpp");
    const beforeLanguageMismatch = await editorState(page);
    await page
      .getByRole("button", {
        name: "<source>:4, go to editor line 4",
      })
      .click();
    assert.equal(
      await page.locator("[data-diagnostic-navigation-status]").textContent(),
      "Code or language has changed. Run tests again to navigate to this location.",
    );
    const afterLanguageMismatch = await editorState(page);
    assert.equal(afterLanguageMismatch.start, beforeLanguageMismatch.start);
    assert.equal(afterLanguageMismatch.end, beforeLanguageMismatch.end);
    assert.deepEqual(errors, []);
  } finally {
    release();
    await page.close();
  }
});
