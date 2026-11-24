// Split out of task.js: the page events the fullscreen and visibility rules
// watch during an attempt, including the set's grace period for coming back to
// fullscreen. It reaches what task.js still owns through `host` (see
// task-state.js), so it can be imported without the page.

import { fullscreenGraceMs } from "./task-controller.js";
import { $ } from "./task-dom.js";
import { host, monitoring, st } from "./task-state.js";

/// Leaving fullscreen during the attempt opens the set's grace period while
/// returns are left; otherwise, and before the attempt runs, it ends it.
function fullscreenLeft() {
  const graceMs = monitoring()
    ? fullscreenGraceMs(
        st.task.rules,
        st.fullscreenReturnsUsed,
        st.task.fullscreenReturns,
      )
    : 0;
  if (!graceMs) return ruleBroken("fullscreen");
  const leftAt = performance.now();
  // The text is an alert, so it is written only when the count changes; a
  // rewrite on every tick could have it read out several times a second.
  let shown;
  const paint = () => {
    const left = Math.max(
      0,
      Math.ceil((graceMs - (performance.now() - leftAt)) / 1000),
    );
    if (left === shown) return;
    shown = left;
    $("fullscreen-return-text").textContent =
      `You left fullscreen. Return within ${left} seconds or the attempt ends.`;
  };
  paint();
  $("fullscreen-return").hidden = false;
  st.fullscreenAway = {
    leftAt,
    ticker: setInterval(paint, 250),
    timer: setTimeout(() => {
      host.endAttempt("invalid", "fullscreen", closeFullscreenAway());
    }, graceMs),
  };
}

/// Closes an open grace period, returning how long the learner was away, or
/// null when none was open.
export function closeFullscreenAway() {
  const away = st.fullscreenAway;
  if (!away) return null;
  clearTimeout(away.timer);
  clearInterval(away.ticker);
  st.fullscreenAway = null;
  // Focus on the button would fall to <body> once its panel is hidden, and the
  // learner's next key with it; the editor is where they were working.
  const panel = $("fullscreen-return");
  if (panel.contains(document.activeElement) && !$("code").disabled)
    $("code").focus();
  panel.hidden = true;
  return performance.now() - away.leftAt;
}

function fullscreenReturned() {
  if (closeFullscreenAway() !== null) st.fullscreenReturnsUsed += 1;
}

/// The click is the gesture fullscreen needs; Escape is held again with it.
function returnToFullscreen() {
  const request = $("workspace").requestFullscreen?.();
  navigator.keyboard?.lock?.(["Escape"]).catch(() => {});
  // The grace period can run out while the request is pending, and the
  // ending then finds nothing to leave; the fullscreen granted after it would
  // keep the finished page in fullscreen with Escape held, so it is left.
  Promise.resolve(request)
    .then(() => {
      if (!monitoring() && st.attempt !== "running") leaveFullscreen();
    })
    .catch(() => {});
}

export function leaveFullscreen() {
  navigator.keyboard?.unlock?.();
  if (document.fullscreenElement) document.exitFullscreen?.().catch(() => {});
}

function ruleBroken(cause) {
  if (st.attempt === "starting") st.startViolation ??= cause;
  host.endAttempt("invalid", cause, 0);
}

/// Called once by the page.
export function watchRules() {
  $("fullscreen-return-button").onclick = returnToFullscreen;
  document.addEventListener("fullscreenchange", () => {
    if (document.fullscreenElement) fullscreenReturned();
    else fullscreenLeft();
  });
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) ruleBroken("visibility");
  });
}
