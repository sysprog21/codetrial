// A browser-level pin for the Restore starter control in web/interview.js: it
// puts the current language's starter back through the editor's own editing
// command, so native undo survives, and it touches nothing else.
import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  DEFAULT_RUNTIME_CONFIG,
  FAKE_SPEECH_CAPTURE,
  launchChromium,
  startStaticServer,
} from "./source.js";

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

/// Page errors, and every dialog the page opens. The control confirms in the
/// toolbar because a native dialog blocks the page's event loop, so a dialog
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

/// A LiveKit room that connects to nothing and records what the page
/// publishes. The page reads `window.LivekitClient`, so this stands in for the
/// vendored SDK, which is blocked below; `state.room` is then set and `publish`
/// no longer returns early, which is what lets a test see the code_update.
const FAKE_LIVEKIT = `
window.LivekitClient = {
  RoomEvent: new Proxy({}, { get: (_, name) => String(name) }),
  Track: { Source: { Microphone: "microphone", Camera: "camera" } },
  Room: class {
    constructor() {
      const agent = { identity: "agent-fake-room", kind: "AGENT", attributes: {} };
      this.remoteParticipants = new Map([[agent.identity, agent]]);
      this.name = "fake-room";
      this.localParticipant = {
        identity: "candidate",
        publishTrack: async () => {},
        publishData: async (bytes, options) => {
          (window.__published ||= []).push({
            topic: options.topic,
            payload: JSON.parse(new TextDecoder().decode(bytes)),
          });
          // The first integrity publication follows pinning the interviewer.
          if (options.topic === "integrity" && !this.ready) {
            this.ready = true;
            window.__handlers.DataReceived(
              new TextEncoder().encode(JSON.stringify({
                type: "interviewer_state", reconnecting: false, startup: "ready",
              })),
              agent, undefined, "control",
            );
          }
        },
        streamBytes: async () => ({ write: async () => {}, close: async () => {} }),
      };
    }
    on(name, handler) {
      (window.__handlers ||= {})[name] = handler;
      return this;
    }
    registerTextStreamHandler() {}
    async connect() {}
    async disconnect() {}
  },
};`;

/// A browser that does not offer the editing command, so a restore could not be
/// undone and asks for a second press. Every real browser offers it, which makes
/// this the one way to reach the confirmation.
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
  await page.addInitScript(FAKE_LIVEKIT);
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
const armed = async (page) =>
  (await page.locator("#editor-restore-starter").getAttribute("data-armed")) ===
  "true";
/// A restore the way a candidate confirms it: a first press arms the control
/// and a second replaces the code. A buffer that already is the starter never
/// arms, so the second press is skipped.
async function restore(page) {
  await press(page);
  if (await armed(page)) await press(page);
}
const undo = (page) => page.keyboard.press("ControlOrMeta+Z");
const redo = (page) => page.keyboard.press("ControlOrMeta+Shift+Z");

/// Replaces the buffer the way a candidate would, by typing over a selection.
async function overwrite(page, text) {
  await page.evaluate(() => document.querySelector("#editor").focus());
  await page.keyboard.press("ControlOrMeta+A");
  if (text === "") await page.keyboard.press("Delete");
  else await page.keyboard.insertText(text);
}

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

test("a browser with the editing command restores on the first press and arms nothing", async (t) => {
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
    assert.equal(await armed(page), false);
    assert.equal(await page.locator("#editor-restore-prompt").isHidden(), true);
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

test("only the restore splits a trailing newline off the span it replaces", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "a\nb");
    await page.evaluate(() => {
      const commands = [];
      window.__commands = commands;
      const run = document.execCommand.bind(document);
      document.execCommand = (name, ...rest) => {
        commands.push(name);
        return run(name, ...rest);
      };
      const editor = document.querySelector("#editor");
      editor.setSelectionRange(0, editor.value.length);
    });
    await page.keyboard.press("ControlOrMeta+/");
    assert.equal(await value(page), "# a\n# b", "the comment toggle ran");
    assert.deepEqual(
      await page.evaluate(() => window.__commands),
      ["insertText"],
      "a comment toggle is one command, with no forward delete after it",
    );
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("a second click confirms even when the browser clears focus on mousedown", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    // Safari does not focus a clicked button and drops the focus that was
    // there, unless the page cancels the mousedown. Playwright's WebKit focuses
    // buttons on click, so the behaviour is modelled here.
    await page.evaluate(() => {
      document.addEventListener("mousedown", (event) => {
        if (!event.defaultPrevented) document.activeElement?.blur();
      });
    });
    await overwrite(page, "mine\n");
    await press(page);
    assert.equal(await armed(page), true, "the first click armed it");
    await press(page);
    assert.equal(await value(page), starters.python, "the second confirmed");
    assert.equal(await armed(page), false);
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("an insert whose forward delete is refused leaves the code and its history as they were", async (t) => {
  const problem = NO_TRAILING_NEWLINE[0];
  const ctx = await openPage(t, { problem });
  if (!ctx) return;
  const { page } = ctx;
  const starter = startersOf(problem).python;
  try {
    // A buffer ending in a newline over a starter that does not takes the
    // insert and then the forward delete; the second is the one that fails.
    await overwrite(page, "first\n");
    await overwrite(page, "mine\n");
    await page.evaluate(() => {
      const run = document.execCommand.bind(document);
      document.execCommand = (name, ...rest) =>
        name === "forwardDelete" ? false : run(name, ...rest);
      document.querySelector("#editor").setSelectionRange(1, 3);
    });
    await press(page);
    assert.equal(await value(page), "mine\n", "the half-done insert is undone");
    assert.equal(await armed(page), true, "asked before assigning the value");
    assert.deepEqual(
      await page.evaluate(() => {
        const editor = document.querySelector("#editor");
        return [editor.selectionStart, editor.selectionEnd];
      }),
      [1, 3],
      "the candidate's selection is back",
    );
    await page.evaluate(() => document.querySelector("#editor").focus());
    await undo(page);
    assert.equal(
      await value(page),
      "first\n",
      "one undo reaches the earlier edit: the refused restore left no step behind",
    );
    await redo(page);
    await press(page);
    await press(page);
    assert.equal(await value(page), starter, "a second press still restores");
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

// Chromium redoes a replaced selection that ends in a newline wrongly, which
// is every buffer ending in a newline restored over a starter that does not.
// The chargeback starter ends in a newline, so it never shows it; these use the
// problems whose starter does not.
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

test("a double-click arms the control and does not confirm it", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "keep this\n");
    await page.dblclick("#editor-restore-starter");
    assert.equal(await armed(page), true, "the first click armed it");
    assert.equal(
      await value(page),
      "keep this\n",
      "the second click of the same gesture did not replace the code",
    );
    await press(page);
    assert.equal(await value(page), starters.python, "a later press confirms");
  } finally {
    await page.close();
  }
});

test("the prompt stays inside the panel and the code under it stays clickable", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "keep this\n");
    for (const width of [320, 390, 600, 1024]) {
      await page.setViewportSize({ width, height: 900 });
      // Fresh each time: a press on an armed control would confirm it.
      await overwrite(page, "keep this\n");
      await press(page);
      assert.equal(await armed(page), true, `${width}: armed`);
      const m = await page.evaluate(() => {
        const panel = document.querySelector(".editor-panel");
        const box = panel.getBoundingClientRect();
        const prompt = document.querySelector("#editor-restore-prompt");
        const at = prompt.getBoundingClientRect();
        const under = document.elementFromPoint(
          at.left + at.width / 2,
          at.top + at.height / 2,
        );
        return {
          inside: at.left >= box.left - 1 && at.right <= box.right + 1,
          scrollsSideways: panel.scrollWidth > panel.clientWidth,
          visible: prompt.checkVisibility(),
          underPrompt: under?.id || under?.tagName,
        };
      });
      assert.equal(m.visible, true, `${width}: shown`);
      assert.equal(m.inside, true, `${width}: inside the panel`);
      assert.equal(m.scrollsSideways, false, `${width}: no sideways scroll`);
      assert.notEqual(
        m.underPrompt,
        "editor-restore-prompt",
        `${width}: the prompt takes no clicks`,
      );
    }
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

test("the first press arms the control and a second one replaces the code", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    const button = page.locator("#editor-restore-starter");
    const toolbar = page.locator(".editor-toolbar");
    const toolbarHeight = (await toolbar.boundingBox()).height;
    await overwrite(page, "mine\n");
    await press(page);
    assert.equal(await armed(page), true);
    assert.equal(await value(page), "mine\n", "one press replaces nothing");
    assert.equal(
      (await page.locator("#editor-restore-prompt").textContent()).trim(),
      "Press again to replace your code",
    );
    assert.equal(
      await button.getAttribute("aria-label"),
      "Press again to replace your code with the starter code",
      "the visible words are inside the accessible name",
    );
    assert.equal(
      (await toolbar.boundingBox()).height,
      toolbarHeight,
      "the prompt hangs below the button, so the toolbar keeps its row",
    );
    assert.match(
      await page.locator("#editor-restore-status").textContent(),
      /Ctrl\+Z \(Cmd\+Z on a Mac\) may not bring it back/,
      "the candidate is told how far undo reaches",
    );
    await press(page);
    assert.equal(await value(page), starters.python);
    assert.equal(await armed(page), false);
    assert.equal(await page.locator("#editor-restore-prompt").isHidden(), true);
    assert.equal(
      await button.getAttribute("aria-label"),
      "Restore the starter code",
    );
    assert.equal(
      await page.locator("#editor-restore-status").textContent(),
      "Restored the starter code. Ctrl+Z (Cmd+Z on a Mac) may undo this, but not after you switch languages.",
    );
    assert.deepEqual(ctx.dialogs, [], "no native dialog, which would block");
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

const ALREADY = "The code is already the starter code.";

test("the starter itself arms nothing, and says so", async (t) => {
  const ctx = await openPage(t);
  if (!ctx) return;
  const { page } = ctx;
  try {
    await press(page);
    assert.equal(await armed(page), false);
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
    assert.equal(await page.locator("#editor-restore-prompt").isHidden(), true);
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

test("a late announcement does not overwrite the prompt an arm has just written", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await page.evaluate(() => {
      const button = document.querySelector("#editor-restore-starter");
      const editor = document.querySelector("#editor");
      // A press on the starter schedules its announcement for the next frame;
      // the buffer then changes and a second press arms before that frame.
      button.click();
      editor.value = "changed\n";
      button.click();
    });
    await page.waitForTimeout(300);
    assert.equal(await armed(page), true);
    assert.match(
      await page.locator("#editor-restore-status").textContent(),
      /Press again to replace your code/,
      "the arm's own words stand",
    );
  } finally {
    await page.close();
  }
});

test("an arm does not run out on its own", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    // Page timers on a fake clock, so a minute passes at once: a screen reader
    // needs as long as it needs to read the status, and a prompt that ran out
    // first would be armed again by the very press meant to confirm it.
    await page.clock.install();
    await overwrite(page, "keep this\n");
    await press(page);
    assert.equal(await armed(page), true);
    await page.clock.fastForward(60_000);
    assert.equal(await armed(page), true, "still armed a minute on");
    assert.equal(await value(page), "keep this\n");
    await press(page);
    assert.equal(await value(page), starters.python, "and it confirms");
  } finally {
    await page.close();
  }
});

// WebKit does not focus a button that is clicked, so a mouse press there arms
// the control with focus still in the editor. A script click is the same: it
// runs the click handler and moves no focus.
const clickWithoutFocus = (page) =>
  page.evaluate(() =>
    document.querySelector("#editor-restore-starter").click(),
  );

test("arming puts focus on the control, so Escape cancels it after a click that did not", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "keep this\n");
    await clickWithoutFocus(page);
    assert.equal(await armed(page), true);
    assert.equal(
      await page.evaluate(() => document.activeElement?.id),
      "editor-restore-starter",
      "arming focused the button, as a click does in Chromium",
    );
    await page.keyboard.press("Escape");
    assert.equal(await armed(page), false, "Escape reached the button");
    await clickWithoutFocus(page);
    assert.equal(
      await armed(page),
      true,
      "a new first press, not a confirmation",
    );
    assert.equal(await value(page), "keep this\n", "nothing was replaced");
  } finally {
    await page.close();
  }
});

test("clicking back into the editor cancels an arm taken without focus", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "keep this\n");
    await clickWithoutFocus(page);
    assert.equal(await armed(page), true);
    // Back into the code without typing: only the button losing focus tells
    // the control, and it has focus only because arming gave it.
    await page.focus("#editor");
    assert.equal(await armed(page), false);
    await clickWithoutFocus(page);
    assert.equal(await armed(page), true, "armed afresh");
    assert.equal(
      await value(page),
      "keep this\n",
      "and the code is still there",
    );
  } finally {
    await page.close();
  }
});

test("leaving the button ends the arm", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "keep this\n");
    await press(page);
    assert.equal(await armed(page), true);
    await page.evaluate(() => document.querySelector("#editor").focus());
    assert.equal(await armed(page), false);
    assert.equal(await page.locator("#editor-restore-prompt").isHidden(), true);
    assert.equal(
      await page.locator("#editor-restore-status").textContent(),
      "",
    );
    await press(page);
    assert.equal(await armed(page), true, "a press now is a first press");
    assert.equal(await value(page), "keep this\n");
  } finally {
    await page.close();
  }
});

test("an edit ends the arm even while the button keeps focus", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "keep this\n");
    await press(page);
    assert.equal(await armed(page), true);
    await page.evaluate(() => {
      const editor = document.querySelector("#editor");
      editor.value += "more\n";
      editor.dispatchEvent(new Event("input", { bubbles: true }));
    });
    assert.equal(await armed(page), false);
    assert.equal(await page.locator("#editor-restore-prompt").isHidden(), true);
  } finally {
    await page.close();
  }
});

test("holding Enter on the button arms it and does not confirm it", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "keep this\n");
    await page.focus("#editor-restore-starter");
    // Auto-repeat is a keydown that arrives again with the key still down, and
    // the browser clicks a button on each of them.
    await page.keyboard.down("Enter");
    for (let i = 0; i < 4; i += 1) await page.keyboard.down("Enter");
    await page.keyboard.up("Enter");
    assert.equal(await armed(page), true, "the first keydown armed it");
    assert.equal(
      await value(page),
      "keep this\n",
      "the repeats did not replace the code",
    );
    await page.keyboard.press("Enter");
    assert.equal(await value(page), starters.python, "a second press does");
  } finally {
    await page.close();
  }
});

test("Escape on the armed control disarms it", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "keep this\n");
    await press(page);
    assert.equal(await armed(page), true);
    await page.keyboard.press("Escape");
    assert.equal(await armed(page), false);
    assert.equal(await value(page), "keep this\n");
  } finally {
    await page.close();
  }
});

test("an edit or a language change between presses makes the next one a first press", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "first\n");
    await press(page);
    await page.evaluate(() => document.querySelector("#editor").focus());
    await page.keyboard.insertText("and more\n");
    await press(page);
    assert.equal(await armed(page), true, "armed afresh for the new buffer");
    assert.equal(await value(page), "first\nand more\n");

    await language(page, "javascript");
    await overwrite(page, "// js\n");
    await press(page);
    assert.equal(await armed(page), true);
    assert.equal(
      await value(page),
      "// js\n",
      "the other language is untouched",
    );
    await language(page, "python");
    assert.equal(await value(page), "first\nand more\n");
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("a language round trip drops the arm even though it lands on the same buffer", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "round trip\n");
    await press(page);
    assert.equal(await armed(page), true);
    await language(page, "javascript");
    assert.equal(await armed(page), false, "switching away disarms");
    await language(page, "python");
    assert.equal(
      await value(page),
      "round trip\n",
      "same language, same buffer",
    );
    // The arm was taken before the switch, so this is a first press: it arms
    // and replaces nothing, where a stale arm would have replaced at once.
    await press(page);
    assert.equal(await armed(page), true);
    assert.equal(await value(page), "round trip\n");
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("an unconfirmed press leaves the buffer, the selection and the undo history alone", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await overwrite(page, "keep this\n");
    await page.keyboard.insertText("and this\n");
    await page.evaluate(() =>
      document.querySelector("#editor").setSelectionRange(3, 8),
    );
    await press(page);
    assert.equal(await value(page), "keep this\nand this\n");
    const selection = await page.evaluate(() => {
      const editor = document.querySelector("#editor");
      return [editor.selectionStart, editor.selectionEnd];
    });
    assert.deepEqual(selection, [3, 8], "the candidate's selection is kept");
    await page.evaluate(() => document.querySelector("#editor").focus());
    await undo(page);
    assert.notEqual(
      await value(page),
      "keep this\nand this\n",
      "the candidate's own typing is still undoable",
    );
    assert.deepEqual(ctx.errors, []);
  } finally {
    await page.close();
  }
});

test("after a language switch the old buffer is gone, as the question says", async (t) => {
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
      starters.python,
      "undo does not cross a switch, which is why the restore asked first",
    );
  } finally {
    await page.close();
  }
});

// A browser with no editing command, or one that refuses it, would have to
// assign `value` and leave nothing to undo. It is asked for a second press
// first, and the first one leaves the code exactly as it was.
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
  test(`${name} keeps the code on the first press and restores on the second`, async (t) => {
    const ctx = await openPage(t);
    if (!ctx) return;
    const { page } = ctx;
    try {
      await overwrite(page, "kept only in the tab\n");
      await page.evaluate(disable);
      await press(page);
      assert.equal(await value(page), "kept only in the tab\n");
      assert.equal(await armed(page), true, "asked before the history is lost");
      await press(page);
      assert.equal(await value(page), starters.python);
      assert.equal(await armed(page), false);
      assert.deepEqual(ctx.dialogs, []);
      await language(page, "javascript");
      await language(page, "python");
      assert.equal(
        await value(page),
        starters.python,
        "the fallback fires the input event, so the buffer is filed",
      );
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

test("Enter and Space on the focused button arm it and then replace the code", async (t) => {
  const ctx = await openPage(t, {}, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    for (const key of ["Enter", "Space"]) {
      await overwrite(page, `before ${key}\n`);
      await page.focus("#editor-restore-starter");
      await page.keyboard.press(key);
      assert.equal(await armed(page), true, `${key} arms`);
      assert.equal(await value(page), `before ${key}\n`);
      await page.keyboard.press(key);
      assert.equal(await value(page), starters.python, `${key} replaces`);
    }
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
        promptHidden: document.querySelector("#editor-restore-prompt").hidden,
        after: node.closest(".restore-control")?.previousElementSibling?.id,
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
      promptHidden: true,
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
    assert.equal(await armed(page), false, "and nothing is left armed");
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
    assert.equal(await armed(page), false);
  } finally {
    await page.close();
  }
});

/// Pausing needs a running clock, and only the real preflight starts it. With
/// the preflight merely hidden the deadline is still zero, so the timer tick a
/// pause makes ends the interview. This is the way run-shortcut.test.js gets a
/// live page, with a fake microphone and camera in a browser of its own.
test("the control is disabled while paused and back on resume", async (t) => {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
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
  await page.addInitScript(WITHOUT_EDITING_COMMAND);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  try {
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
    const button = page.locator("#editor-restore-starter");
    await overwrite(page, "work before the pause\n");
    assert.equal(await button.isDisabled(), false);
    await press(page);
    assert.equal(await armed(page), true, "armed before the pause");
    // Not a click on the Pause button: that would move focus off the control
    // and end the arm through the blur instead of through the pause.
    await page.evaluate(() => document.querySelector("#pause").click());
    await page.waitForFunction(
      () => document.querySelector("#pause").textContent === "Resume",
    );
    // Paused and still live: a pause that ended the interview would disable
    // the control for the wrong reason.
    assert.equal(await page.locator("#ending-overlay").isHidden(), true);
    assert.equal(await button.isDisabled(), true, "paused: nothing to press");
    assert.equal(await armed(page), false, "the pause ended the arm");
    assert.equal(await page.locator("#editor-restore-prompt").isHidden(), true);
    assert.equal(
      await page.evaluate(() => document.activeElement?.id),
      "language-select",
      "focus moved to the picker, not dropped to the body",
    );
    assert.equal(await value(page), "work before the pause\n");
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
    // The arm taken before the pause is dropped with a colour transition, so
    // take the resting look once two reads in a row agree.
    let rest = await look();
    for (let reads = 0; reads < 40; reads += 1) {
      await page.waitForTimeout(100);
      const next = await look();
      if (JSON.stringify(next) === JSON.stringify(rest)) break;
      rest = next;
    }
    assert.equal(rest.opacity, "0.6", "dimmed while disabled");
    assert.equal(rest.cursor, "not-allowed");
    const box = await button.boundingBox();
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    // The highlight is a transition, so give one time to show before judging
    // that none is coming; the enabled case below proves one does.
    await page.waitForTimeout(500);
    assert.deepEqual(await look(), rest, "no hover highlight while disabled");
    await page.click("#pause");
    await page.waitForFunction(
      () => document.querySelector("#pause").textContent === "Pause",
    );
    assert.equal(await button.isDisabled(), false, "resumed: usable again");
    assert.equal(await armed(page), false, "and not still armed from before");
    await page.mouse.move(0, 0);
    const idle = await look();
    assert.equal(idle.opacity, "1", "full strength again");
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
    await restore(page);
    assert.equal(await value(page), starters.python);
    assert.deepEqual(errors, []);
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
    await press(page);
    assert.equal(await armed(page), true, "armed before the round opens");
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
    assert.equal(await armed(page), false, "and no longer armed");
    assert.equal(
      await page.evaluate(() => document.activeElement?.id),
      "language-select",
      "focus moved to the picker, not dropped to the body",
    );
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

test("nothing is sent until the confirming press, and a dropped arm starts over", async (t) => {
  const ctx = await connectedPage(t, { withoutEditingCommand: true });
  if (!ctx) return;
  const { page } = ctx;
  try {
    await sentBy(page, () => overwrite(page, "mine\n"));
    assert.equal(
      await sentQuietly(page, () => press(page)),
      0,
      "armed only: nothing sent",
    );
    // Leaving the button ends the arm, so the next press is not a confirmation.
    await page.evaluate(() => document.querySelector("#editor").focus());
    assert.equal(await armed(page), false);
    assert.equal(
      await sentQuietly(page, () => press(page)),
      0,
      "after the arm ended a press arms again and sends nothing",
    );
    assert.equal(await armed(page), true);
    const packets = await sentBy(page, () => press(page));
    assert.equal(packets.at(-1).code, starters.python, "only this one sends");
    assert.equal(
      await sentQuietly(page, () => restore(page)),
      0,
      "already the starter: nothing sent",
    );
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
    assert.deepEqual(ctx.dialogs, [], "confirmed in the toolbar, not a dialog");
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
