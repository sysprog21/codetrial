import test from "node:test";
import assert from "node:assert/strict";
import {
  shapePoints,
  commitShape,
  createDrawingTurn,
} from "../../web/example-board.js";
import { createBoard, applyOp, MAX_STROKES } from "../../web/whiteboard.js";

function drawingClock(options = {}) {
  let at = 0;
  let blocked = false;
  const frames = [];
  const painted = [];
  const sent = [];
  const turn = createDrawingTurn({
    now: () => at,
    requestFrame: (frame) => frames.push(frame),
    windowMs: () => 3000,
    blocked: () => blocked,
    paint: (progress) => painted.push(progress),
    flush: async () => sent.push("image"),
    yieldTurn: async () => sent.push("yield"),
    ...options,
  });
  return {
    turn,
    painted,
    sent,
    time: (next) => (at = next),
    block: () => (blocked = true),
    async frame(next) {
      at = next;
      frames.shift()?.();
      await new Promise((resolve) => setImmediate(resolve));
    },
  };
}

test("drawing silence fills the ring and sends the image before yielding, once", async () => {
  const clock = drawingClock();
  await clock.frame(0);
  assert.deepEqual(clock.sent, [], "no reply before drawing");
  clock.turn.activity();
  assert.equal(clock.painted.at(-1), 0);
  await clock.frame(1500);
  assert.equal(clock.painted.at(-1), 0.5);
  clock.turn.activity();
  await clock.frame(3000);
  assert.equal(clock.painted.at(-1), 0.5, "another stroke resets silence");
  await clock.frame(4500);
  assert.deepEqual(clock.sent, ["image", "yield"]);
  assert.equal(clock.turn.pending(), false);
  assert.equal(clock.painted.at(-1), null);
  await clock.frame(9000);
  assert.deepEqual(clock.sent, ["image", "yield"]);
  clock.turn.activity();
  await clock.frame(12000);
  assert.deepEqual(clock.sent, ["image", "yield", "image", "yield"]);
});

test("speech while drawing resets the same countdown", async () => {
  const clock = drawingClock();
  clock.turn.activity();
  await clock.frame(2000);
  clock.turn.speech();
  await clock.frame(3000);
  assert.equal(clock.painted.at(-1), 1 / 3);
  assert.deepEqual(clock.sent, []);
  await clock.frame(5000);
  assert.deepEqual(clock.sent, ["image", "yield"]);
});

test("drawing waits for the published window and drops a blocked turn", async () => {
  const clock = drawingClock({ windowMs: () => null });
  clock.turn.activity();
  await clock.frame(100000);
  assert.deepEqual(clock.sent, []);
  assert.equal(clock.painted.at(-1), null);
  clock.block();
  await clock.frame(100001);
  assert.equal(clock.turn.pending(), false);
  clock.turn.activity();
  assert.equal(clock.turn.pending(), false);
});

test("new ink, speech, or a blocked interview cancels a handover during upload", async () => {
  for (const change of ["activity", "speech", "block"]) {
    let uploaded;
    const clock = drawingClock({
      flush: () => new Promise((resolve) => (uploaded = resolve)),
    });
    clock.turn.activity();
    await clock.frame(3000);
    clock.time(3100);
    if (change === "block") clock.block();
    else clock.turn[change]();
    uploaded(true);
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepEqual(clock.sent, [], change);
  }
});

test("an image failure still yields and the button can yield before the countdown", async () => {
  for (const flush of [
    async () => false,
    async () => {
      throw new Error("upload failed");
    },
  ]) {
    const clock = drawingClock({ flush });
    clock.turn.activity();
    await clock.turn.yield();
    assert.deepEqual(clock.sent, ["yield"]);
    assert.equal(clock.turn.pending(), false);
  }
});

test("a slow yield owns the ring and cannot hide a new drawing when it completes", async () => {
  let delivered;
  const clock = drawingClock({
    yieldTurn: () => new Promise((resolve) => (delivered = resolve)),
  });
  clock.turn.activity();
  await clock.frame(3000);
  assert.equal(
    clock.turn.pending(),
    true,
    "the microphone must wait for delivery",
  );
  assert.equal(clock.painted.at(-1), null, "hide before waiting for delivery");
  clock.time(3100);
  clock.turn.activity();
  assert.equal(clock.painted.at(-1), 0);
  delivered();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(
    clock.painted.at(-1),
    0,
    "old delivery must not erase the new ring",
  );
  assert.equal(clock.turn.pending(), true);
  await clock.frame(4600);
  assert.equal(clock.painted.at(-1), 0.5);
});

test("rectangles close and work when dragged backwards", () => {
  assert.deepEqual(
    shapePoints("rectangle", { x: 30, y: 40 }, { x: 10, y: 20 }),
    [30, 40, 10, 40, 10, 20, 30, 20, 30, 40],
  );
});

test("ellipses touch the four sides of the dragged bounding box", () => {
  const points = shapePoints("ellipse", { x: 10, y: 20 }, { x: 110, y: 80 });
  assert.equal(points.length, 130);
  for (const [index, expected] of [
    [0, 110],
    [1, 50],
    [32, 60],
    [33, 80],
    [64, 10],
    [65, 50],
    [96, 60],
    [97, 20],
  ])
    assert.ok(Math.abs(points[index] - expected) < 0.000001);
});

test("arrows have two heads behind the endpoint and a zero-length drag stays finite", () => {
  const points = shapePoints("arrow", { x: 0, y: 0 }, { x: 100, y: 0 });
  assert.deepEqual(points.slice(0, 4), [0, 0, 100, 0]);
  assert.ok(points[4] < 100 && points[8] < 100);
  assert.ok(points[5] > 0 && points[9] < 0);
  assert.deepEqual(
    shapePoints("arrow", { x: 4, y: 5 }, { x: 4, y: 5 }),
    [4, 5, 4, 5],
  );
});

test("each shape is one undoable, replayable stroke", () => {
  for (const tool of ["rectangle", "ellipse", "line", "arrow"]) {
    const model = createBoard();
    assert.equal(
      commitShape(model, tool, "#101418", { x: 40, y: 50 }, { x: 160, y: 170 }),
      true,
    );
    const original = model.strokes();
    const ops = model.takeOps();
    assert.equal(original.length, 1);
    assert.equal(ops.length, 1);
    const replay = createBoard();
    assert.equal(applyOp(replay, ops[0]), true);
    assert.deepEqual(replay.strokes(), original);
    assert.equal(model.undo(), true);
    assert.equal(model.strokeCount(), 0);
    assert.equal(model.redo(), true);
    assert.deepEqual(model.strokes(), original);
    assert.equal(model.clear(), true);
    assert.equal(model.undo(), true);
    assert.deepEqual(model.strokes(), original);
  }
});

test("shapes respect the shared stroke ceiling without replacing existing work", () => {
  const model = createBoard();
  for (let i = 0; i < MAX_STROKES; i++) {
    model.begin("pen", "#101418", i, i);
    model.end();
  }
  const before = model.strokes();
  assert.equal(
    commitShape(model, "line", "#101418", { x: 1, y: 2 }, { x: 3, y: 4 }),
    false,
  );
  assert.deepEqual(model.strokes(), before);
});

test("coding reconnect sends queued code before the latest buffer", async () => {
  const { reconnectCodingInterview } =
    await import("../../web/example-board.js");
  const order = [];
  await reconnectCodingInterview({
    ending: false,
    board: {
      reconnect: async () => {
        order.push("drawing");
        return false;
      },
    },
    flush: () => order.push("queued"),
    publishCode: () => order.push("latest"),
  });
  assert.deepEqual(order, ["queued", "drawing", "latest"]);
});

test("ending reconnect releases the end event after the drawing attempt, even on failure", async () => {
  const { reconnectCodingInterview } =
    await import("../../web/example-board.js");
  for (const fails of [false, true]) {
    const order = [];
    let release;
    const pending = reconnectCodingInterview({
      ending: true,
      board: {
        reconnect: () =>
          new Promise((resolve, reject) => {
            release = () => {
              order.push("drawing");
              fails ? reject(new Error("offline")) : resolve(false);
            };
          }),
      },
      flush: () => order.push("end"),
      publishCode: () => order.push("latest"),
    });
    assert.deepEqual(order, []);
    release();
    if (fails) await assert.rejects(pending, /offline/);
    else await pending;
    assert.deepEqual(order, ["drawing", "end", "latest"]);
  }
});
