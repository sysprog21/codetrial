// A browser-level pin for the Run tests shortcut in web/interview.js:
// Cmd+Enter or Ctrl+Enter anywhere on the page but another text field, while
// the interview is on.
import { after, before, test } from "node:test";
import assert from "node:assert/strict";

import {
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  FAKE_SPEECH_CAPTURE,
  pauseClock,
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

/// The interview page in JavaScript, with focus in the editor and every run of
/// the tests counted. JavaScript runs in the page, so a run finishes without
/// booting Pyodide.
///
/// Runs are counted, not clicks. A handler that called runTests() directly
/// would start a run without any click, so a click count would miss exactly
/// the second run the disabled button is there to stop. "Running..." is
/// written by runTests alone, once per run.
///
/// It also records whether the last Enter press had its default taken. A
/// window listener runs after the page's own on document, so it sees the
/// answer. The newline check alone cannot stand in for this: macOS Chromium
/// types no newline for either chord in a textarea, so a dropped
/// preventDefault passed every assertion there.
///
/// `started` hides the media preflight, which is how the shortcut tells
/// whether the interview has begun. `media` opens the page in a browser with a
/// fake microphone and camera instead, for a case that clears the preflight
/// for real. `clock` installs Playwright's fake clock before the page loads,
/// for a case that has to stop the page's timers.
async function interviewPage(
  t,
  {
    started = true,
    media = false,
    params = {},
    clock = false,
    session = null,
  } = {},
) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await (media ? await mediaBrowser(t) : browser).newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  if (clock) await page.clock.install();
  const query = new URLSearchParams({
    problem: "chargeback-pair-match",
    ...params,
  });
  if (session) {
    await page.route("**/api/session", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(session),
      }),
    );
  }
  await page.goto(`${base}/interview.html?${query}`, {
    waitUntil: "domcontentloaded",
  });
  if (params.mode === "whiteboard") {
    await page.locator("#board-panel").waitFor({ state: "visible" });
    if (started)
      await page.evaluate(() => {
        document.querySelector("#audio-check").hidden = true;
      });
    return { page, errors };
  }
  // The placeholder is set from inside bindEvents, after the shortcut is
  // registered. Waiting on the editor would not do: it carries no disabled
  // attribute, so it looks ready before any listener exists, and a test that
  // expects no run would pass against a page that never listened.
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-input").placeholder !== "",
  );
  await page.locator("#language-select").selectOption("javascript");
  await page.evaluate((started) => {
    if (started) document.querySelector("#audio-check").hidden = true;
    globalThis.runs = 0;
    const run = document.querySelector("#run-tests");
    new MutationObserver((records) => {
      for (const record of records)
        for (const node of record.addedNodes)
          if (node.textContent === "Running...") globalThis.runs++;
    }).observe(run, { childList: true });
    window.addEventListener("keydown", (event) => {
      if (event.key === "Enter")
        globalThis.enterDefaultPrevented = event.defaultPrevented;
    });
    document.querySelector("#editor").focus();
  }, started);
  return { page, errors };
}

const runs = (page) => page.evaluate(() => globalThis.runs);
const enterDefaultPrevented = (page) =>
  page.evaluate(() => globalThis.enterDefaultPrevented);

test("disabled execution blocks button and shortcut runs across language switches", async (t) => {
  const ctx = await interviewPage(t, { params: { editorExecution: "0" } });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    assert.match(
      await page.locator(".compile-disclosure").textContent(),
      /execution is disabled/,
    );
    assert.equal(await page.locator("#run-tests").isVisible(), false);
    assert.equal(await page.locator("#candidate-case").isVisible(), false);
    assert.equal(await page.locator("#results-toggle").isVisible(), false);
    for (const language of ["python", "javascript", "c", "cpp", "java"]) {
      const option = page.locator(
        `#language-select option[value="${language}"]`,
      );
      assert.equal(await option.isDisabled(), false, language);
      await page.locator("#language-select").selectOption(language);
      assert.equal(
        await page.locator("#run-tests").isDisabled(),
        true,
        language,
      );
      await page.locator("#editor").focus();
      for (const chord of ["Meta+Enter", "Control+Enter"]) {
        const before = await page.locator("#editor").inputValue();
        await page.keyboard.press(chord);
        assert.equal(await enterDefaultPrevented(page), true, chord);
        assert.equal(await page.locator("#editor").inputValue(), before, chord);
      }
      await page.locator("#run-tests").evaluate((button) => {
        button.disabled = false;
        button.click();
      });
      assert.equal(
        await runs(page),
        0,
        `the execution entry point must reject ${language}`,
      );
    }
    await page.locator("#end").focus();
    for (const chord of ["Meta+Enter", "Control+Enter"]) {
      await page.keyboard.press(chord);
      assert.equal(await enterDefaultPrevented(page), true, chord);
      assert.equal(await page.locator("#end").isDisabled(), false);
      assert.equal(await page.locator("#report-modal").isVisible(), false);
    }
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("disabled execution keeps C unavailable for class problems", async (t) => {
  const ctx = await interviewPage(t, {
    params: { problem: "bounded-event-queue", editorExecution: "0" },
  });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    const c = page.locator('#language-select option[value="c"]');
    await page.waitForFunction(
      () =>
        document.querySelector('#language-select option[value="c"]').disabled,
    );
    assert.equal(await c.isDisabled(), true);
    assert.match(await c.textContent(), /unavailable/);
    await page.locator("#language-select").selectOption("python");
    assert.equal(await page.locator("#language-select").inputValue(), "python");
    assert.match(
      await page.locator("#editor").inputValue(),
      /^class EventQueue:/,
    );
    for (const language of ["python", "javascript", "cpp", "java"]) {
      const option = page.locator(
        `#language-select option[value="${language}"]`,
      );
      assert.equal(await option.isDisabled(), false, language);
      await page.locator("#language-select").selectOption(language);
      const code = await page.locator("#editor").inputValue();
      assert.notEqual(code, "undefined", language);
      assert.ok(code.trim(), language);
    }
    assert.equal(await runs(page), 0);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("offline reports apply execution preferences only to editor interviews", async (t) => {
  for (const mode of ["coding", "whiteboard"]) {
    const ctx = await interviewPage(t, {
      params: { mode, editorExecution: "0" },
      session: { signedIn: true, user: { login: "test-user" } },
    });
    if (!ctx) return;
    const { page, errors } = ctx;
    try {
      await page.evaluate(() => document.querySelector("#end").click());
      await page.locator("#report-modal").waitFor({ state: "visible" });
      const report = await page.evaluate(async () => {
        const { readLocalHistory } = await import("/history.js");
        return readLocalHistory()[0].report;
      });
      assert.equal(report.codeExecution === false, mode === "coding");
      assert.equal(
        (await page.locator("#report-modal").textContent()).includes(
          "Code execution was disabled for this interview.",
        ),
        mode === "coding",
      );
      assert.deepEqual(errors, []);
    } finally {
      await page.close();
    }
  }
});

test("the language selector stays on the submitted language in a report", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await page.evaluate(() => document.querySelector("#end").click());
    await page.locator("#report-modal").waitFor({ state: "visible" });
    const code = await page.locator("#editor").inputValue();
    await page.locator("#language-select").selectOption("python");
    assert.equal(
      await page.locator("#language-select").inputValue(),
      "javascript",
    );
    assert.equal(await page.locator("#editor").inputValue(), code);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

test("a disabled-execution interview sends its choice with the token request", async (t) => {
  const ctx = await interviewPage(t, {
    started: false,
    media: true,
    params: { editorExecution: "0" },
  });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await page.route("**/api/token", (route) =>
      route.fulfill({
        status: 503,
        contentType: "application/json",
        body: JSON.stringify({ error: "Offline test" }),
      }),
    );
    const token = page.waitForRequest(
      (request) => new URL(request.url()).pathname === "/api/token",
    );
    await clearPreflight(page);
    assert.equal((await token).postDataJSON().codeExecution, false);
    assert.equal(await page.locator("#run-tests").isDisabled(), true);
    assert.equal(await page.locator("#run-tests").isVisible(), false);
    assert.equal(await page.locator(".test-drawer").isVisible(), false);
    await page.keyboard.press("ControlOrMeta+Enter");
    assert.equal(await runs(page), 0);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
});

/// Waits until Run tests is enabled. The disabled button refuses a press while
/// a run is going, so a test that expects a run waits here before pressing, or
/// a press that came too soon would fail it. This wait skips past that
/// refusal, so the in-flight case presses without it, and it is what pins the
/// refusal.
const runButtonReady = (page) =>
  page.waitForFunction(() => !document.querySelector("#run-tests").disabled);

/// A browser of this test's own with a fake microphone and camera, closed when
/// the test ends, for a case that has to clear the real preflight.
async function mediaBrowser(t) {
  const { chromium } = await import("playwright");
  const media = await chromium.launch({
    args: [
      "--use-fake-device-for-media-stream",
      "--use-fake-ui-for-media-stream",
      FAKE_SPEECH_CAPTURE,
    ],
  });
  t.after(() => media.close());
  return media;
}

/// The preflight cleared the way a candidate clears it, as clearMediaGate in
/// scripts/browser-check.cjs does, then a wait for the clock to start.
async function clearPreflight(page) {
  await page.getByRole("button", { name: "Play test tone" }).click();
  await page.getByRole("button", { name: "I heard it" }).click();
  await page
    .waitForFunction(
      () => !document.querySelector("#audio-check-join").disabled,
    )
    .catch(async (error) => {
      throw new Error(
        `Media preflight blocked: ${await page.locator("#audio-check-status").textContent()}`,
        { cause: error },
      );
    });
  await page.click("#audio-check-join");
  await page.waitForFunction(
    () => document.querySelector("#timer").textContent !== "00:00",
  );
}

test("Cmd+Enter and Ctrl+Enter in the editor run the tests without typing a newline", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    const code = await page.locator("#editor").inputValue();
    // Both chords on every platform, since the handler takes either modifier.
    for (const [index, chord] of ["Meta+Enter", "Control+Enter"].entries()) {
      await runButtonReady(page);
      await page.keyboard.press(chord);
      assert.equal(await runs(page), index + 1, `${chord} ran no tests`);
      assert.equal(
        await enterDefaultPrevented(page),
        true,
        `${chord} was left to the browser's default`,
      );
    }
    assert.equal(await page.locator("#editor").inputValue(), code);
    assert.deepEqual(
      errors,
      [],
      `the shortcut threw in the browser: ${errors[0]}`,
    );
  } finally {
    await page.close();
  }
});

test("the shortcut still runs the tests after a click on the problem takes focus from the editor", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await page.click("#problem-panel");
    assert.notEqual(
      await page.evaluate(() => document.activeElement?.id),
      "editor",
    );
    await runButtonReady(page);
    await page.keyboard.press("ControlOrMeta+Enter");
    assert.equal(await runs(page), 1);
    assert.deepEqual(
      errors,
      [],
      `the shortcut threw in the browser: ${errors[0]}`,
    );
  } finally {
    await page.close();
  }
});

test("the shortcut does nothing while the media preflight is still up", async (t) => {
  const ctx = await interviewPage(t, { started: false });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    // Enabled as far as the button knows, so what refuses the press below is
    // the shortcut's own check.
    await runButtonReady(page);
    await page.keyboard.press("ControlOrMeta+Enter");
    assert.equal(await runs(page), 0);
    // Taken, so the press got past the target and modifier checks and was
    // refused by the preflight one.
    assert.equal(await enterDefaultPrevented(page), true);
    assert.deepEqual(
      errors,
      [],
      `the shortcut threw in the browser: ${errors[0]}`,
    );
  } finally {
    await page.close();
  }
});

test("the shortcut does nothing once the interview has ended", async (t) => {
  const ctx = await interviewPage(t, { clock: true });
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    const code = await page.locator("#editor").inputValue();
    // The time-up case: the candidate was still typing when the clock ran out.
    // The wait for the report is a modal dialog, so the workspace behind it,
    // the editor included, is inert and focus moves to the dialog; the press
    // lands there. The listener is the document's, so it still sees it, and
    // the defaultPrevented check below proves it did rather than the press
    // reaching nothing.
    //
    // With no interviewer in the room, ending shows the offline report by
    // itself 300ms later and takes focus off the wait, so a slow machine
    // checking focus below found it on the report. A stopped clock keeps the
    // wait up.
    await pauseClock(page);
    await page.click("#end");
    await page.waitForFunction(
      () => document.querySelector(".editor-panel").inert,
    );
    assert.equal(
      await page.evaluate(() =>
        document
          .querySelector("#ending-overlay")
          .contains(document.activeElement),
      ),
      true,
    );
    await page.keyboard.press("ControlOrMeta+Enter");
    assert.equal(await runs(page), 0);
    // Taken even with nothing to run, so it cannot type into the editor.
    assert.equal(await enterDefaultPrevented(page), true);
    assert.equal(await page.locator("#editor").inputValue(), code);
    assert.deepEqual(
      errors,
      [],
      `the shortcut threw in the browser: ${errors[0]}`,
    );
  } finally {
    await page.close();
  }
});

test("a second press while a run is in flight starts no second run", async (t) => {
  const ctx = await interviewPage(t);
  if (!ctx) return;
  const { page, errors } = ctx;
  try {
    await runButtonReady(page);
    // Both presses in one task, so the first run cannot end between them. A
    // JavaScript run finishes in milliseconds, and two presses sent from here
    // could each find the button enabled again.
    await page.evaluate(() => {
      const editor = document.querySelector("#editor");
      for (let press = 0; press < 2; press++)
        editor.dispatchEvent(
          new KeyboardEvent("keydown", {
            key: "Enter",
            ctrlKey: true,
            bubbles: true,
            cancelable: true,
          }),
        );
    });
    await runButtonReady(page);
    assert.equal(await runs(page), 1);
    assert.deepEqual(
      errors,
      [],
      `the shortcut threw in the browser: ${errors[0]}`,
    );
  } finally {
    await page.close();
  }
});

for (const enabled of [false, true]) {
  test(`example drawing token choice is ${enabled}`, async (t) => {
    const ctx = await interviewPage(t, {
      started: false,
      media: true,
      params: enabled ? { exampleDrawing: "1" } : {},
    });
    if (!ctx) return;
    const { page, errors } = ctx;
    try {
      await page.route("**/api/token", (route) =>
        route.fulfill({
          status: 503,
          contentType: "application/json",
          body: JSON.stringify({ error: "Offline test" }),
        }),
      );
      const token = page.waitForRequest(
        (request) => new URL(request.url()).pathname === "/api/token",
      );
      await clearPreflight(page);
      assert.equal(
        (await token).postDataJSON().exampleDrawing,
        enabled ? true : undefined,
      );
      assert.deepEqual(errors, []);
    } finally {
      await page.close();
    }
  });
}
