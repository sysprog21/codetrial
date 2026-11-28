// "Your turn is done" shares the first dock row with the local recording
// controls, and flex decides which item wraps from their sizes. What sits
// beside the button changes with each phase of a take: the prompt while
// capture is requested, the refusal, End recording with its timer, and the
// download link once a take is saved, when the status is hidden again. The
// recorder paints every phase here itself, so a later change to how one looks
// is laid out as a candidate sees it.

import { after, before, test } from "node:test";
import assert from "node:assert/strict";

import {
  DEFAULT_RUNTIME_CONFIG,
  closeSetupCheck,
  launchChromium,
  startStaticServer,
} from "./source.js";

let browser = null;
let server = null;
let base = "";

before(async () => {
  // The recorder resumes an AudioContext before a take starts.
  browser = await launchChromium({
    args: ["--autoplay-policy=no-user-gesture-required"],
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

/// Gives the dock's recording controls to a recorder of the test's own. The
/// page's recorder keeps listening on the controls it was given, so they are
/// replaced by a copy first. The capture picker answers when the test says,
/// and MediaRecorder stands in for a codec the browser may lack.
function takeOverRecorder(page) {
  return page.evaluate(async () => {
    const original = document.querySelector("#local-recording");
    // The setup check borrows the status for its dialog and appends it back
    // when the check ends, which closeSetupCheck never runs.
    original.append(document.querySelector("[data-local-status]"));
    const panel = original.cloneNode(true);
    original.replaceWith(panel);
    // The recorder clicks Download for a saved take; nothing is saved here.
    panel
      .querySelector("[data-local-download]")
      .addEventListener("click", (event) => event.preventDefault());
    const audio = new AudioContext();
    const configure = navigator.mediaDevices.setCaptureHandleConfig?.bind(
      navigator.mediaDevices,
    );
    navigator.mediaDevices.setCaptureHandleConfig = (config) => {
      window.captureHandle = config.handle;
      try {
        configure?.(config);
      } catch {
        // The test needs only the handle; the real call may refuse it here.
      }
    };
    navigator.mediaDevices.getDisplayMedia = () =>
      new Promise((grant, refuse) => (window.picker = { grant, refuse }));
    window.thisTab = () => {
      const canvas = document.createElement("canvas");
      canvas.getContext("2d").fillRect(0, 0, 10, 10);
      const capture = canvas.captureStream(1);
      const video = capture.getVideoTracks()[0];
      video.getSettings = () => ({ displaySurface: "browser" });
      video.getCaptureHandle = () => ({ handle: window.captureHandle });
      capture.addTrack(
        audio.createMediaStreamDestination().stream.getAudioTracks()[0],
      );
      return capture;
    };
    window.MediaRecorder = class {
      static isTypeSupported = () => true;
      mimeType = "video/webm";
      state = "inactive";
      start() {
        this.state = "recording";
      }
      stop() {
        this.state = "inactive";
        queueMicrotask(() => {
          this.ondataavailable({ data: new Blob(["take"]) });
          this.onstop();
        });
      }
    };
    const { initLocalRecording } = await import("/local-recorder.js");
    initLocalRecording({
      state: { localUserStream: audio.createMediaStreamDestination().stream },
      nodes: { localRecording: panel },
      recordingEnabled: false,
    });
  });
}

/// Starts a take and waits for the picker the recorder opens.
async function pick(page) {
  await page.evaluate(() => (window.picker = null));
  await page.click("[data-local-start]");
  await page.waitForFunction(() => window.picker);
}

/// Each phase in the order a take goes through them, and how to reach it.
const PHASES = [
  ["no recording status", async () => {}],
  ["capture prompt", pick],
  [
    "denied status",
    async (page) => {
      await page.evaluate(() =>
        window.picker.refuse(new DOMException("Denied", "NotAllowedError")),
      );
      await page.waitForFunction(() =>
        document
          .querySelector("[data-local-status]")
          .textContent.includes("denied"),
      );
    },
  ],
  [
    "End recording",
    async (page) => {
      await pick(page);
      await page.evaluate(() => window.picker.grant(window.thisTab()));
      await page.waitForSelector("[data-local-stop]:not([hidden])");
    },
  ],
  [
    "Download link",
    async (page) => {
      await page.click("[data-local-stop]");
      await page.waitForSelector("[data-local-download]:not([hidden])");
    },
  ],
];

/// Lays the dock out at every side panel width the divider allows, from the
/// bounds the divider itself reports, and reads the turn button, the
/// recording controls, the row holding them, and each control button beside
/// the height a copy of it that cannot wrap has.
function dockBoxes(page) {
  return page.evaluate(() => {
    const shell = document.querySelector(".interview-shell");
    const handle = document.querySelector("#sidebar-resizer");
    const button = document.querySelector("#yield-turn");
    const panel = document.querySelector("#local-recording");
    const box = (node) => {
      const { top, bottom, right, width, height } =
        node.getBoundingClientRect();
      return { top, bottom, right, width, height };
    };
    const single = (control) => {
      const probe = control.cloneNode(true);
      probe.style.cssText =
        "position: absolute; visibility: hidden; white-space: nowrap";
      control.parentElement.append(probe);
      const { height } = probe.getBoundingClientRect();
      probe.remove();
      return height;
    };
    const min = Number(handle.getAttribute("aria-valuemin"));
    const max = Number(handle.getAttribute("aria-valuemax"));
    const layouts = [];
    for (let width = min; width <= max; width++) {
      shell.style.setProperty("--sidebar-width", `${width}px`);
      layouts.push({
        width,
        button: box(button),
        recording: box(panel),
        row: box(button.parentElement),
        controls: [...panel.querySelectorAll(".pill-button:not([hidden])")].map(
          (control) => ({
            height: box(control).height,
            single: single(control),
          }),
        ),
      });
    }
    return { min, max, layouts };
  });
}

/// The turn button's height on one line and the width its whole label needs,
/// read from a copy taken out of the flex layout so nothing can squeeze it.
function labelBox(page) {
  return page.evaluate(() => {
    const button = document.querySelector("#yield-turn");
    const probe = button.cloneNode(true);
    probe.removeAttribute("id");
    probe.style.cssText =
      "position: absolute; visibility: hidden; flex: none; width: max-content";
    button.parentElement.append(probe);
    const { width, height } = probe.getBoundingClientRect();
    probe.remove();
    return { width, height };
  });
}

/// Joins consecutive widths into ranges, so a failure reads as the bands the
/// layout breaks in rather than hundreds of single widths.
function ranges(widths) {
  const spans = [];
  for (const width of widths) {
    const last = spans.at(-1);
    if (last && width === last[1] + 1) last[1] = width;
    else spans.push([width, width]);
  }
  return spans
    .map(([from, to]) => (from === to ? `${from}px` : `${from}-${to}px`))
    .join(", ");
}

test("Your turn is done keeps its whole label in every recording phase", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.setViewportSize({ width: 1900, height: 900 });
    await page.goto(`${base}/interview.html`, {
      waitUntil: "domcontentloaded",
    });
    await page.waitForFunction(
      () => document.querySelector("#candidate-case-input").placeholder !== "",
    );
    await closeSetupCheck(page);
    const label = await labelBox(page);
    assert.ok(label.width > 0, "the turn button is not rendered");
    await takeOverRecorder(page);
    const problems = [];
    for (const [phase, reach] of PHASES) {
      await reach(page);
      const { min, max, layouts } = await dockBoxes(page);
      assert.ok(min > 0 && max > min, `the divider reports ${min}-${max}px`);
      const found = { squeezed: [], "control wrapped": [], overflowing: [] };
      for (const { width, button, recording, row, controls } of layouts) {
        if (button.width < label.width - 1 || button.height > label.height + 1)
          found.squeezed.push(width);
        if (controls.some(({ height, single }) => height > single + 1))
          found["control wrapped"].push(width);
        if (recording.right > row.right + 1) found.overflowing.push(width);
      }
      for (const [kind, widths] of Object.entries(found))
        if (widths.length)
          problems.push(`${phase}, ${kind}: ${ranges(widths)}`);
    }
    assert.deepEqual(problems, [], problems.join("\n"));
  } finally {
    await page.close();
  }
});
