/// Open the existing picker in a modal without changing its selection state.
export function mountProblemGrid() {
  if (typeof window.HTMLDialogElement !== "function") return;
  const picker = document.querySelector("details.problem-picker");
  const grid = picker.querySelector(".problem-grid");
  const setup = document.querySelector(".setup-grid");
  const summary = picker.querySelector("summary");
  const search = picker.querySelector("input[type=search]");
  const home = document.createComment("problem-picker-home");
  picker.before(home);
  const dialog = document.createElement("dialog");
  dialog.className = "problem-picker-dialog choosing-problem";
  dialog.setAttribute("aria-labelledby", "problem-picker-heading");
  setup.append(dialog);
  summary.setAttribute("aria-haspopup", "dialog");
  let expanded = false;
  let scrollTop = 0;
  let pressedBackdrop = false;

  function outsideDialog(event) {
    const rect = dialog.getBoundingClientRect();
    return (
      event.clientX < rect.left ||
      event.clientX > rect.right ||
      event.clientY < rect.top ||
      event.clientY > rect.bottom
    );
  }
  dialog.addEventListener("pointerdown", (event) => {
    pressedBackdrop = event.button === 0 && outsideDialog(event);
  });
  dialog.addEventListener("click", (event) => {
    if (pressedBackdrop && outsideDialog(event)) picker.open = false;
    pressedBackdrop = false;
  });

  dialog.addEventListener("cancel", (event) => {
    event.preventDefault();
    picker.open = false;
  });
  search.addEventListener("keydown", (event) => {
    if (
      event.key !== "Escape" ||
      event.isComposing ||
      event.keyCode === 229 ||
      event.repeat ||
      event.ctrlKey ||
      event.metaKey ||
      event.altKey ||
      event.shiftKey
    )
      return;
    // Search fields otherwise consume Escape by clearing the saved query.
    event.preventDefault();
    picker.open = false;
  });

  grid.addEventListener("beforematch", () => {
    picker.open = true;
    update();
  });

  function update() {
    const open = picker.open && !picker.hidden;
    const focused = document.activeElement;
    // Keep collapsed cards available to browser find and fragment navigation.
    if (open) {
      picker.append(grid);
      grid.removeAttribute("hidden");
    } else {
      setup.append(grid);
      grid.setAttribute("hidden", picker.hidden ? "" : "until-found");
    }
    if (open === expanded) return;
    expanded = open;
    if (open) {
      scrollTop = window.scrollY;
      dialog.append(picker);
      summary.removeAttribute("aria-haspopup");
      dialog.showModal();
      const target =
        picker.contains(focused) && focused !== summary ? focused : search;
      target.focus({ preventScroll: true });
      dialog.scrollTop = 0;
    } else {
      dialog.close();
      home.after(picker);
      summary.setAttribute("aria-haspopup", "dialog");
      if (!picker.hidden) summary.focus({ preventScroll: true });
      window.scrollTo({ top: scrollTop, behavior: "instant" });
    }
  }

  const observer = new MutationObserver(update);
  observer.observe(picker, {
    attributes: true,
    attributeFilter: ["open", "hidden"],
  });
  update();
}
