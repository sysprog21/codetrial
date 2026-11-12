import {
  BOARD_WIDTH,
  BOARD_HEIGHT,
  PEN_COLORS,
  MAX_STROKES,
  MAX_STROKES_MESSAGE,
  boardPoint,
  createBoard,
  createPointerGate,
  drawBoard,
} from "./whiteboard.js";
import { mountBoardText } from "./board-text.js";

const TOOL_ICONS = {
  line: '<path d="m4 20 16-16"/>',
  rectangle: '<rect x="4" y="5" width="16" height="14"/>',
  ellipse: '<ellipse cx="12" cy="12" rx="9" ry="7"/>',
  text: '<path d="M4 5h16 M12 5v16 M8 21h8"/>',
  pen: '<path d="m4 16 12-12 4 4-12 12-5 1Z M14 6l4 4"/>',
  eraser: '<path d="m4 14 9-10 7 7-9 10H7Z M8 10l7 7 M11 21h10"/>',
  undo: '<path d="m9 5-5 5 5 5 M4 10h10a6 6 0 0 1 0 12"/>',
  clear: '<path d="M3 6h18 M9 6V3h6v3 M5 6l1 15h12l1-15 M10 10v7 M14 10v7"/>',
  expand: '<path d="M4 9V4h5 M15 4h5v5 M20 15v5h-5 M9 20H4v-5"/>',
  shrink: '<path d="M9 4v5H4 M20 9h-5V4 M15 20v-5h5 M4 15h5v5"/>',
};

const SHAPE_TOOLS = ["line", "rectangle", "ellipse"];

function shapePoints(tool, start, end) {
  const { x, y } = start;
  if (tool === "line") return [x, y, end.x, end.y];
  if (tool === "rectangle")
    return [x, y, end.x, y, end.x, end.y, x, end.y, x, y];
  const cx = (x + end.x) / 2;
  const cy = (y + end.y) / 2;
  const rx = Math.abs(end.x - x) / 2;
  const ry = Math.abs(end.y - y) / 2;
  const points = [];
  // 128 segments keep the error below a board pixel even at full size,
  // with ample room under MAX_POINTS for a single replayable stroke.
  for (let i = 0; i < 128; i++) {
    const angle = (i * 2 * Math.PI) / 128;
    points.push(cx + rx * Math.cos(angle), cy + ry * Math.sin(angle));
  }
  points.push(...points.slice(0, 2));
  return points;
}

function setToolIcon(
  button,
  name,
  label = name[0].toUpperCase() + name.slice(1),
) {
  button.setAttribute("aria-label", label);
  button.title = name === "clear" ? "Clear drawing; Undo restores it" : label;
  button.classList.add("example-tool-icon");
  button.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">${TOOL_ICONS[name]}</svg>`;
}

export function mountExampleBoard(
  root,
  {
    locked,
    publish,
    record,
    activity,
    turnActivity,
    cancelTurn,
    connected = () => true,
  },
) {
  root.hidden = false;
  const canvas = root.querySelector("canvas");
  const context = canvas.getContext("2d");
  const model = createBoard();
  const pointers = createPointerGate();
  const tools = root.querySelector("[data-tools]");
  // The interview wraps the board in `.example-pane` so it can lift out of
  // the drawer; the task page keeps it directly under the details element.
  const status = root.querySelector(
    ":scope > [role=status], :scope > .example-pane > [role=status]",
  );
  const expand = root.querySelector("[data-action=expand]");
  const panel = root.closest(".editor-panel");
  const strip = canvas.parentElement;
  let tool = "pen";
  let color = PEN_COLORS[0].color;
  let drag = null;
  let scrollBack = [];
  let settle = null;
  let unsentSince = 0;
  let dirty = false;
  let uploading = false;
  let stopped = false;
  let edited = false;
  let previewShared = false;
  let lastActivity = -Infinity;

  function noteActivity() {
    turnActivity?.();
    pingActivity();
  }

  function pingActivity() {
    const now = performance.now();
    if (now - lastActivity < 1000) return;
    lastActivity = now;
    activity?.();
  }

  function paint() {
    textDraft.refresh();
    root.querySelector("[data-action=clear]").disabled = locked();
    const strokes = model.strokes();
    drawBoard(context, strokes);
    root.querySelector("[data-action=undo]").disabled =
      locked() || !model.canUndo();
  }

  async function send(preview = false) {
    clearTimeout(settle);
    settle = null;
    if (uploading || (drag && !preview)) return false;
    if (!dirty) return true;
    if (!connected()) {
      status.textContent = "Drawing kept here; it will sync when connected.";
      return false;
    }
    uploading = true;
    dirty = false;
    unsentSince = 0;
    const strokes = model.inkCount();
    const blob = await new Promise((resolve) =>
      canvas.toBlob(resolve, "image/jpeg", 0.72),
    );
    try {
      const sent = await publish(blob, strokes);
      if (!sent) dirty = true;
      status.textContent = sent
        ? "Drawing ready for Jim to read when you refer to it."
        : "Drawing kept here; it will sync when connected.";
      return sent;
    } catch (error) {
      dirty = true;
      status.textContent =
        "Drawing could not sync. Your drawing is still here.";
      console.warn("codetrial example_publish_failed", error);
      return false;
    } finally {
      uploading = false;
      if (dirty && !stopped) schedule();
    }
  }

  function schedule() {
    clearTimeout(settle);
    if (!connected()) return;
    unsentSince ||= Date.now();
    settle = setTimeout(
      () => send(),
      Math.max(0, Math.min(1000, 4000 - (Date.now() - unsentSince))),
    );
  }

  function changed() {
    const ops = model.takeOps();
    if (!ops.length) return;
    previewShared = false;
    edited = true;
    noteActivity();
    record?.(ops);
    dirty = true;
    paint();
    schedule();
  }

  function paintToolState() {
    canvas.style.cursor = tool === "text" ? "crosshair" : "";
    textDraft.refresh();
    for (const button of tools.querySelectorAll("button"))
      button.setAttribute("aria-pressed", String(button.dataset.tool === tool));
    for (const button of root.querySelectorAll("[data-colors] .board-color"))
      button.setAttribute(
        "aria-pressed",
        String(button.dataset.color === color),
      );
  }

  const textDraft = mountBoardText(canvas, {
    pointers,
    active: () => tool === "text" && !drag,
    locked: () => locked() || stopped || !root.open,
    activity() {
      cancelTurn?.();
      pingActivity();
    },
    commit(label) {
      if (drag || !model.addText(label)) return false;
      changed();
      return true;
    },
  });
  for (const name of ["pen", "eraser", "text", ...SHAPE_TOOLS]) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "board-tool";
    button.dataset.tool = name;
    setToolIcon(button, name);
    button.addEventListener("click", () => {
      if (drag) return;
      tool = name;
      paintToolState();
    });
    tools.append(button);
  }
  for (const { color: value, name } of PEN_COLORS) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "board-color";
    button.dataset.color = value;
    button.style.background = value;
    button.setAttribute("aria-label", `Pen ${name}`);
    button.addEventListener("click", () => {
      if (drag) return;
      color = value;
      if (!SHAPE_TOOLS.includes(tool)) tool = "pen";
      paintToolState();
    });
    root.querySelector("[data-colors]").append(button);
  }
  paintToolState();
  for (const action of ["undo", "clear"]) {
    const button = root.querySelector(`[data-action=${action}]`);
    setToolIcon(button, action);
    button.addEventListener("click", () => {
      if (!locked() && !drag && model[action]()) changed();
    });
  }

  canvas.addEventListener("pointerdown", (event) => {
    if (locked() || stopped) return;
    const claim = pointers.down(event);
    if (!claim) return;
    if (claim === "replace") model.cancel();
    const start = boardPoint(canvas, event);
    if (model.strokeCount() >= MAX_STROKES) {
      pointers.up(event);
      status.textContent = MAX_STROKES_MESSAGE;
      return;
    }
    if (
      !SHAPE_TOOLS.includes(tool) &&
      !model.begin(tool, color, start.x, start.y)
    ) {
      pointers.up(event);
      return;
    }
    clearTimeout(settle);
    drag = { start, end: start };
    noteActivity();
    canvas.setPointerCapture(event.pointerId);
    event.preventDefault();
    paint();
  });
  canvas.addEventListener("pointermove", (event) => {
    if (!drag || !pointers.owns(event)) return;
    if (locked()) {
      cancel(event);
      return;
    }
    const next = boardPoint(canvas, event);
    if (next.x === drag.end.x && next.y === drag.end.y) return;
    drag.end = next;
    const changed = extendDrag();
    if (changed) noteActivity();
    paint();
  });
  function extendDrag() {
    if (!SHAPE_TOOLS.includes(tool))
      return model.extend(drag.end.x, drag.end.y);
    // Replace only the uncommitted preview. cancel restores the undo branch;
    // end journals the completed outline once, like an ordinary pen stroke.
    model.cancel();
    const end = {
      x: Math.min(Math.max(drag.end.x, 0), BOARD_WIDTH),
      y: Math.min(Math.max(drag.end.y, 0), BOARD_HEIGHT),
    };
    const points = shapePoints(tool, drag.start, end);
    model.begin("pen", color, points[0], points[1]);
    for (let i = 2; i < points.length; i += 2)
      model.extend(points[i], points[i + 1]);
    return true;
  }
  function finish() {
    if (!drag) return;
    // A shape collapsed to one board pixel is an empty gesture, not a pen dot.
    if (
      SHAPE_TOOLS.includes(tool) &&
      (!model.isDrawing() || model.strokes().at(-1).points.length === 2)
    )
      return dropDrag();
    model.end();
    drag = null;
    changed();
    paint();
  }
  // Dropping a shared preview must also withdraw it from Jim's cached board.
  function dropDrag() {
    model.cancel();
    drag = null;
    if (previewShared) dirty = true;
    previewShared = false;
    paint();
    if (dirty) schedule();
  }
  function cancel(event) {
    if (pointers.up(event)) dropDrag();
  }
  canvas.addEventListener("pointerup", (event) => {
    if (!pointers.up(event)) return;
    if (locked()) return dropDrag();
    if (drag && model.isDrawing()) {
      drag.end = boardPoint(canvas, event);
      extendDrag();
    }
    finish();
  });
  canvas.addEventListener("pointercancel", cancel);
  canvas.addEventListener("lostpointercapture", cancel);
  // Expanding moves no element: the pane is restyled in place, so the canvas,
  // its strokes and anything waiting to reach Jim stay exactly as they were.
  // Only the drawn size changes, and `boardPoint` already scales by it.
  function expandBoard() {
    // Read before the restyle, which can clamp the drawing's scroll.
    scrollBack = [panel.scrollTop, strip.scrollTop, strip.scrollLeft];
    root.setAttribute("data-expanded", "");
    setToolIcon(expand, "shrink", "Shrink drawing");
    expand.title = "Shrink drawing (Esc)";
    expand.setAttribute("aria-keyshortcuts", "Escape");
    // The panel can scroll a little on a short screen, and the pane would
    // scroll with it; the stylesheet stops it scrolling while expanded. A text
    // box near the bottom keeps its controls below the canvas, so the drawing
    // can still scroll; it starts from the canvas's top.
    panel.scrollTop = 0;
    strip.scrollTop = 0;
    // A click focuses the button in Chromium but not in Safari or Firefox on
    // macOS, so focus left in what the stylesheet now hides moves to the
    // button.
    if (getComputedStyle(document.activeElement).visibility === "hidden")
      expand.focus();
  }
  function shrinkBoard() {
    if (!root.hasAttribute("data-expanded")) return;
    root.removeAttribute("data-expanded");
    setToolIcon(expand, "expand", "Expand drawing");
    expand.removeAttribute("aria-keyshortcuts");
    // The panel and the strip go back where the candidate had them, once the
    // drawer's observer has measured the docked height again after this
    // returns; restored before it, the strip is clamped against its old
    // height after a resize while expanded.
    queueMicrotask(() => {
      [panel.scrollTop, strip.scrollTop, strip.scrollLeft] = scrollBack;
    });
  }
  if (expand) {
    setToolIcon(expand, "expand", "Expand drawing");
    expand.addEventListener("click", () => {
      if (drag || textDraft.busy()) return;
      if (root.hasAttribute("data-expanded")) shrinkBoard();
      else expandBoard();
    });
    // Not a modal dialog: the sidebar's turn and mute controls stay usable
    // while the board is large, so Escape is handled here rather than by
    // `showModal`. A text draft claims Escape first and stops it. An ink
    // stroke takes no focus, so after a sidebar button the key still arrives
    // there. A field or a dialog keeps Escape for itself, and so does an
    // input method, a modifier chord, a held key or a handler that already
    // took it. Under a dialog the board is inert, so a key from Jim's stage,
    // which the ending wait lifts over that dialog, is not the board's; with
    // no dialog the stage's card is like the sidebar. The Escape that shrinks
    // the board is marked as taken, so a later listener does not act on it
    // too; one while docked passes through untouched.
    document.addEventListener("keydown", (event) => {
      if (
        !root.hasAttribute("data-expanded") ||
        event.key !== "Escape" ||
        event.isComposing ||
        event.keyCode === 229 ||
        event.repeat ||
        event.altKey ||
        event.ctrlKey ||
        event.metaKey ||
        event.shiftKey ||
        drag ||
        textDraft.busy() ||
        root.closest("[inert]") ||
        event.defaultPrevented ||
        event.target.closest?.("input, textarea, select, dialog, [role=dialog]")
      )
        return;
      event.preventDefault();
      shrinkBoard();
    });
  }
  root.addEventListener("toggle", () => {
    // Nothing on the page closes it while expanded, but whatever does must
    // not leave the editor hidden under a board that is gone. Everything a
    // close undoes is here: the board shrinks, then the turn and any stroke
    // are cancelled.
    if (!root.open) {
      shrinkBoard();
      cancelTurn?.();
      if (drag) return dropDrag();
    }
    paint();
    if (dirty && !drag) schedule();
  });
  canvas.width = BOARD_WIDTH;
  canvas.height = BOARD_HEIGHT;
  paint();
  return {
    refresh: paint,
    // The page asks this before it runs the tests, whose results land in the
    // drawer the board covers. Mid-gesture the board stays, as it does for
    // the button and Escape, and the answer is that the drawer is not shown.
    shrink() {
      if (!drag && !textDraft.busy()) shrinkBoard();
      return !root.hasAttribute("data-expanded");
    },
    async flush() {
      const deadline = Date.now() + 2000;
      while (uploading && Date.now() < deadline)
        await new Promise((resolve) => setTimeout(resolve, 25));
      if (uploading) return false;
      if (drag) {
        // Share the visible preview without ending the pointer's stroke.
        edited = true;
        previewShared = true;
        dirty = true;
      }
      return Promise.race([
        send(true),
        new Promise((resolve) =>
          setTimeout(() => resolve(false), Math.max(0, deadline - Date.now())),
        ),
      ]);
    },
    image() {
      if (!edited) return undefined;
      try {
        return canvas.toDataURL("image/jpeg", 0.72);
      } catch {
        return undefined;
      }
    },
    async reconnect() {
      if (!edited) return true;
      dirty = true;
      if (stopped) {
        const deadline = Date.now() + 2000;
        while (uploading && Date.now() < deadline)
          await new Promise((resolve) => setTimeout(resolve, 25));
        return Promise.race([
          send(),
          new Promise((resolve) => setTimeout(() => resolve(false), 2000)),
        ]);
      }
      schedule();
      return true;
    },
    async finish() {
      stopped = true;
      textDraft.cancel(false);
      finish();
      clearTimeout(settle);
      // An upload that stalls must not hold the interview's End button.
      const deadline = Date.now() + 2000;
      while (uploading && Date.now() < deadline)
        await new Promise((resolve) => setTimeout(resolve, 25));
      await Promise.race([
        send(),
        new Promise((resolve) =>
          setTimeout(resolve, Math.max(0, deadline - Date.now())),
        ),
      ]);
    },
  };
}

// Drawing has no audio turn for Gemini to finish. Count its quiet window on
// the page, flush the visible ink, and explicitly hand over once per pause.
export function createDrawingTurn({
  now = () => performance.now(),
  requestFrame = requestAnimationFrame,
  windowMs,
  blocked,
  paint,
  flush,
  yieldTurn,
}) {
  let lastActivity = null;
  let revision = 0;
  let running = false;
  let flushing = false;

  function reset() {
    lastActivity = null;
    revision++;
    paint(null);
  }

  async function handOver() {
    if (flushing || blocked() || lastActivity === null) return;
    const before = revision;
    flushing = true;
    try {
      try {
        await flush();
      } catch {
        // The turn can still finish when the picture could not be shared.
      }
      if (revision !== before || blocked()) return;
      lastActivity = null;
      revision++;
      paint(null);
      await yieldTurn();
    } catch {
      // Connection failures are handled by the interview's reconnect flow.
    } finally {
      flushing = false;
    }
  }

  function frame() {
    if (blocked()) reset();
    if (lastActivity === null) {
      running = false;
      return;
    }
    const silenceMs = windowMs();
    const progress = silenceMs
      ? Math.min(1, (now() - lastActivity) / silenceMs)
      : null;
    paint(progress);
    if (progress === 1 && !flushing) void handOver();
    requestFrame(frame);
  }

  return {
    activity() {
      if (blocked()) return;
      lastActivity = now();
      revision++;
      paint(0);
      if (!running) {
        running = true;
        requestFrame(frame);
      }
    },
    speech() {
      if (lastActivity === null) return;
      lastActivity = now();
      revision++;
    },
    pending: () => lastActivity !== null || flushing,
    yield: handOver,
    reset,
  };
}

export async function reconnectCodingInterview({
  ending,
  board,
  flush,
  publishCode,
}) {
  if (ending) {
    try {
      await board?.reconnect();
    } finally {
      flush();
      publishCode();
    }
  } else {
    flush();
    void board?.reconnect();
    publishCode();
  }
}
