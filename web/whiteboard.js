/// The whiteboard: the model a drawing is, and the canvas it is painted on.
///
/// Strokes are kept as vectors rather than as pixels, which is what makes undo
/// and redo possible at all: a bitmap board can only be undone by keeping a
/// copy of the bitmap per step, and a board that is redrawn from its strokes
/// costs one array. It is also what lets the exported image be regenerated at
/// any size later without the board on screen deciding the resolution.
///
/// The model is free of the DOM so `tests/browser/whiteboard.test.js` can
/// drive it: everything that touches a canvas takes the context as an
/// argument.

/// The board's own coordinate space, and the size the image is exported at.
///
/// Fixed rather than fitted to the window, and that is the point: the
/// interviewer is sent an image of the whole board, so a board that grew with
/// the candidate's viewport would mean two candidates on different screens
/// drawing on different surfaces, and a scroll position deciding what the
/// interviewer can see. It scrolls inside its panel instead.
export const BOARD_WIDTH = 1600;
export const BOARD_HEIGHT = 1000;

/// What a board is painted on. A whiteboard is opaque, and it has to be: JPEG
/// has no alpha, so a transparent board exports as black.
export const BOARD_BACKGROUND = "#ffffff";

/// Strokes one board keeps, and points one stroke keeps.
///
/// Both are ceilings on memory rather than limits anybody should reach: a
/// dense hand-drawn diagram runs to a few hundred strokes, and one stroke of a
/// long line at pointer resolution to a few hundred points. Past them the
/// oldest work would have to start disappearing under the candidate, so the
/// board stops accepting instead, which is at least visible.
export const MAX_STROKES = 600;
/// Points in one stroke.
///
/// Sized by the replay rather than by the drawing: a stroke is journalled and
/// stored as its points, and one event may not exceed 64 KiB, so this is the
/// ceiling that keeps the longest single stroke a person can draw comfortably
/// inside that. A continuous scribble at pointer rate reaches a few hundred.
export const MAX_POINTS = 1200;

/// The pens the toolbar offers. Black first: it is what a real board is
/// written in, and the rest are for marking up what is already there.
export const PEN_COLORS = ["#101418", "#c2261b", "#1b64c2", "#1f8a4c"];

export const PEN_WIDTH = 3;
/// The eraser is wide enough to be usable with a mouse and narrow enough to
/// take out one line of writing without the one under it.
export const ERASER_WIDTH = 24;

/// The eraser paints the background colour instead of removing strokes.
///
/// A real eraser would have to decide which strokes it touched and split them,
/// which is a geometry problem the interview does not need solved: this way an
/// erasure is itself a stroke, so it undoes, redoes and exports like any
/// other, and on an opaque board it is indistinguishable from the real thing.
/// The alternative, compositing with `destination-out`, punches holes that are
/// transparent and so come out black in the JPEG.
export function strokeColor(tool, color) {
  return tool === "eraser" ? BOARD_BACKGROUND : color;
}

export function strokeWidth(tool) {
  return tool === "eraser" ? ERASER_WIDTH : PEN_WIDTH;
}

/// A coordinate on the board.
///
/// Clamped because a pointer that leaves the canvas mid-drag still reports
/// coordinates, and a stroke that runs to -400 draws nothing while still
/// counting against the ceilings above; rounded because a replay stores every
/// point and a board drawn in whole pixels is half the bytes of one drawn in
/// fractions of them.
function clamp(value, max) {
  return Math.min(Math.max(Math.round(value), 0), max);
}

/// The drawing, and the history over it.
///
/// `strokes` is what the board is; `undone` is what undo has taken off it, in
/// the order redo puts it back. Any new stroke discards the redo stack, which
/// is what every editor does and what stops a redo from resurrecting work that
/// was drawn over.
export function createBoard() {
  let strokes = [];
  let undone = [];
  let open = null;
  /// What has been done to the board since somebody last asked.
  ///
  /// The replay is the drawing as operations rather than as pictures, and this
  /// is where those operations come from: the board is the only thing that
  /// knows an edit happened, so a producer that recorded them beside it would
  /// be a second account of the same board, free to disagree with it.
  let journal = [];

  return {
    /// Everything drawn so far, oldest first, including the stroke in
    /// progress. A copy: a caller that mutated this would move the board
    /// without the history noticing.
    strokes: () => strokes.slice(),
    strokeCount: () => strokes.length,
    canUndo: () => strokes.length > 0,
    canRedo: () => undone.length > 0,
    isDrawing: () => open !== null,

    /// Starts a stroke, and returns whether the board took it.
    begin(tool, color, x, y) {
      if (strokes.length >= MAX_STROKES) return false;
      undone = [];
      open = {
        color: strokeColor(tool, color),
        width: strokeWidth(tool),
        points: [clamp(x, BOARD_WIDTH), clamp(y, BOARD_HEIGHT)],
      };
      strokes.push(open);
      return true;
    },

    /// Extends the open stroke. Points repeated exactly are dropped: a mouse
    /// held still emits them by the dozen, and they cost memory and the stroke
    /// count without changing a pixel.
    extend(x, y) {
      if (!open) return false;
      const px = clamp(x, BOARD_WIDTH);
      const py = clamp(y, BOARD_HEIGHT);
      const points = open.points;
      if (points.length >= MAX_POINTS * 2) return false;
      if (points[points.length - 2] === px && points[points.length - 1] === py) return false;
      points.push(px, py);
      return true;
    },

    /// Ends the open stroke, and returns whether anything was drawn. A click
    /// that never moved is kept: a dot is a mark somebody made on purpose, and
    /// the renderer draws it as one.
    end() {
      if (!open) return false;
      journal.push({ op: "stroke", color: open.color, width: open.width, points: open.points.slice() });
      open = null;
      return true;
    },

    /// A stroke that was drawn somewhere else: the replay and the recording
    /// rebuild a board from the journal of the interview that produced it.
    ///
    /// Everything about it is checked, because it arrives from a server that
    /// stored what a browser sent and hands it back verbatim. A stroke with a
    /// colour that is not a colour paints whatever the canvas makes of it, and
    /// one with an odd number of coordinates draws a line to `undefined`.
    push(color, width, points) {
      if (strokes.length >= MAX_STROKES) return false;
      if (typeof color !== "string" || !/^#[0-9a-f]{6}$/i.test(color)) return false;
      if (!Number.isFinite(width) || width <= 0 || width > ERASER_WIDTH) return false;
      if (!Array.isArray(points) || points.length < 2 || points.length % 2 !== 0) return false;
      if (points.length > MAX_POINTS * 2) return false;
      if (!points.every((value) => Number.isFinite(value))) return false;
      undone = [];
      strokes.push({
        color,
        width,
        points: points.map((value, index) => clamp(value, index % 2 === 0 ? BOARD_WIDTH : BOARD_HEIGHT)),
      });
      return true;
    },

    undo() {
      if (open || strokes.length === 0) return false;
      undone.push(strokes.pop());
      journal.push({ op: "undo" });
      return true;
    },

    redo() {
      if (open || undone.length === 0) return false;
      strokes.push(undone.pop());
      journal.push({ op: "redo" });
      return true;
    },

    /// Clears the board, keeping what was cleared on the redo stack in one
    /// piece: an accidental clear is the most expensive mistake available at a
    /// whiteboard, and without this it is unrecoverable.
    clear() {
      if (open || strokes.length === 0) return false;
      undone = strokes.slice().reverse();
      strokes = [];
      journal.push({ op: "clear" });
      return true;
    },

    /// The operations since the last call, and the journal starts again.
    ///
    /// Taken rather than read, because the caller is the replay producer and
    /// what it does with them is send them once. A reader that left them here
    /// would send the whole interview's drawing with every batch.
    takeOps() {
      const taken = journal;
      journal = [];
      return taken;
    },
  };
}

/// One journalled operation, applied to a board being rebuilt.
///
/// The return value is whether the board changed, which a replay uses to
/// decide whether a stored operation was one this build understands: an
/// unknown op is ignored rather than refused, the same way an unknown replay
/// kind is, because a recording made by a later deploy is still worth watching.
export function applyOp(board, op) {
  switch (op?.op) {
    case "stroke":
      return board.push(op.color, op.width, op.points);
    case "undo":
      return board.undo();
    case "redo":
      return board.redo();
    case "clear":
      return board.clear();
    default:
      return false;
  }
}

/// Paints a whole board, background included.
///
/// Every change redraws everything. At the stroke ceiling above that is a few
/// hundred short paths, which a browser does in a frame; the alternative,
/// painting only what changed, has to answer what "changed" means after an
/// undo, and answers it with a second copy of the board.
export function drawBoard(context, strokes, width = BOARD_WIDTH, height = BOARD_HEIGHT) {
  context.save();
  context.fillStyle = BOARD_BACKGROUND;
  context.fillRect(0, 0, width, height);
  context.lineCap = "round";
  context.lineJoin = "round";
  for (const stroke of strokes) drawStroke(context, stroke);
  context.restore();
}

/// One stroke, including the single-point case.
///
/// A dot is drawn as a filled circle rather than as a zero-length line,
/// because a path with one point paints nothing at all in a canvas: the click
/// that made it would simply vanish, on the board and in the image the
/// interviewer is sent.
export function drawStroke(context, stroke) {
  const points = stroke.points;
  context.strokeStyle = stroke.color;
  context.fillStyle = stroke.color;
  context.lineWidth = stroke.width;
  if (points.length === 2) {
    context.beginPath();
    context.arc(points[0], points[1], stroke.width / 2, 0, Math.PI * 2);
    context.fill();
    return;
  }
  context.beginPath();
  context.moveTo(points[0], points[1]);
  for (let index = 2; index < points.length; index += 2) {
    context.lineTo(points[index], points[index + 1]);
  }
  context.stroke();
}

/// Where a pointer event landed in board coordinates.
///
/// The canvas is laid out at whatever width the panel gives it and drawn at
/// the board's own size, so the two differ by a scale the browser chooses and
/// the candidate can change by resizing the window. Reading it off the element
/// each time is what keeps the ink under the pointer.
export function boardPoint(canvas, event) {
  const bounds = canvas.getBoundingClientRect();
  if (!bounds.width || !bounds.height) return { x: 0, y: 0 };
  return {
    x: ((event.clientX - bounds.left) / bounds.width) * BOARD_WIDTH,
    y: ((event.clientY - bounds.top) / bounds.height) * BOARD_HEIGHT,
  };
}
