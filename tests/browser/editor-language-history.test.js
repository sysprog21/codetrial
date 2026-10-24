import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  closeSetupCheck,
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  startStaticServer,
} from "./source.js";

const LANGUAGES = ["python", "javascript", "c", "cpp", "java"];

let browser;
let server;
let base;

before(async () => {
  browser = await launchChromium();
  if (!browser) return;
  ({ server, base } = await startStaticServer({
    // Offer compiled languages without running code or contacting a compiler.
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
  if (server) await new Promise((resolve) => server.close(resolve));
});

async function editorPage(t, platform) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  if (platform) {
    await page.addInitScript((platform) => {
      Object.defineProperty(navigator, "platform", { value: platform });
    }, platform);
  }
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  t.after(() => assert.deepEqual(errors, []));
  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
    waitUntil: "domcontentloaded",
  });
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-input").placeholder !== "",
  );
  await closeSetupCheck(page);
  const editor = page.locator("#editor");
  const language = page.locator("#language-select");
  const select = async (value) => {
    await language.focus();
    await language.selectOption(value);
    assert.equal(await language.inputValue(), value);
  };
  const append = async (value) => {
    await editor.focus();
    await page.keyboard.press("ControlOrMeta+End");
    await page.keyboard.insertText(value);
  };
  const deleteLast = async () => {
    await editor.focus();
    await page.keyboard.press("ControlOrMeta+End");
    await page.keyboard.press("Home");
    await page.keyboard.press("Shift+End");
    await page.keyboard.press("Backspace");
  };
  const undo = async (chord = "ControlOrMeta+Z") => {
    await editor.focus();
    await page.keyboard.press(chord);
  };
  const redo = () => undo("ControlOrMeta+Shift+Z");
  const value = () => editor.inputValue();
  const selection = () =>
    editor.evaluate((node) => [
      node.selectionStart,
      node.selectionEnd,
      node.selectionDirection,
    ]);
  return {
    page,
    editor,
    select,
    append,
    deleteLast,
    undo,
    redo,
    value,
    selection,
  };
}

test("interleaved deletions undo and redo only in the current language", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const { select, append, deleteLast, undo, redo, value } = ctx;
  const pythonStarter = await value();
  await append("\nAAA\nBBB");
  const pythonFull = `${pythonStarter}\nAAA\nBBB`;
  assert.equal(await value(), pythonFull);
  await select("javascript");
  const javascriptStarter = await value();
  await append("\n111\n222");
  const javascriptFull = `${javascriptStarter}\n111\n222`;
  assert.equal(await value(), javascriptFull);
  await deleteLast();
  const javascriptDeleted = `${javascriptStarter}\n111\n`;
  assert.equal(await value(), javascriptDeleted);
  await select("python");
  assert.equal(await value(), pythonFull);
  await deleteLast();
  const pythonDeleted = `${pythonStarter}\nAAA\n`;
  assert.equal(await value(), pythonDeleted);
  await select("javascript");
  await undo();
  const javascriptUndo = await value();
  await select("python");
  await undo();
  assert.deepEqual(
    { javascript: javascriptUndo, python: await value() },
    { javascript: javascriptFull, python: pythonFull },
  );
  await redo();
  assert.equal(await value(), pythonDeleted);
  await select("javascript");
  await redo();
  assert.equal(await value(), javascriptDeleted);
});

test("the original five-step deletion remains undoable after switching", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const { append, deleteLast, undo, redo, value, select } = ctx;
  await append("\nAAA\nBBB");
  const full = await value();
  await deleteLast();
  const deleted = await value();
  await undo();
  assert.equal(await value(), full);
  await deleteLast();
  await select("javascript");
  await select("python");
  await undo();
  assert.equal(await value(), full);
  await redo();
  assert.equal(await value(), deleted);
});

// Platform overrides exercise shortcut routing, not native Cocoa or IME behavior.
for (const platform of ["Win32", "Linux x86_64", "MacIntel"]) {
  test(`${platform} history shortcuts preserve platform editing commands`, async (t) => {
    const ctx = await editorPage(t, platform);
    if (!ctx) return;
    const mac = platform === "MacIntel";
    const modifier = mac ? { metaKey: true } : { ctrlKey: true };
    const key = (key, extra = {}) =>
      ctx.editor.evaluate(
        (node, args) => {
          const event = new KeyboardEvent("keydown", {
            bubbles: true,
            cancelable: true,
            ...args,
          });
          node.dispatchEvent(event);
          return event.defaultPrevented;
        },
        { key, code: `Key${key.toUpperCase()}`, ...modifier, ...extra },
      );
    const before = await ctx.value();
    await ctx.append("\nplatform edit");
    const after = await ctx.value();
    const selection = await ctx.selection();
    for (const extra of [
      { isComposing: true },
      { isComposing: false, keyCode: 229 },
      { ctrlKey: true, metaKey: true },
      { altKey: true },
      mac
        ? { ctrlKey: true, metaKey: false }
        : { ctrlKey: false, metaKey: true },
    ]) {
      assert.equal(await key("z", extra), false, JSON.stringify(extra));
      assert.equal(await ctx.value(), after);
      assert.deepEqual(await ctx.selection(), selection);
    }
    assert.equal(await key("z"), true);
    assert.equal(await ctx.value(), before);
    const undoneSelection = await ctx.selection();
    if (mac) {
      for (const extra of [
        { ctrlKey: true, metaKey: false },
        { metaKey: true },
      ]) {
        assert.equal(
          await key("y", extra),
          false,
          "leave Cocoa yank and Cmd+Y alone",
        );
        assert.equal(await ctx.value(), before);
        assert.deepEqual(await ctx.selection(), undoneSelection);
      }
    }
    for (const command of mac ? ["z"] : ["z", "y"]) {
      assert.equal(
        await key(command, { shiftKey: command === "z", keyCode: 229 }),
        false,
      );
      assert.equal(await ctx.value(), before);
      assert.deepEqual(await ctx.selection(), undoneSelection);
      assert.equal(await key(command, { shiftKey: command === "z" }), true);
      assert.equal(
        await ctx.value(),
        after,
        "ignored commands must preserve redo",
      );
      assert.equal(await key("z"), true);
      assert.equal(await ctx.value(), before);
    }
  });
}

test("Mac history leaves Ctrl+Y and Cmd+Y available without consuming redo", async (t) => {
  const ctx = await editorPage(t, "MacIntel");
  if (!ctx) return;
  const before = await ctx.value();
  await ctx.append("\nkeep redo");
  const after = await ctx.value();
  await ctx.page.keyboard.press("Meta+Z");
  assert.equal(await ctx.value(), before);
  const selection = await ctx.selection();
  for (const modifier of [{ ctrlKey: true }, { metaKey: true }]) {
    const canceled = await ctx.editor.evaluate((node, modifier) => {
      const event = new KeyboardEvent("keydown", {
        key: "y",
        code: "KeyY",
        bubbles: true,
        cancelable: true,
        ...modifier,
      });
      node.dispatchEvent(event);
      return event.defaultPrevented;
    }, modifier);
    assert.equal(canceled, false);
    assert.equal(await ctx.value(), before);
    assert.deepEqual(await ctx.selection(), selection);
  }
  await ctx.page.keyboard.press("Meta+Shift+Z");
  assert.equal(await ctx.value(), after);
});

for (const chord of ["Tab", "Shift+Tab", "Control+/"]) {
  for (const direction of ["forward", "backward"]) {
    test(`${chord} undoes and redoes with a ${direction} selection`, async (t) => {
      const ctx = await editorPage(t);
      if (!ctx) return;
      const { page, editor, append, undo, redo, value, selection } = ctx;
      const start = (await value()).length;
      await append("\n    alpha\n    beta");
      const end = (await value()).length;
      await editor.evaluate(
        (node, [start, end, direction]) => {
          node.setSelectionRange(start + 1, end, direction);
        },
        [start, end, direction],
      );
      const before = { value: await value(), selection: await selection() };
      await page.keyboard.press(chord);
      const after = { value: await value(), selection: await selection() };
      assert.notEqual(after.value, before.value);
      assert.equal(after.selection[2], direction);
      await undo();
      assert.deepEqual(
        { value: await value(), selection: await selection() },
        before,
      );
      await redo();
      assert.deepEqual(
        { value: await value(), selection: await selection() },
        after,
      );
    });
  }
}

test("typing groups until the caret moves", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  await ctx.append("\n");
  const before = await ctx.value();
  await ctx.page.keyboard.type("abc");
  await ctx.undo();
  assert.equal(await ctx.value(), before);
  await ctx.redo();
  assert.equal(await ctx.value(), `${before}abc`);
  await ctx.page.keyboard.press("ArrowLeft");
  await ctx.page.keyboard.type("X");
  await ctx.undo();
  assert.equal(await ctx.value(), `${before}abc`);
});

test("paste and cut each undo and redo as one edit", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const { page, editor, append, value, undo, redo } = ctx;
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await append("\n");
  const before = await value();
  await page.evaluate(() => navigator.clipboard.writeText("pasted\ntext"));
  await page.keyboard.press("ControlOrMeta+V");
  assert.equal(await value(), `${before}pasted\ntext`);
  await undo();
  assert.equal(await value(), before);
  await redo();
  assert.equal(await value(), `${before}pasted\ntext`);
  await editor.evaluate(
    (node, start) => node.setSelectionRange(start, node.value.length),
    before.length,
  );
  await page.keyboard.press("ControlOrMeta+X");
  assert.equal(await value(), before);
  await undo();
  assert.equal(await value(), `${before}pasted\ntext`);
  await redo();
  assert.equal(await value(), before);
});

test("a composition transaction records its committed text once", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  await ctx.append("\n");
  const before = await ctx.value();
  // Synthetic composition events check transaction boundaries. A platform IME
  // still needs a manual check; these events do not emulate its candidate UI.
  await ctx.editor.evaluate((node) => {
    node.dispatchEvent(
      new CompositionEvent("compositionstart", { bubbles: true }),
    );
    for (const data of ["n", "ni", "你"]) {
      node.dispatchEvent(
        new InputEvent("beforeinput", {
          bubbles: true,
          inputType: "insertCompositionText",
          data,
          isComposing: true,
        }),
      );
      node.setRangeText(
        data,
        node.value.lastIndexOf("\n") + 1,
        node.value.length,
        "end",
      );
      node.dispatchEvent(
        new InputEvent("input", {
          bubbles: true,
          inputType: "insertCompositionText",
          data,
          isComposing: true,
        }),
      );
    }
    node.dispatchEvent(
      new CompositionEvent("compositionend", { bubbles: true, data: "你" }),
    );
  });
  assert.equal(await ctx.value(), `${before}你`);
  await ctx.undo();
  assert.equal(await ctx.value(), before);
  await ctx.redo();
  assert.equal(await ctx.value(), `${before}你`);
});

for (const inputType of ["historyUndo", "historyRedo"]) {
  test(`${inputType} during composition stays with the input method`, async (t) => {
    const ctx = await editorPage(t);
    if (!ctx) return;
    await ctx.append("\n");
    const before = await ctx.value();
    const canceled = await ctx.editor.evaluate((node, inputType) => {
      const start = node.value.length;
      const compose = (data, type = "insertCompositionText") => {
        node.dispatchEvent(
          new InputEvent("beforeinput", {
            bubbles: true,
            cancelable: true,
            inputType: type,
            isComposing: true,
            data,
          }),
        );
        node.setRangeText(data, start, node.value.length, "end");
        node.dispatchEvent(
          new InputEvent("input", {
            bubbles: true,
            inputType: type,
            isComposing: true,
            data,
          }),
        );
      };
      node.dispatchEvent(
        new CompositionEvent("compositionstart", { bubbles: true }),
      );
      compose("n");
      const event = new InputEvent("beforeinput", {
        bubbles: true,
        cancelable: true,
        inputType,
        isComposing: true,
      });
      node.dispatchEvent(event);
      node.setRangeText("ni", start, node.value.length, "end");
      node.dispatchEvent(
        new InputEvent("input", {
          bubbles: true,
          inputType,
          isComposing: true,
        }),
      );
      return event.defaultPrevented;
    }, inputType);
    assert.equal(canceled, false);
    assert.equal(await ctx.value(), `${before}ni`);
    await ctx.editor.evaluate((node) => {
      node.setRangeText("你", node.value.length - 2, node.value.length, "end");
      node.dispatchEvent(
        new InputEvent("input", {
          bubbles: true,
          inputType: "insertCompositionText",
          isComposing: true,
        }),
      );
      node.dispatchEvent(
        new CompositionEvent("compositionend", {
          bubbles: true,
          data: "你",
        }),
      );
    });
    await ctx.undo();
    assert.equal(
      await ctx.value(),
      before,
      "the whole composition is one edit",
    );
    await ctx.redo();
    assert.equal(await ctx.value(), `${before}你`);
  });
}

for (const separator of [" ", "\n"]) {
  test(`typing across ${JSON.stringify(separator)} splits undo groups`, async (t) => {
    const ctx = await editorPage(t);
    if (!ctx) return;
    await ctx.append("\n");
    const before = await ctx.value();
    await ctx.page.keyboard.type(`alpha${separator}beta`);
    await ctx.undo();
    assert.equal(await ctx.value(), `${before}alpha${separator}`);
    await ctx.undo();
    assert.equal(await ctx.value(), `${before}alpha`);
    await ctx.undo();
    assert.equal(await ctx.value(), before);
    for (let i = 0; i < 3; i++) await ctx.redo();
    assert.equal(await ctx.value(), `${before}alpha${separator}beta`);
  });
}

test("history input types are canceled and use the page history", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  await ctx.append("\nAAA\nBBB");
  const full = await ctx.value();
  await ctx.deleteLast();
  const deleted = await ctx.value();
  for (const [inputType, expected] of [
    ["historyUndo", full],
    ["historyRedo", deleted],
  ]) {
    const canceled = await ctx.editor.evaluate((node, inputType) => {
      const event = new InputEvent("beforeinput", {
        bubbles: true,
        cancelable: true,
        inputType,
      });
      node.dispatchEvent(event);
      return event.defaultPrevented;
    }, inputType);
    assert.equal(canceled, true);
    assert.equal(await ctx.value(), expected);
  }
});

for (const inputType of ["historyUndo", "historyRedo"]) {
  for (const beforeInput of [false, true]) {
    test(`${inputType} boundary repair preserves a moved selection with ${beforeInput ? "non-cancelable beforeinput" : "input only"}`, async (t) => {
      const ctx = await editorPage(t);
      if (!ctx) return;
      await ctx.append("\nPython edit");
      await ctx.select("javascript");
      const starter = await ctx.value();
      await ctx.editor.focus();
      const expected = await ctx.editor.evaluate(async (node, beforeInput) => {
        // An input-only notification needs the preceding selection event;
        // beforeinput must also work before that event has been delivered.
        if (beforeInput) return [2, 7, "backward"];
        const selected = new Promise((resolve) =>
          node.addEventListener("selectionchange", resolve, { once: true }),
        );
        node.setSelectionRange(2, 7, "backward");
        await selected;
        return [
          node.selectionStart,
          node.selectionEnd,
          node.selectionDirection,
        ];
      }, beforeInput);
      await ctx.editor.evaluate(
        (node, { inputType, beforeInput }) => {
          if (beforeInput) {
            // Notify in the same task, before selectionchange can run.
            node.setSelectionRange(2, 7, "backward");
            node.dispatchEvent(
              new InputEvent("beforeinput", { inputType, bubbles: true }),
            );
          }
          // Model an already-applied native history edit. Synthetic input does
          // not perform a native undo, so change both text and selection here.
          node.value = "native history from another buffer";
          node.dispatchEvent(
            new InputEvent("input", { inputType, bubbles: true }),
          );
        },
        { inputType, beforeInput },
      );
      assert.equal(await ctx.value(), starter);
      assert.deepEqual(await ctx.selection(), expected);
      // Repairing a boundary must not create an edit in this language.
      await ctx.undo();
      await ctx.redo();
      assert.equal(await ctx.value(), starter);
      assert.deepEqual(await ctx.selection(), expected);
    });
  }
}

test("a drop input is recorded with its original selection", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  await ctx.append("\n");
  const before = await ctx.value();
  // Exercise the drop input path without pretending dispatchEvent performs a
  // trusted operating-system drag. Native dragging is a separate manual check.
  await ctx.editor.evaluate((node) => {
    node.dispatchEvent(
      new InputEvent("beforeinput", {
        bubbles: true,
        cancelable: true,
        inputType: "insertFromDrop",
        data: "dropped",
      }),
    );
    node.setRangeText("dropped", node.selectionStart, node.selectionEnd, "end");
    node.dispatchEvent(
      new InputEvent("input", {
        bubbles: true,
        inputType: "insertFromDrop",
        data: "dropped",
      }),
    );
  });
  assert.equal(await ctx.value(), `${before}dropped`);
  await ctx.undo();
  assert.equal(await ctx.value(), before);
  await ctx.redo();
  assert.equal(await ctx.value(), `${before}dropped`);
});

for (const { name, before, expected, source, target } of [
  {
    name: "down",
    before: "START\nDRAG\n\n",
    expected: "START\n\nDRAG\n",
    source: 1,
    target: 2,
  },
  {
    name: "up",
    before: "START\n\nDRAG\n",
    expected: "START\nDRAG\n\n",
    source: 2,
    target: 1,
  },
  {
    name: "from line 11 to 13",
    before: "HEAD\n".repeat(10) + "DRAG\n\n\n\n",
    expected: "HEAD\n".repeat(10) + "\n\nDRAG\n\n",
    source: 10,
    target: 12,
  },
  {
    name: "onto the last empty line",
    before: "START\nDRAG\n",
    expected: "START\n\nDRAG",
    source: 1,
    target: 2,
  },
]) {
  test(`a native drag ${name} preserves empty lines and undoes and redoes as one action`, async (t) => {
    const ctx = await editorPage(t);
    if (!ctx) return;
    const { page, editor, value, undo, redo } = ctx;
    await editor.fill(before);
    const points = await editor.evaluate(
      (node, { source, target }) => {
        const start = node.value.indexOf("DRAG");
        node.focus();
        node.setSelectionRange(start, start + 4);
        node.scrollTop = 0;
        const rect = node.getBoundingClientRect();
        const style = getComputedStyle(node);
        const canvas = document.createElement("canvas");
        const context = canvas.getContext("2d");
        context.font = style.font;
        const lineHeight = parseFloat(style.lineHeight);
        const x =
          rect.left +
          parseFloat(style.paddingLeft) +
          context.measureText("DR").width;
        return {
          x,
          sourceY:
            rect.top +
            parseFloat(style.paddingTop) +
            (source + 0.5) * lineHeight,
          targetY:
            rect.top +
            parseFloat(style.paddingTop) +
            (target + 0.5) * lineHeight,
        };
      },
      { source, target },
    );
    await editor.evaluate((node) => {
      node.dataset.sawDrop = "false";
      node.addEventListener(
        "drop",
        () => {
          node.dataset.sawDrop = "true";
        },
        { once: true },
      );
    });
    await page.mouse.move(points.x, points.sourceY);
    await page.mouse.down();
    await page.mouse.move(points.x + 5, points.sourceY, { steps: 5 });
    await page.mouse.move(points.x, points.targetY, { steps: 20 });
    await page.mouse.up();
    assert.equal(await editor.getAttribute("data-saw-drop"), "true");
    const after = await value();
    assert.equal(
      after,
      expected,
      "the move must preserve every unselected newline",
    );
    assert.equal(after.split("\n").length, before.split("\n").length);
    await undo();
    assert.equal(
      await value(),
      before,
      "one undo must restore the entire move",
    );
    await redo();
    assert.equal(await value(), after, "one redo must repeat the entire move");
  });
}

test("history shortcuts support non-Latin keys without stealing printable layout keys", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const before = await ctx.value();
  await ctx.append("\nkeyboard edit");
  const after = await ctx.value();
  const key = (key, code, extra = {}) =>
    ctx.editor.evaluate(
      (node, args) => {
        const event = new KeyboardEvent("keydown", {
          bubbles: true,
          cancelable: true,
          ctrlKey: true,
          ...args,
        });
        node.dispatchEvent(event);
        return event.defaultPrevented;
      },
      { key, code, ...extra },
    );
  assert.equal(await key(";", "KeyZ"), false, "Dvorak punctuation is not Undo");
  assert.equal(await ctx.value(), after);
  assert.equal(await key("я", "KeyZ"), true);
  assert.equal(await ctx.value(), before);
  assert.equal(await key("Я", "KeyZ", { shiftKey: true }), true);
  assert.equal(await ctx.value(), after);
  assert.equal(await key("z", "KeyZ", { altKey: true }), false);
  assert.equal(await key("z", "KeyZ", { isComposing: true }), false);
  assert.equal(await ctx.value(), after);
});

test("history shortcuts and menu events leave disabled and read-only buffers alone", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const before = await ctx.value();
  await ctx.append("\nkeep this");
  const after = await ctx.value();
  for (const attribute of ["disabled", "readonly"]) {
    await ctx.editor.evaluate((node, attribute) => {
      node.setAttribute(attribute, "");
      for (const inputType of ["historyUndo", "historyRedo"]) {
        node.dispatchEvent(
          new KeyboardEvent("keydown", {
            key: "z",
            ctrlKey: true,
            shiftKey: inputType === "historyRedo",
            bubbles: true,
            cancelable: true,
          }),
        );
        node.dispatchEvent(
          new InputEvent("beforeinput", {
            inputType,
            bubbles: true,
            cancelable: true,
          }),
        );
      }
    }, attribute);
    assert.equal(await ctx.value(), after);
    await ctx.editor.evaluate(
      (node, attribute) => node.removeAttribute(attribute),
      attribute,
    );
  }
  await ctx.undo();
  assert.equal(
    await ctx.value(),
    before,
    "blocked commands did not move the history cursor",
  );
});

test("undo and redo at a language's history boundaries never fall through", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  await ctx.append("\nPython edit");
  await ctx.select("javascript");
  const starter = await ctx.value();
  const commands = await ctx.editor.evaluate((node) => {
    const results = [];
    for (const inputType of ["historyUndo", "historyRedo"]) {
      const event = new InputEvent("beforeinput", {
        inputType,
        bubbles: true,
        cancelable: true,
      });
      node.dispatchEvent(event);
      results.push(event.defaultPrevented);
    }
    for (const shiftKey of [false, true]) {
      const event = new KeyboardEvent("keydown", {
        key: "z",
        ctrlKey: true,
        shiftKey,
        bubbles: true,
        cancelable: true,
      });
      node.dispatchEvent(event);
      results.push(event.defaultPrevented);
    }
    return results;
  });
  assert.deepEqual(commands, [true, true, true, true]);
  assert.equal(await ctx.value(), starter);
});

test("interleaved edits keep Python, JavaScript, C, C++ and Java histories independent", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  const { page, editor, select, append, undo, redo, value, selection } = ctx;
  const buffers = new Map();
  for (const name of LANGUAGES) {
    await select(name);
    assert.equal(
      await page
        .locator(`#language-select option[value="${name}"]`)
        .isDisabled(),
      false,
    );
    const marker = name === "python" ? "#" : "//";
    await append(`\n${marker} ${name} kept\n${marker} ${name} deleted`);
    buffers.set(name, { full: await value() });
  }
  for (const name of ["java", "python", "cpp", "javascript", "c"]) {
    await select(name);
    const buffer = buffers.get(name);
    assert.equal(await value(), buffer.full);
    await editor.focus();
    await editor.evaluate((node) =>
      node.setSelectionRange(
        node.value.lastIndexOf("\n") + 1,
        node.value.length,
        "backward",
      ),
    );
    buffer.beforeSelection = await selection();
    await page.keyboard.press("Backspace");
    buffer.deleted = await value();
    buffer.afterSelection = await selection();
    assert.equal(
      buffer.deleted,
      buffer.full.slice(0, buffer.full.lastIndexOf("\n") + 1),
    );
  }
  for (const name of ["cpp", "java", "c", "python", "javascript"]) {
    await select(name);
    const buffer = buffers.get(name);
    assert.equal(await value(), buffer.deleted);
    await undo();
    assert.equal(
      await value(),
      buffer.full,
      `${name} undo restores only its deleted line`,
    );
    assert.deepEqual(
      await selection(),
      buffer.beforeSelection,
      `${name} undo restores its selection`,
    );
  }
  for (const name of ["javascript", "cpp", "python", "c", "java"]) {
    await select(name);
    const buffer = buffers.get(name);
    assert.equal(await value(), buffer.full);
    await redo();
    assert.equal(
      await value(),
      buffer.deleted,
      `${name} redo survives other languages' undo and redo`,
    );
    assert.deepEqual(await selection(), buffer.afterSelection);
  }
});

for (const [index, name] of LANGUAGES.entries()) {
  test(`${name} indentation and comments remain undoable after a language switch`, async (t) => {
    const ctx = await editorPage(t);
    if (!ctx) return;
    const { page, editor, select, undo, redo, value, selection } = ctx;
    await select(name);
    const before = `${await value()}\n    first\n    second`;
    const other = LANGUAGES[(index + 1) % LANGUAGES.length];
    for (const chord of ["Tab", "Shift+Tab", "Control+/"]) {
      await editor.fill(before);
      await editor.evaluate((node) =>
        node.setSelectionRange(
          node.value.lastIndexOf("\n    first") + 1,
          node.value.length,
          "backward",
        ),
      );
      const beforeSelection = await selection();
      await page.keyboard.press(chord);
      const after = await value();
      const afterSelection = await selection();
      assert.notEqual(after, before, `${name} ${chord} edits the buffer`);
      assert.equal(afterSelection[2], "backward");
      await select(other);
      await select(name);
      await undo();
      assert.equal(await value(), before, `${name} ${chord} undo`);
      assert.deepEqual(await selection(), beforeSelection);
      await redo();
      assert.equal(await value(), after, `${name} ${chord} redo`);
      assert.deepEqual(await selection(), afterSelection);
    }
  });
}

test("paired brackets and auto-indented Enter share the language history", async (t) => {
  const ctx = await editorPage(t);
  if (!ctx) return;
  await ctx.append("\n");
  const initial = await ctx.value();
  await ctx.page.keyboard.type("(");
  assert.equal(await ctx.value(), `${initial}()`);
  await ctx.page.keyboard.press("Enter");
  const indented = await ctx.value();
  assert.notEqual(indented, `${initial}()`);
  await ctx.select("javascript");
  await ctx.select("python");
  await ctx.undo();
  assert.equal(await ctx.value(), `${initial}()`);
  await ctx.undo();
  assert.equal(await ctx.value(), initial);
  await ctx.redo();
  assert.equal(await ctx.value(), `${initial}()`);
  await ctx.redo();
  assert.equal(await ctx.value(), indented);
});
