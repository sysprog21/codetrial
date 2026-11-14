import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  read,
  FAKE_SPEECH_CAPTURE,
  startStaticServer,
} from "./source.js";

const html = read("web/interview.html");

function preflightChoices(id) {
  const start = html.indexOf('id="audio-check"');
  assert.ok(start >= 0, "the preflight section must exist");
  const end = html.indexOf("</section>", start);
  assert.ok(end > start, "the preflight section must have a closing boundary");
  const preflight = html.slice(start, end);
  const select = preflight.match(
    new RegExp(`<select id="${id}">([\\s\\S]*?)</select>`),
  );
  assert.ok(select, `${id} must be in the preflight section`);
  return [...select[1].matchAll(/<option value="([^"]*)">/g)].map(
    ([, value]) => value,
  );
}

const voiceChoices = preflightChoices("interviewer-voice");
const accentChoices = preflightChoices("interviewer-accent");
const namedVoice = voiceChoices.find((value) => value && value !== "Random");
const namedAccent = accentChoices.find((value) => value && value !== "Random");
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
    runtimeConfig: DEFAULT_RUNTIME_CONFIG,
  }));
});

after(async () => {
  await browser?.close();
  await new Promise((closed) => (server ? server.close(closed) : closed()));
});

test("voice and accent choices are separate preflight controls", () => {
  for (const choices of [voiceChoices, accentChoices]) {
    assert.equal(choices[0], "");
    assert.equal(choices.at(-1), "Random");
    assert.equal(new Set(choices).size, choices.length);
    assert.ok(choices.length > 2, "each selector must offer a named choice");
  }
});

// Capture the real request after the media gate, then stop before LiveKit.
// Each page owns its context so one interview cannot supply another's choice.
const selections = [
  ...voiceChoices.map((voice) => [voice, ""]),
  ...accentChoices.slice(1).map((accent) => ["", accent]),
  [namedVoice, namedAccent],
  ["Random", namedAccent],
  [namedVoice, "Random"],
  ["Random", "Random"],
];
for (const [voice, accent] of selections) {
  test(`the preflight sends voice ${voice || "Default"} and accent ${accent || "Default"}`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await browser.newPage();
    try {
      await page.route("**/api/token", (route) =>
        route.fulfill({
          status: 503,
          contentType: "application/json",
          body: JSON.stringify({
            error: "Token request captured by the test.",
          }),
        }),
      );
      await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
        waitUntil: "domcontentloaded",
      });
      const select = page.getByLabel("Jim's voice", { exact: true });
      assert.equal(await select.inputValue(), "");
      // Returning to Default must clear an explicit override too.
      await select.selectOption(namedVoice);
      await select.selectOption(voice);
      const accentSelect = page.getByLabel("English accent", { exact: true });
      assert.equal(await accentSelect.inputValue(), "");
      await accentSelect.selectOption(namedAccent);
      await accentSelect.selectOption(accent);
      assert.equal(await select.inputValue(), voice);
      await page.locator("#audio-heard").click();
      await page.locator("#camera-skip").click();
      await page.waitForFunction(
        () => !document.querySelector("#audio-check-join").disabled,
      );
      const sent = page.waitForRequest((request) =>
        request.url().endsWith("/api/token"),
      );
      await page.locator("#audio-check-join").click();
      const request = await sent;
      assert.equal(request.method(), "POST");
      const body = request.postDataJSON();
      if (voice) assert.equal(body.interviewerVoice, voice);
      else assert.equal(Object.hasOwn(body, "interviewerVoice"), false);
      if (accent) assert.equal(body.interviewerAccent, accent);
      else assert.equal(Object.hasOwn(body, "interviewerAccent"), false);
    } finally {
      await page.close();
    }
  });
}

test("the interviewer controls fit narrow and desktop preflights", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    for (const width of [320, 360, 480, 481, 768, 1280]) {
      await page.setViewportSize({ width, height: 800 });
      await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
        waitUntil: "domcontentloaded",
      });
      const bounds = await page
        .locator(".interviewer-style-controls")
        .evaluate((controls) => {
          const rect = controls.getBoundingClientRect();
          const selects = [...controls.querySelectorAll("select")];
          const [voice, accent] = selects.map((select) =>
            select.getBoundingClientRect(),
          );
          return {
            count: selects.length,
            stacked: accent.top >= voice.bottom,
            sideBySide:
              Math.abs(voice.top - accent.top) < 1 &&
              accent.left >= voice.right,
            left: rect.left,
            right: rect.right,
            fits: selects.every((select) => {
              const box = select.getBoundingClientRect();
              return (
                box.width > 0 &&
                box.left >= rect.left &&
                box.right <= rect.right
              );
            }),
          };
        });
      assert.equal(bounds.count, 2);
      assert.ok(
        width <= 480 ? bounds.stacked : bounds.sideBySide,
        `controls must ${width <= 480 ? "stack" : "stay side by side"} at ${width}px`,
      );
      assert.ok(
        bounds.left >= 0 && bounds.right <= width && bounds.fits,
        `controls overflow at ${width}px`,
      );
    }
  } finally {
    await page.close();
  }
});

// A URL that skips the lobby names no length, so the server's configured
// default decides; sending the provisional 45 would override it.
for (const [query, sent] of [
  ["", false],
  // Neither names a usable length, so neither may override the server default.
  ["&duration=", false],
  ["&duration=abc", false],
  ["&duration=0", false],
  ["&duration=-5", false],
  // Trailing text is not a number, so it names no length either, and neither
  // is a number that is not finite.
  ["&duration=30abc", false],
  ["&duration=Infinity", false],
  ["&duration=60", 60],
  // The agent reads whole minutes, so the page asks for the same.
  ["&duration=42.8", 42],
]) {
  test(`the token request ${sent ? "names" : "omits"} the length for ${query || "a URL without duration"}`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await browser.newPage();
    try {
      await page.route("**/api/token", (route) =>
        route.fulfill({
          status: 503,
          contentType: "application/json",
          body: JSON.stringify({
            error: "Token request captured by the test.",
          }),
        }),
      );
      await page.goto(
        `${base}/interview.html?problem=chargeback-pair-match${query}`,
        { waitUntil: "domcontentloaded" },
      );
      await page.locator("#audio-heard").click();
      await page.locator("#camera-skip").click();
      await page.waitForFunction(
        () => !document.querySelector("#audio-check-join").disabled,
      );
      const request = page.waitForRequest((r) =>
        r.url().endsWith("/api/token"),
      );
      await page.locator("#audio-check-join").click();
      const body = (await request).postDataJSON();
      assert.equal(Object.hasOwn(body, "durationMin"), sent !== false);
      if (sent !== false) assert.equal(body.durationMin, sent);
    } finally {
      await page.close();
    }
  });
}

// The round plan is drawn from the length, so a URL with no `duration` waits
// for the server's default instead of showing 45 minutes and jumping.
for (const query of ["", "&duration=", "&duration=abc"]) {
  test(`the round plan for ?${query || "no duration"} waits for the server default`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await browser.newPage();
    try {
      let release;
      const held = new Promise((resolve) => (release = resolve));
      await page.route("**/api/session", async (route) => {
        await held;
        await route.fulfill({
          json: { signedIn: false, defaultDurationMin: 30, maxDurationMin: 90 },
        });
      });
      await page.goto(
        `${base}/interview.html?problem=chargeback-pair-match&loop=coding_only${query}`,
        { waitUntil: "domcontentloaded" },
      );
      const plan = page.locator("#round-plan-summary");
      // `renderProblem` writes the title in the same task as `bindEvents`; the
      // static `#audio-check` is there before either has run.
      await page.waitForFunction(
        () =>
          document.querySelector("#problem-title").textContent !==
          "Loading interview...",
      );
      assert.equal(await plan.textContent(), "");
      release();
      await page.waitForFunction(() =>
        /30 minute coding budget/.test(
          document.querySelector("#round-plan-summary").textContent,
        ),
      );
    } finally {
      await page.close();
    }
  });
}

// With no answer from the server either, the plan falls back to the 45 minutes
// a URL without a length shows. `-5` once read as 10 here, because the length
// shown was parsed apart from the decision that it named none.
for (const query of [
  "&duration=-5",
  "&duration=0",
  "&duration=30abc",
  "&duration=Infinity",
]) {
  test(`the round plan for ?${query} falls back to 45 when the session fails`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await browser.newPage();
    try {
      await page.route("**/api/session", (route) =>
        route.fulfill({ status: 500, body: "nope" }),
      );
      await page.goto(
        `${base}/interview.html?problem=chargeback-pair-match&loop=coding_only${query}`,
        { waitUntil: "domcontentloaded" },
      );
      await page.waitForFunction(() =>
        /45 minute coding budget/.test(
          document.querySelector("#round-plan-summary").textContent,
        ),
      );
    } finally {
      await page.close();
    }
  });
}

test("the round plan for a URL with a duration is drawn at once", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.route("**/api/session", (route) =>
      route.fulfill({
        json: { signedIn: false, defaultDurationMin: 30, maxDurationMin: 90 },
      }),
    );
    await page.goto(
      `${base}/interview.html?problem=chargeback-pair-match&loop=coding_only&duration=60`,
      { waitUntil: "domcontentloaded" },
    );
    await page.waitForFunction(() =>
      /60 minute coding budget/.test(
        document.querySelector("#round-plan-summary").textContent,
      ),
    );
  } finally {
    await page.close();
  }
});
