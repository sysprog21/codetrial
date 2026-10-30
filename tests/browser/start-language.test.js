import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  FAKE_SPEECH_CAPTURE,
  startStaticServer,
} from "./source.js";
import { LANGUAGE_KEY } from "../../web/lib.js";

// C is offered only once the judge proves the problem function-style, so a
// compiled runner has to be on for the requested tab to exist at all.
const CE_RUNTIME_CONFIG = DEFAULT_RUNTIME_CONFIG.replace(
  "ENABLED = false",
  "ENABLED = true",
).replace('BASE_URL = ""', 'BASE_URL = "https://godbolt.org"');
let browser = null;
let server = null;
let base = "";

before(async () => {
  browser = await launchChromium({
    args: [
      "--use-fake-device-for-media-stream",
      "--use-fake-ui-for-media-stream",
      FAKE_SPEECH_CAPTURE,
      "--autoplay-policy=no-user-gesture-required",
    ],
  });
  if (!browser) return;
  ({ server, base } = await startStaticServer({
    runtimeConfig: CE_RUNTIME_CONFIG,
  }));
});

after(async () => {
  await browser?.close();
  await new Promise((closed) => (server ? server.close(closed) : closed()));
});

/// Joins through the preflight and answers the token request's body. The
/// token is refused unless `granted`, which answers it with an empty grant:
/// enough for the page to count the interview as started, not to join a room.
/// `within` bounds only the wait from the Start press to the token request;
/// the preflight before it keeps the page's own timeout.
async function joinAndCaptureToken(page, { granted = false, within } = {}) {
  await page.route("**/api/token", (route) =>
    route.fulfill(
      granted
        ? { status: 200, contentType: "application/json", body: "{}" }
        : {
            status: 503,
            contentType: "application/json",
            body: JSON.stringify({
              error: "Token request captured by the test.",
            }),
          },
    ),
  );
  await page.locator("#audio-heard").click();
  await page.locator("#camera-skip").click();
  await page.waitForFunction(
    () => !document.querySelector("#audio-check-join").disabled,
  );
  const sent = page.waitForRequest(
    (request) => request.url().endsWith("/api/token"),
    { timeout: within },
  );
  const [request] = await Promise.all([
    sent,
    page.locator("#audio-check-join").click(),
  ]);
  return request;
}

/// Past `init()`, which runs after the problem loads and so after
/// `domcontentloaded`: until then both menus show the markup's Python, and a
/// pick made before it is overwritten. The placeholder is written once the
/// opening language is set and the menus have their handlers.
const interviewReady = (page) =>
  page.waitForFunction(
    () => document.querySelector("#candidate-case-input").placeholder !== "",
  );

test("the token names a requested C whose judge loads after Start", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  let releaseJudge = () => {};
  try {
    const held = new Promise((resolve) => {
      releaseJudge = resolve;
    });
    await page.route("**/judges/*.json", async (route) => {
      await held;
      await route.continue();
    });
    await page.goto(
      `${base}/interview.html?problem=chargeback-pair-match&language=c`,
      { waitUntil: "domcontentloaded" },
    );
    let tokenSent = false;
    page.on("request", (request) => {
      if (request.url().endsWith("/api/token")) tokenSent = true;
    });
    const request = joinAndCaptureToken(page);
    await page.waitForFunction(
      () => document.querySelector("#audio-check").hidden,
    );
    assert.equal(tokenSent, false, "the token waits for the judge");
    releaseJudge();
    assert.equal((await request).postDataJSON().language, "c");
    // The token was refused, so no interview started in C.
    await page.waitForFunction(
      () => !document.querySelector("#audio-check-join").disabled,
    );
    assert.equal(
      await page.evaluate((key) => localStorage.getItem(key), LANGUAGE_KEY),
      null,
    );
  } finally {
    releaseJudge();
    await page.close();
  }
});

/// Holds every judge request until the returned function is called, so a
/// test can act while the page is still waiting for one.
async function holdJudges(page) {
  let release = () => {};
  const held = new Promise((resolve) => {
    release = resolve;
  });
  await page.route("**/judges/*.json", async (route) => {
    await held;
    await route.continue();
  });
  return () => release();
}

for (const settle of ["judge", "timeout"]) {
  test(`Practice End during the ${settle} language wait never requests a token`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await browser.newPage();
    const releaseJudge = await holdJudges(page);
    let tokens = 0;
    await page.addInitScript(() => {
      window.startupTokenRequests = 0;
      const fetch = window.fetch.bind(window);
      window.fetch = (...args) => {
        if (args[0] === "/api/token") window.startupTokenRequests++;
        return fetch(...args);
      };
    });
    await page.route("**/api/token", (route) => {
      tokens++;
      return route.fulfill({
        status: 503,
        json: { error: "No token may be requested after End." },
      });
    });
    try {
      await page.goto(
        `${base}/interview.html?problem=chargeback-pair-match&language=c`,
        { waitUntil: "domcontentloaded" },
      );
      await page.locator("#audio-heard").click();
      await page.locator("#camera-skip").click();
      await page.waitForFunction(
        () => !document.querySelector("#audio-check-join").disabled,
      );
      await page.clock.install({ time: new Date("2030-01-01T00:00:00Z") });
      // End must win before the language timeout, even on a stalled runner.
      await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
      await page.locator("#audio-check-join").click();
      await page.waitForFunction(
        () => document.querySelector("#audio-check").hidden,
      );
      assert.equal(tokens, 0);
      const timer = await page.locator("#timer").textContent();
      await page.locator("#end").click();
      await page.clock.resume();
      await page.waitForFunction(
        () => !document.querySelector("#report-modal").hidden,
      );
      if (settle === "judge") {
        releaseJudge();
        await page.waitForFunction(
          () =>
            !document.querySelector('#language-select option[value="c"]')
              .disabled,
        );
      }
      await page.clock.fastForward(5_001);
      assert.equal(await page.evaluate(() => window.startupTokenRequests), 0);
      assert.equal(tokens, 0);
      assert.equal(await page.locator("#ending-overlay").isVisible(), false);
      assert.equal(await page.locator("#timer").textContent(), timer);
      assert.match(
        await page.locator("#report-modal").textContent(),
        /No interviewer joined and this session produced no evaluation/,
      );
    } finally {
      releaseJudge();
      await page.close();
    }
  });
}

for (const [query, expected] of [
  ["", "python"],
  ["&language=java", "java"],
]) {
  test(`a ${expected} start neither waits for nor depends on a pending judge`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await browser.newPage();
    const releaseJudge = await holdJudges(page);
    try {
      await page.goto(
        `${base}/interview.html?problem=chargeback-pair-match${query}`,
        { waitUntil: "domcontentloaded" },
      );
      // The placeholder waits for the judge; the starter buffer does not.
      await page.waitForFunction(
        () => document.querySelector("#editor").value !== "",
      );
      // Only C needs the judge, so the tab is already the one asked for.
      assert.equal(
        await page.locator("#language-select").inputValue(),
        expected,
      );
      // Under the 5 s a requested C waits, so a start that waited cannot
      // pass. Only the press is bounded: a slow runner stalling the preflight
      // clicks says nothing about the judge.
      const body = (
        await joinAndCaptureToken(page, { within: 4_000 })
      ).postDataJSON();
      assert.equal(body.language, expected);
    } finally {
      releaseJudge();
      await page.close();
    }
  });
}

test("a judge slower than the wait does not hold the start", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  let releaseJudge = () => {};
  try {
    const held = new Promise((resolve) => {
      releaseJudge = resolve;
    });
    await page.route("**/judges/*.json", async (route) => {
      await held;
      await route.continue();
    });
    await page.goto(
      `${base}/interview.html?problem=chargeback-pair-match&language=c`,
      { waitUntil: "domcontentloaded" },
    );
    // C stays unconfirmed, so the interview starts in the fallback.
    const body = (await joinAndCaptureToken(page)).postDataJSON();
    assert.equal(body.language, "python");
    releaseJudge();
    await page.waitForFunction(() =>
      [
        ...document.querySelectorAll('#language-select option[value="c"]'),
      ].every((option) => !option.disabled),
    );
    assert.equal(await page.locator("#language-select").inputValue(), "python");
  } finally {
    releaseJudge();
    await page.close();
  }
});

test("a started interview remembers its language", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(
      `${base}/interview.html?problem=chargeback-pair-match&language=java`,
      { waitUntil: "domcontentloaded" },
    );
    await interviewReady(page);
    const body = (
      await joinAndCaptureToken(page, { granted: true })
    ).postDataJSON();
    assert.equal(body.language, "java");
    await page.waitForFunction(
      (key) => localStorage.getItem(key) === "java",
      LANGUAGE_KEY,
    );
  } finally {
    await page.close();
  }
});

test("a whiteboard token carries no language", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(
      `${base}/interview.html?problem=chargeback-pair-match&mode=whiteboard`,
      { waitUntil: "domcontentloaded" },
    );
    const body = (await joinAndCaptureToken(page)).postDataJSON();
    assert.equal(Object.hasOwn(body, "language"), false);
  } finally {
    await page.close();
  }
});

/// What the interview page wrote after the candidate's last interview.
const rememberLanguage = (page, language) =>
  page.addInitScript(
    ([key, value]) => localStorage.setItem(key, value),
    [LANGUAGE_KEY, language],
  );

/// The two menus that name the language: the preflight's and the editor's.
const shown = (page) =>
  page.evaluate(() => [
    document.querySelector("#preflight-language").value,
    document.querySelector("#language-select").value,
  ]);

test("the preflight preselects the language last interviewed in", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await rememberLanguage(page, "javascript");
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await interviewReady(page);
    assert.deepEqual(await shown(page), ["javascript", "javascript"]);
    const body = (await joinAndCaptureToken(page)).postDataJSON();
    assert.equal(body.language, "javascript");
  } finally {
    await page.close();
  }
});

test("a language the lobby asked for wins over the remembered one", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await rememberLanguage(page, "javascript");
    await page.goto(
      `${base}/interview.html?problem=chargeback-pair-match&language=java`,
      { waitUntil: "domcontentloaded" },
    );
    await interviewReady(page);
    assert.deepEqual(await shown(page), ["java", "java"]);
  } finally {
    await page.close();
  }
});

test("the language picked before joining is the one the token names", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await interviewReady(page);
    // Python with nothing remembered, so the pick below is a change.
    assert.deepEqual(await shown(page), ["python", "python"]);
    await page.selectOption("#preflight-language", "java");
    assert.deepEqual(await shown(page), ["java", "java"]);
    const body = (await joinAndCaptureToken(page)).postDataJSON();
    assert.equal(body.language, "java");
  } finally {
    await page.close();
  }
});

test("the preflight withholds C from a class problem", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await rememberLanguage(page, "c");
    await page.goto(`${base}/interview.html?problem=backend-pool-sampler`, {
      waitUntil: "domcontentloaded",
    });
    await page.waitForFunction(() =>
      document
        .querySelector('#preflight-language option[value="c"]')
        .textContent.includes("class problem"),
    );
    assert.equal(
      await page
        .locator('#preflight-language option[value="c"]')
        .evaluate((option) => option.disabled),
      true,
    );
    assert.deepEqual(await shown(page), ["python", "python"]);
  } finally {
    await page.close();
  }
});

test("a whiteboard preflight offers no language", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(
      `${base}/interview.html?problem=chargeback-pair-match&mode=whiteboard`,
      { waitUntil: "domcontentloaded" },
    );
    await page.waitForFunction(
      () => document.querySelector("#audio-step-language").hidden,
    );
    assert.equal(await page.locator("#preflight-language").isVisible(), false);
  } finally {
    await page.close();
  }
});
