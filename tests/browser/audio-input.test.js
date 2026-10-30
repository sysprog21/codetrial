import { after, before, test } from "node:test";
import assert from "node:assert/strict";
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
    ],
  });
  if (browser)
    ({ server, base } = await startStaticServer({
      runtimeConfig: DEFAULT_RUNTIME_CONFIG,
    }));
});
after(async () => {
  await browser?.close();
  await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
});

async function preflight(t, { enumeration = true, deviceIds = true } = {}) {
  const page = await browser.newPage();
  t.after(() => page.close());
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  t.after(() => assert.deepEqual(errors, []));
  await page.addInitScript(
    ({ enumeration, deviceIds }) => {
      if (!deviceIds)
        navigator.mediaDevices.getSupportedConstraints = () => ({});
      window.inputs = [
        {
          kind: "audioinput",
          deviceId: "default",
          label: "Default microphone",
        },
        { kind: "audioinput", deviceId: "usb", label: "USB microphone" },
        { kind: "audioinput", deviceId: "headset", label: "Headset" },
        { kind: "audiooutput", deviceId: "speaker", label: "Speaker" },
      ];
      navigator.mediaDevices.enumerateDevices = enumeration
        ? async () => window.inputs
        : undefined;
      const capture = navigator.mediaDevices.getUserMedia.bind(
        navigator.mediaDevices,
      );
      window.microphones = [];
      window.audioRequests = [];
      navigator.mediaDevices.getUserMedia = async (constraints) => {
        if (!constraints.audio) {
          const stream = await capture(constraints);
          window.cameraTrack = stream.getVideoTracks()[0];
          return stream;
        }
        const deviceId = constraints.audio.deviceId?.exact || "";
        window.audioRequests.push(deviceId);
        if (window.refusedInput === deviceId)
          throw new DOMException(
            "Selected microphone refused",
            window.refusedInputError || "NotAllowedError",
          );
        // Hardware ids are synthetic; capture real fake-media tracks so the
        // production meter and publication code still receive MediaStreamTracks.
        const audio = { ...constraints.audio };
        delete audio.deviceId;
        const stream = await capture({ audio });
        const track = stream.getAudioTracks()[0];
        track.testDeviceId = deviceId;
        const stop = track.stop.bind(track);
        track.stop = () => {
          const stopped = JSON.parse(
            sessionStorage.getItem("microphone-stops") || "[]",
          );
          stopped.push(track.id);
          sessionStorage.setItem("microphone-stops", JSON.stringify(stopped));
          stop();
        };
        window.microphones.push(track);
        if (window.heldInput === deviceId)
          await new Promise((resolve) => {
            window.releaseInput = resolve;
          });
        return stream;
      };
    },
    { enumeration, deviceIds },
  );
  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`);
  await page.waitForFunction(
    (count) =>
      window.microphones.length &&
      window.cameraTrack &&
      document.querySelector("#preflight-input").options.length === count,
    enumeration && deviceIds ? 3 : 1,
  );
  return page;
}

test("preflight offers one default and microphone devices without changing initial capture", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await preflight(t);
  assert.deepEqual(
    await page.locator("#preflight-input option").allTextContents(),
    ["System default", "USB microphone", "Headset"],
  );
  assert.equal(await page.locator("#preflight-input").inputValue(), "");
  assert.deepEqual(await page.evaluate(() => window.audioRequests), [""]);
  const original = await page.evaluate(() => window.microphones[0].id);
  await page.locator("#preflight-input").selectOption("usb");
  await page.waitForFunction(() =>
    window.microphones.some((track) => track.testDeviceId === "usb"),
  );
  assert.deepEqual(
    await page.evaluate(() => ({
      old: window.microphones[0].readyState,
      current: window.microphones.at(-1).readyState,
      camera: window.cameraTrack.readyState,
    })),
    { old: "ended", current: "live", camera: "live" },
  );
  assert.notEqual(
    await page.evaluate(() => window.microphones.at(-1).id),
    original,
  );
});

test("switching microphones closes the gate until the replacement proves sound", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await preflight(t);
  await page.locator("#camera-skip").click();
  await page.locator("#audio-heard").click();
  await page.waitForFunction(
    () => !document.querySelector("#audio-check-join").disabled,
  );
  await page.evaluate(() => {
    window.heldInput = "usb";
  });
  await page.locator("#preflight-input").selectOption("usb");
  await page.waitForFunction(() => Boolean(window.releaseInput));
  assert.equal(await page.locator("#audio-check-join").isDisabled(), true);
  assert.equal(
    await page
      .locator("#audio-step-mic")
      .evaluate((node) => node.classList.contains("done")),
    false,
  );
  assert.equal(
    await page
      .locator("#audio-meter-fill")
      .evaluate((node) => node.style.width),
    "0%",
  );
  await page.evaluate(() => {
    window.heldInput = null;
    window.releaseInput();
  });
  await page.waitForFunction(
    () => !document.querySelector("#audio-check-join").disabled,
  );
  assert.equal(await page.locator("#audio-heard").textContent(), "Confirmed");
});

test("rapid microphone selections discard an earlier pending grant", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await preflight(t);
  await page.evaluate(() => {
    window.heldInput = "usb";
  });
  await page.locator("#preflight-input").selectOption("usb");
  await page.waitForFunction(() => Boolean(window.releaseInput));
  await page.locator("#preflight-input").selectOption("headset");
  assert.deepEqual(await page.evaluate(() => window.audioRequests), [
    "",
    "usb",
  ]);
  await page.evaluate(() => {
    window.heldInput = null;
    window.releaseInput();
  });
  await page.waitForFunction(
    () => window.microphones.at(-1).testDeviceId === "headset",
  );
  assert.equal(
    await page.evaluate(
      () =>
        window.microphones.find((track) => track.testDeviceId === "usb")
          .readyState,
    ),
    "ended",
  );
  assert.equal(await page.locator("#preflight-input").inputValue(), "headset");
  assert.equal(
    await page.evaluate(() => window.microphones.at(-1).readyState),
    "live",
  );
});

test("a browser without device enumeration still captures the default microphone", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await preflight(t, { enumeration: false });
  assert.equal(await page.locator("#preflight-input").isDisabled(), true);
  assert.match(
    await page.locator("#preflight-input-note").textContent(),
    /system default/,
  );
  assert.equal(
    await page.evaluate(() => window.microphones[0].readyState),
    "live",
  );
  await page.waitForFunction(() =>
    document.querySelector("#audio-step-mic").classList.contains("done"),
  );
});

test("unsupported device-id constraints keep microphone selection on default", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await preflight(t, { deviceIds: false });
  assert.equal(await page.locator("#preflight-input").isDisabled(), true);
  assert.equal(await page.locator("#preflight-input").inputValue(), "");
  assert.deepEqual(await page.evaluate(() => window.audioRequests), [""]);
  await page.waitForFunction(() =>
    document.querySelector("#audio-step-mic").classList.contains("done"),
  );
});

test("a refused microphone blocks preflight until a different device succeeds", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await preflight(t);
  await page.evaluate(() => {
    window.refusedInput = "usb";
  });
  await page.locator("#preflight-input").selectOption("usb");
  await page.waitForFunction(() =>
    document
      .querySelector("#audio-check-status")
      .textContent.includes("Selected microphone refused"),
  );
  assert.equal(await page.locator("#audio-check-join").isDisabled(), true);
  assert.equal(
    await page.evaluate(() => window.microphones[0].readyState),
    "ended",
  );
  await page.locator("#preflight-input").selectOption("headset");
  await page.waitForFunction(() =>
    document.querySelector("#audio-step-mic").classList.contains("done"),
  );
  assert.equal(
    await page.evaluate(() => window.microphones.at(-1).testDeviceId),
    "headset",
  );
});

test("failed capture retries preserve device menus until a grant or devicechange", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await preflight(t);
  await page.evaluate(() => {
    const enumerate = navigator.mediaDevices.enumerateDevices;
    window.deviceListings = 0;
    navigator.mediaDevices.enumerateDevices = async () => {
      window.deviceListings += 1;
      return enumerate();
    };
    window.inputOption = document.querySelector("#preflight-input").options[1];
    window.outputOption =
      document.querySelector("#preflight-output").options[0];
    window.refusedInput = "usb";
    window.refusedInputError = "OverconstrainedError";
  });
  await page.locator("#preflight-input").selectOption("usb");
  await page.waitForFunction(
    () => window.audioRequests.filter((id) => id === "usb").length >= 2,
  );
  assert.deepEqual(
    await page.evaluate(() => ({
      listings: window.deviceListings,
      inputRetained: window.inputOption.isConnected,
      outputRetained: window.outputOption.isConnected,
    })),
    { listings: 0, inputRetained: true, outputRetained: true },
  );
  await page.evaluate(() => {
    window.inputs.find((device) => device.deviceId === "headset").label =
      "Granted headset";
  });
  await page.locator("#preflight-input").selectOption("headset");
  await page.waitForFunction(
    () =>
      document.querySelector('#preflight-input option[value="headset"]')
        .textContent === "Granted headset",
  );
  await page.evaluate(() => {
    window.inputs.find((device) => device.deviceId === "usb").label =
      "Reconnected USB";
    navigator.mediaDevices.dispatchEvent(new Event("devicechange"));
  });
  await page.waitForFunction(
    () =>
      document.querySelector('#preflight-input option[value="usb"]')
        .textContent === "Reconnected USB",
  );
});

test("unplugging the selected microphone requests default and reconnecting refreshes choices", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await preflight(t);
  await page.locator("#preflight-input").selectOption("usb");
  await page.waitForFunction(
    () => window.microphones.at(-1).testDeviceId === "usb",
  );
  await page.evaluate(() => {
    window.removedInput = window.inputs.find(
      (device) => device.deviceId === "usb",
    );
    window.inputs = window.inputs.filter((device) => device.deviceId !== "usb");
    navigator.mediaDevices.dispatchEvent(new Event("devicechange"));
  });
  await page.waitForFunction(
    () =>
      document.querySelector("#preflight-input").value === "" &&
      window.microphones.at(-1).testDeviceId === "",
  );
  assert.equal(
    await page.evaluate(
      () =>
        window.microphones.find((track) => track.testDeviceId === "usb")
          .readyState,
    ),
    "ended",
  );
  await page.evaluate(() => {
    window.inputs.push(window.removedInput);
    navigator.mediaDevices.dispatchEvent(new Event("devicechange"));
  });
  await page.waitForFunction(
    () => document.querySelector("#preflight-input").options.length === 3,
  );
  assert.equal(await page.locator("#preflight-input").inputValue(), "");
});

test("microphone enumeration errors recover and ignore an older reply", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await preflight(t);
  await page.evaluate(() => {
    navigator.mediaDevices.enumerateDevices = async () => {
      throw new Error("Listing refused");
    };
    navigator.mediaDevices.dispatchEvent(new Event("devicechange"));
  });
  await page.waitForFunction(() =>
    document
      .querySelector("#preflight-input-note")
      .textContent.includes("could not be listed"),
  );
  assert.equal(
    await page.locator("#preflight-input-note").textContent(),
    "Microphones could not be listed. Check your browser permissions and reload the page.",
  );
  await page.evaluate(() => {
    window.pendingLists = [];
    window.holdLists = true;
    navigator.mediaDevices.enumerateDevices = () =>
      window.holdLists
        ? new Promise((resolve) => {
            window.pendingLists.push(resolve);
          })
        : Promise.resolve(window.inputs);
    navigator.mediaDevices.dispatchEvent(new Event("devicechange"));
    window.staleLists = [...window.pendingLists];
    navigator.mediaDevices.dispatchEvent(new Event("devicechange"));
    for (const resolve of window.pendingLists.slice(window.staleLists.length))
      resolve(window.inputs);
    window.holdLists = false;
  });
  await page.waitForFunction(
    () => document.querySelector("#preflight-input-note").hidden,
  );
  await page.evaluate(async () => {
    for (const resolve of window.staleLists) resolve([]);
    await Promise.resolve();
  });
  assert.equal(await page.locator("#preflight-input option").count(), 3);
});

test("the chosen microphone track is the one handed to LiveKit", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await preflight(t);
  await page.locator("#preflight-input").selectOption("usb");
  await page.locator("#camera-skip").click();
  await page.locator("#audio-heard").click();
  await page.waitForFunction(
    () => !document.querySelector("#audio-check-join").disabled,
  );
  const selected = await page.evaluate(() => window.microphones.at(-1).id);
  await page.route("**/api/token", (route) =>
    route.fulfill({
      json: { serverUrl: "wss://test.invalid", token: "test", durationMin: 45 },
    }),
  );
  await page.evaluate(() => {
    const events = window.LivekitClient.RoomEvent;
    window.published = [];
    window.LivekitClient = {
      RoomEvent: events,
      Track: { Source: { Microphone: "microphone", Camera: "camera" } },
      Room: class {
        localParticipant = {
          async publishTrack(track, options) {
            window.published.push({
              id: track.id,
              kind: track.kind,
              deviceId: track.testDeviceId,
              source: options.source,
            });
            // Stop at publication: the test has no LiveKit service or agent.
            throw new Error("Test publication captured");
          },
        };
        on() {
          return this;
        }
        off() {
          return this;
        }
        async connect() {}
        async disconnect() {}
      },
    };
  });
  await page.locator("#audio-check-join").click();
  await page.waitForFunction(() => window.published.length > 0);
  assert.deepEqual(await page.evaluate(() => window.published), [
    { id: selected, kind: "audio", deviceId: "usb", source: "microphone" },
  ]);
});

test("leaving preflight stops a microphone grant that arrives late", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await preflight(t);
  await page.evaluate(() => {
    window.heldInput = "usb";
  });
  await page.locator("#preflight-input").selectOption("usb");
  await page.waitForFunction(() => Boolean(window.releaseInput));
  let releaseLobby;
  const lobby = new Promise((resolve) => {
    releaseLobby = resolve;
  });
  await page.route(`${base}/`, async (route) => {
    await lobby;
    await route.continue();
  });
  try {
    const held = await page.evaluate(() => {
      const ids = window.microphones.map((track) => track.id);
      document.querySelector("#audio-check-leave").click();
      window.releaseInput();
      return ids;
    });
    releaseLobby();
    await page.waitForURL(`${base}/`);
    const stopped = await page.evaluate(() =>
      JSON.parse(sessionStorage.getItem("microphone-stops") || "[]"),
    );
    assert.ok(held.length >= 2);
    assert.deepEqual(
      held.filter((id) => !stopped.includes(id)),
      [],
    );
  } finally {
    releaseLobby();
  }
});
