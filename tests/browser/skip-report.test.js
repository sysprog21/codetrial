import { test } from "node:test";
import assert from "node:assert/strict";
import { functionBody, loadInterview, read } from "./source.js";

const page = read("web/interview.html");
const script = read("web/interview.js");

test("skip action is present on the ending overlay", () => {
  assert.match(page, /id="skip-report"/);
  assert.match(
    script,
    /nodes\.skipReport\.addEventListener\("click", requestSkipReport\)/,
  );
});

test("skip works during ending and report recovery", () => {
  const skip = functionBody(script, "skipReportAndExit");

  const request = functionBody(script, "requestSkipReport");
  const confirm = functionBody(script, "confirmSkipReport");
  const cancel = functionBody(script, "cancelSkipReport");

  assert.match(skip, /"ending"/);
  assert.match(skip, /"report_recovery"/);
  assert.doesNotMatch(skip, /window\.confirm\(/);
  assert.match(request, /state\.skipConfirmationPending = true/);
  assert.match(confirm, /await skipReportAndExit\(\)/);
  assert.match(cancel, /dismissSkipConfirmation\(\)/);
  const dismiss = functionBody(script, "dismissSkipConfirmation");
  assert.match(dismiss, /state\.skipConfirmationPending = false/);
  assert.match(dismiss, /nodes\.confirmSkipReport\.hidden = true/);
  assert.match(dismiss, /nodes\.cancelSkipReport\.hidden = true/);
  assert.match(page, /id="confirm-skip-report"/);
  assert.match(page, /id="cancel-skip-report"/);
});

test("an arriving agent report withdraws pending Skip confirmation", async () => {
  const connect = functionBody(script, "connectLiveKit");
  const accepted = connect.indexOf(
    "if (!acceptsReport(topic, participant)) return;",
  );
  const dismissed = connect.indexOf("dismissSkipConfirmation();", accepted);
  const claim = connect.indexOf(
    "const first = claimReport(payload, receivedReports);",
    accepted,
  );
  assert.ok(accepted >= 0 && dismissed > accepted && claim > dismissed);

  const state = {
    phase: "ending",
    skipConfirmationPending: true,
  };
  const nodes = {
    confirmSkipReport: { hidden: false },
    cancelSkipReport: { hidden: false },
  };
  let skipped = false;
  const { dismissSkipConfirmation, confirmSkipReport } = loadInterview(
    ["dismissSkipConfirmation", "confirmSkipReport"],
    {
      state,
      nodes,
      skipReportAndExit: async () => {
        skipped = true;
      },
    },
  );

  dismissSkipConfirmation();
  state.phase = "report";
  await confirmSkipReport();

  assert.equal(state.skipConfirmationPending, false);
  assert.equal(nodes.confirmSkipReport.hidden, true);
  assert.equal(nodes.cancelSkipReport.hidden, true);
  assert.equal(skipped, false);
});

test("normal report phase does not suppress end_interview publication", () => {
  const end = functionBody(script, "endInterview");
  assert.match(end, /const publishEnd = \(\) => \{/);
  assert.match(end, /if \(state\.phase === "skipped"\) return;/);
  assert.doesNotMatch(end, /if \(state\.phase !== "ending"\) return;/);
  assert.match(
    end,
    /endInterviewPayload\(reason, currentCode\(\), state\.language\)/,
  );
});

test("confirmed Skip updates the ending overlay", () => {
  const skip = functionBody(script, "skipReportAndExit");

  const confirmed = skip.indexOf('state.phase = "skipped"');
  const title = skip.indexOf('"Skipping report..."', confirmed);
  const spinner = skip.indexOf("spinner.hidden = true", confirmed);
  const cleanup = skip.indexOf("await finishSkippedReportCleanup()", confirmed);

  assert.ok(confirmed >= 0);
  assert.ok(title > confirmed);
  assert.ok(spinner > confirmed);
  assert.ok(cleanup > spinner);
  assert.match(skip, /nodes\.endingDetail\.textContent\s*=/);
});

test("confirmed skip cancels report processing", () => {
  const skip = functionBody(script, "skipReportAndExit");

  assert.match(skip, /sendData\(topics\.control,\s*skipReportPayload\(\)\)/);
  assert.match(skip, /reportRecovery\.stop\(\)/);
  assert.match(skip, /state\.phase = "skipped"/);
});

test("skip cleans up this report before leaving", () => {
  const skip = functionBody(script, "skipReportAndExit");
  const cleanup = functionBody(script, "finishSkippedReportCleanup");

  assert.match(skip, /await finishSkippedReportCleanup\(\)/);

  const waitAt = cleanup.indexOf("Promise.allSettled(");
  const deleteAt = cleanup.indexOf("deleteReport(state.reportId, {");
  const leaveAt = cleanup.lastIndexOf('window.location.href = "/"');

  assert.ok(waitAt >= 0, "wait for pending report saves");
  assert.ok(deleteAt > waitAt, "delete after pending saves settle");
  assert.ok(leaveAt > deleteAt, "normal exit follows cleanup");
  assert.match(
    cleanup,
    /state\.savedToAccount\s*\|\|\s*state\.possiblySavedToAccount/,
  );
  assert.match(
    cleanup,
    /if \(state\.savedToAccount && state\.savedReportOwnerConflict\)/,
  );
  assert.doesNotMatch(cleanup, /window\.confirm\(/);
  assert.match(cleanup, /Report copies under different accounts remain/);

  assert.match(cleanup, /state\.skipCleanupPending = true/);
  assert.match(cleanup, /state\.skipCleanupPending = false/);
  assert.doesNotMatch(cleanup, /clearReportHistory\(/);
  assert.match(
    cleanup,
    /state\.reportId\s*&&\s*\(!state\.isGuest\s*\|\|\s*accountMayExist\s*\|\|\s*state\.skipAccountDeleted\)/,
  );
});

test("render failure hides Skip before restoring the ending overlay", () => {
  const failed = functionBody(script, "reportRenderFailed");

  for (const expected of [
    'state.phase = "ending"',
    "nodes.skipReport.hidden = true",
    "nodes.ending.hidden = false",
    "nodes.leaveRoom.hidden = false",
    "nodes.forceReport.hidden = !offlineSummary",
  ]) {
    assert.ok(failed.includes(expected), `Missing: ${expected}`);
  }

  assert.ok(
    failed.indexOf("nodes.skipReport.hidden = true") <
      failed.indexOf("nodes.ending.hidden = false"),
    "Skip must be hidden before the ending overlay is shown",
  );
});

test("skip blocks late report saves", () => {
  const save = functionBody(script, "saveHistory");

  assert.match(save, /state\.phase === ['"]skipped['"]/);
  assert.match(save, /saveHistory\.pending/);
  assert.match(
    save,
    /saveReportHistory\(entry,\s*\{\s*persistAccount:\s*!state\.isGuest,\s*onAccountSaveAttempt:\s*\(\)\s*=>\s*\{\s*state\.possiblySavedToAccount\s*=\s*true;\s*\},\s*onAccountSaved:\s*\(ownerId\)\s*=>/,
  );
});

test("skip stops recovery timers before the first await", () => {
  const skip = functionBody(script, "skipReportAndExit");

  const phaseAt = skip.indexOf('state.phase = "skipped"');
  const recoveryAt = skip.indexOf("reportRecovery.stop()");
  const escapeAt = skip.indexOf("stopEndingEscape()");
  const clockAt = skip.indexOf("stopEndingClock()");
  const firstAwaitAt = skip.indexOf("await ", phaseAt);

  assert.ok(phaseAt >= 0);
  assert.ok(recoveryAt > phaseAt && recoveryAt < firstAwaitAt);
  assert.ok(escapeAt > phaseAt && escapeAt < firstAwaitAt);
  assert.ok(clockAt > phaseAt && clockAt < firstAwaitAt);
});

test("skip bounds control publication before disconnecting", () => {
  const skip = functionBody(script, "skipReportAndExit");

  assert.match(skip, /await Promise\.race\(\[/);
  assert.match(skip, /sendData\(topics\.control,\s*skipReportPayload\(\)\)/);
  assert.match(skip, /setTimeout\(/);
  assert.match(skip, /5000/);

  const publishAt = skip.indexOf("await Promise.race(");
  const disconnectAt = skip.indexOf("room?.disconnect");

  assert.ok(publishAt >= 0);
  assert.ok(
    disconnectAt > publishAt,
    "control publication must settle or time out before disconnecting",
  );
});

test("offline summary enters report phase before rendering", () => {
  const showReport = functionBody(script, "showReport");

  assert.match(showReport, /state\.phase = "report"/);
  assert.ok(
    showReport.indexOf('state.phase = "report"') <
      showReport.indexOf("const saving = saveHistory()"),
    "offline summary must enter report phase before saving",
  );
});

test("skip retries cannot start cleanup before room detachment", () => {
  const skip = functionBody(script, "skipReportAndExit");
  const leave = functionBody(script, "leaveRoom");

  assert.match(skip, /if \(state\.phase === "skipped"\)/);
  assert.match(
    skip,
    /if \(!state\.room\)\s*\{\s*await finishSkippedReportCleanup\(\)/,
  );
  assert.match(skip, /state\.skipPublishPending = true/);
  assert.match(
    skip,
    /await sendData\(topics\.control, skipReportPayload\(\)\)/,
  );
  assert.match(skip, /await finishSkippedReportCleanup\(\)/);

  const detachAt = skip.indexOf("state.room = null");
  const normalCleanupAt = skip.lastIndexOf(
    "await finishSkippedReportCleanup()",
  );

  assert.ok(detachAt >= 0);
  assert.ok(normalCleanupAt > detachAt);
  assert.match(leave, /if \(state\.phase === "skipped"\) return/);
});

test("failed Skip cleanup exposes a retry action", () => {
  const cleanup = functionBody(script, "finishSkippedReportCleanup");
  const skip = functionBody(script, "skipReportAndExit");

  assert.match(cleanup, /catch \(error\)/);
  assert.match(cleanup, /nodes\.ending\.hidden = false/);
  assert.match(cleanup, /nodes\.skipReport\.hidden = false/);
  assert.match(
    cleanup,
    /nodes\.skipReport\.textContent = "Retry cleanup and exit"/,
  );

  assert.match(skip, /if \(state\.phase === "skipped"\)/);
  assert.match(skip, /await finishSkippedReportCleanup\(\)/);
  assert.match(cleanup, /state\.skipCleanupPending = false/);
});

test("skip cleanup preserves account deletion requirements", () => {
  const save = functionBody(script, "saveHistory");
  const cleanup = functionBody(script, "finishSkippedReportCleanup");

  assert.match(save, /result\?\.account === "saved"/);
  assert.match(save, /state\.savedToAccount = true/);
  assert.match(
    cleanup,
    /state\.reportId\s*&&\s*\(!state\.isGuest\s*\|\|\s*accountMayExist\s*\|\|\s*state\.skipAccountDeleted\)/,
  );
  assert.match(cleanup, /account: accountMayExist/);
  assert.match(cleanup, /requireAccountDeletion: accountMayExist/);
  assert.match(cleanup, /localOnly: true/);
});
