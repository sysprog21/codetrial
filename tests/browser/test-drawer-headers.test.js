// Run with: node --test tests/browser/test-drawer-headers.test.js

import { after, before, test } from "node:test";
import assert from "node:assert/strict";

import {
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  startStaticServer,
} from "./source.js";

let browser = null;
let server = null;
let base = "";

before(async () => {
  browser = await launchChromium();
  if (!browser) return;
  ({ server, base } = await startStaticServer({
    runtimeConfig: DEFAULT_RUNTIME_CONFIG,
  }));
});

after(async () => {
  await browser?.close();
  await new Promise((closed) => (server ? server.close(closed) : closed()));
});

async function waitForTransformTransition(marker, action) {
  await action();

  await marker.evaluate(async (el) => {
    const deadline = performance.now() + 1500;
    let previous = getComputedStyle(el).transform;
    let stableFrames = 0;

    while (performance.now() < deadline) {
      await new Promise(requestAnimationFrame);

      const current = getComputedStyle(el).transform;
      const animating = el
        .getAnimations()
        .some((animation) => animation.playState === "running");

      if (!animating && current === previous) {
        stableFrames += 1;
        if (stableFrames >= 4) return;
      } else {
        stableFrames = 0;
      }

      previous = current;
    }

    throw new Error("Chevron transform did not settle");
  });
}

test("drawer headers share typography and toggle their chevrons", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");

  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html`, {
      waitUntil: "domcontentloaded",
    });
    await page.waitForFunction(
      () => document.querySelector("#candidate-case-input").placeholder !== "",
    );

    await page.evaluate(() => {
      document.querySelector("#audio-check").hidden = true;
    });
    const headers = [
      {
        selector: "#candidate-case > summary",
        markerSelector: "#candidate-case > summary .drawer-chevron",
        open: async () => {
          await page.locator("#candidate-case > summary").click();
        },
        close: async () => {
          await page.locator("#candidate-case > summary").click();
        },
      },
      {
        selector: "#results-toggle",
        markerSelector: "#results-chevron",
        open: async () => {
          await page.locator("#results-toggle").click();
        },
        close: async () => {
          await page.locator("#results-toggle").click();
        },
      },
    ];
    const resultsToggle = page.locator("#results-toggle");

    if ((await resultsToggle.getAttribute("aria-expanded")) === "true") {
      await resultsToggle.click();
    }

    await page.waitForFunction(() => {
      const button = document.querySelector("#results-toggle");
      const body = document.querySelector("#results-body");
      return button.getAttribute("aria-expanded") === "false" && body.hidden;
    });

    const styles = await Promise.all(
      headers.map(async ({ selector, markerSelector }) => {
        const header = page.locator(selector);
        const marker = page.locator(markerSelector);

        const headerStyle = await header.evaluate((el) => {
          const style = getComputedStyle(el);
          return {
            fontFamily: style.fontFamily,
            fontSize: style.fontSize,
            fontWeight: style.fontWeight,
            lineHeight: style.lineHeight,
          };
        });

        return headerStyle;
      }),
    );

    assert.deepEqual(
      {
        fontFamily: styles[0].fontFamily,
        fontSize: styles[0].fontSize,
        fontWeight: styles[0].fontWeight,
        lineHeight: styles[0].lineHeight,
      },
      {
        fontFamily: styles[1].fontFamily,
        fontSize: styles[1].fontSize,
        fontWeight: styles[1].fontWeight,
        lineHeight: styles[1].lineHeight,
      },
      "both headers should share typography",
    );

    for (const header of headers) {
      const marker = page.locator(header.markerSelector);
      const toggle = page.locator(header.selector);

      assert.equal(await marker.count(), 1);

      const closedTransform = await marker.evaluate(
        (el) => getComputedStyle(el).transform,
      );

      await waitForTransformTransition(marker, header.open);

      const openTransform = await marker.evaluate(
        (el) => getComputedStyle(el).transform,
      );

      if (header.selector === "#results-toggle") {
        await page.waitForFunction(() => {
          const button = document.querySelector("#results-toggle");
          const body = document.querySelector("#results-body");
          return (
            button.getAttribute("aria-expanded") === "true" &&
            !body.hidden
          );
        });
      } else {
        assert.equal(
          await page.locator("#candidate-case").evaluate((el) => el.open),
          true,
          "candidate case should be open",
        );
      }

      assert.notEqual(
        openTransform,
        closedTransform,
        `${header.selector} chevron should change when opened`,
      );

      await waitForTransformTransition(marker, header.close);

      const restoredTransform = await marker.evaluate(
        (el) => getComputedStyle(el).transform,
      );

      if (header.selector === "#results-toggle") {
        assert.equal(
          await toggle.getAttribute("aria-expanded"),
          "false",
          "results toggle should announce that the panel is closed",
        );
        assert.equal(
          await page.locator("#results-body").evaluate((el) => el.hidden),
          true,
          "results body should be hidden when closed",
        );
      } else {
        assert.equal(
          await page.locator("#candidate-case").evaluate((el) => el.open),
          false,
          "candidate case should be closed",
        );
      }

      assert.equal(
        restoredTransform,
        closedTransform,
        `${header.selector} chevron should return when closed`,
      );
    }
  } finally {
    await page.close();
  }
});
