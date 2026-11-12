import {
  BOARD_WIDTH,
  BOARD_HEIGHT,
  PEN_COLORS,
  TEXT_SIZE,
  TEXT_MIN_SIZE,
  TEXT_MAX_SIZE,
  TEXT_LINE_HEIGHT,
  TEXT_MAX_LENGTH,
  TEXT_MAX_LINES,
  boardPoint,
} from "./whiteboard.js";

// The overlay is a draft, not a drawing operation. Both surfaces commit through
// their existing recording and publishing paths only when Confirm succeeds.
export function mountBoardText(
  canvas,
  { active, locked, commit, activity, pointers },
) {
  const document = canvas.ownerDocument;
  const host = canvas.parentElement;
  const box = document.createElement("div");
  box.className = "board-text-draft";
  box.hidden = true;
  box.setAttribute("role", "group");
  box.setAttribute("aria-label", "Draft text label");
  const input = document.createElement("textarea");
  input.className = "board-text-input";
  input.setAttribute("aria-label", "Label text");
  input.wrap = "off";
  input.maxLength = TEXT_MAX_LENGTH;
  input.spellcheck = false;
  input.dir = "ltr";
  function button(text, className) {
    const node = document.createElement("button");
    node.type = "button";
    node.className = className;
    node.textContent = text;
    return node;
  }
  const edges = ["n", "e", "s", "w"].map((edge) => {
    const node = button("", "board-text-move");
    node.dataset.edge = edge;
    node.setAttribute("aria-label", "Move text box");
    node.title = "Drag to move";
    node.tabIndex = -1;
    return node;
  });
  const handles = ["nw", "n", "ne", "e", "se", "s", "sw", "w"].map((corner) => {
    const node = button("", "board-text-resize");
    node.dataset.handle = corner;
    node.setAttribute("aria-label", `Scale text and box (${corner})`);
    node.title = "Drag to resize";
    node.tabIndex = -1;
    return node;
  });
  const controls = document.createElement("div");
  controls.className = "board-text-controls";
  const colors = document.createElement("div");
  colors.className = "board-pens";
  colors.setAttribute("role", "group");
  colors.setAttribute("aria-label", "Text color");
  for (const entry of PEN_COLORS) {
    const swatch = button("", "board-color");
    swatch.style.background = entry.color;
    swatch.dataset.color = entry.color;
    swatch.setAttribute("aria-label", `${entry.name} text`);
    swatch.title = `${entry.name} text`;
    swatch.addEventListener("click", () => {
      if (!draft || locked()) return;
      draft.color = entry.color;
      lastColor = entry.color;
      layout();
      activity?.();
    });
    colors.append(swatch);
  }
  const confirm = button("Confirm", "board-text-confirm");
  confirm.title = "Confirm (Ctrl+Enter)";
  const cancel = button("Cancel", "board-text-cancel");
  const hint = document.createElement("p");
  hint.className = "board-text-hint";
  hint.textContent = `Enter adds a line; Ctrl+Enter confirms; Escape cancels. Limit: ${TEXT_MAX_LENGTH} UTF-16 units, ${TEXT_MAX_LINES} lines.`;
  const error = document.createElement("p");
  error.className = "board-text-error";
  error.setAttribute("role", "status");
  const id = `text-draft-${document.querySelectorAll(".board-text-draft").length}`;
  hint.id = `${id}-hint`;
  error.id = `${id}-error`;
  input.setAttribute("aria-describedby", `${hint.id} ${error.id}`);
  controls.append(colors, confirm, cancel, hint, error);
  box.append(input, ...edges, ...handles, controls);
  host.append(box);
  canvas.tabIndex = -1;
  canvas.setAttribute(
    "aria-description",
    "Select Text and click or drag to create a label.",
  );
  const measure = document.createElement("canvas").getContext("2d");
  let draft = null;
  let lastColor = PEN_COLORS[0].color;
  let gesture = null;
  let composing = false;
  const limit = (value, min, max) => Math.max(min, Math.min(max, value));

  function layout() {
    if (!draft) return;
    const sx = canvas.clientWidth / BOARD_WIDTH;
    const sy = canvas.clientHeight / BOARD_HEIGHT;
    box.style.left = `${canvas.offsetLeft + canvas.clientLeft + draft.x * sx}px`;
    box.style.top = `${canvas.offsetTop + canvas.clientTop + draft.y * sy}px`;
    box.style.width = `${draft.width * sx}px`;
    box.style.height = `${draft.height * sy}px`;
    // Lay out at the canvas font size before scaling. Measuring a smaller CSS
    // font can round its metrics differently from the scaled board bitmap.
    input.style.width = `${draft.width}px`;
    input.style.height = `${draft.height}px`;
    input.style.fontSize = `${draft.fontSize}px`;
    input.style.lineHeight = `${draft.fontSize * TEXT_LINE_HEIGHT}px`;
    input.style.transform = `scale(${sx}, ${sy})`;
    input.style.color = draft.color;
    for (const swatch of colors.children)
      swatch.setAttribute(
        "aria-pressed",
        String(swatch.dataset.color === draft.color),
      );
    // Keep fixed-size controls within the scrollable surface, even for a small
    // draft at its right edge. They are never scaled with the label.
    const toolbarWidth = Math.min(280, canvas.clientWidth);
    controls.style.left = `${limit((draft.width * sx - toolbarWidth) / 2, -draft.x * sx, canvas.clientWidth - draft.x * sx - toolbarWidth)}px`;
    controls.style.width = `${toolbarWidth}px`;
  }
  function close(focus = true) {
    const previous = gesture;
    gesture = null;
    draft = null;
    composing = false;
    box.hidden = true;
    box.classList.remove("is-creating");
    if (previous?.kind === "create") pointers.up({ pointerId: previous.id });
    if (previous?.target.hasPointerCapture(previous.id))
      previous.target.releasePointerCapture(previous.id);
    if (focus) canvas.focus({ preventScroll: true });
  }
  function refresh() {
    if (!draft) return;
    // Selecting another drawing tool is not a request to discard typed text.
    if (locked()) {
      close(false);
      return;
    }
    layout();
  }
  function begin(point, width = 320, height = TEXT_SIZE * TEXT_LINE_HEIGHT) {
    draft = {
      x: limit(point.x, 0, BOARD_WIDTH - width),
      y: limit(point.y, 0, BOARD_HEIGHT - height),
      width,
      height,
      minimumWidth: width,
      minimumHeight: height,
      fontSize: TEXT_SIZE,
      color: lastColor,
    };
    input.value = "";
    error.textContent = "";
    input.removeAttribute("aria-invalid");
    box.hidden = false;
    composing = false;
    layout();
    activity?.();
  }
  function contentSize() {
    // Textareas expand tab stops, but canvas paints tabs as single spaces.
    // Normalize the visible draft too, preserving the selection after expansion.
    if (input.value.includes("\t")) {
      const value = input.value;
      const { selectionStart, selectionEnd, selectionDirection } = input;
      input.value = value.replace(/\t/g, "    ");
      input.setSelectionRange(
        value.slice(0, selectionStart).replace(/\t/g, "    ").length,
        value.slice(0, selectionEnd).replace(/\t/g, "    ").length,
        selectionDirection,
      );
    }
    measure.font = `${draft.fontSize}px Arial, sans-serif`;
    const lines = input.value.replace(/\r\n?/g, "\n").split("\n");
    return {
      width:
        Math.max(...lines.map((line) => measure.measureText(line).width)) + 2,
      height: lines.length * draft.fontSize * TEXT_LINE_HEIGHT,
    };
  }
  function fitContent() {
    const size = contentSize();
    // Only content-driven growth is reversible; preserve the manually set size.
    draft.width = Math.min(
      BOARD_WIDTH,
      Math.max(draft.minimumWidth, size.width),
    );
    draft.height = Math.min(
      BOARD_HEIGHT,
      Math.max(draft.minimumHeight, size.height),
    );
    draft.x = Math.min(draft.x, BOARD_WIDTH - draft.width);
    draft.y = Math.min(draft.y, BOARD_HEIGHT - draft.height);
    layout();
    // Native text controls can reserve extra descent space after the last line.
    // Include it in the draft so caret scrolling cannot shift the visible label.
    const extraHeight = input.scrollHeight - input.clientHeight;
    if (extraHeight > 0 && draft.height + extraHeight <= BOARD_HEIGHT) {
      draft.height += extraHeight;
      draft.y = Math.min(draft.y, BOARD_HEIGHT - draft.height);
      layout();
    }
    input.scrollTop = 0;
  }
  function fontForHeight(height) {
    // Fill the box with complete line boxes, including the shared line spacing.
    const lines = input.value.replace(/\r\n?/g, "\n").split("\n").length;
    return limit(
      height / (lines * TEXT_LINE_HEIGHT),
      TEXT_MIN_SIZE,
      TEXT_MAX_SIZE,
    );
  }
  function resize(snapshot, dx, dy, handle) {
    // Resize each axis independently, keeping the opposite edge fixed.
    const right = snapshot.x + snapshot.width;
    const bottom = snapshot.y + snapshot.height;
    const minHeight = Math.min(
      TEXT_MIN_SIZE * TEXT_LINE_HEIGHT,
      snapshot.height,
    );
    if (handle.includes("w")) {
      draft.x = limit(snapshot.x + dx, 0, right - 1);
      draft.width = right - draft.x;
    } else if (handle.includes("e")) {
      draft.width = limit(snapshot.width + dx, 1, BOARD_WIDTH - snapshot.x);
    }
    if (handle.includes("w") || handle.includes("e"))
      draft.minimumWidth = draft.width;
    if (handle.includes("n")) {
      draft.y = limit(snapshot.y + dy, 0, bottom - minHeight);
      draft.height = bottom - draft.y;
    } else if (handle.includes("s")) {
      draft.height = limit(
        snapshot.height + dy,
        minHeight,
        BOARD_HEIGHT - snapshot.y,
      );
    }
    // Height controls type size; changing width never stretches the glyphs.
    if (handle.includes("n") || handle.includes("s")) {
      draft.fontSize = fontForHeight(draft.height);
      draft.minimumHeight = draft.height;
    }
  }
  function startGesture(event, kind, target) {
    if (!draft || gesture || locked() || event.button !== 0 || !event.isPrimary)
      return;
    event.preventDefault();
    const point = boardPoint(canvas, event);
    gesture = {
      id: event.pointerId,
      kind,
      target,
      point,
      snapshot: { ...draft },
    };
    target.setPointerCapture(event.pointerId);
  }
  canvas.addEventListener(
    "pointerdown",
    (event) => {
      if (!active() || locked()) return;
      event.stopImmediatePropagation();
      event.preventDefault();
      if (event.button !== 0 || !event.isPrimary) return;
      if (draft && gesture?.kind !== "create") return;
      const claim = pointers.down(event);
      if (!claim) return;
      if (claim === "replace") close(false);
      const point = boardPoint(canvas, event);
      begin(point, 0, 0);
      box.classList.add("is-creating");
      startGesture(event, "create", canvas);
    },
    true,
  );
  for (const edge of edges)
    edge.addEventListener("pointerdown", (event) =>
      startGesture(event, "move", edge),
    );
  for (const handle of handles)
    handle.addEventListener("pointerdown", (event) =>
      startGesture(event, "scale", handle),
    );
  function track(event) {
    if (!gesture || gesture.id !== event.pointerId) return;
    if (locked()) {
      close(false);
      return;
    }
    event.preventDefault();
    const point = boardPoint(canvas, event);
    const { snapshot, kind } = gesture;
    const dx = point.x - gesture.point.x;
    const dy = point.y - gesture.point.y;
    if (kind === "create") {
      const x = limit(point.x, 0, BOARD_WIDTH);
      const y = limit(point.y, 0, BOARD_HEIGHT);
      draft.width = Math.abs(x - gesture.point.x);
      draft.height = Math.abs(y - gesture.point.y);
      draft.fontSize = fontForHeight(draft.height);
      gesture.dragged ||=
        Math.hypot(
          (dx * canvas.clientWidth) / BOARD_WIDTH,
          (dy * canvas.clientHeight) / BOARD_HEIGHT,
        ) >= 4;
      draft.x = limit(
        Math.min(x, gesture.point.x),
        0,
        BOARD_WIDTH - draft.width,
      );
      draft.y = limit(
        Math.min(y, gesture.point.y),
        0,
        BOARD_HEIGHT - draft.height,
      );
    } else if (kind === "move") {
      draft.x = limit(snapshot.x + dx, 0, BOARD_WIDTH - draft.width);
      draft.y = limit(snapshot.y + dy, 0, BOARD_HEIGHT - draft.height);
    } else {
      const handle = gesture.target.dataset.handle;
      resize(snapshot, dx, dy, handle);
      fitContent();
    }
    activity?.();
    layout();
  }
  function finishGesture(event) {
    if (!gesture || gesture.id !== event.pointerId) return;
    const previous = gesture;
    if (previous.kind === "create") pointers.up(event);
    if (event.type === "pointerup") track(event);
    if (!draft) return;
    gesture = null;
    if (previous.target.hasPointerCapture(previous.id))
      previous.target.releasePointerCapture(previous.id);
    if (event.type !== "pointerup") {
      if (previous.kind === "create") {
        close();
        return;
      }
      draft = previous.snapshot;
    }
    if (previous.kind === "create") {
      box.classList.remove("is-creating");
      if (!previous.dragged) begin(previous.point);
      draft.minimumWidth = draft.width;
      draft.minimumHeight = draft.height;
      // A very shallow drag still needs room for one editable line on release.
      fitContent();
      input.focus({ preventScroll: true });
    }
    layout();
  }
  for (const target of [canvas, ...edges, ...handles]) {
    target.addEventListener("pointermove", track);
    target.addEventListener("pointerup", finishGesture);
    target.addEventListener("pointercancel", finishGesture);
    target.addEventListener("lostpointercapture", finishGesture);
  }
  input.addEventListener("input", () => {
    if (!draft) return;
    error.textContent = "";
    input.removeAttribute("aria-invalid");
    fitContent();
    activity?.();
  });
  input.addEventListener("compositionstart", () => {
    composing = true;
  });
  input.addEventListener("compositionend", () => {
    composing = false;
  });
  confirm.addEventListener("click", () => {
    if (!draft || gesture || composing) return;
    if (locked()) {
      close();
      return;
    }
    if (!input.value.trim()) {
      close();
      return;
    }
    const size = contentSize();
    if (
      input.value.length > TEXT_MAX_LENGTH ||
      input.value.split("\n").length > TEXT_MAX_LINES ||
      size.width > BOARD_WIDTH - draft.x ||
      size.height > BOARD_HEIGHT - draft.y
    ) {
      error.textContent =
        "Text exceeds the board or input limits. Shorten it or scale it down.";
    } else if (
      commit({
        text: input.value,
        color: draft.color,
        x: draft.x,
        y: draft.y,
        fontSize: draft.fontSize,
      })
    ) {
      close();
      return;
    } else {
      error.textContent =
        "The board cannot accept more text. Cancel, then Undo or Clear to make room.";
    }
    input.setAttribute("aria-invalid", "true");
    input.focus();
  });
  cancel.addEventListener("click", () => close());
  function onDraftKeydown(event) {
    if (event.isComposing || event.keyCode === 229 || composing || event.repeat)
      return;
    if (!draft || locked()) return;
    if (
      event.key === "Escape" &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey
    ) {
      event.preventDefault();
      event.stopPropagation();
      close();
      return;
    }
    if (
      event.key === "Enter" &&
      event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      !event.shiftKey
    ) {
      event.preventDefault();
      event.stopPropagation();
      confirm.click();
      return;
    }
  }
  box.addEventListener("keydown", onDraftKeydown);
  canvas.addEventListener("keydown", onDraftKeydown);
  const observer = new ResizeObserver(refresh);
  observer.observe(canvas);
  return { refresh, cancel: close, busy: () => gesture !== null };
}
