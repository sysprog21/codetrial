/// Readiness can leave editable work without a deadline. Neither that work nor
/// a timed attempt is safe to leave until a report copy is in Past attempts.
export function shouldWarnBeforeUnload({
  endsAt,
  workStarted,
  reportPersisted,
}) {
  return (
    (workStarted || (Number.isFinite(endsAt) && endsAt > 0)) && !reportPersisted
  );
}

/// The browser owns the confirmation text; the page can only request it.
///
/// The desktop app asks the same question before it closes its window, by
/// dispatching a cancelable `beforeunload` itself and reading
/// `defaultPrevented`, since a window close sends none. That is why `warn`
/// calls `preventDefault` rather than only setting `returnValue`.
export function createBeforeUnloadGuard(target = window) {
  let enabled = false;
  const warn = (event) => {
    event.preventDefault();
    event.returnValue = true;
  };

  const setEnabled = (next) => {
    const shouldEnable = Boolean(next);
    if (shouldEnable === enabled) return;
    enabled = shouldEnable;
    if (enabled) target.addEventListener("beforeunload", warn);
    else target.removeEventListener("beforeunload", warn);
  };

  return { setEnabled, dispose: () => setEnabled(false) };
}
