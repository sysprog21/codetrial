import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { SUPPORTED_MODELS } from "../../web/avatar/model.js";
import {
  DEFAULT_RUNTIME_CONFIG,
  FAKE_SPEECH_CAPTURE,
  launchChromium,
  startStaticServer,
} from "./source.js";

let browser;
let server;
let base;

before(async () => {
  browser = await launchChromium({
    args: [
      "--use-fake-device-for-media-stream",
      "--use-fake-ui-for-media-stream",
      FAKE_SPEECH_CAPTURE,
      "--autoplay-policy=no-user-gesture-required",
    ],
  });
  if (browser)
    ({ server, base } = await startStaticServer({
      runtimeConfig: DEFAULT_RUNTIME_CONFIG,
    }));
});
after(async () => {
  await browser?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
});

test("appearance descriptions suggest voices without changing any voice choice", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
  });
  const requested = [];
  await page.route("https://raw.githubusercontent.com/**", async (route) => {
    requested.push(route.request().url());
    await route.abort();
  });
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    const avatar = page.getByLabel("Jim's appearance", { exact: true });
    const voice = page.getByLabel("Jim's voice", { exact: true });
    await page.waitForFunction(
      () => document.querySelector("#interviewer-avatar").options.length === 2,
    );
    assert.deepEqual(
      await avatar
        .locator("option")
        .evaluateAll((options) => options.map((option) => option.textContent)),
      ["Jim (younger engineer, default)", "Jim (middle-aged senior engineer)"],
    );
    assert.equal(await avatar.inputValue(), "jim");
    assert.equal(await voice.inputValue(), "");
    const hint = page.locator("#avatar-voice-suggestion");
    assert.equal(await hint.getAttribute("role"), "status");
    assert.equal(
      await avatar.getAttribute("aria-describedby"),
      "avatar-voice-suggestion",
    );
    await page.waitForFunction(() =>
      document
        .querySelector("#avatar-voice-suggestion")
        .textContent.includes("Puck"),
    );
    for (const choice of ["", "Random", "Kore", "Orus", "Puck", "Charon"]) {
      await voice.selectOption(choice);
      for (const [id, suggested] of [
        ["jim-business-male-04", "Charon"],
        ["jim", "Puck"],
      ]) {
        await avatar.selectOption(id);
        await page.waitForFunction(
          (suggested) =>
            document.querySelector("#avatar-voice-suggestion").textContent ===
            `Suggested voice: ${suggested}. You can choose any voice.`,
          suggested,
        );
        assert.equal(await voice.inputValue(), choice);
      }
    }
    assert.deepEqual(requested, [], "suggestions do not download a model");
  } finally {
    await page.close();
  }
});

for (const model of SUPPORTED_MODELS) {
  test(`preflight commits ${model.id} without changing the voice`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await browser.newPage({
      viewport: { width: 1440, height: 1000 },
    });
    const requested = [];
    await page.route("https://raw.githubusercontent.com/**", async (route) => {
      requested.push(route.request().url());
      await route.fulfill({
        status: 404,
        body: "Model unavailable in this fixture",
      });
    });
    await page.route("**/api/token", (route) =>
      route.fulfill({
        status: 503,
        contentType: "application/json",
        body: JSON.stringify({ error: "Token request captured" }),
      }),
    );
    try {
      await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
        waitUntil: "domcontentloaded",
      });
      const avatar = page.getByLabel("Jim's appearance", { exact: true });
      await avatar.waitFor({ timeout: 5000 });
      assert.deepEqual(
        await avatar
          .locator("option")
          .evaluateAll((options) => options.map((option) => option.value)),
        SUPPORTED_MODELS.map((entry) => entry.id),
      );
      assert.equal(await avatar.inputValue(), "jim");
      const voice = page.getByLabel("Jim's voice", { exact: true });
      await voice.selectOption("Charon");
      await avatar.selectOption(model.id);
      assert.equal(await voice.inputValue(), "Charon");
      await voice.selectOption("Puck");
      assert.equal(await avatar.inputValue(), model.id);
      assert.deepEqual(
        requested,
        [],
        "preflight does not download an uncommitted avatar",
      );
      await page.locator("#audio-heard").click();
      await page.locator("#camera-skip").click();
      await page.waitForFunction(
        () => !document.querySelector("#audio-check-join").disabled,
      );
      const token = page.waitForRequest((request) =>
        request.url().endsWith("/api/token"),
      );
      await page.locator("#audio-check-join").click();
      const body = (await token).postDataJSON();
      assert.equal(body.interviewerVoice, "Puck");
      assert.equal(
        Object.hasOwn(body, "interviewerAvatar"),
        false,
        "presentation does not enter interviewer behavior metadata",
      );
      await page.waitForFunction(
        () =>
          document.querySelector("#jim-avatar").dataset.avatarState ===
          "unavailable",
      );
      assert.deepEqual(
        requested,
        [model.url],
        "only the committed descriptor is fetched, and a 404 is not retried",
      );
      assert.equal(await avatar.isDisabled(), true);
    } finally {
      await page.close();
    }
  });
}

test("an empty recording snapshot downloads nothing before its selected stage arrives", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  const requested = [];
  let polls = 0;
  let release;
  const secondSnapshot = new Promise((resolve) => {
    release = resolve;
  });
  await page.route("**/vendor/livekit-client.js", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body: `window.LivekitClient = {
      Room: class { remoteParticipants = new Map(); on() { return this; } async connect() {} },
      RoomEvent: {}, Track: { Source: { Camera: "camera" } }
    };`,
    }),
  );
  await page.route("https://raw.githubusercontent.com/**", async (route) => {
    requested.push(route.request().url());
    await route.fulfill({ status: 404, body: "Unavailable model fixture" });
  });
  await page.route("**/api/recording/replay*", async (route) => {
    polls += 1;
    if (polls === 1) return route.fulfill({ json: { events: [], seq: -1 } });
    await secondSnapshot;
    await route.fulfill({
      json: {
        events: [
          { kind: "avatar", payload: { state: "thinking" } },
          {
            kind: "stage",
            payload: {
              title: "Recorded problem",
              avatarId: "jim-business-male-04",
            },
          },
        ],
        seq: 1,
      },
    });
  });
  try {
    await page.goto(
      `${base}/recording/index.html?url=ws://fixture.invalid&token=test-not-a-credential`,
      { waitUntil: "domcontentloaded" },
    );
    const selected = page.waitForRequest(
      (request) => request.url() === SUPPORTED_MODELS[1].url,
    );
    await page.waitForRequest((request) =>
      request.url().includes("/api/recording/replay"),
    );
    assert.deepEqual(requested, []);
    release();
    await selected;
    await page.waitForFunction(
      () =>
        document.querySelector("#jim-avatar").dataset.avatarState ===
        "unavailable",
    );
    await page.evaluate(async () => {
      const { applyReplayEvent } = await import("/recording/recording.js");
      applyReplayEvent({
        kind: "stage",
        payload: { avatarId: "jim-business-male-04" },
      });
    });
    assert.deepEqual(requested, [SUPPORTED_MODELS[1].url]);
    assert.equal(
      await page.locator("#problem-title").textContent(),
      "Recorded problem",
    );
  } finally {
    release();
    await page.close();
  }
});
