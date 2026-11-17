import { read } from "./source.js";

export function exampleMarkup(source = "web/interview.html") {
  const html = read(source);
  const start = html.search(/<details id="(?:example|task)-board"/);
  return html.slice(start, html.indexOf("</details>", start) + 10);
}

export async function openShapeBoard(t, browser, base, source) {
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(base);
  await page.setContent(exampleMarkup(source));
  await page.evaluate(async (base) => {
    const { mountExampleBoard } = await import(`${base}/example-board.js`);
    const { BOARD_WIDTH, BOARD_HEIGHT, MAX_STROKES, MAX_STROKES_MESSAGE } =
      await import(`${base}/whiteboard.js`);
    const root = document.querySelector("details");
    root.open = true;
    const canvas = root.querySelector("canvas");
    canvas.style.width = "640px";
    canvas.style.height = "400px";
    // Synthetic pointers have no native capture slot. The capture suite
    // removes this instance stub before its real off-board mouse drags.
    canvas.setPointerCapture = () => {};
    window.ops = [];
    window.turnActivity = 0;
    window.online = false;
    window.locked = false;
    window.uploads = [];
    window.example = mountExampleBoard(root, {
      turnActivity: () => window.turnActivity++,
      locked: () => window.locked,
      connected: () => window.online,
      record: (ops) => window.ops.push(...ops),
      publish: async (blob, count) => {
        const image = await new Promise((resolve) => {
          const reader = new FileReader();
          reader.onload = () => resolve(reader.result);
          reader.readAsDataURL(blob);
        });
        window.uploads.push({ image, count });
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

export async function moveBoardPointer(page, x, y) {
  const point = await page.locator("canvas").evaluate(
    (canvas, { x, y }) => {
      const bounds = canvas.getBoundingClientRect();
      return {
        x: bounds.x + (x * bounds.width) / canvas.width,
        y: bounds.y + (y * bounds.height) / canvas.height,
      };
    },
    { x, y },
  );
  await page.mouse.move(point.x, point.y);
}
