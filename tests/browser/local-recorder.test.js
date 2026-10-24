import { after, before, test } from "node:test";
import assert from "node:assert/strict";
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
        ...canvas.captureStream(10).getVideoTracks(),
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
        window.nativeRecorder = this;
        window.recordedBytes = 0;
        this.addEventListener(
          "dataavailable",
          ({ data }) => (window.recordedBytes += data.size),
        );
        if (options.fail) throw new Error("Recorder unavailable");
      }
      stop() {
        window.stopCalls++;
        super.stop();
      }
    };
    if (options.unsupported) window.MediaRecorder = undefined;
    window.local = initLocalRecording({
      state: { localUserStream: window.mic },
      nodes: { localRecording: document.querySelector("#local-recording") },
      recordingEnabled: options.serverRecording === true,
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

test("explicit Start mixes tab audio and the existing mic into a downloadable WebM", async (t) => {
  const page = await setup(t);
  if (!page) return;
  assert.equal(await page.evaluate(() => window.captureCalls), 0);
  assert.equal(await page.locator(stop).isDisabled(), true);
  await record(page);
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
    /00:01/.test(document.querySelector("[data-local-status]").textContent),
  );
  assert.match(await page.locator(stop).textContent(), /End recording 00:01/);
  assert.equal(await page.locator(start).isHidden(), true);
  const saved = page.waitForEvent("download");
  await page.locator(stop).click();
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
  await page.waitForFunction(
    () => !document.querySelector("[data-local-start]").disabled,
  );
  assert.equal(await page.locator(download).getAttribute("href"), old);
  assert.deepEqual(await page.evaluate(() => window.revoked), []);
  await page.evaluate(() => (window.rejectCapture = false));
  await record(page);
  assert.deepEqual(await page.evaluate(() => window.revoked), [old]);
  await page.evaluate(() => {
    window.local.finish(document.body);
    window.local.finish(document.body);
  });
  await ready(page);
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
      () => !document.querySelector("[data-local-start]").disabled,
    );
    assert.match(await page.locator(status).textContent(), message);
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
  assert.equal(await page.locator(download).isHidden(), true);
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

for (const denied of [false, true]) {
  test(`preflight recording ${denied ? "denial still joins" : "starts before joining"}`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await browser.newPage();
    t.after(() => page.close());
    await page.addInitScript((denied) => {
      window.recordingOrder = [];
      navigator.mediaDevices.getDisplayMedia = async () => {
        window.recordingOrder.push("capture");
        if (denied) throw new DOMException("Denied", "NotAllowedError");
        const canvas = document.createElement("canvas");
        const ctx = canvas.getContext("2d");
        ctx.fillRect(0, 0, 1, 1);
        const audio = new AudioContext();
        return new MediaStream([
          ...canvas.captureStream().getVideoTracks(),
          ...audio.createMediaStreamDestination().stream.getAudioTracks(),
        ]);
      };
      const NativeRecorder = MediaRecorder;
      window.MediaRecorder = class extends NativeRecorder {
        start(...args) {
          window.recordingOrder.push("record");
          super.start(...args);
        }
      };
    }, denied);
    await page.route("**/api/token", async (route) => {
      await page.evaluate(() => window.recordingOrder.push("join"));
      await route.fulfill({
        status: 503,
        contentType: "application/json",
        body: '{"error":"Disposable test"}',
      });
    });
    await page.goto(`${base}/interview.html?problem=permission-flag-counter`);
    await page.locator("#camera-skip").click();
    await page.locator("#audio-heard").click();
    await page.locator("#local-recording-join").waitFor({ state: "visible" });
    await page.waitForFunction(
      () => !document.querySelector("#local-recording-join").disabled,
    );
    assert.deepEqual(await page.evaluate(() => window.recordingOrder), []);
    await page.locator("#local-recording-join").click();
    await page.waitForFunction(() => window.recordingOrder.includes("join"));
    assert.deepEqual(
      await page.evaluate(() => window.recordingOrder),
      denied ? ["capture", "join"] : ["capture", "record", "join"],
    );
  });
}
