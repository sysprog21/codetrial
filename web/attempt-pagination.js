import { clamp } from "./lib.js";

export function mountAttemptPagination({
  controls,
  heading,
  history,
  size,
  previous,
  next,
  status,
  announcement,
  render,
}) {
  let page = 1;
  let pages = 0;
  let disabled = false;

  function syncDisabled() {
    size.disabled = disabled;
    previous.disabled = disabled || page === 1;
    next.disabled = disabled || page >= pages;
  }

  function go(to) {
    page = to;
    render();
    if (!controls.hidden) announcement.textContent = status.textContent;
  }

  size.addEventListener("change", () => go(1));
  previous.addEventListener("click", () => go(page - 1));
  next.addEventListener("click", () => go(page + 1));

  return {
    reset() {
      page = 1;
    },
    setDisabled(value) {
      disabled = value;
      syncDisabled();
    },
    hasMultiplePages() {
      return pages > 1;
    },
    items(attempts) {
      const focused = document.activeElement;
      const count = attempts.length;
      const limit = Number(size.value);
      pages = Math.ceil(count / limit);
      // Keep the controls available when the list exceeds the smallest page
      // size, even if the chosen size fits every attempt on one page.
      const controlsHidden = count <= Number(size.options[0].value);
      page = clamp(page, 1, Math.max(pages, 1));
      // Set boundary states first so the destination can receive focus.
      controls.hidden = controlsHidden;
      syncDisabled();
      if (
        focused &&
        (history.contains(focused) ||
          (controlsHidden && controls.contains(focused)))
      ) {
        heading.focus();
      } else if (focused === previous && page === 1) {
        (page < pages ? next : size).focus();
      } else if (focused === next && page === pages) {
        (page > 1 ? previous : size).focus();
      }
      status.textContent = count ? `Page ${page} of ${pages}` : "";
      history.start = (page - 1) * limit + 1;
      return [...attempts].reverse().slice((page - 1) * limit, page * limit);
    },
  };
}
