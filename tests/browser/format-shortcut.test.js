// Run with: node --test tests/browser/format-shortcut.test.js
//
// The Format JS button and Shift+Alt+F in a real interview page. What the
// formatter writes is pinned in editor-format.test.js; what can break here is
// the wiring: the button and the chord reaching the formatter in JavaScript
// and no other language, the result landing as one edit that undo takes back
// with the overlay and the gutter in step, and a refusal leaving everything as
// it was.

import { after, before, test } from "node:test";
import assert from "node:assert/strict";

import { createJavaScriptFormatter } from "../../web/format.js";
import {
  DEFAULT_RUNTIME_CONFIG,
  FAKE_SPEECH_CAPTURE,
  fakeLiveKit,
  launchChromium,
  startStaticServer,
} from "./source.js";

const PROBLEM = "chargeback-pair-match";
// A working answer to the problem, written with no formatting at all.
const MESSY =
  "function matchDisputedCharge(nums,target){const seen=new Map();for(let i=0;i<nums.length;i++){const want=target-nums[i];if(seen.has(want))return [seen.get(want),i];seen.set(nums[i],i)}return []}\n";
const CHORD = "Shift+Alt+F";

let browser = null;
let server = null;
let base = "";
let tidy = "";

before(async () => {
  tidy = (await createJavaScriptFormatter()(MESSY, 0, "    ")).value;
  browser = await launchChromium();
  if (!browser) return;
  // Compiled runs on, at an origin nothing answers, so the C, C++ and Java
  // tabs are offered without anything leaving the machine.
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

/// The interview page with the editor reachable, on the JavaScript tab unless
/// told otherwise. Waits on the placeholder bindEvents sets, because the
/// textarea looks ready before any listener exists.
///
/// A window listener records whether the last F keydown had its default
/// taken; it runs after the editor's own, so it sees the answer. Requests for
/// the vendored formatter are counted, to show it loads only when asked for.
async function interviewPage(t, { prepare } = {}) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const formatterRequests = [];
  page.on("request", (request) => {
    if (request.url().includes("/vendor/prettier"))
      formatterRequests.push(request.url());
  });
  await prepare?.(page);
  await openEditor(page);
  await page.locator("#language-select").selectOption("javascript");
  return { page, errors, formatterRequests };
}

/// Loads the interview page and clears the way to its editor.
async function openEditor(page) {
  await page.goto(`${base}/interview.html?problem=${PROBLEM}`, {
    waitUntil: "domcontentloaded",
  });
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-input").placeholder !== "",
  );
  await page.evaluate(() => {
    document.querySelector("#audio-check").hidden = true;
    window.addEventListener("keydown", (event) => {
      if (event.code === "KeyF")
        globalThis.fDefaultPrevented = event.defaultPrevented;
    });
  });
}

const formatButton = (page) =>
  page.getByRole("button", { name: "Format JS", exact: true });

/// Replaces the buffer and places the caret. One input event rather than
/// typing, so auto-indent and bracket pairing have no say in the fixture.
const setBuffer = (page, value, start = 0, end = start) =>
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
const status = (page) => page.locator("#editor-format-status");
const fDefaultPrevented = (page) =>
  page.evaluate(() => globalThis.fDefaultPrevented);

/// Waits until the newest format has answered, then one frame more for the
/// status line, so an absence can be asserted without a fixed sleep.
async function formatSettled(page) {
  await page.waitForFunction(
    () => !document.querySelector("#editor-format").classList.contains("busy"),
  );
  await page.evaluate(() => new Promise(requestAnimationFrame));
}

/// Presses the chord and waits for the formatter's answer, which arrives
/// asynchronously and is always announced in the status line.
async function formatNow(page) {
  await page.keyboard.press(CHORD);
  await page.waitForFunction(
    () => document.querySelector("#editor-format-status").textContent !== "",
  );
}

/// The overlay and the gutter, once the next frame has painted them. The
/// overlay carries one more newline after a trailing one, which highlight.js
/// adds so the last row does not collapse.
async function assertPainted(page, code, message) {
  await page.waitForFunction(
    (overlay) =>
      document.querySelector("#editor-highlight code").textContent === overlay,
    code.endsWith("\n") ? `${code}\n` : code,
  );
  assert.equal(
    await page.locator("#editor-lines").textContent(),
    code
      .split("\n")
      .map((_, index) => index + 1)
      .join("\n"),
    `${message}: line numbers`,
  );
}

test("the Format JS button formats JavaScript and is off on every other tab", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors, formatterRequests } = ctx;
  try {
    // The chord is named where it works: on the editor, on this tab only.
    assert.equal(
      await page.locator("#editor").getAttribute("aria-keyshortcuts"),
      "Shift+Alt+F",
    );
    assert.match(
      await formatButton(page).getAttribute("title"),
      /in the editor/,
    );
    for (const language of ["python", "c", "cpp", "java"]) {
      await page.locator("#language-select").selectOption(language);
      assert.equal(await formatButton(page).isDisabled(), true, language);
    }
    await page.locator("#language-select").selectOption("javascript");
    assert.equal(await formatButton(page).isDisabled(), false);
    // Nothing is downloaded until a candidate formats.
    assert.deepEqual(formatterRequests, []);
    const caret = MESSY.indexOf("new Map") + 2;
    await setBuffer(page, MESSY, caret);
    await formatButton(page).click();
    await page.waitForFunction(
      () => document.querySelector("#editor-format-status").textContent !== "",
    );
    assert.equal(await buffer(page), tidy);
    await assertPainted(page, tidy, "formatted");
    assert.match(await status(page).textContent(), /^Formatted the code\./);
    // The editor has the focus back, with the caret where the formatter put
    // it, so typing goes on in the code.
    assert.equal(
      await page.evaluate(() => document.activeElement?.id),
      "editor",
    );
    const [start] = await selection(page);
    assert.equal(tidy.slice(start - 2, start + 5), "new Map");
    await page.keyboard.press("ControlOrMeta+Z");
    assert.equal(await buffer(page), MESSY);
    await page.keyboard.press("ControlOrMeta+Shift+Z");
    assert.equal(await buffer(page), tidy);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("Shift+Alt+F formats JavaScript as one edit undo takes back, with the overlay and gutter in step", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    // Inside a token, where the caret has one place to go; at a boundary
    // between two, Prettier may keep it with either.
    const caret = MESSY.indexOf("new Map") + 2;
    await setBuffer(page, MESSY, caret);
    await formatNow(page);
    assert.equal(await fDefaultPrevented(page), true);
    assert.equal(await buffer(page), tidy);
    const [start, end] = await selection(page);
    assert.equal(start, end);
    assert.equal(tidy.slice(start - 2, start + 5), "new Map");
    assert.match(await status(page).textContent(), /^Formatted the code\./);
    assert.equal(await status(page).isVisible(), true);
    await assertPainted(page, tidy, "formatted");
    // Repeated, so a second round trip is not a different path from the first.
    for (let round = 0; round < 3; round += 1) {
      await page.keyboard.press("ControlOrMeta+Z");
      assert.equal(await buffer(page), MESSY, `undo ${round}`);
      await assertPainted(page, MESSY, `undo ${round}`);
      await page.keyboard.press("ControlOrMeta+Shift+Z");
      assert.equal(await buffer(page), tidy, `redo ${round}`);
      await assertPainted(page, tidy, `redo ${round}`);
    }
    // Formatting what is already formatted is no edit at all.
    await page.keyboard.press("ControlOrMeta+Z");
    await page.keyboard.press("ControlOrMeta+Shift+Z");
    await formatNow(page);
    assert.equal(
      await status(page).textContent(),
      "The code is already formatted.",
    );
    await page.keyboard.press("ControlOrMeta+Z");
    assert.equal(await buffer(page), MESSY);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("Python, C, C++ and Java keep the chord", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors, formatterRequests } = ctx;
  try {
    for (const language of ["python", "c", "cpp", "java"]) {
      await page.locator("#language-select").selectOption(language);
      await setBuffer(page, "x  =  1\n");
      await page.keyboard.press(CHORD);
      assert.equal(await fDefaultPrevented(page), false, language);
      assert.equal(await buffer(page), "x  =  1\n", language);
      assert.equal(
        await page.locator("#editor").getAttribute("aria-keyshortcuts"),
        null,
        language,
      );
    }
    assert.deepEqual(formatterRequests, []);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("code that does not parse is left as it was, selection and history included", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    const earlier = await buffer(page);
    const broken = "function f(a,b){return a+\n";
    await setBuffer(page, broken, 3, 9);
    await formatNow(page);
    assert.equal(
      await status(page).textContent(),
      "Formatting failed: Unexpected token at line 2, column 1. The code was not changed.",
    );
    assert.equal(await status(page).isVisible(), true);
    // Between the button and the code, where it is seen without scrolling.
    const [note, code] = await Promise.all([
      status(page).boundingBox(),
      page.locator(".editor-stack").boundingBox(),
    ]);
    assert.ok(
      note.y + note.height <= code.y + 1,
      JSON.stringify({ note, code }),
    );
    assert.equal(await buffer(page), broken);
    assert.deepEqual(await selection(page), [3, 9]);
    // The editor is still the candidate's: typing goes on, and the note stays
    // so the editor does not move under the caret.
    const message = await status(page).textContent();
    await page.keyboard.press("End");
    await page.keyboard.type("b");
    assert.equal(await status(page).textContent(), message);
    // No history entry was added: two undos reach the buffer before the fixture.
    await page.keyboard.press("ControlOrMeta+Z");
    assert.equal(await buffer(page), broken);
    await page.keyboard.press("ControlOrMeta+Z");
    assert.equal(await buffer(page), earlier);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a formatter that cannot load changes nothing, and the next press loads it", async (t) => {
  let online = false;
  const ctx = await interviewPage(t, {
    prepare: (page) =>
      page.route("**/vendor/prettier/standalone.mjs", (route) =>
        online ? route.continue() : route.abort(),
      ),
  });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY);
    await formatNow(page);
    assert.equal(
      await status(page).textContent(),
      "Formatting failed: The formatter could not be loaded. The code was not changed.",
    );
    assert.equal(await buffer(page), MESSY);
    online = true;
    await formatNow(page);
    assert.equal(await buffer(page), tidy);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

/// Holds the first formatter module until `release` is called, so a test can
/// act while the formatter is still loading.
function holdFormatter() {
  let release;
  const held = new Promise((resolve) => (release = resolve));
  return {
    release,
    prepare: (page) =>
      page.route("**/vendor/prettier/standalone.mjs", async (route) => {
        await held;
        await route.continue();
      }),
  };
}

test("the button shows its press, and keeps that look until the formatter answers", async (t) => {
  const hold = holdFormatter();
  const ctx = await interviewPage(t, { prepare: hold.prepare });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    // Read where the toolbar's eased colors settle.
    const border = () =>
      formatButton(page).evaluate(async (node) => {
        await Promise.all(node.getAnimations().map((a) => a.finished));
        return getComputedStyle(node).borderColor;
      });
    await setBuffer(page, MESSY);
    const idle = await border();
    const box = await formatButton(page).boundingBox();
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    const pressed = await border();
    assert.notEqual(pressed, idle, "pressed");
    await page.mouse.up();
    // Prettier is still loading, and the button keeps the pressed look.
    await page.mouse.move(0, 0);
    assert.equal(await border(), pressed, "working");
    hold.release();
    await page.waitForFunction(
      () => document.querySelector("#editor-format-status").textContent !== "",
    );
    await page.mouse.move(0, 0);
    await page.evaluate(() => document.activeElement?.blur());
    assert.equal(await border(), idle, "answered");
    assert.equal(await buffer(page), tidy);
    await page.locator("#editor").focus();
    await page.keyboard.press("ControlOrMeta+Z");
    assert.equal(await buffer(page), MESSY);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("an edit made while the formatter loads is kept, and the result dropped", async (t) => {
  const hold = holdFormatter();
  const ctx = await interviewPage(t, { prepare: hold.prepare });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY, MESSY.length);
    const requested = page.waitForRequest("**/vendor/prettier/standalone.mjs");
    await page.keyboard.press(CHORD);
    await requested;
    await page.keyboard.type("//x");
    hold.release();
    await formatSettled(page);
    assert.equal(await buffer(page), `${MESSY}//x`);
    assert.equal(await status(page).textContent(), "");
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("an edit undone while the formatter loads still drops the result and keeps its redo", async (t) => {
  const hold = holdFormatter();
  const ctx = await interviewPage(t, { prepare: hold.prepare });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY, MESSY.length);
    const requested = page.waitForRequest("**/vendor/prettier/standalone.mjs");
    await page.keyboard.press(CHORD);
    await requested;
    // Typed and taken back: the buffer and caret are as they were at the
    // press, but the candidate did edit, and the x waits on the redo stack.
    await page.keyboard.type("x");
    await page.keyboard.press("ControlOrMeta+Z");
    assert.equal(await buffer(page), MESSY);
    hold.release();
    await formatSettled(page);
    assert.equal(await buffer(page), MESSY);
    assert.equal(await status(page).textContent(), "");
    await page.keyboard.press("ControlOrMeta+Shift+Z");
    assert.equal(await buffer(page), `${MESSY}x`);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a tab switch while the formatter loads drops the result", async (t) => {
  const hold = holdFormatter();
  const ctx = await interviewPage(t, { prepare: hold.prepare });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY);
    const requested = page.waitForRequest("**/vendor/prettier/standalone.mjs");
    await page.keyboard.press(CHORD);
    await requested;
    await page.locator("#language-select").selectOption("python");
    const python = await buffer(page);
    hold.release();
    await formatSettled(page);
    assert.equal(await buffer(page), python);
    await page.locator("#language-select").selectOption("javascript");
    assert.equal(await buffer(page), MESSY);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a second press while the formatter loads formats the newer buffer", async (t) => {
  const hold = holdFormatter();
  const ctx = await interviewPage(t, { prepare: hold.prepare });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY, MESSY.length);
    const requested = page.waitForRequest("**/vendor/prettier/standalone.mjs");
    await formatButton(page).click();
    await requested;
    // The candidate edits and asks again before the first answer is in. The
    // first result no longer fits the buffer; the second press is the one
    // that counts.
    await page.keyboard.type("let  after=1");
    await formatButton(page).click();
    hold.release();
    await formatSettled(page);
    const newer = (
      await createJavaScriptFormatter()(`${MESSY}let  after=1`, 0, "    ")
    ).value;
    assert.equal(await buffer(page), newer);
    assert.match(await status(page).textContent(), /^Formatted the code\./);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a caret moved while the formatter loads is where the caret lands", async (t) => {
  const hold = holdFormatter();
  const ctx = await interviewPage(t, { prepare: hold.prepare });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY, 0);
    const requested = page.waitForRequest("**/vendor/prettier/standalone.mjs");
    await page.keyboard.press(CHORD);
    await requested;
    // Moved without typing: the buffer is the same, the caret is not.
    const moved = MESSY.indexOf("seen.set") + 2;
    await page.evaluate(
      (at) => document.querySelector("#editor").setSelectionRange(at, at),
      moved,
    );
    hold.release();
    await formatSettled(page);
    assert.equal(await buffer(page), tidy);
    const [start, end] = await selection(page);
    assert.equal(start, end);
    assert.equal(tidy.slice(start - 2, start + 6), "seen.set");
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("an editor locked while the formatter loads keeps its code", async (t) => {
  const hold = holdFormatter();
  const ctx = await interviewPage(t, { prepare: hold.prepare });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY);
    const requested = page.waitForRequest("**/vendor/prettier/standalone.mjs");
    await page.keyboard.press(CHORD);
    await requested;
    // Locked the way the interview page locks it: Strict before readiness, a
    // refused selection and the behavioral round all set disabled.
    await page.evaluate(() => {
      document.querySelector("#editor").disabled = true;
    });
    hold.release();
    await formatSettled(page);
    assert.equal(await buffer(page), MESSY);
    assert.equal(await status(page).textContent(), "");
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a composition begun while the formatter loads keeps its code", async (t) => {
  const hold = holdFormatter();
  const ctx = await interviewPage(t, { prepare: hold.prepare });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY, MESSY.length);
    const requested = page.waitForRequest("**/vendor/prettier/standalone.mjs");
    await page.keyboard.press(CHORD);
    await requested;
    // An input method has opened its candidate window but changed nothing yet.
    await page.evaluate(() =>
      document
        .querySelector("#editor")
        .dispatchEvent(new CompositionEvent("compositionstart")),
    );
    hold.release();
    await formatSettled(page);
    assert.equal(await buffer(page), MESSY);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a repeated keydown after the formatter answered does not format again", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY);
    await formatNow(page);
    const formatted = await status(page).textContent();
    assert.match(formatted, /^Formatted the code\./);
    // The key still held: another format would announce that the code is
    // already formatted, replacing the message.
    const prevented = await page.evaluate(() => {
      const event = new KeyboardEvent("keydown", {
        key: "F",
        code: "KeyF",
        shiftKey: true,
        altKey: true,
        repeat: true,
        bubbles: true,
        cancelable: true,
      });
      document.querySelector("#editor").dispatchEvent(event);
      return event.defaultPrevented;
    });
    assert.equal(prevented, true);
    await formatSettled(page);
    assert.equal(await status(page).textContent(), formatted);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a round trip to another tab while the formatter loads drops the result", async (t) => {
  const hold = holdFormatter();
  const ctx = await interviewPage(t, { prepare: hold.prepare });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY);
    const requested = page.waitForRequest("**/vendor/prettier/standalone.mjs");
    await page.keyboard.press(CHORD);
    await requested;
    // Back on the same tab, with the same buffer and caret it left with.
    await page.locator("#language-select").selectOption("python");
    await page.locator("#language-select").selectOption("javascript");
    hold.release();
    await formatSettled(page);
    assert.equal(await buffer(page), MESSY);
    assert.equal(await status(page).textContent(), "");
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("the undo hint does not outlive the edit that makes it untrue", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY);
    await formatNow(page);
    assert.match(await status(page).textContent(), /undoes this/);
    const top = (await page.locator(".editor-stack").boundingBox()).y;
    await page.keyboard.type("x");
    // One undo now takes back the x, not the format.
    assert.doesNotMatch(await status(page).textContent(), /undoes this/);
    // And the editor did not move under the caret.
    assert.equal((await page.locator(".editor-stack").boundingBox()).y, top);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("Restore clears what Format JS said", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY);
    await formatNow(page);
    await page.locator("#editor-restore-starter").click();
    await page.evaluate(() => new Promise(requestAnimationFrame));
    assert.equal(await status(page).textContent(), "");
    // A failure survives typing, so Restore is what takes it away: its line
    // and column belong to code that is no longer there.
    await setBuffer(page, "function f(a,b){return a+\n");
    await formatNow(page);
    assert.match(await status(page).textContent(), /^Formatting failed/);
    await page.locator("#editor-restore-starter").click();
    await page.evaluate(() => new Promise(requestAnimationFrame));
    assert.equal(await status(page).textContent(), "");
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a result is not announced over a buffer that changed before the frame", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY);
    // The edit lands after the result was applied and before the frame that
    // writes the status.
    await page.evaluate(() => {
      const frame = window.requestAnimationFrame.bind(window);
      window.requestAnimationFrame = (callback) =>
        frame((time) => {
          if (
            !document.querySelector("#editor-format").classList.contains("busy")
          )
            document.querySelector("#editor").value += "// later";
          callback(time);
        });
    });
    await page.keyboard.press(CHORD);
    await page.waitForFunction(() =>
      document.querySelector("#editor").value.endsWith("// later"),
    );
    // Absence proof: outwait any later frame that could still write the
    // status over the changed buffer.
    await page.evaluate(() => new Promise((done) => setTimeout(done, 100)));
    assert.equal(await status(page).textContent(), "");
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a result is not announced over an editor locked before the frame", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY);
    // The lock lands after the result was applied and before the frame that
    // writes the status, as the behavioral round or the end of coding can.
    await page.evaluate(() => {
      const frame = window.requestAnimationFrame.bind(window);
      window.requestAnimationFrame = (callback) =>
        frame((time) => {
          if (
            !document.querySelector("#editor-format").classList.contains("busy")
          )
            document.querySelector("#editor").disabled = true;
          callback(time);
        });
    });
    await page.keyboard.press(CHORD);
    await page.waitForFunction(
      () => document.querySelector("#editor").disabled,
    );
    assert.equal(await buffer(page), tidy);
    // Absence proof: outwait any later frame that could still write the
    // status over the locked editor.
    await page.evaluate(() => new Promise((done) => setTimeout(done, 100)));
    assert.equal(await status(page).textContent(), "");
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a Format JS button that turns off hands the focus on", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await formatButton(page).focus();
    // The tab changes without the select taking the focus itself.
    await page.evaluate(() => {
      const select = document.querySelector("#language-select");
      select.value = "python";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    assert.equal(await formatButton(page).isDisabled(), true);
    assert.equal(
      await page.evaluate(() => document.activeElement?.id),
      "language-select",
    );
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a held chord formats once", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY);
    await page.keyboard.down("Shift");
    await page.keyboard.down("Alt");
    await page.keyboard.down("F");
    // A second down with no up between is how Playwright sends a repeat.
    await page.keyboard.down("F");
    assert.equal(await fDefaultPrevented(page), true);
    await page.keyboard.up("F");
    await page.keyboard.up("Alt");
    await page.keyboard.up("Shift");
    await page.waitForFunction(
      () => document.querySelector("#editor-format-status").textContent !== "",
    );
    assert.equal(await buffer(page), tidy);
    await page.keyboard.press("ControlOrMeta+Z");
    assert.equal(await buffer(page), MESSY);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

// Option+Shift+F on a Mac arrives as a composed letter, U+00CF. Taken in
// JavaScript, typed as ever anywhere else.
test("the Mac spelling of the chord is the chord", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    for (const [language, taken] of [
      ["javascript", true],
      ["python", false],
    ]) {
      await page.locator("#language-select").selectOption(language);
      await setBuffer(page, "x");
      const prevented = await page.evaluate(() => {
        const event = new KeyboardEvent("keydown", {
          key: "Ï",
          code: "KeyF",
          shiftKey: true,
          altKey: true,
          bubbles: true,
          cancelable: true,
        });
        document.querySelector("#editor").dispatchEvent(event);
        return event.defaultPrevented;
      });
      assert.equal(prevented, taken, language);
    }
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("typing, Ctrl+/ and the other Alt chords are untouched", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors, formatterRequests } = ctx;
  try {
    await setBuffer(page, "x", 1);
    await page.keyboard.type("fF");
    assert.equal(await buffer(page), "xfF");
    await page.keyboard.press("Alt+F");
    assert.equal(await fDefaultPrevented(page), false);
    await page.keyboard.press("Control+Shift+F");
    assert.equal(await fDefaultPrevented(page), false);
    assert.equal(await buffer(page), "xfF");
    await page.keyboard.press("Control+/");
    assert.equal(await buffer(page), "// xfF");
    assert.deepEqual(formatterRequests, []);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("Run tests runs the formatted buffer", async (t) => {
  const ctx = await interviewPage(t, {
    // The runner builds its Worker from a Blob whose first part is the code.
    prepare: (page) =>
      page.addInitScript(() => {
        const NativeBlob = Blob;
        globalThis.__workerSources = [];
        globalThis.Blob = class extends NativeBlob {
          constructor(parts, options) {
            super(parts, options);
            if (typeof parts?.[0] === "string")
              globalThis.__workerSources.push(parts[0]);
          }
        };
      }),
  });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await setBuffer(page, MESSY);
    await formatNow(page);
    assert.equal(await buffer(page), tidy);
    await page.click("#run-tests");
    await page.waitForFunction(
      (tidy) => globalThis.__workerSources.includes(tidy),
      tidy,
    );
    assert.equal(
      await page.evaluate(
        (messy) => globalThis.__workerSources.includes(messy),
        MESSY,
      ),
      false,
    );
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

// The interviewer reads the editor through code_update, so the formatted
// buffer has to reach it the way a typed one does.
test("the formatted buffer reaches the interviewer like any edit", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
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
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
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
  await page.waitForFunction(() =>
    (window.__published || []).some((p) => p.topic === "code_update"),
  );
  await page.locator("#language-select").selectOption("javascript");
  await setBuffer(page, MESSY);
  await formatNow(page);
  await page.waitForFunction(
    (tidy) =>
      (window.__published || []).some(
        (p) =>
          p.topic === "code_update" &&
          p.payload.language === "javascript" &&
          p.payload.code === tidy,
      ),
    tidy,
    { timeout: 5000 },
  );
  assert.deepEqual(errors, []);
});
