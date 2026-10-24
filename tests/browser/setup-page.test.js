// Run with: node --test tests/browser/setup-page.test.js
// The setup page is embedded in Rust, so serve that exact HTML to Chromium.

import { after, before, test } from "node:test";
import assert from "node:assert/strict";

import { launchChromium, read, startStaticServer } from "./source.js";

const source = read("src/web/setup.rs");
const start = source.indexOf('const SETUP_PAGE: &str = r#"');
assert.notEqual(start, -1, "setup page constant exists");
const htmlStart = start + 'const SETUP_PAGE: &str = r#"'.length;
const htmlEnd = source.indexOf('"#;', htmlStart);
assert.notEqual(htmlEnd, -1, "setup page constant ends");
const html = source.slice(htmlStart, htmlEnd);
const lobbyCss = read("web/styles.css");
const paletteTokens = [
  ...new Set([...html.matchAll(/^\s+(--[a-z-]+):/gm)].map((match) => match[1])),
];
assert.ok(paletteTokens.length > 0, "setup palette tokens exist");

let browser = null;
let server = null;
let base = "";
let submissions = [];

before(async () => {
  browser = await launchChromium();
  if (!browser) return;
  ({ server, base } = await startStaticServer({
    handle: async (request, response, url) => {
      if (url.pathname === "/setup") {
        response.setHeader("content-type", "text/html");
        response.end(html);
        return true;
      }
      if (url.pathname === "/api/setup") {
        let body = "";
        for await (const chunk of request) body += chunk;
        submissions.push(JSON.parse(body));
        response.statusCode = 400;
        response.setHeader("content-type", "application/json");
        response.end(
          JSON.stringify({ error: "Check the LiveKit credentials" }),
        );
        return true;
      }
      return false;
    },
  }));
});

after(async () => {
  await browser?.close();
  await new Promise((closed) => (server ? server.close(closed) : closed()));
});

test("setup palette stays aligned with the lobby", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  for (const colorScheme of ["light", "dark"]) {
    const setupPage = await browser.newPage({ colorScheme });
    const lobbyPage = await browser.newPage({ colorScheme });
    t.after(() => Promise.all([setupPage.close(), lobbyPage.close()]));
    await setupPage.goto(`${base}/setup`);
    await lobbyPage.setContent('<body class="practice-page"></body>');
    await lobbyPage.addStyleTag({ content: lobbyCss });

    const tokensOn = (page, selector) =>
      page.locator(selector).evaluate((element, names) => {
        const style = getComputedStyle(element);
        return Object.fromEntries(
          names.map((name) => [name, style.getPropertyValue(name)]),
        );
      }, paletteTokens);
    const setupTokens = await tokensOn(setupPage, ":root");
    const lobbyTokens = await tokensOn(lobbyPage, ".practice-page");
    for (const name of paletteTokens) {
      const compact = (value) => value.replace(/\s+/g, "");
      assert.equal(
        compact(setupTokens[name]),
        compact(lobbyTokens[name]),
        `${colorScheme} ${name} differs from the lobby`,
      );
    }
  }
});

test("setup shows distinct credential groups without narrow-screen overflow", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  for (const colorScheme of ["light", "dark"]) {
    const page = await browser.newPage({
      viewport: { width: 320, height: 800 },
      colorScheme,
    });
    t.after(() => page.close());
    await page.goto(`${base}/setup`);
    assert.equal(await page.locator("fieldset").count(), 2);
    assert.match(
      await page.locator("fieldset").first().innerText(),
      /Required/i,
    );
    assert.match(
      await page.locator("fieldset").last().innerText(),
      /Optional/i,
    );
    // Beside the key, so whoever enters it learns whose project pays.
    assert.match(
      await page.locator("fieldset").last().innerText(),
      /Gemini Live usage is billed to this server's Google AI project, on the whole context of the session/,
    );
    assert.equal(await page.locator("input:required").count(), 3);
    assert.equal(await page.locator("#googleApiKey:required").count(), 0);
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
      true,
      `${colorScheme} setup overflows a 320px viewport`,
    );
    const button = page.locator("#submit");
    const bodyColor = await page
      .locator("body")
      .evaluate((el) => getComputedStyle(el).backgroundColor);
    assert.equal(
      bodyColor,
      colorScheme === "light" ? "rgb(246, 246, 249)" : "rgb(17, 17, 15)",
    );
    assert.equal(
      await button.evaluate((el) => getComputedStyle(el).backgroundColor),
      colorScheme === "light" ? "rgb(110, 43, 240)" : "rgb(116, 76, 230)",
    );
  }
});

test("setup validates required fields and preserves retry after server error", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  submissions = [];
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(`${base}/setup`);
  assert.notEqual(
    await page
      .locator("#status")
      .evaluate((element) => getComputedStyle(element).display),
    "none",
    "the live status region is present before its first update",
  );
  await page.locator("#submit").click();
  assert.equal(submissions.length, 0, "empty required fields block submission");

  await page.locator("#livekitUrl").fill("wss://example.livekit.cloud");
  await page.locator("#livekitApiKey").fill("key");
  await page.locator("#livekitApiSecret").fill("secret");
  await page.locator("#submit").click();
  await page.locator("#status.error").waitFor();
  assert.equal(
    await page.locator("#status").innerText(),
    "Check the LiveKit credentials",
  );
  assert.equal(await page.locator("#submit").isEnabled(), true);
  assert.deepEqual(submissions, [
    {
      livekitUrl: "wss://example.livekit.cloud",
      livekitApiKey: "key",
      livekitApiSecret: "secret",
      googleApiKey: "",
    },
  ]);
});
