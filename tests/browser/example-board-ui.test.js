import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  exerciseTextDraft,
  exerciseTextBaseline,
} from "./board-text-fixture.js";
import { read, launchChromium, startStaticServer } from "./source.js";

let browser;
let server;
let base;

before(async () => {
  browser = await launchChromium();
  if (browser)
    ({ server, base } = await startStaticServer({
      handle: (_request, response, url) => {
        if (url.pathname === "/drawer") {
          response.setHeader("content-type", "text/html");
          response.end(drawerMarkup(url.searchParams.has("sidebar")));
          return true;
        }
        if (url.pathname !== "/") return false;
        response.setHeader("content-type", "text/html");
        response.end("<!doctype html><html><head></head><body></body></html>");
        return true;
      },
    }));
});
after(async () => {
  await browser?.close();
  await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
});

function exampleMarkup() {
  const html = read("web/interview.html");
  const start = html.indexOf('<details id="example-board"');
  return html.slice(start, html.indexOf("</details>", start) + 10);
}

async function openShapeBoard(t) {
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(base);
  await page.setContent(exampleMarkup());
  await page.evaluate(async (base) => {
    const { mountExampleBoard } = await import(`${base}/example-board.js`);
    const { BOARD_WIDTH, BOARD_HEIGHT, MAX_STROKES, MAX_STROKES_MESSAGE } =
      await import(`${base}/whiteboard.js`);
    const root = document.querySelector("#example-board");
    root.open = true;
    const canvas = root.querySelector("canvas");
    canvas.style.width = "640px";
    canvas.style.height = "400px";
    // Synthetic pointers have no native capture slot; drawing and journaling
    // still use the real handlers and canvas, as in the full-board test below.
    canvas.setPointerCapture = () => {};
    window.ops = [];
    window.online = false;
    window.locked = false;
    window.uploads = [];
    window.example = mountExampleBoard(root, {
      locked: () => window.locked,
      connected: () => window.online,
      record: (ops) => window.ops.push(...ops),
      publish: async (blob, count) => {
        window.uploads.push({ blob, count });
        return true;
      },
    });
    window.limits = { MAX_STROKES, MAX_STROKES_MESSAGE };
    window.pointer = (type, x = 20, y = 20) => {
      const box = canvas.getBoundingClientRect();
      canvas.dispatchEvent(
        new PointerEvent(type, {
          pointerId: 1,
          pointerType: "mouse",
          isPrimary: true,
          button: 0,
          buttons: type === "pointerup" ? 0 : 1,
          clientX: box.x + (x * box.width) / BOARD_WIDTH,
          clientY: box.y + (y * box.height) / BOARD_HEIGHT,
        }),
      );
    };
  }, base);
  return page;
}

for (const [start, end, bounds] of [
  [
    [1400, 500],
    [2000, 700],
    [1400, 500, 1600, 700],
  ],
  [
    [200, 500],
    [-400, 700],
    [0, 500, 200, 700],
  ],
  [
    [800, 200],
    [1000, -300],
    [800, 0, 1000, 200],
  ],
  [
    [800, 800],
    [1000, 1300],
    [800, 800, 1000, 1000],
  ],
]) {
  test(`Ellipse released outside the board at (${end}) keeps an elliptical outline`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await openShapeBoard(t);
    await page.locator("canvas").evaluate((canvas) => {
      delete canvas.setPointerCapture;
      window.edgeEvents = [];
      for (const type of [
        "pointerdown",
        "gotpointercapture",
        "pointermove",
        "pointerup",
        "lostpointercapture",
        "pointercancel",
      ])
        canvas.addEventListener(type, (event) => {
          window.edgeEvents.push({
            type,
            id: event.pointerId,
            buttons: event.buttons,
            x: event.clientX,
            y: event.clientY,
            captured: canvas.hasPointerCapture(event.pointerId),
            ops: window.ops.length,
          });
        });
    });
    await page.getByRole("button", { name: "Ellipse", exact: true }).click();
    const box = await page.locator("canvas").boundingBox();
    const move = ([x, y]) => page.mouse.move(box.x + x * 0.4, box.y + y * 0.4);
    await move(start);
    await page.mouse.down();
    await move(end);
    await page.mouse.up();
    const { ops, events } = await page.evaluate(() => ({
      ops: window.ops,
      events: window.edgeEvents,
    }));
    assert.equal(
      ops.length,
      1,
      `native edge drag events: ${JSON.stringify(events)}`,
    );
    const stroke = ops[0];
    assert.equal(stroke.op, "stroke");
    assert.equal(stroke.points.length, 258);
    assert.deepEqual(stroke.points.slice(-2), stroke.points.slice(0, 2));
    const [left, top, right, bottom] = bounds;
    const cx = (left + right) / 2;
    const cy = (top + bottom) / 2;
    const rx = (right - left) / 2;
    const ry = (bottom - top) / 2;
    for (let i = 0; i < stroke.points.length; i += 2) {
      const [x, y] = stroke.points.slice(i, i + 2);
      assert.ok(x >= left && x <= right && y >= top && y <= bottom);
      assert.ok(
        Math.abs(((x - cx) / rx) ** 2 + ((y - cy) / ry) ** 2 - 1) < 0.03,
        `outline point (${x}, ${y}) must stay on the bounded ellipse`,
      );
    }
  });
}

for (const tool of ["Line", "Rectangle", "Ellipse"]) {
  test(`${tool} presses leave existing ink unchanged until the first move`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await openShapeBoard(t);
    const canvas = page.locator("canvas");
    for (const count of [0, 1]) {
      if (count) {
        await page.getByRole("button", { name: "Pen", exact: true }).click();
        await page.evaluate(() => {
          window.pointer("pointerdown", 20, 20);
          window.pointer("pointerup", 20, 20);
        });
      }
      const before = await canvas.evaluate((node) =>
        node.toDataURL("image/jpeg", 0.72),
      );
      await page.getByRole("button", { name: tool, exact: true }).click();
      await page.evaluate(() => {
        window.online = true;
        window.pointer("pointerdown", 200, 100);
      });
      assert.equal(
        await canvas.evaluate((node) => node.toDataURL("image/jpeg", 0.72)),
        before,
      );
      assert.equal(await page.evaluate(() => window.ops.length), count);
      const uploaded = await page.evaluate(async () => {
        if ((await window.example.flush()) !== true)
          throw new Error("flush() did not complete the held-press upload");
        const { blob, count } = window.uploads.at(-1);
        const image = await new Promise((resolve) => {
          const reader = new FileReader();
          reader.onload = () => resolve(reader.result);
          reader.readAsDataURL(blob);
        });
        return { count, image };
      });
      assert.equal(uploaded.count, count);
      assert.equal(uploaded.image, before);
      await page.evaluate(() => window.pointer("pointerup", 200, 100));
      assert.equal(await page.evaluate(() => window.ops.length), count);
      assert.equal(
        await canvas.evaluate((node) => node.toDataURL("image/jpeg", 0.72)),
        before,
      );
    }
  });

  for (const ending of tool === "Line"
    ? ["zero-size", "pointercancel", "locked", "close"]
    : ["zero-size"]) {
    test(`${tool} ${ending} cancels a shared preview by syncing the restored board`, async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await openShapeBoard(t);
      const canvas = page.locator("canvas");
      const blank = await canvas.evaluate((node) =>
        node.toDataURL("image/jpeg", 0.72),
      );
      await page.getByRole("button", { name: tool, exact: true }).click();
      await page.evaluate(async () => {
        window.online = true;
        window.pointer("pointerdown", 200, 100);
        window.pointer("pointermove", 500, 250);
        if ((await window.example.flush()) !== true)
          throw new Error("flush() did not complete the drawing upload");
      });
      await page.waitForFunction(() => window.uploads.length === 1);
      assert.equal(await page.evaluate(() => window.uploads.length), 1);
      assert.equal(await page.evaluate(() => window.uploads[0].count), 1);
      assert.notEqual(
        await canvas.evaluate((node) => node.toDataURL("image/jpeg", 0.72)),
        blank,
      );
      await page.evaluate(async (ending) => {
        if (ending === "close") {
          const root = document.querySelector("#example-board");
          await new Promise((resolve) => {
            root.addEventListener("toggle", resolve, { once: true });
            root.open = false;
          });
        } else {
          if (ending === "locked") window.locked = true;
          window.pointer(
            ending === "pointercancel" ? "pointercancel" : "pointerup",
            200.2,
            100.2,
          );
        }
        if ((await window.example.flush()) !== true)
          throw new Error("flush() did not complete the restored-board upload");
      }, ending);
      await page.waitForFunction(() => window.uploads.length === 2);
      assert.deepEqual(await page.evaluate(() => window.ops), []);
      assert.equal(await page.evaluate(() => window.uploads.length), 2);
      assert.equal(await page.evaluate(() => window.uploads[1].count), 0);
      assert.equal(
        await page.evaluate(
          () =>
            new Promise((resolve) => {
              const reader = new FileReader();
              reader.onload = () => resolve(reader.result);
              reader.readAsDataURL(window.uploads[1].blob);
            }),
        ),
        blank,
      );
    });
  }

  if (tool === "Line")
    test(`${tool} uses the final stroke slot and refuses the next press`, async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await openShapeBoard(t);
      const limit = await page.evaluate(() => {
        for (let i = 0; i < window.limits.MAX_STROKES - 1; i++) {
          window.pointer("pointerdown");
          window.pointer("pointerup");
        }
        return window.limits.MAX_STROKES;
      });
      assert.equal(await page.evaluate(() => window.ops.length), limit - 1);
      const canvas = page.locator("canvas");
      const before = await canvas.evaluate((node) => node.toDataURL());
      await page.getByRole("button", { name: "Text", exact: true }).click();
      await page.evaluate(() => {
        window.pointer("pointerdown", 20, 500);
        window.pointer("pointerup", 20, 500);
      });
      await page
        .getByRole("textbox", { name: "Label text" })
        .fill("Keep this draft");
      await page.getByRole("button", { name: tool, exact: true }).click();
      await page.evaluate(() => window.pointer("pointerdown", 200, 100));
      await canvas.press("Control+Enter");
      assert.equal(await page.evaluate(() => window.ops.length), limit - 1);
      assert.equal(
        await page.getByRole("textbox", { name: "Label text" }).inputValue(),
        "Keep this draft",
      );
      await page.evaluate(() => {
        window.pointer("pointermove", 300, 150);
        window.pointer("pointermove", 500, 200);
      });
      assert.equal(await page.evaluate(() => window.ops.length), limit - 1);
      await page.evaluate(() => window.pointer("pointerup", 600, 250));
      assert.equal(await page.evaluate(() => window.ops.length), limit);
      const stroke = await page.evaluate(() => window.ops.at(-1));
      assert.equal(stroke.op, "stroke");
      assert.ok(stroke.points.length > 2);
      const final = await canvas.evaluate((node) => node.toDataURL());
      assert.notEqual(final, before);
      await page.evaluate(() => window.pointer("pointerdown", 900, 300));
      assert.equal(
        await page.locator(".example-pane > [role=status]").textContent(),
        await page.evaluate(() => window.limits.MAX_STROKES_MESSAGE),
      );
      await page.evaluate(() => {
        window.pointer("pointermove", 1000, 400);
        window.pointer("pointerup", 1000, 400);
      });
      assert.equal(await page.evaluate(() => window.ops.length), limit);
      assert.equal(await canvas.evaluate((node) => node.toDataURL()), final);
      await page.getByRole("button", { name: "Undo", exact: true }).click();
      await page.getByRole("button", { name: "Confirm", exact: true }).click();
      assert.equal(await page.evaluate(() => window.ops.at(-1).op), "text");
    });

  test(`${tool} clicks and rounded zero-size drags leave no edit`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await openShapeBoard(t);
    const canvas = page.locator("canvas");
    const blank = await canvas.evaluate((node) => node.toDataURL());
    await page.getByRole("button", { name: tool, exact: true }).click();
    for (const move of [null, [200.2, 100.2], [500, 250]]) {
      await page.evaluate((move) => {
        window.pointer("pointerdown", 200, 100);
        if (move) window.pointer("pointermove", ...move);
        window.pointer("pointerup", move ? 200.2 : 200, move ? 100.2 : 100);
      }, move);
      assert.deepEqual(await page.evaluate(() => window.ops), []);
      assert.equal(await canvas.evaluate((node) => node.toDataURL()), blank);
      assert.equal(await page.locator("[data-action=undo]").isDisabled(), true);
    }
    await page.getByRole("button", { name: "Pen", exact: true }).click();
    await page.evaluate(() => {
      window.pointer("pointerdown", 200, 100);
      window.pointer("pointerup", 200, 100);
    });
    const ops = await page.evaluate(() => window.ops);
    assert.equal(ops.length, 1);
    assert.equal(ops[0].op, "stroke");
    assert.deepEqual(ops[0].points, [200, 100]);
    assert.notEqual(await canvas.evaluate((node) => node.toDataURL()), blank);
  });
}

for (const tool of ["Line", "Rectangle", "Ellipse"]) {
  test(`${tool} previews and records one stroke in all four directions`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await openShapeBoard(t);
    const canvas = page.locator("canvas");
    const image = () => canvas.evaluate((node) => node.toDataURL());
    const blank = await image();
    const button = page.getByRole("button", { name: tool, exact: true });
    assert.equal(await button.count(), 1);
    await button.click();
    await page.getByRole("button", { name: "Pen Red", exact: true }).click();
    assert.equal(await button.getAttribute("aria-pressed"), "true");
    const box = await canvas.boundingBox();
    const move = (x, y) => page.mouse.move(box.x + x * 0.4, box.y + y * 0.4);
    for (const [dx, dy] of [
      [240, 160],
      [-240, 160],
      [240, -160],
      [-240, -160],
    ]) {
      await t.test(`drag (${dx}, ${dy})`, async () => {
        const opCount = await page.evaluate(() => window.ops.length);
        await move(400, 300);
        await page.mouse.down();
        await move(400 + dx / 2, 300 + dy / 2);
        const preview = await image();
        assert.notEqual(preview, blank);
        assert.equal(await page.evaluate(() => window.ops.length), opCount);
        await move(400 + dx, 300 + dy);
        await page.mouse.up();
        const final = await image();
        assert.notEqual(final, preview);
        const pixels = await canvas.evaluate(
          (node, { tool, dx, dy }) => {
            const context = node.getContext("2d");
            const pixel = (x, y) => [...context.getImageData(x, y, 1, 1).data];
            const cx = 400 + dx / 2;
            const cy = 300 + dy / 2;
            const edge =
              tool === "Line"
                ? [cx, cy]
                : tool === "Rectangle"
                  ? [cx, 300]
                  : [400 + dx, cy];
            return { edge: pixel(...edge), center: pixel(cx, cy) };
          },
          { tool, dx, dy },
        );
        assert.deepEqual(pixels.edge, [194, 38, 27, 255]);
        if (tool !== "Line")
          assert.deepEqual(pixels.center, [255, 255, 255, 255]);
        const ops = await page.evaluate(
          (opCount) => window.ops.slice(opCount),
          opCount,
        );
        assert.equal(ops.length, 1);
        const stroke = ops[0];
        assert.equal(stroke.op, "stroke");
        assert.equal(stroke.color, "#c2261b");
        assert.equal(stroke.width, 3);
        const xs = stroke.points.filter((_, i) => i % 2 === 0);
        const ys = stroke.points.filter((_, i) => i % 2 === 1);
        for (const [actual, expected] of [
          [Math.min(...xs), Math.min(400, 400 + dx)],
          [Math.max(...xs), Math.max(400, 400 + dx)],
          [Math.min(...ys), Math.min(300, 300 + dy)],
          [Math.max(...ys), Math.max(300, 300 + dy)],
        ])
          assert.ok(Math.abs(actual - expected) < 1);
        assert.equal(
          stroke.points.length,
          tool === "Line" ? 4 : tool === "Rectangle" ? 10 : 258,
        );
        if (tool !== "Line")
          assert.deepEqual(stroke.points.slice(-2), stroke.points.slice(0, 2));
        await page.getByRole("button", { name: "Undo", exact: true }).click();
        assert.equal(await image(), blank);
      });
    }
  });
  test(`${tool} supports replay, reconnection, history and cancellation`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await openShapeBoard(t);
    const canvas = page.locator("canvas");
    const image = () => canvas.evaluate((node) => node.toDataURL());
    const blank = await image();
    await page.getByRole("button", { name: tool, exact: true }).click();
    const box = await canvas.boundingBox();
    const move = (x, y) => page.mouse.move(box.x + x * 0.4, box.y + y * 0.4);
    const dx = 240;
    const dy = 160;
    await move(400, 300);
    await page.mouse.down();
    await move(400 + dx, 300 + dy);
    await page.mouse.up();
    const final = await image();
    assert.notEqual(final, blank);
    assert.equal(
      await page.evaluate(async (base) => {
        const { createBoard, applyOp, drawBoard } = await import(
          `${base}/whiteboard.js`
        );
        const replay = createBoard();
        for (const op of window.ops) if (!applyOp(replay, op)) return false;
        const copy = document.createElement("canvas");
        copy.width = 1600;
        copy.height = 1000;
        drawBoard(copy.getContext("2d"), replay.strokes());
        return (
          copy.toDataURL() === document.querySelector("canvas").toDataURL()
        );
      }, base),
      true,
    );
    await page.evaluate(async () => {
      window.online = true;
      await window.example.reconnect();
      await window.example.flush();
    });
    assert.equal(await image(), final);
    assert.equal(await page.evaluate(() => window.uploads.at(-1).count), 1);
    await page.getByRole("button", { name: "Undo", exact: true }).click();
    assert.equal(await image(), blank);
    await move(400, 300);
    await page.mouse.down();
    await move(400 + dx, 300 + dy);
    await page.mouse.up();
    await page.getByRole("button", { name: "Clear", exact: true }).click();
    assert.equal(await image(), blank);
    await page.getByRole("button", { name: "Undo", exact: true }).click();
    assert.equal(await image(), final);
    const count = await page.evaluate(() => window.ops.length);
    await move(700, 500);
    await page.mouse.down();
    await move(800, 600);
    await canvas.evaluate((node) => {
      node.dispatchEvent(
        new PointerEvent("pointercancel", {
          pointerId: 1,
          pointerType: "mouse",
        }),
      );
    });
    await page.mouse.up();
    assert.equal(await image(), final);
    assert.equal(await page.evaluate(() => window.ops.length), count);
  });
}

// The real ending wait, so the board is stacked against the page's own
// overlay rules rather than a stand-in.
function endingMarkup() {
  const html = read("web/interview.html");
  const start = html.lastIndexOf(
    "<section",
    html.indexOf('id="ending-overlay"'),
  );
  return html.slice(start, html.indexOf("</section>", start) + 10);
}

function drawerMarkup(sidebar = false) {
  const panel = `<section class="editor-panel" data-example-drawing="true"><div class="editor-stack"><textarea id="code">const keep = 42;</textarea></div><div class="drawer-resizer" role="separator" tabindex="0" aria-label="Resize drawing and test panel"></div><section class="test-drawer"><details><summary>Add a case</summary><input></details>${exampleMarkup()}<button>Test results</button><div id="results-body" hidden></div></section></section>`;
  const body = sidebar
    ? `<main class="interview-shell"><aside class="interview-sidebar"><select id="sidebar-field"><option>One</option></select><button id="yield-turn" type="button">Your turn is done</button></aside><div class="splitter"></div>${panel}${endingMarkup()}</main>`
    : panel;
  return `<!doctype html><html><head><link rel="stylesheet" href="/styles.css"></head><body>${body}</body></html>`;
}

test("selecting an example pen color switches from erasing to drawing", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(base);
  await page.setContent(exampleMarkup());
  await page.evaluate(async (base) => {
    const { mountExampleBoard } = await import(`${base}/example-board.js`);
    window.ops = [];
    const root = document.querySelector("#example-board");
    root.open = true;
    mountExampleBoard(root, {
      locked: () => false,
      connected: () => false,
      record: (ops) => window.ops.push(...ops),
      publish: async () => true,
    });
  }, base);

  const pen = page.getByRole("button", { name: "Pen", exact: true });
  const eraser = page.getByRole("button", { name: "Eraser", exact: true });
  const red = page.getByRole("button", { name: "Pen Red", exact: true });
  const pressedColors = () =>
    page
      .locator(".board-color")
      .evaluateAll((buttons) =>
        buttons
          .filter((button) => button.getAttribute("aria-pressed") === "true")
          .map((button) => button.getAttribute("aria-label")),
      );
  const canvas = page.locator("canvas");
  async function draw() {
    const box = await canvas.boundingBox();
    await page.mouse.move(box.x + 20, box.y + 20);
    await page.mouse.down();
    await page.mouse.move(box.x + 80, box.y + 40);
    await page.mouse.up();
  }

  assert.deepEqual(await pressedColors(), ["Pen Black"]);
  await draw();
  assert.equal(await page.evaluate(() => window.ops.length), 1);
  await eraser.click();
  await draw();
  assert.deepEqual(
    await page.evaluate(() => {
      const { color, width } = window.ops.at(-1);
      return { color, width };
    }),
    { color: "#ffffff", width: 24 },
  );
  await red.click();
  assert.equal(await page.evaluate(() => window.ops.length), 2);
  await draw();
  assert.deepEqual(
    await page.evaluate(() =>
      window.ops.slice(2).map(({ color, width }) => ({ color, width })),
    ),
    [{ color: "#c2261b", width: 3 }],
  );
  assert.equal(await pen.getAttribute("aria-pressed"), "true");
  assert.equal(await eraser.getAttribute("aria-pressed"), "false");
  assert.deepEqual(await pressedColors(), ["Pen Red"]);

  await eraser.click();
  assert.equal(await pen.getAttribute("aria-pressed"), "false");
  assert.equal(await eraser.getAttribute("aria-pressed"), "true");
  assert.deepEqual(await pressedColors(), ["Pen Red"]);
  await draw();
  assert.deepEqual(
    await page.evaluate(() => {
      const { color, width } = window.ops.at(-1);
      return { color, width };
    }),
    { color: "#ffffff", width: 24 },
  );
  await page.getByRole("button", { name: "Undo", exact: true }).click();
  assert.equal(await page.evaluate(() => window.ops.at(-1).op), "undo");

  await red.click();
  await draw();
  await eraser.click();
  await pen.click();
  await draw();
  assert.deepEqual(
    await page.evaluate(() =>
      window.ops.slice(-2).map(({ color, width }) => ({ color, width })),
    ),
    [
      { color: "#c2261b", width: 3 },
      { color: "#c2261b", width: 3 },
    ],
  );
});

test("example confirmation preserves draft glyph positions", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage({
    viewport: { width: 1100, height: 900 },
  });
  t.after(() => page.close());
  await page.goto(`${base}/drawer`);
  await page.evaluate(async (base) => {
    const { mountExampleBoard } = await import(`${base}/example-board.js`);
    const root = document.querySelector("#example-board");
    root.open = true;
    mountExampleBoard(root, {
      locked: () => false,
      connected: () => false,
      record() {},
      publish: async () => true,
    });
  }, base);
  await exerciseTextBaseline(
    page,
    page.locator("canvas"),
    page.getByRole("button", { name: "Text", exact: true }),
  );
});

test("example text drafts journal only confirmed labels and export the painted image", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage({
    viewport: { width: 1100, height: 900 },
  });
  t.after(() => page.close());
  await page.goto(`${base}/drawer`);
  await page.evaluate(async (base) => {
    const { mountExampleBoard } = await import(`${base}/example-board.js`);
    window.textOps = [];
    window.textUploads = [];
    const root = document.querySelector("#example-board");
    root.open = true;
    window.example = mountExampleBoard(root, {
      locked: () => false,
      record: (ops) => window.textOps.push(...ops),
      publish: async (blob, count) => {
        const image = await new Promise((resolve) => {
          const reader = new FileReader();
          reader.onload = () => resolve(reader.result);
          reader.readAsDataURL(blob);
        });
        window.textUploads.push({ image, count });
        return true;
      },
    });
  }, base);
  const root = page.locator("#example-board");
  const canvas = root.locator("canvas");
  const blank = await exerciseTextDraft(
    page,
    canvas,
    root.getByRole("button", { name: "Text", exact: true }),
  );
  assert.deepEqual(
    await page.evaluate(() => window.textOps.map((op) => op.op)),
    ["text"],
  );
  await page.evaluate(() => window.example.flush());
  assert.equal(
    await page.evaluate(
      () => window.textUploads.at(-1).image === window.example.image(),
    ),
    true,
  );
  assert.equal(await page.evaluate(() => window.textUploads.at(-1).count), 1);
  await root.locator('[data-action="undo"]').click();
  assert.equal(await canvas.evaluate((node) => node.toDataURL()), blank);
});

test("the example drawer preserves code, journals pen strokes, and sends the cleared state after reconnecting", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage({
    viewport: { width: 1100, height: 900 },
  });
  t.after(() => page.close());
  await page.goto(`${base}/drawer`);
  assert.equal(await page.evaluate(() => document.compatMode), "CSS1Compat");
  await page.waitForFunction(
    () =>
      getComputedStyle(document.querySelector(".drawer-resizer")).height ===
      "8px",
  );
  await page.evaluate(async (base) => {
    const { mountExampleBoard } = await import(`${base}/example-board.js`);
    const { mountInterviewDrawer } = await import(
      `${base}/interview-drawer.js`
    );
    window.locked = false;
    window.ops = [];
    window.uploads = [];
    window.activity = [];
    window.example = mountExampleBoard(
      document.querySelector("#example-board"),
      {
        locked: () => window.locked,
        activity: () => window.activity.push(performance.now()),
        record: (ops) => window.ops.push(...ops),
        publish: async (blob, strokes) => {
          if (window.disconnected) return false;
          window.uploads.push({ type: blob.type, size: blob.size, strokes });
          return true;
        },
      },
    );
    mountInterviewDrawer(document.querySelector(".editor-panel"));
  }, base);
  assert.equal(
    await page.locator("#example-board").evaluate((node) => node.open),
    false,
  );
  const beforeHeight = await page
    .locator(".editor-stack")
    .evaluate((node) => node.getBoundingClientRect().height);
  const panelBottom = await page
    .locator(".editor-panel")
    .evaluate((node) => node.getBoundingClientRect().bottom);
  const drawerBottom = await page
    .locator(".test-drawer")
    .evaluate((node) => node.getBoundingClientRect().bottom);
  assert.ok(Math.abs(panelBottom - drawerBottom) <= 2);
  await page.locator("#example-board summary").click();
  await page.waitForFunction(() =>
    document.querySelector(".test-drawer").style.height.endsWith("px"),
  );
  const afterHeight = await page
    .locator(".editor-stack")
    .evaluate((node) => node.getBoundingClientRect().height);
  assert.ok(afterHeight < beforeHeight && afterHeight >= 128);
  const separator = page.getByRole("separator");
  // Opening details schedules layout observers. Start from a settled layout,
  // and measure at pointerdown rather than before hover can scroll the page.
  await page.evaluate(async () => {
    await new Promise(requestAnimationFrame);
    await new Promise(requestAnimationFrame);
    const handle = document.querySelector(".drawer-resizer");
    window.drawerEvents = [];
    window.drawerErrors = [];
    window.addEventListener("error", (event) =>
      window.drawerErrors.push(event.message),
    );
    for (const type of [
      "pointerdown",
      "pointermove",
      "pointerup",
      "pointercancel",
      "gotpointercapture",
      "lostpointercapture",
    ])
      handle.addEventListener(type, (event) =>
        window.drawerEvents.push({
          type,
          id: event.pointerId,
          button: event.button,
          buttons: event.buttons,
          captured: handle.hasPointerCapture(event.pointerId),
        }),
      );
    handle.addEventListener(
      "pointerdown",
      (event) => {
        window.drawerGrab = {
          x: event.clientX,
          y: event.clientY,
          height: document.querySelector(".test-drawer").clientHeight,
          surface: document.querySelector(".example-surface").clientHeight,
          id: event.pointerId,
        };
      },
      { once: true },
    );
  });
  await separator.hover({ position: { x: 30, y: 4 } });
  await page.mouse.down();
  const grab = await page.evaluate(() => window.drawerGrab);
  assert.ok(grab, "pointerdown must reach the drawer separator");
  // Capture becomes active when the browser processes the next pointer event.
  // Observe that transition rather than sampling inside pointerdown.
  await page.mouse.move(grab.x, grab.y - 1);
  const capture = await page.evaluate(() => ({
    events: window.drawerEvents,
    errors: window.drawerErrors,
    active: document
      .querySelector(".drawer-resizer")
      .hasPointerCapture(window.drawerGrab.id),
  }));
  assert.ok(
    capture.active &&
      capture.events.some((event) => event.type === "gotpointercapture"),
    `separator must activate pointer capture: ${JSON.stringify(capture)}`,
  );
  const initialDrawer = grab.height;
  const initialSurface = grab.surface;
  await page.mouse.move(grab.x, grab.y - 70, { steps: 8 });
  await page.mouse.up();
  const dragResult = await page.evaluate(() => ({
    height: document.querySelector(".test-drawer").clientHeight,
    requested: document.querySelector(".test-drawer").style.height,
    panel: document.querySelector(".editor-panel").clientHeight,
    handle: document
      .querySelector(".drawer-resizer")
      .getBoundingClientRect()
      .toJSON(),
  }));
  assert.ok(
    dragResult.height > initialDrawer,
    `drag from ${JSON.stringify(grab)} resulted in ${JSON.stringify(dragResult)}`,
  );
  const enlargedDrawer = await page
    .locator(".test-drawer")
    .evaluate((node) => node.clientHeight);
  assert.ok(
    enlargedDrawer > initialDrawer,
    `drawer height: ${initialDrawer} -> ${enlargedDrawer}`,
  );
  const resizedEditor = await page
    .locator(".editor-stack")
    .evaluate((node) => node.getBoundingClientRect().height);
  assert.ok(
    resizedEditor < afterHeight && resizedEditor >= 128,
    `editor height: ${afterHeight} -> ${resizedEditor}`,
  );
  assert.ok(
    (await page
      .locator(".example-surface")
      .evaluate((node) => node.clientHeight)) > initialSurface,
  );
  await separator.focus();
  await page.keyboard.press("ArrowDown");
  assert.ok(
    (await page.locator(".test-drawer").evaluate((node) => node.clientHeight)) <
      enlargedDrawer,
  );
  assert.deepEqual(
    await page
      .locator("[data-tools] button")
      .evaluateAll((buttons) =>
        buttons.map((button) => button.getAttribute("aria-label")),
      ),
    ["Pen", "Eraser", "Text", "Line", "Rectangle", "Ellipse"],
  );
  assert.equal(await page.locator("[data-action=redo]").count(), 0);
  await page.getByRole("button", { name: "Pen", exact: true }).click();
  await page.getByRole("button", { name: "Pen Red", exact: true }).click();
  const canvas = page.locator("canvas");
  await canvas.scrollIntoViewIfNeeded();
  const box = await canvas.boundingBox();
  await page.mouse.move(box.x + 50, box.y + 50);
  await page.mouse.down();
  assert.equal(await page.evaluate(() => window.activity.length), 1);
  // The board reports activity at most once per 1000 ms of real time
  // (web/example-board.js), so elapsed time is the input; load only lengthens it.
  await new Promise((resolve) => setTimeout(resolve, 1100));
  await page.mouse.move(box.x + 140, box.y + 110);
  assert.equal(await page.evaluate(() => window.activity.length), 2);
  assert.equal(await page.evaluate(() => window.ops.length), 0);
  assert.equal(await page.evaluate(() => window.uploads.length), 0);
  await page.mouse.move(box.x + 145, box.y + 110);
  assert.equal(await page.evaluate(() => window.activity.length), 2);
  await page.mouse.up();
  assert.equal(await page.locator("#code").inputValue(), "const keep = 42;");
  const ops = await page.evaluate(() => window.ops);
  assert.equal(ops.length, 1);
  assert.ok(ops[0].points.length >= 4);
  assert.equal(ops[0].color, "#c2261b");
  assert.equal(
    await page.evaluate(async () => {
      const { createBoard, applyOp } = await import("/whiteboard.js");
      const replay = createBoard();
      return (
        window.ops.every((op) => applyOp(replay, op)) &&
        replay.strokes()[0].color === "#c2261b"
      );
    }),
    true,
  );
  assert.equal(
    await page
      .getByRole("button", { name: "Pen Red", exact: true })
      .evaluate((button) => button.style.background),
    "rgb(194, 38, 27)",
  );
  await page.waitForFunction(() => window.uploads.length === 1);
  assert.deepEqual(
    await page.evaluate(() => [
      window.uploads[0].type,
      window.uploads[0].strokes,
    ]),
    ["image/jpeg", 1],
  );
  await page.getByRole("button", { name: "Clear", exact: true }).click();
  await page.waitForFunction(() => window.uploads.length === 2);
  await page.evaluate(() => window.example.reconnect());
  await page.waitForFunction(() => window.uploads.length === 3);
  assert.equal(await page.evaluate(() => window.uploads[2].strokes), 0);
  await page.getByRole("button", { name: "Undo", exact: true }).click();
  const localDrawing = await page.evaluate(() => window.example.image());
  assert.match(localDrawing, /^data:image\/jpeg;base64,/);
  await page.evaluate(async () => {
    window.disconnected = true;
    await window.example.finish();
  });
  assert.equal(await page.evaluate(() => window.uploads.length), 3);
  assert.equal(await page.evaluate(() => window.example.image()), localDrawing);
  assert.equal(
    await page.evaluate(async () => {
      window.disconnected = false;
      return window.example.reconnect();
    }),
    true,
  );
  assert.equal(await page.evaluate(() => window.uploads.at(-1).strokes), 1);
  assert.deepEqual(await page.evaluate(() => window.ops.map((op) => op.op)), [
    "stroke",
    "clear",
    "undo",
  ]);
  await page.locator("#example-board summary").click();
  await page.waitForFunction(
    () => document.querySelector(".test-drawer").style.height === "auto",
  );
  assert.equal(
    await page
      .locator(".editor-stack")
      .evaluate((node) => node.getBoundingClientRect().height),
    beforeHeight,
  );
});

test("example text drafts do not hand over before confirmation or after cancellation", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage({
    viewport: { width: 1100, height: 900 },
  });
  t.after(() => page.close());
  await page.goto(`${base}/drawer`);
  await page.evaluate(async (base) => {
    const { createDrawingTurn, mountExampleBoard } = await import(
      `${base}/example-board.js`
    );
    window.clock = 0;
    window.sent = [];
    window.pings = 0;
    window.turn = createDrawingTurn({
      now: () => window.clock,
      requestFrame: (frame) => {
        window.frame = frame;
      },
      windowMs: () => 3000,
      blocked: () => false,
      paint() {},
      flush: () => window.example.flush(),
      yieldTurn: async () => {
        window.sent.push("yield");
      },
    });
    const root = document.querySelector("#example-board");
    root.open = true;
    window.example = mountExampleBoard(root, {
      locked: () => false,
      turnActivity: () => window.turn.activity(),
      cancelTurn: () => window.turn.reset(),
      activity: () => {
        window.pings++;
      },
      record() {},
      publish: async () => {
        window.sent.push("image");
        return true;
      },
    });
    window.step = () => {
      window.clock += 3000;
      window.frame?.();
    };
  }, base);
  await page.getByRole("button", { name: "Text", exact: true }).click();
  const canvas = page.locator("canvas");
  const input = page.getByRole("textbox", { name: "Label text" });
  for (const cancel of ["Escape", "Cancel", "empty"]) {
    await canvas.click({ position: { x: 40, y: 40 } });
    await input.fill(cancel === "empty" ? "  " : "unfinished");
    await page.getByRole("button", { name: "Red text" }).click();
    await page.evaluate(() => window.step());
    assert.equal(await page.evaluate(() => window.turn.pending()), false);
    if (cancel === "Cancel")
      await page.getByRole("button", { name: "Cancel", exact: true }).click();
    else await input.press(cancel === "empty" ? "Control+Enter" : "Escape");
    await page.evaluate(() => window.step());
    assert.deepEqual(await page.evaluate(() => window.sent), []);
  }
  assert.ok(await page.evaluate(() => window.pings > 0));
  await canvas.click({ position: { x: 40, y: 40 } });
  await input.fill("ready");
  await input.press("Control+Enter");
  assert.equal(await page.evaluate(() => window.turn.pending()), true);
  await page.evaluate(() => window.step());
  await page.waitForFunction(() => window.sent.includes("yield"));
  assert.deepEqual(await page.evaluate(() => window.sent), ["image", "yield"]);
});

test("a stationary pointer counts down without a microphone, shares its preview, and can keep drawing", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage({
    viewport: { width: 1100, height: 900 },
  });
  t.after(() => page.close());
  await page.goto(base);
  await page.setContent(
    `<button id="ring" hidden>Yield</button>${exampleMarkup()}<style>canvas { width: 800px; height: 400px; }</style>`,
  );
  await page.evaluate(async (base) => {
    const { createDrawingTurn, mountExampleBoard } = await import(
      `${base}/example-board.js`
    );
    window.clock = 0;
    window.sent = [];
    window.ops = [];
    window.blocked = false;
    const ring = document.querySelector("#ring");
    window.turn = createDrawingTurn({
      now: () => window.clock,
      requestFrame: (frame) => (window.frame = frame),
      windowMs: () => 3000,
      blocked: () => window.blocked,
      paint: (value) => {
        ring.hidden = value === null;
        ring.style.setProperty("--turn-progress", String(value));
      },
      flush: () => window.example.flush(),
      yieldTurn: async () => window.sent.push({ type: "yield" }),
    });
    window.example = mountExampleBoard(
      document.querySelector("#example-board"),
      {
        locked: () => false,
        turnActivity: () => window.turn.activity(),
        cancelTurn: () => window.turn.reset(),
        record: (ops) => window.ops.push(...ops),
        publish: async (blob, strokes) => {
          window.sent.push({ type: "image", strokes, bytes: blob.size });
          return true;
        },
      },
    );
    window.step = (at) => {
      window.clock = at;
      window.frame();
    };
  }, base);
  await page.locator("#example-board summary").click();
  await page.getByRole("button", { name: "Pen", exact: true }).click();
  const box = await page.locator("canvas").boundingBox();
  await page.mouse.move(box.x + 50, box.y + 50);
  await page.mouse.down();
  await page.mouse.move(box.x + 140, box.y + 110);
  assert.equal(await page.locator("#ring").isVisible(), true);
  await page.evaluate(() => window.step(1500));
  assert.equal(
    await page
      .locator("#ring")
      .evaluate((node) => node.style.getPropertyValue("--turn-progress")),
    "0.5",
  );
  await page.evaluate(() => window.step(3000));
  await page.waitForFunction(() => window.sent.length === 2);
  const sent = await page.evaluate(() => window.sent);
  assert.equal(sent[0].type, "image");
  assert.equal(sent[0].strokes, 1);
  assert.ok(sent[0].bytes > 100);
  assert.equal(sent[1].type, "yield");
  assert.equal(
    await page.evaluate(() => window.ops.length),
    0,
    "the pointer is still drawing its preview",
  );
  await page.mouse.move(box.x + 200, box.y + 150);
  assert.equal(await page.locator("#ring").isVisible(), true);
  assert.equal(
    await page
      .locator("#ring")
      .evaluate((node) => node.style.getPropertyValue("--turn-progress")),
    "0",
  );
  await page.mouse.up();
  assert.equal(await page.evaluate(() => window.ops.length), 1);
  await page.evaluate(() => window.step(4500));
  assert.equal(
    await page
      .locator("#ring")
      .evaluate((node) => node.style.getPropertyValue("--turn-progress")),
    "0.5",
  );
  await page.locator("#example-board summary").click();
  await page.waitForFunction(() => !window.turn.pending());
  assert.equal(await page.locator("#ring").isVisible(), false);
  await page.evaluate(() => {
    window.blocked = true;
    window.step(4600);
  });
  assert.equal(await page.locator("#ring").isVisible(), false);
  assert.equal(
    await page.evaluate(
      () => window.sent.filter((event) => event.type === "yield").length,
    ),
    1,
  );
  await page.evaluate(() => window.example.finish());
});

test("full boards reject excess strokes and undo makes room for new ink", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(base);
  await page.setContent(exampleMarkup());
  await page.evaluate(async (base) => {
    const { mountExampleBoard } = await import(`${base}/example-board.js`);
    const { MAX_STROKES } = await import(`${base}/whiteboard.js`);
    const root = document.querySelector("#example-board");
    root.open = true;
    const canvas = root.querySelector("canvas");
    // Synthetic pointer events have no native capture slot. The model, canvas,
    // event handlers and image export still run as they do for real pointers.
    canvas.setPointerCapture = () => {};
    window.uploads = [];
    window.ops = [];
    window.example = mountExampleBoard(root, {
      locked: () => false,
      record: (ops) => window.ops.push(...ops),
      publish: async (blob, strokes) => {
        window.uploads.push({ bytes: blob.size, strokes });
        return true;
      },
    });
    window.pointer = (type, x = 20, y = 20) => {
      const box = canvas.getBoundingClientRect();
      canvas.dispatchEvent(
        new PointerEvent(type, {
          pointerId: 1,
          pointerType: "mouse",
          isPrimary: true,
          button: 0,
          buttons: type === "pointerup" ? 0 : 1,
          clientX: box.x + x,
          clientY: box.y + y,
        }),
      );
    };
    for (let i = 0; i < MAX_STROKES; i++) {
      window.pointer("pointerdown");
      window.pointer("pointerup");
    }
    await window.example.flush();
    window.fullImage = window.example.image();
  }, base);
  assert.equal(await page.evaluate(() => window.ops.length), 600);
  assert.equal(
    await page.evaluate(() => window.ops.every((op) => op.color === "#101418")),
    true,
  );
  assert.deepEqual(
    await page.evaluate(() => window.uploads.map((x) => x.strokes)),
    [600],
  );
  for (const name of ["Pen", "Eraser"]) {
    await page.getByRole("button", { name, exact: true }).click();
    await page.evaluate(async () => {
      window.pointer("pointerdown");
      window.pointer("pointermove", 100, 100);
      await window.example.flush();
    });
    assert.equal(
      await page.evaluate(() => window.example.image() === window.fullImage),
      true,
      `${name}: a full board must not show an uncommittable preview`,
    );
    assert.equal(await page.evaluate(() => window.uploads.length), 1, name);
    await page.evaluate(() => window.pointer("pointerup", 100, 100));
    assert.equal(await page.evaluate(() => window.ops.length), 600, name);
  }
  await page.getByRole("button", { name: "Undo", exact: true }).click();
  await page.getByRole("button", { name: "Pen", exact: true }).click();
  await page.evaluate(async () => {
    window.pointer("pointerdown");
    window.pointer("pointermove", 100, 100);
    await window.example.flush();
    window.previewImage = window.example.image();
    window.pointer("pointerup", 100, 100);
  });
  assert.equal(await page.evaluate(() => window.uploads.at(-1).strokes), 600);
  assert.equal(await page.evaluate(() => window.ops.at(-1).op), "stroke");
  assert.equal(
    await page.evaluate(() => window.example.image() === window.previewImage),
    true,
    "the committed stroke must match its synced preview",
  );
  assert.equal(
    await page.evaluate(() => window.example.image() !== window.fullImage),
    true,
    "undo must make room for new ink",
  );
});

test("offline drawings skip JPEG encoding and locked controls repaint", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(base);
  await page.setContent(exampleMarkup());
  await page.evaluate(async (base) => {
    const { mountExampleBoard } = await import(`${base}/example-board.js`);
    window.connected = false;
    window.locked = false;
    window.encodings = 0;
    window.uploads = 0;
    const canvas = document.querySelector("canvas");
    const encode = canvas.toBlob.bind(canvas);
    canvas.toBlob = (...args) => {
      window.encodings++;
      // Hold the JPEG callback so reconnect must finish asynchronously.
      window.releaseEncoding = () => encode(...args);
    };
    window.example = mountExampleBoard(
      document.querySelector("#example-board"),
      {
        locked: () => window.locked,
        connected: () => window.connected,
        record: () => {},
        publish: async () => {
          window.uploads++;
          return true;
        },
      },
    );
  }, base);
  await page.locator("summary").click();
  const box = await page.locator("canvas").boundingBox();
  await page.mouse.move(box.x + 20, box.y + 20);
  await page.mouse.down();
  await page.mouse.move(box.x + 40, box.y + 40);
  await page.mouse.up();
  assert.equal(await page.evaluate(() => window.example.flush()), false);
  assert.deepEqual(
    await page.evaluate(() => [window.encodings, window.uploads]),
    [0, 0],
  );
  await page.evaluate(() => {
    window.connected = true;
    return window.example.reconnect();
  });
  await page.waitForFunction(() => window.encodings === 1);
  assert.equal(await page.evaluate(() => window.uploads), 0);
  await page.evaluate(() => window.releaseEncoding());
  await page.waitForFunction(
    () =>
      window.uploads === 1 &&
      document.querySelector(".example-pane > [role=status]").textContent ===
        "Drawing ready for Jim to read when you refer to it.",
  );
  assert.equal(await page.evaluate(() => window.encodings), 1);
  await page.evaluate(() => {
    window.locked = true;
    window.example.refresh();
  });
  for (const action of ["undo", "clear"])
    assert.equal(
      await page.locator(`[data-action=${action}]`).isDisabled(),
      true,
    );
  await page.evaluate(() => {
    window.locked = false;
    window.example.refresh();
  });
  assert.equal(await page.locator("[data-action=undo]").isDisabled(), false);
  assert.equal(await page.locator("[data-action=clear]").isDisabled(), false);
  await page.evaluate(() => window.example.finish());
});

// The interview's own grid with a stand-in sidebar, so an expanded board can be
// checked against both the editor panel it covers and the sidebar it leaves.
async function openExpandableBoard(viewport) {
  const page = await browser.newPage({ viewport });
  await page.goto(`${base}/drawer?sidebar`);
  await page.waitForFunction(
    () =>
      getComputedStyle(document.querySelector(".drawer-resizer")).height ===
      "8px",
  );
  await page.evaluate(async (base) => {
    const { mountExampleBoard } = await import(`${base}/example-board.js`);
    const { mountInterviewDrawer } = await import(
      `${base}/interview-drawer.js`
    );
    window.uploads = [];
    window.turns = 0;
    document
      .querySelector("#yield-turn")
      .addEventListener("click", () => window.turns++);
    window.example = mountExampleBoard(
      document.querySelector("#example-board"),
      {
        locked: () => false,
        // Kept as the bytes Jim's read_board is handed, to decode later.
        publish: async (blob, strokes) => {
          const image = await new Promise((resolve) => {
            const reader = new FileReader();
            reader.onload = () => resolve(reader.result);
            reader.readAsDataURL(blob);
          });
          window.uploads.push({ strokes, image });
          return true;
        },
      },
    );
    mountInterviewDrawer(document.querySelector(".editor-panel"));
  }, base);
  await page.locator("#example-board summary").click();
  await page.waitForFunction(() =>
    document.querySelector(".example-surface").style.height.endsWith("px"),
  );
  return page;
}

function boardLayout(page) {
  return page.evaluate(() => {
    const box = (selector) => {
      const { x, y, width, height } = document
        .querySelector(selector)
        .getBoundingClientRect();
      return { x, y, width, height };
    };
    const surface = document.querySelector(".example-surface");
    return {
      panel: box(".editor-panel"),
      pane: box(".example-pane"),
      surface: box(".example-surface"),
      canvas: box(".example-surface canvas"),
      status: box(".example-pane > [role=status]"),
      hidden:
        surface.scrollHeight -
        surface.clientHeight +
        surface.scrollWidth -
        surface.clientWidth,
    };
  });
}

const isExpanded = (page) =>
  page
    .locator("#example-board")
    .evaluate((node) => node.hasAttribute("data-expanded"));

async function drawAcross(page, canvas, fromX, toX, y) {
  await page.mouse.move(canvas.x + canvas.width * fromX, canvas.y + y);
  await page.mouse.down();
  await page.mouse.move(canvas.x + canvas.width * toX, canvas.y + y, {
    steps: 4,
  });
  await page.mouse.up();
}

// The darkest red channel around each board point, on the canvas or in an
// image sent to Jim; both keep the board's 1600x1000 pixels.
function inkAt(page, points, upload = null) {
  return page.evaluate(
    async ({ points, upload }) => {
      let context = document
        .querySelector(".example-surface canvas")
        .getContext("2d");
      if (upload !== null) {
        const image = new Image();
        image.src = window.uploads.at(upload).image;
        await image.decode();
        context = document.createElement("canvas").getContext("2d");
        context.canvas.width = image.width;
        context.canvas.height = image.height;
        context.drawImage(image, 0, 0);
      }
      return points.map(([x, y]) => {
        const { data } = context.getImageData(x, y - 3, 1, 7);
        return Math.min(...data.filter((_, index) => index % 4 === 0));
      });
    },
    { points, upload },
  );
}

// The three sizes #579 measured.
for (const viewport of [
  { width: 1280, height: 800 },
  { width: 1440, height: 900 },
  { width: 1920, height: 1080 },
])
  test(`at ${viewport.width}x${viewport.height} the expanded example board shows the whole canvas over the editor panel only`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await openExpandableBoard(viewport);
    t.after(() => page.close());
    await page.getByRole("button", { name: "Expand drawing" }).click();
    const lifted = await boardLayout(page);
    assert.equal(lifted.hidden, 0);
    assert.ok(
      Math.abs(lifted.canvas.width / lifted.canvas.height - 1.6) < 0.01,
    );
    assert.ok(
      Math.abs(lifted.canvas.width - lifted.surface.width) <= 1 ||
        Math.abs(lifted.canvas.height - lifted.surface.height) <= 1,
      "the canvas fills the width or the height it is given",
    );
    const left = lifted.canvas.x - lifted.surface.x;
    const right =
      lifted.surface.x +
      lifted.surface.width -
      lifted.canvas.x -
      lifted.canvas.width;
    assert.ok(Math.abs(left - right) <= 1, "the canvas is centred");
    // The pane is the panel inside its 1px border, and the drawing takes the
    // height its toolbar and status line leave.
    for (const side of ["x", "y", "width", "height"])
      assert.ok(Math.abs(lifted.pane[side] - lifted.panel[side]) <= 2, side);
    assert.ok(
      lifted.pane.y +
        lifted.pane.height -
        lifted.status.y -
        lifted.status.height <=
        16,
    );
    await page.getByRole("button", { name: "Your turn is done" }).click();
    assert.equal(await page.evaluate(() => window.turns), 1);
  });

test("strokes drawn before and while expanded stay on the canvas, in Jim's image and undoable", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  // Board y=100 in the drawer, then board y=500 once expanded.
  const docked = await boardLayout(page);
  await drawAcross(page, docked.canvas, 0.2, 0.6, docked.canvas.height * 0.1);
  await page.waitForFunction(() => window.uploads.length === 1);
  await page.getByRole("button", { name: "Expand drawing" }).click();
  const { canvas } = await boardLayout(page);
  assert.ok((await inkAt(page, [[640, 100]]))[0] < 128);
  await drawAcross(page, canvas, 0.2, 0.6, canvas.height / 2);
  await page.getByRole("button", { name: "Shrink drawing" }).click();

  const [before, during, blank] = await inkAt(page, [
    [640, 100],
    [640, 500],
    [640, 800],
  ]);
  assert.ok(before < 128 && during < 128 && blank > 200);
  // What Jim reads is the sent image, not the screen.
  await page.waitForFunction(() => window.uploads.at(-1).strokes === 2);
  const sent = await inkAt(
    page,
    [
      [640, 100],
      [640, 500],
      [640, 800],
    ],
    -1,
  );
  assert.ok(sent[0] < 128, "Jim's image keeps the stroke from the drawer");
  assert.ok(sent[1] < 128, "Jim's image has the stroke drawn while expanded");
  assert.ok(sent[2] > 200, "Jim's image is not dark everywhere");

  await page.getByRole("button", { name: "Undo", exact: true }).click();
  const [kept, undone] = await inkAt(page, [
    [640, 100],
    [640, 500],
  ]);
  assert.ok(kept < 128 && undone > 200);
});

test("the button or Escape shrinks the board, but not from a sidebar field or mid-stroke", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  const docked = await boardLayout(page);
  const expand = page.getByRole("button", { name: "Expand drawing" });
  const shrink = page.getByRole("button", { name: "Shrink drawing" });
  const turn = page.getByRole("button", { name: "Your turn is done" });
  const icon = await expand.innerHTML();
  await expand.click();
  // The label names the action, so the button is not also a pressed toggle.
  assert.equal(await shrink.getAttribute("aria-pressed"), null);
  // It names Escape, the key that also shrinks it, as Undo names Ctrl+Z.
  assert.equal(await shrink.getAttribute("title"), "Shrink drawing (Esc)");
  assert.equal(await shrink.getAttribute("aria-keyshortcuts"), "Escape");
  assert.notEqual(await shrink.innerHTML(), icon);
  await shrink.click();
  assert.equal(await isExpanded(page), false);
  assert.equal(await expand.getAttribute("title"), "Expand drawing");
  assert.equal(await expand.getAttribute("aria-keyshortcuts"), null);
  await expand.click();
  const { canvas } = await boardLayout(page);

  // A field outside the board keeps Escape for itself, as a select does.
  await page.locator("#sidebar-field").focus();
  await page.keyboard.press("Escape");
  assert.equal(await isExpanded(page), true);

  // Mid-stroke, neither Escape nor the focused button changes the canvas size.
  await shrink.focus();
  await page.mouse.move(canvas.x + 100, canvas.y + 200);
  await page.mouse.down();
  await page.mouse.move(canvas.x + 300, canvas.y + 300, { steps: 3 });
  await page.keyboard.press("Escape");
  assert.equal(await isExpanded(page), true);
  await page.keyboard.press("Enter");
  assert.equal(await isExpanded(page), true);
  await page.mouse.up();

  // A sidebar button has no use for Escape, so from there it shrinks the board.
  await turn.click();
  await page.keyboard.press("Escape");
  assert.equal(await isExpanded(page), false);
  assert.equal(await expand.getAttribute("title"), "Expand drawing");
  assert.equal(await expand.innerHTML(), icon);
  // Focus stays in the sidebar the key came from.
  assert.equal(
    await page.evaluate(() => document.activeElement.id),
    "yield-turn",
  );
  const back = await boardLayout(page);
  assert.ok(Math.abs(back.canvas.width - docked.canvas.width) <= 1);
});

// Escape never moves focus: from the board's own button, a tool, or the
// sidebar, the candidate stays where they were.
test("shrinking by Escape leaves focus where it was, in the board or the sidebar", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  const toggle = page.locator("#example-board [data-action=expand]");
  const focused = (locator) =>
    locator.evaluate((node) => node === document.activeElement);
  for (const from of [
    toggle,
    page.getByRole("button", { name: "Pen", exact: true }),
    page.getByRole("button", { name: "Your turn is done" }),
  ]) {
    await toggle.click();
    assert.equal(await isExpanded(page), true);
    await from.focus();
    await page.keyboard.press("Escape");
    assert.equal(await isExpanded(page), false);
    assert.equal(
      await focused(from),
      true,
      (await from.getAttribute("aria-label")) ?? (await from.textContent()),
    );
  }
  // The button is the same element either way, so it keeps focus through the
  // label change.
  assert.equal(await toggle.getAttribute("aria-label"), "Expand drawing");
});

test("Escape from an input method, a modifier chord or a held key leaves the board expanded", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  await page.getByRole("button", { name: "Expand drawing" }).click();
  const turn = page.getByRole("button", { name: "Your turn is done" });
  // An input method's Escape, flagged by either field depending on the
  // browser, sent from outside any field so the field guard cannot stop it.
  for (const flag of [{ isComposing: true }, { keyCode: 229 }]) {
    await turn.evaluate(
      (node, flag) =>
        node.dispatchEvent(
          new KeyboardEvent("keydown", {
            key: "Escape",
            bubbles: true,
            ...flag,
          }),
        ),
      flag,
    );
    assert.equal(await isExpanded(page), true, JSON.stringify(flag));
  }
  await turn.focus();
  for (const chord of [
    "Alt+Escape",
    "Shift+Escape",
    "Control+Escape",
    "Meta+Escape",
  ]) {
    await page.keyboard.press(chord);
    assert.equal(await isExpanded(page), true, chord);
  }
  // A held key's repeats are not a new press.
  await page.evaluate(() =>
    document.activeElement.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        repeat: true,
        bubbles: true,
      }),
    ),
  );
  assert.equal(await isExpanded(page), true);
});

test("the covered editor panel is out of reach while expanded and all of it comes back on shrink", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  const active = () =>
    page.evaluate(
      () => document.activeElement.id || document.activeElement.tagName,
    );
  // Every control the pane covers, not only the ones Shift+Tab passes.
  const reachable = () =>
    page.evaluate(() =>
      [
        ...document
          .querySelector(".editor-panel")
          .querySelectorAll(
            "button, input, select, textarea, summary, [tabindex]",
          ),
      ]
        .filter(
          (node) =>
            !node.closest(".example-pane") &&
            node.checkVisibility({ visibilityProperty: true }),
        )
        .map((node) => node.id || node.textContent.trim() || node.tagName),
    );
  const docked = await reachable();
  assert.ok(docked.length > 3);
  // Twice, shrinking by the button and then by Escape from the sidebar.
  for (const shrinkBy of ["button", "Escape"]) {
    await page.getByRole("button", { name: "Expand drawing" }).click();
    assert.deepEqual(await reachable(), [], shrinkBy);
    await page.getByRole("button", { name: "Shrink drawing" }).focus();
    await page.keyboard.press("Shift+Tab");
    assert.equal(await active(), "yield-turn", shrinkBy);
    if (shrinkBy === "button")
      await page.getByRole("button", { name: "Shrink drawing" }).click();
    else await page.keyboard.press("Escape");
    assert.equal(await isExpanded(page), false, shrinkBy);
    assert.deepEqual(await reachable(), docked, shrinkBy);
  }
  await page.getByRole("button", { name: "Expand drawing" }).focus();
  await page.keyboard.press("Shift+Tab");
  assert.equal(await active(), "SUMMARY");
  await page.locator("#code").focus();
  assert.equal(await active(), "code");
});

// An ink stroke and each text-box gesture, against each key that resizes the
// board, docked and expanded; each key is asserted alone so a wrongly obeyed
// one cannot be undone by the next.
test("Expand, Shrink and Escape are ignored during a stroke or a text-box gesture, and the button works once it ends", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  const toggle = page.locator("#example-board [data-action=expand]");
  const draft = page.locator(".board-text-draft");
  const centre = (box) => ({
    x: box.x + box.width / 2,
    y: box.y + box.height / 2,
  });
  async function holdsThrough(from, to, keys, what) {
    const expanded = await isExpanded(page);
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(to.x, to.y, { steps: 3 });
    for (const key of keys) {
      // A new text box leaves focus in its label, so the button is focused
      // again first: otherwise Enter would never reach it and prove nothing.
      if (key !== "Escape") {
        await toggle.focus();
        assert.equal(
          await page.evaluate(() => document.activeElement.dataset.action),
          "expand",
        );
      }
      await page.keyboard.press(key);
      assert.equal(await isExpanded(page), expanded, `${what}, ${key}`);
    }
    await page.mouse.up();
  }
  for (const expanded of [false, true]) {
    const state = expanded ? "expanded" : "docked";
    if (expanded) await toggle.click();
    // Every key goes to the focused button, so Escape reaches the board's own
    // handler rather than the draft's, which listens on the box and canvas.
    const keys = expanded ? ["Enter", " ", "Escape"] : ["Enter", " "];
    const { canvas } = await boardLayout(page);
    const at = (x, y) => ({ x: canvas.x + x, y: canvas.y + y });
    await page.getByRole("button", { name: "Pen", exact: true }).click();
    await holdsThrough(at(60, 40), at(200, 90), keys, `${state} stroke`);
    await page.getByRole("button", { name: "Text", exact: true }).click();
    await holdsThrough(at(260, 30), at(420, 100), keys, `${state} new box`);
    // The box the new-box step drew stays open for the move and the scale.
    const edge = centre(await draft.locator("[data-edge=n]").boundingBox());
    await holdsThrough(
      edge,
      { x: edge.x + 30, y: edge.y + 10 },
      keys,
      `${state} moved box`,
    );
    const corner = centre(
      await draft.locator('[data-handle="se"]').boundingBox(),
    );
    await holdsThrough(
      corner,
      { x: corner.x + 30, y: corner.y + 10 },
      keys,
      `${state} scaled box`,
    );
    // Once the gesture is over, the button works again.
    await toggle.focus();
    await page.keyboard.press("Enter");
    assert.equal(await isExpanded(page), !expanded, `${state}, after`);
    if (!expanded) await toggle.click();
    // A closed draft is hidden rather than removed, so it is open when shown.
    if (await draft.isVisible())
      await page.getByRole("textbox", { name: "Label text" }).press("Escape");
  }
});

test("the ending wait covers an expanded board and keeps its own Escape", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  await page.getByRole("button", { name: "Expand drawing" }).click();
  await page
    .locator("#ending-overlay")
    .evaluate((node) => (node.hidden = false));
  const { pane } = await boardLayout(page);
  const covered = await page.evaluate(
    ([x, y]) =>
      Boolean(document.elementFromPoint(x, y).closest("#ending-overlay")),
    [pane.x + pane.width / 2, pane.y + pane.height / 2],
  );
  assert.equal(covered, true);
  // Escape inside that dialog is not the covered board's to take.
  await page
    .locator("#ending-overlay")
    .evaluate((node) =>
      node.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      ),
    );
  assert.equal(await isExpanded(page), true);
});

// Closing Example drawing while docked has nothing to shrink, so the scroll
// expanding would restore is not written either: the drawing opens again
// where the candidate left it.
test("closing Example drawing while docked leaves the drawing's scroll alone", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  const strip = page.locator(".example-surface");
  const board = page.locator("#example-board");
  await strip.evaluate((node) => (node.scrollTop = 100));
  // The details element fires toggle on a later task; waiting on a flag the
  // event sets fails on the wait's own deadline if it never comes.
  const toggle = (open) =>
    board.evaluate(
      (node, open) =>
        new Promise((done) => {
          node.addEventListener("toggle", done, { once: true });
          node.open = open;
        }),
      open,
    );
  await toggle(false);
  await toggle(true);
  assert.equal(await strip.evaluate((node) => node.scrollTop), 100);
});

test("a click that leaves focus in the editor moves it to Shrink", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  await page.locator("#code").focus();
  // Safari and Firefox on macOS do not focus a clicked button; a scripted
  // click does the same here.
  await page
    .locator("#example-board [data-action=expand]")
    .evaluate((node) => node.click());
  assert.equal(await isExpanded(page), true);
  assert.equal(
    await page.evaluate(() =>
      document.activeElement.getAttribute("aria-label"),
    ),
    "Shrink drawing",
  );
});

test("Escape another handler took, or one from a native dialog, leaves the board expanded", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  await page.getByRole("button", { name: "Expand drawing" }).click();
  // A handler that takes Escape with preventDefault alone, before it bubbles.
  const turn = page.getByRole("button", { name: "Your turn is done" });
  await turn.evaluate((node) =>
    node.addEventListener(
      "keydown",
      (event) => event.key === "Escape" && event.preventDefault(),
      { once: true },
    ),
  );
  await turn.focus();
  await page.keyboard.press("Escape");
  assert.equal(await isExpanded(page), true, "taken by another handler");
  // A native dialog carries no role attribute.
  await page.evaluate(() => {
    const dialog = document.createElement("dialog");
    dialog.innerHTML = '<button id="native-dialog-button">OK</button>';
    document.body.append(dialog);
    dialog.show();
  });
  await page.locator("#native-dialog-button").focus();
  await page.keyboard.press("Escape");
  assert.equal(await isExpanded(page), true, "from a native dialog");
});

// A text box near the canvas's bottom puts its controls below the canvas, so
// the drawing still has room to scroll; expanding from a strip scrolled down to
// it must show the canvas from its top.
test("expanding with a text box at the bottom shows the canvas from its top", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  // #579's short window, where the expanded canvas takes the pane's full
  // height and leaves no room below it for the box's controls.
  const page = await openExpandableBoard({ width: 1250, height: 630 });
  t.after(() => page.close());
  await page.getByRole("button", { name: "Text", exact: true }).click();
  const surface = page.locator(".example-surface");
  await surface.evaluate((node) => (node.scrollTop = node.scrollHeight));
  const box = await surface.boundingBox();
  await page.mouse.click(box.x + 120, box.y + box.height - 20);
  assert.ok(await page.locator(".board-text-draft").isVisible());
  await surface.evaluate((node) => (node.scrollTop = node.scrollHeight));
  await page.getByRole("button", { name: "Expand drawing" }).click();
  const { canvas } = await boardLayout(page);
  const top = await surface.evaluate(
    (node) => node.getBoundingClientRect().top,
  );
  assert.ok(canvas.y >= top - 1, `the canvas starts ${top - canvas.y}px above`);
});

test("shrinking puts the docked drawing back where it was scrolled", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  // A panel narrower than the docked canvas's 32rem, as a widened sidebar
  // leaves it, so the strip scrolls sideways as well as down.
  await page
    .locator(".editor-panel")
    .evaluate((node) => (node.style.width = "400px"));
  const strip = page.locator(".example-surface");
  const scrolled = () => strip.evaluate((node) => node.scrollTop);
  const across = () => strip.evaluate((node) => node.scrollLeft);
  await strip.evaluate((node) => {
    node.scrollTop = 100;
    node.scrollLeft = 60;
  });
  assert.deepEqual([await scrolled(), await across()], [100, 60]);
  await page.getByRole("button", { name: "Expand drawing" }).click();
  await page.getByRole("button", { name: "Shrink drawing" }).click();
  assert.deepEqual(
    [await scrolled(), await across()],
    [100, 60],
    "after a round trip",
  );
});

// An open text draft takes Escape first: its own listener on the box marks
// the key as taken and stops it before the board's on the document, so Escape
// from the draft's field or its Cancel button closes the draft and leaves the
// board expanded.
test("Escape from an open text draft closes the draft and leaves the board expanded", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  const draft = page.locator(".board-text-draft");
  const input = page.getByRole("textbox", { name: "Label text" });
  await page.getByRole("button", { name: "Expand drawing" }).click();
  await page.getByRole("button", { name: "Text", exact: true }).click();
  for (const from of ["field", "Cancel"]) {
    await page.locator("canvas").click({ position: { x: 80, y: 80 } });
    await input.fill("unfinished");
    if (from === "Cancel")
      await page.getByRole("button", { name: "Cancel", exact: true }).focus();
    await page.keyboard.press("Escape");
    assert.equal(await draft.isVisible(), false, `closed from the ${from}`);
    assert.equal(await isExpanded(page), true, `expanded after the ${from}`);
  }
});

// The page asks shrink() before running the tests, so an expanded board
// gives the drawer back; mid-gesture it stays, as for the button and Escape,
// and answers that the drawer is not shown, so nothing runs under it.
test("shrink() gives the drawer back and says so, unless a stroke or a text-box gesture is in progress", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  const shrink = () => page.evaluate(() => window.example.shrink());
  await page.getByRole("button", { name: "Expand drawing" }).click();
  await page.getByRole("button", { name: "Pen", exact: true }).click();
  const { canvas } = await boardLayout(page);
  await page.mouse.move(canvas.x + 60, canvas.y + 60);
  await page.mouse.down();
  await page.mouse.move(canvas.x + 160, canvas.y + 120);
  assert.equal(await shrink(), false, "the drawer is not shown mid-stroke");
  assert.equal(await isExpanded(page), true, "mid-stroke");
  await page.mouse.up();
  // A text box being drawn holds it the same way.
  await page.getByRole("button", { name: "Text", exact: true }).click();
  await page.mouse.move(canvas.x + 260, canvas.y + 60);
  await page.mouse.down();
  await page.mouse.move(canvas.x + 420, canvas.y + 120);
  assert.equal(await shrink(), false, "nor mid text box");
  assert.equal(await isExpanded(page), true, "mid text box");
  await page.mouse.up();
  assert.equal(await shrink(), true, "once the gesture is over");
  assert.equal(await isExpanded(page), false, "once the gesture is over");
});

test("the Escape that shrinks the board is marked as taken, and a docked one is not", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  // A listener registered after the board's, as another part of the page is.
  await page.evaluate(() =>
    window.addEventListener("keydown", (event) => {
      if (event.key === "Escape") globalThis.taken = event.defaultPrevented;
    }),
  );
  const taken = () => page.evaluate(() => globalThis.taken);
  const turn = page.getByRole("button", { name: "Your turn is done" });
  await page.getByRole("button", { name: "Expand drawing" }).click();
  await turn.focus();
  await page.keyboard.press("Escape");
  assert.equal(await isExpanded(page), false);
  assert.equal(await taken(), true, "the shrinking Escape");
  await page.keyboard.press("Escape");
  assert.equal(await taken(), false, "a docked Escape");
});

test("closing Example drawing shrinks an expanded board and frees the editor", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openExpandableBoard({ width: 1280, height: 800 });
  t.after(() => page.close());
  await page.getByRole("button", { name: "Expand drawing" }).click();
  await page.locator("#example-board").evaluate((node) => (node.open = false));
  // The details element fires toggle on a later task.
  await page.waitForFunction(
    () =>
      !document.querySelector("#example-board").hasAttribute("data-expanded"),
  );
  assert.ok(
    await page
      .locator(".editor-stack")
      .evaluate((node) => node.checkVisibility({ visibilityProperty: true })),
  );
});
