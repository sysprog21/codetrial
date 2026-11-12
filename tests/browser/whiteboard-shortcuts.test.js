import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  exerciseTextDraft,
  exerciseTextBaseline,
} from "./board-text-fixture.js";
import {
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  read,
  startStaticServer,
} from "./source.js";
import { MAX_STROKES, MAX_STROKES_MESSAGE } from "../../web/whiteboard.js";

let browser;
let server;
let base;

// Only the served copy exposes state setup. Pointer, key, paint, debounce,
// stream and replay handlers remain the production ones; no live room is used.
const setup = `
globalThis.boardTest = {
  uploads: [],
  start() {
    nodes.audioCheck.hidden = true;
    state.phase = "live";
    state.interviewId = "shortcut-test";
    state.connected = true;
    state.room = { localParticipant: { streamBytes: async (options) => {
      const upload = { options, bytes: 0, chunks: [], closed: false };
      this.uploads.push(upload);
      return {
        write: async (bytes) => {
          upload.bytes += bytes.length;
          upload.chunks.push(bytes.slice());
        },
        close: async () => { upload.closed = true; },
      };
    } } };
    board.ready = true;
    paintBoard();
  },
  inspect() {
    return {
      strokes: board.model.strokes(),
      drawing: board.model.isDrawing(),
      canRedo: board.model.canRedo(),
      focus: document.activeElement?.id,
      image: nodes.board.toDataURL(),
      uploads: this.uploads.map(({ chunks, ...upload }) => upload),
    };
  },
  settling() {
    return Boolean(board.settle);
  },
  fill(count) {
    while (board.model.strokeCount() < count)
      if (!board.model.push("#101418", 3, [10, 10, 20, 20]))
        throw new Error("the board refused a stroke below " + count);
    paintBoard();
  },
  strokeCount() {
    return board.model.strokeCount();
  },
  waitUploads() {
    return board.publishing;
  },
  jpeg() {
    return nodes.board.toDataURL("image/jpeg", BOARD_JPEG_QUALITY);
  },
  lock(kind) {
    board.ready = kind !== "not-ready";
    state.phase = kind === "before-start" ? "preflight" : kind === "ended" ? "report" : "live";
    frameworkRound = kind === "behavioral" ? "behavioral" : "coding";
    paintBoard();
  },
};
`;

before(async () => {
  browser = await launchChromium();
  if (!browser) return;
  ({ server, base } = await startStaticServer({
    runtimeConfig: DEFAULT_RUNTIME_CONFIG.replace(
      "CODETRIAL_RECORDING_ENABLED = false",
      "CODETRIAL_RECORDING_ENABLED = true",
    ),
    handle(request, response, url) {
      if (url.pathname === "/interview.js") {
        response.setHeader("content-type", "text/javascript");
        response.end(read("web/interview.js") + setup);
        return true;
      }
      if (url.pathname === "/api/session") {
        response.setHeader("content-type", "application/json");
        response.end('{"signedIn":false}');
        return true;
      }
      return false;
    },
  }));
});

after(async () => {
  await browser?.close();
  await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
});

async function boardPage(t, mac = false, deviceScaleFactor = 1) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
    deviceScaleFactor,
  });
  t.after(() => page.close());
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  t.after(() => assert.deepEqual(errors, []));
  await page.addInitScript((mac) => {
    Object.defineProperty(navigator, "platform", {
      value: mac ? "MacIntel" : "Linux x86_64",
    });
  }, mac);
  const events = [];
  await page.route("**/*", async (route) => {
    const url = route.request().url();
    if (url === `${base}/api/interviews/shortcut-test/events`) {
      events.push(...route.request().postDataJSON().events);
      await page.evaluate((count) => {
        globalThis.recordedBoardOps = count;
      }, ops(events).length);
      return route.fulfill({ status: 200, body: "{}" });
    }
    return url.startsWith(base + "/") ? route.continue() : route.abort();
  });
  await page.goto(
    `${base}/interview.html?problem=chargeback-pair-match&mode=whiteboard`,
  );
  await page.waitForFunction(() => Boolean(globalThis.boardTest));
  await page.evaluate(() => {
    globalThis.boardTest.start();
    window.addEventListener("keydown", (event) => {
      globalThis.keyTaken = event.defaultPrevented;
    });
  });
  return { page, events };
}

const inspect = (page) => page.evaluate(() => globalThis.boardTest.inspect());
test("whiteboard confirmation preserves draft glyph positions", async (t) => {
  const session = await boardPage(t);
  if (!session) return;
  await exerciseTextBaseline(
    session.page,
    session.page.locator("#board"),
    session.page.locator("#board-text"),
  );
});
test("text placement and serialized rendering survive a high-DPI viewport", async (t) => {
  const session = await boardPage(t, false, 2);
  if (!session) return;
  const { page } = session;
  await exerciseTextDraft(
    page,
    page.locator("#board"),
    page.locator("#board-text"),
  );
  const live = await inspect(page);
  await page.setViewportSize({ width: 1000, height: 800 });
  const replay = await page.evaluate(async (strokes) => {
    const { createBoard, applyOp, drawBoard } = await import("/whiteboard.js");
    const board = createBoard();
    for (const { kind, ...label } of strokes) {
      if (kind !== "text") throw new Error("expected a text label");
      applyOp(board, JSON.parse(JSON.stringify({ op: "text", ...label })));
    }
    const canvas = document.createElement("canvas");
    canvas.width = 1600;
    canvas.height = 1000;
    drawBoard(canvas.getContext("2d"), board.strokes());
    return canvas.toDataURL();
  }, live.strokes);
  assert.equal(replay, live.image);
});
test("whiteboard text drafts preserve history, share images and record multiline labels", async (t) => {
  const session = await boardPage(t);
  if (!session) return;
  const { page, events } = session;
  const blank = await exerciseTextDraft(
    page,
    page.locator("#board"),
    page.locator("#board-text"),
  );
  const committed = await inspect(page);
  assert.equal(committed.strokes.length, 1);
  assert.equal(committed.strokes[0].text, "head\ntail");
  assert.equal(committed.strokes[0].color, "#c2261b");
  const uploads = await expectBoardUpload(page, 0, 1);
  await page.waitForFunction(() => globalThis.recordedBoardOps >= 1);
  assert.equal(ops(events).filter((op) => op.op === "text").length, 1);
  await page.locator("#board-undo").click();
  assert.equal((await inspect(page)).image, blank);
  assert.equal((await inspect(page)).canRedo, true);
  await page.locator("#board-text").click();
  await page.locator("#board").click({ position: { x: 50, y: 50 } });
  await page.getByRole("textbox", { name: "Label text" }).fill("discard me");
  await page.getByRole("textbox", { name: "Label text" }).press("Escape");
  assert.equal((await inspect(page)).canRedo, true);
  await page.locator("#board-redo").click();
  assert.equal((await inspect(page)).image, committed.image);
  await expectBoardUpload(page, uploads, 1);
});
const scrollPosition = (page) =>
  page.evaluate(() => {
    const panel = document.querySelector("#board-panel");
    const surface = document.querySelector(".board-surface");
    return [
      window.scrollX,
      window.scrollY,
      panel.scrollLeft,
      panel.scrollTop,
      surface.scrollLeft,
      surface.scrollTop,
    ];
  });
const ops = (events) =>
  events
    .filter((event) => event.kind === "board")
    .flatMap((event) => event.payload.ops);

async function draw(page, offset = 0, dot = false) {
  const canvas = page.locator("#board");
  await canvas.scrollIntoViewIfNeeded();
  const box = await canvas.boundingBox();
  await page.mouse.move(box.x + 40 + offset, box.y + 40);
  await page.mouse.down();
  if (!dot)
    await page.mouse.move(box.x + 90 + offset, box.y + 70, { steps: 3 });
  await page.mouse.up();
}

async function expectBoardUpload(page, previous, strokes) {
  await page.waitForFunction((previous) => {
    const test = globalThis.boardTest;
    return (
      !test.settling() &&
      test.uploads.length > previous &&
      test.uploads.slice(previous).every((upload) => upload.closed)
    );
  }, previous);
  await page.evaluate(() => globalThis.boardTest.waitUploads());
  const sent = await page.evaluate(async () => {
    const upload = globalThis.boardTest.uploads.at(-1);
    const image = await new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result);
      reader.onerror = () => reject(reader.error);
      reader.readAsDataURL(new Blob(upload.chunks, { type: "image/jpeg" }));
    });
    return {
      count: globalThis.boardTest.uploads.length,
      options: upload.options,
      bytes: upload.bytes,
      image,
      expected: globalThis.boardTest.jpeg(),
    };
  });
  assert.ok(sent.bytes > 0);
  assert.equal(sent.options.totalSize, sent.bytes);
  assert.equal(sent.options.topic, "board_image");
  assert.equal(sent.options.mimeType, "image/jpeg");
  assert.equal(sent.options.attributes.strokes, String(strokes));
  assert.equal(sent.image, sent.expected);
  return sent.count;
}

for (const mac of [false, true]) {
  for (const redo of mac
    ? ["Meta+Shift+z"]
    : ["Control+y", "Control+Shift+z"]) {
    test(`${mac ? "Cmd" : "Ctrl"} shortcuts undo and redo through paint, sync and replay (${redo})`, async (t) => {
      const ctx = await boardPage(t, mac);
      if (!ctx) return;
      const { page, events } = ctx;
      await draw(page);
      await draw(page, 80, true);
      const drawn = await inspect(page);
      assert.equal(drawn.strokes.length, 2);
      let uploads = await expectBoardUpload(page, 0, 2);
      await page.waitForFunction(() => globalThis.recordedBoardOps >= 2);
      assert.deepEqual(
        ops(events).map((op) => op.op),
        ["stroke", "stroke"],
      );
      // Focus separately so a missing drawing-focus change cannot hide a
      // missing keyboard binding in the pre-implementation failure.
      await page.locator("#board-undo").focus();
      await page.keyboard.press(mac ? "Meta+z" : "Control+z");
      const undone = await inspect(page);
      assert.equal(undone.strokes.length, 1);
      assert.notEqual(undone.image, drawn.image);
      assert.equal(undone.focus, "board-undo");
      assert.equal(await page.evaluate(() => globalThis.keyTaken), true);
      uploads = await expectBoardUpload(page, uploads, 1);
      await page.waitForFunction(() => globalThis.recordedBoardOps >= 3);
      assert.deepEqual(
        ops(events).map((op) => op.op),
        ["stroke", "stroke", "undo"],
      );
      await page.keyboard.press(redo);
      assert.deepEqual((await inspect(page)).strokes, drawn.strokes);
      assert.equal((await inspect(page)).image, drawn.image);
      await expectBoardUpload(page, uploads, 2);
      // Observe the batch itself: a request wait attached after a slow draw
      // could miss a batch that already arrived on a loaded CI runner.
      await page.waitForFunction(() => globalThis.recordedBoardOps >= 4);
      assert.deepEqual(
        ops(events).map((op) => op.op),
        ["stroke", "stroke", "undo", "redo"],
      );
      const replay = await page.evaluate(async (operations) => {
        const { createBoard, applyOp } = await import("/whiteboard.js");
        const model = createBoard();
        for (const op of operations) applyOp(model, op);
        return model.strokes();
      }, ops(events));
      assert.deepEqual(replay, drawn.strokes);
      for (const [id, chord] of [
        ["undo", mac ? "Meta+Z" : "Control+Z"],
        ["redo", mac ? "Meta+Shift+Z" : "Control+Y Control+Shift+Z"],
      ]) {
        assert.equal(
          await page.locator(`#board-${id}`).getAttribute("aria-keyshortcuts"),
          chord,
        );
        assert.ok(
          (await page.locator(`#board-${id}`).getAttribute("title")).includes(
            mac ? "Cmd" : "Ctrl",
          ),
        );
        if (id === "redo" && !mac) {
          assert.ok(
            (await page.locator("#board-redo").getAttribute("title")).includes(
              "Ctrl+Shift+Z",
            ),
          );
        }
      }
    });
  }
  test(`${mac ? "Cmd" : "Ctrl"} shortcuts preserve Latin keys and fall back for non-Latin layouts`, async (t) => {
    const ctx = await boardPage(t, mac);
    if (!ctx) return;
    const { page, events } = ctx;
    await draw(page);
    await draw(page, 80);
    const drawn = (await inspect(page)).strokes;
    const dispatch = (extra) =>
      page.evaluate(
        ({ mac, extra }) => {
          const { target = "#board", handled, ...options } = extra;
          const event = new KeyboardEvent("keydown", {
            key: "\u044f",
            code: "KeyZ",
            ctrlKey: !mac,
            metaKey: mac,
            bubbles: true,
            cancelable: true,
            ...options,
          });
          if (handled) event.preventDefault();
          document.querySelector(target).dispatchEvent(event);
          return event.defaultPrevented;
        },
        { mac, extra },
      );
    assert.equal(await dispatch({ key: "w", code: "KeyZ" }), false);
    assert.deepEqual((await inspect(page)).strokes, drawn);
    assert.equal(await dispatch({ key: "Z", code: "KeyW" }), true);
    assert.equal((await inspect(page)).strokes.length, 1);
    assert.equal(
      await dispatch({
        key: mac ? "Z" : "Y",
        code: mac ? "KeyY" : "KeyZ",
        shiftKey: mac,
      }),
      true,
    );
    assert.deepEqual((await inspect(page)).strokes, drawn);
    if (!mac) {
      assert.equal(await dispatch({}), true);
      assert.equal(
        await dispatch({ key: "\u042f", code: "KeyZ", shiftKey: true }),
        true,
      );
      assert.deepEqual((await inspect(page)).strokes, drawn);
    }
    assert.equal(await dispatch({}), true);
    assert.equal((await inspect(page)).strokes.length, 1);
    assert.equal(
      await dispatch({
        key: mac ? "\u042f" : "\u043d",
        code: mac ? "KeyZ" : "KeyY",
        shiftKey: mac,
      }),
      true,
    );
    assert.deepEqual((await inspect(page)).strokes, drawn);
    for (const extra of [
      { repeat: true },
      { isComposing: true },
      { keyCode: 229 },
      { altKey: true },
      { [mac ? "ctrlKey" : "metaKey"]: true },
      { handled: true },
      { code: "KeyX" },
      { code: "" },
    ]) {
      assert.equal(await dispatch(extra), Boolean(extra.handled));
      assert.deepEqual((await inspect(page)).strokes, drawn);
    }
    await page.evaluate(() => {
      const field = document.createElement("input");
      field.id = "layout-text";
      document.querySelector(".board-toolbar").append(field);
      field.focus();
    });
    assert.equal(await dispatch({ target: "#layout-text" }), false);
    await page.locator("#end").focus();
    assert.equal(await dispatch({ target: "#end" }), false);
    assert.deepEqual((await inspect(page)).strokes, drawn);
    await page.waitForFunction(
      (count) => globalThis.recordedBoardOps >= count,
      mac ? 6 : 8,
    );
    assert.deepEqual(
      ops(events).map((op) => op.op),
      [
        "stroke",
        "stroke",
        "undo",
        "redo",
        "undo",
        "redo",
        ...(!mac ? ["undo", "redo"] : []),
      ],
    );
  });

  test(`${mac ? "Cmd" : "Ctrl"} shortcuts ignore ASCII punctuation and named keys`, async (t) => {
    const ctx = await boardPage(t, mac);
    if (!ctx) return;
    const { page, events } = ctx;
    await draw(page);
    await draw(page, 80);
    await page.keyboard.press(mac ? "Meta+z" : "Control+z");
    const before = await inspect(page);
    assert.equal(before.strokes.length, 1);
    assert.equal(before.canRedo, true);

    const keys = [
      ...Array.from({ length: 95 }, (_, i) => String.fromCharCode(0x20 + i)),
      "Dead",
      "Unidentified",
      "Enter",
      "F1",
      "",
      "\x00",
      "\x1f",
      "\x7f",
    ].filter((key) => !/^[zy]$/i.test(key));
    for (const key of keys) {
      for (const [redo, shift] of mac
        ? [
            [false, false],
            [true, true],
          ]
        : [
            [false, false],
            [true, false],
            [true, true],
          ]) {
        const taken = await page.evaluate(
          ({ key, mac, redo, shift }) => {
            const event = new KeyboardEvent("keydown", {
              key,
              code: redo && !mac && !shift ? "KeyY" : "KeyZ",
              ctrlKey: !mac,
              metaKey: mac,
              shiftKey: shift,
              bubbles: true,
              cancelable: true,
            });
            // Observe the board handler without running unrelated page chords.
            document
              .querySelector("#board-panel")
              .addEventListener("keydown", (event) => event.stopPropagation(), {
                once: true,
              });
            document.querySelector("#board").dispatchEvent(event);
            return event.defaultPrevented;
          },
          { key, mac, redo, shift },
        );
        assert.equal(taken, false, JSON.stringify({ key, redo }));
        const after = await inspect(page);
        assert.deepEqual(after.strokes, before.strokes);
        assert.equal(after.canRedo, before.canRedo);
        assert.equal(after.image, before.image);
        assert.equal(after.focus, before.focus);
      }
    }
    await page.waitForFunction(() => globalThis.recordedBoardOps >= 3);
    assert.deepEqual(
      ops(events).map((op) => op.op),
      ["stroke", "stroke", "undo"],
    );
  });
}

test("an accepted drawing start focuses the canvas without scrolling", async (t) => {
  const ctx = await boardPage(t);
  if (!ctx) return;
  const { page } = ctx;
  // Keep the canvas partially visible, with its center outside the surface,
  // so ordinary focus would scroll it even though the pointer can reach it.
  await page.addStyleTag({
    content: `
      .board-surface { flex: none; width: 240px; height: 180px; }
      #board {
        min-width: 0; width: 200px; height: 125px;
        margin-left: 180px; margin-top: 140px;
      }
    `,
  });
  await page.locator(".board-toolbar button").first().focus();
  const surface = page.locator(".board-surface");
  await surface.scrollIntoViewIfNeeded();
  await surface.evaluate((node) => node.scrollTo(40, 40));
  const box = await page.locator("#board").boundingBox();
  await page.mouse.move(box.x + 20, box.y + 20);
  const before = await scrollPosition(page);
  try {
    await page.mouse.down();
    const started = await inspect(page);
    assert.equal(started.drawing, true);
    assert.equal(started.focus, "board");
    await page.evaluate(() => new Promise(requestAnimationFrame));
    assert.deepEqual(await scrollPosition(page), before);
  } finally {
    await page.mouse.up();
  }
});

for (const mac of [false, true]) {
  test(`${mac ? "Cmd" : "Ctrl"} toolbar edits retain focus until their button is disabled`, async (t) => {
    const ctx = await boardPage(t, mac);
    if (!ctx) return;
    const { page } = ctx;
    for (const offset of [0, 40, 80]) await draw(page, offset, true);

    for (const action of ["undo", "redo"]) {
      const button = page.locator(`#board-${action}`);
      await button.focus();
      const before = await scrollPosition(page);
      for (let press = 1; press <= 3; press++) {
        await page.keyboard.press("Enter");
        const edited = await inspect(page);
        assert.equal(
          edited.strokes.length,
          action === "undo" ? 3 - press : press,
        );
        assert.equal(edited.focus, press === 3 ? "board" : `board-${action}`);
        assert.equal(await button.isDisabled(), press === 3);
        assert.deepEqual(await scrollPosition(page), before);
      }
    }

    await page.locator("#board-clear").focus();
    const before = await scrollPosition(page);
    await page.keyboard.press("Enter");
    assert.equal((await inspect(page)).strokes.length, 0);
    assert.equal((await inspect(page)).focus, "board");
    assert.equal(await page.locator("#board-clear").isDisabled(), true);
    assert.deepEqual(await scrollPosition(page), before);

    await page.locator("#board-undo").focus();
    await page.keyboard.press(mac ? "Meta+z" : "Control+z");
    assert.equal((await inspect(page)).strokes.length, 3);
    assert.equal((await inspect(page)).focus, "board-undo");
    await page.keyboard.press(mac ? "Meta+Shift+z" : "Control+y");
    assert.equal((await inspect(page)).strokes.length, 0);
    assert.equal((await inspect(page)).focus, "board-undo");
    assert.deepEqual(await scrollPosition(page), before);
  });
}

test("Tab skips the canvas; programmatic focus keeps keyboard and button edits usable", async (t) => {
  const ctx = await boardPage(t);
  if (!ctx) return;
  const { page } = ctx;
  await draw(page);
  assert.equal((await inspect(page)).focus, "board");
  await page.locator("#board-clear").focus();
  await page.keyboard.press("Tab");
  assert.notEqual((await inspect(page)).focus, "board");
  await page.locator("#board").focus();
  assert.equal((await inspect(page)).focus, "board");
  assert.equal(
    await page
      .locator("#board")
      .evaluate(
        (canvas) =>
          canvas.matches(":focus-visible") &&
          getComputedStyle(canvas).outlineStyle !== "none",
      ),
    true,
  );
  const before = await scrollPosition(page);
  await page.keyboard.press("Control+z");
  assert.deepEqual(await scrollPosition(page), before);
  assert.equal((await inspect(page)).focus, "board");
  assert.equal(await page.locator("#board-undo").isDisabled(), true);
  await page.keyboard.press("Control+y");
  for (const id of ["clear", "undo", "redo", "undo"]) {
    await page.locator(`#board-${id}`).click();
    const disabled = await page.locator(`#board-${id}`).isDisabled();
    assert.equal(
      (await inspect(page)).focus,
      disabled ? "board" : `board-${id}`,
      id,
    );
  }
  assert.equal((await inspect(page)).strokes.length, 1);
});

test("eraser gestures, undoable clear and new strokes preserve history semantics", async (t) => {
  const ctx = await boardPage(t);
  if (!ctx) return;
  const { page } = ctx;
  await draw(page);
  await page.locator("#board-eraser").click();
  await draw(page);
  const erased = await inspect(page);
  assert.equal(erased.strokes.length, 2);
  await page.keyboard.press("Control+z");
  assert.equal((await inspect(page)).strokes.length, 1);
  await page.keyboard.press("Control+y");
  assert.equal((await inspect(page)).image, erased.image);
  await page.locator("#board-clear").click();
  assert.equal((await inspect(page)).strokes.length, 0);
  await page.keyboard.press("Control+z");
  assert.deepEqual((await inspect(page)).strokes, erased.strokes);
  await page.keyboard.press("Control+y");
  assert.equal((await inspect(page)).strokes.length, 0);
  await page.keyboard.press("Control+z");
  await page.keyboard.press("Control+z");
  await draw(page, 140);
  assert.equal((await inspect(page)).canRedo, false);
  const before = await inspect(page);
  await page.keyboard.press("Control+y");
  assert.deepEqual((await inspect(page)).strokes, before.strokes);
  assert.equal(await page.evaluate(() => globalThis.keyTaken), false);
});

for (const mac of [false, true]) {
  const chords = mac
    ? ["Meta+z", "Meta+Shift+z"]
    : ["Control+z", "Control+y", "Control+Shift+z"];

  test(`${mac ? "Cmd" : "Ctrl"} locked boards and unfinished strokes refuse shortcuts`, async (t) => {
    const ctx = await boardPage(t, mac);
    if (!ctx) return;
    const { page } = ctx;
    await draw(page);
    await draw(page, 80);
    await page.keyboard.press(chords[0]);
    const before = (await inspect(page)).strokes;
    assert.equal(before.length, 1);
    assert.equal((await inspect(page)).canRedo, true);
    for (const kind of ["not-ready", "before-start", "behavioral", "ended"]) {
      await page.evaluate((kind) => globalThis.boardTest.lock(kind), kind);
      await page.locator("#board").focus();
      for (const chord of chords) {
        await page.keyboard.press(chord);
        assert.deepEqual(
          (await inspect(page)).strokes,
          before,
          `${kind}: ${chord}`,
        );
        assert.equal(await page.evaluate(() => globalThis.keyTaken), false);
      }
    }
    await page.evaluate(() => globalThis.boardTest.lock("live"));
    const box = await page.locator("#board").boundingBox();
    await page.mouse.move(box.x + 80, box.y + 80);
    await page.mouse.down();
    const drawing = (await inspect(page)).strokes;
    for (const chord of chords) {
      await page.keyboard.press(chord);
      assert.equal((await inspect(page)).drawing, true);
      assert.deepEqual((await inspect(page)).strokes, drawing);
      assert.equal(await page.evaluate(() => globalThis.keyTaken), false);
    }
    await page.mouse.up();
    assert.equal((await inspect(page)).strokes.length, 2, "no queued edit");
  });

  test(`${mac ? "Cmd" : "Ctrl"} empty history and guarded chords add no operation`, async (t) => {
    const ctx = await boardPage(t, mac);
    if (!ctx) return;
    const { page, events } = ctx;
    await page.locator("#board").focus();
    for (const chord of chords) {
      await page.keyboard.press(chord);
      assert.equal(await page.evaluate(() => globalThis.keyTaken), false);
    }
    await draw(page);
    await draw(page, 80);
    await page.keyboard.press(chords[0]);
    await page.waitForFunction(() => globalThis.recordedBoardOps >= 3);
    assert.deepEqual(
      ops(events).map((op) => op.op),
      ["stroke", "stroke", "undo"],
    );
    const before = (await inspect(page)).strokes;
    assert.equal(before.length, 1);
    assert.equal((await inspect(page)).canRedo, true);
    const bindings = mac
      ? [
          { key: "z", shiftKey: false },
          { key: "z", shiftKey: true },
        ]
      : [
          { key: "z", shiftKey: false },
          { key: "y", shiftKey: false },
          { key: "z", shiftKey: true },
        ];
    for (const binding of bindings) {
      for (const extra of [
        { repeat: true },
        { isComposing: true },
        { keyCode: 229 },
        { altKey: true },
        { [mac ? "ctrlKey" : "metaKey"]: true },
        { key: "x" },
        { handled: true },
      ]) {
        const taken = await page.evaluate(
          ({ mac, binding, extra }) => {
            const { handled, ...options } = extra;
            const event = new KeyboardEvent("keydown", {
              ctrlKey: !mac,
              metaKey: mac,
              bubbles: true,
              cancelable: true,
              ...binding,
              ...options,
            });
            if (handled) event.preventDefault();
            document.querySelector("#board").dispatchEvent(event);
            return event.defaultPrevented;
          },
          { mac, binding, extra },
        );
        assert.equal(taken, Boolean(extra.handled));
        assert.deepEqual((await inspect(page)).strokes, before);
        assert.equal((await inspect(page)).canRedo, true);
      }
    }
    for (const chord of mac
      ? ["Meta+y", "Meta+Shift+y", "Control+z", "Control+Shift+z"]
      : ["Control+Shift+y", "Meta+z", "Meta+Shift+z"]) {
      await page.keyboard.press(chord);
      assert.equal(
        await page.evaluate(() => globalThis.keyTaken),
        false,
        chord,
      );
      assert.deepEqual((await inspect(page)).strokes, before, chord);
      assert.equal((await inspect(page)).canRedo, true);
    }
    const modifier = mac ? "Meta" : "Control";
    await page.keyboard.down(modifier);
    await page.keyboard.down("z");
    await page.keyboard.down("z");
    await page.keyboard.up("z");
    await page.keyboard.up(modifier);
    assert.equal((await inspect(page)).strokes.length, 0);
    assert.equal(await page.evaluate(() => globalThis.keyTaken), false);
    await page.waitForFunction(() => globalThis.recordedBoardOps >= 4);
    assert.deepEqual(
      ops(events).map((op) => op.op),
      ["stroke", "stroke", "undo", "undo"],
    );
  });
}

test("a full board says why the pen and the eraser draw nothing", async (t) => {
  const ctx = await boardPage(t);
  if (!ctx) return;
  const { page } = ctx;
  const status = page
    .getByRole("status")
    .filter({ hasText: MAX_STROKES_MESSAGE });
  const count = () => page.evaluate(() => globalThis.boardTest.strokeCount());
  assert.equal(await status.count(), 0);
  // A live region announces a change only if it was in the accessibility
  // tree before the change, so the empty line may give up its space but not
  // its place there.
  assert.notEqual(
    await page
      .locator("#board-status")
      .evaluate((node) => getComputedStyle(node).display),
    "none",
    "the empty status stays in the accessibility tree",
  );

  await page.evaluate((max) => globalThis.boardTest.fill(max), MAX_STROKES);
  await draw(page);
  await status.waitFor();
  assert.equal(await count(), MAX_STROKES, "the refused stroke left no mark");

  // The same text written again is not a change a screen reader announces,
  // so a repeated refusal empties the line and then sets it, as two updates.
  await page.evaluate(() => {
    const node = document.querySelector("#board-status");
    globalThis.statusUpdates = [];
    new MutationObserver(() =>
      globalThis.statusUpdates.push(node.textContent),
    ).observe(node, { childList: true, characterData: true, subtree: true });
  });
  await draw(page, 40);
  await page.evaluate(
    () =>
      new Promise((frame) =>
        requestAnimationFrame(() => requestAnimationFrame(frame)),
      ),
  );
  assert.deepEqual(await page.evaluate(() => globalThis.statusUpdates), [
    "",
    MAX_STROKES_MESSAGE,
  ]);

  // Keyboard Undo is how most candidates take a stroke back.
  await page.locator("#board").focus();
  await page.keyboard.press("Control+z");
  assert.equal(await count(), MAX_STROKES - 1);
  assert.equal(await status.count(), 0, "keyboard Undo clears the message");

  // Redo fills the board again without refusing anything, so nothing is said
  // until the next stroke is refused.
  await page.keyboard.press("Control+y");
  assert.equal(await count(), MAX_STROKES);
  assert.equal(await status.count(), 0, "a Redo to the ceiling says nothing");
  await draw(page);
  await status.waitFor();

  // The message set a frame after a refusal must not outlive its reason: an
  // Undo and a Redo before that frame leave a full board that refused nothing.
  await page.evaluate(() => {
    const board = document.querySelector("#board");
    const { left, top } = board.getBoundingClientRect();
    board.dispatchEvent(
      new PointerEvent("pointerdown", {
        bubbles: true,
        cancelable: true,
        pointerId: 1,
        pointerType: "mouse",
        isPrimary: true,
        button: 0,
        buttons: 1,
        clientX: left + 60,
        clientY: top + 60,
      }),
    );
    document.querySelector("#board-undo").click();
    document.querySelector("#board-redo").click();
  });
  await page.evaluate(
    () =>
      new Promise((frame) =>
        requestAnimationFrame(() => requestAnimationFrame(frame)),
      ),
  );
  assert.equal(await count(), MAX_STROKES);
  assert.equal(await status.count(), 0, "a stale refusal stays silent");
  await draw(page);
  await status.waitFor();

  // Undo frees one slot, and the message goes with the reason for it.
  await page.locator("#board-undo").click();
  assert.equal(await status.count(), 0, "Undo clears the message");
  await draw(page);
  assert.equal(await count(), MAX_STROKES);
  assert.equal(await status.count(), 0, "an accepted stroke shows nothing");

  // A label refused on a full board reports in the text draft's own line.
  await page.locator("#board-text").click();
  const box = await page.locator("#board").boundingBox();
  await page.mouse.move(box.x + 200, box.y + 200);
  await page.mouse.down();
  await page.mouse.move(box.x + 320, box.y + 260);
  await page.mouse.up();
  await page.locator(".board-text-input").fill("x");
  await page.locator(".board-text-confirm").click();
  assert.match(
    await page.locator(".board-text-error").textContent(),
    /cannot accept more text/,
  );
  assert.equal(await count(), MAX_STROKES);
  assert.equal(
    await page.locator("#board-status").textContent(),
    "",
    "a refused label leaves the board status alone",
  );
  await page.locator(".board-text-cancel").click();

  // An erasure is a stroke, so it meets the same ceiling and the same answer.
  await page.locator("#board-eraser").click();
  await draw(page, 80);
  await status.waitFor();
  assert.equal(await count(), MAX_STROKES);

  // A locked board disables Undo and Clear board, so a message naming them
  // would point at two buttons that cannot work.
  await page.evaluate(() => globalThis.boardTest.lock("behavioral"));
  assert.equal(await count(), MAX_STROKES);
  assert.equal(await status.count(), 0, "a locked board drops the message");
  await page.evaluate(() => globalThis.boardTest.lock("live"));
  await draw(page, 160);
  await status.waitFor();
  await page.locator("#board-clear").click();
  assert.equal(await count(), 0);
  assert.equal(await status.count(), 0, "Clear board clears the message");
});

test("text targets and focus outside the drawing toolbar keep their own undo and redo", async (t) => {
  const ctx = await boardPage(t);
  if (!ctx) return;
  const { page } = ctx;
  await draw(page);
  await draw(page, 80);
  await page.keyboard.press("Control+z");
  assert.equal((await inspect(page)).canRedo, true);
  for (const kind of ["input", "textarea", "editable"]) {
    await page.evaluate((kind) => {
      document.querySelector("#text-target")?.remove();
      const field = document.createElement(kind === "editable" ? "div" : kind);
      field.id = "text-target";
      if (kind === "editable") field.contentEditable = "true";
      document.querySelector("#board-panel .board-toolbar").append(field);
      field.focus();
    }, kind);
    await page.keyboard.type("abc");
    // The field's own undo follows the host: Chromium on macOS edits text with
    // Cmd, whatever platform the page is told it runs on.
    await page.keyboard.press("ControlOrMeta+z");
    assert.equal(await page.evaluate(() => globalThis.keyTaken), false, kind);
    assert.equal(
      await page
        .locator("#text-target")
        .evaluate((field) => field.value ?? field.textContent),
      "",
      kind,
    );
    await page.keyboard.press("ControlOrMeta+Shift+z");
    assert.equal(await page.evaluate(() => globalThis.keyTaken), false, kind);
    assert.equal(
      await page
        .locator("#text-target")
        .evaluate((field) => field.value ?? field.textContent),
      "abc",
      kind,
    );
    // On a macOS host the presses above are Cmd, which a page told it runs on
    // Linux never treats as the board's undo, so they prove nothing there.
    // The board's own chords must still leave a text target alone.
    for (const chord of ["Control+z", "Control+Shift+z"]) {
      await page.keyboard.press(chord);
      assert.equal(await page.evaluate(() => globalThis.keyTaken), false, kind);
    }
    assert.equal((await inspect(page)).strokes.length, 1);
  }
  await page.locator("#end").focus();
  for (const chord of ["Control+z", "Control+y", "Control+Shift+z"]) {
    await page.keyboard.press(chord);
    assert.equal((await inspect(page)).strokes.length, 1);
    assert.equal((await inspect(page)).canRedo, true);
    assert.equal(await page.evaluate(() => globalThis.keyTaken), false);
  }
});

test("P, E, and T shortcuts respect text, lock, and drawing states", async (t) => {
  const ctx = await boardPage(t);
  if (!ctx) return;
  const { page } = ctx;
  const board = page.locator("#board");
  const pressed = (name) =>
    page
      .getByRole("button", { name, exact: true })
      .getAttribute("aria-pressed");

  await board.focus();

  for (const [key, name] of [
    ["p", "Red pen"],
    ["p", "Blue pen"],
    ["p", "Green pen"],
    ["p", "Black pen"],
  ]) {
    await page.keyboard.press(key);
    assert.equal(await pressed(name), "true", `${key} selects ${name}`);
  }

  await page.evaluate(() => {
    document.querySelector("#board").dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "p",
        repeat: true,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  assert.equal(
    await pressed("Black pen"),
    "true",
    "held keys do not cycle colors",
  );

  const modifiedShortcutHandled = await page.evaluate(() => {
    const event = new KeyboardEvent("keydown", {
      key: "p",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    document.querySelector("#board").dispatchEvent(event);
    return event.defaultPrevented;
  });
  assert.equal(
    modifiedShortcutHandled,
    false,
    "modified keys remain available to the browser",
  );
  assert.equal(await pressed("Black pen"), "true");

  await page.getByRole("button", { name: "Eraser", exact: true }).focus();
  await page.keyboard.press("p");
  assert.equal(
    await pressed("Red pen"),
    "true",
    "shortcuts also work from the toolbar",
  );
  for (const name of ["Blue pen", "Green pen", "Black pen"]) {
    await page.keyboard.press("p");
    assert.equal(
      await pressed(name),
      "true",
      `P selects ${name} from the toolbar`,
    );
  }

  await page.keyboard.press("e");
  assert.equal(await pressed("Eraser"), "true");
  assert.equal(await pressed("Medium eraser"), "true");
  for (const name of ["Large eraser", "Small eraser", "Medium eraser"]) {
    await page.keyboard.press("e");
    assert.equal(await pressed(name), "true", `E selects ${name}`);
  }

  await page.keyboard.press("t");
  assert.equal(await pressed("Text"), "true");
  await page.keyboard.press("t");
  assert.equal(
    await pressed("Eraser"),
    "true",
    "T returns to the previous tool",
  );

  await page.keyboard.press("p");
  assert.equal(await pressed("Black pen"), "true");
  await page.keyboard.press("t");
  assert.equal(await pressed("Text"), "true");
  await page.keyboard.press("t");
  assert.equal(
    await pressed("Black pen"),
    "true",
    "T returns to the previous pen",
  );

  await page.keyboard.press("t");
  const box = await board.boundingBox();
  await page.mouse.click(box.x + 100, box.y + 100);
  const input = page.locator(".board-text-input");
  await input.waitFor();
  await page.keyboard.type("pet");
  assert.equal(await input.inputValue(), "pet");
  assert.equal(await pressed("Text"), "true", "typing does not switch tools");

  await page.locator(".board-text-cancel").click();
  await page.evaluate(() => globalThis.boardTest.lock("ended"));
  await board.focus();
  for (const key of ["p", "e", "t"]) {
    await page.keyboard.press(key);
    assert.equal(
      await pressed("Text"),
      "true",
      `${key} is ignored while locked`,
    );
    assert.equal(
      await page.evaluate(() => globalThis.keyTaken),
      false,
      `${key} is not consumed while locked`,
    );
  }

  await page.evaluate(() => globalThis.boardTest.lock("live"));
  await page.getByRole("button", { name: "Black pen", exact: true }).click();
  await board.focus();
  const boardBox = await board.boundingBox();
  await page.mouse.move(boardBox.x + 80, boardBox.y + 80);
  await page.mouse.down();
  for (const key of ["p", "e", "t"]) {
    await page.keyboard.press(key);
    assert.equal(
      await pressed("Black pen"),
      "true",
      `${key} is ignored while drawing`,
    );
    assert.equal(
      await pressed("Eraser"),
      "false",
      `${key} does not select eraser`,
    );
    assert.equal(await pressed("Text"), "false", `${key} does not select text`);
    assert.equal(
      await page.evaluate(() => globalThis.keyTaken),
      false,
      `${key} is not consumed while drawing`,
    );
  }
  await page.mouse.up();

  await board.focus();
  await page.keyboard.press("t");
  assert.equal(await pressed("Text"), "true");
  const textDragStart = { x: boardBox.x + 120, y: boardBox.y + 120 };
  await page.mouse.move(textDragStart.x, textDragStart.y);
  await page.mouse.down();
  await page.mouse.move(textDragStart.x + 120, textDragStart.y + 60);
  await page.locator(".board-text-draft.is-creating").waitFor();
  assert.equal(
    await page.evaluate(() => document.activeElement?.id),
    "board",
    "the board keeps focus while the text box is being dragged out",
  );
  await page.keyboard.press("p");
  assert.equal(
    await pressed("Text"),
    "true",
    "P is ignored while creating a text box",
  );
  assert.equal(
    await page.evaluate(() => globalThis.keyTaken),
    false,
    "P is not consumed during a text-box gesture",
  );
  await page.mouse.up();
  await page.locator(".board-text-input").waitFor();
  assert.equal(
    await page
      .locator(".board-text-input")
      .evaluate((input) => input === document.activeElement),
    true,
    "releasing the pointer completes the text draft",
  );
});

test("P shortcut uses event.code for a non-Latin event.key", async (t) => {
  const ctx = await boardPage(t);
  if (!ctx) return;
  const { page } = ctx;
  await page.locator("#board").focus();

  const handled = await page.evaluate(() => {
    const event = new KeyboardEvent("keydown", {
      key: "π",
      code: "KeyP",
      bubbles: true,
      cancelable: true,
    });
    document.querySelector("#board").dispatchEvent(event);
    return event.defaultPrevented;
  });
  assert.equal(handled, true, "the physical P key is handled on this layout");
  assert.equal(
    await page
      .getByRole("button", { name: "Red pen", exact: true })
      .getAttribute("aria-pressed"),
    "true",
    "event.code selects the next pen when event.key is non-Latin",
  );
});
