import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { launchChromium, startStaticServer } from "./source.js";
let browser, server, base;
before(async () => {
  browser = await launchChromium();
  if (browser) ({ server, base } = await startStaticServer());
});
after(async () => {
  await browser?.close();
  await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
});
for (const expired of [false, true]) {
  test(`synthetic persisted lobby lifecycle restores ${expired ? "expired" : "live"} availability`, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await browser.newPage();
    t.after(() => page.close());
    let active = false,
      reads = 0,
      removed = 0,
      beats = 0;
    await page.route("**/vendor/livekit-client.js", (route) =>
      route.fulfill({
        contentType: "text/javascript",
        body: `window.connections=0;window.disconnections=0;window.LivekitClient={Room:class{async connect(){window.connections++}async disconnect(){window.disconnections++}}};`,
      }),
    );
    await page.route("**/api/session", (route) =>
      route.fulfill({
        json: {
          signedIn: true,
          identityVerified: true,
          peerEnabled: true,
          user: { login: "alice" },
        },
      }),
    );
    await page.route("**/api/peer/profile", (route) =>
      route.fulfill({ json: { nickname: "Lucky7" } }),
    );
    await page.route("**/api/peer/sessions", (route) =>
      route.fulfill({ json: { sessions: [] } }),
    );
    await page.route("**/api/peer/lobby", (route) => {
      reads++;
      return route.fulfill({
        json: {
          available: [],
          requests: [],
          you: { availabilityId: active ? "mine" : null },
        },
      });
    });
    await page.route("**/api/peer/availability/heartbeat", (route) => {
      beats++;
      return route.fulfill({ json: {} });
    });
    await page.route("**/api/peer/availability", (route) => {
      if (route.request().method() === "DELETE") {
        removed++;
        active = false;
      } else active = true;
      return route.fulfill({
        json: { availabilityId: "mine", url: "wss://example", token: "token" },
      });
    });
    await page.clock.install();
    await page.goto(`${base}/peer-lobby.html`);
    await page.locator("#peer-content").waitFor({ state: "visible" });
    await page.locator("#peer-opt-in").check();
    await page.locator("#peer-available").click();
    await page.waitForFunction(() => window.connections === 1);
    await page.evaluate(() =>
      dispatchEvent(new PageTransitionEvent("pagehide", { persisted: true })),
    );
    await page.waitForFunction(() => window.disconnections === 1);
    assert.equal(
      removed,
      0,
      "persisted suspension must leave the TTL to expire availability",
    );
    if (expired) active = false;
    await page.evaluate(() =>
      dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true })),
    );
    await page.waitForFunction(
      () =>
        document
          .getElementById("peer-presence-status")
          .textContent.includes("visible") ||
        !document.getElementById("peer-opt-in").checked,
    );
    // The restoring refresh is not the timer: wait for it to settle, then count
    // only reads that the polling interval makes afterwards.
    await page.clock.runFor(1000);
    const settled = reads;
    await page.clock.runFor(15000);
    assert.ok(
      reads > settled,
      "restoration must resume authoritative roster polling",
    );
    if (expired) {
      assert.equal(await page.locator("#peer-opt-in").isChecked(), false);
      assert.equal(await page.evaluate(() => window.connections), 1);
      assert.equal(beats, 0);
    } else {
      await page.waitForFunction(() => window.connections === 2);
      assert.ok(beats > 0, "restoration must resume availability heartbeats");
    }
  });
}

test("availability publication resolving after suspension cannot open a room", async (t) => {
  const { mountPeerPresence } = await import("../../web/peer-presence.js");
  let resolvePublication;
  let notifyRequested;
  const requested = new Promise((resolve) => {
    notifyRequested = resolve;
  });
  t.mock.method(globalThis, "fetch", async () => {
    notifyRequested();
    return new Promise((resolve) => {
      resolvePublication = () =>
        resolve(
          new Response(
            JSON.stringify({
              availabilityId: "mine",
              url: "wss://example",
              token: "token",
            }),
            { status: 200 },
          ),
        );
    });
  });
  let connects = 0;
  const optIn = new EventTarget();
  optIn.checked = true;
  const presence = mountPeerPresence({
    nodes: { optIn, status: {} },
    sdk: {
      Room: class {
        async connect() {
          connects++;
        }
        async disconnect() {}
      },
    },
    onAvailabilityChanged() {},
  });
  const pending = presence.enter({ language: "javascript" });
  await requested;
  await presence.stop({ persisted: true });
  resolvePublication();
  await pending;
  assert.equal(connects, 0);
});
