// Run with: node --test tests/browser/interview-layout.test.js
//
// Where the interview page puts its controls, and one layout rule of the
// replay page. Each case is a placement a redesign could quietly undo while
// every behavior test stays green: Start scrolled out of the setup check, the
// setup check no longer modal, End interview pushed out of a short sidebar,
// Run tests wrapped onto a second toolbar row, the dock a screen below the
// problem on a phone, the report letting focus fall back into the workspace
// behind it, the replay headline running under the column beside it.

import { after, before, test } from "node:test";
import assert from "node:assert/strict";

import {
  closeSetupCheck,
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  read,
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

async function openInterview(width, height, query = "") {
  const page = await browser.newPage({ viewport: { width, height } });
  await page.goto(
    `${base}/interview.html?problem=chargeback-pair-match${query}`,
    { waitUntil: "domcontentloaded" },
  );
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-input").placeholder !== "",
  );
  return page;
}

// Two frames: the drawer repaints from observers a frame or two after a
// change, and a wheel scrolls on a later frame.
const settled = (page) =>
  page.evaluate(
    () =>
      new Promise((done) =>
        requestAnimationFrame(() => requestAnimationFrame(done)),
      ),
  );

/// What holdWorkspaceBehindDialogs in web/interview.js makes inert.
const WORKSPACE = [
  "#interview-sidebar",
  "#sidebar-resizer",
  ".editor-panel",
  "#board-panel",
  "#jim-stage",
];

const workspaceInert = (page) =>
  page.evaluate(
    (selectors) =>
      selectors.map((selector) => document.querySelector(selector).inert),
    WORKSPACE,
  );

test("Start interview is on screen in the setup check without scrolling", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(1280, 720);
  try {
    const layout = await page.evaluate(() => {
      const start = document
        .querySelector("#audio-check-join")
        .getBoundingClientRect();
      const back = document
        .querySelector("#audio-check-leave")
        .getBoundingClientRect();
      return {
        start: { top: start.top, bottom: start.bottom },
        back: { top: back.top, bottom: back.bottom },
        overlayScrolled: document.querySelector("#audio-check").scrollTop,
        viewport: innerHeight,
      };
    });
    assert.equal(layout.overlayScrolled, 0);
    for (const [name, box] of Object.entries({
      start: layout.start,
      back: layout.back,
    })) {
      assert.ok(
        box.top >= 0 && box.bottom <= layout.viewport,
        `${name} sits at ${box.top}-${box.bottom}, outside a ${layout.viewport}px window`,
      );
    }
  } finally {
    await page.close();
  }
});

test("the setup check is a modal dialog over an inert workspace", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(1280, 800);
  try {
    const open = await page.evaluate(() => {
      const dialog = document.querySelector("#audio-check");
      return {
        role: dialog.getAttribute("role"),
        modal: dialog.getAttribute("aria-modal"),
        name: document.getElementById(dialog.getAttribute("aria-labelledby"))
          ?.textContent,
        focused: document.activeElement.id,
      };
    });
    assert.equal(open.role, "dialog");
    assert.equal(open.modal, "true");
    assert.equal(open.name, "Check your setup");
    assert.equal(open.focused, "audio-check-title");
    assert.deepEqual(await workspaceInert(page), [
      true,
      true,
      true,
      true,
      true,
    ]);

    // Tab from the title stays in the dialog: the workspace behind it offers
    // nothing to land on.
    await page.keyboard.press("Tab");
    assert.equal(
      await page.evaluate(() =>
        document.querySelector("#audio-check").contains(document.activeElement),
      ),
      true,
    );

    await closeSetupCheck(page);
    assert.deepEqual(await workspaceInert(page), [
      false,
      false,
      false,
      false,
      false,
    ]);
  } finally {
    await page.close();
  }
});

test("the dock fits in the sidebar with the Meet notes open on a short screen", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(1280, 720);
  try {
    await closeSetupCheck(page);
    await page.locator("#meet-mode-summary").click();
    await page.waitForFunction(() => document.querySelector("#meet-mode").open);
    const layout = await page.evaluate(() => {
      const box = (selector) => {
        const { top, bottom } = document
          .querySelector(selector)
          .getBoundingClientRect();
        return { top, bottom };
      };
      return {
        dock: box(".session-dock"),
        end: box("#end"),
        turn: box("#yield-turn"),
        sidebar: box("#interview-sidebar"),
      };
    });
    // The whole dock, Meet notes included: the sidebar clips what overflows
    // it, so a dock taller than its room loses its bottom (the Meet output
    // picker) with no way to scroll to it.
    for (const name of ["dock", "end", "turn"])
      assert.ok(
        layout[name].top >= layout.sidebar.top &&
          layout[name].bottom <= layout.sidebar.bottom,
        `the ${name} is cut off by the sidebar with the Meet notes open`,
      );
    // Apart from the action used most, below it rather than beside it.
    assert.ok(layout.end.top >= layout.turn.bottom);
  } finally {
    await page.close();
  }
});

test("Run tests shares one toolbar row with the languages on a laptop", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(1280, 800);
  try {
    await closeSetupCheck(page);
    const rows = await page.evaluate(() => {
      const middle = (selector) => {
        const box = document.querySelector(selector).getBoundingClientRect();
        return box.top + box.height / 2;
      };
      return {
        language: middle("#language-select"),
        run: middle("#run-tests"),
        theme: middle("#editor-theme-toggle"),
      };
    });
    assert.ok(
      Math.abs(rows.run - rows.language) < 4,
      `Run tests centres at ${rows.run}, the languages at ${rows.language}`,
    );
    assert.ok(Math.abs(rows.theme - rows.language) < 4);
  } finally {
    await page.close();
  }
});

test("on a phone the turn button is on the first screen, under the problem", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(390, 844);
  try {
    await closeSetupCheck(page);
    const layout = await page.evaluate(() => ({
      turn: document.querySelector("#yield-turn").getBoundingClientRect()
        .bottom,
      problem: document.querySelector("#problem-panel").getBoundingClientRect()
        .top,
      viewport: innerHeight,
    }));
    assert.ok(
      layout.turn <= layout.viewport,
      `Your turn is done ends at ${layout.turn}, past a ${layout.viewport}px screen`,
    );
    assert.ok(layout.turn > layout.problem);
  } finally {
    await page.close();
  }
});

// The wait for the report is not the report: Jim is still in the room and
// the recording still running. The harness has no agent, so ending goes
// straight to the report; the wait is opened here the way endInterview does.
test("the wait keeps Jim heard and the recording withdrawable", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(1440, 900);
  try {
    await closeSetupCheck(page);
    await page.evaluate(() => {
      document.querySelector("#ending-overlay").hidden = false;
    });
    const wait = await page.evaluate(() => {
      const stage = document.querySelector("#jim-stage");
      const overlay = document.querySelector("#ending-overlay");
      const stop = document.querySelector("#ending-withdraw-consent");
      return {
        stageInert: stage.inert,
        stageAbove:
          Number(getComputedStyle(stage).zIndex) >
          Number(getComputedStyle(overlay).zIndex),
        workspaceInert: document.querySelector(".editor-panel").inert,
        stopInWait: overlay.contains(stop),
        stopHidden: stop.hidden,
      };
    });
    assert.equal(wait.stageInert, false, "Jim's captions are announced");
    assert.equal(wait.stageAbove, true, "and drawn over the wait");
    assert.equal(wait.workspaceInert, true);
    assert.equal(wait.stopInWait, true, "the wait has its own Stop recording");
    assert.equal(wait.stopHidden, true, "hidden where nothing is recorded");

    // Revealed as the page reveals it once a recording starts: on screen, and
    // nothing around it holds it out of reach. What it does is the same
    // withdrawal as the sidebar's; recording-state.test.js holds both.
    await page.evaluate(() => {
      document.querySelector("#ending-withdraw-consent").hidden = false;
    });
    const stop = await page.evaluate(() => {
      const button = document.querySelector("#ending-withdraw-consent");
      return {
        visible: button.checkVisibility(),
        reachable: !button.closest("[inert]"),
      };
    });
    assert.deepEqual(stop, { visible: true, reachable: true });
  } finally {
    await page.close();
  }
});

// #579's own case: a 13.6-inch MacBook Air window with the divider at its
// maximum, where the editor panel overflows by a few pixels. The expanded
// board must not scroll with it and show what it covers.
test("the expanded Example drawing stays at the panel's top when the panel could scroll, and the scroll comes back on shrink", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(1250, 630, "&exampleDrawing=1");
  try {
    await closeSetupCheck(page);
    await page.locator("#example-board").evaluate((node) => (node.open = true));
    await page.locator(".drawer-resizer").focus();
    await page.keyboard.press("End");
    const offset = () =>
      page.evaluate(() => {
        const panel = document.querySelector(".editor-panel");
        const pane = document.querySelector(".example-pane");
        return Math.round(
          pane.getBoundingClientRect().top - panel.getBoundingClientRect().top,
        );
      });
    // Scrolled before expanding, and wheeled over the board after.
    const scrolled = await page
      .locator(".editor-panel")
      .evaluate(
        (panel) => (panel.scrollTop = panel.scrollHeight) && panel.scrollTop,
      );
    assert.ok(scrolled > 0, "the panel can scroll at this size");
    await page.getByRole("button", { name: "Expand drawing" }).click();
    assert.ok(
      Math.abs(await offset()) <= 1,
      "the pane starts at the panel's top",
    );
    const pane = await page.locator(".example-pane").boundingBox();
    await page.mouse.move(pane.x + pane.width / 2, pane.y + pane.height / 2);
    await page.mouse.wheel(0, 300);
    // The wheel scrolls on a later frame.
    await settled(page);
    assert.ok(Math.abs(await offset()) <= 1, "and stays there under the wheel");
    // Shrinking puts the panel back where the candidate had scrolled it.
    await page.getByRole("button", { name: "Shrink drawing" }).click();
    assert.equal(
      await page.locator(".editor-panel").evaluate((panel) => panel.scrollTop),
      scrolled,
      "the panel's scroll comes back on shrink",
    );
  } finally {
    await page.close();
  }
});

// The drawer sizes the docked drawing from its own layout, which an expanded
// board is not in: a resize while expanded must neither measure it from the
// floating box nor leave a wrong height for the docked board to come back to.
test("a resize while expanded leaves the docked height alone until shrinking sizes it for the new window", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(1280, 800, "&exampleDrawing=1");
  try {
    await closeSetupCheck(page);
    await page.locator("#example-board").evaluate((node) => (node.open = true));
    const surface = () =>
      page
        .locator(".example-surface")
        .evaluate((node) => node.getBoundingClientRect().height);
    const reserved = () =>
      page.locator(".example-surface").evaluate((node) => node.style.height);
    // The divider at its maximum, which the shorter window then lowers, so
    // the docked drawing must come back shorter than it left.
    await page.locator(".drawer-resizer").focus();
    await page.keyboard.press("End");
    await settled(page);
    const docked = await reserved();
    await page.getByRole("button", { name: "Expand drawing" }).click();
    await page.setViewportSize({ width: 1280, height: 600 });
    await settled(page);
    // The resize repaints the drawer, which leaves the expanded board's docked
    // height alone rather than measuring it from the box the board floats in.
    assert.equal(await reserved(), docked, "while expanded");
    await page.getByRole("button", { name: "Shrink drawing" }).click();
    await settled(page);
    const back = await surface();
    // What the drawer gives the drawing at this size when it paints afresh.
    await page
      .locator("#results-body")
      .evaluate((node) => (node.hidden = !node.hidden));
    await settled(page);
    await page
      .locator("#results-body")
      .evaluate((node) => (node.hidden = !node.hidden));
    await settled(page);
    assert.ok(Math.abs(back - (await surface())) <= 1, `${back}px`);
  } finally {
    await page.close();
  }
});

// A resize while expanded changes the docked strip's height, which the drawer
// measures again only after the board shrinks; the strip's scroll waits for
// that, or it is clamped against the old height and lands short.
test("a resize while expanded keeps the docked strip's scroll on shrink", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(1440, 900, "&exampleDrawing=1");
  try {
    await closeSetupCheck(page);
    await page.locator("#example-board").evaluate((node) => (node.open = true));
    await page.locator(".drawer-resizer").focus();
    await page.keyboard.press("End");
    await settled(page);
    const strip = page.locator(".example-surface");
    const scrolled = await strip.evaluate(
      (node) => (node.scrollTop = node.scrollHeight) && node.scrollTop,
    );
    assert.ok(scrolled > 0, "the strip can scroll at this size");
    await page.getByRole("button", { name: "Expand drawing" }).click();
    await page.setViewportSize({ width: 1280, height: 700 });
    await settled(page);
    const back = await page
      .getByRole("button", { name: "Shrink drawing" })
      .evaluate((button) => {
        button.click();
        const node = document.querySelector(".example-surface");
        return {
          top: node.scrollTop,
          max: node.scrollHeight - node.clientHeight,
        };
      });
    assert.ok(back.max >= scrolled, "the new strip can hold the old scroll");
    assert.equal(back.top, scrolled);
  } finally {
    await page.close();
  }
});

// Each of the page's own overlays over an expanded board, on the real page:
// the pane sits under them, so whichever one opens is what the candidate sees.
test("an expanded Example drawing stays under the setup check, the ending wait and the report", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(1280, 800, "&exampleDrawing=1");
  try {
    await closeSetupCheck(page);
    await page.locator("#example-board").evaluate((node) => (node.open = true));
    await page.getByRole("button", { name: "Expand drawing" }).click();
    for (const overlay of [
      "#audio-check",
      "#ending-overlay",
      "#report-modal",
    ]) {
      const covered = await page.evaluate((overlay) => {
        const node = document.querySelector(overlay);
        node.hidden = false;
        const pane = document
          .querySelector(".example-pane")
          .getBoundingClientRect();
        const hit = document.elementFromPoint(
          pane.x + pane.width / 2,
          pane.y + pane.height / 2,
        );
        node.hidden = true;
        return node.contains(hit);
      }, overlay);
      assert.ok(covered, overlay);
    }
  } finally {
    await page.close();
  }
});

// On the real page, wide and narrow: expanding makes Jim's stage the compact
// card a narrow window already has, so the captions stay on top of the board
// while the avatar and Hide avatar go; shrinking gives the wide window its full
// stage back.
test("an expanded Example drawing turns Jim's stage into the compact card over the board", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  for (const [width, height, docked] of [
    [1440, 900, false],
    [900, 800, true],
  ]) {
    const size = `${width}x${height}`;
    const page = await openInterview(width, height, "&exampleDrawing=1");
    try {
      await closeSetupCheck(page);
      await page
        .locator("#example-board")
        .evaluate((node) => (node.open = true));
      const compact = () =>
        page
          .locator("#jim-stage")
          .evaluate((node) => node.classList.contains("compact"));
      // Whether the element at the centre of `selector` is inside it.
      const onTop = (selector) =>
        page.evaluate((selector) => {
          const node = document.querySelector(selector);
          const box = node.getBoundingClientRect();
          return node.contains(
            document.elementFromPoint(
              box.x + box.width / 2,
              box.y + box.height / 2,
            ),
          );
        }, selector);
      assert.equal(await compact(), docked, size);
      // The avatar's mount has no width, which is what stops its drawing, and
      // Hide avatar goes with it; a wide window has both before expanding.
      const avatar = () =>
        page.evaluate(() => [
          document.querySelector("#jim-avatar").clientWidth > 0,
          getComputedStyle(document.querySelector("#jim-stage .hide-avatar"))
            .display !== "none",
        ]);
      assert.deepEqual(await avatar(), [!docked, !docked], size);
      assert.equal(
        await page
          .getByRole("button", { name: "Expand drawing" })
          .evaluate((button) => {
            button.click();
            return document
              .querySelector("#jim-stage")
              .classList.contains("compact");
          }),
        true,
        "the stage becomes compact before the expand handler returns",
      );
      assert.ok(await onTop("#captions-bar"), size);
      assert.deepEqual(await avatar(), [false, false], size);
      // The card's volume slider, under Audio controls, keeps Escape for itself.
      await page
        .locator("#jim-stage .jim-volume-controls")
        .evaluate((node) => (node.open = true));
      await page.locator("#jim-volume").focus();
      assert.equal(
        await page.evaluate(() => document.activeElement.id),
        "jim-volume",
        size,
      );
      await page.keyboard.press("Escape");
      assert.equal(
        await page
          .locator("#example-board")
          .evaluate((node) => node.hasAttribute("data-expanded")),
        true,
        size,
      );
      // Blocked audio puts Resume audio in the card, on top as well.
      await page.evaluate(
        () => (document.querySelector("#audio-playback-notice").hidden = false),
      );
      assert.ok(await onTop("#resume-audio"), size);
      await page.evaluate(
        () => (document.querySelector("#audio-playback-notice").hidden = true),
      );
      await page.getByRole("button", { name: "Shrink drawing" }).click();
      await page.waitForFunction(
        (docked) =>
          document.querySelector("#jim-stage").classList.contains("compact") ===
          docked,
        docked,
      );
    } finally {
      await page.close();
    }
  }
});

// Safari and Firefox on macOS leave focus where it was on a click, so it can
// still be on Hide avatar when the card hides that button; it moves to the
// stage, as when an automatic turn-off hides it.
test("expanding with focus on Hide avatar moves it to Jim's stage", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(1440, 900, "&exampleDrawing=1");
  try {
    await closeSetupCheck(page);
    await page.locator("#example-board").evaluate((node) => (node.open = true));
    await page.locator("#hide-avatar").focus();
    // A resize that leaves the window wide keeps the button, and its focus.
    await page.setViewportSize({ width: 1400, height: 900 });
    await settled(page);
    assert.equal(
      await page.evaluate(() => document.activeElement.id),
      "hide-avatar",
    );
    // A scripted click moves no focus, as those browsers' clicks do not.
    await page
      .locator("#example-board [data-action=expand]")
      .evaluate((node) => node.click());
    await page.waitForFunction(() =>
      document.querySelector("#jim-stage").classList.contains("compact"),
    );
    assert.equal(
      await page.evaluate(() => document.activeElement.id),
      "jim-stage",
    );
  } finally {
    await page.close();
  }
});

// The wait for the report over an expanded board: its own rule lifts Jim's card
// over the dialog so his closing lines are seen, and the board behind the
// dialog is inert, so Escape from the card leaves it expanded.
test("over an expanded Example drawing the ending wait lifts Jim's card and keeps Escape from the board", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(1440, 900, "&exampleDrawing=1");
  try {
    await closeSetupCheck(page);
    await page.locator("#example-board").evaluate((node) => (node.open = true));
    await page.getByRole("button", { name: "Expand drawing" }).click();
    await page
      .locator("#ending-overlay")
      .evaluate((node) => (node.hidden = false));
    // The page makes the workspace inert on the observer's microtask.
    await page.waitForFunction(
      () => document.querySelector(".editor-panel").inert,
    );
    const captions = await page.evaluate(() => {
      const bar = document.querySelector("#captions-bar");
      const box = bar.getBoundingClientRect();
      return bar.contains(
        document.elementFromPoint(
          box.x + box.width / 2,
          box.y + box.height / 2,
        ),
      );
    });
    assert.ok(captions, "the card's captions are over the wait");
    await page.locator("#jim-stage").focus();
    await page.keyboard.press("Escape");
    assert.equal(
      await page
        .locator("#example-board")
        .evaluate((node) => node.hasAttribute("data-expanded")),
      true,
    );
  } finally {
    await page.close();
  }
});

// The whiteboard page has no Example drawing to expand, so only the window and
// its panel decide: a wide one keeps Jim's full stage, resize after resize.
test("the whiteboard page keeps Jim's full stage in a wide window", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage({
    viewport: { width: 1440, height: 900 },
  });
  try {
    await page.goto(
      `${base}/interview.html?problem=chargeback-pair-match&mode=whiteboard`,
      { waitUntil: "domcontentloaded" },
    );
    await page.waitForFunction(
      () => document.querySelector(".editor-panel") === null,
    );
    const compact = async () => {
      await settled(page);
      return page.evaluate(() =>
        document.querySelector("#jim-stage").classList.contains("compact"),
      );
    };
    assert.equal(await compact(), false);
    for (const width of [1400, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      assert.equal(await compact(), false, `${width}`);
    }
  } finally {
    await page.close();
  }
});

// On a phone the compact stage runs along the bottom edge above the wait, so a
// long closing line must not grow it over the wait's own buttons.
test("on a phone Jim's closing lines leave the wait's buttons reachable", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(390, 600);
  try {
    await closeSetupCheck(page);
    await page.evaluate(() => {
      document.querySelector("#ending-overlay").hidden = false;
      document.querySelector("#ending-withdraw-consent").hidden = false;
      document.querySelector("#captions-text").textContent =
        "Thanks for walking me through that. ".repeat(30);
    });
    const covered = await page.evaluate(() =>
      [...document.querySelectorAll("#ending-overlay button")]
        .filter((button) => button.offsetParent)
        .filter((button) => {
          const box = button.getBoundingClientRect();
          const top = document.elementFromPoint(
            box.x + box.width / 2,
            box.y + box.height / 2,
          );
          return !button.contains(top);
        })
        .map((button) => button.id),
    );
    assert.equal(
      await page
        .locator("#jim-stage")
        .evaluate((s) => s.classList.contains("compact")),
      true,
    );
    assert.deepEqual(covered, []);
  } finally {
    await page.close();
  }
});

// The camera stays live through the wait, so a recording candidate's self view
// would add its height to the compact card, which lets taps through but is
// drawn over the wait's buttons.
test("on a phone the self view does not grow the card over the wait", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(390, 480);
  try {
    await closeSetupCheck(page);
    const selfView = page.locator("#self-view");
    await selfView.evaluate((video) => (video.hidden = false));
    assert.equal(await selfView.isVisible(), true);
    await page.evaluate(() => {
      document.querySelector("#ending-overlay").hidden = false;
      document.querySelector("#ending-withdraw-consent").hidden = false;
      document.querySelector("#captions-text").textContent =
        "Thanks for walking me through that. ".repeat(30);
    });
    await page.waitForFunction(
      () =>
        parseFloat(
          getComputedStyle(document.querySelector("#ending-overlay"))
            .paddingBottom,
        ) >=
        document.querySelector("#jim-stage").getBoundingClientRect().height +
          47,
    );
    const under = await page.evaluate(() => {
      const card = document.querySelector("#jim-stage").getBoundingClientRect();
      return [...document.querySelectorAll("#ending-overlay button")]
        .filter((button) => button.offsetParent)
        .filter((button) => {
          const box = button.getBoundingClientRect();
          return box.bottom > card.top && box.top < card.bottom;
        })
        .map((button) => button.id);
    });
    assert.deepEqual(under, []);
  } finally {
    await page.close();
  }
});

test("a narrow window and a narrow panel both make the stage compact", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(1440, 900);
  try {
    await closeSetupCheck(page);
    const compact = () =>
      page.evaluate(() =>
        document.querySelector("#jim-stage").classList.contains("compact"),
      );
    assert.equal(await compact(), false, "a wide window keeps the avatar");
    // Under 1024px the sidebar stacks above the editor, so the panel is wide
    // again and only the window says compact.
    await page.setViewportSize({ width: 1000, height: 900 });
    const panelMinRem = Number(
      read("web/interview.js").match(/JIM_AVATAR_PANEL_MIN_REM = (\d+);/)[1],
    );
    assert.ok(
      await page.evaluate(
        (minRem) =>
          document.querySelector(".editor-panel").getBoundingClientRect()
            .width >
          minRem *
            Number.parseFloat(
              getComputedStyle(document.documentElement).fontSize,
            ),
        panelMinRem,
      ),
      "the panel alone would not make it compact here",
    );
    await page.waitForFunction(() =>
      document.querySelector("#jim-stage").classList.contains("compact"),
    );
    const card = await page.evaluate(() => ({
      avatar: getComputedStyle(document.querySelector(".jim-avatar")).display,
      background: getComputedStyle(document.querySelector("#jim-stage"))
        .backgroundColor,
    }));
    assert.equal(card.avatar, "none", "the avatar goes, the captions stay");
    assert.notEqual(card.background, "rgba(0, 0, 0, 0)", "as one card");
  } finally {
    await page.close();
  }
});

test("the report is a modal dialog with one primary action", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openInterview(1280, 800);
  page.on("dialog", (dialog) => dialog.accept());
  try {
    await closeSetupCheck(page);
    await page.locator("#end").click();
    // With no interviewer in the room, ending shows the offline report by
    // itself 300ms later and hides the ending overlay. Clicking "Show offline
    // summary" raced that, and a click that landed late waited out its timeout
    // on a button that was gone.
    await page.locator("#report-modal #done").waitFor();
    const state = await page.evaluate(() => {
      const report = document.querySelector("#report-modal");
      const fill = (selector) =>
        getComputedStyle(report.querySelector(selector)).backgroundColor;
      return {
        role: report.getAttribute("role"),
        modal: report.getAttribute("aria-modal"),
        focused: document.activeElement === report,
        workspaceInert: document.querySelector(".editor-panel").inert,
        stageInert: document.querySelector("#jim-stage").inert,
        download: fill("#download-report"),
        done: fill("#done"),
      };
    });
    assert.equal(state.role, "dialog");
    assert.equal(state.modal, "true");
    assert.equal(state.focused, true);
    assert.equal(state.workspaceInert, true);
    assert.equal(
      state.stageInert,
      true,
      "Hide avatar is reachable behind the report",
    );
    // Done is filled; the download is outlined, not a second equal button.
    assert.equal(state.download, "rgba(0, 0, 0, 0)");
    assert.notEqual(state.done, "rgba(0, 0, 0, 0)");
  } finally {
    await page.close();
  }
});

// The replay page, from the same server: its headline sits in a 300px list
// column, and replay-render.test.js reads it through a stub document that
// applies no stylesheet, so only a real browser sees it run under the column
// beside it.
test("the replay headline stays inside the list column", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage({
    viewport: { width: 1440, height: 900 },
  });
  try {
    await page.goto(`${base}/replay.html`, { waitUntil: "domcontentloaded" });
    const layout = await page.evaluate(() => {
      const heading = document.querySelector(".replay-sidebar h1");
      // The text's own extent, not the block's: a block is as wide as its
      // column whatever the words inside it do.
      const range = document.createRange();
      range.selectNodeContents(heading);
      return {
        text: range.getBoundingClientRect().right,
        column: document
          .querySelector(".replay-sidebar")
          .getBoundingClientRect().right,
        detail: document.querySelector(".replay-detail").getBoundingClientRect()
          .left,
      };
    });
    assert.ok(
      layout.text <= layout.column && layout.text <= layout.detail,
      `the headline ends at ${layout.text}, past its column at ${layout.column}`,
    );
  } finally {
    await page.close();
  }
});

// The list marks the open recording the way the timeline marks its moment.
// `select` writes `aria-current`, so the stylesheet has to key on that and not
// on a class nothing sets.
test("the replay list marks the open recording", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage({
    viewport: { width: 1440, height: 900 },
  });
  await page.route(/\/api\/recordings$/, (route) =>
    route.fulfill({
      json: {
        recordings: [
          { recordingId: "first", createdAt: 1700000000, state: "ready" },
          { recordingId: "second", createdAt: 1700000600, state: "ready" },
        ],
      },
    }),
  );
  await page.route(/\/api\/recordings\/.+/, (route) =>
    route.fulfill({ status: 404, body: "" }),
  );
  try {
    await page.goto(`${base}/replay.html`, { waitUntil: "domcontentloaded" });
    await page.locator('.replay-item[data-recording="second"]').click();
    await page.waitForFunction(
      () =>
        document
          .querySelector('.replay-item[data-recording="second"]')
          .getAttribute("aria-current") === "true",
    );
    const borders = await page.evaluate(() => {
      const accent = document.createElement("div");
      accent.style.color = "var(--accent)";
      document.body.append(accent);
      const border = (id) =>
        getComputedStyle(
          document.querySelector(`.replay-item[data-recording="${id}"]`),
        ).borderTopColor;
      return {
        accent: getComputedStyle(accent).color,
        chosen: border("second"),
        other: border("first"),
      };
    });
    assert.equal(borders.chosen, borders.accent);
    assert.notEqual(borders.other, borders.accent);
  } finally {
    await page.close();
  }
});

test("local recording controls preserve the original problem reading space", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  for (const [width, height, fontFamily] of [
    [1280, 720, null],
    [1280, 720, "Verdana, sans-serif"],
    [390, 844, null],
    [390, 844, "Verdana, sans-serif"],
  ]) {
    const page = await openInterview(width, height);
    try {
      await closeSetupCheck(page);
      await page.evaluate((fontFamily) => {
        document.body.style.fontFamily = fontFamily || "";
      }, fontFamily);
      const sizes = await page.evaluate(() => {
        const panel = document.querySelector("#local-recording");
        const height = () =>
          document.querySelector("#problem-panel").getBoundingClientRect()
            .height;
        const idle = height();
        panel.hidden = true;
        const original = height();
        panel.hidden = false;
        panel.querySelector("[data-local-start]").hidden = true;
        const stop = panel.querySelector("[data-local-stop]");
        stop.hidden = false;
        stop.textContent = "End recording 00:05";
        return { original, idle, recording: height() };
      });
      assert.ok(
        sizes.original > 0,
        JSON.stringify({ width, fontFamily, ...sizes }),
      );
      assert.equal(
        sizes.idle,
        sizes.original,
        JSON.stringify({ width, fontFamily, ...sizes }),
      );
      assert.equal(
        sizes.recording,
        sizes.original,
        JSON.stringify({ width, fontFamily, ...sizes }),
      );
    } finally {
      await page.close();
    }
  }
});
