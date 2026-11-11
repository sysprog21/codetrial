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
/// What a drawing surface says when it refuses a stroke at `MAX_STROKES`.
/// One sentence for both boards, so rewording one cannot leave the other
/// saying something else.
export const MAX_STROKES_MESSAGE =
  "Drawing limit reached. Undo or clear before adding more.";
/// Use four pages as a provisional planning ceiling for a 128 MiB per-session
/// whiteboard budget. Assuming 8-byte numeric slots, the raw coordinate
/// payload is 4 * 600 * 1200 * 2 coordinates * 2 copies * 8 bytes =
/// 92,160,000 bytes (about 87.9 MiB) for the board strokes and undrained
/// journal copy. A heap benchmark of that state measured about 106 MiB,
/// including array and object overhead. The budget therefore depends on
/// draining each page's journal and is not a hard allocator limit.
export const MAX_PAGES = 4;
export const TEXT_SIZE = 32;
export const TEXT_MIN_SIZE = 12;
export const TEXT_MAX_SIZE = 128;
export const TEXT_LINE_HEIGHT = 1.25;
export const TEXT_MAX_LENGTH = 500;
export const TEXT_MAX_LINES = 20;
/// Points in one stroke.
///
/// Sized by the replay rather than by the drawing: a stroke is journalled and
/// stored as its points, and one event may not exceed 64 KiB, so this is the
/// ceiling that keeps the longest single stroke a person can draw comfortably
/// inside that. A continuous scribble at pointer rate reaches a few hundred.
export const MAX_POINTS = 1200;

/// The pens the toolbar offers. Black first: it is what a real board is
/// written in, and the rest are for marking up what is already there.
///
/// Each carries the name its swatch is announced by. The swatch is a dot of
/// color with no text, so without one a screen reader reads out the hex value
/// and Voice Control asks the candidate to say it.
export const PEN_COLORS = [
  { color: "#101418", name: "Black" },
  { color: "#c2261b", name: "Red" },
  { color: "#1b64c2", name: "Blue" },
  { color: "#1f8a4c", name: "Green" },
];

export const PEN_WIDTH = 3;
/// The eraser sizes the toolbar offers, in board pixels.
///
/// A short list rather than a slider: the small one takes out a character
/// inside a word, the middle one a line of writing without the one under it,
/// and the large one a whole region without a dozen passes. A continuous range
/// would put a width on the replay that no button can reproduce.
export const ERASER_WIDTHS = [12, 24, 56];
/// The size the eraser starts at.
export const ERASER_WIDTH = 24;
/// The widest stroke a board accepts from somewhere else.
export const MAX_STROKE_WIDTH = Math.max(PEN_WIDTH, ...ERASER_WIDTHS);

/// The eraser paints the background colour instead of removing strokes.
///
/// A real eraser would have to decide which strokes it touched and split them,
/// which is a geometry problem the interview does not need solved: this way an
/// erasure is itself a stroke, so it undoes, redoes and exports like any
/// other, and on an opaque board it is indistinguishable from the real thing.
/// The alternative, compositing with `destination-out`, punches holes that are
/// transparent and so come out black in the JPEG.
function strokeColor(tool, color) {
  return tool === "eraser" ? BOARD_BACKGROUND : color;
}

/// A size that is not one of the offered ones falls back to the default rather
/// than being drawn, so the board only ever holds widths a button can make.
function strokeWidth(tool, eraserWidth = ERASER_WIDTH) {
  if (tool !== "eraser") return PEN_WIDTH;
  return ERASER_WIDTHS.includes(eraserWidth) ? eraserWidth : ERASER_WIDTH;
}

/// Which pointer holds an in-progress board gesture, with palm rejection for
/// a pen.
///
/// Mouse, pen and touch all reach the board as pointer events, so the board
/// takes all three, one pointer at a time: a second finger never extends or
/// ends the first finger's stroke. A hand resting on a tablet reports as
/// touch, so a touch is refused while a pen is down. A palm that landed first
/// has already begun a stroke, and a pen arriving takes the board from it:
/// `down` answers `"replace"`, and the caller cancels that stroke before
/// beginning the pen's.
export function createPointerGate() {
  let owner = null;
  const pens = new Set();
  return {
    held: () => owner !== null,
    /// `"begin"`, `"replace"`, or `null` for a pointer that must not draw.
    down({ pointerId, pointerType, button }) {
      if (pointerType === "pen") pens.add(pointerId);
      if (button !== 0) return null;
      if (owner) {
        if (pointerType !== "pen" || owner.type !== "touch") return null;
        owner = { id: pointerId, type: pointerType };
        return "replace";
      }
      if (pointerType === "touch" && pens.size > 0) return null;
      owner = { id: pointerId, type: pointerType };
      return "begin";
    },
    owns: ({ pointerId }) => owner?.id === pointerId,
    /// Forgets the pointer, and returns whether it was the one drawing.
    up({ pointerId }) {
      pens.delete(pointerId);
      if (owner?.id !== pointerId) return false;
      owner = null;
      return true;
    },
  };
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

/// What an undone clear leaves on the redo stack, in the place a stroke would.
const CLEAR = Symbol("clear");

/// A finished stroke, frozen so the board's own copy cannot be changed through
/// the one `strokes()` hands out.
function finished(stroke) {
  Object.freeze(stroke.points);
  return Object.freeze(stroke);
}

/// The drawing, and the history over it.
///
/// `strokes` is what the board is; `undone` is what undo has taken off it, in
/// the order redo puts it back: a stroke, or `CLEAR` for a clear. Any new
/// stroke discards the redo stack, which is what every editor does and what
/// stops a redo from resurrecting work that was drawn over.
export function createBoard() {
  let strokes = [];
  let undone = [];
  // Clear is one edit, even though it removes every stroke. Only the newest
  // clear needs its own snapshot: drawing after it deliberately starts a new
  // undo branch, just as drawing after an ordinary undo does. Undoing it puts
  // `CLEAR` on the redo stack rather than setting a flag beside it, so undoing
  // strokes from under it and redoing them back leaves the clear still there
  // to redo.
  let cleared = null;
  let open = null;
  /// The redo history `begin` discarded, kept until the stroke ends so that a
  /// stroke cancelled before it ended discards nothing.
  let beforeOpen = null;
  /// What has been done to the board since somebody last asked.
  ///
  /// The replay is the drawing as operations rather than as pictures, and this
  /// is where those operations come from: the board is the only thing that
  /// knows an edit happened, so a producer that recorded them beside it would
  /// be a second account of the same board, free to disagree with it.
  let journal = [];

  return {
    /// Everything drawn so far, oldest first, including the stroke in
    /// progress. A copy of the list, and every finished stroke in it frozen,
    /// rather than a copy of every stroke: this is called on every repaint,
    /// and copying each point of a full board per frame is what freezing them
    /// once avoids. Either way a caller cannot move the board without the
    /// history noticing.
    strokes: () => strokes.slice(),
    strokeCount: () => strokes.length,
    /// Strokes that left ink, which is what the interviewer is told the board
    /// carries. An erasure is a stroke in the background colour, so three
    /// swipes of the eraser over an empty board would otherwise count as a
    /// drawing.
    inkCount: () =>
      strokes.filter((stroke) => stroke.color !== BOARD_BACKGROUND).length,
    canUndo: () => strokes.length > 0 || cleared !== null,
    canRedo: () => undone.length > 0,
    isDrawing: () => open !== null,

    // Drafts live outside this model. Only confirmation discards redo history.
    addText({ text, color, x, y, fontSize }, record = true) {
      if (open || strokes.length >= MAX_STROKES) return false;
      if (
        typeof text !== "string" ||
        !text.trim() ||
        text.length > TEXT_MAX_LENGTH
      )
        return false;
      text = text.replace(/\r\n?/g, "\n");
      if (text.split("\n").length > TEXT_MAX_LINES) return false;
      if (typeof color !== "string" || !/^#[0-9a-f]{6}$/i.test(color))
        return false;
      if (
        ![x, y, fontSize].every(Number.isFinite) ||
        fontSize < TEXT_MIN_SIZE ||
        fontSize > TEXT_MAX_SIZE
      )
        return false;
      if (
        x < 0 ||
        x > BOARD_WIDTH ||
        y < 0 ||
        y + text.split("\n").length * fontSize * TEXT_LINE_HEIGHT > BOARD_HEIGHT
      )
        return false;
      const item = Object.freeze({ kind: "text", text, color, x, y, fontSize });
      strokes.push(item);
      undone = [];
      cleared = null;
      if (record) journal.push({ op: "text", text, color, x, y, fontSize });
      return true;
    },

    /// Starts a stroke, and returns whether the board took it.
    begin(tool, color, x, y, eraserWidth = ERASER_WIDTH) {
      if (strokes.length >= MAX_STROKES) return false;
      beforeOpen = { undone, cleared };
      undone = [];
      cleared = null;
      open = {
        color: strokeColor(tool, color),
        width: strokeWidth(tool, eraserWidth),
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
      if (points[points.length - 2] === px && points[points.length - 1] === py)
        return false;
      points.push(px, py);
      return true;
    },

    /// Ends the open stroke, and returns whether anything was drawn. A click
    /// that never moved is kept: a dot is a mark somebody made on purpose, and
    /// the renderer draws it as one.
    end() {
      if (!open) return false;
      journal.push({
        op: "stroke",
        color: open.color,
        width: open.width,
        points: open.points.slice(),
      });
      finished(open);
      open = null;
      beforeOpen = null;
      return true;
    },

    /// Takes back the open stroke as though it had never begun: nothing is
    /// journaled, and the redo history its start dropped is restored. This is
    /// a palm that reached the board before the pen did, which is no edit the
    /// candidate made and should not cost them a redo.
    cancel() {
      if (!open) return false;
      strokes.splice(strokes.indexOf(open), 1);
      ({ undone, cleared } = beforeOpen);
      open = null;
      beforeOpen = null;
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
      if (typeof color !== "string" || !/^#[0-9a-f]{6}$/i.test(color))
        return false;
      if (!Number.isFinite(width) || width <= 0 || width > MAX_STROKE_WIDTH)
        return false;
      if (
        !Array.isArray(points) ||
        points.length < 2 ||
        points.length % 2 !== 0
      )
        return false;
      if (points.length > MAX_POINTS * 2) return false;
      if (!points.every((value) => Number.isFinite(value))) return false;
      undone = [];
      cleared = null;
      strokes.push(
        finished({
          color,
          width,
          points: points.map((value, index) =>
            clamp(value, index % 2 === 0 ? BOARD_WIDTH : BOARD_HEIGHT),
          ),
        }),
      );
      return true;
    },

    undo() {
      if (open) return false;
      if (cleared !== null) {
        strokes = cleared;
        cleared = null;
        undone.push(CLEAR);
        journal.push({ op: "undo" });
        return true;
      }
      if (strokes.length === 0) return false;
      undone.push(strokes.pop());
      journal.push({ op: "undo" });
      return true;
    },

    redo() {
      if (open || undone.length === 0) return false;
      const next = undone.pop();
      if (next === CLEAR) {
        cleared = strokes;
        strokes = [];
      } else {
        cleared = null;
        strokes.push(next);
      }
      journal.push({ op: "redo" });
      return true;
    },

    /// Clears the board, keeping what was cleared on the redo stack in one
    /// piece: an accidental clear is the most expensive mistake available at a
    /// whiteboard, and without this it is unrecoverable.
    clear() {
      if (open || strokes.length === 0) return false;
      cleared = strokes;
      strokes = [];
      undone = [];
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

/// The whiteboard pages, each with its own drawing and edit history.
///
/// Page selection belongs to the interview UI. This model only owns the
/// collection so navigation can switch between complete `createBoard`
/// instances without sharing Undo, Redo or Clear state.
/// Each page journal currently has no page identity; the follow-up navigation
/// and recording step must drain every page and tag each operation before
/// replay, rather than draining only the visible page.
export function createBoardPages() {
  const pages = [createBoard()];

  return {
    count: () => pages.length,
    get(index) {
      return Number.isInteger(index) ? pages[index] : undefined;
    },
    /// The index of the new page, or -1 once MAX_PAGES are open.
    add() {
      if (pages.length >= MAX_PAGES) return -1;
      pages.push(createBoard());
      return pages.length - 1;
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
    case "text":
      return board.addText(op, false);
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
/// For a board painted once, as the replay paints one, and for the layer
/// below whenever it has to start over. A board being drawn on goes through
/// `createBoardPainter` instead: this costs every point on the board, and a
/// pointer move repaints the board.
export function drawBoard(
  context,
  strokes,
  width = BOARD_WIDTH,
  height = BOARD_HEIGHT,
) {
  context.save();
  context.fillStyle = BOARD_BACKGROUND;
  context.fillRect(0, 0, width, height);
  context.lineCap = "round";
  context.lineJoin = "round";
  for (const stroke of strokes) drawStroke(context, stroke);
  context.restore();
}

/// Paints the board a candidate is drawing on, at the cost of what changed.
///
/// Repainting every stroke on every pointer move made a move cost the whole
/// drawing: in Chromium a board at the stroke ceiling took 34ms a move, two
/// frames, and 150ms with the CPU slowed fourfold, so the ink trailed the pen
/// on exactly the boards where the candidate had written the most. The
/// finished strokes are kept on a layer instead, and a move copies the layer
/// and draws only the stroke under the pointer.
///
/// The layer is checked against the board on every paint rather than told
/// about edits. It remembers which strokes it holds, by identity, and is
/// reused only while those are still the board's first strokes in order: a
/// stroke that ends or a redo extends them and is drawn onto the layer alone,
/// while an undo or a clear breaks them and the layer is repainted from the
/// board. Identity rather than a count, because an undo followed by a new
/// stroke before the next paint leaves the board as long as it was with a
/// different stroke at the end, and so does a different board handed to the
/// same painter. No edit path can leave the layer stale by forgetting to say
/// what it did, which is what keeps it a cache of the board rather than a
/// second copy of it.
///
/// `createLayer(width, height)` returns a canvas the size of the board; it is
/// a parameter so this module stays free of the DOM.
export function createBoardPainter(createLayer) {
  let layer = null;
  let layerContext = null;
  /// The strokes on the layer, oldest first; null until it is first painted.
  let painted = null;

  return {
    /// `drawing` is whether the last stroke is still under the pointer. It is
    /// painted over the layer and kept off it until it ends, since every move
    /// changes it.
    paint(context, strokes, drawing) {
      const finished = Math.max(
        0,
        drawing ? strokes.length - 1 : strokes.length,
      );
      if (!layer) {
        layer = createLayer(BOARD_WIDTH, BOARD_HEIGHT);
        layerContext = layer.getContext("2d");
      }
      const reusable =
        painted !== null &&
        painted.length <= finished &&
        painted.every((stroke, index) => stroke === strokes[index]);
      if (reusable) {
        drawStrokes(layerContext, strokes.slice(painted.length, finished));
      } else {
        drawBoard(layerContext, strokes.slice(0, finished));
      }
      painted = strokes.slice(0, finished);
      context.drawImage(layer, 0, 0);
      if (finished < strokes.length)
        drawStrokes(context, strokes.slice(finished));
    },
  };
}

/// Strokes over whatever is already painted, in the caps and joins
/// `drawBoard` uses, so a stroke looks the same however it reached the canvas.
function drawStrokes(context, strokes) {
  if (strokes.length === 0) return;
  context.save();
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
function drawStroke(context, stroke) {
  if (stroke.kind === "text") {
    context.font = `${stroke.fontSize}px Arial, sans-serif`;
    context.textBaseline = "alphabetic";
    context.textAlign = "left";
    context.direction = "ltr";
    context.fillStyle = stroke.color;
    const metrics = context.measureText(stroke.text);
    const ascent = metrics.fontBoundingBoxAscent;
    const descent = metrics.fontBoundingBoxDescent;
    const lineHeight = stroke.fontSize * TEXT_LINE_HEIGHT;
    // Match the textarea's half-leading above and below the font box.
    const baseline = (lineHeight - ascent - descent) / 2 + ascent;
    stroke.text.split("\n").forEach((line, index) => {
      context.fillText(
        line,
        stroke.x,
        stroke.y + index * lineHeight + baseline,
      );
    });
    return;
  }
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
