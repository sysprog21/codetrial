// A browser-level pin for the Restore starter control in web/interview.js: it
// puts the current language's starter back in one undoable page-history edit.
import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  DEFAULT_RUNTIME_CONFIG,
  FAKE_SPEECH_CAPTURE,
  fakeLiveKit,
  launchChromium,
  startStaticServer,
} from "./source.js";
import { animationsFinished } from "./wait.js";

const PROBLEM = "chargeback-pair-match";
const LANGUAGES = ["python", "javascript", "c", "cpp", "java"];

const webFile = (path) => new URL(`../../web/${path}`, import.meta.url);
const startersOf = (problem) =>
  JSON.parse(readFileSync(webFile(`problems/${problem}.json`), "utf8"))
    .starterCode;
const starters = startersOf(PROBLEM);

let browser = null;
let server = null;
let base = "";

before(async () => {
  browser = await launchChromium();
  if (!browser) return;
  // The compiled languages stay disabled in the default config, and a disabled
  // tab ignores the click, so the C, C++ and Java cases need them offered.
  // Nothing here runs a compiled test, so the origin never has to answer.
  ({ server, base } = await startStaticServer({
    runtimeConfig: DEFAULT_RUNTIME_CONFIG.replace(
      "COMPILER_EXPLORER_ENABLED = false",
      "COMPILER_EXPLORER_ENABLED = true",
    ).replace(
      'COMPILER_EXPLORER_BASE_URL = ""',
      'COMPILER_EXPLORER_BASE_URL = "https://compiler.invalid"',
    ),
  }));
});

after(async () => {
  await browser?.close();
  await new Promise((closed) => (server ? server.close(closed) : closed()));
});

/// Page errors, and every dialog the page opens. Restore relies on
/// page history because a native dialog blocks the event loop, so a dialog
/// showing up at all is a failure; they are dismissed here only so a test that
/// hits one fails instead of hanging.
function watch(page) {
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const dialogs = [];
  page.on("dialog", (dialog) => {
    dialogs.push(dialog.message());
    return dialog.dismiss();
  });
  return { errors, dialogs };
}

/// Restore works even when the browser does not offer an editing command.
const WITHOUT_EDITING_COMMAND = `document.queryCommandSupported = () => false;`;

/// A page that has cleared the real preflight and joined the fake room, in a
/// browser of its own with a fake microphone and camera.
async function connectedPage(t, { withoutEditingCommand = false } = {}) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const { chromium } = await import("playwright");
  const media = await chromium.launch({
    args: [
      "--use-fake-device-for-media-stream",
      "--use-fake-ui-for-media-stream",
      FAKE_SPEECH_CAPTURE,
    ],
  });
  t.after(() => media.close());
  const page = await media.newPage();
  const watched = watch(page);
  if (withoutEditingCommand) await page.addInitScript(WITHOUT_EDITING_COMMAND);
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
  await page.goto(`${base}/interview.html?problem=${PROBLEM}`, {
    waitUntil: "domcontentloaded",
  });
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-input").placeholder !== "",
  );
  await page.getByRole("button", { name: "Play test tone" }).click();
  await page.getByRole("button", { name: "I heard it" }).click();
  await page.waitForFunction(
    () => !document.querySelector("#audio-check-join").disabled,
  );
  await page.click("#audio-check-join");
  await page.waitForFunction(
    () => document.querySelector("#timer").textContent !== "00:00",
  );
  // Joined, not merely started: the connect call publishes the first buffer.
  await page.waitForFunction(() =>
    (window.__published || []).some((p) => p.topic === "code_update"),
  );
  return { page, ...watched };
}

const codePackets = (page) =>
  page.evaluate(() =>
    (window.__published || [])
      .filter((p) => p.topic === "code_update")
      .map(({ payload }) => ({
        language: payload.language,
        code: payload.code,
      })),
  );

/// Runs `action` and waits for the code_update it produces, which the debounce
/// holds back for a moment, then returns every packet sent so far. Counted
/// rather than matched on content: the buffer a test resets to has already been
/// sent once, when the page joined, so a match would be satisfied at once.
async function sentBy(page, action) {
  const seen = (await codePackets(page)).length;
  await action();
  await page.waitForFunction(
    (seen) =>
      (window.__published || []).filter((p) => p.topic === "code_update")
        .length > seen,
    seen,
    { timeout: 5000 },
  );
  return codePackets(page);
}

/// Runs `action` and returns how many code_update packets it caused after the
/// debounce has had every chance to fire.
async function sentQuietly(page, action) {
  const seen = (await codePackets(page)).length;
  await action();
  // Absence proof: nothing may be sent, so outwait the code_update debounce.
  await page.waitForTimeout(900);
  return (await codePackets(page)).length - seen;
}

/// An interview page with the media preflight out of the way. Page errors are
/// collected so every test can assert the reset raised none.
async function openPage(
  t,
  params = {},
  { withoutEditingCommand = false } = {},
) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await browser.newPage();
  const { errors, dialogs } = watch(page);
  if (withoutEditingCommand) await page.addInitScript(WITHOUT_EDITING_COMMAND);
  const query = new URLSearchParams({ problem: PROBLEM, ...params });
  await page.goto(`${base}/interview.html?${query}`, {
    waitUntil: "domcontentloaded",
  });
  if (params.mode === "whiteboard") return { page, errors, dialogs };
  // The placeholder is written from inside bindEvents, so waiting on it means
  // the click handlers exist. The editor itself looks ready long before.
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-input").placeholder !== "",
  );
  await page.evaluate(() => {
    document.querySelector("#audio-check").hidden = true;
  });
  return { page, errors, dialogs };
}

const value = (page) => page.locator("#editor").inputValue();
const language = (page, name) => page.selectOption("#language-select", name);
const press = (page) => page.click("#editor-restore-starter");
const restore = press;
const undo = (page) => page.keyboard.press("ControlOrMeta+Z");
const redo = (page) => page.keyboard.press("ControlOrMeta+Shift+Z");

/// Replaces the buffer the way a candidate would, by typing over a selection.
async function overwrite(page, text) {
  await page.evaluate(() => document.querySelector("#editor").focus());
  await page.keyboard.press("ControlOrMeta+A");
  if (text === "") await page.keyboard.press("Delete");
  else await page.keyboard.insertText(text);
}

test("one restore press uses page history without calling native editing commands", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "candidate work\n\n");
    await page.evaluate(() => {
      window.__commands = [];
      document.execCommand = (name) => {
        window.__commands.push(name);
        return false;
      };
      document.querySelector("#editor").setSelectionRange(1, 5, "backward");
    });
    await press(page);
    assert.equal(await value(page), starters.python);
    assert.deepEqual(await page.evaluate(() => window.__commands), []);
    await undo(page);
    assert.equal(await value(page), "candidate work\n\n");
    assert.deepEqual(
      await page
        .locator("#editor")
        .evaluate((node) => [
          node.selectionStart,
          node.selectionEnd,
          node.selectionDirection,
        ]),
      [1, 5, "backward"],
    );
    await redo(page);
    assert.equal(await value(page), starters.python);
    assert.deepEqual(ctx.errors, []);
    assert.deepEqual(ctx.dialogs, []);
  } finally {
    await page.close();
  }
});

test("every language gets exactly its own starter back", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  try {
    for (const name of LANGUAGES) {
      await language(ctx.page, name);
      assert.equal(await value(ctx.page), starters[name], `${name} loads`);
      await overwrite(ctx.page, `// my ${name} attempt\n`);
      await restore(ctx.page);
      assert.equal(await value(ctx.page), starters[name], name);
    }
    assert.deepEqual(ctx.errors, []);
  } finally {
    await ctx.page.close();
  }
});

test("restoring one language leaves the other buffers alone", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    const mine = {};
    for (const name of LANGUAGES) {
      await language(page, name);
      mine[name] = `// ${name} work in progress\n`;
      await overwrite(page, mine[name]);
    }
    await language(page, "c");
    await restore(page);
    assert.equal(await value(page), starters.c);
    for (const name of LANGUAGES.filter((name) => name !== "c")) {
      await language(page, name);
      assert.equal(await value(page), mine[name], `${name} kept its work`);
    }
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("a restored buffer is the language's buffer after a tab round trip", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "print('mine')\n");
    await restore(page);
    await language(page, "javascript");
    await language(page, "python");
    assert.equal(
      await value(page),
      starters.python,
      "the reset reached the per-language buffer, not only the textarea",
    );
  } finally {
    await page.close();
  }
});

test("one undo brings back the buffer from before, and redo the starter", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "def mine():\n    return 1\n");
    const edited = await value(page);
    await restore(page);
    assert.equal(await value(page), starters.python);
    await undo(page);
    assert.equal(await value(page), edited, "undo returns the lost work");
    await redo(page);
    assert.equal(await value(page), starters.python, "redo returns the reset");
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("undo and redo are exact whichever way the old buffer ends", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    // Replacing a whole selection in one command corrupts Chromium's redo when
    // the old buffer ends in a newline like the starter does. Both endings,
    // plus a buffer that shares nothing with the starter, must round-trip.
    for (const before of [
      "def mine():\n    return 1\n",
      "def mine(): return 1",
      "x",
      "\n",
      "class Solution:\n    pass",
    ]) {
      await overwrite(page, before);
      await restore(page);
      assert.equal(await value(page), starters.python, JSON.stringify(before));
      await undo(page);
      assert.equal(await value(page), before, `undo ${JSON.stringify(before)}`);
      await redo(page);
      assert.equal(
        await value(page),
        starters.python,
        `redo ${JSON.stringify(before)}`,
      );
    }
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("indent, dedent and comment edits share the path and stay exact under undo and redo", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    for (const [text, start, end, key] of [
      ["a\nb\nc\n", 0, 4, "Tab"],
      ["a\nb\nc\n", 0, 3, "Tab"],
      ["    a\n    b\nc\n", 0, 12, "Shift+Tab"],
      ["a\nb\nc\n", 0, 4, "ControlOrMeta+/"],
      ["a\nb\nc\n", 0, 6, "ControlOrMeta+/"],
    ]) {
      const label = `${key} over ${JSON.stringify(text)}`;
      await overwrite(page, text);
      await page.evaluate(
        ([s, e]) => document.querySelector("#editor").setSelectionRange(s, e),
        [start, end],
      );
      await page.keyboard.press(key);
      const edited = await value(page);
      assert.notEqual(edited, text, `${label} edits`);
      await undo(page);
      assert.equal(await value(page), text, `undo ${label}`);
      await redo(page);
      assert.equal(await value(page), edited, `redo ${label}`);
    }
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("a browser with the editing command restores on the first press", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    assert.equal(
      await page.evaluate(() => document.queryCommandSupported("insertText")),
      true,
      "this browser offers the command",
    );
    await overwrite(page, "mine\n");
    await press(page);
    assert.equal(await value(page), starters.python, "one press restores");

    assert.equal(
      await page.locator("#editor-restore-starter").getAttribute("aria-label"),
      "Restore the starter code",
    );
    await undo(page);
    assert.equal(await value(page), "mine\n", "one undo brings the code back");
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("a click restores and focuses the editor when mousedown clears focus", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await page.evaluate(() =>
      document.addEventListener("mousedown", () =>
        document.activeElement?.blur(),
      ),
    );
    await overwrite(page, "mine\n");
    await press(page);
    assert.equal(await value(page), starters.python);
    assert.equal(
      await page.evaluate(() => document.activeElement?.id),
      "editor",
    );
    await undo(page);
    assert.equal(await value(page), "mine\n");
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

// Keep the trailing-newline cases that previously exposed native redo bugs.
// Page-history undo and redo must restore every byte without a workaround.
const NO_TRAILING_NEWLINE = Object.entries(
  JSON.parse(readFileSync(webFile("problem-pages.json"), "utf8")),
)
  .map(([, { page }]) => page)
  .filter((page) => !startersOf(page).python.endsWith("\n"))
  .sort()
  .slice(0, 5);

test("undo and redo are exact over a starter that does not end in a newline", async (t) => {
  assert.ok(NO_TRAILING_NEWLINE.length >= 3, "the bank has such starters");
  for (const problem of NO_TRAILING_NEWLINE) {
    const ctx = await openPage(t, { problem });
    if (!ctx) return;
    const { page } = ctx;
    const starter = startersOf(problem).python;
    try {
      for (const before of ["x = 1\n", "a\nb\n", "\n", "x\n\n", "no newline"]) {
        await overwrite(page, before);
        await restore(page);
        assert.equal(
          await value(page),
          starter,
          `${problem} ${JSON.stringify(before)}`,
        );
        await undo(page);
        assert.equal(
          await value(page),
          before,
          `${problem} undo ${JSON.stringify(before)}`,
        );
        await redo(page);
        assert.equal(
          await value(page),
          starter,
          `${problem} redo ${JSON.stringify(before)}`,
        );
      }
      assert.deepEqual(ctx.errors, []);
    } finally {
      await page.close();
    }
  }
});

test("a double-click restores once and leaves one undo step", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "keep this\n");
    await page.dblclick("#editor-restore-starter");
    assert.equal(await value(page), starters.python);
    await page.locator("#editor").focus();
    await undo(page);
    assert.equal(await value(page), "keep this\n");
    await redo(page);
    assert.equal(await value(page), starters.python);
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("the restore control stays inside narrow panels and leaves the editor clickable", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    for (const width of [320, 390, 600, 1024]) {
      await page.setViewportSize({ width, height: 900 });
      await overwrite(page, "keep this\n");
      await press(page);
      const inside = await page.evaluate(() => {
        const panel = document
          .querySelector(".editor-panel")
          .getBoundingClientRect();
        const button = document
          .querySelector("#editor-restore-starter")
          .getBoundingClientRect();
        return button.left >= panel.left && button.right <= panel.right;
      });
      assert.equal(inside, true, String(width));
      await page.locator("#editor").click();
      assert.equal(
        await page.evaluate(() => document.activeElement?.id),
        "editor",
      );
      await undo(page);
      assert.equal(await value(page), "keep this\n");
    }
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("restoring twice adds one undo step, not two", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "work\n");
    await restore(page);
    await restore(page);
    await undo(page);
    assert.equal(
      await value(page),
      "work\n",
      "the second press had nothing to change, so it left no history entry",
    );
  } finally {
    await page.close();
  }
});

test("restoring an untouched starter changes nothing", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await restore(page);
    assert.equal(await value(page), starters.python);
    await undo(page);
    assert.equal(
      await value(page),
      starters.python,
      "nothing was recorded for undo to take back",
    );
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("a cleared editor gets the starter back", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "");
    assert.equal(await value(page), "");
    await restore(page);
    assert.equal(await value(page), starters.python);
    await language(page, "java");
    await language(page, "python");
    assert.equal(await value(page), starters.python);
  } finally {
    await page.close();
  }
});

test("the caret lands at the top and the editor has focus", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "x".repeat(500));
    await restore(page);
    const state = await page.evaluate(() => {
      const editor = document.querySelector("#editor");
      return {
        focused: document.activeElement === editor,
        start: editor.selectionStart,
        end: editor.selectionEnd,
      };
    });
    assert.deepEqual(state, { focused: true, start: 0, end: 0 });
  } finally {
    await page.close();
  }
});

test("the painted overlays follow the restored buffer", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "one\ntwo\nthree\nfour\nfive\nsix\nseven\n");
    await restore(page);
    // The gutter numbers the empty line after a final newline too.
    const lines = starters.python.split("\n").length;
    await page.waitForFunction(
      ({ text, lines }) => {
        const painted = document.querySelector("#editor-highlight code");
        const gutter = document.querySelector("#editor-lines");
        return (
          painted.textContent.trimEnd() === text.trimEnd() &&
          gutter.textContent.trim().split(/\s+/).length === lines
        );
      },
      { text: starters.python, lines },
      { timeout: 5000 },
    );
  } finally {
    await page.close();
  }
});

const ALREADY = "The code is already the starter code.";

test("the untouched starter is unchanged and announced", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await press(page);

    assert.equal(await value(page), starters.python);
    // Said on the next frame, so wait for it: a press that did nothing in
    // silence looked broken, and told a screen reader nothing.
    await page.waitForFunction(
      (message) =>
        document.querySelector("#editor-restore-status").textContent ===
        message,
      ALREADY,
      { timeout: 5000 },
    );
  } finally {
    await page.close();
  }
});

test("every press on the starter is announced, not only the first", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    // A live region reads a change, so the words have to leave the region and
    // come back for a second press to be heard. Counted as the times they were
    // written there.
    await page.evaluate((message) => {
      globalThis.announced = 0;
      new MutationObserver(() => {
        if (
          document.querySelector("#editor-restore-status").textContent ===
          message
        )
          globalThis.announced += 1;
      }).observe(document.querySelector("#editor-restore-status"), {
        childList: true,
        characterData: true,
        subtree: true,
      });
    }, ALREADY);
    for (const presses of [1, 2, 3]) {
      await press(page);
      await page.waitForFunction((n) => globalThis.announced >= n, presses, {
        timeout: 5000,
      });
    }
    assert.equal(await page.evaluate(() => globalThis.announced), 3);
  } finally {
    await page.close();
  }
});

test("an announcement that stopped being true before its frame is not made", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    // A press on the starter schedules "already the starter code" for the next
    // frame. An edit in between makes it false, and so does the control being
    // disabled; neither may be followed by the message.
    await page.evaluate(() => {
      const button = document.querySelector("#editor-restore-starter");
      button.click();
      document.querySelector("#editor").value = "changed\n";
    });
    // Absence proof: the announcement is due on the next frame, so outwait it.
    await page.waitForTimeout(300);
    assert.equal(
      await page.locator("#editor-restore-status").textContent(),
      "",
      "the buffer changed before the frame",
    );
    await page.evaluate((starter) => {
      document.querySelector("#editor").value = starter;
      const button = document.querySelector("#editor-restore-starter");
      button.click();
      button.disabled = true;
    }, starters.python);
    // Absence proof, as above, for the disabled control.
    await page.waitForTimeout(300);
    assert.equal(
      await page.locator("#editor-restore-status").textContent(),
      "",
      "the control was disabled before the frame",
    );
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("a late announcement does not overwrite a successful restore", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await page.evaluate(() => {
      const button = document.querySelector("#editor-restore-starter");
      button.click();
      document.querySelector("#editor").value = "changed\n";
      button.click();
    });
    await page.evaluate(() => new Promise(requestAnimationFrame));
    assert.match(
      await page.locator("#editor-restore-status").textContent(),
      /^Restored the starter code\./,
    );
    assert.equal(await value(page), starters.python);
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("a restored buffer can be undone and redone after switching languages", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "my precious work\n");
    await restore(page);
    await language(page, "javascript");
    await language(page, "python");
    await page.evaluate(() => document.querySelector("#editor").focus());
    await undo(page);
    assert.equal(
      await value(page),
      "my precious work\n",
      "undo restores this language's work after a switch",
    );
    await language(page, "javascript");
    await language(page, "python");
    await page.locator("#editor").focus();
    await redo(page);
    assert.equal(await value(page), starters.python);
  } finally {
    await page.close();
  }
});

// Native editing command support does not affect page-history restores.
for (const [name, disable] of [
  [
    "a browser that offers the editing command but refuses it",
    () => {
      document.execCommand = () => false;
    },
  ],
  [
    "a browser without the editing command at all",
    () => {
      document.queryCommandSupported = () => false;
      document.execCommand = undefined;
    },
  ],
]) {
  test(`${name} restores on the first press`, async (t) => {
    const ctx = await openPage(t);
    if (!ctx) return;
    const { page } = ctx;
    try {
      await overwrite(page, "kept only in the tab\n");
      await page.evaluate(disable);
      await press(page);
      assert.equal(await value(page), starters.python);

      assert.deepEqual(ctx.dialogs, []);
      await language(page, "javascript");
      await language(page, "python");
      assert.equal(
        await value(page),
        starters.python,
        "the input event saves the restored buffer",
      );
      await page.locator("#editor").focus();
      await undo(page);
      assert.equal(await value(page), "kept only in the tab\n");
      await redo(page);
      assert.equal(await value(page), starters.python);
      assert.deepEqual(ctx.errors, []);
    } finally {
      await page.close();
    }
  });
}

test("twenty presses in a row settle on the starter without errors", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "scribble\n");
    await page.evaluate(() => {
      const button = document.querySelector("#editor-restore-starter");
      for (let i = 0; i < 20; i += 1) button.click();
    });
    assert.equal(await value(page), starters.python);
    await undo(page);
    assert.equal(await value(page), "scribble\n");
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("typing then restoring close together ends with the starter on the wire", async (t) => {
  const ctx = await connectedPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    const seen = (await codePackets(page)).length;
    await overwrite(page, "typed a moment ago\n");
    await restore(page);
    // Whether the two edits share one packet depends on how fast the runner
    // is, so only where the wire ends up is pinned: a packet newer than the
    // join one, and the last one carrying the starter. Waited for, not timed,
    // because a fixed wait races the debounced flush.
    await page.waitForFunction(
      ({ seen, code }) => {
        const sent = (window.__published || []).filter(
          (p) => p.topic === "code_update",
        );
        return sent.length > seen && sent.at(-1).payload.code === code;
      },
      { seen, code: starters.python },
      { timeout: 5000 },
    );
    assert.deepEqual((await codePackets(page)).at(-1), {
      language: "python",
      code: starters.python,
    });
    assert.equal(await value(page), starters.python);
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("restoring after a tab switch restores the tab on screen", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "python work\n");
    await language(page, "cpp");
    await overwrite(page, "cpp work\n");
    await restore(page);
    assert.equal(await value(page), starters.cpp);
    await language(page, "python");
    assert.equal(await value(page), "python work\n", "the other tab is intact");
  } finally {
    await page.close();
  }
});

test("a buffer of other scripts and a longer one restores whole", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "αβγ \u{1F600} é\r\nline\r\n");
    await restore(page);
    assert.equal(await value(page), starters.python);
    await page.evaluate(() => {
      const editor = document.querySelector("#editor");
      editor.focus();
      editor.select();
      document.execCommand("insertText", false, "y = 1\n".repeat(200));
    });
    await restore(page);
    assert.equal(await value(page), starters.python);
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("Enter and Space on the focused button each restore on the first press", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    for (const key of ["Enter", "Space"]) {
      await overwrite(page, `before ${key}\n`);
      await page.focus("#editor-restore-starter");
      await page.keyboard.press(key);
      assert.equal(await value(page), starters.python, key);
      await undo(page);
      assert.equal(await value(page), `before ${key}\n`);
    }
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("the control is a named button beside the language it acts on", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    const button = await page.evaluate(() => {
      const node = document.querySelector("#editor-restore-starter");
      return {
        tag: node.tagName,
        type: node.type,
        glyph: node.firstElementChild.textContent.trim(),
        glyphHidden: node.firstElementChild.getAttribute("aria-hidden"),
        label: node.getAttribute("aria-label"),
        title: node.title,
        after: node.previousElementSibling?.id,
        inToolbar: Boolean(node.closest(".editor-toolbar")),
        inDisplaySettings: Boolean(node.closest(".editor-view-controls")),
        inEnd: Boolean(node.closest(".toolbar-end")),
        status: document
          .querySelector("#editor-restore-status")
          .getAttribute("role"),
        visible: node.checkVisibility(),
      };
    });
    // It edits code, so it sits with the language picker on the left and not
    // with the display settings; the accessible name is the label, not the icon.
    assert.deepEqual(button, {
      tag: "BUTTON",
      type: "button",
      glyph: "\u21BA",
      glyphHidden: "true",
      label: "Restore the starter code",
      title: "Restore the starter code",
      after: "language-select",
      inToolbar: true,
      inDisplaySettings: false,
      inEnd: false,
      status: "status",
      visible: true,
    });
  } finally {
    await page.close();
  }
});

test("pressing it after the interview has ended changes nothing", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "final answer\n");
    await page.evaluate(() => document.querySelector("#end").click());
    await page.waitForFunction(() => document.querySelector("#end").disabled);
    await page.evaluate(() => {
      const button = document.querySelector("#editor-restore-starter");
      button.click();
      button.click();
    });
    assert.equal(
      await value(page),
      "final answer\n",
      "the submitted code is not replaced once the interview is over",
    );
  } finally {
    await page.close();
  }
});

test("a disabled editor is not restored", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "locked work\n");
    await page.evaluate(() => {
      document.querySelector("#editor").disabled = true;
      const button = document.querySelector("#editor-restore-starter");
      button.click();
      button.click();
    });
    assert.equal(await value(page), "locked work\n");
  } finally {
    await page.close();
  }
});

test("the behavioral round disables the control when the agent opens it", async (t) => {
  const ctx = await connectedPage(t, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    const button = page.locator("#editor-restore-starter");
    await overwrite(page, "coding round work\n");
    assert.equal(await button.isDisabled(), false);

    // A disabled button has to look disabled, and `.font-size-button` only
    // dimmed itself for aria-disabled, so it read as fully active and still
    // brightened under the pointer. Compared as computed style, since a rule
    // that names the wrong attribute leaves every other assertion green.
    const look = () =>
      button.evaluate((node) => {
        const style = getComputedStyle(node);
        return {
          opacity: style.opacity,
          cursor: style.cursor,
          border: style.borderColor,
          color: style.color,
        };
      });

    // The enabled control first, since the round cannot be closed again: full
    // strength, and a hover that does highlight, which is what makes the
    // disabled case below mean something.
    await page.mouse.move(0, 0);
    const idle = await look();
    assert.equal(idle.opacity, "1", "full strength while enabled");
    const box = await button.boundingBox();
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.waitForFunction(
      ({ idle }) => {
        const style = getComputedStyle(
          document.querySelector("#editor-restore-starter"),
        );
        return style.borderColor !== idle.border || style.color !== idle.color;
      },
      { idle },
      { timeout: 5000 },
    );

    await button.focus();

    // Off the control, so the resting look below is read without a hover.
    await page.mouse.move(0, 0);
    // What the agent sends when the behavioral round opens, delivered through
    // the handler the page registered on the room, as LiveKit would.
    await page.evaluate(() =>
      window.__handlers.DataReceived(
        new TextEncoder().encode(
          JSON.stringify({
            type: "round_state",
            round: "behavioral",
            status: "started",
          }),
        ),
        { identity: "interviewer-room", kind: "AGENT" },
        undefined,
        "control",
      ),
    );
    await page.waitForFunction(
      () => document.querySelector("#editor").disabled,
    );
    assert.equal(await button.isDisabled(), true, "disabled with the editor");

    assert.equal(
      await page.evaluate(() => document.activeElement?.id),
      "language-select",
      "focus moved to the picker, not dropped to the body",
    );

    // The arm taken before the round opened is dropped with a colour
    // transition, so the resting look is read once every animation on the
    // control has finished. Two reads agreeing is not enough: a starved
    // renderer can hold a transition at the same midpoint across both.
    const settled = () =>
      animationsFinished(button, "the restore button's colour transition");
    await settled();
    const rest = await look();
    assert.equal(rest.opacity, "0.6", "dimmed while disabled");
    assert.equal(rest.cursor, "not-allowed");
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    // The highlight is a transition, so give one time to show before judging
    // that none is coming; the enabled case above proves one does.
    await page.waitForTimeout(500);
    await settled();
    assert.deepEqual(await look(), rest, "no hover highlight while disabled");

    await page.evaluate(() => {
      const node = document.querySelector("#editor-restore-starter");
      node.click();
      node.click();
    });
    assert.equal(await value(page), "coding round work\n");
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("it works with every editor option off", async (t) => {
  const ctx = await openPage(t, {
    editorHighlight: "0",
    editorAutoIndent: "0",
    editorAutoClose: "0",
    editorMonospace: "0",
  });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "plain\n");
    await restore(page);
    assert.equal(await value(page), starters.python);
    await undo(page);
    assert.equal(await value(page), "plain\n");
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("with code execution off the control still works", async (t) => {
  const ctx = await openPage(t, { editorExecution: "0" });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "no runner\n");
    await restore(page);
    assert.equal(await value(page), starters.python);
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

// The PR's contract with the agent is that a reset is an ordinary code_update.
// Without a room `publish` returns early, so reading the editor back proves
// nothing about it; these run against a room that records what goes out.
test("a reset goes out as a code_update for the current language", async (t) => {
  const ctx = await connectedPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await sentBy(page, () => overwrite(page, "mine\n"));
    const before = (await codePackets(page)).length;
    const packets = await sentBy(page, () => restore(page));
    assert.deepEqual(packets.at(-1), {
      language: "python",
      code: starters.python,
    });
    assert.equal(packets.length, before + 1, "one packet for one reset");

    await sentBy(page, () => language(page, "javascript"));
    await sentBy(page, () => overwrite(page, "// js work\n"));
    const js = await sentBy(page, () => restore(page));
    assert.deepEqual(js.at(-1), {
      language: "javascript",
      code: starters.javascript,
    });
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("the first restore press publishes once and a repeated no-op publishes nothing", async (t) => {
  const ctx = await connectedPage(t, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await sentBy(page, () => overwrite(page, "mine\n"));
    const before = (await codePackets(page)).length;
    const packets = await sentBy(page, () => press(page));
    assert.equal(packets.length, before + 1);
    assert.deepEqual(packets.at(-1), {
      language: "python",
      code: starters.python,
    });
    assert.equal(await sentQuietly(page, () => press(page)), 0);
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("a browser that refuses the editing command still sends the reset", async (t) => {
  const ctx = await connectedPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await sentBy(page, () => overwrite(page, "mine\n"));
    await page.evaluate(() => {
      document.execCommand = () => false;
    });
    const packets = await sentBy(page, () => restore(page));
    assert.equal(packets.at(-1).code, starters.python);
    assert.deepEqual(ctx.dialogs, [], "restoring opens no blocking dialog");
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("a whiteboard interview loads without the editor panel and without error", async (t) => {
  const ctx = await openPage(t, { mode: "whiteboard" });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await page.locator("#board-panel").waitFor({ state: "visible" });
    // initWhiteboard removes the editor panel, and the control went with it.
    // The listener was bound to the detached node before that, so nothing
    // threw; a regression here shows up as a page error.
    assert.equal(await page.locator("#editor-restore-starter").count(), 0);
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("restore, undo and redo publish the active buffer after language switches", async (t) => {
  const ctx = await connectedPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    const mine = "candidate solution\n";
    await sentBy(page, () => overwrite(page, mine));
    await page
      .locator("#editor")
      .evaluate((node) => node.setSelectionRange(2, 9, "backward"));
    let packets = await sentBy(page, () => restore(page));
    assert.deepEqual(packets.at(-1), {
      language: "python",
      code: starters.python,
    });
    await sentBy(page, () => language(page, "javascript"));
    await sentBy(page, () => overwrite(page, "// JavaScript stays here\n"));
    await sentBy(page, () => language(page, "python"));
    await page.locator("#editor").focus();
    packets = await sentBy(page, () => undo(page));
    assert.deepEqual(packets.at(-1), { language: "python", code: mine });
    assert.deepEqual(
      await page
        .locator("#editor")
        .evaluate((node) => [
          node.selectionStart,
          node.selectionEnd,
          node.selectionDirection,
        ]),
      [2, 9, "backward"],
    );
    packets = await sentBy(page, () => redo(page));
    assert.deepEqual(packets.at(-1), {
      language: "python",
      code: starters.python,
    });
    await sentBy(page, () => language(page, "javascript"));
    assert.equal(await value(page), "// JavaScript stays here\n");
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

for (const [index, name] of LANGUAGES.entries()) {
  test(`${name} starter restore stays undoable and redoable after switching languages`, async (t) => {
    const ctx = await openPage(t);
    if (!ctx) return;
    const { page } = ctx;
    try {
      await language(page, name);
      const marker = name === "python" ? "#" : "//";
      const mine = `${starters[name]}\n${marker} ${name} candidate work\n`;
      await overwrite(page, mine);
      await page
        .locator("#editor")
        .evaluate((node) => node.setSelectionRange(2, 7, "backward"));
      await restore(page);
      assert.equal(await value(page), starters[name]);
      const other = LANGUAGES[(index + 1) % LANGUAGES.length];
      await language(page, other);
      assert.equal(await value(page), starters[other]);
      await language(page, name);
      await page.locator("#editor").focus();
      await undo(page);
      assert.equal(await value(page), mine);
      assert.deepEqual(
        await page
          .locator("#editor")
          .evaluate((node) => [
            node.selectionStart,
            node.selectionEnd,
            node.selectionDirection,
          ]),
        [2, 7, "backward"],
      );
      await language(page, other);
      await language(page, name);
      await page.locator("#editor").focus();
      await redo(page);
      assert.equal(await value(page), starters[name]);
      assert.deepEqual(ctx.errors, []);
    } finally {
      await page.close();
    }
  });
}
