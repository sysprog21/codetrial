import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { launchChromium, startStaticServer } from "./source.js";
import { INVITE_STORAGE_KEY } from "../../web/peer-lib.js";
let browser, server, base;
before(async () => {
  browser = await launchChromium();
  if (browser) ({ server, base } = await startStaticServer());
});
after(async () => {
  await browser?.close();
  await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
});
const initial = {
  sessionId: "session-a",
  status: "pending",
  version: 1,
  you: { slot: 1 },
  members: [
    { slot: 0, login: "alice", identity: "a", entered: false, ready: false },
    { slot: 1, login: "bob", identity: "b", entered: false, ready: false },
  ],
  currentRound: null,
};
async function setup(
  t,
  account = {
    signedIn: true,
    peerEnabled: true,
    identityVerified: true,
    user: { login: "bob" },
  },
) {
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
    route.fulfill({ json: account }),
  );
  await page.route("**/api/peer/sessions", (route) =>
    route.fulfill({ json: { sessions: [initial] } }),
  );
  await page.route("**/api/peer/sessions/session-a", (route) =>
    route.fulfill({ json: initial }),
  );
  return page;
}
test("signed-out invite survives fragment clearing and GitHub sign-in", async (t) => {
  const page = await setup(t, { signedIn: false });
  if (!page) return;
  await page.route("**/api/login?next=peer-join", (route) =>
    route.fulfill({ contentType: "text/html", body: "Sign in" }),
  );
  await page.goto(`${base}/peer-lobby.html#invite=secret`);
  await page.waitForURL("**/api/login?next=peer-join");
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      INVITE_STORAGE_KEY,
    ),
    "secret",
  );
});
test("signed-in invite accepted once, token cleared, start waits for both ready", async (t) => {
  const page = await setup(t);
  if (!page) return;
  let accepts = 0;
  await page.route("**/api/peer/invites/accept", (route) => {
    accepts++;
    assert.equal(route.request().postDataJSON().token, "secret");
    return route.fulfill({ json: initial });
  });
  await page.goto(`${base}/peer-lobby.html#invite=secret`);
  await page.locator("#peer-session").waitFor({ state: "visible" });
  await page.waitForFunction(
    () => !sessionStorage.getItem("codetrial:peerInvite"),
  );
  assert.equal(accepts, 1);
  assert.equal(new URL(page.url()).hash, "");
  assert.equal(await page.locator("#peer-start").isDisabled(), true);
});
test("unavailable invitation explains recovery", async (t) => {
  const page = await setup(t);
  if (!page) return;
  await page.route("**/api/peer/invites/accept", (route) =>
    route.fulfill({ status: 410, json: { code: "invite_unavailable" } }),
  );
  await page.goto(`${base}/peer-lobby.html#invite=expired`);
  await page.waitForFunction(() =>
    document.getElementById("peer-status").textContent.includes("unavailable"),
  );
  assert.match(await page.locator("#peer-status").textContent(), /new link/);
});
test("a rejected invitation is forgotten, so a reload does not retry it", async (t) => {
  const page = await setup(t);
  if (!page) return;
  let accepts = 0;
  await page.route("**/api/peer/invites/accept", (route) => {
    accepts++;
    return route.fulfill({ status: 410, json: { code: "invite_unavailable" } });
  });
  await page.goto(`${base}/peer-lobby.html#invite=expired`);
  await page.waitForFunction(() =>
    document.getElementById("peer-status").textContent.includes("unavailable"),
  );
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      INVITE_STORAGE_KEY,
    ),
    null,
  );
  await page.reload();
  await page.locator("#peer-content").waitFor({ state: "visible" });
  assert.equal(accepts, 1, "a reload retried the dead invitation");
});
test("a failed session list read does not stop the session from polling", async (t) => {
  const page = await setup(t);
  if (!page) return;
  let reads = 0;
  await page.route("**/api/peer/invites/accept", (route) =>
    route.fulfill({ json: initial }),
  );
  await page.route("**/api/peer/sessions/session-a", (route) => {
    reads++;
    return route.fulfill({ json: initial });
  });
  await page.route("**/api/peer/sessions", (route) =>
    route.fulfill({ status: 500, json: { code: "database_unavailable" } }),
  );
  await page.clock.install();
  await page.goto(`${base}/peer-lobby.html#invite=secret`);
  await page.locator("#peer-session").waitFor({ state: "visible" });
  await page.waitForFunction(() =>
    document.getElementById("peer-status").textContent.includes("busy"),
  );
  const settled = reads;
  for (let tick = 0; reads === settled && tick < 10; tick++) {
    await page.clock.runFor(5000);
    // Yield so the poll's fetch response is handled before the next tick.
    await page.waitForTimeout(50);
  }
  assert.ok(reads > settled, "the session was never polled again");
});
test("a session that appears is announced in words and takes focus", async (t) => {
  const page = await setup(t);
  if (!page) return;
  await page.route("**/api/peer/invites/accept", (route) =>
    route.fulfill({ json: initial }),
  );
  await page.goto(`${base}/peer-lobby.html#invite=secret`);
  await page.locator("#peer-session").waitFor({ state: "visible" });
  assert.equal(
    await page.evaluate(() => document.activeElement?.id),
    "session-title",
  );
  assert.equal(
    await page.locator("#peer-session-status").textContent(),
    "Waiting for you both to enter and get ready.",
  );
  assert.equal(
    await page.locator("#peer-session-status").getAttribute("role"),
    "status",
  );
});

test("the round length field stops at the longest round the server accepts", async (t) => {
  const page = await setup(t, {
    signedIn: true,
    peerEnabled: true,
    peerMaxRoundMinutes: 45,
    identityVerified: true,
    user: { login: "bob" },
  });
  if (!page) return;
  await page.goto(`${base}/peer-lobby.html`);
  await page.locator("#peer-content").waitFor({ state: "visible" });
  assert.equal(
    await page.locator('[name="roundMinutes"]').getAttribute("max"),
    "45",
  );
});
