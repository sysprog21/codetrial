export function mountInterviewDrawer(panel) {
  const drawer = panel.querySelector(".test-drawer");
  const handle = panel.querySelector(".drawer-resizer");
  const example = panel.querySelector("#example-board");
  const surface = panel.querySelector(".example-surface");
  const results = panel.querySelector("#results-body");
  let height = null;
  let drag = null;

  function expanded() {
    return (
      [...drawer.querySelectorAll("details")].some((node) => node.open) ||
      !results.hidden
    );
  }

  function limits() {
    const toolbar = panel.querySelector(".editor-toolbar")?.offsetHeight ?? 0;
    return {
      min: 160,
      max: Math.max(
        160,
        Math.min(
          panel.clientHeight * 0.7,
          panel.clientHeight - toolbar - handle.offsetHeight - 128,
        ),
      ),
    };
  }

  function paint() {
    const { min, max } = limits();
    const open = expanded();
    const sized = open && (example.open || height !== null);
    if (sized) {
      height = Math.max(
        min,
        Math.min(max, height ?? panel.clientHeight * 0.45),
      );
      drawer.style.height = `${height}px`;
    } else drawer.style.height = "auto";
    handle.setAttribute("aria-valuemin", String(sized ? Math.round(min) : 0));
    handle.setAttribute("aria-valuemax", String(Math.round(max)));
    handle.setAttribute(
      "aria-valuenow",
      String(Math.round(sized ? height : drawer.clientHeight)),
    );
    if (example.open) {
      // Reserve room for summaries, tools and results; the drawing scrolls
      // inside its viewport without changing the model's coordinates.
      const contentHeight = [...drawer.children].reduce((total, node) => {
        if (node.hidden) return total;
        const style = getComputedStyle(node);
        return (
          total +
          node.getBoundingClientRect().height +
          parseFloat(style.marginTop) +
          parseFloat(style.marginBottom)
        );
      }, 0);
      const other = contentHeight - surface.getBoundingClientRect().height;
      surface.style.height = `${Math.max(120, drawer.clientHeight - other)}px`;
    }
  }

  function resize(next) {
    if (!expanded()) example.open = true;
    height = next;
    paint();
  }

  handle.addEventListener("pointerdown", (event) => {
    if (event.button !== 0 || drag) return;
    drag = {
      id: event.pointerId,
      y: event.clientY,
      height: drawer.clientHeight,
    };
    handle.setPointerCapture(event.pointerId);
    event.preventDefault();
  });
  handle.addEventListener("pointermove", (event) => {
    if (drag?.id !== event.pointerId) return;
    resize(drag.height + drag.y - event.clientY);
  });
  const end = (event) => {
    if (drag?.id === event.pointerId) drag = null;
  };
  handle.addEventListener("pointerup", (event) => {
    if (drag?.id === event.pointerId)
      resize(drag.height + drag.y - event.clientY);
    end(event);
  });
  handle.addEventListener("pointercancel", end);
  handle.addEventListener("lostpointercapture", end);
  handle.addEventListener("keydown", (event) => {
    if (!["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const { min, max } = limits();
    resize(
      event.key === "Home"
        ? min
        : event.key === "End"
          ? max
          : drawer.clientHeight + (event.key === "ArrowUp" ? 24 : -24),
    );
  });
  for (const details of drawer.querySelectorAll("details"))
    details.addEventListener("toggle", paint);
  new MutationObserver(paint).observe(results, {
    attributes: true,
    attributeFilter: ["hidden"],
    childList: true,
    subtree: true,
  });
  new ResizeObserver(paint).observe(panel);
  paint();
}
