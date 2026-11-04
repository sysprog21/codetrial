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
async function fixture(t) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.route("**/api/peer/profile", (route) =>
    route.fulfill({ json: { nickname: "Lucky7" } }),
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
  await page.route("**/api/peer/sessions", (route) =>
    route.fulfill({ json: { sessions: [] } }),
  );
  await page.route("**/vendor/livekit-client.js", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body: `window.waitingConnects=0;window.waitingDisconnects=0;window.LivekitClient={Room:class{async connect(){window.waitingConnects++}async disconnect(){window.waitingDisconnects++}}};`,
    }),
  );
  return page;
}
test("availability requires opt-in and opting out leaves the waiting room", async (t) => {
  const page = await fixture(t);
  if (!page) return;
  let published = 0,
    removed = 0;
  let active = false;
  await page.route("**/api/peer/lobby", (route) =>
    route.fulfill({
      json: {
        available: [],
        requests: [],
        you: { availabilityId: active ? "mine" : null },
      },
    }),
  );
  await page.route("**/api/peer/availability", (route) => {
    if (route.request().method() === "DELETE") {
      removed++;
      active = false;
      return route.fulfill({ json: {} });
    }
    published++;
    active = true;
    return route.fulfill({
      json: {
        availabilityId: "mine",
        url: "wss://example",
        token: "token",
        room: "waiting-room",
      },
    });
  });
  await page.goto(`${base}/peer-lobby.html`);
  await page.locator("#peer-content").waitFor({ state: "visible" });
  assert.equal(await page.locator("#peer-available").isDisabled(), true);
  assert.equal(published, 0);
  await page.locator("#peer-opt-in").check();
  await page.locator("#peer-available").click();
  await page.waitForFunction(() => window.waitingConnects === 1);
  await page.waitForFunction(() =>
    document
      .getElementById("peer-presence-status")
      .textContent.includes("You are visible"),
  );
  assert.equal(removed, 0);
  assert.equal(published, 1);
  await page.locator("#peer-opt-in").uncheck();
  await page.waitForFunction(() => window.waitingDisconnects === 1);
  assert.equal(removed, 1);
});
test("opting out while availability is being created withdraws it", async (t) => {
  const page = await fixture(t);
  if (!page) return;
  let removed = 0;
  let release;
  const gate = new Promise((resolve) => (release = resolve));
  await page.route("**/api/peer/lobby", (route) =>
    route.fulfill({
      json: { available: [], requests: [], you: { availabilityId: null } },
    }),
  );
  await page.route("**/api/peer/availability", async (route) => {
    if (route.request().method() === "DELETE") {
      removed++;
      return route.fulfill({ json: {} });
    }
    await gate;
    return route.fulfill({
      json: { availabilityId: "mine", url: "wss://example", token: "token" },
    });
  });
  await page.goto(`${base}/peer-lobby.html`);
  await page.locator("#peer-content").waitFor({ state: "visible" });
  await page.locator("#peer-opt-in").check();
  await page.locator("#peer-available").click();
  await page.locator("#peer-opt-in").uncheck();
  release();
  // Outwaits the late availability response and the withdrawal it triggers.
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal(removed, 1);
  assert.equal(await page.evaluate(() => window.waitingConnects ?? 0), 0);
});
test("a lobby poll during availability creation does not undo the opt-out", async (t) => {
  const page = await fixture(t);
  if (!page) return;
  await page.addInitScript(() => {
    window.peerTicks = [];
    const original = window.setInterval;
    window.setInterval = (fn, ms, ...args) =>
      ms === 5000
        ? (window.peerTicks.push(fn), 9999)
        : original(fn, ms, ...args);
  });
  let provisioning = false;
  let removed = 0;
  let release;
  const gate = new Promise((resolve) => (release = resolve));
  let polls = 0;
  await page.route("**/api/peer/lobby", (route) => {
    polls++;
    return route.fulfill({
      json: {
        available: [],
        requests: [],
        you: { availabilityId: provisioning ? "mine" : null },
      },
    });
  });
  await page.route("**/api/peer/availability", async (route) => {
    if (route.request().method() === "DELETE") {
      removed++;
      return route.fulfill({ json: {} });
    }
    provisioning = true;
    await gate;
    return route.fulfill({
      json: { availabilityId: "mine", url: "wss://example", token: "token" },
    });
  });
  await page.goto(`${base}/peer-lobby.html`);
  await page.locator("#peer-content").waitFor({ state: "visible" });
  await page.locator("#peer-opt-in").check();
  await page.locator("#peer-available").click();
  await page.locator("#peer-opt-in").uncheck();
  const before = polls;
  await page.evaluate(() => window.peerTicks.forEach((tick) => tick()));
  for (let wait = 0; polls === before && wait < 250; wait++)
    await new Promise((resolve) => setTimeout(resolve, 20));
  assert.ok(polls > before, "the poll never fired");
  // Outwaits the poll's response being applied to the page.
  await new Promise((resolve) => setTimeout(resolve, 150));
  assert.equal(await page.locator("#peer-opt-in").isChecked(), false);
  release();
  // Outwaits the late availability response and the withdrawal it triggers.
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal(removed >= 1, true);
  assert.equal(await page.evaluate(() => window.waitingConnects ?? 0), 0);
});
test("roster join request needs both confirmations and explicit observer consent", async (t) => {
  const page = await fixture(t);
  if (!page) return;
  let requested = 0,
    confirmed = 0;
  const request = {
    requestId: "r",
    fromLogin: "alice",
    toLogin: "bob",
    fromNickname: "Lucky7",
    toNickname: "Lucky7",
    status: "pending",
    confirmedByYou: false,
    confirmedByPartner: false,
    settings: { roundMinutes: 25 },
  };
  await page.route("**/api/peer/lobby", (route) =>
    route.fulfill({
      json: {
        available: [
          {
            availabilityId: "b",
            login: "bob",
            nickname: "Lucky7",
            settings: { difficulty: "easy", language: "python" },
          },
          {
            availabilityId: "c",
            login: "charlie",
            nickname: "Lucky7",
            settings: { difficulty: "easy", language: "python" },
          },
        ],
        requests: requested ? [request] : [],
        you: { availabilityId: null },
      },
    }),
  );
  await page.route("**/api/peer/pairing-requests", (route) => {
    requested++;
    assert.equal(route.request().postDataJSON().availabilityId, "b");
    return route.fulfill({ json: request });
  });
  await page.route("**/api/peer/pairing-requests/r/confirm", (route) => {
    confirmed++;
    assert.equal(route.request().postDataJSON().observerConsent, true);
    request.confirmedByYou = true;
    return route.fulfill({ json: request });
  });
  await page.goto(`${base}/peer-lobby.html`);
  await page.getByRole("button", { name: "Request to join bob" }).waitFor();
  assert.equal(await page.getByText("Lucky7", { exact: true }).count(), 2);
  assert.match(
    await page.locator("#peer-available-list").textContent(),
    /GitHub @bob/,
  );
  assert.match(
    await page.locator("#peer-available-list").textContent(),
    /GitHub @charlie/,
  );
  await page.getByRole("button", { name: "Request to join bob" }).click();
  const confirm = page.getByRole("button", {
    name: "Confirm practice with bob",
  });
  await confirm.waitFor();
  assert.equal(await confirm.isDisabled(), true);
  await page
    .getByLabel("I agree to a silent AI observer and post-session suggestions")
    .check();
  await confirm.click();
  assert.equal(confirmed, 1);
  await page.setViewportSize({ width: 390, height: 844 });
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    true,
  );
  await page.screenshot({
    path: "/tmp/codetrial-reciprocal-lobby-mobile.png",
    fullPage: true,
  });
  await page.waitForFunction(() =>
    document
      .getElementById("peer-requests")
      .textContent.includes("Waiting for partner confirmation"),
  );
});

test("nickname is required before roster and rejects non-alphanumeric or digit-only names", async (t) => {
  const page = await fixture(t);
  if (!page) return;
  let rosterReads = 0,
    saves = 0;
  await page.route("**/api/peer/profile", (route) => {
    if (route.request().method() === "GET")
      return route.fulfill({ json: { nickname: null } });
    saves++;
    assert.equal(route.request().postDataJSON().nickname, "Lucky7");
    return route.fulfill({ json: { nickname: "Lucky7" } });
  });
  await page.route("**/api/peer/lobby", (route) => {
    rosterReads++;
    return route.fulfill({
      json: { available: [], requests: [], you: { availabilityId: null } },
    });
  });
  await page.goto(`${base}/peer-lobby.html`);
  await page.locator("#peer-profile").waitFor({ state: "visible" });
  assert.equal(rosterReads, 0);
  assert.equal(await page.locator("#peer-content").isHidden(), true);
  for (const nickname of ["1234", "Lucky 7", "Lucky_7", "🙂"]) {
    await page.locator("#peer-nickname").fill(nickname);
    await page.locator("#peer-nickname-error").evaluate((node) => {
      node.textContent = "";
    });
    await page.locator("#peer-profile-save").click();
    assert.match(
      await page.locator("#peer-nickname-error").textContent(),
      /1 to 24 English letters or digits/,
    );
    assert.equal(saves, 0, `${nickname} must not reach the server`);
    assert.equal(rosterReads, 0);
  }
  await page.locator("#peer-nickname").fill("Lucky7");
  await page.locator("#peer-profile-save").click();
  await page.locator("#peer-content").waitFor({ state: "visible" });
  await page.waitForFunction(() =>
    document
      .getElementById("peer-available-list")
      .textContent.includes("No partners"),
  );
  assert.ok(rosterReads > 0);
});

test("historical paired requests do not reopen completed sessions", async (t) => {
  const page = await fixture(t);
  if (!page) return;
  let staleReads = 0;
  await page.route("**/api/peer/lobby", (route) =>
    route.fulfill({
      json: {
        available: [],
        you: { availabilityId: null, sessionId: null },
        requests: [
          {
            requestId: "old",
            status: "paired",
            fromLogin: "alice",
            toLogin: "bob",
            confirmedByYou: true,
            confirmedByPartner: true,
            settings: {},
            sessionId: "old-session",
          },
        ],
      },
    }),
  );
  await page.route("**/api/peer/sessions/old-session", (route) => {
    staleReads++;
    return route.fulfill({ status: 404, json: {} });
  });
  await page.goto(`${base}/peer-lobby.html`);
  await page.waitForFunction(() =>
    document
      .getElementById("peer-requests")
      .textContent.includes("Both participants confirmed"),
  );
  assert.equal(staleReads, 0);
  assert.equal(await page.locator("#peer-session").isHidden(), true);
});

test("waiting room reconnect obtains a fresh token with immutable settings", async (t) => {
  const page = await fixture(t);
  if (!page) return;
  const proposals = [];
  let active = false;
  await page.route("**/vendor/livekit-client.js", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body: `window.waitingRooms=[];window.LivekitClient={RoomEvent:{Disconnected:'disconnected'},Room:class{handlers=new Map();constructor(){window.waitingRooms.push(this)}on(k,f){this.handlers.set(k,f)}async connect(url,token){this.token=token}async disconnect(){this.closed=true;this.handlers.get('disconnected')?.()}}};`,
    }),
  );
  await page.route("**/api/peer/lobby", (route) =>
    route.fulfill({
      json: {
        available: [],
        requests: [],
        you: { availabilityId: active ? "mine" : null },
      },
    }),
  );
  await page.route("**/api/peer/availability", (route) => {
    if (route.request().method() === "DELETE") {
      active = false;
      return route.fulfill({ json: {} });
    }
    proposals.push(route.request().postDataJSON());
    active = true;
    return route.fulfill({
      json: {
        availabilityId: "mine",
        url: "wss://example",
        token: `token-${proposals.length}`,
      },
    });
  });
  await page.goto(`${base}/peer-lobby.html`);
  await page.locator("#peer-content").waitFor({ state: "visible" });
  await page.locator("#peer-opt-in").check();
  await page.locator("#peer-available").click();
  await page.waitForFunction(() => window.waitingRooms[0]?.token === "token-1");
  await page.evaluate(() =>
    window.waitingRooms[0].handlers.get("disconnected")(),
  );
  await page.waitForFunction(() => window.waitingRooms[1]?.token === "token-2");
  assert.equal(proposals.length, 2);
  assert.deepEqual(proposals[0], proposals[1]);
  assert.equal(await page.evaluate(() => window.waitingRooms[0].closed), true);
});

test("a match whose session fails to load is retried on the next poll", async (t) => {
  const page = await fixture(t);
  if (!page) return;
  let loads = 0;
  const session = {
    sessionId: "s1",
    status: "pending",
    version: 1,
    you: { slot: 0 },
    members: [{ slot: 0, login: "alice", entered: false, ready: false }],
    currentRound: null,
  };
  await page.route("**/api/peer/lobby", (route) =>
    route.fulfill({
      json: {
        available: [],
        requests: [],
        you: { availabilityId: null, sessionId: "s1" },
      },
    }),
  );
  await page.route("**/api/peer/sessions/s1", (route) => {
    loads++;
    return loads === 1
      ? route.fulfill({ status: 500, json: { code: "database_unavailable" } })
      : route.fulfill({ json: session });
  });
  await page.clock.install();
  await page.goto(`${base}/peer-lobby.html`);
  await page.locator("#peer-content").waitFor({ state: "visible" });
  for (let tick = 0; tick < 10; tick++) {
    if (await page.locator("#peer-session").isVisible()) break;
    await page.clock.runFor(5000);
    // Yield so the poll's fetch response is handled before the next tick.
    await page.waitForTimeout(50);
  }
  assert.equal(await page.locator("#peer-session").isVisible(), true);
  assert.ok(loads >= 2);
});
