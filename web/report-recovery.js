export const reportRecoveryLimits = Object.freeze({
  expiresInSeconds: 300,
  retryAfterSeconds: 30,
  quotaRetryAfterSeconds: 60,
  retryWaitSeconds: 145,
  // The agent announces the end of its window. This page's clock starts when
  // the offer arrives, later than the agent's, so it waits a little past it
  // for that word and falls back on its own only when the word never comes.
  closeGraceSeconds: 15,
  retryStatuses: Object.freeze(["accepted", "early", "closed"]),
  // Why the report failed: `schema` when it answered and was refused, so the
  // page does not call that a passing outage.
  causes: Object.freeze(["schema", "unavailable"]),
});

// The provisional failure is finalized once, whether the candidate retries,
// the window closes, or the agent leaves. A retry never rewrites history.
//
// `keep` saves the provisional failure the moment it arrives. Holding it only
// in this tab for the window lost it to a crash, a killed browser or a
// suspended tab; the final outcome is saved under the same id over it.
//
// A retry is counted from the agent's answer, not from the click. A request
// queued behind a reconnect starts the generation late, and a page timing it
// from the click gave up on a report the agent was still writing.
export function createReportRecovery({
  send,
  offer,
  waiting,
  finalize,
  keep = () => {},
  timers = globalThis,
}) {
  let report = null;
  // One window per interview: a duplicate or replayed offer cannot reopen it.
  let opened = false;
  // The offer ran out while a retry was in flight. The agent's window is over
  // too, so an `early` answer arriving this late cannot reopen it.
  let expired = false;
  let ready = false;
  let cause = "unavailable";
  // "idle", "sent" while the request awaits its answer, "accepted" once the
  // agent is generating.
  let retry = "idle";
  let expiry = 0;
  let readyTimer = 0;
  let wait = 0;
  function clear() {
    for (const handle of [expiry, readyTimer, wait])
      if (handle) timers.clearTimeout(handle);
    expiry = readyTimer = wait = 0;
  }
  function finish() {
    if (!report) return;
    const final = report;
    stop();
    finalize(final);
  }
  function stop() {
    clear();
    report = null;
    ready = false;
  }
  // The agent gives regeneration its own deadline, 125 seconds against Gemini
  // and longer against a local model, and the server says which in
  // /runtime-config.js. The report it produces may take every delivery
  // attempt to arrive. The limit here is a floor the server can only raise.
  function waitForReport() {
    if (wait) timers.clearTimeout(wait);
    const seconds = Math.max(
      reportRecoveryLimits.retryWaitSeconds,
      Number(globalThis.CODETRIAL_REPORT_RETRY_WAIT_SECONDS) || 0,
    );
    wait = timers.setTimeout(finish, seconds * 1000);
  }
  function offerAfter(seconds) {
    ready = false;
    offer(false, seconds, cause);
    if (readyTimer) timers.clearTimeout(readyTimer);
    readyTimer = timers.setTimeout(() => {
      readyTimer = 0;
      ready = true;
      offer(true, seconds, cause);
    }, seconds * 1000);
  }
  return {
    start(raw) {
      const recovery = raw?.reportRecovery;
      if (!raw?.incomplete || !recovery || opened) return false;
      const expires = recovery.expiresInSeconds;
      const cooldown = recovery.retryAfterSeconds;
      if (
        !Number.isInteger(expires) ||
        expires <= 0 ||
        expires > reportRecoveryLimits.expiresInSeconds ||
        !Number.isInteger(cooldown) ||
        cooldown < reportRecoveryLimits.retryAfterSeconds ||
        cooldown >= expires
      )
        return false;
      opened = true;
      // Untrusted like the rest of the packet: anything outside the list is
      // the ordinary wording.
      cause = reportRecoveryLimits.causes.includes(recovery.cause)
        ? recovery.cause
        : "unavailable";
      report = { ...raw };
      delete report.reportRecovery;
      keep(report);
      offerAfter(cooldown);
      // A retry in flight is timed by its own wait instead, so the offer
      // running out cannot cut off a regeneration whose answer was lost.
      expiry = timers.setTimeout(
        () => {
          expiry = 0;
          expired = true;
          if (retry === "idle") finish();
        },
        (expires + reportRecoveryLimits.closeGraceSeconds) * 1000,
      );
      return true;
    },
    retry() {
      if (!report || !ready || retry !== "idle") return false;
      retry = "sent";
      ready = false;
      // From the click as well as from the answer, so a request whose answer
      // was lost is still bounded.
      waitForReport();
      waiting();
      send();
      return true;
    },
    // The agent's answer to a retry, or its word that the window closed.
    // Anything else, or an answer to a request this page never sent, changes
    // nothing.
    notice(message) {
      if (!report || message?.type !== "report_retry") return false;
      if (message.status === "closed") {
        if (retry === "accepted") return false;
        finish();
        return true;
      }
      if (retry !== "sent") return false;
      if (message.status === "accepted") {
        retry = "accepted";
        waitForReport();
        return true;
      }
      const after = message.retryAfterSeconds;
      if (
        message.status === "early" &&
        Number.isInteger(after) &&
        after > 0 &&
        after <= reportRecoveryLimits.expiresInSeconds
      ) {
        if (expired) {
          finish();
          return true;
        }
        // Not spent: the agent did not start, so the turn is still ours.
        retry = "idle";
        if (wait) timers.clearTimeout(wait);
        wait = 0;
        offerAfter(after);
        return true;
      }
      return false;
    },
    finish,
    stop,
  };
}
