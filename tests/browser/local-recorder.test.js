import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { grant } from "./interview-fixture.js";
import {
  functionBody,
  FAKE_SPEECH_CAPTURE,
  launchChromium,
  read,
  startStaticServer,
} from "./source.js";

let browser, server, base;
before(async () => {
  browser = await launchChromium({
    args: [
      "--autoplay-policy=no-user-gesture-required",
      "--use-fake-device-for-media-stream",
      "--use-fake-ui-for-media-stream",
      FAKE_SPEECH_CAPTURE,
    ],
  });
  if (browser) ({ server, base } = await startStaticServer());
});
after(async () => {
  await browser?.close();
  server?.closeAllConnections();
  await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
});

function captureHandleFixture(options = {}) {
  const configure = navigator.mediaDevices.setCaptureHandleConfig?.bind(
    navigator.mediaDevices,
  );
  navigator.mediaDevices.setCaptureHandleConfig = options.noCaptureHandle
    ? undefined
    : (config) => {
        if (options.captureHandleFailure)
          throw new Error("Capture handle refused");
        configure(config);
        window.captureHandleConfig = config;
      };
  window.captureSurface =
    options.surface === undefined ? "browser" : options.surface;
  window.captureHandle = options.handle || "current";
  window.captureVideo = (track) => {
    const settings = track.getSettings.bind(track);
    track.getSettings = () => ({
      ...settings(),
      displaySurface: window.captureSurface,
    });
    track.getCaptureHandle = () =>
      window.captureHandle === "missing"
        ? null
        : {
            handle:
              window.captureHandle === "current"
                ? window.captureHandleConfig?.handle
                : window.captureHandle,
          };
    return track;
  };
}

async function setup(t, options = {}) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage();
  t.after(async () => {
    await page.evaluate(async () => {
      window.local?.dispose();
      window.mic?.getTracks().forEach((track) => track.stop());
      window.capture?.getTracks().forEach((track) => track.stop());
      clearInterval(window.drawTimer);
      await window.probeAudio?.close();
    });
    await page.close();
  });
  const panel = read("web/interview.html").match(
    /<div id="local-recording"[\s\S]*?<\/div>/,
  )[0];
  await page.route("**/recorder-probe", (route) =>
    route.fulfill({ contentType: "text/html", body: panel }),
  );
  await page.goto(`${base}/recorder-probe`);
  await page.evaluate(captureHandleFixture, options);
  await page.evaluate(async (options) => {
    const { initLocalRecording } = await import("/local-recorder.js");
    const audio = new AudioContext();
    window.probeAudio = audio;
    await audio.resume();
    const tone = (frequency) => {
      const source = audio.createOscillator();
      source.frequency.value = frequency;
      const gain = audio.createGain();
      gain.gain.value = 0.1;
      const output = audio.createMediaStreamDestination();
      source.connect(gain).connect(output);
      source.start();
      return output.stream;
    };
    window.mic = tone(440);
    const canvas = document.createElement("canvas");
    const drawing = canvas.getContext("2d");
    drawing.fillRect(0, 0, 100, 100);
    window.drawTimer = setInterval(() => drawing.fillRect(0, 0, 100, 100), 100);
    const makeCapture = () =>
      new MediaStream([
        window.captureVideo(canvas.captureStream(10).getVideoTracks()[0]),
        ...(options.noAudio ? [] : tone(880).getAudioTracks()),
      ]);
    window.capture = makeCapture();
    window.captureCalls = 0;
    navigator.mediaDevices.getUserMedia = () => {
      throw new Error("The recorder must reuse the existing microphone");
    };
    navigator.mediaDevices.getDisplayMedia = async (captureOptions) => {
      window.captureOptions = captureOptions;
      window.captureCalls++;
      if (options.denied || window.rejectCapture)
        throw new DOMException("Denied", "NotAllowedError");
      if (options.pending)
        await new Promise((resolve) => (window.allowCapture = resolve));
      if (window.capture.getVideoTracks()[0].readyState === "ended")
        window.capture = makeCapture();
      return window.capture;
    };
    const NativeRecorder = MediaRecorder;
    window.recorderCalls = 0;
    window.stopCalls = 0;
    window.created = [];
    window.revoked = [];
    window.downloadClicks = 0;
    window.recordingChanges = [];
    document
      .querySelector("[data-local-download]")
      .addEventListener("click", () => window.downloadClicks++);
    const create = URL.createObjectURL.bind(URL);
    const revoke = URL.revokeObjectURL.bind(URL);
    URL.createObjectURL = (blob) => {
      const url = create(blob);
      window.created.push(url);
      window.result = blob;
      return url;
    };
    URL.revokeObjectURL = (url) => {
      window.revoked.push(url);
      revoke(url);
    };
    window.MediaRecorder = class extends NativeRecorder {
      constructor(stream, config) {
        super(stream, config);
        window.recorderCalls++;
        window.recorded = stream;
        window.recorderOptions = config;
        window.nativeRecorder = this;
        window.recordedBytes = 0;
        this.addEventListener(
          "dataavailable",
          ({ data }) => (window.recordedBytes += data.size),
        );
        if (options.pendingStop)
          this.addEventListener("stop", (event) => {
            event.stopImmediatePropagation();
            window.completeStop = () => this.onstop(event);
          });
        if (options.fail) throw new Error("Recorder unavailable");
      }
      stop() {
        window.stopCalls++;
        super.stop();
      }
    };
    if (options.unsupported) window.MediaRecorder = undefined;
    if (options.pendingResume) {
      const NativeAudioContext = AudioContext;
      window.pauseResume = true;
      window.localTimers = 0;
      const setInterval = window.setInterval.bind(window);
      window.setInterval = (...args) => {
        window.localTimers++;
        return setInterval(...args);
      };
      window.AudioContext = class extends NativeAudioContext {
        constructor(...args) {
          super(...args);
          window.mixContext = this;
        }
        createMediaStreamDestination() {
          const destination = super.createMediaStreamDestination();
          window.mixDestination = destination;
          return destination;
        }
        async resume() {
          if (window.pauseResume)
            await new Promise((resolve) => (window.resumeCapture = resolve));
          try {
            return await super.resume();
          } finally {
            (window.resumedContexts ||= []).push(this);
          }
        }
      };
    }
    window.recordingState = { localUserStream: window.mic };
    window.local = initLocalRecording({
      state: window.recordingState,
      nodes: { localRecording: document.querySelector("#local-recording") },
      recordingEnabled: options.serverRecording === true,
      saveReplacedTake: options.saveReplacedTake === true,
      onRecordingChange: (active) => window.recordingChanges.push(active),
    });
  }, options);
  return page;
}

const start = "[data-local-start]";
const stop = "[data-local-stop]";
const status = "[data-local-status]";
const download = "[data-local-download]";

async function record(page) {
  await page.locator(start).click();
  await page.waitForFunction(() =>
    document
      .querySelector("[data-local-status]")
      .textContent.includes("Recording locally"),
  );
  await page.waitForFunction(() => window.recordedBytes > 0);
}
async function ready(page) {
  await page.locator(download).waitFor({ state: "visible" });
}

async function recorderResult(page, name) {
  await page.evaluate((name) => {
    window.recorderResult = { settled: false, error: null };
    window[name].then(
      (value) => Object.assign(window.recorderResult, { settled: true, value }),
      (error) =>
        Object.assign(window.recorderResult, {
          settled: true,
          error: error?.stack || String(error),
        }),
    );
  }, name);
  await page.waitForFunction(() => window.recorderResult.settled);
  const result = await page.evaluate(() => window.recorderResult);
  assert.equal(result.error, null);
  return result.value;
}

test("local recording exposes a distinct handle only to its own origin", async (t) => {
  const page = await setup(t);
  if (!page) return;
  const other = await setup(t);
  const config = await page.evaluate(() => window.captureHandleConfig);
  assert.ok(config.handle);
  assert.equal(config.exposeOrigin, false);
  assert.deepEqual(config.permittedOrigins, [new URL(base).origin]);
  assert.notEqual(
    config.handle,
    await other.evaluate(() => window.captureHandleConfig.handle),
  );
});

for (const [name, options] of [
  ["a window", { surface: "window" }],
  ["a monitor", { surface: "monitor" }],
  ["an unknown surface", { surface: null }],
  ["another tab", { handle: "another-CodeTrial-document" }],
  ["a tab without a handle", { handle: "missing" }],
]) {
  test(`choosing ${name} releases capture and allows this tab to be retried`, async (t) => {
    const page = await setup(t, options);
    if (!page) return;
    await page.evaluate(() => {
      window.startResult = window.local.start();
    });
    assert.equal(await recorderResult(page, "startResult"), false);
    assert.equal(await page.evaluate(() => window.recorderCalls), 0);
    assert.match(
      await page.locator(status).textContent(),
      /this CodeTrial tab/,
    );
    assert.equal(
      await page.evaluate(() =>
        window.capture
          .getTracks()
          .every((track) => track.readyState === "ended"),
      ),
      true,
    );
    assert.equal(
      await page.evaluate(() => window.mic.getAudioTracks()[0].readyState),
      "live",
    );
    await page.evaluate(() => {
      window.captureSurface = "browser";
      window.captureHandle = "current";
    });
    await record(page);
    await page.locator(stop).click();
    await ready(page);
  });
}

for (const [name, options] of [
  ["unavailable", { noCaptureHandle: true }],
  ["refused", { captureHandleFailure: true }],
]) {
  test(`a ${name} capture handle leaves setup usable and reports the limitation`, async (t) => {
    const page = await setup(t, options);
    if (!page) return;
    await page.evaluate(() => {
      window.startResult = window.local.start();
    });
    assert.equal(await recorderResult(page, "startResult"), false);
    assert.equal(await page.evaluate(() => window.captureCalls), 0);
    assert.equal(
      await page.evaluate(() => window.local.needsMicrophone),
      false,
    );
    assert.equal(
      await page.evaluate(() => window.mic.getAudioTracks()[0].readyState),
      "live",
    );
    assert.match(
      await page.locator(status).textContent(),
      /capture handle|capture-handle/i,
    );
    assert.equal(
      await page.locator(start).getAttribute("aria-disabled"),
      "false",
    );
  });
}

test("a capture changed during audio startup is rejected before recording", async (t) => {
  const page = await setup(t, { pendingResume: true });
  if (!page) return;
  await page.evaluate(() => {
    window.startResult = window.local.start();
  });
  await page.waitForFunction(() => window.resumeCapture);
  await page.evaluate(() => {
    window.captureHandle = "another-CodeTrial-document";
    window.resumeCapture();
  });
  await page.waitForFunction(() => window.mixContext.state === "closed");
  assert.equal(await recorderResult(page, "startResult"), false);
  assert.equal(await page.evaluate(() => window.recorderCalls), 0);
  assert.equal(
    await page.evaluate(() =>
      window.capture.getTracks().every((track) => track.readyState === "ended"),
    ),
    true,
  );
  assert.match(await page.locator(status).textContent(), /this CodeTrial tab/);
});

test("a capture handle change cancels audio startup without waiting for resume", async (t) => {
  const page = await setup(t, { pendingResume: true });
  if (!page) return;
  await page.evaluate(() => {
    window.startResult = window.local.start();
  });
  await page.waitForFunction(() => window.resumeCapture);
  await page.evaluate(() => {
    window.captureHandle = "another-CodeTrial-document";
    window.capture
      .getVideoTracks()[0]
      .dispatchEvent(new Event("capturehandlechange"));
  });
  await page.waitForFunction(() => window.mixContext.state === "closed");
  assert.equal(await recorderResult(page, "startResult"), false);
  assert.equal(await page.evaluate(() => window.recorderCalls), 0);
  assert.equal(
    await page.evaluate(() =>
      window.capture.getTracks().every((track) => track.readyState === "ended"),
    ),
    true,
  );
  assert.equal(
    await page.evaluate(() => window.mic.getAudioTracks()[0].readyState),
    "live",
  );
  await page.evaluate(() => window.resumeCapture());
});

test("switching away from this tab stops once and preserves the partial recording", async (t) => {
  const page = await setup(t);
  if (!page) return;
  await record(page);
  await page.evaluate(() => {
    window.captureHandle = "another-CodeTrial-document";
    const video = window.capture.getVideoTracks()[0];
    video.dispatchEvent(new Event("capturehandlechange"));
    video.dispatchEvent(new Event("capturehandlechange"));
  });
  assert.equal(await page.evaluate(() => window.stopCalls), 1);
  await ready(page);
  assert.equal(await page.evaluate(() => window.created.length), 1);
  assert.match(await page.locator(status).textContent(), /this CodeTrial tab/);
  assert.equal(
    await page.evaluate(() => window.mic.getAudioTracks()[0].readyState),
    "live",
  );
});

test("keyboard recording controls hand focus to the next available action", async (t) => {
  const page = await setup(t, { pending: true });
  if (!page) return;
  await page.locator(start).focus();
  await page.locator(start).press("Enter");
  await page.waitForFunction(() => window.allowCapture);
  await page.evaluate(() => window.allowCapture());
  await page.waitForFunction(
    () =>
      document.activeElement === document.querySelector("[data-local-stop]"),
  );
  await page.waitForFunction(() => window.recordedBytes > 0);
  await page.locator(stop).press("Enter");
  await ready(page);
  await page.waitForFunction(() =>
    document.activeElement?.matches(
      "[data-local-start], [data-local-download]",
    ),
  );
  assert.equal(await page.locator(stop).isHidden(), true);
});

test("recording ticks and browser Stop sharing preserve editor focus", async (t) => {
  const page = await setup(t);
  if (!page) return;
  await record(page);
  const elapsed = await page.locator(stop).textContent();
  await page.evaluate(() => {
    const editor = document.createElement("textarea");
    editor.id = "editor";
    document.body.append(editor);
    editor.focus();
  });
  await page.waitForFunction(
    (elapsed) =>
      document.querySelector("[data-local-stop]").textContent !== elapsed,
    elapsed,
  );
  assert.equal(
    await page
      .locator("#editor")
      .evaluate((node) => node === document.activeElement),
    true,
  );
  await page.evaluate(() =>
    window.capture.getVideoTracks()[0].dispatchEvent(new Event("ended")),
  );
  await ready(page);
  assert.equal(
    await page
      .locator("#editor")
      .evaluate((node) => node === document.activeElement),
    true,
  );
});

test("local recording status is accessible before its first instruction and error", async (t) => {
  const page = await setup(t, { pending: true });
  if (!page) return;
  assert.equal(await page.getByRole("status").count(), 1);
  assert.equal(await page.locator(status).textContent(), "");
  await page.locator(start).click();
  await page.waitForFunction(() => window.allowCapture);
  assert.match(
    await page.getByRole("status").textContent(),
    /Choose the CodeTrial tab/,
  );
  await page.evaluate(() => {
    window.capture.getAudioTracks().forEach((track) => {
      window.capture.removeTrack(track);
      track.stop();
    });
    window.allowCapture();
  });
  await page.waitForFunction(() =>
    document
      .querySelector("[data-local-status]")
      .textContent.includes("No tab audio"),
  );
  assert.match(await page.getByRole("status").textContent(), /No tab audio/);
});

test("Stop sharing during audio startup cancels capture and allows another recording", async (t) => {
  const page = await setup(t, { pendingResume: true });
  if (!page) return;
  await page.evaluate(() => {
    window.startResult = window.local.start();
  });
  await page.waitForFunction(() => window.resumeCapture);
  await page.evaluate(() => {
    window.cancelledMixContext = window.mixContext;
    const video = window.capture.getVideoTracks()[0];
    video.stop();
    video.dispatchEvent(new Event("ended"));
  });
  await page.waitForFunction(
    () =>
      window.recorderCalls ||
      (document
        .querySelector("[data-local-start]")
        .getAttribute("aria-disabled") !== "true" &&
        window.mixContext.state === "closed"),
  );
  assert.deepEqual(
    await page.evaluate(() => ({
      recorderCalls: window.recorderCalls,
      timerCalls: window.localTimers,
      captureEnded: window.capture
        .getTracks()
        .every((track) => track.readyState === "ended"),
      mixEnded: window.mixDestination.stream
        .getTracks()
        .every((track) => track.readyState === "ended"),
      context: window.mixContext.state,
      microphone: window.mic.getAudioTracks()[0].readyState,
      active: window.recordingChanges.includes(true),
    })),
    {
      recorderCalls: 0,
      timerCalls: 0,
      captureEnded: true,
      mixEnded: true,
      context: "closed",
      microphone: "live",
      active: false,
    },
  );
  assert.equal(await recorderResult(page, "startResult"), false);
  assert.doesNotMatch(
    await page.locator(status).textContent(),
    /Recording locally/,
  );
  await page.evaluate(() => (window.pauseResume = false));
  await record(page);
  await page.evaluate(() => window.resumeCapture());
  await page.waitForFunction(() =>
    window.resumedContexts.includes(window.cancelledMixContext),
  );
  assert.equal(
    await page.evaluate(() => window.nativeRecorder.state),
    "recording",
  );
  assert.equal(await page.evaluate(() => window.mixContext.state), "running");
  assert.equal(
    await page.evaluate(() =>
      window.capture.getTracks().every((track) => track.readyState === "live"),
    ),
    true,
  );
  await page.locator(stop).click();
  await ready(page);
  assert.equal(await page.evaluate(() => window.recorderCalls), 1);
});

test("explicit Start mixes tab audio and the existing mic into a downloadable WebM", async (t) => {
  const page = await setup(t);
  if (!page) return;
  assert.equal(
    await page.evaluate(() => typeof window.local.stop().then),
    "function",
  );
  await page.evaluate(() => {
    window.stopped = window.local.stop();
  });
  await recorderResult(page, "stopped");
  assert.equal(await page.evaluate(() => window.captureCalls), 0);
  assert.equal(await page.locator(stop).isDisabled(), true);
  await record(page);
  assert.equal(
    await page.evaluate(() => window.recorderOptions.videoBitsPerSecond),
    1_000_000,
  );
  assert.ok(
    await page.evaluate(
      () => window.nativeRecorder.videoBitsPerSecond <= 1_000_000,
    ),
  );
  assert.deepEqual(await page.evaluate(() => window.captureOptions.audio), {
    suppressLocalAudioPlayback: false,
    restrictOwnAudio: false,
    echoCancellation: false,
    noiseSuppression: false,
    autoGainControl: false,
  });
  assert.deepEqual(
    await page.evaluate(() => [
      window.captureCalls,
      window.recorded.getVideoTracks().length,
      window.recorded.getAudioTracks().length,
    ]),
    [1, 1, 1],
  );
  await page.waitForFunction(() =>
    /00:01/.test(document.querySelector("[data-local-stop]").textContent),
  );
  assert.match(await page.locator(stop).textContent(), /End recording 00:01/);
  assert.equal(await page.locator(start).isHidden(), true);
  const saved = page.waitForEvent("download");
  assert.equal(
    await page.evaluate(() => {
      window.stopped = window.local.stop();
      return window.stopped === window.local.stop();
    }),
    true,
  );
  await recorderResult(page, "stopped");
  assert.equal(await page.locator(download).isVisible(), true);
  assert.match((await saved).suggestedFilename(), /^codetrial-.*\.webm$/);
  await ready(page);
  assert.deepEqual(await page.evaluate(() => window.recordingChanges), [
    true,
    false,
  ]);
  assert.equal(await page.evaluate(() => window.downloadClicks), 1);
  const result = await page.evaluate(async () => {
    const decoder = new AudioContext();
    const audio = await decoder.decodeAudioData(
      await window.result.arrayBuffer(),
    );
    const samples = audio.getChannelData(0);
    // Both source tones must survive encoding; silence or either source alone
    // fails this check even when the stream wiring looks correct.
    const magnitude = (hz) => {
      let real = 0,
        imaginary = 0;
      for (let i = 0; i < samples.length; i++) {
        const phase = (2 * Math.PI * hz * i) / audio.sampleRate;
        real += samples[i] * Math.cos(phase);
        imaginary += samples[i] * Math.sin(phase);
      }
      return Math.hypot(real, imaginary) / samples.length;
    };
    const result = {
      levels: [magnitude(440), magnitude(880)],
      type: window.result.type,
      mic: window.mic.getAudioTracks()[0].readyState,
      capture: window.capture.getTracks().map((t) => t.readyState),
      mixed: window.recorded.getAudioTracks()[0].readyState,
    };
    await decoder.close();
    return result;
  });
  assert.ok(result.type.startsWith("video/webm"));
  assert.ok(
    result.levels.every((level) => level > 0.01),
    JSON.stringify(result),
  );
  assert.equal(result.mic, "live");
  assert.ok(result.capture.every((state) => state === "ended"));
  assert.equal(result.mixed, "ended");
  const old = await page.locator(download).getAttribute("href");
  await page.evaluate(() => (window.rejectCapture = true));
  await page.locator(start).click();
  await page.waitForFunction(() =>
    document
      .querySelector("[data-local-status]")
      .textContent.includes("permission was denied"),
  );
  assert.equal(await page.locator(download).getAttribute("href"), old);
  assert.deepEqual(await page.evaluate(() => window.revoked), []);
  await page.evaluate(() => (window.rejectCapture = false));
  await record(page);
  assert.deepEqual(await page.evaluate(() => window.revoked), [old]);
  await page.locator(stop).focus();
  await page.evaluate(() => {
    const ending = document.createElement("section");
    ending.hidden = true;
    document.body.append(ending);
    window.local.finish(ending);
    window.local.finish(ending);
    ending.hidden = false;
  });
  await ready(page);
  await page.waitForFunction(
    () =>
      document.activeElement ===
      document.querySelector("[data-local-download]"),
  );
  assert.deepEqual(
    await page.evaluate(() => [
      window.stopCalls,
      window.created.length,
      window.downloadClicks,
    ]),
    [2, 2, 2],
  );
  assert.equal(await page.locator(start).isDisabled(), true);
  await page.evaluate(() => window.local.dispose());
  assert.equal(await page.locator(download).isHidden(), true);
});

for (const boundary of ["capture selection", "audio startup"]) {
  for (const replaced of [false, true]) {
    test(`${boundary} ${replaced ? "records the replacement microphone" : "reports a lost microphone"}`, async (t) => {
      const page = await setup(t, {
        pending: boundary === "capture selection",
        pendingResume: boundary === "audio startup",
      });
      if (!page) return;
      await page.evaluate(() => {
        window.startResult = window.local.start();
      });
      await page.waitForFunction(
        (boundary) =>
          boundary === "capture selection"
            ? window.allowCapture
            : window.resumeCapture,
        boundary,
      );
      await page.evaluate(
        ({ replaced, boundary }) => {
          const old = window.mic.getAudioTracks()[0];
          const replacement = replaced ? old.clone() : null;
          old.stop();
          window.mic.removeTrack(old);
          if (replacement) window.mic.addTrack(replacement);
          if (boundary === "capture selection") window.allowCapture();
          else window.resumeCapture();
        },
        { replaced, boundary },
      );
      const started = await recorderResult(page, "startResult");
      if (!replaced) {
        assert.equal(await page.evaluate(() => window.recorderCalls), 0);
        assert.equal(started, false);
        assert.match(
          await page.locator(status).textContent(),
          /needs the interview microphone/,
        );
        assert.ok(
          await page.evaluate(() =>
            window.capture
              .getTracks()
              .every((track) => track.readyState === "ended"),
          ),
        );
        return;
      }
      await page.waitForFunction(() => window.recordedBytes > 0);
      await page.locator(stop).click();
      await ready(page);
      const microphoneLevel = await page.evaluate(async () => {
        const decoder = new AudioContext();
        const audio = await decoder.decodeAudioData(
          await window.result.arrayBuffer(),
        );
        const samples = audio.getChannelData(0);
        let real = 0,
          imaginary = 0;
        for (let i = 0; i < samples.length; i++) {
          const phase = (2 * Math.PI * 440 * i) / audio.sampleRate;
          real += samples[i] * Math.cos(phase);
          imaginary += samples[i] * Math.sin(phase);
        }
        await decoder.close();
        return Math.hypot(real, imaginary) / samples.length;
      });
      assert.ok(microphoneLevel > 0.01, `microphone level: ${microphoneLevel}`);
      assert.equal(started, true);
      assert.equal(
        await page.evaluate(() => window.mic.getAudioTracks()[0].readyState),
        "live",
      );
    });
  }
}

for (const [name, options, message] of [
  ["permission denial", { denied: true }, /permission was denied/],
  [
    "missing tab audio",
    { noAudio: true },
    /choose the CodeTrial tab.*Share tab audio/,
  ],
  ["unsupported capture", { unsupported: true }, /Chrome or Chromium/],
  ["recorder failure", { fail: true }, /Recorder unavailable/],
]) {
  test(`${name} leaves Start usable and the interview microphone live`, async (t) => {
    const page = await setup(t, options);
    if (!page) return;
    await page.locator(start).click();
    await page.waitForFunction(
      (pattern) =>
        new RegExp(pattern).test(
          document.querySelector("[data-local-status]").textContent,
        ),
      message.source,
    );
    assert.equal(
      await page.locator(start).getAttribute("aria-disabled"),
      "false",
    );
    assert.match(await page.locator(status).textContent(), message);
    assert.equal(
      await page.evaluate(() => typeof window.local.stop().then),
      "function",
    );
    await page.evaluate(() => {
      window.stopped = window.local.stop();
    });
    await recorderResult(page, "stopped");
    assert.equal(
      await page.evaluate(() => window.local.needsMicrophone),
      false,
    );
    assert.equal(await page.locator(stop).isDisabled(), true);
    assert.equal(
      await page.evaluate(() => window.mic.getAudioTracks()[0].readyState),
      "live",
    );
    if (options.noAudio || options.fail)
      assert.ok(
        await page.evaluate(() =>
          window.capture.getTracks().every((t) => t.readyState === "ended"),
        ),
      );
    assert.equal(await page.locator(download).isHidden(), true);
  });
}

test("ending while permission is pending releases a late grant without recording", async (t) => {
  const page = await setup(t, { pending: true });
  if (!page) return;
  await page.locator(start).click();
  await page.evaluate(() => {
    window.local.finish(document.body);
    window.allowCapture();
  });
  await page.waitForFunction(() =>
    window.capture.getTracks().every((t) => t.readyState === "ended"),
  );
  assert.equal(await page.evaluate(() => window.recorderCalls), 0);
  assert.equal(await page.evaluate(() => window.local.needsMicrophone), false);
  assert.equal(await page.locator(download).isHidden(), true);
});

test("discarding a setup take creates no download and allows a fresh recording", async (t) => {
  const page = await setup(t);
  if (!page) return;
  await record(page);
  await page.evaluate(() => {
    window.stopped = window.local.stop({ discard: true });
    if (window.stopped !== window.local.stop({ discard: true }))
      throw new Error("Discard must share the pending stop");
  });
  await recorderResult(page, "stopped");
  assert.deepEqual(
    await page.evaluate(() => [window.created.length, window.downloadClicks]),
    [0, 0],
  );
  assert.equal(await page.locator(download).isHidden(), true);
  assert.match(await page.locator(status).textContent(), /discarded/i);
  assert.equal(
    await page.evaluate(() => window.mic.getAudioTracks()[0].readyState),
    "live",
  );
  assert.ok(
    await page.evaluate(() =>
      window.capture.getTracks().every((track) => track.readyState === "ended"),
    ),
  );
  await record(page);
  await page.evaluate(() => {
    window.stopped = window.local.stop();
  });
  await recorderResult(page, "stopped");
  await ready(page);
  assert.deepEqual(
    await page.evaluate(() => [window.created.length, window.downloadClicks]),
    [1, 1],
  );
});

test("a recording reports unsaved media until it is downloaded", async (t) => {
  const page = await setup(t);
  if (!page) return;
  assert.equal(await page.evaluate(() => window.local.unsaved), false);
  await record(page);
  assert.equal(await page.evaluate(() => window.local.unsaved), true);
  await page.evaluate(() => {
    window.stopped = window.local.stop();
  });
  await recorderResult(page, "stopped");
  await ready(page);
  // The automatic download counts as saved.
  assert.equal(await page.evaluate(() => window.local.unsaved), false);
});

test("the browser Stop sharing action finalizes once", async (t) => {
  const page = await setup(t);
  if (!page) return;
  await record(page);
  await page.evaluate(() =>
    window.capture.getVideoTracks()[0].dispatchEvent(new Event("ended")),
  );
  await ready(page);
  await page.evaluate(() => {
    window.local.finish();
    window.local.finish();
  });
  assert.deepEqual(
    await page.evaluate(() => [window.stopCalls, window.created.length]),
    [1, 1],
  );
});

test("server recording retains its existing controls", async (t) => {
  const page = await setup(t, { serverRecording: true });
  if (!page) return;
  assert.equal(await page.locator("#local-recording").isHidden(), true);
  assert.equal(await page.evaluate(() => window.captureCalls), 0);
});

test("offline cleanup can preserve the microphone for an active recording", () => {
  const tracks = ["audio", "video"].map((kind) => ({
    kind,
    stopped: false,
    stop() {
      this.stopped = true;
    },
  }));
  let ringsStopped = 0;
  let viewsDetached = 0;
  const cleanup = new Function(
    "stopTurnRing",
    "detachSelfView",
    "recordingMicrophoneDeviceId",
    `${functionBody(read("web/interview.js"), "stopPreflight")}\n}\nreturn stopPreflight;`,
  )(
    () => ringsStopped++,
    () => viewsDetached++,
  );
  const preflight = { userStream: { getTracks: () => tracks } };
  cleanup(preflight, { preserveMicrophone: true });
  assert.deepEqual(
    tracks.map((track) => track.stopped),
    [false, true],
  );
  assert.equal(ringsStopped, 1);
  assert.equal(viewsDetached, 1);
  cleanup(preflight);
  assert.deepEqual(
    tracks.map((track) => track.stopped),
    [true, true],
  );
});

test("interview shutdown finalizes local recording before releasing media and preserves its download in the report", () => {
  const source = read("web/interview.js");
  assert.match(
    functionBody(source, "endInterview"),
    /localRecording\.finish\(nodes\.ending\)/,
  );
  const teardown = functionBody(source, "stopLocalMedia");
  assert.notEqual(teardown.indexOf("localRecording.finish"), -1);
  assert.notEqual(teardown.indexOf("stopPreflight"), -1);
  assert.ok(
    teardown.indexOf("localRecording.finish") <
      teardown.indexOf("stopPreflight"),
  );
  const report = functionBody(source, "renderReport");
  assert.notEqual(report.indexOf("localRecording.finish(nodes.report)"), -1);
  assert.notEqual(report.indexOf("nodes.report.innerHTML"), -1);
  assert.ok(
    report.indexOf("localRecording.finish(nodes.report)") >
      report.indexOf("nodes.report.innerHTML"),
  );
  assert.match(
    functionBody(source, "finalizeRecoveryOnPageHide"),
    /localRecording\.dispose\(\)/,
  );
});

test("a recorder error finalizes available chunks without releasing the interview mic", async (t) => {
  const page = await setup(t);
  if (!page) return;
  await record(page);
  await page.evaluate(() => {
    window.nativeRecorder.onerror();
  });
  assert.equal(await page.locator(status).isVisible(), true);
  await page.evaluate(() => {
    window.nativeRecorder.stop();
  });
  await ready(page);
  assert.match(
    await page.locator(status).textContent(),
    /Local recording failed/,
  );
  assert.equal(
    await page.evaluate(() => window.mic.getAudioTracks()[0].readyState),
    "live",
  );
  assert.equal(await page.evaluate(() => window.created.length), 1);
});

test("an undownloaded partial remains available until a new take is ready", async (t) => {
  const page = await setup(t);
  if (!page) return;
  await record(page);
  await page.evaluate(() => {
    window.nativeRecorder.onerror();
    window.nativeRecorder.stop();
  });
  await ready(page);
  const partial = await page.locator(download).getAttribute("href");
  assert.equal(await page.evaluate(() => window.downloadClicks), 0);
  await page.evaluate(() => (window.rejectCapture = true));
  await page.locator(start).click();
  await page.waitForFunction(() =>
    document
      .querySelector("[data-local-status]")
      .textContent.includes("permission was denied"),
  );
  assert.equal(await page.locator(download).getAttribute("href"), partial);
  await page.evaluate(() => (window.rejectCapture = false));
  await record(page);
  assert.equal(await page.locator(download).isVisible(), true);
  assert.equal(await page.locator(download).getAttribute("href"), partial);
  assert.deepEqual(await page.evaluate(() => window.revoked), []);
  assert.ok(
    await page.evaluate(
      async (url) => (await (await fetch(url)).blob()).size > 0,
      partial,
    ),
  );
  await page.evaluate(() => {
    window.createRecordingURL = URL.createObjectURL;
    URL.createObjectURL = () => {
      throw new Error("Disposable save failure");
    };
    window.stopped = window.local.stop();
  });
  await recorderResult(page, "stopped");
  assert.equal(await page.locator(download).getAttribute("href"), partial);
  assert.deepEqual(await page.evaluate(() => window.revoked), []);
  await page.evaluate(() => (URL.createObjectURL = window.createRecordingURL));
  await record(page);
  await page.locator(download).focus();
  await page.evaluate(() => {
    window.stopped = window.local.stop();
  });
  await recorderResult(page, "stopped");
  assert.notEqual(await page.locator(download).getAttribute("href"), partial);
  assert.deepEqual(await page.evaluate(() => window.revoked), [partial]);
  assert.equal(await page.evaluate(() => window.downloadClicks), 1);
  assert.equal(
    await page
      .locator(download)
      .evaluate((node) => node === document.activeElement),
    true,
  );
});

test("a host can keep an undownloaded take by saving it when another replaces it", async (t) => {
  const page = await setup(t, { saveReplacedTake: true });
  if (!page) return;
  await record(page);
  await page.evaluate(() => {
    window.nativeRecorder.onerror();
    window.nativeRecorder.stop();
  });
  await ready(page);
  assert.equal(await page.evaluate(() => window.downloadClicks), 0);
  await record(page);
  await page.evaluate(() => {
    window.stopped = window.local.stop();
  });
  await recorderResult(page, "stopped");
  // The earlier take, then the new one.
  assert.equal(await page.evaluate(() => window.downloadClicks), 2);
});

test("a downloaded partial is released when another recording starts", async (t) => {
  const page = await setup(t);
  if (!page) return;
  await record(page);
  await page.evaluate(() => {
    window.nativeRecorder.onerror();
    window.nativeRecorder.stop();
  });
  await ready(page);
  const partial = await page.locator(download).getAttribute("href");
  await page.locator(download).click();
  await record(page);
  assert.deepEqual(await page.evaluate(() => window.revoked), [partial]);
  assert.equal(await page.locator(download).isHidden(), true);
  await page.evaluate(() => {
    window.stopped = window.local.stop();
  });
  await recorderResult(page, "stopped");
  assert.equal(await page.evaluate(() => window.downloadClicks), 2);
});

test("microphone loss saves a partial take and ignores a replaced input", async (t) => {
  const page = await setup(t);
  if (!page) return;
  await record(page);
  await page.evaluate(() => {
    const previous = window.mic.getAudioTracks()[0];
    const replacement = previous.clone();
    window.mic.removeTrack(previous);
    window.mic.addTrack(replacement);
    window.local.refreshMicrophone();
    previous.stop();
    previous.dispatchEvent(new Event("ended"));
  });
  assert.equal(
    await page.evaluate(() => window.nativeRecorder.state),
    "recording",
  );
  assert.equal(await page.evaluate(() => window.stopCalls), 0);
  await page.evaluate(() => {
    const current = window.mic.getAudioTracks()[0];
    current.stop();
    current.dispatchEvent(new Event("ended"));
  });
  await ready(page);
  assert.match(await page.locator(status).textContent(), /microphone stopped/);
  assert.deepEqual(
    await page.evaluate(() => ({
      stops: window.stopCalls,
      files: window.created.length,
      downloads: window.downloadClicks,
    })),
    { stops: 1, files: 1, downloads: 0 },
  );
  assert.ok(await page.evaluate(() => window.result.size > 0));
  assert.ok(
    await page.evaluate(() =>
      window.capture.getTracks().every((track) => track.readyState === "ended"),
    ),
  );
});

test("leaving waits for the recording to finish before navigating", async (t) => {
  const page = await setup(t, { pendingStop: true });
  if (!page) return;
  await record(page);
  await page.evaluate(
    (source) => {
      window.navigations = [];
      const leave = new Function(
        "scope",
        `with (scope) { ${source}\n}\nreturn leaveRoom; }`,
      )({
        state: { phase: "ending" },
        audioTiming: null,
        beforeUnloadGuard: { dispose() {} },
        stopEndingClock() {},
        stopAvatar() {},
        stopLocalMedia: () => window.local.finish(),
        localRecording: window.local,
        window: {
          location: {
            set href(value) {
              window.navigations.push(value);
            },
          },
        },
      });
      window.leaveResult = leave();
    },
    functionBody(read("web/interview.js"), "leaveRoom"),
  );
  await page.waitForFunction(() => window.completeStop);
  assert.deepEqual(await page.evaluate(() => window.navigations), []);
  assert.equal(await page.locator(download).isHidden(), true);
  await page.evaluate(() => window.completeStop());
  await recorderResult(page, "leaveResult");
  await ready(page);
  assert.deepEqual(await page.evaluate(() => window.navigations), ["/"]);
  assert.deepEqual(
    await page.evaluate(() => [window.created.length, window.downloadClicks]),
    [1, 1],
  );
  assert.ok(await page.evaluate(() => window.result.size > 0));
});

test("a stranded microphone switch stops once and preserves the partial recording", async (t) => {
  const page = await setup(t);
  if (!page) return;
  await record(page);
  assert.equal(
    await page.evaluate(() => {
      const mic = window.mic.getAudioTracks()[0];
      window.mic.removeTrack(mic);
      mic.stop();
      window.local.refreshMicrophone();
      window.local.refreshMicrophone();
      return window.stopCalls;
    }),
    1,
  );
  await ready(page);
  assert.equal(await page.evaluate(() => window.created.length), 1);
  assert.equal(await page.evaluate(() => window.downloadClicks), 0);
  assert.equal(await page.evaluate(() => window.local.needsMicrophone), false);
  assert.deepEqual(await page.evaluate(() => window.recordingChanges), [
    true,
    false,
  ]);
  assert.match(
    await page.locator(status).textContent(),
    /microphone stopped.*Download/,
  );
  assert.doesNotMatch(
    await page.locator(status).textContent(),
    /Complete the interview setup/,
  );
  assert.equal(await page.locator(start).isVisible(), true);
});

for (const outcome of [
  "starts before joining",
  "stopping while joining keeps microphone",
  "ending while joining preserves recording",
  "denial retries before joining",
  "denial uses plain join",
  "lost mic stays in setup",
  "publish failure keeps local recording available",
  "strict token failure discards setup recording",
  "strict publish failure discards setup recording",
  "strict dock recording stops on disconnect",
  "dock recording survives disconnect",
  "idle disconnect releases microphone",
  "idle publish failure releases microphone",
  "dock recording follows switched microphone",
  "switched microphone after live loss",
  "switched microphone failure after live loss",
  "switched microphone disconnect after live loss",
  "disconnect during microphone switch keeps recording audible",
  "disconnect during microphone switch keeps recording muted",
  "readiness loss restarts with the replacement microphone",
]) {
  test(`preflight recording ${outcome}`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const strict = outcome.startsWith("strict ");
    const publishFailure = outcome.includes("publish failure");
    const page = await browser.newPage();
    t.after(() => page.close());
    await page.addInitScript(captureHandleFixture);
    await page.addInitScript((outcome) => {
      const joinedRoom =
        outcome.includes("disconnect") ||
        outcome.includes("switched microphone");
      const offlineRestart =
        outcome === "starts before joining" ||
        outcome === "publish failure keeps local recording available" ||
        outcome === "idle publish failure releases microphone";
      window.recordingOrder = [];
      window.microphoneRequests = 0;
      window.microphoneConstraints = [];
      window.recordedBytes = 0;
      window.recordedStops = 0;
      window.finishedRecordingBytes = [];
      window.downloadClicks = 0;
      window.deferPreflightStop =
        outcome === "readiness loss restarts with the replacement microphone" ||
        outcome === "ending while joining preserves recording";
      window.createdRecordingBlobs = 0;
      window.createdRecordingUrls = 0;
      if (outcome.startsWith("strict ")) {
        const NativeBlob = Blob;
        window.Blob = class extends NativeBlob {
          constructor(parts, options) {
            super(parts, options);
            if (this.type.startsWith("video/")) window.createdRecordingBlobs++;
          }
        };
        const createObjectURL = URL.createObjectURL.bind(URL);
        URL.createObjectURL = (blob) => {
          if (blob.type.startsWith("video/")) window.createdRecordingUrls++;
          return createObjectURL(blob);
        };
      }
      window.denyCapture = outcome.startsWith("denial");
      document.addEventListener("click", (event) => {
        if (event.target.matches("[data-local-download]"))
          window.downloadClicks++;
      });
      window.microphones = [];
      if (
        outcome === "readiness loss restarts with the replacement microphone" ||
        joinedRoom ||
        offlineRestart
      ) {
        navigator.mediaDevices.enumerateDevices = async () => [
          {
            kind: "audioinput",
            deviceId: "laptop",
            label: "Default microphone",
          },
          {
            kind: "audioinput",
            deviceId: "usb",
            label: "Replacement microphone",
          },
        ];
        window.mixedMicrophones = [];
        const NativeAudioContext = AudioContext;
        window.AudioContext = class extends NativeAudioContext {
          createAnalyser() {
            const analyser = super.createAnalyser();
            analyser.getByteTimeDomainData = (samples) => {
              for (let index = 0; index < samples.length; index++)
                samples[index] = index % 2 ? 200 : 60;
            };
            return analyser;
          }
          createMediaStreamSource(stream) {
            const source = super.createMediaStreamSource(stream);
            const connect = source.connect.bind(source);
            source.connect = (destination, ...args) => {
              if (destination instanceof MediaStreamAudioDestinationNode)
                window.mixedMicrophones.push(
                  ...stream
                    .getAudioTracks()
                    .filter((track) => window.microphones.includes(track))
                    .map((track) => track.id),
                );
              return connect(destination, ...args);
            };
            return source;
          }
        };
      }
      const getUserMedia = navigator.mediaDevices.getUserMedia.bind(
        navigator.mediaDevices,
      );
      navigator.mediaDevices.getUserMedia = async (constraints) => {
        if (constraints.audio) {
          window.microphoneConstraints.push(constraints.audio);
          if (window.microphoneFailure)
            throw new DOMException(
              "Microphone unavailable",
              window.microphoneFailure,
            );
          if (
            window.missingSelectedMicrophone &&
            constraints.audio.deviceId?.exact === "usb"
          )
            throw new DOMException("Microphone disconnected", "NotFoundError");
        }
        if (constraints.audio && window.loseMicrophone)
          throw new DOMException("Microphone disconnected", "NotFoundError");
        let requested = constraints;
        if (
          (outcome ===
            "readiness loss restarts with the replacement microphone" ||
            joinedRoom ||
            offlineRestart) &&
          constraints.audio
        ) {
          const audio = { ...constraints.audio };
          delete audio.deviceId;
          requested = { audio };
        }
        const stream = await getUserMedia(requested);
        if (constraints.audio) {
          window.microphoneRequests++;
          window.preflightMic = stream.getAudioTracks()[0];
          window.microphones.push(window.preflightMic);
          if (joinedRoom || offlineRestart) {
            const microphone = window.preflightMic;
            const settings = microphone.getSettings.bind(microphone);
            const deviceId = constraints.audio?.deviceId?.exact || "laptop";
            microphone.getSettings = () => ({
              ...settings(),
              deviceId:
                (offlineRestart ||
                  outcome === "idle disconnect releases microphone") &&
                microphone.readyState === "ended"
                  ? ""
                  : deviceId,
            });
          }
        }
        if (constraints.audio && window.deferRecordingMic) {
          await new Promise(
            (resolve) => (window.releaseRecordingMic = resolve),
          );
        }
        return stream;
      };
      if (outcome.includes("publish failure") || joinedRoom) {
        window.disconnectStops = [];
        const published = [];
        window.LivekitClient = {
          RoomEvent: new Proxy({}, { get: (_, name) => name }),
          Track: { Source: { Microphone: "microphone", Camera: "camera" } },
          Room: class {
            constructor(options) {
              window.testRoom = this;
              this.options = options;
              window.stopLocalTrackOnUnpublish =
                options.stopLocalTrackOnUnpublish;
            }
            handlers = new Map();
            // A joined room answers its first integrity packet with startup
            // ready, the way an agent whose opening produced output does.
            remoteParticipants = new Map(
              joinedRoom
                ? [
                    [
                      "interviewer-test",
                      { identity: "interviewer-test", attributes: {} },
                    ],
                  ]
                : [],
            );
            canPlaybackAudio = true;
            localParticipant = {
              publishData: async (_bytes, options) => {
                if (!joinedRoom || options?.topic !== "integrity" || this.ready)
                  return;
                this.ready = true;
                const ready = new TextEncoder().encode(
                  JSON.stringify({
                    type: "interviewer_state",
                    reconnecting: false,
                    startup: "ready",
                  }),
                );
                const agent = this.remoteParticipants.get("interviewer-test");
                for (const fn of this.handlers.get("DataReceived") || [])
                  fn(ready, agent, undefined, "control");
              },
              getTrackPublication: () => ({ track: this.microphone }),
              publishTrack: async (track) => {
                published.push(track);
                if (!joinedRoom) {
                  if (window.stopLocalTrackOnUnpublish !== false) track.stop();
                  throw new Error("Disposable publish failure");
                }
                const handlers = new Map();
                this.microphone = {
                  mediaStreamTrack: track,
                  isMuted: false,
                  replaceTrack: async (replacement) => {
                    if (window.deferMicrophoneSwitch)
                      await new Promise(
                        (resolve) => (window.resumeMicrophoneSwitch = resolve),
                      );
                    this.microphone.mediaStreamTrack = replacement;
                    replacement.enabled = !this.microphone.isMuted;
                  },
                  mute: async () => {
                    this.microphone.isMuted = true;
                    this.microphone.mediaStreamTrack.enabled = false;
                  },
                  unmute: async () => {
                    this.microphone.isMuted = false;
                    this.microphone.mediaStreamTrack.enabled = true;
                    for (const fn of handlers.get("unmuted") || []) fn();
                  },
                  on(name, fn) {
                    handlers.set(name, [...(handlers.get(name) || []), fn]);
                  },
                  off(name, fn) {
                    handlers.set(
                      name,
                      (handlers.get(name) || []).filter((each) => each !== fn),
                    );
                  },
                };
                return { track: this.microphone };
              },
            };
            on(name, fn) {
              this.handlers.set(name, [...(this.handlers.get(name) || []), fn]);
              return this;
            }
            off(name, fn) {
              this.handlers.set(
                name,
                (this.handlers.get(name) || []).filter((each) => each !== fn),
              );
              return this;
            }
            async connect() {}
            async disconnect(stopTracks = true) {
              window.disconnectStops.push(stopTracks);
              if (stopTracks) published.forEach((track) => track.stop());
              for (const fn of this.handlers.get("Disconnected") || []) fn();
            }
          },
        };
      }
      navigator.mediaDevices.getDisplayMedia = async () => {
        window.recordingOrder.push("capture");
        await new Promise((resolve) => (window.allowCapture = resolve));
        if (window.denyCapture)
          throw new DOMException("Denied", "NotAllowedError");
        const canvas = document.createElement("canvas");
        const ctx = canvas.getContext("2d");
        ctx.fillRect(0, 0, 1, 1);
        setInterval(() => ctx.fillRect(0, 0, 1, 1), 100);
        const audio = new AudioContext();
        const tabAudio = audio.createMediaStreamDestination();
        const tone = audio.createOscillator();
        tone.connect(tabAudio);
        tone.start();
        window.recordingCapture = new MediaStream([
          window.captureVideo(canvas.captureStream(10).getVideoTracks()[0]),
          ...tabAudio.stream.getAudioTracks(),
        ]);
        return window.recordingCapture;
      };
      const NativeRecorder = MediaRecorder;
      window.MediaRecorder = class extends NativeRecorder {
        constructor(...args) {
          super(...args);
          window.recordingRecorder = this;
          let bytes = 0;
          this.addEventListener("dataavailable", ({ data }) => {
            window.recordedBytes += data.size;
            bytes += data.size;
          });
          this.addEventListener("stop", (event) => {
            window.recordedStops++;
            window.finishedRecordingBytes.push(bytes);
            if (window.deferPreflightStop) {
              event.stopImmediatePropagation();
              window.finishPreflightStop = () => this.onstop(event);
            }
          });
        }
        start(...args) {
          window.recordingOrder.push("record");
          super.start(...args);
        }
      };
    }, outcome);
    const joinedRoom =
      outcome.includes("disconnect") || outcome.includes("switched microphone");
    if (publishFailure || joinedRoom)
      await page.route("**/vendor/livekit-client.js", (route) =>
        route.fulfill({ contentType: "application/javascript", body: "" }),
      );
    let releaseConnection;
    const connectionHeld = new Promise(
      (resolve) => (releaseConnection = resolve),
    );
    t.after(() => releaseConnection());
    if (strict)
      await page.route("**/api/session", (route) =>
        route.fulfill({ json: { signedIn: true, loginRequired: true } }),
      );
    let tokenRequests = 0;
    await page.route("**/api/token", async (route) => {
      tokenRequests++;
      if (
        outcome !== "denial uses plain join" &&
        outcome !== "idle publish failure releases microphone" &&
        !joinedRoom
      )
        await page.waitForFunction(() => window.recordedBytes > 0);
      if (
        outcome === "stopping while joining keeps microphone" ||
        outcome === "ending while joining preserves recording"
      )
        await connectionHeld;
      await page.evaluate(() => window.recordingOrder.push("join"));
      await route.fulfill({
        status: publishFailure || joinedRoom ? 200 : 503,
        json:
          publishFailure || joinedRoom
            ? strict
              ? grant("permission-flag-counter", "disposable-strict")
              : { serverUrl: "ws://disposable.test", token: "disposable" }
            : { error: "Disposable test" },
      });
    });
    await page.goto(
      `${base}/interview.html?${strict ? "selection=strict" : "problem=permission-flag-counter"}`,
      { waitUntil: "domcontentloaded" },
    );
    if (outcome === "dock recording survives disconnect")
      await page.evaluate(() => {
        document.querySelector("#screen-recording").checked = true;
      });
    await page.locator("#camera-skip").click();
    await page.locator("#audio-heard").click();
    await page.locator("#local-recording-join").waitFor({ state: "visible" });
    await page.waitForFunction(
      () => !document.querySelector("#local-recording-join").disabled,
    );
    if (
      outcome === "starts before joining" ||
      outcome === "publish failure keeps local recording available" ||
      outcome.startsWith("idle ")
    ) {
      await page.locator("#preflight-input").selectOption("usb");
      await page.waitForFunction(
        () =>
          window.preflightMic.getSettings().deviceId === "usb" &&
          !document.querySelector("#local-recording-join").disabled,
      );
    }
    assert.deepEqual(await page.evaluate(() => window.recordingOrder), []);
    assert.equal(
      await page
        .locator(status)
        .evaluate(
          (node) => !node.closest("[hidden], [inert], [aria-hidden='true']"),
        ),
      true,
    );
    if (joinedRoom || outcome === "idle publish failure releases microphone") {
      await page.locator("#audio-check-join").click();
      if (joinedRoom)
        await page.waitForFunction(() => window.testRoom?.microphone);
      await page.waitForFunction(
        () => document.querySelector("#audio-check").hidden,
      );
      assert.deepEqual(await page.evaluate(() => window.recordingOrder), [
        "join",
      ]);
      if (!outcome.startsWith("idle ")) {
        await page.locator(start).click();
        await page.waitForFunction(() => window.allowCapture);
        await page.evaluate(() => window.allowCapture());
        await page.waitForFunction(() => window.recordedBytes > 0);
      }
      if (outcome.endsWith("after live loss")) {
        const failed = outcome.includes("failure");
        const disconnected = outcome.includes("disconnect");
        await page.evaluate(
          ({ failed, disconnected }) => {
            if (failed) window.microphoneFailure = "NotReadableError";
            window.deferMicrophoneSwitch = disconnected;
            const lost = window.microphones[0];
            lost.stop();
            lost.dispatchEvent(new Event("ended"));
          },
          { failed, disconnected },
        );
        if (disconnected) {
          await page.waitForFunction(() => window.resumeMicrophoneSwitch);
          await page.evaluate(() => window.testRoom.disconnect(false));
          assert.equal(
            await page.evaluate(() => window.recordingRecorder.state),
            "inactive",
          );
          await page.evaluate(() => window.resumeMicrophoneSwitch());
          await page.waitForFunction(
            () => window.microphones[1].readyState === "ended",
          );
        }
        if (failed || disconnected) {
          await ready(page);
          assert.equal(await page.evaluate(() => window.recordedStops), 1);
          assert.equal(await page.evaluate(() => window.downloadClicks), 0);
          assert.match(
            await page.locator(status).textContent(),
            /microphone stopped.*Download/,
          );
          assert.ok(
            await page.evaluate(() => window.finishedRecordingBytes[0] > 0),
          );
        } else {
          await page.waitForFunction(() =>
            document
              .querySelector("#mic-input-note")
              .textContent.includes("moved to the default"),
          );
          assert.equal(
            await page.evaluate(() => window.recordingRecorder.state),
            "recording",
          );
          assert.equal(await page.evaluate(() => window.recordedStops), 0);
          assert.deepEqual(
            await page.evaluate(() => window.mixedMicrophones),
            await page.evaluate(() =>
              window.microphones.map((track) => track.id),
            ),
          );
          await page.locator(stop).click();
          await ready(page);
          assert.equal(await page.evaluate(() => window.recordedStops), 1);
          assert.equal(
            await page.evaluate(() => window.preflightMic.readyState),
            "live",
          );
        }
        return;
      }
      if (outcome.startsWith("disconnect during microphone switch")) {
        const enabled = !outcome.endsWith("muted");
        if (!enabled) await page.locator("#mic").click();
        await page.evaluate(() => (window.deferMicrophoneSwitch = true));
        await page.locator("#mic-input-select").waitFor({ state: "visible" });
        await page.locator("#mic-input-select").selectOption("usb");
        await page.waitForFunction(() => window.resumeMicrophoneSwitch);
        assert.equal(
          await page.evaluate(() => window.microphones[0].enabled),
          false,
        );
        await page.evaluate(() =>
          window.testRoom.disconnect(
            window.testRoom.options.stopLocalTrackOnUnpublish,
          ),
        );
        assert.equal(
          await page.evaluate(() => window.microphones[0].enabled),
          enabled,
        );
        await page.evaluate(() => window.resumeMicrophoneSwitch());
        await page.waitForFunction(
          () => window.microphones[1].readyState === "ended",
        );
        assert.equal(
          await page.evaluate(() => window.microphones[0].readyState),
          "live",
        );
        assert.equal(
          await page.evaluate(() => window.microphones[0].enabled),
          enabled,
        );
        assert.equal(await page.evaluate(() => window.recordedStops), 0);
        assert.equal(await page.evaluate(() => window.downloadClicks), 0);
        assert.equal(await page.locator(stop).isVisible(), true);
        await page.locator(stop).click();
        await ready(page);
        assert.equal(
          await page.evaluate(() => window.microphones[0].readyState),
          "ended",
        );
        return;
      }
      if (outcome === "dock recording follows switched microphone") {
        await page.locator("#mic-input-select").waitFor({ state: "visible" });
        await page.locator("#mic-input-select").selectOption("usb");
        await page.waitForFunction(
          () =>
            window.microphones.length === 2 &&
            window.microphones[0].readyState === "ended",
        );
        assert.deepEqual(
          await page.evaluate(() => window.mixedMicrophones),
          await page.evaluate(() =>
            window.microphones.map((track) => track.id),
          ),
        );
      } else {
        if (joinedRoom)
          await page.evaluate(() =>
            window.testRoom.disconnect(
              window.testRoom.options.stopLocalTrackOnUnpublish,
            ),
          );
        if (outcome.startsWith("idle ")) {
          await page.waitForFunction(
            () => window.preflightMic.readyState === "ended",
          );
          assert.equal(await page.evaluate(() => window.recordedStops), 0);
          const before = await page.evaluate(
            () => window.microphoneConstraints.length,
          );
          await page.locator(start).click();
          await page.waitForFunction(() => window.allowCapture);
          await page.evaluate(() => window.allowCapture());
          await page.waitForFunction(() => window.recordedBytes > 0);
          assert.deepEqual(
            await page.evaluate(
              (before) =>
                window.microphoneConstraints
                  .slice(before)
                  .map((audio) => audio.deviceId?.exact || null),
              before,
            ),
            ["usb"],
          );
          assert.equal(
            await page.evaluate(() => window.mixedMicrophones.at(-1)),
            await page.evaluate(() => window.preflightMic.id),
          );
          await page.locator(stop).click();
          await ready(page);
          assert.equal(
            await page.evaluate(() => window.preflightMic.readyState),
            "ended",
          );
          return;
        }
        if (strict) {
          await ready(page);
          assert.deepEqual(
            await page.evaluate(() => ({
              blobs: window.createdRecordingBlobs,
              urls: window.createdRecordingUrls,
              downloads: window.downloadClicks,
              stops: window.recordedStops,
              recorder: window.recordingRecorder.state,
              microphone: window.preflightMic.readyState,
              captureEnded: window.recordingCapture
                .getTracks()
                .every((track) => track.readyState === "ended"),
            })),
            {
              blobs: 1,
              urls: 1,
              downloads: 1,
              stops: 1,
              recorder: "inactive",
              microphone: "ended",
              captureEnded: true,
            },
          );
          assert.ok(
            await page.evaluate(() => window.finishedRecordingBytes[0] > 0),
          );
          assert.match(
            await page.locator(status).textContent(),
            /ready.*Download/i,
          );
          assert.equal(await page.locator(stop).isHidden(), true);
          assert.equal(await page.locator(stop).isDisabled(), true);
          assert.equal(await page.locator(start).isHidden(), true);
          assert.equal(await page.locator("#run-tests").isDisabled(), true);
          assert.match(
            await page.locator("#presence-banner").textContent(),
            /Strict Interview connection dropped/,
          );
          assert.equal(tokenRequests, 1);
          return;
        }
      }
      assert.equal(
        await page.evaluate(() => window.preflightMic.readyState),
        "live",
      );
      assert.equal(await page.evaluate(() => window.recordedStops), 0);
      assert.equal(await page.locator(stop).isVisible(), true);
      await page.locator(stop).click();
      await ready(page);
      assert.equal(await page.evaluate(() => window.recordedStops), 1);
      assert.equal(
        await page.evaluate(() => window.preflightMic.readyState),
        outcome === "dock recording follows switched microphone"
          ? "live"
          : "ended",
      );
      return;
    }
    await page.locator("#local-recording-join").click();
    await page.waitForFunction(() => window.allowCapture);
    assert.equal(
      await page
        .getByRole("status")
        .filter({
          hasText: "Choose the CodeTrial tab and enable Share tab audio.",
        })
        .count(),
      1,
    );
    await page.evaluate((outcome) => {
      if (outcome === "lost mic stays in setup") {
        window.loseMicrophone = true;
        window.preflightMic.stop();
        window.preflightMic.dispatchEvent(new Event("ended"));
      }
      if (outcome === "readiness loss restarts with the replacement microphone")
        document
          .querySelector("#preflight-output")
          .dispatchEvent(new Event("outputchange"));
      window.allowCapture();
    }, outcome);
    if (outcome.startsWith("denial")) {
      await page.waitForFunction(() =>
        document
          .querySelector("[data-local-status]")
          .textContent.includes("permission was denied"),
      );
      assert.equal(await page.locator("#audio-check").isVisible(), true);
      assert.equal(
        await page.locator("#local-recording-join").isEnabled(),
        true,
      );
      assert.deepEqual(await page.evaluate(() => window.recordingOrder), [
        "capture",
      ]);
      assert.equal(await page.locator(status).isVisible(), true);
      if (outcome === "denial uses plain join") {
        await page.locator("#audio-check-join").click();
      } else {
        await page.evaluate(() => (window.denyCapture = false));
        await page.locator("#local-recording-join").click();
        await page.waitForFunction(
          () =>
            window.recordingOrder.filter((step) => step === "capture")
              .length === 2,
        );
        await page.evaluate(() => window.allowCapture());
      }
    }
    if (outcome === "stopping while joining keeps microphone") {
      await page.locator(stop).waitFor({ state: "visible" });
      await page.waitForFunction(() => window.recordedBytes > 0);
      await page.locator(stop).click();
      await ready(page);
      assert.equal(
        await page.evaluate(() => window.preflightMic.readyState),
        "live",
      );
      releaseConnection();
      await page.waitForFunction(
        () => window.preflightMic.readyState === "ended",
      );
      return;
    }
    if (outcome === "lost mic stays in setup") {
      await page.waitForFunction(() =>
        document
          .querySelector("[data-local-status]")
          .textContent.includes("needs the interview microphone"),
      );
      assert.equal(await page.locator("#audio-check").isVisible(), true);
      assert.equal(
        await page.locator("#local-recording-join").isDisabled(),
        true,
      );
      assert.deepEqual(await page.evaluate(() => window.recordingOrder), [
        "capture",
      ]);
      return;
    }
    if (outcome === "readiness loss restarts with the replacement microphone") {
      await page.waitForFunction(() =>
        window.recordingOrder.includes("record"),
      );
      await page.locator("#audio-heard").click();
      await page.waitForFunction(
        () =>
          window.finishPreflightStop ||
          !document.querySelector("#local-recording-join").disabled,
      );
      assert.equal(
        await page.locator("#local-recording-join").isDisabled(),
        true,
      );
      await page.evaluate(() => {
        window.deferPreflightStop = false;
        window.finishPreflightStop();
      });
      await page.waitForFunction(
        () => !document.querySelector("#local-recording-join").disabled,
      );
      assert.equal(await page.evaluate(() => window.recordedStops), 1);
      assert.equal(await page.locator(stop).isHidden(), true);
      assert.equal(
        await page.evaluate(() => window.microphones[0].readyState),
        "live",
      );
      await page.locator("#preflight-input").selectOption("usb");
      await page.waitForFunction(
        () =>
          window.microphones.length === 2 &&
          !document.querySelector("#local-recording-join").disabled,
      );
      assert.equal(
        await page.evaluate(() => window.microphones[0].readyState),
        "ended",
      );
      await page.locator("#local-recording-join").click();
      await page.waitForFunction(
        () =>
          window.recordingOrder.filter((step) => step === "capture").length ===
          2,
      );
      await page.evaluate(() => window.allowCapture());
    }
    if (outcome === "ending while joining preserves recording") {
      await page.waitForFunction(() => window.recordedBytes > 0);
      await page.locator("#end").click();
      await page.waitForFunction(() => window.finishPreflightStop);
      const failed = page.waitForEvent("console", {
        predicate: (message) =>
          message.text().includes("codetrial connect_failed"),
      });
      releaseConnection();
      await failed;
      await page.evaluate(() => window.finishPreflightStop());
      assert.equal(await page.evaluate(() => window.downloadClicks), 1);
      await ready(page);
      assert.equal(await page.evaluate(() => window.recordedStops), 1);
      assert.ok(
        await page.evaluate(() => window.finishedRecordingBytes[0] > 0),
      );
      assert.equal(
        await page.evaluate(() => window.preflightMic.readyState),
        "ended",
      );
      return;
    }
    if (strict) {
      await page.waitForFunction(
        (message) =>
          document.querySelector("#audio-check-status").textContent === message,
        publishFailure ? "Disposable publish failure" : "Disposable test",
      );
      await page.waitForFunction(
        () =>
          window.recordedStops === 1 &&
          window.preflightMic.readyState === "ended",
      );
      assert.ok(await page.evaluate(() => window.recordedBytes > 0));
      assert.ok(
        await page.evaluate(() => window.finishedRecordingBytes[0] > 0),
      );
      assert.deepEqual(
        await page.evaluate(() => ({
          blobs: window.createdRecordingBlobs,
          urls: window.createdRecordingUrls,
          downloads: window.downloadClicks,
          captureEnded: window.recordingCapture
            .getTracks()
            .every((track) => track.readyState === "ended"),
          order: window.recordingOrder,
        })),
        {
          blobs: 0,
          urls: 0,
          downloads: 0,
          captureEnded: true,
          order: ["capture", "record", "join"],
        },
      );
      assert.equal(await page.locator(download).isHidden(), true);
      assert.match(await page.locator(status).textContent(), /discarded/i);
      assert.equal(await page.locator(stop).isHidden(), true);
      assert.equal(await page.locator("#audio-check").isVisible(), true);
      assert.equal(await page.locator("#audio-check-join").isEnabled(), true);
      assert.equal(
        await page.locator("#audio-check-join").textContent(),
        "Retry media preflight",
      );
      assert.equal(await page.locator("#audio-check-leave").isEnabled(), true);
      assert.equal(await page.locator("#editor").isDisabled(), true);
      assert.equal(await page.locator("#run-tests").isDisabled(), true);
      assert.doesNotMatch(
        await page.locator("#transcript-panel").textContent(),
        /Offline (?:mode|practice)/,
      );
      assert.equal(
        tokenRequests,
        1,
        "failed Strict setup waits for an explicit retry",
      );
      if (publishFailure)
        assert.deepEqual(await page.evaluate(() => window.disconnectStops), [
          true,
        ]);
      return;
    }
    await page.waitForFunction(() => window.recordingOrder.includes("join"));
    assert.deepEqual(
      await page.evaluate(() => window.recordingOrder),
      outcome === "denial uses plain join"
        ? ["capture", "join"]
        : outcome === "denial retries before joining"
          ? ["capture", "capture", "record", "join"]
          : outcome ===
              "readiness loss restarts with the replacement microphone"
            ? ["capture", "record", "capture", "record", "join"]
            : ["capture", "record", "join"],
    );
    await page.waitForFunction(() =>
      document
        .querySelector("#transcript-panel")
        .textContent.includes("Offline mode is ready"),
    );
    if (outcome === "denial uses plain join") {
      await page.waitForFunction(
        () => window.preflightMic.readyState === "ended",
      );
      assert.equal(await page.locator(download).isHidden(), true);
      assert.equal(await page.evaluate(() => window.recordedStops), 0);
      return;
    }
    assert.equal(await page.locator(stop).isVisible(), true);
    assert.equal(await page.locator(start).isHidden(), true);
    assert.equal(
      await page.evaluate(() => window.preflightMic.readyState),
      "live",
    );
    assert.equal(
      await page.evaluate(() => window.recordedStops),
      outcome === "readiness loss restarts with the replacement microphone"
        ? 1
        : 0,
    );
    assert.equal(await page.evaluate(() => window.downloadClicks), 0);
    if (outcome === "readiness loss restarts with the replacement microphone") {
      assert.deepEqual(
        await page.evaluate(() => window.mixedMicrophones),
        await page.evaluate(() => window.microphones.map((track) => track.id)),
      );
    }
    if (outcome === "publish failure keeps local recording available") {
      assert.deepEqual(await page.evaluate(() => window.disconnectStops), [
        false,
      ]);
      assert.equal(
        await page.evaluate(() => window.stopLocalTrackOnUnpublish),
        false,
      );
    }
    if (
      outcome === "starts before joining" ||
      outcome === "publish failure keeps local recording available"
    ) {
      const requests = await page.evaluate(() => window.microphoneRequests);
      const stopSharing = () =>
        page.evaluate(() => {
          const video = window.recordingCapture.getVideoTracks()[0];
          video.stop();
          video.dispatchEvent(new Event("ended"));
        });
      const restart = async () => {
        const count = await page.evaluate(
          () => window.recordingOrder.filter((x) => x === "capture").length,
        );
        await page.locator(start).click();
        await page.waitForFunction(
          (count) =>
            window.recordingOrder.filter((x) => x === "capture").length > count,
          count,
        );
        await page.evaluate(() => window.allowCapture());
      };
      if (outcome === "starts before joining") await stopSharing();
      else await page.locator(stop).click();
      await ready(page);
      assert.equal(
        await page.evaluate(() => window.preflightMic.readyState),
        "ended",
      );
      if (outcome === "starts before joining") {
        await page.evaluate(() => (window.deferRecordingMic = true));
        await restart();
        await page.waitForFunction(() => window.releaseRecordingMic);
        await stopSharing();
        await page.waitForFunction(
          () =>
            document
              .querySelector("[data-local-start]")
              .getAttribute("aria-disabled") === "false",
        );
        await page.evaluate(() => {
          window.deferRecordingMic = false;
          window.releaseRecordingMic();
        });
        await page.waitForFunction(
          () => window.preflightMic.readyState === "ended",
        );
      } else {
        await page.locator("#mic").click();
        for (const name of ["NotAllowedError", "NotReadableError"]) {
          const before = await page.evaluate(
            () => window.microphoneConstraints.length,
          );
          await page.evaluate(
            (name) => (window.microphoneFailure = name),
            name,
          );
          await restart();
          await page.waitForFunction(() =>
            document
              .querySelector("[data-local-status]")
              .textContent.includes("microphone could not be reopened"),
          );
          assert.deepEqual(
            await page.evaluate(
              (before) =>
                window.microphoneConstraints
                  .slice(before)
                  .map((audio) => audio.deviceId?.exact || null),
              before,
            ),
            ["usb"],
            `${name} must not retry another microphone`,
          );
        }
        assert.equal(
          await page.evaluate(() => window.preflightMic.readyState),
          "ended",
        );
        assert.ok(
          await page.evaluate(() =>
            window.recordingCapture
              .getTracks()
              .every((track) => track.readyState === "ended"),
          ),
        );
        await page.evaluate(() => (window.microphoneFailure = ""));
      }
      const beforeRestart = await page.evaluate(
        () => window.microphoneConstraints.length,
      );
      if (outcome === "starts before joining")
        await page.evaluate(() => (window.missingSelectedMicrophone = true));
      await restart();
      await page.locator(stop).waitFor({ state: "visible" });
      assert.deepEqual(
        await page.evaluate(
          (before) =>
            window.microphoneConstraints
              .slice(before)
              .map((audio) => audio.deviceId?.exact || null),
          beforeRestart,
        ),
        outcome === "starts before joining" ? ["usb", null] : ["usb"],
      );
      assert.equal(
        await page.evaluate(() => window.preflightMic.getSettings().deviceId),
        outcome === "starts before joining" ? "laptop" : "usb",
      );
      assert.equal(
        await page.evaluate(() => window.preflightMic.readyState),
        "live",
      );
      assert.equal(
        await page.evaluate(() => window.preflightMic.enabled),
        outcome === "starts before joining",
      );
      assert.equal(
        await page.evaluate(() => window.microphoneRequests),
        requests + (outcome === "starts before joining" ? 2 : 1),
      );
    }
    if (outcome === "starts before joining")
      await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
    else await page.locator("#end").click();
    await page.waitForFunction(
      () => window.preflightMic.readyState === "ended",
    );
    assert.equal(await page.locator(start).isHidden(), true);
  });
}
