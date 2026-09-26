import { after, before, test } from "node:test";
import assert from "node:assert/strict";

import { DEFAULT_RUNTIME_CONFIG, launchChromium, startStaticServer } from "./source.js";
import { COMPILED_LANGUAGES } from "../../web/compiler-explorer.js";

let browser = null;
let server = null;
let base = "";

before(async () => {
  browser = await launchChromium();
  if (!browser) return;
  ({ server, base } = await startStaticServer({ runtimeConfig: DEFAULT_RUNTIME_CONFIG }));
});

after(async () => {
  await browser?.close();
  await new Promise((closed) => (server ? server.close(closed) : closed()));
});

/// The interview page binds its handlers and fetches the judge after the
/// document arrives, and `addCandidateCase` awaits that judge before it accepts
/// anything. A click sent before either is a click nothing is listening for, so
/// the case never lands and the wait for it times out on a cold runner while
/// passing on a warm laptop. The placeholder is written from the judge's own
/// first case, so it appearing means both halves are ready.
async function candidateCasesReady(page) {
  await page.waitForFunction(
    () => (document.querySelector("#candidate-case-input")?.placeholder ?? "") !== "",
  );
}

async function runCandidate(language, code) {
  const page = await browser.newPage();
  try {
    await page.goto(base);
    return await page.evaluate(async ({ language, code }) => {
      const { runBrowserTests } = await import("/runners.js");
      return runBrowserTests("chargeback-pair-match", code, language, null, [{ input: [[2, 7, 11, 15], 9] }]);
    }, { language, code });
  } finally {
    await page.close();
  }
}

test("candidate case output is shown by the JavaScript runner", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const summary = await runCandidate("javascript", "function matchDisputedCharge(nums, target) { return [0, 1]; }");
  assert.equal(summary.total, summary.cases.length - 1);
  assert.equal(summary.cases.at(-1).candidate, true);
  assert.equal(summary.cases.at(-1).pass, null);
  assert.equal(summary.cases.at(-1).got, "[0,1]");
});

test("candidate case output is shown by the Python runner", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const summary = await runCandidate("python", "def matchDisputedCharge(nums, target):\n    return [0, 1]\n");
  assert.equal(summary.total, summary.cases.length - 1);
  assert.equal(summary.cases.at(-1).candidate, true);
  assert.equal(summary.cases.at(-1).pass, null);
  assert.equal(summary.cases.at(-1).got, "[0,1]");
});

test("candidate cases run when session storage cannot be written", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.addInitScript(() => {
      const setItem = Storage.prototype.setItem;
      Storage.prototype.setItem = function store(key, value) {
        if (key.startsWith("codetrial.candidateCases.")) throw new DOMException("quota", "QuotaExceededError");
        return setItem.call(this, key, value);
      };
    });
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, { waitUntil: "domcontentloaded" });
    await candidateCasesReady(page);
    await page.evaluate(() => {
      document.querySelector("#candidate-case-input").value = "[[2,7,11,15],9]";
      document.querySelector("#candidate-case-add").click();
    });
    await page.waitForFunction(() => document.querySelector("#candidate-case-list").children.length === 1);
    assert.equal(await page.locator("#candidate-case-input").inputValue(), "");

    await page.evaluate(() => {
      document.querySelector('[data-language="javascript"]').click();
      document.querySelector("#run-tests").click();
    });
    await page.waitForFunction(() => document.querySelector("#results-body").textContent.includes("Your case 1"));
  } finally {
    await page.close();
  }
});

// Blocked site data fails a step earlier than a refused write: reading the
// `sessionStorage` global itself throws. Resolving it at the call site put
// that throw after the case was pushed, so the run stopped over a valid case
// and the next attempt added it a second time.
test("candidate cases run when session storage cannot be reached", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.addInitScript(() => {
      Object.defineProperty(window, "sessionStorage", {
        get() {
          throw new DOMException("blocked", "SecurityError");
        },
      });
    });
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, { waitUntil: "domcontentloaded" });
    await candidateCasesReady(page);
    await page.evaluate(() => {
      document.querySelector("#candidate-case-input").value = "[[2,7,11,15],9]";
      document.querySelector('[data-language="javascript"]').click();
      document.querySelector("#run-tests").click();
    });
    await page.waitForFunction(() => document.querySelector("#results-body").textContent.includes("Your case 1"));
    assert.equal(await page.locator("#candidate-case-list").evaluate((list) => list.children.length), 1);
  } finally {
    await page.close();
  }
});

// The list length alone proves nothing here: the handler clears the input on
// success, so the later clicks would fail to parse an empty string and leave
// one case behind whether or not the button was disabled. What the guard
// decides is the status line, which those refusals would otherwise overwrite
// with a parse error.
test("rapid add clicks add one case and keep its status", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, { waitUntil: "domcontentloaded" });
    await candidateCasesReady(page);
    await page.evaluate(() => {
      const input = document.querySelector("#candidate-case-input");
      const add = document.querySelector("#candidate-case-add");
      for (let index = 0; index < 6; index++) {
        input.value = `[[${index},7,11,15],${index + 7}]`;
        add.click();
      }
    });
    await page.waitForFunction(() => document.querySelector("#candidate-case-list").children.length === 1);
    assert.equal(await page.locator("#candidate-case-list").locator("li").count(), 1);
    assert.equal(await page.locator("#candidate-case-status").textContent(), "1/5 cases ready.");
    // The addition reads the box when the judge answers, not when the button
    // was pressed, which is the same contract `runTests` relies on to pick up
    // whatever the candidate has typed. So the case that lands is the last
    // value written, not the first.
    assert.equal(
      await page.locator("#candidate-case-list").locator("li").evaluate((item) => item.querySelector("span").textContent.trim()),
      "Your case 1: [[5,7,11,15],12]",
    );
  } finally {
    await page.close();
  }
});

test("adding then running during judge load shares one addition", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    let releaseJudge;
    const judgeHeld = new Promise((resolve) => { releaseJudge = resolve; });
    await page.route("**/judges/chargeback-pair-match.json", async (route) => {
      await judgeHeld;
      await route.continue();
    });
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, { waitUntil: "domcontentloaded" });
    // `candidateCasesReady` is no use here: it waits on the judge, which this
    // test is holding. Retry the click instead and let the guard report its
    // own readiness, because `addCandidateCase` disables the button before its
    // first await, so a click that took is proof `bindEvents` has run. That
    // function is synchronous, so the language tabs and the run button are
    // bound by then too.
    await page.waitForFunction(() => {
      const add = document.querySelector("#candidate-case-add");
      if (!add || add.disabled) return Boolean(add?.disabled);
      document.querySelector("#candidate-case-input").value = "[[2,7,11,15],9]";
      add.click();
      return add.disabled;
    });
    await page.evaluate(() => {
      document.querySelector('[data-language="javascript"]').click();
      document.querySelector("#run-tests").click();
    });
    releaseJudge();
    await page.waitForFunction(() => document.querySelector("#results-body").textContent.includes("Your case 1"));
    assert.equal(await page.locator("#candidate-case-status").textContent(), "1/5 cases ready.");
    assert.equal(await page.locator("#candidate-case-list").locator("li").count(), 1);
  } finally {
    await page.close();
  }
});

test("code that never runs is reported, not swallowed", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(base);
    // The candidate's code is the worker's own first statements now rather
    // than a string evaluated inside it, which is what lets the page withhold
    // `unsafe-eval`. It also moves where a failure to run surfaces: a syntax
    // error stops the worker script instead of throwing out of an eval, so it
    // arrives through `onerror`. Both still have to reach the candidate as the
    // reason their run produced nothing.
    const results = await page.evaluate(async () => {
      const { runBrowserTests } = await import("/runners.js");
      const run = (code) => runBrowserTests("chargeback-pair-match", code, "javascript");
      const [broken, throwing, missing] = await Promise.all([
        run("function matchDisputedCharge( {{{"),
        run("throw new Error('boom at top level');"),
        run("const notTheEntry = 1;"),
      ]);
      return [broken.setupError, throwing.setupError, missing.setupError];
    });
    assert.match(results[0], /SyntaxError/);
    assert.match(results[1], /boom at top level/);
    assert.match(results[2], /Could not find matchDisputedCharge/);
  } finally {
    await page.close();
  }
});

test("a malformed candidate case stops the run it was typed into", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, { waitUntil: "domcontentloaded" });
    await candidateCasesReady(page);
    // The opposite of the cap below. A case the cap turned away still lets the
    // run go ahead, and a case that cannot be parsed must not: running without
    // it answers a question the candidate did not ask, and the box still holds
    // what they typed. The reason is asserted to be the one the add path
    // wrote, because `runTests` used to recover it by reading the status line
    // back out, so rewording that line changed which runs were blocked.
    await page.evaluate(() => {
      document.querySelector("#candidate-case-input").value = "not json";
      document.querySelector('[data-language="javascript"]').click();
      document.querySelector("#run-tests").click();
    });
    await page.waitForFunction(() => {
      const results = document.querySelector("#results-body").textContent;
      return results.length > 0 && results === document.querySelector("#candidate-case-status").textContent;
    });
    assert.equal(await page.locator("#candidate-case-list").locator("li").count(), 0);
    assert.doesNotMatch(await page.locator("#results-body").textContent(), /test cases passed/);
    assert.equal(await page.locator("#run-tests").textContent(), "Run tests");
  } finally {
    await page.close();
  }
});

test("a sixth candidate case is refused without blocking the run", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, { waitUntil: "domcontentloaded" });
    await candidateCasesReady(page);
    // Fill the allowance, then type one more and run. The sixth is refused on
    // its own; the judge's cases and the five already accepted still run.
    //
    // Each add waits for the row it creates rather than for a fixed delay:
    // `addCandidateCase` awaits the judge before it accepts anything, so a
    // sleep long enough on a warm laptop is short enough on a cold runner to
    // let the next value overwrite the box mid-add and lose the case.
    for (let index = 0; index < 5; index++) {
      await page.evaluate((value) => {
        document.querySelector("#candidate-case-input").value = value;
        document.querySelector("#candidate-case-add").click();
      }, `[[${index},7,11,15],${index + 7}]`);
      await page.waitForFunction(
        (expected) => document.querySelector("#candidate-case-list").children.length === expected,
        index + 1,
      );
    }

    await page.evaluate(() => {
      document.querySelector("#candidate-case-input").value = "[[9,9,9,9],18]";
      document.querySelector('[data-language="javascript"]').click();
      document.querySelector("#run-tests").click();
    });
    await page.waitForFunction(() => document.querySelector("#results-body").textContent.includes("Your case 5"));
    assert.match(await page.locator("#candidate-case-status").textContent(), /up to 5 cases/);
    assert.equal(await page.locator("#candidate-case-list").locator("li").count(), 5);
  } finally {
    await page.close();
  }
});


test("removing a saved candidate case frees its slot and persists", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, { waitUntil: "domcontentloaded" });
    await candidateCasesReady(page);
    for (let index = 0; index < 5; index++) {
      await page.evaluate((index) => {
        document.querySelector("#candidate-case-input").value = JSON.stringify([[index, 7], index + 7]);
        document.querySelector("#candidate-case-add").click();
      }, index);
      await page.waitForFunction((count) => document.querySelector("#candidate-case-list").children.length === count, index + 1);
    }
    await page.evaluate(() => { document.querySelector("#candidate-case").open = true; });
    assert.equal(await page.locator("[data-remove-case]").count(), 5);
    await page.evaluate(() => document.querySelector('[data-remove-case="2"]').click());
    assert.equal(await page.locator("#candidate-case-status").textContent(), "4/5 cases ready.");
    assert.equal(await page.evaluate(() => document.activeElement.getAttribute("aria-label")), "Remove case 3");
    // Reload before the replacement is added: the add writes the in-memory
    // list to storage, so it would cover for a removal whose own write never
    // happened. `candidateCasesReady` cannot pace this one - the placeholder
    // is written only when storage came back empty - so the first rendered
    // row is the ready signal and the count is asserted, not awaited.
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => document.querySelector("#candidate-case-list").children.length > 0);
    assert.equal(await page.locator("#candidate-case-list").locator("li").count(), 4);
    assert.doesNotMatch(await page.locator("#candidate-case-list").textContent(), /\[\[2,7\],9\]/);
    await page.evaluate(() => {
      document.querySelector("#candidate-case-input").value = "[[10,7],17]";
      document.querySelector("#candidate-case-add").click();
    });
    await page.waitForFunction(() => document.querySelector("#candidate-case-list").children.length === 5);
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => document.querySelector("#candidate-case-list").children.length > 0);
    assert.equal(await page.locator("#candidate-case-list").locator("li").count(), 5);
    assert.match(await page.locator("#candidate-case-list").textContent(), /\[\[10,7\],17\]/);
    // Removal ignores a second click that lands before the next animation
    // frame, so the tight in-page loop this used to be would take one case
    // and then spin. Five clicks from here instead, one frame apart - the
    // row is gone before the click returns, so only a frame can pace the
    // guard - and the empty list is asserted after them, so a removal that
    // stops removing fails by name here instead of spinning or timing out.
    await page.evaluate(() => { document.querySelector("#candidate-case").open = true; });
    for (let clicks = 0; clicks < 5; clicks++) {
      await page.evaluate(() => document.querySelector("[data-remove-case]").click());
      await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(resolve)));
    }
    assert.equal(await page.locator("#candidate-case-list").locator("li").count(), 0);
    assert.equal(await page.locator("#candidate-case-status").textContent(), "0/5 cases ready.");
    assert.equal(await page.evaluate(() => document.activeElement.id), "candidate-case-add");
  } finally {
    await page.close();
  }
});

// Seeding goes through addInitScript because initializeCandidateCases reads
// sessionStorage once, during module evaluation: a value written after goto is
// a value the page never saw. The judge is then held so the run below parks
// inside addCandidateCase with runningTests already set -- the window a removal
// would race, since the parked run still holds the pre-removal array and its
// completion repaints and publishes it. No sleeps: the hold is the window.
test("a case cannot be removed while tests are running", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.addInitScript(() => {
      sessionStorage.setItem(
        "codetrial.candidateCases.chargeback-pair-match",
        JSON.stringify([{ input: [[0, 7], 7] }, { input: [[1, 7], 8] }]),
      );
    });
    let releaseJudge;
    const judgeHeld = new Promise((resolve) => { releaseJudge = resolve; });
    await page.route("**/judges/chargeback-pair-match.json", async (route) => {
      await judgeHeld;
      await route.continue();
    });
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, { waitUntil: "domcontentloaded" });
    // candidateCasesReady waits on the judge this test is holding, so the
    // rendered rows are the ready signal instead: bindEvents registers the
    // delegated remove listener before it calls initializeCandidateCases, so
    // the seeded rows being on screen means the listener already is too.
    await page.waitForFunction(() => document.querySelector("#candidate-case-list").children.length === 2);
    await page.evaluate(() => {
      document.querySelector("#candidate-case").open = true;
      document.querySelector("#candidate-case-input").value = "[[2,7],9]";
      document.querySelector('[data-language="javascript"]').click();
      document.querySelector("#run-tests").click();
    });
    // A removal re-renders the list, so the rows still being there alongside
    // the refusal proves the guard answered the click, not that the click
    // missed its listener.
    await page.evaluate(() => document.querySelector("[data-remove-case]").click());
    assert.equal(await page.locator("#candidate-case-list").locator("li").count(), 2);
    assert.equal(
      await page.locator("#candidate-case-status").textContent(),
      "Wait for the test run to finish before removing a case.",
    );
    // loadJudge caches per id, so this one release answers the module's judge
    // promise, the parked add and the run. The typed case lands on the answer,
    // per the same contract the add button has, and the list ends at three.
    releaseJudge();
    await page.waitForFunction(() => document.querySelector("#results-body").textContent.includes("Your case 1"));
    assert.equal(await page.locator("#candidate-case-list").locator("li").count(), 3);
    // The guard covers the run window only: with the run done, the same click
    // removes. Each awaited step above cost frames, so the double-click guard
    // has long re-armed.
    await page.evaluate(() => document.querySelector("[data-remove-case]").click());
    assert.equal(await page.locator("#candidate-case-list").locator("li").count(), 2);
    assert.equal(await page.locator("#candidate-case-status").textContent(), "2/5 cases ready.");
  } finally {
    await page.close();
  }
});

/// Holds every judge request until the returned function is called, so a test
/// can look at the page before the judge has loaded.
async function holdJudge(page) {
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  await page.route("**/judges/*.json", async (route) => {
    await held;
    await route.continue();
  });
  return release;
}

/// Clicks a tab even when it is disabled, and answers whether it took.
async function clickSelects(tab) {
  await tab.evaluate((button) => button.click());
  return tab.evaluate((button) => button.classList.contains("selected"));
}

test("disabled compiled runners cannot be selected while the judge loads", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  let releaseJudge = () => {};
  try {
    releaseJudge = await holdJudge(page);
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, { waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => document.querySelector('[data-language="c"]').disabled);
    for (const language of COMPILED_LANGUAGES) {
      const tab = page.locator(`[data-language="${language}"]`);
      assert.equal(await tab.isDisabled(), true);
      assert.match(await tab.getAttribute("title"), /disabled by this server/);
      assert.equal(await clickSelects(tab), false);
    }
    releaseJudge();
    await candidateCasesReady(page);
    for (const language of ["python", "javascript"]) {
      const tab = page.locator(`[data-language="${language}"]`);
      assert.equal(await tab.isDisabled(), false);
      assert.equal(await clickSelects(tab), true);
    }
    for (const language of COMPILED_LANGUAGES) {
      assert.equal(await page.locator(`[data-language="${language}"]`).isDisabled(), true);
    }
  } finally {
    releaseJudge();
    await page.close();
  }
});

test("C stays unavailable until a function-style judge has loaded", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const runtimeConfig = DEFAULT_RUNTIME_CONFIG
    .replace("ENABLED = false", "ENABLED = true")
    .replace('BASE_URL = ""', 'BASE_URL = "https://godbolt.org"');
  const enabled = await startStaticServer({ runtimeConfig });
  try {
    for (const [problem, classStyle] of [["edge-thumbnail-store", true], ["chargeback-pair-match", false]]) {
      const page = await browser.newPage();
      let releaseJudge = () => {};
      try {
        releaseJudge = await holdJudge(page);
        await page.goto(`${enabled.base}/interview.html?problem=${problem}`, { waitUntil: "domcontentloaded" });
        const c = page.locator('[data-language="c"]');
        await page.waitForFunction(() => document.querySelector('[data-language="c"]').disabled);
        assert.match(await c.getAttribute("title"), /only after function-style tests load/);
        assert.equal(await clickSelects(c), false);
        releaseJudge();
        await candidateCasesReady(page);
        assert.equal(await c.isDisabled(), classStyle);
        if (classStyle) assert.match(await c.getAttribute("title"), /only builds function judges/);
        else assert.equal(await clickSelects(c), true);
        for (const language of ["python", "javascript", "cpp", "java"]) {
          assert.equal(await page.locator(`[data-language="${language}"]`).isDisabled(), false);
        }
      } finally {
        releaseJudge();
        await page.close();
      }
    }
  } finally {
    await new Promise((closed) => enabled.server.close(closed));
  }
});
