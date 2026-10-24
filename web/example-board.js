import {
  BOARD_WIDTH,
  BOARD_HEIGHT,
  PEN_COLORS,
  MAX_STROKES,
  boardPoint,
  createBoard,
  createPointerGate,
  drawBoard,
} from "./whiteboard.js";

const TOOL_ICONS = {
  pen: '<path d="m4 16 12-12 4 4-12 12-5 1Z M14 6l4 4"/>',
  eraser: '<path d="m4 14 9-10 7 7-9 10H7Z M8 10l7 7 M11 21h10"/>',
  undo: '<path d="m9 5-5 5 5 5 M4 10h10a6 6 0 0 1 0 12"/>',
  clear: '<path d="M3 6h18 M9 6V3h6v3 M5 6l1 15h12l1-15 M10 10v7 M14 10v7"/>',
};

function setToolIcon(button, name) {
  const label = name[0].toUpperCase() + name.slice(1);
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
  const status = root.querySelector("[role=status]");
  let tool = "pen";
  let color = PEN_COLORS[0].color;
  let drag = null;
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
    const now = performance.now();
    if (now - lastActivity < 1000) return;
    lastActivity = now;
    activity?.();
  }

  function paint() {
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
    record(ops);
    dirty = true;
    paint();
    schedule();
  }

  function paintToolState() {
    for (const button of tools.querySelectorAll("button"))
      button.setAttribute("aria-pressed", String(button.dataset.tool === tool));
    for (const button of root.querySelectorAll(".board-color"))
      button.setAttribute(
        "aria-pressed",
        String(button.dataset.color === color),
      );
  }

  for (const name of ["pen", "eraser"]) {
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
      tool = "pen";
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
      status.textContent =
        "Drawing limit reached. Undo or clear before adding more.";
      return;
    }
    if (!model.begin(tool, color, start.x, start.y)) {
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
    const changed = model.extend(drag.end.x, drag.end.y);
    if (changed) noteActivity();
    paint();
  });
  function finish() {
    if (!drag) return;
    model.end();
    drag = null;
    changed();
    paint();
  }
  function cancel(event) {
    if (!pointers.up(event)) return;
    model.cancel();
    drag = null;
    paint();
    if (previewShared) dirty = true;
    previewShared = false;
    if (dirty) schedule();
  }
  canvas.addEventListener("pointerup", (event) => {
    if (!pointers.up(event)) return;
    if (locked()) {
      model.cancel();
      drag = null;
      if (previewShared) dirty = true;
      previewShared = false;
      paint();
      if (dirty) schedule();
      return;
    }
    if (drag) {
      drag.end = boardPoint(canvas, event);
      model.extend(drag.end.x, drag.end.y);
    }
    finish();
  });
  canvas.addEventListener("pointercancel", cancel);
  canvas.addEventListener("lostpointercapture", cancel);
  root.addEventListener("toggle", () => {
    if (!root.open) cancelTurn?.();
    if (!root.open && drag) {
      model.cancel();
      drag = null;
      if (previewShared) dirty = true;
      previewShared = false;
    }
    paint();
    if (dirty && !drag) schedule();
  });
  canvas.width = BOARD_WIDTH;
  canvas.height = BOARD_HEIGHT;
  paint();
  return {
    refresh: paint,
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
