import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { parseCandidateCase } from "../../web/runners.js";
import { writeStored } from "../../web/audio-output.js";

import {
  closeSetupCheck,
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  loadInterview,
  startStaticServer,
} from "./source.js";
import { COMPILED_LANGUAGES } from "../../web/compiler-explorer.js";

let browser = null;
let server = null;
let base = "";
const CE_RUNTIME_CONFIG = DEFAULT_RUNTIME_CONFIG.replace(
  "ENABLED = false",
  "ENABLED = true",
).replace('BASE_URL = ""', 'BASE_URL = "https://godbolt.org"');

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

/// The interview page binds its handlers and fetches the judge after the
/// document arrives, and `addCandidateCase` awaits that judge before it accepts
/// anything. A click sent before either is a click nothing is listening for, so
/// the case never lands and the wait for it times out on a cold runner while
/// passing on a warm laptop. The placeholder is written from the judge's own
/// first case, so it appearing means both halves are ready.
async function candidateCasesReady(page) {
  await page.waitForFunction(
    () =>
      (document.querySelector("#candidate-case-input")?.placeholder ?? "") !==
      "",
  );
}

async function runCandidate(language, code) {
  const page = await browser.newPage();
  try {
    await page.goto(base);
    return await page.evaluate(
      async ({ language, code }) => {
        const { runBrowserTests } = await import("/runners.js");
        return runBrowserTests("chargeback-pair-match", code, language, null, [
          { input: [[2, 7, 11, 15], 9] },
        ]);
      },
      { language, code },
    );
  } finally {
    await page.close();
  }
}

test("candidate case output is shown by the JavaScript runner", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const summary = await runCandidate(
    "javascript",
    "function matchDisputedCharge(nums, target) { return [0, 1]; }",
  );
  assert.equal(summary.total, summary.cases.length - 1);
  assert.equal(summary.cases.at(-1).candidate, true);
  assert.equal(summary.cases.at(-1).pass, null);
  assert.equal(summary.cases.at(-1).got, "[0,1]");
});

test("candidate case output is shown by the Python runner", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const summary = await runCandidate(
    "python",
    "def matchDisputedCharge(nums, target):\n    return [0, 1]\n",
  );
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
        if (key.startsWith("codetrial.candidateCases."))
          throw new DOMException("quota", "QuotaExceededError");
        return setItem.call(this, key, value);
      };
    });
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    await page.evaluate(() => {
      document.querySelector("#candidate-case-input").value = "[[2,7,11,15],9]";
      document.querySelector("#candidate-case-add").click();
    });
    await page.waitForFunction(
      () =>
        document.querySelector("#candidate-case-list").children.length === 1,
    );
    assert.equal(await page.locator("#candidate-case-input").inputValue(), "");

    await page.locator("#language-select").selectOption("javascript");
    await page.evaluate(() => {
      document.querySelector("#run-tests").click();
    });
    await page.waitForFunction(() =>
      document
        .querySelector("#results-body")
        .textContent.includes("Your case 1"),
    );
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
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    await page.locator("#language-select").selectOption("javascript");
    await page.evaluate(() => {
      document.querySelector("#candidate-case-input").value = "[[2,7,11,15],9]";
      document.querySelector("#run-tests").click();
    });
    await page.waitForFunction(() =>
      document
        .querySelector("#results-body")
        .textContent.includes("Your case 1"),
    );
    assert.equal(
      await page
        .locator("#candidate-case-list")
        .evaluate((list) => list.children.length),
      1,
    );
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
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    await page.evaluate(() => {
      const input = document.querySelector("#candidate-case-input");
      const add = document.querySelector("#candidate-case-add");
      for (let index = 0; index < 6; index++) {
        input.value = `[[${index},7,11,15],${index + 7}]`;
        add.click();
      }
    });
    await page.waitForFunction(
      () =>
        document.querySelector("#candidate-case-list").children.length === 1,
    );
    assert.equal(
      await page.locator("#candidate-case-list").locator("li").count(),
      1,
    );
    assert.equal(
      await page.locator("#candidate-case-status").textContent(),
      "1/5 cases ready.",
    );
    // The addition reads the box when the judge answers, not when the button
    // was pressed, which is the same contract `runTests` relies on to pick up
    // whatever the candidate has typed. So the case that lands is the last
    // value written, not the first.
    assert.equal(
      await page
        .locator("#candidate-case-list")
        .locator("li")
        .evaluate((item) => item.querySelector("span").textContent.trim()),
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
    const judgeHeld = new Promise((resolve) => {
      releaseJudge = resolve;
    });
    await page.route("**/judges/chargeback-pair-match.json", async (route) => {
      await judgeHeld;
      await route.continue();
    });
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    // `candidateCasesReady` is no use here: it waits on the judge, which this
    // test is holding. Retry the click instead and let the guard report its
    // own readiness, because `addCandidateCase` disables the button before its
    // first await, so a click that took is proof `bindEvents` has run. That
    // function is synchronous, so the language selector and the run button are
    // bound by then too.
    await page.waitForFunction(() => {
      const add = document.querySelector("#candidate-case-add");
      if (!add || add.disabled) return Boolean(add?.disabled);
      document.querySelector("#candidate-case-input").value = "[[2,7,11,15],9]";
      add.click();
      return add.disabled;
    });
    await page.locator("#language-select").selectOption("javascript");
    await page.evaluate(() => {
      document.querySelector("#run-tests").click();
    });
    releaseJudge();
    await page.waitForFunction(() =>
      document
        .querySelector("#results-body")
        .textContent.includes("Your case 1"),
    );
    assert.equal(
      await page.locator("#candidate-case-status").textContent(),
      "1/5 cases ready.",
    );
    assert.equal(
      await page.locator("#candidate-case-list").locator("li").count(),
      1,
    );
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
      const run = (code) =>
        runBrowserTests("chargeback-pair-match", code, "javascript");
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
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    // The opposite of the cap below. A case the cap turned away still lets the
    // run go ahead, and a case that cannot be parsed must not: running without
    // it answers a question the candidate did not ask, and the box still holds
    // what they typed. The reason is asserted to be the one the add path
    // wrote, because `runTests` used to recover it by reading the status line
    // back out, so rewording that line changed which runs were blocked.
    await page.locator("#language-select").selectOption("javascript");
    await page.evaluate(() => {
      document.querySelector("#candidate-case-input").value = "not json";
      document.querySelector("#run-tests").click();
    });
    await page.waitForFunction(() => {
      const results = document.querySelector("#results-content").textContent;
      return (
        results.length > 0 &&
        results === document.querySelector("#candidate-case-status").textContent
      );
    });
    assert.equal(
      await page.locator("#candidate-case-list").locator("li").count(),
      0,
    );
    assert.doesNotMatch(
      await page.locator("#results-body").textContent(),
      /test cases passed/,
    );
    assert.equal(await page.locator("#run-tests").textContent(), "Run tests");
  } finally {
    await page.close();
  }
});

test("a sixth candidate case is refused without blocking the run", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    // Fill the allowance, then type one more and run. The sixth is refused on
    // its own; the judge's cases and the five already accepted still run.
    //
    // Each add waits for the row it creates rather than for a fixed delay:
    // `addCandidateCase` awaits the judge before it accepts anything, so a
    // sleep long enough on a warm laptop is short enough on a cold runner to
    // let the next value overwrite the box mid-add and lose the case.
    for (let index = 0; index < 5; index++) {
      await page.evaluate(
        (value) => {
          document.querySelector("#candidate-case-input").value = value;
          document.querySelector("#candidate-case-add").click();
        },
        `[[${index},7,11,15],${index + 7}]`,
      );
      await page.waitForFunction(
        (expected) =>
          document.querySelector("#candidate-case-list").children.length ===
          expected,
        index + 1,
      );
    }

    await page.locator("#language-select").selectOption("javascript");
    await page.evaluate(() => {
      document.querySelector("#candidate-case-input").value = "[[9,9,9,9],18]";
      document.querySelector("#run-tests").click();
    });
    await page.waitForFunction(() =>
      document
        .querySelector("#results-body")
        .textContent.includes("Your case 5"),
    );
    assert.match(
      await page.locator("#candidate-case-status").textContent(),
      /up to 5 cases/,
    );
    assert.equal(
      await page.locator("#candidate-case-list").locator("li").count(),
      5,
    );
  } finally {
    await page.close();
  }
});

test("removing a saved candidate case frees its slot and persists", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    // Where focus goes after a removal is part of this test, and nothing under
    // the setup check can hold focus while it is open.
    await closeSetupCheck(page);
    for (let index = 0; index < 5; index++) {
      await page.evaluate((index) => {
        document.querySelector("#candidate-case-input").value = JSON.stringify([
          [index, 7],
          index + 7,
        ]);
        document.querySelector("#candidate-case-add").click();
      }, index);
      await page.waitForFunction(
        (count) =>
          document.querySelector("#candidate-case-list").children.length ===
          count,
        index + 1,
      );
    }
    await page.evaluate(() => {
      document.querySelector("#candidate-case").open = true;
    });
    assert.equal(await page.locator("[data-remove-case]").count(), 5);
    await page.evaluate(() =>
      document.querySelector('[data-remove-case="2"]').click(),
    );
    assert.equal(
      await page.locator("#candidate-case-status").textContent(),
      "4/5 cases ready.",
    );
    assert.equal(
      await page.evaluate(() =>
        document.activeElement.getAttribute("aria-label"),
      ),
      "Remove case 3",
    );
    // Reload before the replacement is added: the add writes the in-memory
    // list to storage, so it would cover for a removal whose own write never
    // happened. `candidateCasesReady` cannot pace this one - the placeholder
    // is written only when storage came back empty - so the first rendered
    // row is the ready signal and the count is asserted, not awaited.
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () => document.querySelector("#candidate-case-list").children.length > 0,
    );
    assert.equal(
      await page.locator("#candidate-case-list").locator("li").count(),
      4,
    );
    assert.doesNotMatch(
      await page.locator("#candidate-case-list").textContent(),
      /\[\[2,7\],9\]/,
    );
    await page.evaluate(() => {
      document.querySelector("#candidate-case-input").value = "[[10,7],17]";
      document.querySelector("#candidate-case-add").click();
    });
    await page.waitForFunction(
      () =>
        document.querySelector("#candidate-case-list").children.length === 5,
    );
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () => document.querySelector("#candidate-case-list").children.length > 0,
    );
    assert.equal(
      await page.locator("#candidate-case-list").locator("li").count(),
      5,
    );
    assert.match(
      await page.locator("#candidate-case-list").textContent(),
      /\[\[10,7\],17\]/,
    );
    // The reload opened the setup check again, and focus is asserted below.
    await closeSetupCheck(page);
    // Removal ignores a second click that lands before the next animation
    // frame, so the tight in-page loop this used to be would take one case
    // and then spin. Five clicks from here instead, one frame apart - the
    // row is gone before the click returns, so only a frame can pace the
    // guard - and the empty list is asserted after them, so a removal that
    // stops removing fails by name here instead of spinning or timing out.
    await page.evaluate(() => {
      document.querySelector("#candidate-case").open = true;
    });
    for (let clicks = 0; clicks < 5; clicks++) {
      await page.evaluate(() =>
        document.querySelector("[data-remove-case]").click(),
      );
      await page.evaluate(
        () => new Promise((resolve) => requestAnimationFrame(resolve)),
      );
    }
    assert.equal(
      await page.locator("#candidate-case-list").locator("li").count(),
      0,
    );
    assert.equal(
      await page.locator("#candidate-case-status").textContent(),
      "0/5 cases ready.",
    );
    assert.equal(
      await page.evaluate(() => document.activeElement.id),
      "candidate-case-add",
    );
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
    const judgeHeld = new Promise((resolve) => {
      releaseJudge = resolve;
    });
    await page.route("**/judges/chargeback-pair-match.json", async (route) => {
      await judgeHeld;
      await route.continue();
    });
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    // candidateCasesReady waits on the judge this test is holding, so the
    // rendered rows are the ready signal instead: bindEvents registers the
    // delegated remove listener before it calls initializeCandidateCases, so
    // the seeded rows being on screen means the listener already is too.
    await page.waitForFunction(
      () =>
        document.querySelector("#candidate-case-list").children.length === 2,
    );
    await page.locator("#language-select").selectOption("javascript");
    await page.evaluate(() => {
      document.querySelector("#candidate-case").open = true;
      document.querySelector("#candidate-case-input").value = "[[2,7],9]";
      document.querySelector("#run-tests").click();
    });
    // A removal re-renders the list, so the rows still being there alongside
    // the refusal proves the guard answered the click, not that the click
    // missed its listener.
    await page.evaluate(() =>
      document.querySelector("[data-remove-case]").click(),
    );
    assert.equal(
      await page.locator("#candidate-case-list").locator("li").count(),
      2,
    );
    assert.equal(
      await page.locator("#candidate-case-status").textContent(),
      "Wait for the test run to finish before removing a case.",
    );
    // loadJudge caches per id, so this one release answers the module's judge
    // promise, the parked add and the run. The typed case lands on the answer,
    // per the same contract the add button has, and the list ends at three.
    releaseJudge();
    await page.waitForFunction(() =>
      document
        .querySelector("#results-body")
        .textContent.includes("Your case 1"),
    );
    assert.equal(
      await page.locator("#candidate-case-list").locator("li").count(),
      3,
    );
    // The guard covers the run window only: with the run done, the same click
    // removes. Each awaited step above cost frames, so the double-click guard
    // has long re-armed.
    await page.evaluate(() =>
      document.querySelector("[data-remove-case]").click(),
    );
    assert.equal(
      await page.locator("#candidate-case-list").locator("li").count(),
      2,
    );
    assert.equal(
      await page.locator("#candidate-case-status").textContent(),
      "2/5 cases ready.",
    );
  } finally {
    await page.close();
  }
});

/// Holds every judge request until the returned function is called, so a test
/// can look at the page before the judge has loaded.
async function holdJudge(page) {
  let release;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  await page.route("**/judges/*.json", async (route) => {
    await held;
    await route.continue();
  });
  return release;
}

/// Clicks a tab even when it is disabled, and answers whether it took.
async function selectLanguage(page, language) {
  const select = page.locator("#language-select");
  const option = page.locator(`#language-select option[value="${language}"]`);
  if (await option.isDisabled()) return false;
  await select.selectOption(language);
  return (await select.inputValue()) === language;
}

async function withHeldRequestedC(body) {
  const enabled = await startStaticServer({
    runtimeConfig: CE_RUNTIME_CONFIG,
  });
  const page = await browser.newPage();
  let releaseJudge = () => {};
  try {
    releaseJudge = await holdJudge(page);
    await page.goto(
      `${enabled.base}/interview.html?problem=chargeback-pair-match&language=c`,
      { waitUntil: "domcontentloaded" },
    );
    await page.waitForFunction(
      () => document.querySelector('[data-language="c"]').disabled,
    );
    await body(page, () => releaseJudge());
  } finally {
    releaseJudge();
    await page.close();
    await new Promise((closed) => enabled.server.close(closed));
  }
}

test("disabled compiled runners cannot be selected while the judge loads", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  let releaseJudge = () => {};
  try {
    releaseJudge = await holdJudge(page);
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await page.waitForFunction(
      () => document.querySelector('[data-language="c"]').disabled,
    );
    for (const language of COMPILED_LANGUAGES) {
      const option = page.locator(
        `#language-select option[value="${language}"]`,
      );
      assert.equal(await option.isDisabled(), true);
      assert.match(
        await option.textContent(),
        /unavailable: disabled by server/,
      );
      assert.match(
        await option.getAttribute("title"),
        /disabled by this server/,
      );
      assert.equal(await selectLanguage(page, language), false);
    }
    releaseJudge();
    await candidateCasesReady(page);
    for (const language of ["python", "javascript"]) {
      const option = page.locator(
        `#language-select option[value="${language}"]`,
      );
      assert.equal(await option.isDisabled(), false);
      assert.equal(await selectLanguage(page, language), true);
    }
    for (const language of COMPILED_LANGUAGES) {
      assert.equal(
        await page
          .locator(`#language-select option[value="${language}"]`)
          .isDisabled(),
        true,
      );
    }
  } finally {
    releaseJudge();
    await page.close();
  }
});

test("C stays unavailable until a function-style judge has loaded", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const enabled = await startStaticServer({
    runtimeConfig: CE_RUNTIME_CONFIG,
  });
  try {
    for (const [problem, classStyle] of [
      ["edge-thumbnail-store", true],
      ["chargeback-pair-match", false],
    ]) {
      const page = await browser.newPage();
      let releaseJudge = () => {};
      try {
        const c = page.locator('#language-select option[value="c"]');
        releaseJudge = await holdJudge(page);
        await page.goto(`${enabled.base}/interview.html?problem=${problem}`, {
          waitUntil: "domcontentloaded",
        });
        await page.waitForFunction(
          () => document.querySelector('[data-language="c"]').disabled,
        );
        assert.match(await c.textContent(), /unavailable: waiting for judge/);
        assert.match(
          await c.getAttribute("title"),
          /function-style tests load/,
        );
        assert.equal(await selectLanguage(page, "c"), false);
        releaseJudge();
        await candidateCasesReady(page);
        assert.equal(await c.isDisabled(), classStyle);
        if (classStyle) {
          assert.match(await c.textContent(), /unavailable: class problem/);
          assert.match(await c.getAttribute("title"), /class/);
        } else {
          assert.equal(await selectLanguage(page, "c"), true);
          assert.equal(await c.getAttribute("title"), "");
        }
        for (const language of ["python", "javascript", "cpp", "java"]) {
          assert.equal(
            await page
              .locator(`#language-select option[value="${language}"]`)
              .isDisabled(),
            false,
          );
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

const CASES_KEY = "codetrial.candidateCases.chargeback-pair-match";

async function addCase(page, input) {
  const count = await page.locator("#candidate-case-list > li").count();
  await page.evaluate((value) => {
    document.querySelector("#candidate-case-input").value = value;
    document.querySelector("#candidate-case-add").click();
  }, input);
  await page.waitForFunction(
    (n) => document.querySelector("#candidate-case-list").children.length === n,
    count + 1,
  );
  const stored = await page.evaluate(
    (k) => sessionStorage.getItem(k),
    CASES_KEY,
  );
  assert.ok(stored?.includes(input));
}

test("ending an interview clears candidate cases from session storage", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    await addCase(page, "[[2,7,11,15],9]");

    await page.evaluate(() => document.querySelector("#end").click());
    await page.waitForFunction(
      () =>
        sessionStorage.getItem(
          "codetrial.candidateCases.chargeback-pair-match",
        ) === null,
    );
  } finally {
    await page.close();
  }
});

test("rendering a report clears candidate cases from session storage", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    await addCase(page, "[[2,7,11,15],9]");

    await page.evaluate(() => {
      document.querySelector("#force-report").click();
    });
    await page.waitForFunction(
      () =>
        sessionStorage.getItem(
          "codetrial.candidateCases.chargeback-pair-match",
        ) === null,
    );
  } finally {
    await page.close();
  }
});

test("in-flight candidate case additions are refused when interview leaves live", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  let releaseJudge = () => {};
  try {
    releaseJudge = await holdJudge(page);

    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });

    await page.waitForFunction(() => {
      const add = document.querySelector("#candidate-case-add");
      if (!add || add.disabled) return false;

      document.querySelector("#candidate-case-input").value = "[[2,7,11,15],9]";
      add.click();

      if (!add.disabled) return false;

      document.querySelector("#end").click();
      return true;
    });
    releaseJudge();
    await page.waitForFunction(
      () => !document.querySelector("#candidate-case-add").disabled,
    );
    const stored = await page.evaluate(() =>
      sessionStorage.getItem("codetrial.candidateCases.chargeback-pair-match"),
    );
    assert.equal(stored, null);
    const casesCount = await page.evaluate(
      () => document.querySelector("#candidate-case-list").children.length,
    );
    assert.equal(casesCount, 0);
    const status = await page.evaluate(
      () => document.querySelector("#candidate-case-status").textContent,
    );
    assert.equal(status, "The interview has ended.");
  } finally {
    releaseJudge();
    await page.close();
  }
});

test("ending an interview empties the candidate case list", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    await addCase(page, "[[2,7,11,15],9]");
    await addCase(page, "[[1,2,3],5]");

    await page.evaluate(() => document.querySelector("#end").click());
    await page.waitForFunction(
      () =>
        document.querySelector("#candidate-case-list").children.length === 0,
    );
    const count = await page.locator("#candidate-case-list > li").count();
    assert.equal(count, 0);
  } finally {
    await page.close();
  }
});

test("candidate cases do not persist across new interview sessions in the same tab", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    await addCase(page, "[[2,7,11,15],9]");

    await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);

    const count = await page.locator("#candidate-case-list > li").count();
    assert.equal(count, 0);
    const stored = await page.evaluate(
      (k) => sessionStorage.getItem(k),
      CASES_KEY,
    );
    assert.equal(stored, null);
  } finally {
    await page.close();
  }
});

test("candidate cases are preserved on page reload", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    await addCase(page, "[[2,7,11,15],9]");

    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () =>
        document.querySelector("#candidate-case-list").children.length === 1,
    );

    const count = await page.locator("#candidate-case-list > li").count();
    assert.equal(count, 1);
    const stored = await page.evaluate(
      (k) => sessionStorage.getItem(k),
      CASES_KEY,
    );
    assert.ok(stored?.includes("[[2,7,11,15],9]"));
  } finally {
    await page.close();
  }
});

test("a requested C tab is restored after its judge loads", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  await withHeldRequestedC(async (page, releaseJudge) => {
    assert.equal(await page.locator("#language-select").inputValue(), "python");
    releaseJudge();
    await candidateCasesReady(page);
    assert.equal(await page.locator("#language-select").inputValue(), "c");
  });
});

test("editing before a requested C judge loads keeps Python", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const edited = "# Keep this candidate edit.\n";
  await withHeldRequestedC(async (page, releaseJudge) => {
    await page.locator("#editor").evaluate((editor, value) => {
      editor.value = value;
      editor.dispatchEvent(new Event("input", { bubbles: true }));
    }, edited);
    releaseJudge();
    await candidateCasesReady(page);
    assert.equal(await page.locator("#language-select").inputValue(), "python");
    assert.equal(await page.locator("#editor").inputValue(), edited);
  });
});

test("choosing JavaScript before a requested C judge loads keeps JavaScript", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  await withHeldRequestedC(async (page, releaseJudge) => {
    assert.equal(await selectLanguage(page, "javascript"), true);
    releaseJudge();
    await candidateCasesReady(page);
    assert.equal(
      await page.locator("#language-select").inputValue(),
      "javascript",
    );
  });
});

test("ending before a requested C judge loads keeps Python", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  await withHeldRequestedC(async (page, releaseJudge) => {
    await page.evaluate(() => document.querySelector("#end").click());
    // With no interviewer in the room, the offline report replaces the
    // ending overlay 300ms after End, so a slow machine could miss the
    // overlay. Either one says the interview ended before the judge loaded.
    await page.waitForFunction(
      () =>
        !document.querySelector("#ending-overlay").hidden ||
        !document.querySelector("#report-modal").hidden,
    );
    releaseJudge();
    await candidateCasesReady(page);
    assert.equal(await page.locator("#language-select").inputValue(), "python");
  });
});

test("running before a requested C judge loads keeps Python", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  await withHeldRequestedC(async (page, releaseJudge) => {
    await page.evaluate(() => document.querySelector("#run-tests").click());
    await page.waitForFunction(
      () => document.querySelector("#run-tests").textContent === "Running...",
    );
    releaseJudge();
    await candidateCasesReady(page);
    assert.equal(await page.locator("#language-select").inputValue(), "python");
  });
});

test("saved candidate cases display their expected output safely", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");

  const scenarios = [
    { input: "", shown: "Not specified" },
    { input: "7", shown: "7" },
    { input: "0", shown: "0" },
    { input: "false", shown: "false" },
    { input: "null", shown: "null" },
    { input: "[0,1]", shown: "[0,1]" },
    { input: '"<b>sample</b>"', shown: '"<b>sample</b>"' },
  ];

  for (const scenario of scenarios) {
    const page = await browser.newPage();
    try {
      await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
        waitUntil: "domcontentloaded",
      });
      await candidateCasesReady(page);

      await page.evaluate((expected) => {
        document.querySelector("#candidate-case-input").value =
          "[[2,7,11,15],9]";
        document.querySelector("#candidate-case-expected").value = expected;
        document.querySelector("#candidate-case-add").click();
      }, scenario.input);

      await page.waitForFunction(
        () =>
          document.querySelector("#candidate-case-list").children.length === 1,
      );

      const row = page.locator("#candidate-case-list > li").first();

      assert.equal(
        await row.locator("span").nth(1).textContent(),
        `Expected: ${scenario.shown}`,
        `Expected JSON field: ${JSON.stringify(scenario.input)}`,
      );
      assert.equal(await row.locator("b").count(), 0);
    } finally {
      await page.close();
    }
  }
});

test("editing a saved case at the limit preserves order and persists", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");

  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    await closeSetupCheck(page);
    await page.locator("#candidate-case").evaluate((element) => {
      element.open = true;
    });

    for (let index = 0; index < 5; index++) {
      await page.evaluate((index) => {
        document.querySelector("#candidate-case-input").value = JSON.stringify([
          [index, index + 1],
          index * 2 + 1,
        ]);
        document.querySelector("#candidate-case-expected").value = "[0,1]";
        document.querySelector("#candidate-case-add").click();
      }, index);

      await page.waitForFunction(
        (count) =>
          document.querySelector("#candidate-case-list").children.length ===
          count,
        index + 1,
      );
    }

    const rows = page.locator("#candidate-case-list > li");
    const before = await rows.locator("span:first-child").allTextContents();

    await page.locator('[data-edit-case="2"]').click();

    assert.equal(
      await page.locator("#candidate-case-input").inputValue(),
      "[[2,3],5]",
    );
    assert.equal(
      await page.locator("#candidate-case-expected").inputValue(),
      "[0,1]",
    );
    assert.equal(await page.locator("#run-tests").isDisabled(), true);

    await page.locator("#candidate-case-input").fill("[[8,9],17]");
    await page.locator("#candidate-case-expected").fill("[1,0]");
    await page.locator("#candidate-case-add").click();

    await page.waitForFunction(
      () =>
        document.querySelector("#candidate-case-status").textContent ===
        "Case 3 updated.",
    );

    const expectedRows = [...before];
    expectedRows[2] = "Your case 3: [[8,9],17]";

    assert.deepEqual(
      await rows.locator("span:first-child").allTextContents(),
      expectedRows,
    );
    assert.equal(
      await rows.nth(2).locator("span").nth(1).textContent(),
      "Expected: [1,0]",
    );
    assert.equal(await page.locator("#candidate-case-cancel").isHidden(), true);
    assert.equal(
      await page.locator("#candidate-case-add").textContent(),
      "Add case",
    );

    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () =>
        document.querySelector("#candidate-case-list").children.length === 5,
    );

    assert.deepEqual(
      await rows.locator("span:first-child").allTextContents(),
      expectedRows,
    );
    assert.equal(
      await rows.nth(2).locator("span").nth(1).textContent(),
      "Expected: [1,0]",
    );
  } finally {
    await page.close();
  }
});

test("invalid edits preserve the saved case and Cancel restores the draft", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");

  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    await closeSetupCheck(page);
    await page.locator("#candidate-case").evaluate((element) => {
      element.open = true;
    });

    await page.locator("#candidate-case-input").fill("[[2,7],9]");
    await page.locator("#candidate-case-expected").fill("[0,1]");
    await page.locator("#candidate-case-add").click();

    await page.waitForFunction(
      () =>
        document.querySelector("#candidate-case-list").children.length === 1,
    );

    const row = page.locator("#candidate-case-list > li").first();
    const original = await row.textContent();

    await page.locator("#candidate-case-input").fill("[[8,9],17]");
    await page.locator("#candidate-case-expected").fill("[1,0]");

    await page.locator('[data-edit-case="0"]').click();
    await page.locator("#candidate-case-input").fill("not JSON");
    await page.locator("#candidate-case-add").click();

    await page.waitForFunction(
      () => !document.querySelector("#candidate-case-add").disabled,
    );

    assert.equal(await row.textContent(), original);
    assert.equal(
      await page.locator("#candidate-case-input").inputValue(),
      "not JSON",
    );
    assert.equal(
      await page.locator("#candidate-case-add").textContent(),
      "Save changes",
    );
    assert.equal(
      await page.locator("#candidate-case-cancel").isVisible(),
      true,
    );

    await page.locator("#candidate-case-cancel").click();

    assert.equal(await row.textContent(), original);
    assert.equal(
      await page.locator("#candidate-case-input").inputValue(),
      "[[8,9],17]",
    );
    assert.equal(
      await page.locator("#candidate-case-expected").inputValue(),
      "[1,0]",
    );
    assert.equal(
      await page.locator("#candidate-case-add").textContent(),
      "Add case",
    );
    assert.equal(await page.locator("#candidate-case-cancel").isHidden(), true);
    assert.equal(await page.locator("#run-tests").isDisabled(), false);
    assert.equal(
      await page
        .locator('[data-edit-case="0"]')
        .evaluate((button) => button === document.activeElement),
      true,
    );
  } finally {
    await page.close();
  }
});

test("Run tests refuses an active edit without adding a duplicate", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");

  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    await closeSetupCheck(page);
    await page.locator("#candidate-case").evaluate((element) => {
      element.open = true;
    });
    await addCase(page, "[[2,7],9]");

    await page.locator('[data-edit-case="0"]').click();
    await page.locator("#candidate-case-input").fill("[[8,9],17]");

    assert.equal(await page.locator("#run-tests").isDisabled(), true);

    // Exercise the handler guard even when the disabled button is bypassed.
    await page.evaluate(() => {
      document
        .querySelector("#run-tests")
        .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    assert.equal(
      await page.locator("#candidate-case-status").textContent(),
      "Save or cancel the current edit before running tests.",
    );
    assert.equal(await page.locator("#candidate-case-list > li").count(), 1);
    assert.equal(
      await page.locator("#candidate-case-input").inputValue(),
      "[[8,9],17]",
    );
    assert.equal(
      await page.locator("#candidate-case-add").textContent(),
      "Save changes",
    );
  } finally {
    await page.close();
  }
});

test("ending during a pending save clears the edit without restoring cases", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");

  const page = await browser.newPage();
  try {
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await candidateCasesReady(page);
    await closeSetupCheck(page);
    await page.locator("#candidate-case").evaluate((element) => {
      element.open = true;
    });
    await addCase(page, "[[2,7],9]");

    await page.locator('[data-edit-case="0"]').click();
    await page.locator("#candidate-case-input").fill("[[8,9],17]");

    // End in the same task, before the save resumes after awaiting the judge.
    await page.evaluate(() => {
      document.querySelector("#candidate-case-add").click();
      document.querySelector("#end").click();
    });

    await page.waitForFunction(
      () =>
        document.querySelector("#candidate-case-list").children.length === 0 &&
        !document.querySelector("#candidate-case-add").disabled,
    );

    assert.equal(await page.locator("#candidate-case-cancel").isHidden(), true);
    assert.equal(
      await page.locator("#candidate-case-add").textContent(),
      "Add case",
    );
    assert.equal(
      await page.evaluate(() =>
        sessionStorage.getItem(
          "codetrial.candidateCases.chargeback-pair-match",
        ),
      ),
      null,
    );
    assert.equal(
      await page.locator("#candidate-case-status").textContent(),
      "The interview has ended.",
    );
  } finally {
    await page.close();
  }
});

async function openCandidateEditPage(problem = "conveyor-grid-routing") {
  const page = await browser.newPage({
    viewport: { width: 1280, height: 900 },
  });
  await page.goto(`${base}/interview.html?problem=${problem}`, {
    waitUntil: "domcontentloaded",
  });
  await page.waitForFunction(
    () => document.querySelector("#candidate-case-input")?.placeholder,
  );
  await closeSetupCheck(page);
  await page.locator("#candidate-case").evaluate((element) => {
    element.open = true;
  });
  return page;
}
async function addCaseWithExpectation(page, input, expected = "") {
  const count = await page.locator("#candidate-case-list > li").count();
  await page.locator("#candidate-case-input").fill(input);
  await page.locator("#candidate-case-expected").fill(expected);
  await page.locator("#candidate-case-add").click();
  await page.waitForFunction(
    (count) =>
      document.querySelector("#candidate-case-list").children.length === count,
    count + 1,
  );
}
async function saveEditedCandidateCase(page, input, expected = "") {
  await page.locator("#candidate-case-input").fill(input);
  await page.locator("#candidate-case-expected").fill(expected);
  await page.locator("#candidate-case-add").click();
  await page.waitForFunction(
    () =>
      document.querySelector("#candidate-case-status").textContent ===
      "Case 1 updated.",
  );
}
async function runEditedCandidateTests(page) {
  await page.locator("#run-tests").click();
  await page.waitForFunction(
    () =>
      !document.querySelector("#run-tests").disabled &&
      document
        .querySelector("#results-content")
        .textContent.includes("Your case 1"),
  );
}
test("edited inputs actually run; save clears current results; Cancel and saved run history remain intact", async () => {
  const page = await openCandidateEditPage();
  try {
    await page.locator("#language-select").selectOption("javascript");
    await page
      .locator("#editor")
      .fill("function cheapestRouteCost(grid) { return grid[0][0]; }");
    await addCaseWithExpectation(page, "[[[2]]]", "3");
    await runEditedCandidateTests(page);
    const results = page.locator("#results-content");
    const first = await results.textContent();
    assert.match(first, /expected 3\s+got 2/);
    await page.locator('[data-edit-case="0"]').click();
    await page.locator("#candidate-case-input").fill("[[[99]]]");
    await page.locator("#candidate-case-cancel").click();
    assert.equal(await results.textContent(), first);
    await page.locator('[data-edit-case="0"]').click();
    await saveEditedCandidateCase(page, "[[[7]]]", "8");
    assert.equal(
      (await results.textContent()).trim(),
      "Run the test cases any time - Jim sees your results too.",
    );
    assert.equal(await page.locator("#results-filter").isHidden(), true);
    await runEditedCandidateTests(page);
    assert.match(await results.textContent(), /expected 8\s+got 7/);

    await page.locator('[data-edit-case="0"]').click();

    await page.locator("#candidate-case-cancel").click();
    await page.locator("#force-report").evaluate((button) => button.click());
    const downloadPromise = page.waitForEvent("download");
    await page.locator("#download-report").click();
    const download = await downloadPromise;
    const markdown = await readFile(await download.path(), "utf8");
    const history = markdown.slice(markdown.indexOf("## Test Run History"));
    assert.equal([...history.matchAll(/^### Run \d+/gm)].length, 2);
    const oldRun = history.split("### Run 2")[0];
    const newRun = history.split("### Run 2")[1];
    assert.match(oldRun.replaceAll("\\[", "["), /- Input: \[\[\[2\]\]\]/);
    assert.match(oldRun, /- Expected: 3\s+- Actual: 2/);
    assert.match(newRun.replaceAll("\\[", "["), /- Input: \[\[\[7\]\]\]/);
    assert.match(newRun, /- Expected: 8\s+- Actual: 7/);
  } finally {
    await page.close();
  }
});
test("editing preserves absent expectations and explicit JSON values", async () => {
  const page = await openCandidateEditPage();
  try {
    await addCaseWithExpectation(page, "[[[1]]]", "7");
    for (const text of ["", "null", "0", "false", '"<b>sample</b>"', "[0,1]"]) {
      await page.locator('[data-edit-case="0"]').click();
      await saveEditedCandidateCase(page, "[[[1]]]", text);
      const stored = await page.evaluate(() =>
        JSON.parse(
          sessionStorage.getItem(
            "codetrial.candidateCases.conveyor-grid-routing",
          ),
        ),
      );
      assert.equal(Object.hasOwn(stored[0], "expected"), text !== "");
      if (text !== "") assert.deepEqual(stored[0].expected, JSON.parse(text));
      assert.equal(await page.locator("#candidate-case-list > li").count(), 1);
      assert.equal(
        await page
          .locator("#candidate-case-list > li span")
          .nth(1)
          .textContent(),
        `Expected: ${text || "Not specified"}`,
      );
      assert.equal(await page.locator("#candidate-case-list b").count(), 0);
      await page.locator('[data-edit-case="0"]').click();
      assert.equal(
        await page.locator("#candidate-case-expected").inputValue(),
        text,
      );
      await page.locator("#candidate-case-cancel").click();
    }
  } finally {
    await page.close();
  }
});
test("invalid expected JSON and repeated saves cannot corrupt or duplicate a saved case", async () => {
  const page = await openCandidateEditPage();
  try {
    await addCaseWithExpectation(page, "[[[1]]]", "1");
    await page.locator('[data-edit-case="0"]').click();
    await page.locator("#candidate-case-expected").fill("not JSON");
    await page.locator("#candidate-case-add").click();
    await page.waitForFunction(
      () => !document.querySelector("#candidate-case-add").disabled,
    );
    assert.equal(
      await page.locator("#candidate-case-list > li span").nth(1).textContent(),
      "Expected: 1",
    );
    assert.equal(
      await page.locator("#candidate-case-expected").inputValue(),
      "not JSON",
    );
    await page.locator("#candidate-case-input").fill("[[[2]]]");
    await page.locator("#candidate-case-expected").fill("2");
    await page.evaluate(() => {
      const button = document.querySelector("#candidate-case-add");
      button.click();
      button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await page.waitForFunction(
      () =>
        document.querySelector("#candidate-case-status").textContent ===
        "Case 1 updated.",
    );
    assert.equal(await page.locator("#candidate-case-list > li").count(), 1);
    assert.equal(
      await page
        .locator("#candidate-case-list > li span")
        .first()
        .textContent(),
      "Your case 1: [[[2]]]",
    );
    await page.locator('[data-edit-case="0"]').click();
    assert.equal(
      await page.locator('[data-remove-case="0"]').isDisabled(),
      true,
    );
    await page.locator('[data-remove-case="0"]').dispatchEvent("click");
    assert.equal(await page.locator("#candidate-case-list > li").count(), 1);
    assert.match(
      await page.locator("#candidate-case-status").textContent(),
      /Save or cancel/,
    );
  } finally {
    await page.close();
  }
});
test("editing is refused during an active run", async () => {
  const page = await browser.newPage();
  let release;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  try {
    await page.addInitScript(() =>
      sessionStorage.setItem(
        "codetrial.candidateCases.chargeback-pair-match",
        JSON.stringify([{ input: [[2, 7], 9], expected: [0, 1] }]),
      ),
    );
    await page.route("**/judges/*.json", async (route) => {
      await held;
      await route.continue();
    });
    await page.goto(`${base}/interview.html?problem=chargeback-pair-match`, {
      waitUntil: "domcontentloaded",
    });
    await page.waitForFunction(
      () =>
        document.querySelector("#candidate-case-list").children.length === 1,
    );
    await closeSetupCheck(page);
    await page.locator("#candidate-case").evaluate((element) => {
      element.open = true;
    });
    await page.locator("#language-select").selectOption("javascript");
    await page.locator("#run-tests").click();
    await page.locator('[data-edit-case="0"]').click();
    assert.match(
      await page.locator("#candidate-case-status").textContent(),
      /Wait for the current operation/,
    );
    assert.equal(await page.locator("#candidate-case-cancel").isHidden(), true);
    assert.equal(
      (await page.locator("#candidate-case-add").textContent()).trim(),
      "Add case",
    );
  } finally {
    release();
    await page.close();
  }
});

test("editing retains the palindrome expectation restriction", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const page = await openCandidateEditPage("mirrored-tag-segment");
  try {
    await addCaseWithExpectation(page, '["aba"]', '"aba"');
    await page.locator('[data-edit-case="0"]').click();
    await page.locator("#candidate-case-expected").fill("null");
    await page.locator("#candidate-case-add").click();
    await page.waitForFunction(
      () => !document.querySelector("#candidate-case-add").disabled,
    );
    assert.equal(
      await page.locator("#candidate-case-list > li span").nth(1).textContent(),
      'Expected: "aba"',
    );
    assert.equal(
      await page.locator("#candidate-case-status").textContent(),
      "A palindrome expectation must be a string, or leave it blank to observe the result.",
    );
    assert.equal(
      await page.locator("#candidate-case-expected").inputValue(),
      "null",
    );
    await page.locator("#candidate-case-cancel").click();
    await page.locator('[data-edit-case="0"]').click();
    await saveEditedCandidateCase(page, '["aba"]', "");
    assert.equal(
      await page.locator("#candidate-case-list > li span").nth(1).textContent(),
      "Expected: Not specified",
    );
  } finally {
    await page.close();
  }
});

test("saving edits preserves Practice storage and Strict non-persistence", async () => {
  const spec = JSON.parse(
    await readFile(
      new URL("../../web/judges/conveyor-grid-routing.json", import.meta.url),
      "utf8",
    ),
  );
  for (const strict of [false, true]) {
    const history = [{ runNumber: 1, cases: [{ got: "2" }] }];
    const state = {
      candidateCaseEdit: { index: 0 },
      candidateCaseAddition: null,
      candidateCases: [{ input: [[[2]]], expected: 3 }],
      runningTests: false,
      phase: "live",
      latestSummary: { passed: 0 },
      testHistory: history,
    };
    const writes = [];
    const storage = {
      setItem(key, value) {
        writes.push({ key, value });
      },
    };
    const nodes = {
      candidateCaseInput: { value: "[[[7]]]" },
      candidateCaseExpected: { value: "8" },
      candidateCaseAdd: {},
      candidateCaseCancel: {},
      candidateCaseStatus: {},
      resultsLabel: {},
      resultsFilter: {},
      resultsContent: {},
    };
    const { saveCandidateCaseEdit } = loadInterview(
      [
        "candidateCaseFromForm",
        "invalidateCandidateCaseResults",
        "saveCandidateCaseEdit",
      ],
      {
        state,
        nodes,
        strict,
        tabStorage: storage,
        candidateCaseStorageKey: "synthetic-cases",
        problem: { page: "conveyor-grid-routing" },
        loadJudge: async () => spec,
        parseCandidateCase,
        codingClosed: () => false,
        writeStored,
        cancelCandidateCaseEdit: () => {
          state.candidateCaseEdit = null;
        },
      },
    );
    await saveCandidateCaseEdit();
    assert.deepEqual(state.candidateCases, [{ input: [[[7]]], expected: 8 }]);
    assert.equal(state.latestSummary, null);
    assert.strictEqual(state.testHistory, history);
    assert.deepEqual(history, [{ runNumber: 1, cases: [{ got: "2" }] }]);
    assert.equal(writes.length, strict ? 0 : 1);
    if (!strict)
      assert.deepEqual(JSON.parse(writes[0].value), state.candidateCases);
  }
});
