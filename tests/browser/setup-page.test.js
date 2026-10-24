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
// What /api/setup/offer answers, or null for the 404 of no offer.
let offered = null;

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
      if (url.pathname === "/api/setup/offer") {
        response.statusCode = offered ? 200 : 404;
        response.setHeader("content-type", "application/json");
        response.end(JSON.stringify(offered ?? {}));
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

const configFile = (name, text) => ({
  name,
  mimeType: "text/plain",
  buffer: Buffer.from(text),
});

test("setup fills its fields from a chosen config file without sending it", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  submissions = [];
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(`${base}/setup`);
  assert.notEqual(
    await page
      .locator("#import-status")
      .evaluate((element) => getComputedStyle(element).display),
    "none",
    "the import status region is present before its first update",
  );
  assert.equal(
    await page.locator("#import-file").isVisible(),
    false,
    "the button stands in for the file input",
  );

  // What read_config_file accepts: comments, blank lines, spaces around the
  // equals sign, quotes at either end, and keys Setup does not take.
  await page
    .locator("#import-file")
    .setInputFiles(
      configFile(
        "codetrial.env.local",
        [
          "# saved by the CLI",
          "",
          'LIVEKIT_URL = "wss://example.livekit.cloud"',
          "LIVEKIT_API_KEY=key",
          "LIVEKIT_API_SECRET='secret'",
          "GOOGLE_API_KEY=google",
          "CODETRIAL_DB_PATH=/elsewhere/codetrial.db",
        ].join("\r\n"),
      ),
    );
  await page.locator("#import-status.ok").waitFor();
  assert.match(
    await page.locator("#import-status").innerText(),
    /^Filled from codetrial\.env\.local\./,
  );
  assert.equal(submissions.length, 0, "importing sends nothing");
  for (const [field, value] of [
    ["#livekitUrl", "wss://example.livekit.cloud"],
    ["#livekitApiKey", "key"],
    ["#livekitApiSecret", "secret"],
    ["#googleApiKey", "google"],
  ]) {
    assert.equal(await page.locator(field).inputValue(), value, field);
  }

  await page.locator("#submit").click();
  await page.locator("#status.error").waitFor();
  assert.deepEqual(submissions, [
    {
      livekitUrl: "wss://example.livekit.cloud",
      livekitApiKey: "key",
      livekitApiSecret: "secret",
      googleApiKey: "google",
    },
  ]);
});

test("setup takes the first key of an imported rotation list and says so", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(`${base}/setup`);
  await page
    .locator("#import-file")
    .setInputFiles(
      configFile(
        "rotation.env",
        [
          "LIVEKIT_URL=wss://example.livekit.cloud",
          "LIVEKIT_API_KEY=key",
          "LIVEKIT_API_SECRET=secret",
          "GOOGLE_API_KEY=single",
          "GOOGLE_API_KEYS= first , second ",
        ].join("\n"),
      ),
    );
  await page.locator("#import-status.ok").waitFor();
  assert.equal(await page.locator("#googleApiKey").inputValue(), "first");
  assert.match(
    await page.locator("#import-status").innerText(),
    /GOOGLE_API_KEYS lists 2 keys; Setup saves the first/,
  );
});

test("setup refuses an imported file it cannot read and keeps the fields", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(`${base}/setup`);
  await page.locator("#livekitApiKey").fill("typed");

  await page
    .locator("#import-file")
    .setInputFiles(
      configFile(
        "broken.env",
        "LIVEKIT_URL=wss://example.livekit.cloud\nnot a setting\n",
      ),
    );
  await page.locator("#import-status.error").waitFor();
  assert.equal(
    await page.locator("#import-status").innerText(),
    "broken.env: not a KEY=value line: not a setting",
  );
  assert.equal(await page.locator("#livekitApiKey").inputValue(), "typed");

  await page
    .locator("#import-file")
    .setInputFiles(
      configFile("partial.env", "LIVEKIT_URL=wss://example.livekit.cloud\n"),
    );
  // The status already shows the last refusal, so wait for this one.
  await page
    .locator("#import-status.error", { hasText: "partial.env" })
    .waitFor();
  assert.match(
    await page.locator("#import-status").innerText(),
    /^partial\.env does not set LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET\.$/,
  );
  assert.equal(await page.locator("#livekitApiKey").inputValue(), "typed");
  assert.equal(await page.locator("#livekitUrl").inputValue(), "");
});

test("setup offers nothing when the launcher found no config", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  offered = null;
  const page = await browser.newPage();
  t.after(() => page.close());
  const asked = page.waitForResponse("**/api/setup/offer");
  await page.goto(`${base}/setup`);
  await asked;
  assert.equal(await page.locator("#offer").isVisible(), false);
  assert.equal(await page.locator("#import-status").innerText(), "");
});

test("setup names an offered config and uses it without the keys", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  submissions = [];
  offered = {
    path: "/home/me/checkout/config/codetrial.env.local",
    livekitEndpoint: "wss://example.livekit.cloud",
    google: true,
    note: null,
  };
  t.after(() => {
    offered = null;
  });
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(`${base}/setup`);
  await page.locator("#offer").waitFor();
  assert.equal(
    await page.locator("#offer-text").innerText(),
    "Keys found in /home/me/checkout/config/codetrial.env.local: LiveKit " +
      "wss://example.livekit.cloud, with a Google key.",
  );

  await page.locator("#use-offer").click();
  await page.locator("#offer-status.error").waitFor();
  assert.deepEqual(submissions, [{ useOffer: true }]);
  assert.equal(
    await page.locator("#offer-status").innerText(),
    "Check the LiveKit credentials",
  );
  assert.equal(await page.locator("#use-offer").isEnabled(), true);
  assert.equal(await page.locator("#submit").isEnabled(), true);
  for (const field of ["#livekitUrl", "#livekitApiKey", "#livekitApiSecret"]) {
    assert.equal(await page.locator(field).inputValue(), "", field);
  }
});

test("setup reports an offered config it cannot use", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  offered = {
    path: "/home/me/codetrial.env.local",
    error:
      "/home/me/codetrial.env.local does not set LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET.",
  };
  t.after(() => {
    offered = null;
  });
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(`${base}/setup`);
  await page.locator("#import-status.error").waitFor();
  assert.match(
    await page.locator("#import-status").innerText(),
    /^\/home\/me\/codetrial\.env\.local does not set LIVEKIT_URL/,
  );
  assert.equal(await page.locator("#offer").isVisible(), false);
});
