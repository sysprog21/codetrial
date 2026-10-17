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
  const transitionFinished = marker.evaluate(
    (el) =>
      new Promise((resolve) => {
        const onTransitionEnd = (event) => {
          if (event.propertyName !== "transform") return;
          el.removeEventListener("transitionend", onTransitionEnd);
          resolve();
        };

        el.addEventListener("transitionend", onTransitionEnd);
      }),
  );

  await action();
  await transitionFinished;
}

test("drawer headers share typography and toggle their chevrons", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");

  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html`, {
      waitUntil: "domcontentloaded",
    });

    const headers = [
      {
        selector: "#candidate-case > summary",
        open: async () =>
          page.locator("#candidate-case").evaluate((el) => {
            el.open = true;
          }),
        close: async () =>
          page.locator("#candidate-case").evaluate((el) => {
            el.open = false;
          }),
      },
      {
        selector: "#results-toggle",
        open: async () =>
          page.locator("#results-toggle").evaluate((el) => {
            el.classList.add("open");
          }),
        close: async () =>
          page.locator("#results-toggle").evaluate((el) => {
            el.classList.remove("open");
          }),
      },
    ];

    const styles = await Promise.all(
      headers.map(({ selector }) =>
        page.locator(selector).evaluate((header) => {
          const style = getComputedStyle(header);
          const marker = header.querySelector(".drawer-chevron");
          const markerStyle = getComputedStyle(marker);
          return {
            fontFamily: style.fontFamily,
            fontSize: style.fontSize,
            fontWeight: style.fontWeight,
            lineHeight: style.lineHeight,
            markerTransform: markerStyle.transform,
            markerTransition: markerStyle.transition,
          };
        }),
      ),
    );

    assert.deepEqual(
      styles[0],
      styles[1],
      "both headers should share typography and chevron styles",
    );

    for (const header of headers) {
      const marker = page.locator(`${header.selector} .drawer-chevron`);

      assert.equal(await marker.count(), 1);

      const closedTransform = await marker.evaluate(
        (el) => getComputedStyle(el).transform,
      );

      await waitForTransformTransition(marker, header.open);

      const openTransform = await marker.evaluate(
        (el) => getComputedStyle(el).transform,
      );

      const openRotation = await marker.evaluate((el) => {
        const { a, b } = new DOMMatrixReadOnly(getComputedStyle(el).transform);
        return Math.round(Math.atan2(b, a) * (180 / Math.PI));
      });

      assert.equal(
        openRotation,
        90,
        `${header.selector} chevron should rotate 90 degrees when opened`,
      );

      assert.notEqual(
        openTransform,
        closedTransform,
        `${header.selector} chevron should change when opened`,
      );

      await waitForTransformTransition(marker, header.close);

      const restoredTransform = await marker.evaluate(
        (el) => getComputedStyle(el).transform,
      );

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
