export function mountAttemptPagination({
  controls,
  heading,
  history,
  size,
  previous,
  next,
  status,
  render,
}) {
  size.value = "10";
  let page = 1;
  let pages = 0;
  let disabled = false;

  function syncDisabled() {
    size.disabled = disabled;
    previous.disabled = disabled || page === 1;
    next.disabled = disabled || page >= pages;
  }

  size.addEventListener("change", () => {
    page = 1;
    render();
  });
  previous.addEventListener("click", () => {
    page -= 1;
    render();
  });
  next.addEventListener("click", () => {
    page += 1;
    render();
  });

  return {
    reset() {
      page = 1;
    },
    setDisabled(value) {
      disabled = value;
      syncDisabled();
    },
    hasMultiplePages(attempts) {
      return attempts.length > Number(size.value);
    },
    items(attempts) {
      const focused = document.activeElement;
      const count = attempts.length;
      const limit = Number(size.value);
      pages = Math.ceil(count / limit);
      const controlsHidden = count <= Number(size.options[0].value);
      page = Math.min(Math.max(page, 1), Math.max(pages, 1));
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
