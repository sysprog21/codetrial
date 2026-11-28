import { test } from "node:test";
import assert from "node:assert/strict";
import {
  FINAL_CODE_NOT_SAVED,
  reportMarkup,
  reportMarkdown,
  mountFinalCodeCopy,
} from "../../web/render.js";
import { launchChromium, startStaticServer } from "./source.js";
import { mountCopyButton } from "../../web/copy.js";

const session = (code) => ({
  report: { incomplete: true, summary: "Interview ended.", hintsUsed: 0 },
  problemTitle: "Example",
  language: "python",
  code,
});

function controls() {
  const attributes = new Map();
  const button = {
    addEventListener: (_, listener) => {
      button.click = listener;
    },
    setAttribute: (name, value) => attributes.set(name, value),
    removeAttribute: (name) => attributes.delete(name),
  };
  const status = { textContent: "" };
  const root = {
    querySelector: (selector) =>
      ({
        "[data-copy-code]": button,
        "[data-copy-code-status]": status,
      })[selector] ?? null,
  };
  return { root, button, status, attributes };
}

test("copy controls appear only for available editor code", () => {
  assert.match(reportMarkup(session("print(1)")), /data-copy-code>Copy code/);
  for (const code of ["", " \n\t", undefined, null, FINAL_CODE_NOT_SAVED]) {
    assert.doesNotMatch(reportMarkup(session(code)), /data-copy-code/);
  }
  assert.doesNotMatch(
    reportMarkup({
      ...session("print(1)"),
      board: "data:image/png;base64,YQ==",
    }),
    /data-copy-code/,
  );
  mountFinalCodeCopy({ querySelector: () => null }, undefined);
});

test("unsaved code uses the same placeholder in the report and download", () => {
  assert.ok(
    reportMarkup(session(FINAL_CODE_NOT_SAVED)).includes(FINAL_CODE_NOT_SAVED),
  );
  assert.ok(reportMarkdown(session(null)).includes(FINAL_CODE_NOT_SAVED));
});

test("copy updates the status emitted by reportMarkup in a browser", async (t) => {
  const browser = await launchChromium();
  if (!browser) return t.skip("playwright chromium unavailable");
  let server;
  try {
    const hosted = await startStaticServer();
    server = hosted.server;
    const page = await browser.newPage();
    await page.goto(hosted.base);
    const code = "\tprint('<&>')  \r\n";
    const result = await page.evaluate(async (code) => {
      const { reportMarkup, mountFinalCodeCopy } = await import("/render.js");
      const root = document.createElement("main");
      root.innerHTML = reportMarkup({
        report: { incomplete: true, summary: "Interview ended.", hintsUsed: 0 },
        problemTitle: "Example",
        language: "python",
        code,
      });
      document.body.replaceChildren(root);
      const button = root.querySelector("[data-copy-code]");
      const status = root.querySelector("[data-copy-code-status]");
      const writes = [];
      let finish;
      mountFinalCodeCopy(root, code, {
        writeText: (text) => {
          writes.push(text);
          return new Promise((resolve) => {
            finish = resolve;
          });
        },
      });
      const originalCode = root.querySelector("pre").textContent;
      button.focus();
      button.click();
      const pending = status.textContent;
      button.click();
      finish();
      await Promise.resolve();
      const success = status.textContent;
      const enabled = !button.hasAttribute("aria-disabled");
      const focused = document.activeElement === button;
      return {
        writes,
        pending,
        success,
        enabled,
        focused,
        unchanged: root.querySelector("pre").textContent === originalCode,
      };
    }, code);
    assert.deepEqual(result, {
      writes: [code],
      pending: "Copying...",
      success: "Copied!",
      enabled: true,
      focused: true,
      unchanged: true,
    });
  } finally {
    await browser.close();
    if (server) await new Promise((resolve) => server.close(resolve));
  }
});

test("copy preserves original whitespace, line endings and HTML characters", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const code = "\tif x < 2:\r\n    print('&<>')  \r\n\r\n";
  const original = reportMarkup(session(code));
  const view = controls();
  const writes = [];
  mountFinalCodeCopy(view.root, code, {
    writeText: async (text) => writes.push(text),
  });
  await view.button.click();
  assert.deepEqual(writes, [code]);
  assert.equal(view.status.textContent, "Copied!");
  assert.equal(view.attributes.has("aria-disabled"), false);
  assert.equal(reportMarkup(session(code)), original);
  t.mock.timers.tick(2000);
  assert.equal(view.status.textContent, "");
});

test("repeated copies restart the success feedback timer", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const view = controls();
  const writes = [];
  mountFinalCodeCopy(view.root, "code", {
    writeText: async (text) => writes.push(text),
  });

  await view.button.click();
  t.mock.timers.tick(1500);
  await view.button.click();
  assert.deepEqual(writes, ["code", "code"]);
  t.mock.timers.tick(1000);
  assert.equal(view.status.textContent, "Copied!");
  t.mock.timers.tick(999);
  assert.equal(view.status.textContent, "Copied!");
  t.mock.timers.tick(1);
  assert.equal(view.status.textContent, "");
});

test("clipboard failure and unavailable API show an error and allow retry", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  for (const clipboard of [
    undefined,
    {},
    {
      writeText: () => {
        throw new Error("denied");
      },
    },
    {
      writeText: async () => {
        throw new Error("denied");
      },
    },
  ]) {
    const view = controls();
    mountFinalCodeCopy(view.root, "code", clipboard);
    await view.button.click();
    assert.match(view.status.textContent, /Could not copy code.*manually/);
    assert.equal(view.attributes.has("aria-disabled"), false);
  }
  const view = controls();
  let reject = true;
  mountFinalCodeCopy(view.root, "code", {
    writeText: async () => {
      if (reject) throw new Error("denied");
    },
  });
  await view.button.click();
  reject = false;
  await view.button.click();
  assert.equal(view.status.textContent, "Copied!");
});

test("a pending copy ignores duplicate clicks", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const view = controls();
  let finish;
  let calls = 0;
  mountFinalCodeCopy(view.root, "code", {
    writeText: () => {
      calls++;
      return new Promise((resolve) => {
        finish = resolve;
      });
    },
  });
  const copying = view.button.click();
  assert.equal(view.status.textContent, "Copying...");
  assert.equal(view.attributes.get("aria-disabled"), "true");
  await view.button.click();
  assert.equal(calls, 1);
  finish();
  await copying;
  assert.equal(view.status.textContent, "Copied!");
});

for (const rejectOld of [false, true]) {
  for (const rejectNew of [false, true]) {
    test(`invalidating a pending ${rejectOld ? "failure" : "success"} preserves a newer ${rejectNew ? "failure" : "success"}`, async (t) => {
      t.mock.timers.enable({ apis: ["setTimeout"] });
      const view = controls();
      const writes = [];
      const completions = [];
      let text = "old results";
      const invalidate = mountCopyButton(
        view.button,
        view.status,
        () => text,
        "Copy failed.",
        {
          writeText: (value) => {
            writes.push(value);
            return new Promise((resolve, reject) => {
              completions.push({ resolve, reject });
            });
          },
        },
      );
      const oldCopy = view.button.click();
      await view.button.click();
      assert.deepEqual(writes, ["old results"]);
      invalidate();
      assert.equal(view.status.textContent, "");
      assert.equal(view.attributes.has("aria-disabled"), false);
      text = "new results";
      const newCopy = view.button.click();
      assert.deepEqual(writes, ["old results", "new results"]);
      if (rejectOld) completions[0].reject(new Error("old write denied"));
      else completions[0].resolve();
      await oldCopy;
      assert.equal(view.status.textContent, "Copying...");
      assert.equal(view.attributes.get("aria-disabled"), "true");
      await view.button.click();
      assert.deepEqual(writes, ["old results", "new results"]);
      if (rejectNew) completions[1].reject(new Error("new write denied"));
      else completions[1].resolve();
      await newCopy;
      assert.equal(
        view.status.textContent,
        rejectNew ? "Copy failed." : "Copied!",
      );
      assert.equal(view.attributes.has("aria-disabled"), false);
    });
  }
}
