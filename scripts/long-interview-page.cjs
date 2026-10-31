// The page half of the long-interview driver: the controller that turns a
// scheduled action into something the candidate does in the interview page.
//
// Clip playback is controller-clocked. Chromium's fake audio capture file runs
// on the browser's own clock from launch and loops, so it cannot place a clip
// at a scheduled second. The page's microphone is instead a MediaStream the
// controller feeds: installMicrophone() replaces getUserMedia's audio before
// the page loads, and playClip() decodes one clip into that stream once.
//
// Not exercised in a real browser: tests drive this through a fake page. That
// the interview's media gate accepts a stream that is silent until a clip
// plays, and that the agent hears the injected audio, are for a real browser run of the long interview to prove.
"use strict";

const { revisionFixture } = require("./long-interview-schedule.cjs");

/// Runs in the page before any of its scripts.
function microphoneInitScript() {
  return `(() => {
    const devices = navigator.mediaDevices;
    if (!devices) return;
    const original = devices.getUserMedia.bind(devices);
    let ctx = null;
    let dest = null;
    let gate = null;
    const ensure = () => {
      if (!ctx) {
        ctx = new AudioContext({ sampleRate: 16000 });
        dest = ctx.createMediaStreamDestination();
        // The media gate refuses a microphone that carries no sound, and the
        // stream is otherwise silent until a clip plays. A tone holds the gate
        // open until the join is clicked, and is silenced before the room can
        // publish the track, so the interviewer never hears it.
        const osc = ctx.createOscillator();
        gate = ctx.createGain();
        gate.gain.value = 0.3;
        osc.frequency.value = 440;
        osc.connect(gate);
        gate.connect(dest);
        osc.start();
      }
    };
    devices.getUserMedia = async (constraints) => {
      if (!constraints || !constraints.audio) return original(constraints);
      ensure();
      await ctx.resume().catch(() => {});
      const stream = new MediaStream(dest.stream.getAudioTracks());
      if (constraints.video) {
        const video = await original({ video: constraints.video });
        for (const track of video.getVideoTracks()) stream.addTrack(track);
      }
      return stream;
    };
    window.__longInterviewMic = {
      ready: () => Boolean(ctx),
      silenceGate: () => {
        if (gate) gate.gain.value = 0;
        return Boolean(gate);
      },
      play: async (base64) => {
        ensure();
        await ctx.resume();
        const raw = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
        const buffer = await ctx.decodeAudioData(raw.buffer);
        const source = ctx.createBufferSource();
        source.buffer = buffer;
        source.connect(dest);
        source.start();
        return buffer.duration;
      },
    };
  })();`;
}

async function installMicrophone(page) {
  await page.addInitScript(microphoneInitScript());
}

/// Ends the tone that held the media gate open. Called right after the join is
/// clicked: the track is published to the room only once it has connected.
async function silenceGateTone(page) {
  const silenced = await page.evaluate(() =>
    window.__longInterviewMic?.silenceGate(),
  );
  if (!silenced) throw new Error("the microphone gate tone was never started");
}

/// `selectLanguage` is browser-check's helper, passed in rather than copied.
function pageController(page, { selectLanguage, mode = "coding" }) {
  let languageSet = false;
  const pressed = async (locator) =>
    (await locator.getAttribute("aria-pressed")) === "true";
  const jimTranscriptRows = () =>
    page.locator("p").filter({ hasText: /^Jim$/ }).count();
  return {
    async playClip(entry, bytes) {
      if (!bytes) throw new Error(`no audio for ${entry.clip_id}`);
      const seconds = await page.evaluate(
        (base64) => window.__longInterviewMic.play(base64),
        Buffer.from(bytes).toString("base64"),
      );
      if (!(seconds > 0)) throw new Error(`${entry.clip_id} played no audio`);
    },
    async applyRevision(step) {
      const fixture = revisionFixture(mode, step.index);
      if (fixture.marker !== step.marker) {
        throw new Error(`revision ${step.index} is not the scheduled one`);
      }
      if (fixture.kind === "board") {
        await drawBoardRevision(page, fixture, step.index);
        return;
      }
      if (fixture.kind !== "editor") {
        throw new Error(`no way to drive a ${fixture.kind} revision`);
      }
      if (!languageSet) {
        await selectLanguage(page, "JavaScript");
        languageSet = true;
      }
      await page.getByLabel("Code editor").fill(fixture.source);
    },
    async setThinking(on) {
      const button = page.locator("#thinking");
      if ((await pressed(button)) !== on) await button.click();
      // The button follows the interviewer's acknowledgement, which arrives
      // over the data channel after the click, not with it.
      try {
        await page.waitForFunction(
          ([want]) =>
            document
              .querySelector("#thinking")
              ?.getAttribute("aria-pressed") === String(want),
          [on],
          { timeout: 10000 },
        );
      } catch {
        throw new Error(`Thinking did not become ${on ? "held" : "released"}`);
      }
    },
    async agentSpeaking() {
      const text = await page.locator("#agent-state").innerText();
      return text.replace(/…|\.{3}/g, "").trim() === "Speaking";
    },
    agentReplyCount: jimTranscriptRows,
    async end() {
      // A smoke whose target equals the interview's length reaches the page's
      // own countdown at the moment it would click: the page then ends the
      // interview itself and disables the button. That is reported, not
      // waited out as a two minute click timeout.
      const button = page.getByRole("button", { name: "End interview" });
      const packet = page.getByRole("heading", {
        name: "Your performance packet",
      });
      let clicked = null;
      for (let waited = 0; clicked === null && waited < 5000; waited += 250) {
        if (await packet.isVisible()) clicked = false;
        else if (await button.isEnabled()) {
          await button.click();
          clicked = true;
        } else await page.waitForTimeout(250);
      }
      // Still disabled with no packet: the page began ending itself and is
      // generating the report. The packet is waited for below, and its absence
      // is the missing_performance class rather than a click timeout.
      if (clicked === null) clicked = false;
      return { ...(await afterEnd(page, 240000)), clicked };
    },
    async awaitServerDeadline() {
      return afterEnd(page, 240000);
    },
  };
}

/// Draws one closed shape per entry of the revision's strokes, in a row of its
/// own, so each revision leaves a board that differs from the last. The board
/// has no text tool, so the marker cannot be written on it: a revision is told
/// apart by position and shape count, not by a legible marker.
async function drawBoardRevision(page, fixture, index) {
  const bounds = await page.locator("#board").boundingBox();
  if (!bounds) throw new Error("the visible board had no drawing bounds");
  const row = bounds.y + 30 + ((index - 1) % 6) * 70;
  let column = bounds.x + 30;
  for (let i = 0; i < fixture.strokes.length; i++) {
    await page.mouse.move(column, row);
    await page.mouse.down();
    await page.mouse.move(column + 80, row, { steps: 4 });
    await page.mouse.move(column + 80, row + 40, { steps: 4 });
    await page.mouse.move(column, row + 40, { steps: 4 });
    await page.mouse.move(column, row, { steps: 4 });
    await page.mouse.up();
    column += 110;
  }
  if (await page.getByRole("button", { name: "Clear board" }).isDisabled()) {
    throw new Error("the board stayed empty after the revision was drawn");
  }
}

/// What the end left on the page: the performance packet and a stored report.
async function afterEnd(page, timeout) {
  let performanceReceived = false;
  try {
    await page
      .getByRole("heading", { name: "Your performance packet" })
      .waitFor({ timeout });
    performanceReceived = true;
  } catch {
    // Recorded as a missing packet, not thrown: the class names it.
  }
  const reportReceived = await page
    .evaluate(() => {
      const history = JSON.parse(
        localStorage.getItem("codetrial_history") ?? "[]",
      );
      return Boolean(history[0]?.report);
    })
    .catch(() => false);
  return { performanceReceived, reportReceived };
}

module.exports = {
  installMicrophone,
  microphoneInitScript,
  pageController,
  silenceGateTone,
};
