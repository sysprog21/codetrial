import { test } from "node:test";
import assert from "node:assert/strict";
import { reportMarkup, mountFinalCodeCopy } from "../../web/render.js";

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
      selector === "[data-copy-code]" ? button : status,
  };
  return { root, button, status, attributes };
}

test("copy controls appear only for available editor code", () => {
  assert.match(reportMarkup(session("print(1)")), /data-copy-code>Copy code/);
  for (const code of [
    "",
    " \n\t",
    undefined,
    null,
    "(final code was not saved)",
  ]) {
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
