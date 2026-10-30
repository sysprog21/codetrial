import { LANGUAGE_KEY } from "../../web/lib.js";
import { PRACTICE_LIST_KEY } from "../../web/practice-list.js";
import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  STRICT_ASSIGNMENT_KEY,
  previousAssignment,
  rememberAssignment,
  resetAssignments,
} from "../../web/selection.js";
import { grant, openInterview, strictFailure } from "./interview-fixture.js";
import {
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  memoryStorage,
  pauseClock,
  read,
  startStaticServer,
} from "./source.js";

test("selection receipts stay tab-local and storage failure is not a new sequence", () => {
  const first = memoryStorage();
  const second = memoryStorage();
  assert.equal(previousAssignment(first), null);
  rememberAssignment(first, "signed-first");
  assert.equal(previousAssignment(first), "signed-first");
  assert.equal(previousAssignment(second), null);
  rememberAssignment(first, "signed-second");
  assert.equal(previousAssignment(first), "signed-second");
  resetAssignments(first);
  assert.equal(previousAssignment(first), null);
  assert.throws(() => previousAssignment(null), /session storage/);
  assert.throws(
    () =>
      previousAssignment({
        getItem: () => "previous",
        setItem: () => {
          throw new Error("quota");
        },
      }),
    /session storage/,
  );
  assert.throws(() => rememberAssignment(second, ""), /invalid/);
  assert.throws(
    () =>
      rememberAssignment(
        {
          setItem: () => {},
          getItem: () => null,
        },
        "receipt",
      ),
    /session storage/,
  );
});

let browser;
let server;
let base;
before(async () => {
  browser = await launchChromium();
  if (browser)
    ({ server, base } = await startStaticServer({
      runtimeConfig: DEFAULT_RUNTIME_CONFIG,
    }));
});
after(async () => {
  await browser?.close();
  await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
});

async function interview(
  t,
  query,
  handler,
  context = browser,
  session = { signedIn: true, loginRequired: true },
) {
  return openInterview({ browser, base }, t, query, handler, context, session);
}

async function consentContext(t) {
  const context = await browser.newContext();
  t.after(() => context.close());
  const consents = [];
  await context.exposeBinding("noteConsent", () => consents.push("consent"));
  await context.route("**/replay-feed.js", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body: read("web/replay-feed.js").replace(
        "export async function recordConsent() {",
        "export async function recordConsent() { await window.noteConsent();",
      ),
    }),
  );
  return { context, consents };
}

/// With `pause`, the page's clock stops once Start is ready and before it is
/// pressed, so no timer the start sets can expire on its own.
async function preflight(page, { pause = false } = {}) {
  await page.locator("#audio-heard").click();
  await page.locator("#camera-skip").click();
  try {
    if (pause) {
      await page
        .locator("#audio-check-join:not([disabled])")
        .waitFor({ timeout: 10000 });
      await pauseClock(page);
    }
    await page.locator("#audio-check-join").click({ timeout: 10000 });
  } catch (error) {
    throw new Error(await page.locator("#audio-check-status").textContent(), {
      cause: error,
    });
  }
}

for (const [requested, expected] of [
  [null, "python"],
  ["javascript", "javascript"],
  ["unsupported", "python"],
]) {
  test(`Strict restores ${requested ?? "the default"} language for its server-assigned problem`, async (t) => {
    const query = new URLSearchParams({ selection: "strict" });
    if (requested) query.set("language", requested);
    const page = await interview(t, query, (route) =>
      route.fulfill({
        json: grant("chargeback-pair-match", "language-assignment"),
      }),
    );
    if (!page) return;
    await preflight(page);
    await page.locator("#editor:not([disabled])").waitFor();
    assert.equal(await page.locator("#language-select").inputValue(), expected);
    assert.match(
      await page.locator("#editor").inputValue(),
      expected === "python"
        ? /def matchDisputedCharge/
        : /function matchDisputedCharge/,
    );
  });
}

/// A context with compiled runs on, so C is withheld only for want of a judge,
/// and judges held until `release` is called.
async function heldJudgeContext(t) {
  let release;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  t.after(() => release());
  const context = await browser.newContext();
  t.after(() => context.close());
  await context.route("**/runtime-config.js", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body: DEFAULT_RUNTIME_CONFIG.replace(
        "ENABLED = false",
        "ENABLED = true",
      ).replace('BASE_URL = ""', 'BASE_URL = "https://godbolt.org"'),
    }),
  );
  await context.route("**/judges/*.json", async (route) => {
    await held;
    await route.continue();
  });
  return { context, release };
}

test("Strict keeps the token's language when the assigned judge lands late", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { context, release } = await heldJudgeContext(t);
  const page = await interview(
    t,
    "selection=strict&language=c",
    (route) =>
      route.fulfill({
        json: grant("chargeback-pair-match", "delayed-language-assignment"),
      }),
    context,
  );
  await preflight(page);
  await page.locator("#editor:not([disabled])").waitFor();
  assert.equal(await page.locator("#language-select").inputValue(), "python");
  const code = await page.locator("#editor").inputValue();
  // The token named Python, so a judge enabling C afterwards does not switch
  // to it even though the candidate only waited.
  release();
  await page.waitForFunction(
    () =>
      !document.querySelector('#language-select option[value="c"]').disabled,
  );
  assert.equal(await page.locator("#language-select").inputValue(), "python");
  assert.equal(await page.locator("#editor").inputValue(), code);
});

// A Practice start that prefers C waits a bounded turn for the judge before
// asking for a token, with the workspace already open. A judge landing in
// that turn switches to C only if the candidate has not committed to a tab.
for (const action of ["wait", "edit", "choose", "run", "end"]) {
  test(`Practice judge landing during the C wait respects ${action}`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { context, release } = await heldJudgeContext(t);
    await context.clock.install();
    const page = await interview(
      t,
      "problem=chargeback-pair-match&language=c",
      (route) =>
        route.fulfill({
          json: grant("chargeback-pair-match", "practice-c-wait"),
        }),
      context,
    );
    let tokens = 0;
    page.on("request", (request) => {
      if (new URL(request.url()).pathname === "/api/token") tokens++;
    });
    assert.equal(await page.locator("#language-select").inputValue(), "python");
    // Paused before Start, the bounded wait cannot expire before the judge is
    // released, however slow the runner.
    await preflight(page, { pause: true });
    await page.locator("#audio-check").waitFor({ state: "hidden" });
    let expected = action === "wait" ? "c" : "python";
    let code = null;
    if (action === "edit") {
      code = "# Keep this candidate edit.\n";
      await page.locator("#editor").fill(code);
    } else if (action === "choose") {
      expected = "javascript";
      await page.locator("#language-select").selectOption(expected);
      code = await page.locator("#editor").inputValue();
    } else if (action === "run") {
      await page.locator("#run-tests").click();
      assert.equal(
        await page.locator("#run-tests").textContent(),
        "Running...",
      );
    } else if (action === "end") {
      await page.locator("#end").click();
      await page.locator("#ending-overlay:not([hidden])").waitFor();
    }
    assert.equal(tokens, 0, "still inside the C wait");
    const token = action === "end" ? null : page.waitForRequest("**/api/token");
    release();
    await page.waitForFunction(
      () =>
        !document.querySelector('#language-select option[value="c"]').disabled,
    );
    await page.waitForFunction(
      (language) =>
        document.querySelector("#language-select").value === language,
      expected,
    );
    if (code !== null)
      assert.equal(await page.locator("#editor").inputValue(), code);
    if (action === "wait")
      assert.match(
        await page.locator("#editor").inputValue(),
        /matchDisputedCharge\(/,
      );
    if (token) assert.equal((await token).postDataJSON().language, expected);
    else {
      assert.equal(await page.locator("#ending-overlay").isVisible(), true);
      // The wait is over once C is enabled; an ended interview asks for no
      // token, so it gets no room and no interviewer.
      await page.evaluate(
        () => new Promise((resolve) => queueMicrotask(resolve)),
      );
      assert.equal(tokens, 0, "an ended interview requests no token");
      assert.equal(await page.evaluate(() => window.roomJoins.length), 0);
      // With no room, nothing later stops the preflight's devices, so ending
      // in the wait must release them itself.
      const states = await page.evaluate(() =>
        window.acquiredTracks.map((track) => track.readyState),
      );
      assert.ok(states.length > 0, "the preflight acquired a microphone");
      assert.deepEqual([...new Set(states)], ["ended"]);
    }
  });
}

// A start that fails after the interview has already ended offers no
// fallback under that ending. Practice is ended by the candidate while its
// token is held, and the token is then refused. Strict keeps End disabled
// until it has started, so its interview ends through a report that arrives
// during the join, after which the track publish fails.
for (const selection of ["practice", "strict"]) {
  test(`${selection === "strict" ? "Strict" : "Practice"} offers no fallback when its start fails after the interview ended`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    let respond;
    const responded = new Promise((resolve) => {
      respond = resolve;
    });
    t.after(() => respond());
    // Paused before Start: with no interviewer, the end becomes the offline
    // summary 300 ms later and takes the ending overlay down, which a slow
    // runner would otherwise reach before this case looks for it.
    const context = await browser.newContext();
    t.after(() => context.close());
    await context.clock.install();
    const strict = selection === "strict";
    const page = await interview(
      t,
      strict ? "selection=strict" : "problem=chargeback-pair-match",
      async (route) => {
        if (strict)
          return route.fulfill({
            json: grant("chargeback-pair-match", "failed-after-end"),
          });
        await responded;
        return route.fulfill({ status: 503, json: { error: "No provider" } });
      },
      context,
    );
    if (strict) {
      await page.evaluate(() => {
        window.roomTrackHeld = new Promise((resolve, reject) => {
          window.failTracks = () => reject(new Error("publish failed"));
        });
        window.roomTrackHeld.catch(() => {});
      });
      await preflight(page, { pause: true });
      await page.waitForFunction(() => window.roomJoins.length === 1);
      await deliverReport(page, { summary: "Ended by the interviewer." });
      await page.locator("#report-modal").waitFor({ state: "visible" });
      await page.evaluate(() => window.failTracks());
    } else {
      const requested = page.waitForRequest("**/api/token");
      await preflight(page, { pause: true });
      await requested;
      await page.locator("#end").click();
      await page.locator("#ending-overlay:not([hidden])").waitFor();
      respond();
    }
    await tracksReleased(page);
    // The failure has been handled once the tracks are released, and the
    // fallback would have been written in the same turn.
    await page.evaluate(
      () => new Promise((resolve) => queueMicrotask(resolve)),
    );
    assert.doesNotMatch(
      await page.locator("#transcript-panel").textContent(),
      /Offline (?:mode|practice)/,
    );
    assert.equal(await page.locator("#audio-check").isHidden(), true);
    // The failed publish does not take the room from the report, whose
    // receipt the paused clock still holds.
    if (strict)
      assert.equal(await page.evaluate(() => window.roomLeaves ?? 0), 0);
  });
}

// Past the C wait, a start still awaits the token response and then the room
// join, with the workspace open for both. Ending in either leaves no
// interview behind: no room, or one left at once, and no live device.
for (const stage of ["token", "join"]) {
  test(`Practice ended while its start awaits the ${stage} leaves no room and releases devices`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    let respond;
    const responded = new Promise((resolve) => {
      respond = resolve;
    });
    t.after(() => respond());
    // The ending's own timers lead to the offline summary, whose cleanup
    // would release the devices too. A paused clock leaves only the start
    // that noticed the end to do it.
    const context = await browser.newContext();
    t.after(() => context.close());
    await context.clock.install();
    const page = await interview(
      t,
      "problem=chargeback-pair-match",
      async (route) => {
        if (stage === "token") await responded;
        await route.fulfill({
          json: grant("chargeback-pair-match", `ended-at-${stage}`),
        });
      },
      context,
    );
    if (stage === "join")
      await page.evaluate(() => {
        window.roomJoinHeld = new Promise((resolve) => {
          window.releaseJoin = resolve;
        });
      });
    const requested = page.waitForRequest("**/api/token");
    await preflight(page, { pause: true });
    await requested;
    if (stage === "join")
      await page.waitForFunction(() => window.roomJoins.length === 1);
    await page.locator("#end").click();
    await page.locator("#ending-overlay:not([hidden])").waitFor();
    if (stage === "token") respond();
    else await page.evaluate(() => window.releaseJoin());
    // The start ends itself once its await returns. A page that carried on
    // would keep a microphone live and never get here.
    await page.waitForFunction(
      () =>
        window.acquiredTracks.length > 0 &&
        window.acquiredTracks.every((track) => track.readyState === "ended"),
      null,
      { timeout: 5_000 },
    );
    assert.deepEqual(
      await page.evaluate(() => ({
        joins: window.roomJoins.length,
        leaves: window.roomLeaves ?? 0,
      })),
      stage === "token" ? { joins: 0, leaves: 0 } : { joins: 1, leaves: 1 },
    );
  });
}

// On a running clock an end with no interviewer becomes the offline summary
// within a moment, so a slow join finds the page in `report` rather than
// `ending`. That summary is no report from this room, and the join still
// gives the room up.
test("a join that outlasts the offline summary still leaves its room and releases devices", async (t) => {
  const page = await interview(t, "problem=chargeback-pair-match", (route) =>
    route.fulfill({
      json: grant("chargeback-pair-match", "join-after-summary"),
    }),
  );
  if (!page) return;
  await page.evaluate(() => {
    window.roomJoinHeld = new Promise((resolve) => {
      window.releaseJoin = resolve;
    });
  });
  await preflight(page);
  await page.waitForFunction(() => window.roomJoins.length === 1);
  await page.locator("#end").click();
  await page.locator("#report-modal").waitFor({ state: "visible" });
  await page.evaluate(() => window.releaseJoin());
  await page.waitForFunction(
    () =>
      window.acquiredTracks.length > 0 &&
      window.acquiredTracks.every((track) => track.readyState === "ended"),
    null,
    { timeout: 5_000 },
  );
  assert.deepEqual(
    await page.evaluate(() => ({
      leaves: window.roomLeaves ?? 0,
      claimed: (window.roomPublishes ?? []).includes("integrity"),
    })),
    { leaves: 1, claimed: false },
    "the room is left before it is claimed",
  );
});

/// Hands `report` to the room's report handlers as the interviewer would.
/// Not awaited: delivery waits on timers a paused clock may hold back.
async function deliverReport(page, report) {
  await page.evaluate((body) => {
    const payload = new TextEncoder().encode(JSON.stringify(body));
    for (const handler of window.roomEvents.DataReceived)
      void handler(payload, { kind: "AGENT" }, null, "report");
  }, report);
}

/// Waits for every track the preflight acquired to end.
async function tracksReleased(page) {
  await page.waitForFunction(
    () =>
      window.acquiredTracks.length > 0 &&
      window.acquiredTracks.every((track) => track.readyState === "ended"),
    null,
    { timeout: 5_000 },
  );
}

/// Holds the join's track publishes until `window.releaseTracks()`, which is
/// after the room has connected and before it is claimed.
async function holdTrackPublishes(page) {
  await page.evaluate(() => {
    window.roomTrackHeld = new Promise((resolve) => {
      window.releaseTracks = resolve;
    });
  });
}

// A report that arrives during the join is using its room, for a receipt or
// a retry, so the join keeps it: a report being received leaves it itself,
// and recovery is handed it for the retry. The interview behind that report
// is over, and the join starts nothing for it: the tracks end, and no
// integrity event, which goes out only from a claimed stream, is published.
async function reportDuringJoin(t, report) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  // Paused, so neither the receipt's bound nor the recovery's timers move
  // the page on while it is checked.
  const context = await browser.newContext();
  t.after(() => context.close());
  await context.clock.install();
  const page = await interview(
    t,
    "problem=chargeback-pair-match",
    (route) =>
      route.fulfill({
        json: grant("chargeback-pair-match", "report-during-join"),
      }),
    context,
  );
  await holdTrackPublishes(page);
  await page.evaluate(() => {
    window.roomPublishHeld = new Promise((resolve) => {
      window.releasePublishes = resolve;
    });
  });
  await preflight(page, { pause: true });
  await page.waitForFunction(() => window.roomJoins.length === 1);
  await deliverReport(page, report);
  // The receipt is on its way and held.
  await page.waitForFunction(() => window.roomPublishes?.includes("control"));
  await page.evaluate(() => window.releaseTracks());
  await tracksReleased(page);
  assert.deepEqual(
    await page.evaluate(() => ({
      leaves: window.roomLeaves ?? 0,
      started: window.roomPublishes.includes("integrity"),
    })),
    { leaves: 0, started: false },
    "the room stays for the report, and nothing starts behind it",
  );
  return page;
}

// A complete report is still being received until its receipt has gone out,
// and leaving the room then could drop that receipt. The delivered report
// leaves the room itself afterwards.
test("a report still being received when the join finishes keeps the room for its receipt", async (t) => {
  const page = await reportDuringJoin(t, {
    summary: "Delivered while the join was finishing.",
  });
  if (!page) return;
  await page.evaluate(() => window.releasePublishes());
  // Past the receipt's 250 ms to leave.
  await page.clock.runFor(1_000);
  await page.waitForFunction(() => (window.roomLeaves ?? 0) === 1, null, {
    timeout: 5_000,
  });
});

// The receipt goes out over the data channel and need not wait for the join.
// Once it is out, `receiveReport` has left this room, while the report stays
// claimed until its history save settles. A join finishing then must not put
// the left room back, or leaving the report would disconnect it again.
test("a join finishing after a report's receipt went out does not take the room back", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const context = await browser.newContext();
  t.after(() => context.close());
  await context.clock.install();
  let save;
  const saved = new Promise((resolve) => {
    save = resolve;
  });
  t.after(() => save());
  await context.route("**/api/reports", async (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    await saved;
    await route.fulfill({ json: {} });
  });
  await context.route(`${base}/`, (route) =>
    route.fulfill({ contentType: "text/html", body: "<p>Lobby</p>" }),
  );
  const page = await interview(
    t,
    "problem=chargeback-pair-match",
    (route) =>
      route.fulfill({
        json: grant("chargeback-pair-match", "receipt-before-join"),
      }),
    context,
  );
  await holdTrackPublishes(page);
  await preflight(page, { pause: true });
  await page.waitForFunction(() => window.roomJoins.length === 1);
  await deliverReport(page, { summary: "Delivered before the join finished." });
  await page.locator("#report-modal").waitFor({ state: "visible" });
  // The receipt is hashed before it is published. Wait for the publish, so
  // the clock below moves the receipt on rather than running out its bound.
  await page.waitForFunction(() => window.roomPublishes?.includes("control"));
  // Past the receipt's 250 ms to leave: `receiveReport` then leaves the room.
  await page.clock.runFor(1_000);
  await page.waitForFunction(() => (window.roomLeaves ?? 0) === 1, null, {
    timeout: 5_000,
  });
  // Leaving navigates away, so its disconnects are counted where they last.
  await page.evaluate(() => {
    const room = window.LivekitClient.Room.prototype;
    const leave = room.disconnect;
    room.disconnect = function disconnect() {
      const leaves = Number(sessionStorage.getItem("leftAfterReport") ?? 0);
      sessionStorage.setItem("leftAfterReport", String(leaves + 1));
      return leave.call(this);
    };
  });
  await page.evaluate(() => window.releaseTracks());
  await tracksReleased(page);
  save();
  await page.locator("#done:not([disabled])").waitFor();
  await page.locator("#done").click();
  await page.waitForURL(`${base}/`);
  assert.equal(
    await page.evaluate(() => sessionStorage.getItem("leftAfterReport")),
    null,
    "leaving finds no room to disconnect again",
  );
});

// Publication alone does not start the interview. End during an integrity
// write cancels startup without asking for evaluation or publishing more.
test("Practice End during startup integrity publication leaves without a report", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  // Paused, so no ending timer can move the page on while the start settles.
  const context = await browser.newContext();
  t.after(() => context.close());
  await context.clock.install();
  const page = await interview(
    t,
    "problem=chargeback-pair-match",
    (route) =>
      route.fulfill({
        json: grant("chargeback-pair-match", "ended-in-integrity"),
      }),
    context,
  );
  await page.evaluate(() => {
    window.roomPublishHeld = new Promise((resolve) => {
      window.releasePublishes = resolve;
    });
  });
  await preflight(page, { pause: true });
  await page.waitForFunction(() => window.roomPublishes?.includes("integrity"));
  await page.locator("#end").click();
  await page.locator("#ending-overlay:not([hidden])").waitFor();
  assert.equal(
    await page.evaluate(() => window.roomPublishes.includes("control")),
    false,
    "startup End sends no evaluation request",
  );
  await page.evaluate(() => window.releasePublishes());
  await tracksReleased(page);
  await page.evaluate(() => new Promise((resolve) => queueMicrotask(resolve)));
  assert.equal(
    await page.evaluate(() => window.roomLeaves ?? 0),
    1,
    "startup End leaves without a report",
  );
  assert.equal(
    await page.evaluate(() => window.roomPublishes.includes("code_update")),
    false,
    "the start does not go on to publish the editor",
  );
});

// A report can land while the join is still publishing those events. It has
// stopped the monitors by then, and recovery keeps the room for its retry,
// so a heartbeat restarted behind it would go on publishing to that room.
test("a report arriving while the join publishes integrity events starts no monitor behind it", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  // Paused, so the heartbeat only beats when the clock is moved below.
  const context = await browser.newContext();
  t.after(() => context.close());
  await context.clock.install();
  const page = await interview(
    t,
    "problem=chargeback-pair-match",
    (route) =>
      route.fulfill({
        json: grant("chargeback-pair-match", "report-in-integrity"),
      }),
    context,
  );
  await page.evaluate(() => {
    window.roomPublishHeld = new Promise((resolve) => {
      window.releasePublishes = resolve;
    });
  });
  await preflight(page, { pause: true });
  await page.waitForFunction(() => window.roomPublishes?.includes("integrity"));
  await deliverReport(page, {
    incomplete: true,
    summary: "503",
    reportRecovery: { expiresInSeconds: 300, retryAfterSeconds: 30 },
  });
  await page.waitForFunction(
    () =>
      document.querySelector("#ending-title").textContent ===
      "Evaluation temporarily unavailable",
    null,
    { timeout: 5_000 },
  );
  const integrity = () =>
    page.evaluate(
      () =>
        window.roomPublishes.filter((topic) => topic === "integrity").length,
    );
  const expected = await integrity();
  assert.equal(expected, 1, "only SESSION_START is held");
  await page.evaluate(() => window.releasePublishes());
  await page.waitForFunction(() =>
    window.roomPublicationSettled?.includes("integrity"),
  );
  // Two heartbeat periods, short of the recovery's 30 s retry.
  await page.clock.runFor(12_000);
  assert.equal(
    await integrity(),
    expected,
    "no later startup event or heartbeat behind the report",
  );
});

// The interviewer can end the session itself, so a report may arrive while
// the join is still publishing tracks. One that offers recovery needs this
// room for its retry.
test("a report arriving while the join publishes tracks keeps the room for recovery", async (t) => {
  const page = await reportDuringJoin(t, {
    incomplete: true,
    summary: "503",
    reportRecovery: { expiresInSeconds: 300, retryAfterSeconds: 30 },
  });
  if (!page) return;
  assert.equal(
    await page.locator("#ending-title").textContent(),
    "Evaluation temporarily unavailable",
  );
  // The retry goes out through the room the join handed over.
  const controls = () =>
    page.evaluate(
      () => window.roomPublishes.filter((topic) => topic === "control").length,
    );
  const before = await controls();
  await page.clock.runFor(31_000);
  await page.locator("#retry-report:not([hidden]):not([disabled])").click();
  await page.waitForFunction(
    (count) =>
      window.roomPublishes.filter((topic) => topic === "control").length >
      count,
    before,
    { timeout: 5_000 },
  );
});

// Startup End requests no evaluation. A late report cannot undo that exit;
// reports already being received or recovered have their own cases above.
test("Practice startup End ignores a late recovering report during the join", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  // Paused, so the summary's 300 ms runs only when the clock is moved below.
  const context = await browser.newContext();
  t.after(() => context.close());
  await context.clock.install();
  const page = await interview(
    t,
    "problem=chargeback-pair-match",
    (route) =>
      route.fulfill({
        json: grant("chargeback-pair-match", "recovery-over-summary"),
      }),
    context,
  );
  await holdTrackPublishes(page);
  await preflight(page, { pause: true });
  await page.waitForFunction(() => window.roomJoins.length === 1);
  await page.locator("#end").click();
  await page.locator("#ending-overlay:not([hidden])").waitFor();
  await deliverReport(page, {
    incomplete: true,
    summary: "503",
    reportRecovery: { expiresInSeconds: 300, retryAfterSeconds: 30 },
  });
  await page.clock.runFor(1_000);
  assert.equal(await page.locator("#report-modal").isVisible(), true);
  assert.equal(await page.locator("#retry-report").isHidden(), true);
  assert.match(
    await page.locator("#report-modal").textContent(),
    /Nothing you did was assessed/,
  );
  await page.evaluate(() => window.releaseTracks());
  await tracksReleased(page);
  assert.equal(
    await page.evaluate(() => window.roomLeaves ?? 0),
    1,
    "the canceled startup cannot reclaim a room for report recovery",
  );
});

// On a running clock the offline summary can be on screen before a slow join
// finishes, and a report the interviewer sends then is ignored. It holds
// nothing up: the join still leaves its room and starts nothing.
test("a report arriving after the offline summary does not keep a slow join's room", async (t) => {
  const page = await interview(t, "problem=chargeback-pair-match", (route) =>
    route.fulfill({
      json: grant("chargeback-pair-match", "report-after-summary"),
    }),
  );
  if (!page) return;
  await holdTrackPublishes(page);
  await preflight(page);
  await page.waitForFunction(() => window.roomJoins.length === 1);
  await page.locator("#end").click();
  await page.locator("#report-modal").waitFor({ state: "visible" });
  await deliverReport(page, { summary: "Too late for the summary." });
  await page.waitForFunction(() => window.roomPublishes?.includes("control"));
  await page.evaluate(() => window.releaseTracks());
  await tracksReleased(page);
  assert.deepEqual(
    await page.evaluate(() => ({
      leaves: window.roomLeaves ?? 0,
      started: (window.roomPublishes ?? []).includes("integrity"),
    })),
    { leaves: 1, started: false },
    "an ignored report claims nothing, so the room is still the join's to leave",
  );
});

/// A context whose interview page remembers `language` from an earlier start.
async function rememberingContext(t, language, runtimeConfig) {
  const context = await browser.newContext();
  t.after(() => context.close());
  if (language)
    await context.addInitScript(
      ([key, value]) => localStorage.setItem(key, value),
      [LANGUAGE_KEY, language],
    );
  if (runtimeConfig)
    await context.route("**/runtime-config.js", (route) =>
      route.fulfill({ contentType: "text/javascript", body: runtimeConfig }),
    );
  return context;
}

for (const [remembered, picked, expected] of [
  ["javascript", null, "javascript"],
  [null, "javascript", "javascript"],
  ["c", null, "python"],
]) {
  test(`Strict starts the assigned problem in ${expected} when ${remembered ?? "nothing"} is remembered and ${picked ?? "nothing"} is picked`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    // Compiled runs on, so C is withheld for want of a judge rather than by
    // the server.
    const context = await rememberingContext(
      t,
      remembered,
      DEFAULT_RUNTIME_CONFIG.replace(
        "ENABLED = false",
        "ENABLED = true",
      ).replace('BASE_URL = ""', 'BASE_URL = "https://godbolt.org"'),
    );
    let sent;
    const page = await interview(
      t,
      "selection=strict",
      (route) => {
        sent = route.request().postDataJSON();
        return route.fulfill({
          json: grant("chargeback-pair-match", "chosen-language"),
        });
      },
      context,
    );
    const c = page.locator('#preflight-language option[value="c"]');
    assert.equal(await c.isDisabled(), true);
    // No judge is coming before assignment, so the label names what decides
    // C rather than a wait or a reload that would change nothing.
    assert.equal(
      await c.textContent(),
      "C (unavailable: decided by assigned problem)",
    );
    assert.doesNotMatch(await c.getAttribute("title"), /reload|waiting/i);
    if (picked) await page.locator("#preflight-language").selectOption(picked);
    assert.equal(
      await page.locator("#preflight-language").inputValue(),
      expected,
    );
    await preflight(page);
    await page.locator("#editor:not([disabled])").waitFor();
    assert.equal(sent.language, expected);
    assert.equal(await page.locator("#language-select").inputValue(), expected);
    assert.match(
      await page.locator("#editor").inputValue(),
      expected === "python"
        ? /def matchDisputedCharge/
        : /function matchDisputedCharge/,
    );
  });
}

for (const loginRequired of [false, true]) {
  test(`Strict redirects guests to sign in when loginRequired is ${loginRequired}`, async (t) => {
    let requests = 0;
    const page = await interview(
      t,
      "selection=strict&problem=chargeback-pair-match",
      (route) => {
        requests++;
        return route.fulfill({ json: { error: "Unexpected token request" } });
      },
      browser,
      { signedIn: false, loginRequired, guestModeAvailable: true },
    );
    if (!page) return;
    await page.waitForURL(`${base}/`);
    assert.equal(requests, 0);
  });
}

test("Practice guests still join their specified problem after preflight", async (t) => {
  const requests = [];
  const page = await interview(
    t,
    "problem=kiosk-panel-mirror-audit",
    (route) => {
      requests.push(route.request().postDataJSON());
      return route.fulfill({
        json: {
          serverUrl: "wss://example.test",
          token: "guest-practice",
          roomName: "guest-practice",
          durationMin: 30,
          guest: true,
        },
      });
    },
    browser,
    { signedIn: false, loginRequired: true, guestModeAvailable: true },
  );
  if (!page) return;
  await preflight(page);
  await page.waitForFunction(() => window.roomJoins.length === 1);
  assert.equal(requests.length, 1);
  assert.equal(requests[0].problemId, "kiosk-panel-mirror-audit");
  assert.equal(requests[0].selectionMode, undefined);
  assert.equal(requests[0].previousAssignment, undefined);
});

test("Strict refresh keeps difficulty, ignores URL problem and joins a fresh assignment", async (t) => {
  const calls = [];
  const pages = ["chargeback-pair-match", "kiosk-panel-mirror-audit"];
  const page = await interview(
    t,
    "selection=strict&difficulty=Easy&problem=terrain-runoff-pooling&exampleDrawing=1",
    (route) => {
      calls.push(route.request().postDataJSON());
      return route.fulfill({
        json: grant(pages[calls.length - 1], `receipt-${calls.length}`),
      });
    },
  );
  if (!page) return;
  let warnings = 0;
  page.on("dialog", () => warnings++);
  assert.equal(calls.length, 0);
  assert.equal(await page.locator("#end").isDisabled(), true);
  assert.equal(
    await page.locator("#problem-title").textContent(),
    "Strict Interview",
  );
  await page.locator("#interviewer-voice").selectOption("Puck");
  await page.locator("#interviewer-accent").selectOption("British");
  await preflight(page);
  await page.waitForFunction(() => window.roomJoins.length === 1);
  await page.locator("#editor").fill("previous attempt code");
  const firstTitle = await page.locator("#problem-title").textContent();
  assert.equal(await page.locator("#end").isEnabled(), true);
  assert.equal(calls[0].selectionMode, "strict");
  assert.equal(calls[0].exampleDrawing, true);
  assert.equal(await page.locator("#example-board").isVisible(), true);
  assert.equal(calls[0].interviewerVoice, "Puck");
  assert.equal(calls[0].interviewerAccent, "British");
  assert.deepEqual(calls[0].difficulties, ["Easy"]);
  assert.equal(calls[0].codeExecution, undefined);
  assert.equal(calls[0].problemId, undefined);
  assert.equal(calls[0].previousAssignment, undefined);
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    "receipt-1",
  );
  // A transport reconnect reuses this attempt and does not reallocate.
  await page.evaluate(() => {
    for (const event of ["Reconnecting", "Reconnected"])
      for (const callback of window.roomEvents[event] ?? []) callback();
  });
  assert.equal(calls.length, 1);
  await page.reload();
  assert.equal(
    warnings,
    1,
    "the existing unsaved-interview warning still runs",
  );
  assert.equal(await page.locator("#audio-check").isVisible(), true);
  assert.equal(calls.length, 1);
  assert.equal(await page.locator("#editor").inputValue(), "");
  await page.locator("#interviewer-voice").selectOption("Random");
  await page.locator("#interviewer-accent").selectOption("Random");
  await preflight(page);
  await page.waitForFunction(() => window.roomJoins.length === 1);
  assert.notEqual(
    await page.locator("#problem-title").textContent(),
    firstTitle,
  );
  assert.equal(calls[1].exampleDrawing, true);
  assert.equal(await page.locator("#example-board").isVisible(), true);
  assert.equal(calls[1].interviewerVoice, "Random");
  assert.equal(calls[1].interviewerAccent, "Random");
  assert.deepEqual(calls[1].difficulties, ["Easy"]);
  assert.deepEqual(new URL(page.url()).searchParams.getAll("difficulty"), [
    "Easy",
  ]);
  assert.equal(calls[1].previousAssignment, "receipt-1");
  assert.equal(calls[1].problemId, undefined);
  assert.equal(
    await page.evaluate(() => window.roomJoins[0].token),
    "receipt-2",
  );
  assert.notEqual(
    await page.locator("#editor").inputValue(),
    "previous attempt code",
  );
});

test("Practice reopens its URL problem and keeps the existing offline fallback", async (t) => {
  const calls = [];
  const page = await interview(t, "problem=chargeback-pair-match", (route) => {
    calls.push(route.request().postDataJSON());
    return route.fulfill({ status: 503, json: { error: "No provider" } });
  });
  if (!page) return;
  for (let attempt = 0; attempt < 2; attempt++) {
    if (attempt) await page.reload();
    await preflight(page);
    await page.waitForFunction(() =>
      document
        .querySelector("#transcript-panel")
        .textContent.includes("Offline mode is ready"),
    );
    assert.equal(calls[attempt].problemId, "chargeback-pair-match");
    assert.equal(calls[attempt].selectionMode, undefined);
    assert.equal(calls[attempt].difficulties, undefined);
    assert.equal(await page.locator("#editor").isEnabled(), true);
    assert.equal(await page.locator("#run-tests").isEnabled(), true);
  }
});

test("a refused Strict assignment returns to the lobby without retrying or spending its receipt", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { context, consents } = await consentContext(t);
  const calls = [];
  const message = "Selection unavailable. Retry media preflight.";
  const query = "selection=strict&difficulty=Easy";
  const page = await interview(
    t,
    query,
    (route) => {
      const request = route.request().postDataJSON();
      calls.push(request);
      assert.deepEqual(request.difficulties, ["Easy"]);
      assert.equal(request.previousAssignment, "previous");
      return route.fulfill({
        status: 409,
        json: { code: "strict_no_alternative", error: message },
      });
    },
    context,
  );
  await page.evaluate(
    (key) => sessionStorage.setItem(key, "previous"),
    STRICT_ASSIGNMENT_KEY,
  );
  await preflight(page);
  await strictFailure(page, message);
  await page.waitForFunction(
    () =>
      document.querySelector(".editor-panel").inert &&
      document.querySelector("#jim-stage").inert &&
      document.activeElement === document.querySelector("#audio-check-title"),
  );
  assert.equal(await page.locator("#audio-check-join").isHidden(), true);
  assert.equal(await page.locator("#audio-check-join").isDisabled(), true);
  // A disabled control cannot resubmit even when activated without a pointer.
  await page.locator("#audio-check-join").evaluate((button) => button.click());
  assert.equal(page.url(), `${base}/interview.html?${query}`);
  assert.equal(calls.length, 1);
  assert.equal(consents.length, 1);
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    "previous",
  );
  await page.locator("#audio-check-leave").click();
  await page.waitForURL(`${base}/`);
  assert.equal(calls.length, 1);
  assert.equal(consents.length, 1);
});

test("an invalid Strict receipt is cleared only by explicitly starting a new selection sequence", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { context, consents } = await consentContext(t);
  const calls = [];
  const query = "selection=strict&difficulty=Easy&difficulty=Hard";
  const message = "Selection unavailable.";
  const page = await interview(
    t,
    query,
    (route) => {
      calls.push(route.request().postDataJSON());
      return calls.length === 1
        ? route.fulfill({
            status: 400,
            json: { code: "strict_assignment_invalid", error: message },
          })
        : route.fulfill({
            json: grant("chargeback-pair-match", "new-sequence"),
          });
    },
    context,
  );
  await page.evaluate(
    (key) => sessionStorage.setItem(key, "invalid-receipt"),
    STRICT_ASSIGNMENT_KEY,
  );
  await preflight(page);
  await strictFailure(page, message);
  assert.equal(
    await page.locator("#audio-check-join").textContent(),
    "Start a new selection sequence",
  );
  assert.equal(await page.locator("#audio-check-join").isEnabled(), true);
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    "invalid-receipt",
  );
  assert.equal(calls.length, 1);
  assert.equal(consents.length, 1);

  const reloaded = page.waitForEvent("domcontentloaded");
  await page.locator("#audio-check-join").click();
  await reloaded;
  await page.locator("#audio-check").waitFor();
  assert.equal(page.url(), `${base}/interview.html?${query}`);
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    null,
  );
  assert.equal(calls.length, 1);
  assert.equal(consents.length, 1);
  assert.equal(await page.locator("#audio-check-join").isDisabled(), true);
  await preflight(page);
  await page.waitForFunction(() => window.roomJoins.length === 1);
  assert.equal(calls.length, 2);
  assert.equal(consents.length, 2);
  assert.equal(calls[0].previousAssignment, "invalid-receipt");
  assert.equal(calls[1].previousAssignment, undefined);
  assert.deepEqual(calls[1].difficulties, ["Easy", "Hard"]);
  assert.equal(calls[1].problemId, undefined);
});

for (const failure of ["throws", "silently retains"]) {
  test(`an invalid Strict receipt cannot restart when storage ${failure} the receipt removal`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { context, consents } = await consentContext(t);
    let calls = 0;
    const message = "Selection unavailable.";
    const page = await interview(
      t,
      "selection=strict&difficulty=Easy",
      (route) => {
        calls++;
        return route.fulfill({
          status: 400,
          json: { code: "strict_assignment_invalid", error: message },
        });
      },
      context,
    );
    await page.evaluate(
      (key) => sessionStorage.setItem(key, "invalid-receipt"),
      STRICT_ASSIGNMENT_KEY,
    );
    await preflight(page);
    await strictFailure(page, message);
    let navigations = 0;
    page.on("framenavigated", (frame) => {
      if (frame === page.mainFrame()) navigations++;
    });
    await page.evaluate((mode) => {
      Storage.prototype.removeItem = () => {
        if (mode === "throws") throw new Error("blocked");
      };
    }, failure);
    await page.locator("#audio-check-join").click();
    await strictFailure(
      page,
      "Could not clear the previous assignment. Allow site storage or return to the lobby.",
    );
    assert.equal(navigations, 0);
    assert.equal(calls, 1);
    assert.equal(consents.length, 1);
    assert.equal(await page.locator("#audio-check-join").isEnabled(), true);
    assert.equal(
      await page.evaluate(
        (key) => sessionStorage.getItem(key),
        STRICT_ASSIGNMENT_KEY,
      ),
      "invalid-receipt",
    );
  });
}

test("a temporary Strict refusal retains its receipt and retries only after another media preflight", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { context, consents } = await consentContext(t);
  const calls = [];
  const message = "strict_no_alternative";
  const page = await interview(
    t,
    "selection=strict&difficulty=Easy",
    (route) => {
      calls.push(route.request().postDataJSON());
      return calls.length === 1
        ? route.fulfill({
            status: 503,
            json: { code: "agent_capacity_exhausted", error: message },
          })
        : route.fulfill({
            json: grant("chargeback-pair-match", "retry-receipt"),
          });
    },
    context,
  );
  await page.evaluate(
    (key) => sessionStorage.setItem(key, "previous"),
    STRICT_ASSIGNMENT_KEY,
  );
  await preflight(page);
  await strictFailure(page, message);
  assert.equal(await page.locator("#audio-check-join").isVisible(), true);
  assert.equal(await page.locator("#audio-check-join").isEnabled(), true);
  assert.equal(
    await page.locator("#audio-check-join").textContent(),
    "Retry media preflight",
  );
  assert.equal(calls.length, 1);
  assert.equal(consents.length, 1);
  const reloaded = page.waitForEvent("domcontentloaded");
  await page.locator("#audio-check-join").click();
  await reloaded;
  await page.locator("#audio-check").waitFor();
  assert.equal(calls.length, 1);
  assert.equal(consents.length, 1);
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    "previous",
  );
  await preflight(page);
  await page.waitForFunction(() => window.roomJoins.length === 1);
  assert.equal(calls.length, 2);
  assert.equal(consents.length, 2);
  assert.equal(calls[1].previousAssignment, "previous");
});

for (const failure of ["headers", "body", 408, 504]) {
  for (const strict of [true, false]) {
    test(`${strict ? "Strict" : "Practice"} handles token timeout at ${failure} without changing mode`, async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const { context, consents } = await consentContext(t);
      const requests = [];
      if (typeof failure === "string") {
        await context.addInitScript((phase) => {
          const nativeFetch = window.fetch;
          window.tokenFetches = 0;
          window.fetch = async (...args) => {
            if (new URL(args[0], location.href).pathname !== "/api/token")
              return nativeFetch(...args);
            window.tokenFetches++;
            const timeout = () => {
              throw new DOMException(
                "Offline practice is ready",
                "TimeoutError",
              );
            };
            if (phase === "headers") return timeout();
            return { ok: true, status: 200, json: async () => timeout() };
          };
        }, failure);
      }
      const page = await interview(
        t,
        strict
          ? "selection=strict&difficulty=Easy"
          : "problem=chargeback-pair-match",
        (route) => {
          requests.push(route.request().postDataJSON());
          return route.fulfill({
            status: typeof failure === "number" ? failure : 500,
            json: {
              code: "token_timeout",
              error: "Offline practice is ready",
            },
          });
        },
        context,
      );
      if (strict)
        await page.evaluate(
          (key) => sessionStorage.setItem(key, "previous"),
          STRICT_ASSIGNMENT_KEY,
        );
      await preflight(page);
      if (strict) {
        const message =
          "Strict Interview connection timed out. Return to the lobby or retry media preflight.";
        await strictFailure(page, message);
        assert.equal(
          await page.locator("#audio-check-join").textContent(),
          "Retry media preflight",
        );
        assert.equal(await page.locator("#audio-check-join").isVisible(), true);
        assert.equal(await page.locator("#audio-check-join").isEnabled(), true);
        assert.doesNotMatch(
          await page.locator("#presence-banner").textContent(),
          /Offline (?:mode|practice)/,
        );
        assert.equal(
          await page.evaluate(
            (key) => sessionStorage.getItem(key),
            STRICT_ASSIGNMENT_KEY,
          ),
          "previous",
        );
      } else {
        await page.waitForFunction(() =>
          document
            .querySelector("#transcript-panel")
            .textContent.includes("Offline mode is ready"),
        );
        assert.equal(await page.locator("#editor").isEnabled(), true);
        assert.equal(await page.locator("#audio-check").isHidden(), true);
        assert.equal(await page.evaluate(() => window.roomJoins.length), 0);
      }
      assert.equal(consents.length, 1);
      if (typeof failure === "string") {
        assert.equal(await page.evaluate(() => window.tokenFetches), 1);
        assert.equal(requests.length, 0);
      } else {
        assert.equal(requests.length, 1);
        assert.equal(
          requests[0].problemId,
          strict ? undefined : "chargeback-pair-match",
        );
      }
    });
  }
}

for (const strict of [true, false]) {
  test(`${strict ? "Strict" : "Practice"} handles the token deadline signal without an automatic retry`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { context, consents } = await consentContext(t);
    await context.addInitScript(() => {
      const nativeFetch = window.fetch;
      const nativeTimeout = AbortSignal.timeout.bind(AbortSignal);
      const deadlines = new WeakMap();
      window.tokenDeadlines = [];
      window.tokenFetches = 0;
      AbortSignal.timeout = (milliseconds) => {
        if (milliseconds !== 20_000) return nativeTimeout(milliseconds);
        const controller = new AbortController();
        deadlines.set(controller.signal, milliseconds);
        window.expireToken = () =>
          controller.abort(new DOMException("Token deadline", "TimeoutError"));
        return controller.signal;
      };
      window.fetch = (...args) => {
        if (new URL(args[0], location.href).pathname !== "/api/token")
          return nativeFetch(...args);
        window.tokenFetches++;
        window.tokenDeadlines.push(deadlines.get(args[1].signal));
        return new Promise((_, reject) => {
          args[1].signal.addEventListener(
            "abort",
            () => reject(args[1].signal.reason),
            { once: true },
          );
        });
      };
    });
    let requests = 0;
    const page = await interview(
      t,
      strict ? "selection=strict" : "problem=chargeback-pair-match",
      () => requests++,
      context,
    );
    await page.evaluate(
      (key) => sessionStorage.setItem(key, "previous"),
      STRICT_ASSIGNMENT_KEY,
    );
    let navigations = 0;
    page.on("framenavigated", (frame) => {
      if (frame === page.mainFrame()) navigations++;
    });
    await preflight(page);
    await page.waitForFunction(() => window.tokenFetches === 1);
    assert.deepEqual(
      await page.evaluate(() => window.tokenDeadlines),
      [20_000],
    );
    await page.evaluate(() => window.expireToken());
    if (strict) {
      const message =
        "Strict Interview connection timed out. Return to the lobby or retry media preflight.";
      await strictFailure(page, message);
      assert.equal(
        await page.locator("#presence-banner").textContent(),
        message,
      );
      assert.equal(
        await page.locator("#audio-check-join").textContent(),
        "Retry media preflight",
      );
      assert.equal(await page.locator("#audio-check-join").isEnabled(), true);
    } else {
      await page.waitForFunction(() =>
        document
          .querySelector("#transcript-panel")
          .textContent.includes("Offline mode is ready"),
      );
      assert.equal(await page.locator("#agent-state").textContent(), "Offline");
      assert.match(
        await page.locator("#presence-banner").textContent(),
        /You can still work the problem/,
      );
      assert.equal(await page.locator("#editor").isEnabled(), true);
      assert.equal(await page.locator("#audio-check").isHidden(), true);
    }
    assert.equal(navigations, 0);
    assert.equal(consents.length, 1);
    assert.equal(requests, 0);
    assert.equal(await page.evaluate(() => window.tokenFetches), 1);
    assert.equal(
      await page.evaluate(
        (key) => sessionStorage.getItem(key),
        STRICT_ASSIGNMENT_KEY,
      ),
      "previous",
    );
  });
}

test("a Strict HTTP 504 with a non-JSON body retains its timeout status", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { context, consents } = await consentContext(t);
  let calls = 0;
  const page = await interview(
    t,
    "selection=strict",
    (route) => {
      calls++;
      return route.fulfill({
        status: 504,
        contentType: "text/html",
        body: "<h1>Gateway Timeout</h1>",
      });
    },
    context,
  );
  await preflight(page);
  await strictFailure(
    page,
    "Strict Interview connection timed out. Return to the lobby or retry media preflight.",
  );
  assert.equal(
    await page.locator("#audio-check-join").textContent(),
    "Retry media preflight",
  );
  assert.equal(await page.locator("#audio-check-join").isVisible(), true);
  assert.equal(await page.locator("#audio-check-join").isEnabled(), true);
  assert.doesNotMatch(
    await page.locator("#presence-banner").textContent(),
    /Offline (?:mode|practice)/,
  );
  assert.equal(calls, 1);
  assert.equal(consents.length, 1);
});

test("a missing Strict problem retains its receipt without loading the fallback into an interview", async (t) => {
  const page = await interview(t, "selection=strict", (route) =>
    route.fulfill({ json: grant("missing-problem", "new-receipt") }),
  );
  if (!page) return;
  await preflight(page);
  await page.waitForFunction(
    () =>
      document.querySelector("#audio-check-join").textContent ===
      "Retry media preflight",
  );
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    "new-receipt",
  );
  assert.equal(await page.evaluate(() => window.roomJoins.length), 0);
  assert.equal(
    await page.locator("#problem-title").textContent(),
    "Strict Interview",
  );
});

test("blocked storage refuses Strict before requesting an attempt", async (t) => {
  let calls = 0;
  const page = await interview(t, "selection=strict", (route) => {
    calls++;
    return route.fulfill({ json: grant("chargeback-pair-match", "receipt") });
  });
  if (!page) return;
  await page.evaluate(() => {
    Storage.prototype.setItem = () => {
      throw new Error("blocked");
    };
  });
  await preflight(page);
  await page.waitForFunction(
    () =>
      document.querySelector("#audio-check-join").textContent ===
      "Retry media preflight",
  );
  assert.equal(calls, 0);
  assert.match(
    await page.locator("#audio-check-status").textContent(),
    /session storage/,
  );
});

test("two Strict tabs keep separate previous assignments in the same browser context", async (t) => {
  const firstCalls = [];
  const first = await interview(t, "selection=strict", (route) => {
    firstCalls.push(route.request().postDataJSON());
    return route.fulfill({ json: grant("chargeback-pair-match", "first-tab") });
  });
  if (!first) return;
  await preflight(first);
  await first.waitForFunction(() => window.roomJoins.length === 1);
  const secondCalls = [];
  const second = await interview(
    t,
    "selection=strict&problem=two-sum",
    (route) => {
      secondCalls.push(route.request().postDataJSON());
      return route.fulfill({
        json: grant("kiosk-panel-mirror-audit", "second-tab"),
      });
    },
    first.context(),
  );
  await preflight(second);
  await second.waitForFunction(() => window.roomJoins.length === 1);
  assert.equal(firstCalls[0].previousAssignment, undefined);
  assert.equal(secondCalls[0].previousAssignment, undefined);
  assert.equal(
    await first.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    "first-tab",
  );
  assert.equal(
    await second.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    "second-tab",
  );
});

test("a receipt write failing after allocation cannot reveal or join the assigned problem", async (t) => {
  let calls = 0;
  const page = await interview(t, "selection=strict", async (route) => {
    calls++;
    await page.evaluate(() => {
      Storage.prototype.setItem = () => {
        throw new Error("quota");
      };
    });
    await route.fulfill({ json: grant("chargeback-pair-match", "new") });
  });
  if (!page) return;
  await preflight(page);
  await page.waitForFunction(
    () =>
      document.querySelector("#audio-check-join").textContent ===
      "Retry media preflight",
  );
  assert.equal(calls, 1);
  assert.equal(
    await page.locator("#problem-title").textContent(),
    "Strict Interview",
  );
  assert.equal(await page.evaluate(() => window.roomJoins.length), 0);
});

test("a failed preflight starts no Strict attempt", async (t) => {
  let calls = 0;
  const page = await interview(t, "selection=strict", (route) => {
    calls++;
    return route.fulfill({ status: 500 });
  });
  if (!page) return;
  // The output confirmation is deliberately absent, even after microphone
  // sampling has succeeded and the optional camera has been skipped.
  await page.locator("#camera-skip").click();
  assert.equal(await page.locator("#audio-check-join").isDisabled(), true);
  assert.equal(calls, 0);
});

test("Strict can undo Camera Skip without allocating an attempt", async (t) => {
  const calls = [];
  const page = await interview(t, "selection=strict", (route) => {
    calls.push(route.request().postDataJSON());
    return route.fulfill({
      json: grant("chargeback-pair-match", "camera-receipt"),
    });
  });
  if (!page) return;
  await page.locator("#audio-heard").click();
  await page.locator("#camera-skip").click();
  await page.locator("#audio-check-join:not([disabled])").waitFor();
  assert.equal(await page.locator("#camera-skip").textContent(), "Use camera");
  assert.equal(calls.length, 0);

  await page.locator("#camera-skip").click();
  assert.equal(
    await page.locator("#camera-skip").textContent(),
    "Continue without camera",
  );
  assert.equal(await page.locator("#audio-check-join").isDisabled(), true);
  assert.equal(await page.locator("#camera-name-disclosure").isVisible(), true);
  assert.equal(calls.length, 0);

  await page.locator("#camera-skip").click();
  await page.locator("#audio-check-join").click();
  await page.waitForFunction(() => window.roomJoins.length === 1);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].selectionMode, "strict");
  assert.equal(calls[0].problemId, undefined);
});

test("the lobby defaults to Practice and starts Strict without a selected problem URL", async (t) => {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.route("**/api/session", (route) =>
    route.fulfill({
      json: { signedIn: true, user: { login: "candidate" } },
    }),
  );
  await page.route("**/api/reports", (route) =>
    route.fulfill({ json: { reports: [] } }),
  );
  await page.route("**/interview?**", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: "<p>New interview</p>",
    }),
  );
  const savedProblem = "chargeback-pair-match";
  await page.addInitScript(
    ({ key, problem }) => localStorage.setItem(key, JSON.stringify([problem])),
    { key: PRACTICE_LIST_KEY, problem: savedProblem },
  );
  await page.goto(base);
  await page.locator("#practice-list-pick:not([disabled])").waitFor();
  await page.locator("#practice-list-pick").click();
  assert.equal(
    await page.locator(".problem-card.selected").getAttribute("data-problem"),
    savedProblem,
  );
  for (const option of [
    "highlight",
    "execution",
    "autoIndent",
    "autoClose",
    "monospace",
  ])
    await page.locator(`[name="editor-option"][value="${option}"]`).uncheck();
  assert.equal(await page.locator(".setup-panel").count(), 3);
  assert.equal(
    await page
      .locator('[aria-labelledby="challenge-heading"] #selection-mode')
      .count(),
    1,
  );
  assert.equal(await page.locator("#selection-mode").inputValue(), "practice");
  assert.equal(await page.locator(".random-picker").isVisible(), true);
  assert.equal(await page.locator(".practice-list-picker").isVisible(), true);
  assert.equal(await page.locator(".problem-picker").isVisible(), true);
  assert.equal(await page.locator(".difficulty-row").isVisible(), true);
  await page.locator(".problem-picker > summary").click();
  await page.locator(".setup-grid > .problem-grid").waitFor();
  await page.locator("#selection-mode").selectOption("strict");
  assert.equal(await page.locator(".random-picker").isVisible(), false);
  assert.equal(await page.locator(".practice-list-picker").isVisible(), false);
  assert.equal(await page.locator(".problem-picker").isVisible(), false);
  await page.locator(".problem-grid").waitFor({ state: "hidden" });
  assert.equal(
    await page.locator(".problem-picker-outline").isVisible(),
    false,
  );
  await page.locator(".problem-grid").dispatchEvent("beforematch");
  assert.equal(await page.locator(".problem-grid").isVisible(), false);
  assert.equal(await page.locator(".difficulty-row").isVisible(), true);
  for (const width of [320, 1280]) {
    await page.setViewportSize({ width, height: 800 });
    await page.locator(".problem-grid").waitFor({ state: "hidden" });
    assert.equal(
      await page.locator(".problem-picker-outline").isVisible(),
      false,
    );
    const bounds = await page.locator("#selection-mode").boundingBox();
    assert.ok(
      bounds.width > 0 && bounds.x >= 0 && bounds.x + bounds.width <= width,
    );
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth > innerWidth,
      ),
      false,
    );
  }
  await page
    .getByRole("checkbox", { name: "Enable Example drawing", exact: true })
    .check();
  await page.locator('[name="difficulty"][value="Easy"]').check();
  await page.locator('[name="difficulty"][value="Hard"]').check();
  await page.locator('[name="difficulty"][value="Medium"]').uncheck();
  await page.locator("#selection-mode").selectOption("practice");
  await page.locator(".problem-grid").waitFor({ state: "visible" });
  assert.equal(await page.locator(".random-picker").isVisible(), true);
  assert.equal(await page.locator(".practice-list-picker").isVisible(), true);
  assert.equal(await page.locator(".problem-picker").isVisible(), true);
  await page.locator("#selection-mode").selectOption("strict");
  await page.evaluate(
    (key) => sessionStorage.setItem(key, "previous"),
    STRICT_ASSIGNMENT_KEY,
  );
  await page.locator("#start").click();
  await page.waitForURL("**/interview?**");
  const url = new URL(page.url());
  assert.equal(url.searchParams.get("selection"), "strict");
  assert.equal(url.searchParams.get("exampleDrawing"), "1");
  assert.deepEqual(url.searchParams.getAll("difficulty"), ["Easy", "Hard"]);
  assert.equal(url.searchParams.has("problem"), false);
  for (const parameter of [
    "editorHighlight",
    "editorExecution",
    "editorAutoIndent",
    "editorAutoClose",
    "editorMonospace",
  ])
    assert.equal(url.searchParams.get(parameter), "0", parameter);
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    null,
  );
});

test("restoring a Strict preflight page from history repeats preflight without allocating an attempt", async (t) => {
  let calls = 0;
  const page = await interview(t, "selection=strict", (route) => {
    calls++;
    return route.fulfill({
      json: grant("chargeback-pair-match", "history-receipt"),
    });
  });
  if (!page) return;
  await page.locator("#audio-check").waitFor({ state: "visible" });
  await page.evaluate(
    (key) => sessionStorage.setItem(key, "history-receipt"),
    STRICT_ASSIGNMENT_KEY,
  );
  const reloaded = page.waitForEvent("domcontentloaded");
  // Chromium's automation disables BFCache. Exercise its persisted-pageshow
  // signal, as the lobby restore tests do, without weakening the unload path.
  await page.evaluate(() =>
    window.dispatchEvent(
      new PageTransitionEvent("pageshow", { persisted: true }),
    ),
  );
  await reloaded;
  await page.locator("#audio-check").waitFor({ state: "visible" });
  assert.equal(
    await page.locator("#problem-title").textContent(),
    "Strict Interview",
  );
  assert.equal(await page.locator("#editor").inputValue(), "");
  assert.equal(calls, 0);
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    "history-receipt",
  );
});

for (const selection of ["strict", "practice"]) {
  test(`a cached ${selection} interview preserves its active attempt and unload warning`, async (t) => {
    let calls = 0;
    const query = new URLSearchParams({
      selection,
      problem: "chargeback-pair-match",
    });
    const page = await interview(t, query, (route) => {
      calls++;
      return route.fulfill({
        json: grant("chargeback-pair-match", "active-history-receipt"),
      });
    });
    if (!page) return;
    await preflight(page);
    await page.locator("#editor:not([disabled])").waitFor();
    await page.locator("#editor").fill("active attempt must survive");
    await page.waitForFunction(() => {
      const event = new Event("beforeunload", { cancelable: true });
      window.dispatchEvent(event);
      return event.defaultPrevented;
    });
    const guarded = await page.evaluate(() => {
      window.dispatchEvent(
        new PageTransitionEvent("pageshow", { persisted: true }),
      );
      const event = new Event("beforeunload", { cancelable: true });
      window.dispatchEvent(event);
      return event.defaultPrevented;
    });
    assert.equal(
      guarded,
      true,
      "history restoration keeps the existing warning",
    );
    assert.equal(
      await page.locator("#editor").inputValue(),
      "active attempt must survive",
    );
    assert.equal(await page.locator("#audio-check").isVisible(), false);
    assert.equal(calls, 1);
    assert.equal(await page.evaluate(() => window.roomJoins.length), 1);
    if (selection === "strict")
      assert.equal(
        await page.evaluate(
          (key) => sessionStorage.getItem(key),
          STRICT_ASSIGNMENT_KEY,
        ),
        "active-history-receipt",
      );
  });
}

for (const saved of [false, true]) {
  test(
    saved
      ? "a cached Strict page returns to preflight after its report is saved"
      : "a cached Strict page keeps a report that failed both history saves",
    async (t) => {
      let calls = 0;
      let reportSaves = 0;
      const page = await interview(t, "selection=strict", (route) => {
        calls++;
        return route.fulfill({
          json: grant("chargeback-pair-match", "report-history-receipt"),
        });
      });
      if (!page) return;
      await page.route("**/api/reports", (route) => {
        assert.equal(route.request().method(), "POST");
        reportSaves++;
        return route.fulfill({ status: saved ? 200 : 503, json: {} });
      });
      if (!saved)
        await page.evaluate(() => {
          const setItem = Storage.prototype.setItem;
          Storage.prototype.setItem = function store(key, value) {
            if (this === localStorage && key === "codetrial_history")
              throw new Error("History storage is full");
            return setItem.call(this, key, value);
          };
        });
      await preflight(page);
      await page.locator("#editor:not([disabled])").waitFor();
      await page.waitForFunction(() => {
        const event = new Event("beforeunload", { cancelable: true });
        window.dispatchEvent(event);
        return event.defaultPrevented;
      });
      await page.evaluate(async () => {
        const handlers = window.roomEvents.DataReceived;
        if (!handlers?.length) throw new Error("Report handler is missing");
        const payload = new TextEncoder().encode(
          JSON.stringify({ summary: "The cached report remains available." }),
        );
        await Promise.all(
          handlers.map((handler) =>
            handler(payload, { kind: "AGENT" }, null, "report"),
          ),
        );
      });
      assert.equal(
        await page.locator("#report-save-status").textContent(),
        saved ? "Saved to your account" : "Report was not saved",
      );
      assert.equal(reportSaves, 1);
      assert.equal(await page.locator("#report-modal").isVisible(), true);
      assert.match(
        await page.locator("#report-modal").textContent(),
        /The cached report remains available/,
      );
      if (saved) {
        const reloaded = page.waitForEvent("domcontentloaded");
        await page.evaluate(() =>
          window.dispatchEvent(
            new PageTransitionEvent("pageshow", { persisted: true }),
          ),
        );
        await reloaded;
        await page.locator("#audio-check").waitFor({ state: "visible" });
        assert.equal(await page.locator("#report-modal").isVisible(), false);
      } else {
        const guarded = await page.evaluate(() => {
          window.dispatchEvent(
            new PageTransitionEvent("pageshow", { persisted: true }),
          );
          const event = new Event("beforeunload", { cancelable: true });
          window.dispatchEvent(event);
          return event.defaultPrevented;
        });
        assert.equal(guarded, true, "a failed save keeps the unload warning");
        assert.equal(await page.locator("#report-modal").isVisible(), true);
        assert.match(
          await page.locator("#report-modal").textContent(),
          /The cached report remains available/,
        );
        assert.equal(await page.locator("#audio-check").isVisible(), false);
      }
      assert.equal(calls, 1);
      assert.equal(reportSaves, 1);
      assert.equal(
        await page.evaluate(
          (key) => sessionStorage.getItem(key),
          STRICT_ASSIGNMENT_KEY,
        ),
        "report-history-receipt",
      );
    },
  );
}

test("Strict assignment, reconnect and refresh retain disabled editor assistance and execution", async (t) => {
  const calls = [];
  const pages = ["chargeback-pair-match", "kiosk-panel-mirror-audit"];
  const query = new URLSearchParams({
    selection: "strict",
    problem: "two-sum",
    editorHighlight: "0",
    editorExecution: "0",
    editorAutoIndent: "0",
    editorAutoClose: "0",
    editorMonospace: "0",
  });
  const page = await interview(t, query, (route) => {
    calls.push(route.request().postDataJSON());
    return route.fulfill({
      json: grant(pages[calls.length - 1], `options-${calls.length}`),
    });
  });
  if (!page) return;
  const parsers = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.startsWith("/vendor/tree-sitter"))
      parsers.push(request.url());
  });
  assert.equal(calls.length, 0);
  for (let attempt = 0; attempt < 2; attempt++) {
    if (attempt) await page.reload();
    assert.equal(await page.locator("#audio-check").isVisible(), true);
    assert.equal(calls.length, attempt);
    await preflight(page);
    await page.locator("#editor:not([disabled])").waitFor();
    const request = calls[attempt];
    assert.equal(request.selectionMode, "strict");
    assert.equal(
      request.difficulties,
      undefined,
      "old Strict URLs keep an unrestricted draw",
    );
    assert.equal(request.problemId, undefined);
    assert.equal(request.codeExecution, false);
    assert.equal(request.previousAssignment, attempt ? "options-1" : undefined);
    assert.equal(await page.locator("#editor-highlight").isVisible(), false);
    assert.equal(
      await page
        .locator(".editor-stack")
        .evaluate((element) =>
          element.style.getPropertyValue("--editor-font-family"),
        ),
      "inherit",
    );
    await page.locator("#editor").fill("");
    await page.keyboard.type("(");
    assert.equal(await page.locator("#editor").inputValue(), "(");
    await page.locator("#editor").fill("    if ready:");
    await page.keyboard.press("End");
    await page.keyboard.press("Enter");
    assert.equal(await page.locator("#editor").inputValue(), "    if ready:\n");

    await page.locator("#language-select").selectOption("javascript");
    await page.locator("#editor").fill("const answer = 1;");
    const code = await page.locator("#editor").inputValue();
    for (const chord of ["Control+Enter", "Meta+Enter"])
      await page.keyboard.press(chord);
    assert.equal(await page.locator("#editor").inputValue(), code);
    await page.evaluate(() => {
      for (const event of ["Reconnecting", "Reconnected"])
        for (const callback of window.roomEvents[event] ?? []) callback();
    });
    assert.equal(await page.locator("#run-tests").isVisible(), false);
    assert.equal(await page.locator("#run-tests").isDisabled(), true);
    assert.equal(await page.locator(".test-drawer").isVisible(), false);
    assert.equal(await page.locator("#editor").inputValue(), code);
    assert.equal(
      await page.locator("#language-select").inputValue(),
      "javascript",
    );
    assert.equal(calls.length, attempt + 1);
  }
  assert.deepEqual(parsers, []);
});

for (const [levels, assigned] of [
  [["Easy"], "chargeback-pair-match"],
  [["Medium"], "carousel-slot-shift"],
  [["Hard"], "terrain-runoff-pooling"],
  [["Easy", "Hard"], "terrain-runoff-pooling"],
]) {
  test(`Strict forwards ${levels.join("/")} to the server and loads its assignment`, async (t) => {
    const calls = [];
    const query = new URLSearchParams({
      selection: "strict",
      problem: "template-delimiter-audit",
    });
    for (const level of levels) query.append("difficulty", level);
    const page = await interview(t, query, (route) => {
      calls.push(route.request().postDataJSON());
      return route.fulfill({ json: grant(assigned, "difficulty-receipt") });
    });
    if (!page) return;
    await preflight(page);
    await page.waitForFunction(() => window.roomJoins.length === 1);
    assert.equal(calls.length, 1);
    assert.deepEqual(calls[0].difficulties, levels);
    assert.equal(calls[0].problemId, undefined);
    const problem = await page.evaluate(
      async (pageName) => (await fetch(`/problems/${pageName}.json`)).json(),
      assigned,
    );
    assert.equal(
      await page.locator("#problem-title").textContent(),
      problem.title,
    );
    assert.ok(levels.includes(problem.difficulty));
  });
}

test("an invalid Strict URL difficulty returns to the lobby without broadening or retrying selection", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { context, consents } = await consentContext(t);
  const calls = [];
  const message = "Choose different options. Retry media preflight.";
  const query =
    "selection=strict&difficulty=Easy&difficulty=Impossible&problem=two-sum";
  const page = await interview(
    t,
    query,
    (route) => {
      calls.push(route.request().postDataJSON());
      return route.fulfill({
        status: 400,
        json: { code: "strict_difficulty_invalid", error: message },
      });
    },
    context,
  );
  await preflight(page);
  await strictFailure(page, message);
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].difficulties, ["Easy", "Impossible"]);
  assert.equal(calls[0].problemId, undefined);
  assert.equal(await page.locator("#audio-check-join").isHidden(), true);
  assert.equal(await page.locator("#audio-check-join").isDisabled(), true);
  await page.locator("#audio-check-join").evaluate((button) => button.click());
  assert.equal(page.url(), `${base}/interview.html?${query}`);
  assert.equal(calls.length, 1);
  assert.equal(consents.length, 1);
  await page.locator("#audio-check-leave").click();
  await page.waitForURL(`${base}/`);
  assert.equal(calls.length, 1);
  assert.equal(consents.length, 1);
});

test("Practice still filters by difficulty and opens a specifically chosen problem after switching modes", async (t) => {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.route("**/api/session", (route) =>
    route.fulfill({
      json: { signedIn: true, user: { login: "candidate" } },
    }),
  );
  await page.route("**/api/reports", (route) =>
    route.fulfill({ json: { reports: [] } }),
  );
  await page.route("**/interview?**", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: "<p>New interview</p>",
    }),
  );
  await page.goto(base);
  await page.locator("#start:not([disabled])").waitFor();
  await page.locator("#selection-mode").selectOption("strict");
  await page.locator("#selection-mode").selectOption("practice");
  await page.locator('[name="difficulty"][value="Easy"]').check();
  await page.locator('[name="difficulty"][value="Medium"]').uncheck();
  await page.locator(".problem-picker summary").click();
  const chosen = "chargeback-pair-match";
  assert.equal(
    await page.locator('[data-problem="carousel-slot-shift"]').isVisible(),
    false,
  );
  await page.locator(`[data-problem="${chosen}"]`).click();
  await page.locator("#start").click();
  await page.waitForURL("**/interview?**");
  const params = new URL(page.url()).searchParams;
  assert.equal(params.get("problem"), chosen);
  assert.equal(params.has("selection"), false);
  assert.equal(
    params.has("difficulty"),
    false,
    "Practice URL shape is unchanged",
  );
});

test("Restore Starter is harmless while Strict waits for its assigned problem", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  let release;
  let requested;
  const pending = new Promise((resolve) => {
    release = resolve;
  });
  const tokenRequested = new Promise((resolve) => {
    requested = resolve;
  });
  t.after(() => release());
  const page = await interview(t, "selection=strict", async (route) => {
    requested();
    await pending;
    return route.fulfill({
      json: grant("chargeback-pair-match", "pending-starter"),
    });
  });
  await preflight(page);
  await tokenRequested;
  assert.equal(await page.locator("#audio-check").isHidden(), true);
  assert.equal(await page.locator("#editor").isDisabled(), true);
  assert.equal(await page.locator("#run-tests").isDisabled(), true);
  const before = await page.locator("#editor").inputValue();
  await page.locator("#editor-restore-starter").click();
  assert.equal(await page.locator("#editor").inputValue(), before);
  release();
  await page.locator("#run-tests:not([disabled])").waitFor();
  assert.equal(await page.evaluate(() => window.roomJoins.length), 1);
  assert.notEqual(await page.locator("#editor").inputValue(), "");
});

for (const outcome of ["success", "failure"]) {
  test(`Strict keeps Run disabled through a slow connection and ${outcome}`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    let releaseJudge;
    const judge = new Promise((resolve) => {
      releaseJudge = resolve;
    });
    t.after(() => releaseJudge());
    let calls = 0;
    const page = await interview(t, "selection=strict", (route) => {
      calls++;
      return route.fulfill({
        json: grant("chargeback-pair-match", "slow-receipt"),
      });
    });
    await page.route("**/judges/*.json", async (route) => {
      await judge;
      await route.continue();
    });
    await page.evaluate(() => {
      const Room = window.LivekitClient.Room;
      window.testPackets = [];
      window.LivekitClient.Room = class extends Room {
        constructor(...args) {
          super(...args);
          this.localParticipant.publishData = async (payload, options) => {
            window.testPackets.push({
              topic: options.topic,
              payload: JSON.parse(new TextDecoder().decode(payload)),
            });
          };
        }
        async connect(...args) {
          window.connectionPending = true;
          await new Promise((resolve, reject) => {
            window.finishConnection = resolve;
            window.failConnection = () =>
              reject(new Error("LiveKit join failed"));
          });
          return super.connect(...args);
        }
      };
    });
    await preflight(page);
    await page.waitForFunction(() => window.connectionPending);
    assert.equal(await page.locator("#run-tests").isDisabled(), true);
    await page.locator("#language-select").selectOption("javascript");
    const initialResults = await page.locator("#results-content").textContent();
    await page.evaluate(() => {
      document
        .querySelector("#run-tests")
        .dispatchEvent(new MouseEvent("click"));
    });
    assert.equal(
      (await page.locator("#run-tests").textContent()).trim(),
      "Run tests",
    );
    assert.equal(
      await page.locator("#results-content").textContent(),
      initialResults,
    );
    assert.equal(
      await page.evaluate(
        () =>
          window.testPackets.filter((p) => p.topic === "test_results").length,
      ),
      0,
    );

    if (outcome === "failure") {
      await page.evaluate(() => window.failConnection());
      await strictFailure(page, "LiveKit join failed");
    }
    releaseJudge();
    // The placeholder is painted after the delayed judge's language update.
    await page.waitForFunction(
      () => document.querySelector("#candidate-case-input").placeholder !== "",
    );
    assert.equal(await page.locator("#run-tests").isDisabled(), true);
    await page.evaluate(() => {
      document
        .querySelector("#run-tests")
        .dispatchEvent(new MouseEvent("click"));
    });
    assert.equal(
      (await page.locator("#run-tests").textContent()).trim(),
      "Run tests",
    );
    assert.equal(
      await page.locator("#results-content").textContent(),
      initialResults,
    );
    assert.doesNotMatch(
      await page.locator("#transcript-panel").textContent(),
      /I ran the tests|tests passed/,
    );
    assert.equal(calls, 1);

    if (outcome === "success") {
      await page.evaluate(() => window.finishConnection());
      await page.locator("#run-tests:not([disabled])").waitFor();
      assert.equal(await page.locator("#editor").isEnabled(), true);
      await page.locator("#run-tests").click();
      await page.waitForFunction(() =>
        window.testPackets.some((p) => p.topic === "test_results"),
      );
      const packets = await page.evaluate(() =>
        window.testPackets.filter((p) => p.topic === "test_results"),
      );
      assert.equal(packets.length, 1);
      assert.ok(packets[0].payload.total > 0);
      assert.doesNotMatch(
        await page.locator("#transcript-panel").textContent(),
        /I ran the tests/,
      );
    } else {
      assert.equal(await page.evaluate(() => window.roomJoins.length), 0);
    }
  });
}

test("Strict Run follows reconnect, terminal disconnect and ending states", async (t) => {
  const calls = [];
  const page = await interview(t, "selection=strict", (route) => {
    calls.push(route.request().postDataJSON());
    return route.fulfill({
      json: grant("chargeback-pair-match", "run-lifecycle"),
    });
  });
  if (!page) return;
  await preflight(page);
  await page.locator("#end:not([disabled])").waitFor();
  await page.locator("#run-tests:not([disabled])").waitFor();
  const event = (name) =>
    page.evaluate((name) => {
      const handlers = window.roomEvents[name];
      if (!handlers?.length) throw new Error(`Missing ${name} handler`);
      for (const callback of handlers) callback();
    }, name);
  const blockedRun = async () => {
    assert.equal(await page.locator("#run-tests").isDisabled(), true);
    await page.evaluate(() => {
      document
        .querySelector("#run-tests")
        .dispatchEvent(new MouseEvent("click"));
    });
    assert.equal(
      (await page.locator("#run-tests").textContent()).trim(),
      "Run tests",
    );
  };
  await event("Reconnecting");
  await blockedRun();
  await page.locator("#language-select").selectOption("javascript");
  await blockedRun();
  await event("Reconnected");
  assert.equal(await page.locator("#run-tests").isEnabled(), true);
  await event("Disconnected");
  await blockedRun();
  assert.equal(
    await page.locator("#agent-state").textContent(),
    "Not connected",
  );
  assert.equal(
    await page.locator("#presence-banner").textContent(),
    "The Strict Interview connection dropped.",
  );
  assert.equal(calls.length, 1);
  assert.equal(await page.evaluate(() => window.roomJoins.length), 1);
  // End during the terminal drop, then simulate a late transport notification.
  const disabled = await page.evaluate(() => {
    document.querySelector("#end").click();
    for (const callback of window.roomEvents.Reconnected) callback();
    return document.querySelector("#run-tests").disabled;
  });
  assert.equal(disabled, true);
  assert.equal(calls.length, 1);
});
