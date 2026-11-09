import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  functionBody,
  launchChromium,
  read,
  startStaticServer,
} from "./source.js";

let browser;
let server;
let base;
before(async () => {
  browser = await launchChromium();
  if (browser) ({ server, base } = await startStaticServer());
});
after(async () => {
  await browser?.close();
  await new Promise((done) => (server ? server.close(done) : done()));
});

test("a pending microphone toggle suppresses pill hover and active styles until retry is available", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  t.after(() => page.close());
  // Keep the real document and styles, but exercise the SDK-owned microphone
  // path without starting an interview or acquiring a physical microphone.
  await page.route("**/interview.js", (route) =>
    route.fulfill({ contentType: "text/javascript", body: "" }),
  );
  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`);
  const source = read("web/interview.js");
  await page.addScriptTag({
    content: `{
      document.querySelector("#audio-check").hidden = true;
      const nodes = {
        mic: document.querySelector("#mic"),
        micMutedStatus: document.querySelector("#mic-muted-status"),
        micControlStatus: document.querySelector("#mic-control-status"),
      };
      const state = {
        phase: "live", micEnabled: true, micChanging: false,
        room: { localParticipant: { setMicrophoneEnabled: () =>
          new Promise((resolve, reject) => { window.rejectMicrophone = reject; }) } },
      };
      ${functionBody(source, "toggleMicrophone")}\n}
      ${functionBody(source, "paintMicrophoneState")}\n}
      nodes.mic.addEventListener("click", () => void toggleMicrophone());
    }`,
  });
  await page.emulateMedia({ reducedMotion: "reduce" });
  const mic = page.locator("#mic");
  const appearance = () =>
    mic.evaluate(async (button) => {
      await Promise.all(
        button.getAnimations().map((animation) => animation.finished),
      );
      const style = getComputedStyle(button);
      return [style.borderTopColor, style.color, style.backgroundColor];
    });
  await page.mouse.move(0, 0);
  const idle = await appearance();
  await mic.hover();
  const enabledHover = await appearance();
  assert.notDeepEqual(enabledHover.slice(0, 2), idle.slice(0, 2));
  await page.mouse.down();
  const enabledActive = await appearance();
  assert.notEqual(enabledActive[2], idle[2]);
  await page.mouse.up();
  await page.waitForFunction(() => Boolean(window.rejectMicrophone));
  assert.equal(await mic.getAttribute("aria-disabled"), "true");
  assert.equal(await mic.evaluate((button) => button.disabled), false);
  assert.deepEqual(await appearance(), idle, "pending hover stays neutral");
  await page.mouse.down();
  assert.deepEqual(await appearance(), idle, "pending press stays neutral");
  await page.mouse.up();
  await page.evaluate(() =>
    window.rejectMicrophone(new Error("device unavailable")),
  );
  await page.waitForFunction(
    () =>
      document.querySelector("#mic").getAttribute("aria-disabled") === "false",
  );
  assert.match(
    await page.locator("#mic-control-status").innerText(),
    /still on/,
  );
  await mic.hover();
  assert.deepEqual(await appearance(), enabledHover, "retry restores hover");
  await page.mouse.down();
  assert.deepEqual(await appearance(), enabledActive, "retry restores active");
  await page.mouse.move(0, 0);
  await page.mouse.up();
});
