import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { launchChromium, startStaticServer } from "./source.js";
let browser, server, base;
before(async () => {
  browser = await launchChromium();
  if (browser) ({ server, base } = await startStaticServer());
});
after(async () => {
  await browser?.close();
  await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
});
test("completed practice shows member advice and only the chosen saved artifacts", async (t) => {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  let transcripts = 0,
    audioReads = 0;
  let transcriptDeleted = false;
  const state = {
    sessionId: "done",
    status: "complete",
    version: 10,
    serverNow: Math.floor(Date.now() / 1000),
    you: { slot: 0 },
    members: [
      { slot: 0, login: "alice", nickname: "Lucky7" },
      { slot: 1, login: "bob", nickname: "Runner2" },
    ],
    settings: { saveTranscript: true, saveAudio: false },
    currentRound: null,
  };
  await page.route("**/api/peer/sessions/done", (route) =>
    route.fulfill({ json: state }),
  );
  await page.route("**/api/peer/sessions/done/advice", (route) =>
    route.fulfill({
      json: {
        status: "ready",
        advice: [
          {
            roundIndex: 0,
            role: "candidate",
            strengths: ["Explained the invariant."],
            improvements: ["Try a boundary case."],
            evidenceRefs: [{ segmentIndex: 2, slot: 0, roundIndex: 0 }],
          },
        ],
      },
    }),
  );
  await page.route("**/api/peer/sessions/done/transcript", (route) => {
    if (route.request().method() === "DELETE") {
      transcriptDeleted = true;
      return route.fulfill({ json: { status: "deleted" } });
    }
    transcripts++;
    return route.fulfill({
      json: {
        status: "ready",
        segments: [
          {
            slot: 0,
            roundIndex: 0,
            startedAt: 1,
            endedAt: 2,
            text: "Consider an empty input.",
          },
        ],
      },
    });
  });
  await page.route("**/api/peer/sessions/done/audio/status", (route) => {
    audioReads++;
    return route.fulfill({ json: { status: "ready" } });
  });
  await page.goto(`${base}/peer-interview.html?session=done`);
  await page.locator("#peer-advice-panel").waitFor({ state: "visible" });
  await page.waitForFunction(() =>
    document
      .getElementById("peer-advice")
      .textContent.includes("Explained the invariant."),
  );
  assert.match(
    await page.locator("#peer-advice").textContent(),
    /Try a boundary case/,
  );
  assert.match(
    await page.locator("#peer-advice").textContent(),
    /Observed examples[\s\S]*conversation moment 3/,
  );
  assert.equal(
    await page.locator("#peer-download-transcript").isVisible(),
    true,
  );
  assert.equal(await page.locator("#peer-download-audio").isHidden(), true);
  assert.equal(transcripts, 0);
  assert.equal(audioReads, 0);
  const downloaded = page.waitForEvent("download");
  await page.locator("#peer-download-transcript").click();
  const download = await downloaded;
  assert.match(download.suggestedFilename(), /transcript.*\.txt$/);
  assert.equal(transcripts, 1);
  await page.locator("#peer-withdraw-transcript").click();
  await page.locator("#peer-download-transcript").waitFor({ state: "hidden" });
  assert.equal(transcriptDeleted, true);
});

test("audio-only choice exposes member download and withdrawal without fetching transcript", async (t) => {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  let deleted = false,
    transcripts = 0;
  const state = {
    sessionId: "audio",
    status: "complete",
    version: 10,
    serverNow: Math.floor(Date.now() / 1000),
    you: { slot: 0 },
    members: [
      { slot: 0, login: "alice" },
      { slot: 1, login: "bob" },
    ],
    settings: { saveTranscript: false, saveAudio: true },
    currentRound: null,
  };
  await page.route("**/api/peer/sessions/audio", (route) =>
    route.fulfill({ json: state }),
  );
  await page.route("**/api/peer/sessions/audio/advice", (route) =>
    route.fulfill({ json: { status: "unavailable", advice: [] } }),
  );
  await page.route("**/api/peer/sessions/audio/transcript", (route) => {
    transcripts++;
    return route.fulfill({ status: 404, json: {} });
  });
  await page.route("**/api/peer/sessions/audio/audio/status", (route) =>
    route.fulfill({ json: { status: deleted ? "deleted" : "ready" } }),
  );
  await page.route("**/api/peer/sessions/audio/audio", (route) => {
    assert.equal(route.request().method(), "DELETE");
    deleted = true;
    return route.fulfill({ json: { status: "deleted" } });
  });
  await page.goto(`${base}/peer-interview.html?session=audio`);
  await page.locator("#peer-download-audio").waitFor({ state: "visible" });
  assert.equal(
    await page.locator("#peer-download-transcript").isHidden(),
    true,
  );
  assert.equal(
    await page.locator("#peer-download-audio").getAttribute("href"),
    "/api/peer/sessions/audio/audio",
  );
  assert.match(await page.locator("#peer-advice").textContent(), /unavailable/);
  await page.locator("#peer-withdraw-audio").click();
  await page.locator("#peer-download-audio").waitFor({ state: "hidden" });
  assert.equal(deleted, true);
  assert.equal(transcripts, 0);
});

test("expired practice artifacts say so instead of offering a retry", async (t) => {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.route("**/api/peer/sessions/gone", (route) =>
    route.fulfill({
      json: {
        sessionId: "gone",
        status: "complete",
        version: 10,
        serverNow: Math.floor(Date.now() / 1000),
        you: { slot: 0 },
        members: [
          { slot: 0, login: "alice", nickname: "Lucky7" },
          { slot: 1, login: "bob", nickname: "Runner2" },
        ],
        settings: { saveTranscript: true, saveAudio: true },
        currentRound: null,
      },
    }),
  );
  await page.route("**/api/peer/sessions/gone/advice", (route) =>
    route.fulfill({ status: 404, json: { code: "not_found" } }),
  );
  await page.route("**/api/peer/sessions/gone/audio/status", (route) =>
    route.fulfill({ status: 404, json: { code: "not_found" } }),
  );
  await page.goto(`${base}/peer-interview.html?session=gone`);
  await page.waitForFunction(() =>
    document.getElementById("peer-advice").textContent.includes("have expired"),
  );
  assert.equal(
    await page.locator("#peer-download-transcript").isVisible(),
    false,
  );
  assert.equal(await page.locator("#peer-advice-retry").isVisible(), false);
});

function transcriptFixture(id, transcript) {
  return async (t) => {
    if (!browser) {
      t.skip("playwright chromium unavailable");
      return null;
    }
    const page = await browser.newPage();
    t.after(() => page.close());
    await page.route(`**/api/peer/sessions/${id}`, (route) =>
      route.fulfill({
        json: {
          sessionId: id,
          status: "complete",
          version: 10,
          serverNow: Math.floor(Date.now() / 1000),
          you: { slot: 0 },
          members: [
            { slot: 0, login: "alice" },
            { slot: 1, login: "bob" },
          ],
          settings: { saveTranscript: true, saveAudio: false },
          currentRound: null,
        },
      }),
    );
    await page.route(`**/api/peer/sessions/${id}/advice`, (route) =>
      route.fulfill({ json: { status: "ready", advice: [] } }),
    );
    await page.route(`**/api/peer/sessions/${id}/transcript`, (route) =>
      route.fulfill(transcript),
    );
    await page.goto(`${base}/peer-interview.html?session=${id}`);
    await page.locator("#peer-download-transcript").waitFor();
    return page;
  };
}

test("a withdrawn transcript drops only its own controls", async (t) => {
  const page = await transcriptFixture("withdrawn", {
    status: 404,
    json: { code: "not_found" },
  })(t);
  if (!page) return;
  const refreshed = page.waitForResponse((r) => r.url().endsWith("/advice"));
  await page.locator("#peer-download-transcript").click();
  await page.waitForFunction(() =>
    document
      .getElementById("peer-transcript-note")
      .textContent.includes("no longer available"),
  );
  await refreshed;
  assert.equal(
    await page.locator("#peer-download-transcript").isVisible(),
    false,
  );
  assert.equal(
    await page
      .locator("#peer-advice")
      .textContent()
      .then((x) => x.includes("expired")),
    false,
  );
});

test("a transcript that expired before download says so and drops every control", async (t) => {
  const page = await transcriptFixture("late", {
    status: 404,
    json: { code: "not_found" },
  })(t);
  if (!page) return;
  await page.route("**/api/peer/sessions/late/advice", (route) =>
    route.fulfill({ status: 404, json: { code: "not_found" } }),
  );
  await page.locator("#peer-download-transcript").click();
  await page.waitForFunction(() =>
    document.getElementById("peer-advice").textContent.includes("have expired"),
  );
  for (const id of [
    "peer-download-transcript",
    "peer-withdraw-transcript",
    "peer-advice-retry",
  ])
    assert.equal(await page.locator(`#${id}`).isVisible(), false, id);
});

test("an unavailable transcript with no segments offers no blank download", async (t) => {
  const page = await transcriptFixture("blank", {
    json: { status: "unavailable", segments: [] },
  })(t);
  if (!page) return;
  let downloads = 0;
  page.on("download", () => downloads++);
  await page.locator("#peer-download-transcript").click();
  await page.waitForFunction(() =>
    document
      .getElementById("peer-transcript-note")
      .textContent.includes("No transcript is available"),
  );
  assert.equal(
    await page.locator("#peer-download-transcript").isVisible(),
    false,
  );
  assert.equal(downloads, 0);
});
