import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { launchChromium, startStaticServer } from "./source.js";
import { openShapeBoard, moveBoardPointer } from "./example-board-fixture.js";

let browser;
let server;
let base;

before(async () => {
  // Local Ubuntu shell probes included cases with no capture or loss event.
  // Keep these real off-board drags in the full browser, without a stub.
  browser = await launchChromium({ channel: "chromium" });
  if (browser)
    ({ server, base } = await startStaticServer({
      handle: (_request, response, url) => {
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
    const page = await openShapeBoard(t, browser, base);
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
    await moveBoardPointer(page, ...start);
    await page.mouse.down();
    await moveBoardPointer(page, ...end);
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
