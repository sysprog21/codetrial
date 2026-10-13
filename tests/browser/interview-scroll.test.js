import { after, before, test } from "node:test";
import assert from "node:assert/strict";

import {
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  startStaticServer,
} from "./source.js";

let browser;
let server;
let base;

before(async () => {
  browser = await launchChromium();
  if (browser)
    ({ server, base } = await startStaticServer({
      runtimeConfig: DEFAULT_RUNTIME_CONFIG,
    }));
});

test("a short interview can scroll to the bottom of Jim's stage", async (t) => {
  if (!browser) return t.skip("Chromium is not installed");
  const page = await browser.newPage({
    viewport: { width: 1280, height: 160 },
  });
  try {
    await openInterview(page);
    await page.evaluate(() => {
      document.querySelector("#candidate-case").open = false;
    });
    const bottom = await page.locator("#captions-bar").boundingBox();
    assert.ok(bottom.y + bottom.height > 160, "the stage needs scrolling");
    await page.mouse.move(1275, 80);
    await page.mouse.wheel(0, Math.ceil(bottom.y + bottom.height - 150));
    const reachable = await page
      .waitForFunction(
        () => {
          const captions = document.querySelector("#captions-bar");
          const box = captions.getBoundingClientRect();
          return (
            box.bottom <= innerHeight &&
            document
              .elementFromPoint(box.left + box.width / 2, box.bottom - 4)
              ?.closest("#captions-bar") === captions
          );
        },
        null,
        { timeout: 2000 },
      )
      .then(
        () => true,
        () => false,
      );
    assert.ok(
      reachable,
      "the fixed stage stays clipped while the page scrolls",
    );
    await page.setViewportSize({ width: 1280, height: 720 });
    await page.waitForFunction(
      () =>
        getComputedStyle(document.querySelector("#jim-stage")).position ===
        "fixed",
    );
  } finally {
    await page.close();
  }
});

test("Jim's stage follows changing captions", async (t) => {
  if (!browser) return t.skip("Chromium is not installed");
  const page = await browser.newPage({
    viewport: { width: 1280, height: 480 },
  });
  try {
    await openInterview(page);
    await page.waitForFunction(() => {
      const stage = document.querySelector("#jim-stage");
      const box = stage.getBoundingClientRect();
      const toolbar = document
        .querySelector(".editor-toolbar")
        .getBoundingClientRect();
      return (
        getComputedStyle(stage).position === "fixed" &&
        box.top >= toolbar.bottom &&
        box.bottom <= innerHeight
      );
    });
    await page.evaluate(() => {
      document.querySelector("#captions-text").textContent =
        "A longer interviewer caption. ".repeat(40);
    });
    await page.waitForFunction(
      () =>
        getComputedStyle(document.querySelector("#jim-stage")).position ===
        "absolute",
      null,
      { timeout: 2000 },
    );
    await page.evaluate(() => {
      document.querySelector("#captions-text").textContent = "Listening...";
    });
    await page.waitForFunction(
      () =>
        getComputedStyle(document.querySelector("#jim-stage")).position ===
        "fixed",
    );
  } finally {
    await page.close();
  }
});

test("Jim's compact stage stays inside a narrow viewport", async (t) => {
  if (!browser) return t.skip("Chromium is not installed");
  const page = await browser.newPage({
    viewport: { width: 240, height: 480 },
  });
  try {
    await openInterview(page);
    const box = await page.locator("#jim-stage").boundingBox();
    assert.ok(
      box.x >= 0 && box.x + box.width <= 240,
      "captions are clipped horizontally",
    );
  } finally {
    await page.close();
  }
});

test("widening a short viewport rechecks the stage after the avatar appears", async (t) => {
  if (!browser) return t.skip("Chromium is not installed");
  const page = await browser.newPage({
    viewport: { width: 1190, height: 360 },
  });
  try {
    await openInterview(page);
    await page.waitForFunction(
      () =>
        getComputedStyle(document.querySelector("#jim-avatar")).display ===
        "none",
    );
    await page.setViewportSize({ width: 1280, height: 360 });
    await page.waitForFunction(() => {
      const stage = document.querySelector("#jim-stage");
      return (
        getComputedStyle(document.querySelector("#jim-avatar")).display !==
          "none" && getComputedStyle(stage).position === "absolute"
      );
    });
  } finally {
    await page.close();
  }
});

test("a shorter caption after page scrolling leaves Jim below the toolbar", async (t) => {
  if (!browser) return t.skip("Chromium is not installed");
  const page = await browser.newPage({
    viewport: { width: 1280, height: 500 },
  });
  try {
    await openInterview(page);
    await page.evaluate(() => {
      document.querySelector("#captions-text").textContent =
        "A longer interviewer caption. ".repeat(40);
    });
    await page.waitForFunction(() =>
      document.querySelector("#jim-stage").classList.contains("scrollable"),
    );
    await page.mouse.move(1275, 280);
    await page.mouse.wheel(0, 50);
    await page.waitForFunction(() => scrollY > 0);
    await page.evaluate(() => {
      document.querySelector("#captions-text").textContent =
        "A shorter caption. ".repeat(8);
    });
    await waitForStageLayout(page);
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.waitForFunction(() => scrollY === 0);
    const clearsToolbar = await page.evaluate(() => {
      const toolbar = document
        .querySelector(".editor-toolbar")
        .getBoundingClientRect();
      const stage = document
        .querySelector("#jim-stage")
        .getBoundingClientRect();
      return stage.top >= toolbar.bottom;
    });
    assert.ok(
      clearsToolbar,
      "the stage keeps the scrolled toolbar offset after returning to the top",
    );
  } finally {
    await page.close();
  }
});

test("caption changes after panel scrolling keep Jim's top reachable", async (t) => {
  if (!browser) return t.skip("Chromium is not installed");
  const page = await browser.newPage({
    viewport: { width: 1280, height: 360 },
  });
  try {
    await openInterview(page);
    const top = await page.locator("#jim-stage").boundingBox();
    await page.mouse.move(1260, 180);
    await page.mouse.wheel(0, 2000);
    await page.waitForFunction(
      () => document.querySelector(".editor-panel").scrollTop > 100,
    );
    await page.evaluate(() => {
      document.querySelector("#captions-text").textContent =
        "A longer interviewer caption. ".repeat(40);
    });
    await page.waitForFunction(
      () => document.querySelector("#captions-bar").scrollHeight > 100,
    );
    await waitForStageLayout(page);
    const reachable = await page
      .waitForFunction(
        (top) => {
          const stage = document
            .querySelector("#jim-stage")
            .getBoundingClientRect();
          return stage.top + scrollY >= top;
        },
        top.y,
        { timeout: 2000 },
      )
      .then(
        () => true,
        () => false,
      );
    assert.ok(
      reachable,
      "panel scrolling moves the stage above the document's reachable top",
    );
  } finally {
    await page.close();
  }
});

after(async () => {
  await browser?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
});

async function openInterview(page) {
  await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
    waitUntil: "domcontentloaded",
  });
  await page.waitForSelector("#editor:not([disabled])");
  await page.evaluate(() => {
    document.querySelector("#audio-check").hidden = true;
    document.querySelector("#jim-stage").inert = false;
    document.querySelector("#candidate-case").open = true;
  });
}

async function waitForStageLayout(page) {
  // Caption DOM changes precede ResizeObserver's placement. Cross a rendering
  // update so assertions cannot pass against the previous stage position.
  await page.evaluate(
    () =>
      new Promise((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(resolve)),
      ),
  );
}

test("a short interview viewport can scroll to the test results", async (t) => {
  if (!browser) return t.skip("Chromium is not installed");
  const page = await browser.newPage({
    viewport: { width: 1280, height: 360 },
  });
  try {
    await openInterview(page);
    // Locator clicks scroll clipped ancestors programmatically, which a
    // candidate's mouse wheel cannot do for overflow: hidden.
    await page.mouse.move(1260, 180);
    await page.mouse.wheel(0, 2000);
    await page.waitForFunction(
      () =>
        window.scrollY > 0 ||
        document.querySelector(".editor-panel").scrollTop > 0,
    );
    await page.mouse.wheel(0, 2000);
    const reachable = await page
      .waitForFunction(
        () => {
          const button = document.querySelector("#results-toggle");
          const box = button.getBoundingClientRect();
          return (
            box.top >= 0 &&
            box.bottom <= innerHeight &&
            document
              .elementFromPoint(
                box.left + box.width / 2,
                box.top + box.height / 2,
              )
              ?.closest("#results-toggle") === button
          );
        },
        null,
        { timeout: 2000 },
      )
      .then(
        () => true,
        () => false,
      );
    assert.ok(reachable, "the results button is clipped instead of scrollable");
    await page.locator("#results-toggle").click();
  } finally {
    await page.close();
  }
});

test("a narrow interview can scroll horizontally to an enlarged input", async (t) => {
  if (!browser) return t.skip("Chromium is not installed");
  const page = await browser.newPage({
    viewport: { width: 320, height: 480 },
  });
  try {
    const session = await page.context().newCDPSession(page);
    await session.send("Page.setFontSizes", {
      fontSizes: { standard: 32 },
    });
    await openInterview(page);
    await page.evaluate(() => {
      window.scrollTo(0, document.documentElement.scrollHeight);
    });
    const box = await page.locator("#candidate-case-expected").boundingBox();
    await page.mouse.move(240, box.y + box.height / 2);
    await page.mouse.wheel(2000, 0);
    const reachable = await page
      .waitForFunction(
        () => {
          const input = document.querySelector("#candidate-case-expected");
          const box = input.getBoundingClientRect();
          return (
            box.right <= innerWidth &&
            document.elementFromPoint(
              box.right - 4,
              box.top + box.height / 2,
            ) === input
          );
        },
        null,
        { timeout: 2000 },
      )
      .then(
        () => true,
        () => false,
      );
    assert.ok(reachable, "the input's right edge is clipped");
  } finally {
    await page.close();
  }
});
