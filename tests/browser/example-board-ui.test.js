import { after, before, test } from "node:test";
import assert from "node:assert/strict";
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
          response.end(drawerMarkup());
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

function drawerMarkup() {
  return `<!doctype html><html><head><link rel="stylesheet" href="/styles.css"></head><body><section class="editor-panel" data-example-drawing="true"><div class="editor-stack"><textarea id="code">const keep = 42;</textarea></div><div class="drawer-resizer" role="separator" tabindex="0" aria-label="Resize drawing and test panel"></div><section class="test-drawer"><details><summary>Add a case</summary><input></details>${exampleMarkup()}<button>Test results</button><div id="results-body" hidden></div></section></section></body></html>`;
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
  const canvas = page.locator("canvas");
  async function draw() {
    const box = await canvas.boundingBox();
    await page.mouse.move(box.x + 20, box.y + 20);
    await page.mouse.down();
    await page.mouse.move(box.x + 80, box.y + 40);
    await page.mouse.up();
  }

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
  assert.equal(await red.getAttribute("aria-pressed"), "true");

  await eraser.click();
  assert.equal(await pen.getAttribute("aria-pressed"), "false");
  assert.equal(await eraser.getAttribute("aria-pressed"), "true");
  assert.equal(await red.getAttribute("aria-pressed"), "true");
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
    ["Pen", "Eraser"],
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
      document.querySelector("#example-board [role=status]").textContent ===
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
