import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  read,
  startStaticServer,
} from "./source.js";

const html = read("web/interview.html");
let browser = null;
let server = null;
let base = "";

before(async () => {
  browser = await launchChromium({
    args: [
      "--use-fake-device-for-media-stream",
      "--use-fake-ui-for-media-stream",
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
  const start = html.indexOf('id="audio-check"');
  assert.ok(start >= 0, "the preflight section must exist");
  const end = html.indexOf("</section>", start);
  assert.ok(end > start, "the preflight section must have a closing boundary");
  const preflight = html.slice(start, end);
  for (const [id, choices] of [
    ["interviewer-voice", ["", "Puck", "Charon", "Kore", "Orus", "Random"]],
    [
      "interviewer-accent",
      ["", "American", "British", "Australian", "Canadian", "Indian", "Random"],
    ],
  ]) {
    const select = preflight.match(
      new RegExp(`<select id="${id}">([\\s\\S]*?)</select>`),
    );
    assert.ok(select);
    assert.deepEqual(
      [...select[1].matchAll(/<option value="([^"]*)">/g)].map(
        ([, value]) => value,
      ),
      choices,
    );
  }
});

// Capture the real request after the media gate, then stop before LiveKit.
// Each page owns its context so one interview cannot supply another's choice.
const selections = [
  ...["", "Puck", "Charon", "Kore", "Orus", "Random"].map((voice) => [
    voice,
    "",
  ]),
  ...["American", "British", "Australian", "Canadian", "Indian", "Random"].map(
    (accent) => ["", accent],
  ),
  ["Charon", "Indian"],
  ["Random", "British"],
  ["Kore", "Random"],
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
      await select.selectOption("Kore");
      await select.selectOption(voice);
      const accentSelect = page.getByLabel("English accent", { exact: true });
      assert.equal(await accentSelect.inputValue(), "");
      await accentSelect.selectOption("British");
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
