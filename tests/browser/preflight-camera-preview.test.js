import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  DEFAULT_RUNTIME_CONFIG,
  FAKE_SPEECH_CAPTURE,
  startStaticServer,
} from "./source.js";

let browser;
let server;
let base;

before(async () => {
  try {
    const { chromium } = await import("playwright");
    browser = await chromium.launch({
      args: [
        "--use-fake-device-for-media-stream",
        "--use-fake-ui-for-media-stream",
        FAKE_SPEECH_CAPTURE,
      ],
    });
  } catch (error) {
    if (process.env.CI) throw error;
    return;
  }

  ({ server, base } = await startStaticServer({
    runtimeConfig: DEFAULT_RUNTIME_CONFIG,
  }));
});

after(async () => {
  await browser?.close();
  await new Promise((closed) => (server ? server.close(closed) : closed()));
});

// These lifecycle checks need a speaking mic to pass preflight. Keep the real
// camera tracks while making microphone samples independent of headless audio.
async function provideMicrophoneSignal(page) {
  await page.addInitScript(() => {
    const NativeAudioContext = window.AudioContext;
    window.AudioContext = class extends NativeAudioContext {
      createAnalyser() {
        const analyser = super.createAnalyser();
        analyser.getByteTimeDomainData = (samples) => {
          for (let index = 0; index < samples.length; index++)
            samples[index] = index % 2 ? 200 : 60;
        };
        return analyser;
      }
    };
  });
}

test("preflight shows a live camera preview", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");

  const page = await browser.newPage();
  t.after(() => page.close());

  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`);

  // Wait until the existing camera check reports that the track is ready.
  await page.waitForFunction(() =>
    document.querySelector("#audio-step-camera").classList.contains("done"),
  );

  await page.waitForFunction(() => {
    const video = document.querySelector("#camera-preview");

    return (
      video?.srcObject instanceof MediaStream &&
      video.videoWidth > 0 &&
      video.videoHeight > 0
    );
  });

  const preview = page.locator("#camera-preview");

  assert.equal(await preview.count(), 1);
  assert.equal(await preview.isVisible(), true);

  const videoState = await preview.evaluate((video) => ({
    hasStream: video.srcObject instanceof MediaStream,
    hasLiveTrack: video.srcObject?.getVideoTracks()[0]?.readyState === "live",
    hasFrames: video.videoWidth > 0 && video.videoHeight > 0,
    isMuted: video.muted,
  }));

  assert.equal(videoState.hasStream, true);
  assert.equal(videoState.hasLiveTrack, true);
  assert.equal(videoState.hasFrames, true);
  assert.equal(videoState.isMuted, true);
});

test("preflight clears and restores the camera preview when skipped and re-enabled", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");

  const page = await browser.newPage();
  t.after(() => page.close());

  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`);

  // Wait for the initial camera preview.
  await page.waitForFunction(() => {
    const video = document.querySelector("#camera-preview");
    return (
      video?.srcObject instanceof MediaStream &&
      video.videoWidth > 0 &&
      video.videoHeight > 0
    );
  });

  // Skip the camera.
  await page.locator("#camera-skip").click();

  await page.waitForFunction(() => {
    const video = document.querySelector("#camera-preview");
    return video.srcObject === null;
  });

  assert.equal(
    await page.locator(".camera-preview-container").isVisible(),
    false,
  );

  assert.equal(
    await page.locator("#camera-preview").evaluate((video) => video.srcObject),
    null,
  );

  // Re-enable the camera.
  await page.locator("#camera-skip").click();

  await page.waitForFunction(() => {
    const video = document.querySelector("#camera-preview");
    return (
      video?.srcObject instanceof MediaStream &&
      video.srcObject.getVideoTracks()[0]?.readyState === "live" &&
      video.videoWidth > 0 &&
      video.videoHeight > 0
    );
  });

  assert.equal(await page.locator("#camera-preview").isVisible(), true);
});

test("starting the interview clears the camera preview", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");

  const page = await browser.newPage();
  t.after(() => page.close());
  await provideMicrophoneSignal(page);

  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`);

  await page.waitForFunction(() => {
    const video = document.querySelector("#camera-preview");

    return (
      video?.srcObject instanceof MediaStream &&
      video.videoWidth > 0 &&
      video.videoHeight > 0
    );
  });

  // Prevent the test from connecting to a real interview.
  await page.route("**/api/token", (route) =>
    route.fulfill({ status: 503, body: "local test" }),
  );

  await page.locator("#audio-heard").click();

  await page.waitForFunction(
    () => !document.querySelector("#audio-check-join").disabled,
  );

  await page.locator("#audio-check-join").click();

  const result = await page.evaluate(() => ({
    previewCleared:
      document.querySelector("#camera-preview").srcObject === null,
    previewHidden: document.querySelector(".camera-preview-container").hidden,
  }));

  assert.equal(result.previewCleared, true);
  assert.equal(result.previewHidden, true);
});
test("Meet presentation keeps the preflight preview until joining", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");

  const page = await browser.newPage();
  t.after(() => page.close());
  await provideMicrophoneSignal(page);

  await page.addInitScript(() => {
    const capture = navigator.mediaDevices.getUserMedia.bind(
      navigator.mediaDevices,
    );

    navigator.mediaDevices.getUserMedia = async (constraints) => {
      const stream = await capture(constraints);

      if (constraints.video) {
        window.testCameraTrack = stream.getVideoTracks()[0];
      }

      return stream;
    };
  });

  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`);

  await page.waitForFunction(() => {
    const video = document.querySelector("#camera-preview");

    return (
      video?.srcObject instanceof MediaStream &&
      video.videoWidth > 0 &&
      video.videoHeight > 0
    );
  });

  // Enable Meet presentation mode while the preview is still active.
  await page.locator("#meet-presentation").check();

  assert.equal(await page.locator("#camera-preview").isVisible(), true);

  // Avoid connecting to a real interview.
  await page.route("**/api/token", (route) =>
    route.fulfill({ status: 503, body: "local test" }),
  );

  await page.locator("#audio-heard").click();

  await page.waitForFunction(
    () => !document.querySelector("#audio-check-join").disabled,
  );

  await page.locator("#audio-check-join").click();

  await page.waitForFunction(
    () => window.testCameraTrack?.readyState === "ended",
  );

  const result = await page.evaluate(() => ({
    previewCleared:
      document.querySelector("#camera-preview").srcObject === null,
    previewHidden: document.querySelector(".camera-preview-container").hidden,
    cameraTrackState: window.testCameraTrack?.readyState,
  }));
  assert.equal(result.previewCleared, true);
  assert.equal(result.previewHidden, true);
  assert.equal(result.cameraTrackState, "ended");
});
test("camera preview hides on mute and returns on unmute", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");

  const page = await browser.newPage();
  t.after(() => page.close());

  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`);

  await page.waitForFunction(() => {
    const video = document.querySelector("#camera-preview");

    return (
      video?.srcObject instanceof MediaStream &&
      video.videoWidth > 0 &&
      video.videoHeight > 0
    );
  });

  const result = await page.evaluate(async () => {
    const video = document.querySelector("#camera-preview");
    const container = document.querySelector(".camera-preview-container");
    const track = video.srcObject.getVideoTracks()[0];

    // Simulate a temporarily unavailable camera.
    Object.defineProperty(track, "muted", {
      configurable: true,
      value: true,
    });
    track.dispatchEvent(new Event("mute"));

    const hiddenWhileMuted = container.hidden && video.srcObject === null;

    // Simulate the camera becoming available again.
    Object.defineProperty(track, "muted", {
      configurable: true,
      value: false,
    });
    track.dispatchEvent(new Event("unmute"));

    const restoredAfterUnmute =
      !container.hidden && video.srcObject?.getVideoTracks()[0] === track;

    return { hiddenWhileMuted, restoredAfterUnmute };
  });

  assert.equal(result.hiddenWhileMuted, true);
  assert.equal(result.restoredAfterUnmute, true);
});
test("skipping the camera prevents old track events from restoring the preview", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");

  const page = await browser.newPage();
  t.after(() => page.close());

  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`);

  await page.waitForFunction(() => {
    const video = document.querySelector("#camera-preview");

    return (
      video?.srcObject instanceof MediaStream &&
      video.videoWidth > 0 &&
      video.videoHeight > 0
    );
  });

  // Keep a reference to the original track before skipping the camera.
  await page.evaluate(() => {
    window.oldCameraTrack = document
      .querySelector("#camera-preview")
      .srcObject.getVideoTracks()[0];
  });

  await page.locator("#camera-skip").click();

  await page.waitForFunction(
    () => document.querySelector("#camera-preview").srcObject === null,
  );

  const result = await page.evaluate(() => {
    // A late event from the old track must not restore the preview.
    window.oldCameraTrack.dispatchEvent(new Event("unmute"));

    const video = document.querySelector("#camera-preview");
    const container = document.querySelector(".camera-preview-container");

    return {
      previewCleared: video.srcObject === null,
      previewHidden: container.hidden,
      oldTrackEnded: window.oldCameraTrack.readyState === "ended",
    };
  });

  assert.equal(result.previewCleared, true);
  assert.equal(result.previewHidden, true);
  assert.equal(result.oldTrackEnded, true);
});

test("preflight help preserves preferences and keeps the complete time budget visible", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage({
    viewport: { width: 1448, height: 1000 },
  });
  t.after(() => page.close());
  await page.goto(
    `${base}/interview.html?problem=chargeback-pair-match&duration=45&loop=coding_behavioral`,
  );
  await page.waitForFunction(
    () => document.querySelector("#round-plan-summary").textContent !== "",
  );
  assert.equal(await page.locator("#examples-help").count(), 1);
  assert.equal(await page.locator("#round-plan-summary").isVisible(), true);
  assert.equal(
    await page.locator("#round-plan-summary").textContent(),
    "Coding + behavioral loop · 37 minute coding budget · 8 minute behavioral reserve.",
  );
  for (const selector of [
    "#preflight-language",
    "#interviewer-voice",
    "#interviewer-accent",
  ])
    assert.equal(await page.locator(selector).isVisible(), true);
  await page.locator("#interviewer-voice").selectOption("Puck");
  await page.locator("#interviewer-accent").selectOption("British");
  await page.locator("#hide-examples").check();
  const help = page.locator("#examples-help");
  assert.equal(await help.getAttribute("open"), null);
  await help.locator("summary").focus();
  await page.keyboard.press("Enter");
  assert.equal(await help.locator("p").isVisible(), true);
  await page.keyboard.press("Space");
  assert.equal(await help.locator("p").isVisible(), false);
  assert.equal(await page.locator("#round-plan-summary").isVisible(), true);
  assert.equal(await page.locator("#interviewer-voice").inputValue(), "Puck");
  assert.equal(
    await page.locator("#interviewer-accent").inputValue(),
    "British",
  );
  assert.equal(await page.locator("#hide-examples").isChecked(), true);
  for (const [selector, text] of [
    [
      "#meet-consequence",
      "If you present in Meet, CodeTrial hands the camera to Meet when you join, camera integrity is off and the report says so; your microphone stays with CodeTrial.",
    ],
    [
      "#recorder-consequence",
      "If you record the screen, your camera appears below Jim unless you present in Meet or choose Continue without camera. Leave the screen recorder's own camera off.",
    ],
  ])
    assert.equal(
      (await page.locator(selector).textContent()).replace(/\s+/g, " ").trim(),
      text,
    );
  for (const [selector, text] of [
    ["#meet-help p", "CodeTrial stops watching your camera after the handoff."],
    ["#recorder-help p", "The browser already holds the camera."],
  ])
    assert.equal(
      (await page.locator(selector).textContent()).replace(/\s+/g, " ").trim(),
      text,
    );
  for (const width of [1448, 375]) {
    await page.setViewportSize({ width, height: 1000 });
    for (const selector of ["#meet-consequence", "#recorder-consequence"])
      assert.equal(await page.locator(selector).isVisible(), true);
  }
  await page.setViewportSize({ width: 1448, height: 1000 });
  await page.locator("#meet-presentation").check();
  assert.equal(await page.locator("#meet-consequence").isVisible(), true);
  assert.equal(await page.locator("#meet-help").getAttribute("open"), null);
  assert.match(
    await page.locator("#meet-consequence").textContent(),
    /camera\s+integrity/i,
  );
  await page.locator("#screen-recording").check();
  assert.equal(await page.locator("#recorder-consequence").isVisible(), true);
  await page.reload();
  await page.waitForFunction(
    () => document.querySelector("#round-plan-summary").textContent !== "",
  );
  assert.equal(await page.locator("#meet-consequence").isVisible(), true);
  assert.equal(await page.locator("#recorder-consequence").isVisible(), true);
  for (const [checkbox, consequence, help] of [
    ["#meet-presentation", "#meet-consequence", "#meet-help"],
    ["#screen-recording", "#recorder-consequence", "#recorder-help"],
  ]) {
    await page.locator(checkbox).uncheck();
    assert.equal(await page.locator(consequence).isVisible(), true);
    assert.equal(await page.locator(help).getAttribute("open"), null);
  }
  assert.equal(await page.locator("#audio-check-join").isDisabled(), true);
});

test("preflight cards and help fit desktop and narrow screens with actions in reach", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(
    `${base}/interview.html?problem=chargeback-pair-match&duration=45&loop=coding_behavioral`,
  );
  await page.waitForFunction(
    () => document.querySelector("#round-plan-summary").textContent !== "",
  );
  assert.equal(await page.locator("#output-help").count(), 1);
  for (const [width, height] of [
    [1448, 1000],
    [1280, 720],
    [861, 720],
    [860, 720],
    [375, 812],
    [320, 568],
    [640, 480],
  ]) {
    await page.setViewportSize({ width, height });
    await page.locator("#round-plan-summary").scrollIntoViewIfNeeded();
    const budget = await page.locator("#round-plan-summary").boundingBox();
    assert.ok(
      budget &&
        budget.height > 0 &&
        budget.y >= 0 &&
        budget.y + budget.height <= height,
      `time budget is hidden at ${width}x${height}`,
    );
    const help = page.locator("#output-help");
    await help.locator("summary").click();
    assert.equal(await help.locator("p").isVisible(), true);
    const layout = await page.evaluate(() => {
      const body = document.querySelector(".preflight-body");
      const footer = document
        .querySelector(".preflight-footer")
        .getBoundingClientRect();
      const start = document
        .querySelector("#audio-check-join")
        .getBoundingClientRect();
      return {
        columns: getComputedStyle(body).gridTemplateColumns.split(" ").length,
        overflow: body.scrollWidth > body.clientWidth,
        pageOverflow: document.documentElement.scrollWidth > innerWidth,
        footerBottom: footer.bottom,
        startTop: start.top,
        startBottom: start.bottom,
      };
    });
    assert.equal(layout.columns, width > 860 ? 2 : 1);
    assert.equal(layout.overflow, false, JSON.stringify({ width, layout }));
    assert.equal(layout.pageOverflow, false);

    assert.ok(
      layout.footerBottom <= height &&
        layout.startTop >= 0 &&
        layout.startBottom <= height,
      JSON.stringify({ width, height, layout }),
    );
    await help.locator("summary").click();
  }
});

test("option help stays centered on its label row as explanations expand", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`);
  await page.waitForFunction(
    () => document.querySelector("#round-plan-summary").textContent !== "",
  );
  for (const [width, height] of [
    [1448, 1000],
    [1280, 720],
    [375, 812],
    [320, 568],
  ]) {
    await page.setViewportSize({ width, height });
    for (const id of [
      "audio-step-examples",
      "audio-step-meet",
      "audio-step-recorder",
    ]) {
      const option = page.locator(`#${id}`);
      const help = option.locator(".preflight-help");
      const summary = help.locator("summary");
      const rowBounds = () =>
        option.evaluate((node) => ({
          label: node
            .querySelector(":scope > p")
            .getBoundingClientRect()
            .toJSON(),
          button: node
            .querySelector(".preflight-help > summary")
            .getBoundingClientRect()
            .toJSON(),
        }));
      for (const open of [false, true]) {
        if (open) await summary.click();
        const { label, button } = await rowBounds();
        assert.ok(
          label &&
            button &&
            Math.abs(
              label.y + label.height / 2 - button.y - button.height / 2,
            ) < 1,
          JSON.stringify({ id, width, open, label, button }),
        );
        assert.equal(await help.locator("p").isVisible(), open);
      }
      await summary.click();
      await option.locator('input[type="checkbox"]').check();
      const { label, button } = await rowBounds();
      assert.ok(
        Math.abs(label.y + label.height / 2 - button.y - button.height / 2) < 1,
        `${id} sharing consequences moved its help button at ${width}px`,
      );
      await option.locator('input[type="checkbox"]').uncheck();
    }
  }
});

test("microphone label clears help with enlarged text", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage({
    viewport: { width: 375, height: 1200 },
  });
  t.after(() => page.close());
  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`);
  await page.waitForFunction(
    () => document.querySelector("#round-plan-summary").textContent !== "",
  );
  for (const [width, fontSize] of [
    [375, 24],
    [320, 24],
    [375, 32],
  ]) {
    await page.setViewportSize({ width, height: 1200 });
    await page.evaluate((size) => {
      document.documentElement.style.fontSize = `${size}px`;
    }, fontSize);
    const overlaps = await page
      .locator('label[for="preflight-input"]')
      .evaluate((label) => {
        const help = document
          .querySelector("#audio-step-mic .preflight-help summary")
          .getBoundingClientRect();
        const text = label.firstChild;
        if (text?.nodeType !== Node.TEXT_NODE || !text.textContent.trim())
          throw new Error("Expected nonempty microphone label text");
        const range = document.createRange();
        const overlaps = [];
        for (let index = 0; index < text.length; index++) {
          if (!text.textContent[index].trim()) continue;
          range.setStart(text, index);
          range.setEnd(text, index + 1);
          for (const rect of range.getClientRects()) {
            if (
              rect.left < help.right &&
              rect.right > help.left &&
              rect.top < help.bottom &&
              rect.bottom > help.top
            )
              overlaps.push(text.textContent[index]);
          }
        }
        return overlaps;
      });
    assert.deepEqual(overlaps, [], JSON.stringify({ width, fontSize }));
  }
});

test("short preflight keeps Back reachable with enlarged text and unavailable microphone", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage({ viewport: { width: 800, height: 580 } });
  t.after(() => page.close());
  await page.addInitScript(() => {
    navigator.mediaDevices.getUserMedia = async () => {
      throw new DOMException("No device available", "NotAllowedError");
    };
  });
  await page.goto(
    `${base}/interview.html?problem=chargeback-pair-match&duration=45&loop=coding_behavioral`,
  );
  await page.waitForFunction(() =>
    document
      .querySelector("#audio-check-status")
      .textContent.startsWith("Microphone unavailable:"),
  );
  const back = page.locator("#audio-check-leave");
  for (const [height, fontSize] of [
    [580, 24],
    [520, 24],
    [520, 28],
  ]) {
    await page.setViewportSize({ width: 800, height });
    await page.evaluate((size) => {
      document.documentElement.style.fontSize = `${size}px`;
    }, fontSize);
    assert.equal(await back.isEnabled(), true);
    const reachable = await back.evaluate((button) => {
      const rect = button.getBoundingClientRect();
      return [rect.top + 1, rect.bottom - 1].every(
        (y) =>
          document
            .elementFromPoint((rect.left + rect.right) / 2, y)
            ?.closest("#audio-check-leave") === button,
      );
    });
    assert.equal(
      reachable,
      true,
      `viewport height ${height}px, root font ${fontSize}px`,
    );
    const language = page.locator("#preflight-language");
    await language.scrollIntoViewIfNeeded();
    await language.click({ trial: true });
    await back.focus();
    let checked = 0;
    for (let index = 0; index < 22; index++) {
      await page.keyboard.press("Shift+Tab");
      const focus = await page.evaluate(() => {
        const active = document.activeElement;
        if (!active.closest(".preflight-content")) return null;
        const rect = active.getBoundingClientRect();
        const hit = document.elementFromPoint(
          (rect.left + rect.right) / 2,
          (rect.top + rect.bottom) / 2,
        );
        return {
          id: active.id,
          tag: active.tagName,
          uncovered: active === hit || active.contains(hit),
        };
      });
      if (focus) {
        checked++;
        assert.equal(
          focus.uncovered,
          true,
          JSON.stringify({ height, fontSize, focus }),
        );
      }
    }
    assert.ok(checked > 0, "keyboard traversal must reach settings");
    await page.locator("#round-plan-summary").scrollIntoViewIfNeeded();
    const budget = await page.locator("#round-plan-summary").boundingBox();
    const footer = await page.locator(".preflight-footer").boundingBox();
    assert.ok(
      budget && footer && budget.y >= 0 && budget.y + budget.height <= footer.y,
      JSON.stringify({ height, fontSize, budget, footer }),
    );
  }
  await back.click();
  await page.waitForURL(`${base}/`);
});
