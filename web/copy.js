export function mountCopyButton(
  button,
  status,
  getText,
  failureMessage,
  clipboard,
) {
  let pendingRevision = null;
  let revision = 0;
  let reset;
  button.addEventListener("click", async () => {
    if (pendingRevision === revision) return;
    const text = getText();
    if (text === null) return;
    const copiedRevision = revision;
    pendingRevision = copiedRevision;
    clearTimeout(reset);
    button.setAttribute("aria-disabled", "true");
    status.textContent = "Copying...";
    try {
      await clipboard.writeText(text);
      if (copiedRevision !== revision) return;
      status.textContent = "Copied!";
      reset = setTimeout(() => {
        status.textContent = "";
      }, 2000);
    } catch {
      if (copiedRevision === revision) status.textContent = failureMessage;
    } finally {
      if (pendingRevision === copiedRevision) {
        pendingRevision = null;
        button.removeAttribute("aria-disabled");
      }
    }
  });
  return () => {
    revision++;
    button.removeAttribute("aria-disabled");
    clearTimeout(reset);
    status.textContent = "";
  };
}
