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
  MAX_POINTS,
  MAX_STROKES,
  PEN_COLORS,
  PEN_WIDTH,
  boardPoint,
  createBoard,
  drawBoard,
  strokeColor,
  strokeWidth,
} from "../../web/whiteboard.js";

const BLACK = PEN_COLORS[0];

/// Draws one stroke from (x1,y1) to (x2,y2), the way a press-drag-release does.
function stroke(board, x1, y1, x2, y2, tool = "pen") {
  board.begin(tool, BLACK, x1, y1);
  board.extend(x2, y2);
  return board.end();
}

/// A 2D context that records the calls instead of painting them.
function recordingContext() {
  const calls = [];
  const record = (name) => (...args) => calls.push([name, ...args]);
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
  };
}

test("a stroke is kept as the points it was drawn from", () => {
  const board = createBoard();
  assert.equal(stroke(board, 10, 20, 30, 40), true);
  assert.deepEqual(board.strokes(), [
    { color: BLACK, width: PEN_WIDTH, points: [10, 20, 30, 40] },
  ]);
  assert.equal(board.strokeCount(), 1);
  assert.equal(board.isDrawing(), false);
});

test("the strokes a caller is handed are a copy of the board", () => {
  const board = createBoard();
  stroke(board, 1, 1, 2, 2);
  board.strokes().push({ color: BLACK, width: PEN_WIDTH, points: [0, 0] });
  assert.equal(board.strokeCount(), 1);
});

test("points outside the board are clamped onto it", () => {
  const board = createBoard();
  board.begin("pen", BLACK, -40, -10);
  board.extend(BOARD_WIDTH + 500, BOARD_HEIGHT + 500);
  board.end();
  assert.deepEqual(board.strokes()[0].points, [0, 0, BOARD_WIDTH, BOARD_HEIGHT]);
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
  assert.equal(strokeColor("eraser", BLACK), BOARD_BACKGROUND);
  assert.equal(strokeColor("pen", BLACK), BLACK);
  assert.equal(strokeWidth("eraser"), ERASER_WIDTH);
  assert.equal(strokeWidth("pen"), PEN_WIDTH);

  const board = createBoard();
  stroke(board, 0, 0, 10, 10);
  stroke(board, 0, 0, 10, 10, "eraser");
  assert.deepEqual(
    board.strokes().map((item) => [item.color, item.width]),
    [[BLACK, PEN_WIDTH], [BOARD_BACKGROUND, ERASER_WIDTH]],
  );

  // The erasure undoes, which is the property the composite-operation
  // alternative does not have: it would have removed pixels there is no
  // record of.
  board.undo();
  assert.deepEqual(board.strokes().map((item) => item.color), [BLACK]);
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

  // Three redos rather than one: the board comes back in the order it was
  // drawn, which is what the renderer needs to paint the later strokes over
  // the earlier ones.
  for (const expected of [[0, 0, 1, 1], [2, 2, 3, 3], [4, 4, 5, 5]]) {
    assert.equal(board.redo(), true);
    assert.deepEqual(board.strokes().at(-1).points, expected);
  }
  assert.equal(createBoard().clear(), false, "an empty board has nothing to clear");
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
    assert.equal(long.extend(index % BOARD_WIDTH, index), true, `point ${index}`);
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
  assert.deepEqual(context.calls[1], ["fillRect", 0, 0, BOARD_WIDTH, BOARD_HEIGHT]);
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

test("a pointer is read in board coordinates whatever the canvas is laid out at", () => {
  // Half scale: the element is 800 CSS pixels wide and the board is 1600.
  const canvas = {
    getBoundingClientRect: () => ({ left: 100, top: 50, width: 800, height: 500 }),
  };
  assert.deepEqual(boardPoint(canvas, { clientX: 100, clientY: 50 }), { x: 0, y: 0 });
  assert.deepEqual(boardPoint(canvas, { clientX: 500, clientY: 300 }), { x: 800, y: 500 });
  assert.deepEqual(boardPoint(canvas, { clientX: 900, clientY: 550 }), {
    x: BOARD_WIDTH,
    y: BOARD_HEIGHT,
  });

  // A canvas that has not been laid out yet has no scale to read, and
  // dividing by its zero width would put every stroke at NaN.
  const unlaid = { getBoundingClientRect: () => ({ left: 0, top: 0, width: 0, height: 0 }) };
  assert.deepEqual(boardPoint(unlaid, { clientX: 10, clientY: 10 }), { x: 0, y: 0 });
});

test("the journal is the drawing, and rebuilding from it gives the same board", () => {
  const drawn = createBoard();
  stroke(drawn, 0, 0, 10, 10);
  stroke(drawn, 20, 20, 30, 30, "eraser");
  drawn.undo();
  stroke(drawn, 40, 40, 50, 50);
  drawn.clear();
  // Before the next stroke, not after: drawing discards the redo stack, so a
  // redo there is a no-op and journals nothing.
  drawn.redo();
  stroke(drawn, 60, 60, 70, 70);

  const ops = drawn.takeOps();
  assert.deepEqual(
    ops.map((op) => op.op),
    ["stroke", "stroke", "undo", "stroke", "clear", "redo", "stroke"],
  );
  assert.deepEqual(drawn.takeOps(), [], "taken once, not once per reader");

  const rebuilt = createBoard();
  for (const op of ops) assert.equal(applyOp(rebuilt, op), true, `${op.op} was refused`);
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
    ["#101418", ERASER_WIDTH + 1, points],
    ["#101418", 3, new Array(MAX_POINTS * 2 + 2).fill(1)],
  ]) {
    assert.equal(board.push(color, width, bad), false, `${color} ${width} ${JSON.stringify(bad)}`);
  }
  assert.equal(board.strokeCount(), 0);

  // And a well-formed one is taken, clamped onto the board.
  assert.equal(board.push("#101418", 3, [-5, -5, BOARD_WIDTH + 5, BOARD_HEIGHT + 5]), true);
  assert.deepEqual(board.strokes()[0].points, [0, 0, BOARD_WIDTH, BOARD_HEIGHT]);

  // An operation from a later deploy is ignored rather than refused, the same
  // way an unknown replay kind is.
  assert.equal(applyOp(board, { op: "highlight" }), false);
  assert.equal(applyOp(board, null), false);
  assert.equal(board.strokeCount(), 1);
});
