// The board's model and its renderer. Neither touches the DOM: the model is
// plain data, and the renderer is given a context, so the recording stub below
// is enough to assert what a browser would have been asked to paint.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  applyOp,
  BOARD_BACKGROUND,
  BOARD_HEIGHT,
  BOARD_WIDTH,
  ERASER_WIDTH,
  ERASER_WIDTHS,
  MAX_POINTS,
  MAX_STROKE_WIDTH,
  MAX_STROKES,
  PEN_COLORS,
  PEN_WIDTH,
  boardPoint,
  createBoard,
  createBoardPainter,
  createPointerGate,
  drawBoard,
} from "../../web/whiteboard.js";

const BLACK = PEN_COLORS[0].color;

/// Draws one stroke from (x1,y1) to (x2,y2), the way a press-drag-release does.
function stroke(board, x1, y1, x2, y2, tool = "pen") {
  board.begin(tool, BLACK, x1, y1);
  board.extend(x2, y2);
  return board.end();
}

/// A 2D context that records the calls instead of painting them.
function recordingContext() {
  const calls = [];
  const record =
    (name) =>
    (...args) =>
      calls.push([name, ...args]);
  return {
    calls,
    save: record("save"),
    restore: record("restore"),
    beginPath: record("beginPath"),
    moveTo: record("moveTo"),
    lineTo: record("lineTo"),
    stroke: record("stroke"),
    fill: record("fill"),
    arc: record("arc"),
    fillRect: record("fillRect"),
    drawImage: record("drawImage"),
    fillText: record("fillText"),
    measureText: () => ({
      fontBoundingBoxAscent: 30,
      fontBoundingBoxDescent: 8,
    }),
  };
}

/// A painter whose layer is a recording context, and the sizes it was asked
/// to make the layer at.
function recordingPainter() {
  const layer = recordingContext();
  const canvas = { getContext: () => layer };
  const made = [];
  const painter = createBoardPainter((width, height) => {
    made.push([width, height]);
    return canvas;
  });
  return { painter, layer, canvas, made };
}

/// How many strokes a context was asked to paint.
function strokesPainted(context) {
  return context.calls.filter(([name]) => name === "stroke").length;
}

test("text is normalized, serialized and rendered at explicit board coordinates", () => {
  const board = createBoard();
  assert.equal(
    board.addText({
      text: "head\r\ntail",
      color: BLACK,
      x: 120,
      y: 80,
      fontSize: 32,
    }),
    true,
  );
  const operations = JSON.parse(JSON.stringify(board.takeOps()));
  assert.deepEqual(operations, [
    {
      op: "text",
      text: "head\ntail",
      color: BLACK,
      x: 120,
      y: 80,
      fontSize: 32,
    },
  ]);
  const replay = createBoard();
  operations.forEach((op) => applyOp(replay, op));
  assert.deepEqual(replay.strokes(), board.strokes());
  const context = recordingContext();
  drawBoard(context, replay.strokes());
  assert.deepEqual(
    context.calls.filter(([name]) => name === "fillText"),
    [
      ["fillText", "head", 120, 111],
      ["fillText", "tail", 120, 151],
    ],
  );
  assert.equal(context.font, "32px Arial, sans-serif");
  assert.equal(context.textBaseline, "alphabetic");
  assert.equal(context.fillStyle, BLACK);
});

test("invalid text does not discard redo or add recorded operations", () => {
  const board = createBoard();
  stroke(board, 10, 10, 20, 20);
  board.undo();
  board.takeOps();
  const valid = { text: "head", color: BLACK, x: 20, y: 20, fontSize: 32 };
  for (const invalid of [
    { text: "  \n " },
    { text: "x".repeat(501) },
    { text: "x\n".repeat(20) },
    { color: "red" },
    { x: NaN },
    { x: -1 },
    { y: 999 },
    { fontSize: 0 },
    { fontSize: 129 },
  ]) {
    assert.equal(board.addText({ ...valid, ...invalid }), false);
    assert.equal(board.canRedo(), true);
    assert.deepEqual(board.strokes(), []);
    assert.deepEqual(board.takeOps(), []);
  }
  assert.equal(board.redo(), true);
});

test("text, ink, erasing and clear replay each history transition", () => {
  const board = createBoard();
  const replay = createBoard();
  const actions = [
    () => stroke(board, 5, 5, 50, 50),
    () =>
      board.addText({
        text: "O(n)\ntime",
        color: BLACK,
        x: 60,
        y: 60,
        fontSize: 24,
      }),
    () => stroke(board, 60, 60, 90, 60, "eraser"),
    () => board.undo(),
    () => board.redo(),
    () => board.clear(),
    () => board.undo(),
    () => board.redo(),
    () => board.undo(),
    () => board.undo(),
    () => board.undo(),
    () => board.redo(),
  ];
  for (const action of actions) {
    assert.equal(action(), true);
    JSON.parse(JSON.stringify(board.takeOps())).forEach((op) =>
      applyOp(replay, op),
    );
    assert.deepEqual(replay.strokes(), board.strokes());
    assert.equal(replay.canRedo(), board.canRedo());
    const live = recordingContext();
    const recorded = recordingContext();
    drawBoard(live, board.strokes());
    drawBoard(recorded, replay.strokes());
    assert.deepEqual(recorded.calls, live.calls);
  }
  assert.equal(board.strokes().at(-1).kind, "text");
});

test("a stroke is kept as the points it was drawn from", () => {
  const board = createBoard();
  assert.equal(stroke(board, 10, 20, 30, 40), true);
  assert.deepEqual(board.strokes(), [
    { color: BLACK, width: PEN_WIDTH, points: [10, 20, 30, 40] },
  ]);
  assert.equal(board.strokeCount(), 1);
  assert.equal(board.isDrawing(), false);
});

test("the strokes a caller is handed cannot move the board", () => {
  const board = createBoard();
  stroke(board, 1, 1, 2, 2);
  board.strokes().push({ color: BLACK, width: PEN_WIDTH, points: [0, 0] });
  assert.equal(board.strokeCount(), 1);

  // The strokes in the list are the board's own, frozen rather than copied,
  // so a point changed through them throws instead of redrawing the board
  // behind the history's back.
  const [first] = board.strokes();
  assert.throws(() => {
    first.points[0] = 999;
  }, TypeError);
  assert.throws(() => {
    first.color = "#ffffff";
  }, TypeError);
  assert.deepEqual(board.strokes()[0], {
    color: BLACK,
    width: PEN_WIDTH,
    points: [1, 1, 2, 2],
  });
});

test("points outside the board are clamped onto it", () => {
  const board = createBoard();
  board.begin("pen", BLACK, -40, -10);
  board.extend(BOARD_WIDTH + 500, BOARD_HEIGHT + 500);
  board.end();
  assert.deepEqual(board.strokes()[0].points, [
    0,
    0,
    BOARD_WIDTH,
    BOARD_HEIGHT,
  ]);
});

test("a point the pointer repeated is not a point", () => {
  const board = createBoard();
  board.begin("pen", BLACK, 5, 5);
  assert.equal(board.extend(5, 5), false);
  assert.equal(board.extend(5.4, 5.4), false, "the same pixel after rounding");
  assert.equal(board.extend(6, 5), true);
  assert.deepEqual(board.strokes()[0].points, [5, 5, 6, 5]);
});

test("the eraser paints the background so an erasure is a stroke like any other", () => {
  const board = createBoard();
  stroke(board, 0, 0, 10, 10);
  stroke(board, 0, 0, 10, 10, "eraser");
  assert.deepEqual(
    board.strokes().map((item) => [item.color, item.width]),
    [
      [BLACK, PEN_WIDTH],
      [BOARD_BACKGROUND, ERASER_WIDTH],
    ],
  );

  // The erasure undoes, which is the property the composite-operation
  // alternative does not have: it would have removed pixels there is no
  // record of.
  board.undo();
  assert.deepEqual(
    board.strokes().map((item) => item.color),
    [BLACK],
  );
});

test("an erasure is a stroke but leaves no ink to count", () => {
  const board = createBoard();
  for (let swipe = 0; swipe < 3; swipe += 1)
    stroke(board, 0, 0, 10, 10, "eraser");
  assert.equal(board.strokeCount(), 3);
  assert.equal(board.inkCount(), 0, "three swipes on an empty board");
  stroke(board, 0, 0, 10, 10);
  assert.equal(board.inkCount(), 1);
});

test("the eraser draws at the size picked, and only at an offered size", () => {
  const largest = Math.max(...ERASER_WIDTHS);
  assert.ok(ERASER_WIDTHS.includes(ERASER_WIDTH));
  assert.equal(MAX_STROKE_WIDTH, largest);

  const board = createBoard();
  for (const width of ERASER_WIDTHS) {
    board.begin("eraser", BLACK, 0, 0, width);
    board.end();
  }
  // The pen ignores the eraser's size, and a size no button offers falls back
  // to the default rather than reaching the board.
  board.begin("pen", BLACK, 0, 0, largest);
  board.end();
  board.begin("eraser", BLACK, 0, 0, 37);
  board.end();
  board.begin("eraser", BLACK, 0, 0, largest);
  board.extend(10, 10);
  board.end();
  board.begin("eraser", BLACK, 0, 0, 999);
  board.end();
  assert.deepEqual(
    board.strokes().map((item) => item.width),
    [...ERASER_WIDTHS, PEN_WIDTH, ERASER_WIDTH, largest, ERASER_WIDTH],
  );

  // And the journal carries the size, so a replay rebuilds the same erasure.
  const replayed = createBoard();
  for (const op of board.takeOps()) assert.equal(applyOp(replayed, op), true);
  assert.deepEqual(
    replayed.strokes().map((item) => item.width),
    [...ERASER_WIDTHS, PEN_WIDTH, ERASER_WIDTH, largest, ERASER_WIDTH],
  );
});

test("undo and redo walk the same strokes back and forward", () => {
  const board = createBoard();
  stroke(board, 0, 0, 1, 1);
  stroke(board, 2, 2, 3, 3);
  assert.equal(board.canUndo(), true);
  assert.equal(board.canRedo(), false);

  assert.equal(board.undo(), true);
  assert.equal(board.strokeCount(), 1);
  assert.equal(board.canRedo(), true);
  assert.equal(board.redo(), true);
  assert.deepEqual(board.strokes()[1].points, [2, 2, 3, 3]);

  assert.equal(board.redo(), false, "nothing left to redo");
  board.undo();
  board.undo();
  assert.equal(board.undo(), false, "nothing left to undo");
  assert.equal(board.strokeCount(), 0);
});

test("drawing after an undo drops what redo would have brought back", () => {
  const board = createBoard();
  stroke(board, 0, 0, 1, 1);
  board.undo();
  stroke(board, 5, 5, 6, 6);
  assert.equal(board.canRedo(), false);
  assert.deepEqual(board.strokes()[0].points, [5, 5, 6, 6]);
});

test("a clear is undone in one piece", () => {
  const board = createBoard();
  stroke(board, 0, 0, 1, 1);
  stroke(board, 2, 2, 3, 3);
  stroke(board, 4, 4, 5, 5);
  assert.equal(board.clear(), true);
  assert.equal(board.strokeCount(), 0);
  assert.equal(board.canUndo(), true);
  assert.equal(board.canRedo(), false);
  assert.equal(board.clear(), false, "a cleared board has nothing to clear");
  assert.equal(board.undo(), true);
  assert.deepEqual(
    board.strokes().map((item) => item.points),
    [
      [0, 0, 1, 1],
      [2, 2, 3, 3],
      [4, 4, 5, 5],
    ],
  );
  assert.equal(board.redo(), true);
  assert.equal(board.strokeCount(), 0);
  assert.equal(
    createBoard().clear(),
    false,
    "an empty board has nothing to clear",
  );
});

test("a clear stays redoable under the strokes undone from beneath it", () => {
  const board = createBoard();
  stroke(board, 0, 0, 1, 1);
  stroke(board, 2, 2, 3, 3);
  board.clear();
  board.undo();
  board.undo();
  assert.equal(board.strokeCount(), 1);

  // Forward again: the stroke, then the clear it sat under. A flag for the
  // clear was reset by the second undo, and the cleared board could not be
  // reached by redo at all.
  assert.equal(board.redo(), true);
  assert.equal(board.strokeCount(), 2);
  assert.equal(board.canRedo(), true);
  assert.equal(board.redo(), true);
  assert.equal(board.strokeCount(), 0);
  assert.equal(board.canRedo(), false);

  // And the journal says the same, so a replay lands on the same board.
  const replayed = createBoard();
  for (const op of board.takeOps()) applyOp(replayed, op);
  assert.equal(replayed.strokeCount(), 0);
  assert.equal(replayed.undo(), true);
  assert.equal(replayed.strokeCount(), 2);
});

test("an edit in the middle of a stroke is refused", () => {
  const board = createBoard();
  stroke(board, 0, 0, 1, 1);
  board.begin("pen", BLACK, 9, 9);
  assert.equal(board.undo(), false);
  assert.equal(board.redo(), false);
  assert.equal(board.clear(), false);
  assert.equal(board.isDrawing(), true);
  assert.equal(board.end(), true);
  assert.equal(board.undo(), true);
});

test("the board stops accepting rather than dropping the oldest work", () => {
  const board = createBoard();
  for (let index = 0; index < MAX_STROKES; index += 1) {
    assert.equal(stroke(board, index, 0, index, 1), true, `stroke ${index}`);
  }
  assert.equal(board.begin("pen", BLACK, 0, 0), false);
  assert.equal(board.strokeCount(), MAX_STROKES);

  const long = createBoard();
  long.begin("pen", BLACK, 0, 0);
  for (let index = 1; index < MAX_POINTS; index += 1) {
    assert.equal(
      long.extend(index % BOARD_WIDTH, index),
      true,
      `point ${index}`,
    );
  }
  assert.equal(long.extend(7, 7), false);
  assert.equal(long.strokes()[0].points.length, MAX_POINTS * 2);
});

test("a board is painted background first, then its strokes in order", () => {
  const board = createBoard();
  stroke(board, 0, 0, 10, 10);
  const context = recordingContext();
  drawBoard(context, board.strokes());
  assert.deepEqual(context.calls[0], ["save"]);
  assert.deepEqual(context.calls[1], [
    "fillRect",
    0,
    0,
    BOARD_WIDTH,
    BOARD_HEIGHT,
  ]);
  assert.deepEqual(context.calls.slice(2), [
    ["beginPath"],
    ["moveTo", 0, 0],
    ["lineTo", 10, 10],
    ["stroke"],
    ["restore"],
  ]);
});

test("a click with no drag is painted as a dot", () => {
  // A one-point path strokes nothing at all in a canvas, so the mark the
  // candidate made would be missing from the board and from the image the
  // interviewer is sent.
  const board = createBoard();
  board.begin("pen", BLACK, 40, 50);
  board.end();
  const context = recordingContext();
  drawBoard(context, board.strokes());
  assert.deepEqual(context.calls.slice(2), [
    ["beginPath"],
    ["arc", 40, 50, PEN_WIDTH / 2, 0, Math.PI * 2],
    ["fill"],
    ["restore"],
  ]);
});

test("a move repaints the stroke being drawn, not the board under it", () => {
  const board = createBoard();
  stroke(board, 0, 0, 10, 10);
  stroke(board, 20, 20, 30, 30);
  stroke(board, 40, 40, 50, 50);
  board.begin("pen", BLACK, 60, 60);
  board.extend(70, 70);
  const { painter, layer, canvas, made } = recordingPainter();
  const screen = recordingContext();

  painter.paint(screen, board.strokes(), board.isDrawing());
  assert.deepEqual(made, [[BOARD_WIDTH, BOARD_HEIGHT]]);
  assert.equal(strokesPainted(layer), 3);
  assert.deepEqual(screen.calls[0], ["drawImage", canvas, 0, 0]);
  assert.equal(strokesPainted(screen), 1);

  // Every later move: the layer is left alone, and the screen is the layer
  // with the open stroke over it.
  layer.calls.length = 0;
  screen.calls.length = 0;
  board.extend(80, 80);
  painter.paint(screen, board.strokes(), board.isDrawing());
  assert.deepEqual(layer.calls, []);
  assert.deepEqual(screen.calls[0], ["drawImage", canvas, 0, 0]);
  assert.equal(strokesPainted(screen), 1);
  assert.deepEqual(made, [[BOARD_WIDTH, BOARD_HEIGHT]]);
});

test("a stroke that ends is added to the layer on its own", () => {
  const board = createBoard();
  stroke(board, 0, 0, 10, 10);
  const { painter, layer } = recordingPainter();
  const screen = recordingContext();
  painter.paint(screen, board.strokes(), board.isDrawing());

  layer.calls.length = 0;
  stroke(board, 20, 20, 30, 30);
  painter.paint(screen, board.strokes(), board.isDrawing());
  assert.equal(strokesPainted(layer), 1);
  assert.equal(
    layer.calls.some(([name]) => name === "fillRect"),
    false,
  );
});

test("an undo or a clear repaints the layer from the board", () => {
  const board = createBoard();
  stroke(board, 0, 0, 10, 10);
  stroke(board, 20, 20, 30, 30);
  const { painter, layer } = recordingPainter();
  const screen = recordingContext();
  painter.paint(screen, board.strokes(), board.isDrawing());

  layer.calls.length = 0;
  board.undo();
  painter.paint(screen, board.strokes(), board.isDrawing());
  assert.deepEqual(layer.calls[1], [
    "fillRect",
    0,
    0,
    BOARD_WIDTH,
    BOARD_HEIGHT,
  ]);
  assert.equal(strokesPainted(layer), 1);

  layer.calls.length = 0;
  board.clear();
  painter.paint(screen, board.strokes(), board.isDrawing());
  assert.equal(layer.calls[1][0], "fillRect");
  assert.equal(strokesPainted(layer), 0);
});

test("a board as long as the layer but with other strokes is repainted", () => {
  // An undo and a new stroke before the next paint leave the board the length
  // it was, so a layer reused by count would keep the stroke that was undone.
  const board = createBoard();
  stroke(board, 0, 0, 10, 10);
  stroke(board, 20, 20, 30, 30);
  const { painter, layer } = recordingPainter();
  const screen = recordingContext();
  painter.paint(screen, board.strokes(), board.isDrawing());

  layer.calls.length = 0;
  board.undo();
  stroke(board, 40, 40, 50, 50);
  painter.paint(screen, board.strokes(), board.isDrawing());
  assert.equal(layer.calls[1][0], "fillRect");
  assert.deepEqual(
    layer.calls.filter(([name]) => name === "moveTo"),
    [
      ["moveTo", 0, 0],
      ["moveTo", 40, 40],
    ],
  );
});

test("a stroke cancelled for a pen leaves the screen with the layer alone", () => {
  // A palm's stroke is open, so it is on the screen and not on the layer. The
  // layer is reused when the pen cancels it, and copying the layer over the
  // screen is what takes the palm's ink off.
  const board = createBoard();
  stroke(board, 0, 0, 10, 10);
  board.begin("pen", BLACK, 20, 20);
  board.extend(30, 30);
  const { painter, layer, canvas } = recordingPainter();
  const screen = recordingContext();
  painter.paint(screen, board.strokes(), board.isDrawing());
  assert.equal(strokesPainted(screen), 1);

  layer.calls.length = 0;
  screen.calls.length = 0;
  board.cancel();
  painter.paint(screen, board.strokes(), board.isDrawing());
  assert.deepEqual(layer.calls, []);
  assert.deepEqual(screen.calls, [["drawImage", canvas, 0, 0]]);

  screen.calls.length = 0;
  board.begin("pen", BLACK, 40, 40);
  board.extend(50, 50);
  painter.paint(screen, board.strokes(), board.isDrawing());
  assert.deepEqual(layer.calls, []);
  assert.deepEqual(screen.calls[0], ["drawImage", canvas, 0, 0]);
  assert.deepEqual(
    screen.calls.filter(([name]) => name === "moveTo"),
    [["moveTo", 40, 40]],
  );
});

test("an undone clear draws the strokes back onto the empty layer", () => {
  // The layer a clear leaves holds no strokes, so the restored ones extend it
  // and are drawn over its background without painting it again.
  const board = createBoard();
  stroke(board, 0, 0, 10, 10);
  stroke(board, 20, 20, 30, 30);
  const { painter, layer } = recordingPainter();
  const screen = recordingContext();
  painter.paint(screen, board.strokes(), board.isDrawing());
  board.clear();
  painter.paint(screen, board.strokes(), board.isDrawing());

  layer.calls.length = 0;
  board.undo();
  painter.paint(screen, board.strokes(), board.isDrawing());
  assert.equal(
    layer.calls.some(([name]) => name === "fillRect"),
    false,
  );
  assert.deepEqual(
    layer.calls.filter(([name]) => name === "moveTo"),
    [
      ["moveTo", 0, 0],
      ["moveTo", 20, 20],
    ],
  );
});

test("a pointer is read in board coordinates whatever the canvas is laid out at", () => {
  // Half scale: the element is 800 CSS pixels wide and the board is 1600.
  const canvas = {
    getBoundingClientRect: () => ({
      left: 100,
      top: 50,
      width: 800,
      height: 500,
    }),
  };
  assert.deepEqual(boardPoint(canvas, { clientX: 100, clientY: 50 }), {
    x: 0,
    y: 0,
  });
  assert.deepEqual(boardPoint(canvas, { clientX: 500, clientY: 300 }), {
    x: 800,
    y: 500,
  });
  assert.deepEqual(boardPoint(canvas, { clientX: 900, clientY: 550 }), {
    x: BOARD_WIDTH,
    y: BOARD_HEIGHT,
  });

  // A canvas that has not been laid out yet has no scale to read, and
  // dividing by its zero width would put every stroke at NaN.
  const unlaid = {
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 0, height: 0 }),
  };
  assert.deepEqual(boardPoint(unlaid, { clientX: 10, clientY: 10 }), {
    x: 0,
    y: 0,
  });
});

test("the journal is the drawing, and rebuilding from it gives the same board", () => {
  const drawn = createBoard();
  stroke(drawn, 0, 0, 10, 10);
  stroke(drawn, 20, 20, 30, 30, "eraser");
  drawn.undo();
  stroke(drawn, 40, 40, 50, 50);
  drawn.clear();
  drawn.undo();
  drawn.redo();
  stroke(drawn, 60, 60, 70, 70);

  const ops = drawn.takeOps();
  assert.deepEqual(
    ops.map((op) => op.op),
    ["stroke", "stroke", "undo", "stroke", "clear", "undo", "redo", "stroke"],
  );
  assert.deepEqual(drawn.takeOps(), [], "taken once, not once per reader");

  const rebuilt = createBoard();
  for (const op of ops)
    assert.equal(applyOp(rebuilt, op), true, `${op.op} was refused`);
  assert.deepEqual(rebuilt.strokes(), drawn.strokes());
});

test("a stroke from a recording is checked before it is drawn", () => {
  // This arrives from a server that stored what a browser sent and hands it
  // back verbatim, so it is exactly as trustworthy as the report payload is.
  const board = createBoard();
  const points = [0, 0, 10, 10];
  for (const [color, width, bad] of [
    ["red", 3, points],
    ["#10141", 3, points],
    ["#101418", 0, points],
    ["#101418", 3, [0, 0, 10]],
    ["#101418", 3, [0, 0, "10", 10]],
    ["#101418", 3, []],
    ["#101418", 3, "0,0,10,10"],
    ["#101418", Number.POSITIVE_INFINITY, points],
    ["#101418", MAX_STROKE_WIDTH + 1, points],
    ["#101418", 3, new Array(MAX_POINTS * 2 + 2).fill(1)],
  ]) {
    assert.equal(
      board.push(color, width, bad),
      false,
      `${color} ${width} ${JSON.stringify(bad)}`,
    );
  }
  assert.equal(board.strokeCount(), 0);

  // And a well-formed one is taken, clamped onto the board.
  assert.equal(
    board.push("#101418", 3, [-5, -5, BOARD_WIDTH + 5, BOARD_HEIGHT + 5]),
    true,
  );
  assert.deepEqual(board.strokes()[0].points, [
    0,
    0,
    BOARD_WIDTH,
    BOARD_HEIGHT,
  ]);

  // An operation from a later deploy is ignored rather than refused, the same
  // way an unknown replay kind is.
  assert.equal(applyOp(board, { op: "highlight" }), false);
  assert.equal(applyOp(board, null), false);
  assert.equal(board.strokeCount(), 1);
});

const mouse = (pointerId, button = 0) => ({
  pointerId,
  pointerType: "mouse",
  button,
});
const pen = (pointerId) => ({ pointerId, pointerType: "pen", button: 0 });
const touch = (pointerId) => ({ pointerId, pointerType: "touch", button: 0 });

test("a mouse, a pen and a finger can each draw", () => {
  for (const pointer of [mouse(1), pen(2), touch(3)]) {
    const gate = createPointerGate();
    assert.equal(gate.held(), false);
    assert.equal(gate.down(pointer), "begin");
    assert.equal(gate.held(), true);
    assert.ok(gate.owns(pointer));
    assert.equal(gate.up(pointer), true);
    assert.equal(gate.held(), false);
    assert.equal(gate.owns(pointer), false);
  }
  // Only the primary button draws: a right click or a pen's barrel button is
  // not a stroke.
  assert.equal(createPointerGate().down(mouse(1, 2)), null);
});

test("one pointer draws at a time, and a second finger is not part of its stroke", () => {
  const gate = createPointerGate();
  assert.equal(gate.down(touch(1)), "begin");
  assert.equal(gate.held(), true);
  assert.equal(gate.down(touch(2)), null);
  assert.equal(gate.owns(touch(2)), false);
  assert.equal(gate.up(touch(2)), false, "the second finger ends nothing");
  assert.ok(gate.owns(touch(1)));
  assert.equal(gate.held(), true, "the first finger still holds the board");
});

test("a palm is kept off the board while a pen is down", () => {
  const gate = createPointerGate();
  assert.equal(gate.down(pen(1)), "begin");
  assert.equal(gate.down(touch(2)), null);
  // Lifting the pen between strokes leaves the resting palm refused, since it
  // went down while the pen was there; the pen's next stroke is unaffected.
  gate.up(pen(1));
  assert.equal(gate.owns(touch(2)), false);
  assert.equal(gate.down(pen(1)), "begin");
  gate.up(pen(1));
  gate.up(touch(2));
  // With no pen down a finger draws again.
  assert.equal(gate.down(touch(3)), "begin");
});

test("a pen takes the board from a palm that landed first", () => {
  const gate = createPointerGate();
  assert.equal(gate.down(touch(1)), "begin");
  assert.equal(gate.down(pen(2)), "replace");
  assert.ok(gate.owns(pen(2)));
  assert.equal(gate.owns(touch(1)), false);
  assert.equal(gate.up(touch(1)), false, "the palm lifting ends nothing");
  // A pen does not take a stroke from a mouse, which no palm produces.
  const desk = createPointerGate();
  desk.down(mouse(1));
  assert.equal(desk.down(pen(2)), null);
});

test("text creation shares palm rejection and releases the gate after cancellation", async (t) => {
  const { launchChromium, startStaticServer } = await import("./source.js");
  const { exerciseTextPalmRejection } = await import("./board-text-fixture.js");
  const browser = await launchChromium();
  if (!browser) return t.skip("playwright chromium unavailable");
  t.after(() => browser.close());
  const { server, base } = await startStaticServer();
  t.after(() => new Promise((resolve) => server.close(resolve)));
  const page = await browser.newPage();
  await page.goto(base);
  await page.evaluate(async () => {
    const { mountBoardText } = await import("/board-text.js");
    const { createPointerGate } = await import("/whiteboard.js");
    document.body.innerHTML =
      '<div style="position:relative"><canvas width="1600" height="1000" style="width:800px;height:500px"></canvas></div>';
    const style = document.createElement("link");
    style.rel = "stylesheet";
    style.href = "/styles.css";
    document.head.append(style);
    mountBoardText(document.querySelector("canvas"), {
      active: () => true,
      locked: () => false,
      pointers: createPointerGate(),
      commit: () => {
        throw new Error("pointer gestures must not commit text");
      },
    });
  });
  await exerciseTextPalmRejection(page, page.locator("canvas"));
});

test("a cancelled stroke leaves the board, the journal and redo as they were", () => {
  const board = createBoard();
  stroke(board, 0, 0, 10, 10);
  stroke(board, 20, 20, 30, 30);
  board.undo();
  board.takeOps();
  const before = board.strokes();

  board.begin("pen", BLACK, 50, 50);
  board.extend(60, 60);
  assert.equal(board.cancel(), true);
  assert.deepEqual(board.strokes(), before);
  assert.deepEqual(board.takeOps(), []);
  assert.equal(board.isDrawing(), false);
  assert.ok(board.redo(), "the palm did not cost the redo it would have");
  assert.equal(board.cancel(), false, "nothing open, nothing to cancel");
});
