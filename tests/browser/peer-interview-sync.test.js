import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { launchChromium, startStaticServer } from "./source.js";
import { until } from "./wait.js";
let browser, server, base;
before(async () => {
  browser = await launchChromium();
  if (browser) ({ server, base } = await startStaticServer());
});
after(async () => {
  await browser?.close();
  await new Promise((r) => (server ? server.close(r) : r()));
});
async function fixture(t, role, deferredRun = false, options = {}) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.addInitScript(() => {
    window.peerTicks = [];
    window.watchTicks = [];
    const original = window.setInterval;
    window.setInterval = (fn, ms, ...args) =>
      ms === 5000
        ? (window.peerTicks.push(fn), 9999)
        : ms === 20000
          ? (window.watchTicks.push(fn), 8888)
          : original(fn, ms, ...args);
  });
  const state = {
    status: "active",
    version: 3,
    serverNow: Math.floor(Date.now() / 1000),
    you: { slot: 0 },
    settings: { language: "javascript" },
    members: [
      { slot: 0, login: "a" },
      { slot: 1, login: "b" },
    ],
    currentRound: {
      index: 0,
      status: options.feedback
        ? "feedback"
        : options.finishing
          ? "finishing"
          : "active",
      role,
      deadline: Math.floor(Date.now() / 1000) + 1500,
    },
  };
  const snapshot = {
    revision: options.emptyInitial ? 0 : 2,
    language: "javascript",
    code: options.emptyInitial ? "" : "const initial = 1;",
    tests: null,
  };
  let reads = 0,
    owner = "original",
    writes = 0;
  await page.route("**/api/peer/sessions/s", (r) => r.fulfill({ json: state }));
  let problemReads = 0;
  await page.route("**/api/peer/sessions/s/rounds/0/problem", (r) => {
    problemReads++;
    return r.fulfill(
      options.failInitial && problemReads === 1
        ? { status: 503, json: { code: "database_unavailable" } }
        : { json: { page: "chargeback-pair-match", title: "Pair" } },
    );
  });
  await page.route("**/api/peer/sessions/s/rounds/0/guide", (r) =>
    r.fulfill({ json: { contract: "Find pair" } }),
  );
  await page.route("**/api/peer/sessions/s/rounds/0/snapshot", (r) => {
    if (r.request().method() === "PUT") {
      writes++;
      if (options.cas) {
        const body = r.request().postDataJSON();
        if (body.expectedRevision !== snapshot.revision)
          return r.fulfill({ status: 409, json: { code: "version_conflict" } });
        snapshot.revision++;
        snapshot.code = body.code;
        return r.fulfill({ json: { revision: snapshot.revision } });
      }
      return r.fulfill({ status: 409, json: { code: "lease_lost" } });
    }
    reads++;
    return r.fulfill({ json: snapshot });
  });
  const writerRequests = [];
  await page.route("**/api/peer/sessions/s/rounds/0/writer", (r) => {
    const body = r.request().postDataJSON();
    writerRequests.push(body);
    if (options.takeoverRace && reads > 0) snapshot.revision++;
    if (body.previousLease && body.previousLease !== owner)
      return r.fulfill({ status: 409, json: { code: "lease_lost" } });
    owner = body.previousLease ? "renewed" : "original";
    return r.fulfill({ json: { lease: owner } });
  });
  if (options.feedback)
    await page.route("**/api/peer/sessions/s/rounds/0/feedback", (r) => {
      if (r.request().method() === "PUT") {
        options.feedback.payload = r.request().postDataJSON();
        options.feedback.submitted = options.feedback.payload.submitted;
        return r.fulfill({ json: { saved: true } });
      }
      return r.fulfill({ json: { feedback: [options.feedback] } });
    });
  if (deferredRun)
    await page.route("**/runners.js", (r) =>
      r.fulfill({
        contentType: "text/javascript",
        body: `export async function runBrowserTests() { return new Promise(resolve => { window.finishPeerRun = () => resolve({passed:1,total:1,cases:[{label:"obsolete result",pass:true}]}); }); }`,
      }),
    );
  if (options.audio)
    await page.route("**/vendor/livekit-client.js", (r) =>
      r.fulfill({
        contentType: "text/javascript",
        body: `window.connectionAttempts=0;window.LivekitClient={RoomEvent:{},Room:class{localParticipant={trackPublications:new Map(),setMicrophoneEnabled:async()=>{}};on(){}async connect(){window.connectionAttempts++}async disconnect(){}}};`,
      }),
    );
  await page.goto(`${base}/peer-interview.html?session=s`);
  if (options.failInitial)
    await page.waitForFunction(() =>
      document
        .getElementById("peer-error")
        .textContent.includes("The service is busy"),
    );
  else
    await page.waitForFunction(
      () =>
        document.getElementById("editor").value.length > 0 &&
        window.peerTicks.length === 2,
    );
  return {
    page,
    state,
    snapshot,
    writerRequests,
    get reads() {
      return reads;
    },
    get writes() {
      return writes;
    },
    takeover() {
      owner = "other";
    },
  };
}
test("interviewer polling refreshes authoritative code and test results", async (t) => {
  const f = await fixture(t, "interviewer");
  if (!f) return;
  f.snapshot.revision = 3;
  f.snapshot.code = "const authoritative = 2;";
  f.snapshot.tests = {
    passed: 1,
    total: 1,
    cases: [{ label: "fresh result", pass: true }],
  };
  await f.page.evaluate(() => window.peerTicks[0]());
  await f.page.waitForFunction(
    () =>
      document.getElementById("editor").value === "const authoritative = 2;",
    {},
    { timeout: 3000 },
  );
  assert.match(
    await f.page.locator("#peer-results").textContent(),
    /fresh result/,
  );
});
for (const replaced of [false, true])
  test(`expired writer recovery preserves dirty code, takeover=${replaced}`, async (t) => {
    const f = await fixture(t, "candidate");
    if (!f) return;
    await f.page.locator("#editor").fill("const unsaved = 9;");
    if (replaced) f.takeover();
    await f.page.evaluate(() => window.peerTicks[1]());
    await f.page.locator("#peer-download").waitFor({ state: "visible" });
    await f.page.locator("#peer-recover-writer").click({ timeout: 3000 });
    await f.page.waitForFunction(
      () => !document.getElementById("peer-recover-writer").disabled,
    );
    assert.deepEqual(f.writerRequests, [{}, { previousLease: "original" }]);
    assert.equal(
      await f.page.locator("#editor").inputValue(),
      "const unsaved = 9;",
    );
    assert.equal(
      await f.page.locator("#editor").getAttribute("readonly"),
      replaced ? "" : null,
    );
    assert.equal(await f.page.locator("#peer-download").isVisible(), replaced);
  });

test("persisted page restoration resumes polling and preserves unsaved candidate code", async (t) => {
  const f = await fixture(t, "candidate");
  if (!f) return;
  await f.page.locator("#editor").fill("const retained = 7;");
  await f.page.evaluate(() =>
    window.dispatchEvent(
      new PageTransitionEvent("pagehide", { persisted: true }),
    ),
  );
  f.state.members[1].nickname = "Restored7";
  await f.page.evaluate(() =>
    window.dispatchEvent(
      new PageTransitionEvent("pageshow", { persisted: true }),
    ),
  );
  await f.page.waitForFunction(
    () =>
      document
        .getElementById("peer-partners")
        .textContent.includes("Restored7"),
    {},
    { timeout: 3000 },
  );
  assert.equal(
    await f.page.locator("#editor").inputValue(),
    "const retained = 7;",
  );
});

test("sealed round loads final empty code and clears prior test results", async (t) => {
  const f = await fixture(t, "interviewer");
  if (!f) return;
  f.snapshot.revision++;
  f.snapshot.tests = {
    passed: 1,
    total: 1,
    cases: [{ label: "prior case", pass: true }],
  };
  await f.page.evaluate(() => window.peerTicks[0]());
  await f.page.waitForFunction(() =>
    document.getElementById("peer-results").textContent.includes("prior case"),
  );
  await f.page.route("**/api/peer/sessions/s/rounds/0/feedback", (r) =>
    r.fulfill({ json: {} }),
  );
  f.state.currentRound.status = "feedback";
  f.snapshot.revision++;
  f.snapshot.code = "";
  f.snapshot.tests = null;
  await f.page.evaluate(() => window.peerTicks[0]());
  await f.page.locator("#peer-feedback-panel").waitFor({ state: "visible" });
  assert.equal(await f.page.locator("#editor").inputValue(), "");
  assert.doesNotMatch(
    await f.page.locator("#peer-results").textContent(),
    /prior case/,
  );
});

test("test completion for older code does not publish stale results or save newer edits", async (t) => {
  const f = await fixture(t, "candidate", true);
  if (!f) return;
  await f.page.locator("#peer-run").click();
  await f.page.waitForFunction(
    () => typeof window.finishPeerRun === "function",
  );
  await f.page.locator("#editor").fill("const newer = 3;");
  await f.page.evaluate(() => window.finishPeerRun());
  await f.page.waitForFunction(
    () => !document.getElementById("peer-run").disabled,
  );
  assert.equal(f.writes, 0);
  assert.doesNotMatch(
    await f.page.locator("#peer-results").textContent(),
    /obsolete result/,
  );
  assert.equal(
    await f.page.locator("#editor").inputValue(),
    "const newer = 3;",
  );
});

test("initial round fetch failure retries without reloading the page", async (t) => {
  const f = await fixture(t, "candidate", false, { failInitial: true });
  if (!f) return;
  assert.equal(await f.page.evaluate(() => window.peerTicks.length), 2);
  await f.page.evaluate(() => window.peerTicks[0]());
  await f.page.waitForFunction(
    () => !document.getElementById("editor").readOnly,
    {},
    { timeout: 3000 },
  );
  assert.equal(
    await f.page.locator("#editor").inputValue(),
    "const initial = 1;",
  );
});

test("unsaved round uses starter while saved empty code remains empty", async (t) => {
  const f = await fixture(t, "interviewer", false, { emptyInitial: true });
  if (!f) return;
  assert.ok((await f.page.locator("#editor").inputValue()).length > 0);
  f.snapshot.revision = 1;
  await f.page.evaluate(() => window.peerTicks[0]());
  await f.page.waitForFunction(
    () => document.getElementById("editor").value === "",
    {},
    { timeout: 3000 },
  );
});

test("token resolution during persisted suspension does not reconnect audio", async (t) => {
  const f = await fixture(t, "candidate", false, { audio: true });
  if (!f) return;
  let release;
  let requested;
  const pending = new Promise((resolve) => {
    requested = resolve;
  });
  await f.page.route("**/api/peer/sessions/s/token", (r) => {
    release = () =>
      r.fulfill({ json: { url: "wss://example", token: "token" } });
    requested();
  });
  await f.page.locator("#peer-connect").click();
  await pending;
  await f.page.evaluate(() =>
    dispatchEvent(new PageTransitionEvent("pagehide", { persisted: true })),
  );
  await release();
  await f.page.waitForFunction(
    () => !document.getElementById("peer-connect").disabled,
    {},
    { timeout: 3000 },
  );
  assert.equal(await f.page.evaluate(() => window.connectionAttempts), 0);
});

test("writer takeover reads the snapshot after acquiring its lease", async (t) => {
  const f = await fixture(t, "candidate", false, {
    cas: true,
    takeoverRace: true,
  });
  if (!f) return;
  await f.page.locator("#editor").fill("const owned = 8;");
  await f.page.evaluate(() => window.peerTicks[1]());
  await f.page.waitForFunction(
    () =>
      document
        .getElementById("peer-save-status")
        .textContent.includes("Saved to session"),
    {},
    { timeout: 3000 },
  );
  assert.equal(f.snapshot.code, "const owned = 8;");
});

test("snapshot CAS conflict reconciles revision without replacing unsaved code", async (t) => {
  const f = await fixture(t, "candidate", false, { cas: true });
  if (!f) return;
  f.snapshot.revision++;
  await f.page.locator("#editor").fill("const retained = 8;");
  const reads = f.reads;
  await f.page.evaluate(() => window.peerTicks[1]());
  await f.page.waitForFunction(
    () =>
      document
        .getElementById("peer-save-status")
        .textContent.includes("Unsaved changes retained"),
    {},
    { timeout: 3000 },
  );
  assert.ok(f.reads > reads);
  assert.equal(
    await f.page.locator("#editor").inputValue(),
    "const retained = 8;",
  );
  await f.page.evaluate(() => window.peerTicks[1]());
  await f.page.waitForFunction(
    () =>
      document
        .getElementById("peer-save-status")
        .textContent.includes("Saved to session"),
    {},
    { timeout: 3000 },
  );
  assert.equal(f.snapshot.code, "const retained = 8;");
});

for (const submitted of [false, true])
  test(`saved feedback hydrates once and respects submitted=${submitted}`, async (t) => {
    const feedback = {
      authorSlot: 0,
      submitted,
      payload: {
        dimensions: {
          preparation: {
            score: 3,
            example: "Saved example",
            improvement: "Try next",
          },
        },
      },
    };
    const f = await fixture(t, "candidate", false, { feedback });
    if (!f) return;
    const input = f.page.locator('[name="preparation:example"]');
    assert.equal(await input.inputValue(), "Saved example");
    assert.equal(await input.isDisabled(), submitted);
    assert.equal(
      await f.page
        .locator('#peer-feedback button[value="submit"]')
        .isDisabled(),
      submitted,
    );
    assert.equal(
      await f.page.locator("#peer-advance").isDisabled(),
      !submitted,
      "the role swap waits for this round's own submitted feedback",
    );
    if (submitted) return;
    await input.fill("Edited example");
    await f.page.evaluate(() => window.peerTicks[0]());
    assert.equal(await input.inputValue(), "Edited example");
    await f.page.locator('#peer-feedback button[value="submit"]').click();
    await input.waitFor({ state: "visible" });
    await f.page.waitForFunction(
      () => document.querySelector('[name="preparation:example"]').disabled,
      {},
      { timeout: 3000 },
    );
    assert.equal(
      feedback.payload.dimensions.preparation.example,
      "Edited example",
    );
  });

test("a candidate page opened while the round is finishing takes a writer lease and saves the final code", async (t) => {
  const f = await fixture(t, "candidate", false, {
    finishing: true,
    cas: true,
  });
  if (!f) return;
  // Outwaits the lease request and the final save that follow the first load.
  await new Promise((resolve) => setTimeout(resolve, 500));
  assert.deepEqual(f.writerRequests, [{}]);
  assert.equal(f.writes, 1);
});

test("a message from an action survives the next poll", async (t) => {
  const f = await fixture(t, "interviewer");
  if (!f) return;
  // No route answers finishing, so the action fails and says so.
  await f.page.locator("#peer-finish").click();
  await f.page.waitForFunction(
    () => document.getElementById("peer-error").textContent !== "",
  );
  const message = await f.page.locator("#peer-error").textContent();
  // A tick is skipped while another poll is in flight, so keep ticking until
  // a session response lands after the click.
  let polled = 0;
  f.page.on("response", (r) => {
    if (r.url().endsWith("/api/peer/sessions/s")) polled++;
  });
  for (let attempt = 0; attempt < 50 && polled === 0; attempt++) {
    await f.page.evaluate(() => window.peerTicks[0]());
    // Give the response a moment to arrive before ticking again.
    await f.page.evaluate(() => new Promise((done) => setTimeout(done, 100)));
  }
  assert.ok(polled > 0, "the session was never polled after the click");
  // The response arriving is not the handler finishing; give it a turn.
  await f.page.evaluate(() => new Promise((done) => setTimeout(done, 200)));
  assert.equal(await f.page.locator("#peer-error").textContent(), message);
});

test("the partner's feedback is read as fields, not as raw data", async (t) => {
  const f = await fixture(t, "interviewer");
  if (!f) return;
  await f.page.route("**/api/peer/sessions/s/rounds/0/feedback", (r) =>
    r.fulfill({
      json: {
        feedback: [
          {
            authorSlot: 1,
            submitted: true,
            payload: {
              dimensions: {
                reasoning: {
                  score: 3,
                  example: "Named the invariant early",
                  improvement: "State the complexity sooner",
                },
              },
            },
          },
        ],
      },
    }),
  );
  f.state.currentRound.status = "feedback";
  await f.page.evaluate(() => window.peerTicks[0]());
  await f.page.waitForFunction(() =>
    document
      .getElementById("peer-feedback-results")
      .textContent.includes("Named the invariant early"),
  );
  const text = await f.page.locator("#peer-feedback-results").textContent();
  assert.match(text, /Your partner's feedback/);
  assert.match(text, /Rating 3 of 4/);
  assert.match(text, /To improve: State the complexity sooner/);
  assert.doesNotMatch(text, /authorSlot|payload|[{}]/);
});

test("a cancelled practice says so instead of asking to prepare", async (t) => {
  const f = await fixture(t, "interviewer");
  if (!f) return;
  await f.page.route("**/api/peer/sessions/s/advice", (r) =>
    r.fulfill({ json: { status: "unavailable", advice: [] } }),
  );
  f.state.status = "cancelled";
  f.state.currentRound = null;
  await f.page.evaluate(() => window.peerTicks[0]());
  await f.page.waitForFunction(
    () =>
      document.getElementById("peer-title").textContent ===
      "Practice cancelled",
  );
  assert.match(
    await f.page.locator("#peer-error").textContent(),
    /cancelled\. Start another from the lobby/,
  );
});

test("confirming the swap shows that the partner is awaited", async (t) => {
  const feedback = {
    authorSlot: 0,
    submitted: true,
    payload: { dimensions: {} },
  };
  const f = await fixture(t, "candidate", false, { feedback });
  if (!f) return;
  let advances = 0;
  await f.page.route("**/api/peer/sessions/s/advance", (r) => {
    advances++;
    return r.fulfill({ json: {} });
  });
  await f.page.evaluate(() => window.peerTicks[0]());
  await f.page.waitForFunction(
    () => !document.getElementById("peer-advance").disabled,
  );
  await f.page.locator("#peer-advance").click();
  await f.page.waitForFunction(
    () =>
      document.getElementById("peer-advance").textContent ===
      "Waiting for your partner to confirm",
  );
  // The next poll must keep waiting, not offer the button again.
  // A tick that lands while the click's own refresh is running does nothing,
  // so keep ticking until a poll actually goes out.
  let polled = 0;
  f.page.on("response", (r) => {
    if (r.url().endsWith("/api/peer/sessions/s")) polled++;
  });
  await until(async () => {
    await f.page.evaluate(() => window.peerTicks[0]());
    return polled > 0;
  }, "a session poll after the click");
  // The response arriving is not the handler finishing; give it a turn.
  await f.page.evaluate(() => new Promise((done) => setTimeout(done, 200)));
  assert.equal(await f.page.locator("#peer-advance").isDisabled(), true);
  assert.equal(
    await f.page.locator("#peer-advance").textContent(),
    "Waiting for your partner to confirm",
  );
  assert.equal(advances, 1);
});

test("a partner who submits after the practice ended is still shown", async (t) => {
  const f = await fixture(t, "interviewer");
  if (!f) return;
  await f.page.route("**/api/peer/sessions/s/advice", (r) =>
    r.fulfill({ json: { status: "processing", advice: [] } }),
  );
  let partner = false;
  await f.page.route("**/api/peer/sessions/s/rounds/0/feedback", (r) =>
    r.fulfill({
      json: {
        feedback: partner
          ? [
              {
                authorSlot: 1,
                submitted: true,
                payload: {
                  dimensions: { reasoning: { score: 2, example: "Late note" } },
                },
              },
            ]
          : [],
      },
    }),
  );
  f.state.status = "complete";
  f.state.currentRound.status = "feedback";
  await f.page.evaluate(() => window.peerTicks[0]());
  await f.page.waitForFunction(() => window.watchTicks.length === 1);
  partner = true;
  await f.page.evaluate(() => window.watchTicks[0]());
  await f.page.waitForFunction(() =>
    document
      .getElementById("peer-feedback-results")
      .textContent.includes("Late note"),
  );
});

test("the late-feedback watch spends nothing while hidden and reads one at a time", async (t) => {
  const f = await fixture(t, "interviewer");
  if (!f) return;
  await f.page.route("**/api/peer/sessions/s/advice", (r) =>
    r.fulfill({ json: { status: "processing", advice: [] } }),
  );
  let reads = 0;
  let hold = false;
  let release;
  const gate = new Promise((done) => (release = done));
  await f.page.route("**/api/peer/sessions/s/rounds/0/feedback", async (r) => {
    reads++;
    if (hold) await gate;
    await r.fulfill({ json: { feedback: [] } });
  });
  f.state.status = "complete";
  f.state.currentRound.status = "feedback";
  await f.page.evaluate(() => window.peerTicks[0]());
  await f.page.waitForFunction(() => window.watchTicks.length === 1);
  const baseline = reads;
  await f.page.evaluate(async () => {
    Object.defineProperty(document, "hidden", {
      configurable: true,
      get: () => true,
    });
    for (let i = 0; i < 70; i++) await window.watchTicks[0]();
    Object.defineProperty(document, "hidden", {
      configurable: true,
      get: () => false,
    });
  });
  assert.equal(reads, baseline, "a hidden tab reads nothing");
  hold = true;
  await f.page.evaluate(() => {
    void window.watchTicks[0]();
    void window.watchTicks[0]();
  });
  await until(async () => reads === baseline + 1, "one read in flight");
  // Outwaits a second request that would already have left the page.
  await f.page.evaluate(() => new Promise((done) => setTimeout(done, 200)));
  assert.equal(reads, baseline + 1, "a second read waits for the first");
  release();
  hold = false;
  await until(async () => {
    await f.page.evaluate(() => window.watchTicks[0]());
    return reads > baseline + 1;
  }, "a read after the first finished, and a budget that hidden ticks left");
});
