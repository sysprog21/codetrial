// Run with: node --test tests/browser/turn-row-layout.test.js
//
// The countdown ring and the line that explains it share a row under Jim's
// avatar, with the shortcut hint on the line below. Which item wraps where is
// decided by flex from sizes no stylesheet reading settles, and the ring is
// hidden until a real silence window, so no screenshot under tests/golden
// covers this: a browser with the ring showing is the only thing that sees it.

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

/// The three ways the stage is laid out. `.compact` is the class
/// `compactJimStageBesideNarrowPanel` sets beside a panel the candidate has
/// narrowed; the stylesheet gives it the same 260px the breakpoint does, so a
/// window wide enough for the default stage still reaches that width.
const STAGE_LAYOUTS = [
  { name: "the default stage", width: 1280, compact: false },
  { name: "the breakpoint's narrow stage", width: 1100, compact: false },
  { name: "a narrowed panel's stage", width: 1280, compact: true },
];

/// Reads the turn row out of a browser with the countdown showing.
async function turnRowBoxes(page, { width, compact }) {
  await page.setViewportSize({ width, height: 900 });
  await page.goto(`${base}/interview.html`, { waitUntil: "domcontentloaded" });
  // The placeholder `init()` sets, so the stage has been placed and its width
  // observer has already run: the class added below is not undone afterwards.
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-input").placeholder !== "",
  );
  return page.evaluate((useCompact) => {
    // The preflight covers the stage until a device check clears it.
    document.querySelector("#audio-check").hidden = true;
    if (useCompact) {
      document.querySelector("#jim-stage").classList.add("compact");
    }
    const ring = document.querySelector("#turn-ring");
    ring.hidden = false;
    const box = (selector) => {
      const { top, right, bottom, left, width } = document
        .querySelector(selector)
        .getBoundingClientRect();
      return { top, right, bottom, left, width };
    };
    return {
      stage: box("#jim-stage"),
      ring: box("#turn-ring"),
      status: box("#turn-status"),
      hint: box("#turn-shortcut"),
    };
  }, compact);
}

test("the countdown ring stays beside the line explaining it, at every stage width", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    for (const layout of STAGE_LAYOUTS) {
      const { stage, ring, status, hint } = await turnRowBoxes(page, layout);
      const where = `${layout.name} (${Math.round(stage.width)}px)`;

      // `.turn-row` wraps so the hint can take a line of its own, which is
      // what put the ring on a line above its own explanation once. The ring
      // carries no text, so a ring without that line beside it explains
      // nothing.
      const ringCentre = (ring.top + ring.bottom) / 2;
      assert.ok(
        ringCentre >= status.top && ringCentre <= status.bottom,
        `${where}: the ring is not on the status line's row`,
      );
      assert.ok(
        status.left >= ring.right,
        `${where}: the status line does not start right of the ring`,
      );

      // And the hint is below both rather than beside either: left at its
      // content width it fits next to the status on the wider stage and
      // squeezes it to a few characters.
      assert.ok(
        hint.top >= status.bottom && hint.top >= ring.bottom,
        `${where}: the shortcut hint shares a row instead of taking its own`,
      );
    }
  } finally {
    await page.close();
  }
});

test("a long word in the status line does not wrap the ring away from it", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await turnRowBoxes(page, STAGE_LAYOUTS[0]);
    // A flex item's `min-width: auto` floors it at its longest word, which is
    // enough to push the status line off the ring's row however small its
    // basis is. Today's copy has no word long enough to show that, so the
    // test above passes with the floor still in place.
    const { ring, status } = await page.evaluate(() => {
      document.querySelector("#turn-status").textContent =
        "Supercalifragilisticexpialidocious reply early.";
      const box = (selector) => {
        const { top, right, bottom, left } = document
          .querySelector(selector)
          .getBoundingClientRect();
        return { top, right, bottom, left };
      };
      return { ring: box("#turn-ring"), status: box("#turn-status") };
    });
    const ringCentre = (ring.top + ring.bottom) / 2;
    assert.ok(
      ringCentre >= status.top && ringCentre <= status.bottom,
      "a long word took the status line off the ring's row",
    );
    assert.ok(
      status.left >= ring.right,
      "a long word moved the status line out from beside the ring",
    );
  } finally {
    await page.close();
  }
});
