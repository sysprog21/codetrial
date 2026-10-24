// The lobby's behaviour is a state machine over four things a candidate can
// touch and two the server answers late, and none of it is visible to a test
// that reads app.js as text. Every bug this file pins was found by driving a
// real browser and none of them by reading: a recommendation that rerolled when
// the report fetch landed, a card click that wiped the difficulty filter and
// the duration with it, a start button that shipped `?problem=undefined`, an
// interview length derived from a checkbox nobody had checked.
//
// The server here is a stub rather than the built binary. Nothing under test
// needs Rust: app.js reads two JSON endpoints and the static files, so a stub
// keeps this hermetic and fast enough to sit in the default gate.
//
// Skipped, not failed, when Playwright's Chromium is not installed, the same
// contract tests/browser/face-detector.test.js works under. CI installs it
// before scripts/test.sh, so a skip there means the install did nothing.
import { test, before, beforeEach, after } from "node:test";
import assert from "node:assert/strict";

import {
  functionBody,
  launchChromium,
  reaches,
  read,
  startStaticServer,
} from "./source.js";
import { ACTIVE_CONTRACT, frameworkPhases } from "../../web/lib.js";
import { PRACTICE_LIST_KEY } from "../../web/practice-list.js";

let browser = null;
let server = null;
let base = "";

/// What the stub answers with. Set per test before the page loads, because the
/// whole point of most of these is what the lobby does once these arrive.
let session = { signedIn: true, user: { login: "candidate" } };
let reports = [];
/// Endpoint paths the stub answers with a 500 instead of a body, so the tests
/// about a server that cannot answer are about an actual failure rather than a
/// success carrying an odd payload.
let failing = new Set();
/// When set, `/api/reports` waits for this before answering. Tests about what
/// the lobby does before its history arrives hold the page in that state and
/// release it deliberately, rather than racing a stopwatch that a loaded CI
/// worker wins.
let holdReports = null;
/// Hold the initial account check separately from the reports it may load.
let holdSession = null;
/// The same gate for `/api/login`, so a test about what the candidate can touch
/// while a start is mid-flight holds that request open rather than guessing a
/// latency long enough to win the race.
let holdLogin = null;
let deleteRequests = 0;
/// Ids `DELETE /api/reports/{id}` was asked for, in order.
let deletedIds = [];

before(async () => {
  browser = await launchChromium();
  if (!browser) return;

  ({ server, base } = await startStaticServer({
    // Answered at once. A blanket delay here was about five of this file's
    // eleven seconds, paid twice per page load, and the two tests that need to
    // act while a request is in flight hold that request open instead, which is
    // a state rather than a race they hope to win.
    handle: async (request, response, url) => {
      const json = (body) => {
        response.setHeader("content-type", "application/json");
        response.end(JSON.stringify(body));
        return true;
      };

      if (request.method === "DELETE" && url.pathname === "/api/reports") {
        deleteRequests += 1;
      }

      if (failing.has(url.pathname)) {
        response.statusCode = 500;
        response.end("nope");
        return true;
      }
      if (url.pathname === "/api/session") {
        if (holdSession) await holdSession;
        return json(session);
      }
      if (
        request.method === "DELETE" &&
        url.pathname.startsWith("/api/reports/")
      ) {
        const id = decodeURIComponent(
          url.pathname.slice("/api/reports/".length),
        );
        deletedIds.push(id);
        const before = reports.length;
        reports = reports.filter((report) => report.id !== id);
        return json({ deleted: before - reports.length });
      }
      if (url.pathname === "/api/reports") {
        if (request.method === "DELETE") {
          const deleted = reports.length;
          reports = [];
          return json({ deleted });
        }
        if (holdReports) await holdReports;
        return json({ reports });
      }
      if (url.pathname === "/api/login") {
        session = { signedIn: true, user: { login: "candidate" } };
        if (holdLogin) await holdLogin;
        return json({ ok: true });
      }
      // Only somewhere for the start button to land, so the query string it
      // built can be read back off the URL.
      if (url.pathname === "/interview") {
        response.setHeader("content-type", "text/html");
        response.end("<h1>interview</h1>");
        return true;
      }
      return false;
    },
  }));
});

/// Reset here rather than at the top of each test. The per-test assignments
/// only ran when the test before them reached its last line, so one failure
/// leaked a stub mode into everything after it and turned a single red test
/// into a run nobody could read.
beforeEach(() => {
  session = { signedIn: true, user: { login: "candidate" } };
  reports = [];
  failing = new Set();
  holdReports = null;
  holdSession = null;
  holdLogin = null;
  deleteRequests = 0;
  deletedIds = [];
});

/// Hold `/api/reports` open and hand back the release. Everything between the
/// two is the pre-history state, for as long as the assertions need.
function holdHistory() {
  let release;
  holdReports = new Promise((resolve) => (release = resolve));
  return release;
}

after(async () => {
  await browser?.close();
  await new Promise((closed) => (server ? server.close(closed) : closed()));
});

/// Everything a candidate can see about what pressing start would do.
const snapshot = (page) =>
  page.evaluate(() => {
    const one = (selector) => document.querySelector(selector);
    return {
      note: one("#recommendation").textContent,
      focus: one("#practice-focus").hidden
        ? ""
        : one("#practice-focus").textContent,
      card: one(".problem-card.selected")?.dataset.problem ?? null,
      pressed:
        one('.problem-card[aria-pressed="true"]')?.dataset.problem ?? null,
      duration: one(".duration-button.selected")?.dataset.duration ?? null,
      durationsOff: [...document.querySelectorAll(".duration-button")]
        .filter((button) => button.disabled)
        .map((button) => button.dataset.duration),
      durationNote: one("#duration-note").hidden
        ? ""
        : one("#duration-note").textContent,
      levels: [...document.querySelectorAll('[name="difficulty"]:checked')].map(
        (i) => i.value,
      ),
      visibleCards: [...document.querySelectorAll(".problem-card")].filter(
        (c) => !c.hidden,
      ).length,
      startDisabled: one("#start").disabled,
      startText: one("#start").textContent,
      account: one("#account-status").textContent,
      history: one("#history").hidden ? "" : one("#history").textContent,
    };
  });

/// Toggling through the DOM rather than `page.check`, because a card the filter
/// has hidden is not clickable and half these cases are about hidden cards.
const setLevel = (page, level, on) =>
  page.evaluate(
    ([level, on]) => {
      const input = document.querySelector(
        `[name="difficulty"][value="${level}"]`,
      );
      if (input.checked === on) return;
      input.checked = on;
      input.dispatchEvent(new Event("change", { bubbles: true }));
    },
    [level, on],
  );

/// A page whose first recommendation has landed.
async function lobby(page) {
  await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    () => document.querySelector("#recommendation").textContent !== "",
  );
  return snapshot(page);
}

/// The length index.html preselects, which tests/web.rs pins to the server's
/// DEFAULT_DURATION_MIN. Read from the markup rather than from app.js's
/// `let duration`, which the module-scope setup overwrites before anything
/// reads it: a chain running through a value nothing observes is not a chain.
const markupDuration = () =>
  read("web/index.html").match(
    /duration-button selected"[^>]*data-duration="(\d+)"/,
  )[1];

/// The page name a card ships in the URL, from the map the generator writes.
const pageOf = (problemId) =>
  JSON.parse(read("web/problem-pages.json"))[problemId].page;
const TOPICS_BY_PAGE = JSON.parse(read("web/problem-topics.json"));

const assessed =
  (decision) =>
  (problemId, { createdAt, topics } = {}) => ({
    problemId,
    ...(createdAt !== undefined && { createdAt }),
    payload: { report: { decision, ...(topics && { topics }) } },
  });
const hired = assessed("HIRE");
const missed = assessed("NO_HIRE");
const isFocused = (locator) =>
  locator.evaluate((node) => node === document.activeElement);
const savedAttempt = (problemId) => ({
  problemId,
  payload: {
    problemId,
    date: "2026-01-01T00:00:00Z",
    report: { decision: "HIRE" },
  },
});

/// A graded report whose plan files `weaknesses`, most important first, under
/// one `phase`. `assessed` adds the phase scores, whose weakness tags are what
/// the progress panel reads; without it the report is one no rubric scored.
const focusedAttempt = (
  problemId,
  { phase = "Test", weaknesses = ["Test boundaries"], assessed = false } = {},
) => ({
  problemId,
  payload: {
    problemId,
    date: "2026-01-01T00:00:00Z",
    report: {
      codingScore: 60,
      communicationScore: 60,
      decision: "NO_HIRE",
      summary: "Grounded assessment.",
      codingFeedback: { strengths: [], improvements: weaknesses },
      communicationFeedback: { strengths: [], improvements: [] },
      improvementPlan: weaknesses.map((weakness) => ({
        phase,
        weakness,
        impact: "high",
        frequency: 1,
        drill: "Build a test table",
        durationMin: 10,
        successCriterion: "Predict each output",
        selfReview: ["Name a boundary"],
      })),
      ...(assessed && {
        frameworkAssessment: {
          rubricVersion: 1,
          phases: frameworkPhases.map((row) => ({
            phase: row,
            score: row === phase ? 50 : null,
            weaknessTags: row === phase ? weaknesses : [],
          })),
        },
      }),
    },
  },
});

/// Ids from the real bank, so these break if the bank stops carrying them
/// rather than testing against problems that do not exist.
// Cards and new history are keyed by page name; `legacy` history below still
// carries the published ids an earlier build saved.
const EASY = ["two-sum", "valid-parentheses"].map(pageOf);
const MEDIUM = ["jump-game", "gas-station"].map(pageOf);
const HARD = ["candy", "trapping-rain-water"].map(pageOf);

/// A test with a page of its own, closed even when an assertion throws. The
/// bare `await page.close()` each of these used to end on was skipped by any
/// failure, leaving a Chromium page and its 150-card DOM alive for the rest of
/// the run.
function lobbyTest(name, body, options) {
  test(name, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const context = await browser.newContext(options);
    const page = await context.newPage();
    try {
      await body(page, t);
    } finally {
      await context.close();
    }
  });
}

// Wait for the synchronization task, not a guessed network delay.
const normalPageshow = (page) =>
  page.evaluate(
    () =>
      new Promise((resolve) => {
        window.dispatchEvent(new PageTransitionEvent("pageshow"));
        setTimeout(resolve, 0);
      }),
  );

lobbyTest(
  "restored Strict waits for the initial session and reports despite deferred synchronization",
  async (page) => {
    let releaseSession;
    holdSession = new Promise((resolve) => (releaseSession = resolve));
    const releaseReports = holdHistory();
    await page.route("**/app.js", (route) =>
      route.fulfill({
        contentType: "text/javascript",
        body: `document.querySelector("#selection-mode").value = "strict";
${read("web/app.js")}`,
      }),
    );
    try {
      const sessionRequest = page.waitForRequest(
        (request) => new URL(request.url()).pathname === "/api/session",
      );
      await page.goto(`${base}/`, { waitUntil: "load" });
      await sessionRequest;

      for (const pending of ["session", "reports"]) {
        await normalPageshow(page);
        assert.equal(
          await page.isDisabled("#start"),
          true,
          `Strict was enabled while the initial ${pending} request was pending`,
        );
        // Dispatch directly to cover the entry guard as well as the disabled
        // button: DOM dispatch does not enforce a button's disabled attribute.
        const afterClick = await page.evaluate(() => {
          document
            .querySelector("#start")
            .dispatchEvent(new MouseEvent("click", { bubbles: true }));
          return {
            startDisabled: document.querySelector("#start").disabled,
            selectionDisabled:
              document.querySelector("#selection-mode").disabled,
          };
        });
        assert.deepEqual(afterClick, {
          startDisabled: true,
          selectionDisabled: false,
        });
        if (pending === "session") {
          const reportsRequest = page.waitForRequest(
            (request) => new URL(request.url()).pathname === "/api/reports",
          );
          releaseSession();
          await reportsRequest;
        }
      }

      releaseReports();
      await awaitReady(page);
      assert.equal(await page.isDisabled("#start"), false);
      await page.click("#start");
      await page.waitForURL(/\/interview/);
      const params = new URL(page.url()).searchParams;
      assert.equal(params.get("selection"), "strict");
      assert.deepEqual(params.getAll("difficulty"), ["Medium"]);
      assert.equal(params.has("problem"), false);
    } finally {
      releaseSession();
      releaseReports();
    }
  },
);

// Restore the DOM value directly: browser form restoration emits no change.
for (const restoration of [
  { name: "before initialization", initial: true },
  { name: "before normal pageshow", persisted: false },
  { name: "after normal pageshow", persisted: false, after: true },
  { name: "before cached pageshow", persisted: true },
  { name: "after cached pageshow", persisted: true, after: true },
]) {
  lobbyTest(
    `a Strict selection restored ${restoration.name} keeps its view and Start URL`,
    async (page) => {
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.addInitScript(() => {
        window.selectionChanges = 0;
        document.addEventListener("change", (event) => {
          if (event.target.id === "selection-mode") window.selectionChanges++;
        });
      });
      if (restoration.initial) {
        await page.route("**/app.js", (route) =>
          route.fulfill({
            contentType: "text/javascript",
            // Set the restored value before the module runs, then observe its
            // view before pageshow can mask a missing initialization call.
            body: `
document.querySelector("#selection-mode").value = "strict";
${read("web/app.js")}
window.initialSelectionView = [
  document.querySelector(".random-picker").hidden,
  document.querySelector(".problem-picker").hidden,
  document.querySelector("#selection-note").hidden,
];
`,
          }),
        );
      }
      await lobby(page);
      if (restoration.initial) {
        assert.deepEqual(
          await page.evaluate(() => window.initialSelectionView),
          [true, true, false],
        );
      } else {
        await page.evaluate(({ persisted, after }) => {
          const select = document.querySelector("#selection-mode");
          if (!after) select.value = "strict";
          window.dispatchEvent(
            new PageTransitionEvent("pageshow", { persisted }),
          );
          if (after) select.value = "strict";
        }, restoration);
      }
      await page.waitForFunction(
        () => document.querySelector(".problem-picker").hidden,
      );
      assert.equal(
        await page.locator("#selection-mode").inputValue(),
        "strict",
      );
      assert.equal(await page.locator(".random-picker").isVisible(), false);
      assert.equal(await page.locator(".problem-picker").isVisible(), false);
      assert.equal(await page.locator("#selection-note").isVisible(), true);
      assert.equal(await page.locator(".difficulty-row").isVisible(), true);
      assert.equal(await page.evaluate(() => window.selectionChanges), 0);
      await awaitReady(page);
      await page.locator("#start").click();
      await page.waitForURL(/\/interview/);
      const params = new URL(page.url()).searchParams;
      assert.equal(params.get("selection"), "strict");
      assert.deepEqual(params.getAll("difficulty"), ["Medium"]);
      assert.equal(params.has("problem"), false);
      assert.deepEqual(errors, []);
    },
  );
}

lobbyTest(
  "Start reads a silently restored Strict selection before building its URL",
  async (page) => {
    await lobby(page);
    // Restoring and clicking in one task also covers a pageshow sync that
    // has been scheduled but has not run yet.
    const view = await page.evaluate(() => {
      window.dispatchEvent(new PageTransitionEvent("pageshow"));
      document.querySelector("#selection-mode").value = "strict";
      document.querySelector("#start").click();
      return [
        document.querySelector(".random-picker").hidden,
        document.querySelector(".problem-picker").hidden,
        document.querySelector("#selection-note").hidden,
      ];
    });
    assert.deepEqual(view, [true, true, false]);
    await page.waitForURL(/\/interview/);
    const params = new URL(page.url()).searchParams;
    assert.equal(params.get("selection"), "strict");
    assert.deepEqual(params.getAll("difficulty"), ["Medium"]);
    assert.equal(params.has("problem"), false);
  },
);

for (const guestModeAvailable of [false, true]) {
  lobbyTest(
    `Strict requires lobby sign-in when guest admission is ${guestModeAvailable ? "enabled" : "disabled"}`,
    async (page) => {
      session = { signedIn: false, loginRequired: true, guestModeAvailable };
      let logins = 0;
      page.on("request", (request) => {
        if (new URL(request.url()).pathname === "/api/login") logins++;
      });
      await lobby(page);
      // Restore the form silently, as Firefox may do after initialization.
      await page.evaluate(() => {
        document.querySelector("#selection-mode").value = "strict";
      });
      await normalPageshow(page);
      assert.equal(await page.isDisabled("#start"), true);
      assert.match(
        await page.locator("#selection-note").textContent(),
        /Sign in/,
      );
      assert.equal(await page.locator("#github-login").isVisible(), true);
      assert.equal(await page.locator("#login-link").isVisible(), true);
      await page.evaluate(() => {
        document.querySelector("#start").dispatchEvent(new MouseEvent("click"));
      });
      await normalPageshow(page);
      assert.equal(new URL(page.url()).pathname, "/");
      assert.equal(logins, 0);
      assert.equal(await page.isDisabled("#selection-mode"), false);

      await page.fill("#github-login", "candidate");
      await page.click("#login-link");
      await awaitReady(page);
      assert.match((await snapshot(page)).account, /Signed in as candidate/);
      assert.equal(logins, 1);
      await page.click("#start");
      await page.waitForURL("**/interview?**");
      const params = new URL(page.url()).searchParams;
      assert.equal(params.get("selection"), "strict");
      assert.equal(params.has("problem"), false);
      assert.equal(params.has("language"), false);
    },
  );
}

lobbyTest(
  "Guest Practice still starts without signing in after visiting Strict",
  async (page) => {
    session = {
      signedIn: false,
      loginRequired: true,
      guestModeAvailable: true,
    };
    let logins = 0;
    page.on("request", (request) => {
      if (new URL(request.url()).pathname === "/api/login") logins++;
    });
    await lobby(page);
    const picked = (await snapshot(page)).card;
    await page.selectOption("#selection-mode", "strict");
    assert.equal(await page.isDisabled("#start"), true);
    await page.selectOption("#selection-mode", "practice");
    assert.equal(await page.isDisabled("#start"), false);
    await page.click("#start");
    await page.waitForURL("**/interview?**");
    const params = new URL(page.url()).searchParams;
    assert.equal(params.get("problem"), picked);
    assert.equal(params.has("selection"), false);
    assert.equal(logins, 0);
  },
);

lobbyTest(
  "editor options default to all five features enabled",
  async (page) => {
    await lobby(page);
    const group = page.getByRole("group", { name: "Editor options" });
    assert.equal(await group.getByRole("checkbox").count(), 5);
    for (const label of [
      "Syntax highlighting",
      "Code execution",
      "Auto indentation",
      "Bracket auto-close",
      "Monospace font",
    ])
      assert.equal(
        await group
          .getByRole("checkbox", { name: label, exact: true })
          .isChecked(),
        true,
        label,
      );
    const difficulty = await page
      .getByRole("group", { name: "Difficulty", exact: true })
      .boundingBox();
    const options = await group.boundingBox();
    assert.ok(options.y >= difficulty.y + difficulty.height);
    await page.setViewportSize({ width: 375, height: 800 });
    for (const label of await group.locator("label").all()) {
      const bounds = await label.boundingBox();
      assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= 375);
    }
  },
);

lobbyTest(
  "whiteboard disables editor options without losing editor preferences",
  async (page) => {
    await lobby(page);
    const group = page.getByRole("group", { name: "Editor options" });
    const highlight = group.getByRole("checkbox", {
      name: "Syntax highlighting",
      exact: true,
    });
    await highlight.uncheck();
    await page.locator('[data-mode="whiteboard"]').click();
    for (const input of await group.getByRole("checkbox").all())
      assert.equal(await input.isDisabled(), true);
    assert.equal(await highlight.isChecked(), false);
    await page.locator('[data-mode="coding"]').click();
    for (const input of await group.getByRole("checkbox").all())
      assert.equal(await input.isDisabled(), false);
    assert.equal(await highlight.isChecked(), false);
    await lobby(page);
    assert.equal(await highlight.isChecked(), false);
    assert.equal(await highlight.isDisabled(), false);
  },
);

lobbyTest(
  "editor options survive reloads and travel with the selected problem",
  async (page) => {
    await lobby(page);
    await page
      .getByRole("checkbox", { name: "Syntax highlighting", exact: true })
      .uncheck();
    await page
      .getByRole("checkbox", { name: "Bracket auto-close", exact: true })
      .uncheck();
    await lobby(page);
    assert.equal(
      await page
        .getByRole("checkbox", { name: "Syntax highlighting", exact: true })
        .isChecked(),
      false,
    );
    assert.equal(
      await page
        .getByRole("checkbox", { name: "Bracket auto-close", exact: true })
        .isChecked(),
      false,
    );
    assert.equal(
      await page
        .getByRole("checkbox", { name: "Code execution", exact: true })
        .isChecked(),
      true,
    );
    await page.click("#start");
    await page.waitForURL(/\/interview/);
    const params = new URL(page.url()).searchParams;
    assert.equal(params.get("editorHighlight"), "0");
    assert.equal(params.get("editorAutoClose"), "0");
    for (const name of [
      "editorExecution",
      "editorAutoIndent",
      "editorMonospace",
    ])
      assert.equal(params.get(name), null, name);
    assert.ok(params.get("problem"));
  },
);

lobbyTest(
  "editor options are captured before the start waits for login",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await lobby(page);
    await page
      .getByRole("checkbox", { name: "Code execution", exact: true })
      .uncheck();
    let finishLogin;
    holdLogin = new Promise((resolve) => (finishLogin = resolve));
    try {
      await page.fill("#github-login", "candidate");
      const request = page.waitForRequest(
        (request) => new URL(request.url()).pathname === "/api/login",
      );
      await page.click("#start");
      await request;
      await page
        .getByRole("checkbox", { name: "Code execution", exact: true })
        .check();
      await page
        .getByRole("checkbox", { name: "Monospace font", exact: true })
        .uncheck();
      finishLogin();
      await page.waitForURL(/\/interview/);
      const params = new URL(page.url()).searchParams;
      assert.equal(params.get("editorExecution"), "0");
      assert.equal(params.get("editorMonospace"), null);
    } finally {
      finishLogin();
    }
  },
);

/// A lobby whose two fetches are still in flight, and the release that lets
/// them land. Everything between is the pre-history state.
async function heldLobby(page) {
  const release = holdHistory();
  await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
  await reaches(
    page,
    () =>
      document.querySelector("#account-status").textContent !==
      "Checking account...",
  );
  return release;
}

/// The lobby has chosen: `settle` is what enables the button, so this is the
/// one condition that means every derived thing on the page is in place.
const awaitReady = (page) =>
  reaches(page, () => !document.querySelector("#start").disabled);

const restore = (page) =>
  page.evaluate(() =>
    window.dispatchEvent(
      new PageTransitionEvent("pageshow", { persisted: true }),
    ),
  );

async function openTopicFilter(page) {
  await page.click("details.problem-picker summary");
  await page.waitForFunction(
    () => !document.querySelector("#problem-topic").disabled,
  );
}

const practiceInput = (page, id) =>
  page.locator(`[data-practice-problem="${id}"]`);

lobbyTest(
  "practice checkboxes preserve the current problem and remove the last saved item",
  async (page) => {
    await lobby(page);
    await openTopicFilter(page);
    const before = await snapshot(page);
    const pick = page.getByRole("button", {
      name: "Pick from my list",
      exact: true,
    });
    assert.equal(await pick.isDisabled(), true);
    assert.equal(
      await pick.evaluate((button) => getComputedStyle(button).cursor),
      "not-allowed",
    );
    assert.match(
      await page.locator("#practice-list-status").textContent(),
      /Open the problem picker/,
    );
    const problemTitle = await page
      .locator(`[data-problem="${MEDIUM[0]}"] .problem-title`)
      .textContent();
    const checkbox = page.getByRole("checkbox", {
      name: `In my list: ${problemTitle}`,
      exact: true,
    });
    assert.equal(await checkbox.count(), 1);
    assert.equal(await checkbox.locator("xpath=ancestor::button").count(), 0);
    await checkbox.check();
    assert.deepEqual(await snapshot(page), before);
    assert.equal(await pick.isEnabled(), true);
    assert.equal(
      await pick.evaluate((button) => getComputedStyle(button).cursor),
      "pointer",
    );
    assert.match(
      await page.locator("#practice-list-status").textContent(),
      /1 problem in your list/,
    );
    await checkbox.focus();
    await page.keyboard.press("Space");
    assert.equal(await checkbox.isChecked(), false);
    assert.deepEqual(await snapshot(page), before);
    assert.equal(await pick.isDisabled(), true);
    assert.deepEqual(
      await page.evaluate(
        (key) => JSON.parse(localStorage.getItem(key)),
        PRACTICE_LIST_KEY,
      ),
      [],
    );
  },
);

lobbyTest(
  "practice list additions and removals survive a lobby reload",
  async (page) => {
    await lobby(page);
    await openTopicFilter(page);
    await practiceInput(page, MEDIUM[0]).check();
    await practiceInput(page, MEDIUM[1]).check();
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () => !document.querySelector("#random-problem").disabled,
    );
    assert.equal(await practiceInput(page, MEDIUM[0]).isChecked(), true);
    assert.equal(await practiceInput(page, MEDIUM[1]).isChecked(), true);
    await openTopicFilter(page);
    await practiceInput(page, MEDIUM[0]).uncheck();
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () => !document.querySelector("#random-problem").disabled,
    );
    assert.equal(await practiceInput(page, MEDIUM[0]).isChecked(), false);
    assert.equal(await practiceInput(page, MEDIUM[1]).isChecked(), true);
    assert.deepEqual(
      await page.evaluate(
        (key) => JSON.parse(localStorage.getItem(key)),
        PRACTICE_LIST_KEY,
      ),
      [MEDIUM[1]],
    );
  },
);

lobbyTest(
  "a saved practice list can be drawn with the problem picker closed",
  async (page) => {
    await page.addInitScript(
      ([key, id]) => localStorage.setItem(key, JSON.stringify([id])),
      [PRACTICE_LIST_KEY, MEDIUM[0]],
    );
    await lobby(page);
    assert.equal(
      await page
        .locator("details.problem-picker")
        .evaluate((picker) => picker.open),
      false,
    );
    assert.equal(await page.locator("#practice-list-pick").isVisible(), true);
    assert.equal(await page.locator("#practice-list-status").isVisible(), true);
    await page.click("#practice-list-pick");
    assert.equal((await snapshot(page)).card, MEDIUM[0]);
    await page.reload({ waitUntil: "domcontentloaded" });
    await awaitReady(page);
    assert.equal(
      await page
        .locator("details.problem-picker")
        .evaluate((picker) => picker.open),
      false,
    );
    assert.equal(await page.locator("#practice-list-pick").isVisible(), true);
    for (const width of [320, 375, 1280]) {
      await page.setViewportSize({ width, height: 800 });
      assert.equal(await page.locator("#practice-list-pick").isVisible(), true);
      assert.deepEqual(
        await page.evaluate(() => [
          document.documentElement.scrollWidth,
          document.documentElement.clientWidth,
        ]),
        [width, width],
      );
    }
  },
);

lobbyTest(
  "emptying a practice list in another tab preserves keyboard focus",
  async (page) => {
    const other = await page.context().newPage();
    try {
      await lobby(page);
      await openTopicFilter(page);
      await practiceInput(page, MEDIUM[0]).check();
      await page.click("details.problem-picker summary");
      await lobby(other);
      await openTopicFilter(other);
      await page.locator("#practice-list-pick").focus();
      assert.equal(
        await page.evaluate(() => document.activeElement.id),
        "practice-list-pick",
      );
      await practiceInput(other, MEDIUM[0]).uncheck();
      await reaches(
        page,
        () => document.querySelector("#practice-list-pick").disabled,
      );
      assert.equal(
        await page.evaluate(() => document.activeElement.id),
        "practice-list-status",
      );
      assert.equal(
        await page.locator("#practice-list-status").isVisible(),
        true,
      );
      assert.match(
        await page.locator("#practice-list-status").textContent(),
        /0 problems/,
      );
      await page.keyboard.press("Tab");
      assert.equal(
        await page.evaluate(() => document.activeElement.tagName),
        "SUMMARY",
      );
    } finally {
      await other.close();
    }
  },
);

for (const filter of ["difficulty", "topic"]) {
  lobbyTest(
    `a practice draw follows explicit ${filter} changes without changing the saved list`,
    async (page) => {
      const id = filter === "difficulty" ? EASY[1] : MEDIUM[0];
      await page.addInitScript(
        ([key, id]) => localStorage.setItem(key, JSON.stringify([id])),
        [PRACTICE_LIST_KEY, id],
      );
      await lobby(page);
      await openTopicFilter(page);
      await page.click('[data-duration="60"]');
      await page.click("#practice-list-pick");
      assert.equal((await snapshot(page)).card, id);
      if (filter === "difficulty") await setLevel(page, "Easy", true);
      else await page.selectOption("#problem-topic", "Array");
      assert.equal((await snapshot(page)).card, id);
      if (filter === "difficulty") await setLevel(page, "Easy", false);
      else await page.selectOption("#problem-topic", "Stack");
      const filtered = await snapshot(page);
      assert.notEqual(filtered.card, id);
      assert.equal((await cardInfo(page, filtered.card)).level, "Medium");
      if (filter === "topic")
        assert.equal(TOPICS_BY_PAGE[filtered.card].includes("Stack"), true);
      assert.equal(await practiceInput(page, id).isHidden(), true);
      assert.equal(await practiceInput(page, id).isChecked(), true);
      assert.equal(filtered.duration, "60");
      assert.deepEqual(
        await page.evaluate(
          (key) => JSON.parse(localStorage.getItem(key)),
          PRACTICE_LIST_KEY,
        ),
        [id],
      );
      await page.click("#practice-list-pick");
      const drawn = await snapshot(page);
      assert.equal(drawn.card, id);
      assert.deepEqual(drawn.levels, filtered.levels);
      assert.equal(drawn.duration, "60");
      assert.equal(
        await page.locator("#problem-topic").inputValue(),
        filter === "topic" ? "Stack" : "",
      );
    },
  );
}

lobbyTest("practice lists stay in sync across lobby tabs", async (page) => {
  const other = await page.context().newPage();
  try {
    await lobby(page);
    await lobby(other);
    await openTopicFilter(page);
    await openTopicFilter(other);
    const before = await snapshot(other);
    await practiceInput(page, MEDIUM[0]).check();
    await reaches(other, () =>
      document
        .querySelector("#practice-list-status")
        .textContent.startsWith("1 problem"),
    );
    assert.equal(await practiceInput(other, MEDIUM[0]).isChecked(), true);
    assert.deepEqual(await snapshot(other), before);
    assert.equal(await other.locator("#practice-list-pick").isEnabled(), true);
    await practiceInput(other, MEDIUM[1]).check();
    await reaches(page, () =>
      document
        .querySelector("#practice-list-status")
        .textContent.startsWith("2 problems"),
    );
    assert.equal(await practiceInput(page, MEDIUM[1]).isChecked(), true);
    assert.deepEqual(
      await page.evaluate(
        (key) => JSON.parse(localStorage.getItem(key)),
        PRACTICE_LIST_KEY,
      ),
      MEDIUM,
    );
    await practiceInput(page, MEDIUM[0]).uncheck();
    await reaches(other, () =>
      document
        .querySelector("#practice-list-status")
        .textContent.startsWith("1 problem"),
    );
    assert.equal(await practiceInput(other, MEDIUM[0]).isChecked(), false);
    await other.click("#practice-list-pick");
    assert.equal((await snapshot(other)).card, MEDIUM[1]);
    for (const clearAll of [false, true]) {
      await page.evaluate(
        ([key, clearAll]) => {
          if (clearAll) localStorage.clear();
          else localStorage.removeItem(key);
        },
        [PRACTICE_LIST_KEY, clearAll],
      );
      await reaches(
        other,
        () => document.querySelector("#practice-list-pick").disabled,
      );
      assert.equal(
        await other.locator("[data-practice-problem]:checked").count(),
        0,
      );
      if (!clearAll) await practiceInput(other, MEDIUM[1]).check();
    }
  } finally {
    await other.close();
  }
});

lobbyTest(
  "a practice edit merges stored changes before their notification arrives",
  async (page) => {
    await page.addInitScript(() => {
      window.addEventListener(
        "storage",
        (event) => event.stopImmediatePropagation(),
        true,
      );
    });
    const other = await page.context().newPage();
    try {
      await lobby(page);
      await lobby(other);
      await openTopicFilter(page);
      await openTopicFilter(other);
      await practiceInput(other, MEDIUM[0]).check();
      assert.equal(await practiceInput(page, MEDIUM[0]).isChecked(), false);
      await practiceInput(page, MEDIUM[1]).check();
      assert.equal(await practiceInput(page, MEDIUM[0]).isChecked(), true);
      assert.deepEqual(
        await page.evaluate(
          (key) => JSON.parse(localStorage.getItem(key)),
          PRACTICE_LIST_KEY,
        ),
        MEDIUM,
      );
      await practiceInput(other, MEDIUM[0]).uncheck();
      await practiceInput(page, MEDIUM[1]).uncheck();
      assert.deepEqual(
        await page.evaluate(
          (key) => JSON.parse(localStorage.getItem(key)),
          PRACTICE_LIST_KEY,
        ),
        [],
      );
      assert.equal(
        await page.locator("#practice-list-pick").isDisabled(),
        true,
      );
      assert.equal(await practiceInput(page, MEDIUM[0]).isChecked(), false);
      await practiceInput(page, MEDIUM[0]).check();
      await other.evaluate(
        (key) => localStorage.removeItem(key),
        PRACTICE_LIST_KEY,
      );
      await practiceInput(page, MEDIUM[1]).check();
      assert.deepEqual(
        await page.evaluate(
          (key) => JSON.parse(localStorage.getItem(key)),
          PRACTICE_LIST_KEY,
        ),
        [MEDIUM[1]],
      );
      assert.equal(await practiceInput(page, MEDIUM[0]).isChecked(), false);
    } finally {
      await other.close();
    }
  },
);

for (const change of ["replace", "empty", "remove", "clear"]) {
  lobbyTest(
    `a practice draw refreshes before a delayed storage notification (${change})`,
    async (page) => {
      await page.addInitScript(() => {
        window.addEventListener(
          "storage",
          (event) => event.stopImmediatePropagation(),
          true,
        );
      });
      const other = await page.context().newPage();
      try {
        await lobby(page);
        await openTopicFilter(page);
        await practiceInput(page, MEDIUM[0]).check();
        const before = await snapshot(page);
        await lobby(other);
        await other.evaluate(
          ([key, id, change]) => {
            if (change === "clear") localStorage.clear();
            else if (change === "remove") localStorage.removeItem(key);
            else
              localStorage.setItem(
                key,
                JSON.stringify(change === "replace" ? [id] : []),
              );
          },
          [PRACTICE_LIST_KEY, MEDIUM[1], change],
        );
        // The write is visible in storage, while this tab has no notification.
        assert.equal(await practiceInput(page, MEDIUM[0]).isChecked(), true);
        assert.equal(await practiceInput(page, MEDIUM[1]).isChecked(), false);
        assert.equal(
          await page.locator("#practice-list-pick").isEnabled(),
          true,
        );
        await page.click("#practice-list-pick");
        if (change === "replace") {
          assert.equal((await snapshot(page)).card, MEDIUM[1]);
          assert.equal(await practiceInput(page, MEDIUM[1]).isChecked(), true);
          assert.match(
            await page.locator("#practice-list-status").textContent(),
            /^1 problem in your list/,
          );
        } else {
          assert.deepEqual(await snapshot(page), before);
          assert.equal(
            await page.locator("#practice-list-pick").isDisabled(),
            true,
          );
          assert.equal(
            await page.locator("[data-practice-problem]:checked").count(),
            0,
          );
          assert.match(
            await page.locator("#practice-list-status").textContent(),
            /^0 problems in your list/,
          );
          assert.equal(
            await page.evaluate(() => document.activeElement.id),
            "practice-list-status",
          );
        }
        assert.equal(await practiceInput(page, MEDIUM[0]).isChecked(), false);
      } finally {
        await other.close();
      }
    },
  );
}

lobbyTest(
  "practice draws reveal only the selected exception without rewriting filters or duration",
  async (page) => {
    await page.addInitScript(
      ([key, ids]) => localStorage.setItem(key, JSON.stringify(ids)),
      [PRACTICE_LIST_KEY, [EASY[1], HARD[0]]],
    );
    await lobby(page);
    await openTopicFilter(page);
    await page.selectOption("#problem-topic", "Array");
    await page.click('[data-duration="60"]');
    const before = await snapshot(page);
    await page.evaluate(() => {
      Math.random = () => 0;
    });
    await page.click("#practice-list-pick");
    const first = await snapshot(page);
    assert.equal(first.card, EASY[1]);
    assert.equal(first.pressed, EASY[1]);
    assert.deepEqual(first.levels, before.levels);
    assert.equal(first.duration, "60");
    assert.equal(await page.locator("#problem-topic").inputValue(), "Array");
    assert.match(first.note, /From my list:.*\(Easy\)/);
    assert.equal(await practiceInput(page, EASY[1]).isVisible(), true);
    assert.equal(await practiceInput(page, HARD[0]).isVisible(), false);
    await page.click("#practice-list-pick");
    assert.equal((await snapshot(page)).card, HARD[0]);
    assert.equal(await practiceInput(page, EASY[1]).isVisible(), false);
    assert.equal(await practiceInput(page, HARD[0]).isVisible(), true);
    await restore(page);
    await page.waitForFunction(
      () => !document.querySelector("#practice-list-pick").disabled,
    );
    assert.equal((await snapshot(page)).card, HARD[0]);
    assert.equal(await practiceInput(page, HARD[0]).isVisible(), true);
    await page.click(`[data-problem="${MEDIUM[0]}"]`);
    assert.equal((await snapshot(page)).card, MEDIUM[0]);
    await page.click("#random-problem");
    const general = await snapshot(page);
    assert.equal([EASY[1], HARD[0]].includes(general.card), false);
    assert.deepEqual(general.levels, before.levels);
    assert.equal(general.duration, "60");
  },
);

lobbyTest(
  "a sole saved problem can be redrawn and supplies its suggested duration",
  async (page) => {
    await page.addInitScript(
      ([key, id]) => localStorage.setItem(key, JSON.stringify([id])),
      [PRACTICE_LIST_KEY, EASY[0]],
    );
    await lobby(page);
    await openTopicFilter(page);
    for (let draw = 0; draw < 2; draw++) {
      await page.click("#practice-list-pick");
      const picked = await snapshot(page);
      assert.equal(picked.card, EASY[0]);
      assert.equal(picked.duration, "30");
      assert.deepEqual(picked.levels, ["Medium"]);
    }
    await page.click("#start");
    await page.waitForURL("**/interview?*");
    const params = new URL(page.url()).searchParams;
    assert.equal(params.get("problem"), EASY[0]);
    assert.equal(params.get("duration"), "30");
    assert.equal(
      [...params.keys()].some((key) => /practice|list/i.test(key)),
      false,
    );
  },
);

lobbyTest(
  "practice draws wait for history and account refreshes",
  async (page) => {
    await page.addInitScript(
      ([key, id]) => localStorage.setItem(key, JSON.stringify([id])),
      [PRACTICE_LIST_KEY, MEDIUM[0]],
    );
    let release = holdHistory();
    try {
      await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
      await page.waitForFunction(() =>
        document.querySelector("[data-problem]").hasAttribute("aria-pressed"),
      );
      assert.equal(
        await page.locator("#practice-list-pick").isDisabled(),
        true,
      );
      release();
      await page.waitForFunction(
        () => !document.querySelector("#practice-list-pick").disabled,
      );
      release = holdHistory();
      await restore(page);
      assert.equal(
        await page.locator("#practice-list-pick").isDisabled(),
        true,
      );
      release();
      await page.waitForFunction(
        () => !document.querySelector("#practice-list-pick").disabled,
      );
    } finally {
      release();
    }
  },
);

lobbyTest(
  "malformed and retired saved IDs do not enable a practice draw",
  async (page) => {
    for (const stored of ["{", "{}", '["retired",null,42]']) {
      await page.addInitScript(
        ([key, stored]) => localStorage.setItem(key, stored),
        [PRACTICE_LIST_KEY, stored],
      );
      await lobby(page);
      assert.equal(
        await page.locator("#practice-list-pick").isDisabled(),
        true,
      );
      assert.equal(
        await page.locator("[data-practice-problem]:checked").count(),
        0,
      );
    }
  },
);

for (const failure of ["throw", "ignore"]) {
  lobbyTest(
    `a practice list warns when writes ${failure} on its last removal before reload`,
    async (page) => {
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.addInitScript(
        ([key, failure]) => {
          const setItem = Storage.prototype.setItem;
          window.failPracticeWrites = false;
          Storage.prototype.setItem = function (storedKey, value) {
            if (storedKey === key && window.failPracticeWrites) {
              if (failure === "throw") throw new Error("quota exceeded");
              return;
            }
            return setItem.call(this, storedKey, value);
          };
        },
        [PRACTICE_LIST_KEY, failure],
      );
      await lobby(page);
      await openTopicFilter(page);
      await practiceInput(page, MEDIUM[0]).check();
      await page.evaluate(() => {
        window.failPracticeWrites = true;
      });
      await practiceInput(page, MEDIUM[0]).uncheck();
      assert.equal(await practiceInput(page, MEDIUM[0]).isChecked(), false);
      assert.equal(
        await page.locator("#practice-list-pick").isDisabled(),
        true,
      );
      assert.deepEqual(
        await page.evaluate(
          (key) => JSON.parse(localStorage.getItem(key)),
          PRACTICE_LIST_KEY,
        ),
        [MEDIUM[0]],
      );
      assert.match(
        await page.locator("#practice-list-status").textContent(),
        /^0 problems in your list.*Kept for this visit only/,
      );
      await page.reload({ waitUntil: "domcontentloaded" });
      await awaitReady(page);
      assert.equal(await practiceInput(page, MEDIUM[0]).isChecked(), true);
      assert.equal(await page.locator("#practice-list-pick").isEnabled(), true);
      assert.match(
        await page.locator("#practice-list-status").textContent(),
        /^1 problem in your list.*Saved in this browser only/,
      );
      assert.deepEqual(errors, []);
    },
  );

  lobbyTest(
    `a practice list preserves unsaved changes when writes ${failure} but reads work`,
    async (page) => {
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.addInitScript(
        ([key, id, failure]) => {
          localStorage.setItem(key, JSON.stringify([id]));
          const setItem = Storage.prototype.setItem;
          window.allowPracticeWrites = false;
          Storage.prototype.setItem = function (storedKey, value) {
            if (storedKey === key && !window.allowPracticeWrites) {
              if (failure === "throw") throw new Error("quota exceeded");
              return;
            }
            return setItem.call(this, storedKey, value);
          };
        },
        [PRACTICE_LIST_KEY, MEDIUM[0], failure],
      );
      await lobby(page);
      await openTopicFilter(page);
      await practiceInput(page, MEDIUM[1]).check();
      await practiceInput(page, MEDIUM[0]).uncheck();
      assert.equal(await practiceInput(page, MEDIUM[1]).isChecked(), true);
      assert.equal(await practiceInput(page, MEDIUM[0]).isChecked(), false);
      assert.match(
        await page.locator("#practice-list-status").textContent(),
        /1 problem in your list.*Kept for this visit only/,
      );
      assert.deepEqual(
        await page.evaluate(
          (key) => JSON.parse(localStorage.getItem(key)),
          PRACTICE_LIST_KEY,
        ),
        [MEDIUM[0]],
      );
      await page.click("#practice-list-pick");
      assert.equal((await snapshot(page)).card, MEDIUM[1]);
      const other = await page.context().newPage();
      try {
        await lobby(other);
        await page.evaluate((key) => {
          window.practiceStorageChanged = false;
          window.addEventListener("storage", (event) => {
            if (event.key === key) window.practiceStorageChanged = true;
          });
        }, PRACTICE_LIST_KEY);
        await other.evaluate(
          (key) => localStorage.setItem(key, "[]"),
          PRACTICE_LIST_KEY,
        );
        await reaches(page, () => window.practiceStorageChanged);
        assert.equal(await practiceInput(page, MEDIUM[1]).isChecked(), true);
        assert.equal(await practiceInput(page, MEDIUM[0]).isChecked(), false);
        assert.equal(
          await page.locator("#practice-list-pick").isEnabled(),
          true,
        );
        assert.match(
          await page.locator("#practice-list-status").textContent(),
          /Kept for this visit only/,
        );
        await page.evaluate(() => {
          window.allowPracticeWrites = true;
        });
        await practiceInput(page, MEDIUM[0]).check();
        assert.deepEqual(
          await page.evaluate(
            (key) => JSON.parse(localStorage.getItem(key)),
            PRACTICE_LIST_KEY,
          ),
          [MEDIUM[1], MEDIUM[0]],
        );
        assert.match(
          await page.locator("#practice-list-status").textContent(),
          /Saved in this browser only/,
        );
        await other.evaluate(
          (key) => localStorage.setItem(key, "[]"),
          PRACTICE_LIST_KEY,
        );
        await reaches(
          page,
          () => document.querySelector("#practice-list-pick").disabled,
        );
        assert.equal(
          await page.locator("[data-practice-problem]:checked").count(),
          0,
        );
        assert.deepEqual(errors, []);
      } finally {
        await other.close();
      }
    },
  );
}

for (const failure of ["access", "read"]) {
  lobbyTest(
    `a practice list reports unavailable storage on load after a failed ${failure}`,
    async (page) => {
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.addInitScript(
        ([key, failure]) => {
          if (failure === "access") {
            Object.defineProperty(window, "localStorage", {
              get() {
                throw new Error("blocked");
              },
            });
          } else {
            const getItem = Storage.prototype.getItem;
            Storage.prototype.getItem = function (storedKey) {
              if (storedKey === key) throw new Error("blocked");
              return getItem.call(this, storedKey);
            };
          }
        },
        [PRACTICE_LIST_KEY, failure],
      );
      await lobby(page);
      assert.match(
        await page.locator("#practice-list-status").textContent(),
        /^0 problems in your list.*Kept for this visit only/,
      );
      assert.equal(
        await page.locator("#practice-list-pick").isDisabled(),
        true,
      );
      assert.deepEqual(errors, []);
    },
  );
}

lobbyTest(
  "a practice list stays usable in memory when browser storage is blocked",
  async (page) => {
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.addInitScript(() =>
      Object.defineProperty(window, "localStorage", {
        get() {
          throw new Error("blocked");
        },
      }),
    );
    await lobby(page);
    await openTopicFilter(page);
    await practiceInput(page, MEDIUM[0]).check();
    await practiceInput(page, MEDIUM[1]).check();
    assert.equal(await practiceInput(page, MEDIUM[0]).isChecked(), true);
    assert.equal(await practiceInput(page, MEDIUM[1]).isChecked(), true);
    await practiceInput(page, MEDIUM[1]).uncheck();
    await page.click("#practice-list-pick");
    assert.equal((await snapshot(page)).card, MEDIUM[0]);
    assert.match(
      await page.locator("#practice-list-status").textContent(),
      /Kept for this visit only/,
    );
    assert.deepEqual(errors, []);
  },
);

/// What the difficulty filter did to one card.
const cardInfo = (page, id) =>
  page.evaluate((problem) => {
    const card = document.querySelector(`[data-problem="${problem}"]`);
    return { level: card.dataset.difficulty, hidden: card.hidden };
  }, id);

lobbyTest(
  "the lobby fits on one screen instead of a wall of problems",
  async (page) => {
    await lobby(page);

    // Measured against the viewport this test actually runs at, not a number
    // beside it. The option was dropped once already when this became a
    // `lobbyTest`, leaving an 800 written here and a 720 on screen.
    const { height } = page.viewportSize();
    // The whole reason the picker collapsed. 150 cards made this about 7500.
    for (const font of ["", "Arial, sans-serif"]) {
      await page.evaluate((font) => {
        document.body.style.fontFamily = font;
      }, font);
      const closed = await page.evaluate(() => document.body.scrollHeight);
      assert.ok(
        closed <= height,
        `the lobby is ${closed}px tall with ${font || "the system font"}, past one ${height}px screen`,
      );

      // And the wall is still reachable, just not in the way.
      await page.click("details.problem-picker summary");
      await page.locator(".problem-grid").waitFor({ state: "visible" });
      const open = await page.evaluate(() => document.body.scrollHeight);
      assert.ok(open > closed, "opening the picker shows nothing");
      await page.click("details.problem-picker summary");
      await page.locator(".problem-grid").waitFor({ state: "hidden" });
    }
  },
  { viewport: { width: 1280, height: 800 } },
);

// The row the test above budgets for, at both ends of it. A fixed column beside
// the button shrank below one word of a title on a phone and pushed the page
// wider than the window; this title overflowed even at 375px.
lobbyTest(
  "the selected problem drops below the button only when it cannot fit beside it",
  async (page) => {
    await lobby(page);
    await page.click("details.problem-picker summary");
    await page.click(
      `[data-problem="${pageOf("find-first-and-last-position-of-element-in-sorted-array")}"]`,
    );
    await page.click("details.problem-picker summary");

    for (const [width, beside] of [
      [320, false],
      [375, false],
      [1280, true],
    ]) {
      await page.setViewportSize({ width, height: 800 });
      const layout = await page.evaluate(() => {
        const button = document
          .querySelector("#random-problem")
          .getBoundingClientRect();
        const line = document
          .querySelector("#recommendation")
          .getBoundingClientRect();
        return {
          scrollWidth: document.documentElement.scrollWidth,
          clientWidth: document.documentElement.clientWidth,
          beside: line.top < button.bottom,
        };
      });
      assert.equal(
        layout.scrollWidth,
        layout.clientWidth,
        `the lobby scrolls sideways at ${width}px`,
      );
      assert.equal(
        layout.beside,
        beside,
        `at ${width}px the line should sit ${beside ? "beside" : "below"} the button`,
      );
    }
  },
);

lobbyTest(
  "topic and difficulty filters narrow the problem cards together",
  async (page) => {
    const requests = [];
    page.on("request", (request) =>
      requests.push(new URL(request.url()).pathname),
    );
    await lobby(page);
    assert.equal(
      requests.includes("/problem-topics.json"),
      false,
      "the initial lobby disclosed the topic map",
    );
    assert.equal(
      await page.locator("[data-topics]").count(),
      0,
      "a problem card disclosed its topics in the HTML",
    );
    assert.equal(
      await page.locator("#problem-filter-summary").textContent(),
      "",
      "the untouched live region announced a count",
    );
    await openTopicFilter(page);
    assert.equal(requests.includes("/problem-topics.json"), true);

    await page.selectOption("#problem-topic", "Array");
    const filtered = await page
      .locator("[data-problem]:visible")
      .evaluateAll((cards) =>
        cards.map((card) => ({
          id: card.dataset.problem,
          difficulty: card.dataset.difficulty,
        })),
      );
    assert.ok(filtered.length > 0, "the topic filter hid every problem");
    assert.ok(
      filtered.every(
        (card) =>
          card.difficulty === "Medium" &&
          TOPICS_BY_PAGE[card.id].includes("Array"),
      ),
      "a visible card did not match both filters",
    );

    await page.click("#problem-filters-reset");
    assert.ok(
      (await page.locator("[data-problem]:visible").count()) > filtered.length,
      "reset did not restore the other Medium problems",
    );
    assert.equal(
      await page.locator("#problem-filter-summary").textContent(),
      "",
    );
  },
);

lobbyTest(
  "title search only narrows visible cards and combines with topic filtering",
  async (page) => {
    await lobby(page);
    await openTopicFilter(page);
    await page.selectOption("#problem-topic", "Array");

    const available = await page
      .locator(".problem-card:visible")
      .evaluateAll((cards) =>
        cards.map((card) => ({
          id: card.dataset.problem,
          title: card.querySelector(".problem-title").textContent.trim(),
        })),
      );
    assert.ok(available.length > 0);
    const original = await snapshot(page);
    const target = available[0];

    await page.fill("#problem-search", target.title.toUpperCase());
    const found = await page
      .locator(".problem-card:visible")
      .evaluateAll((cards) => cards.map((card) => card.dataset.problem));
    assert.deepEqual(found, [target.id]);
    assert.equal((await snapshot(page)).card, original.card);
    assert.match(
      await page.locator("#problem-filter-summary").textContent(),
      /tagged Array and matching/,
    );

    await page.fill("#problem-search", "title_that_does_not_exist_12409");
    assert.equal(await page.locator(".problem-card:visible").count(), 0);
    assert.match(
      await page.locator("#problem-filter-summary").textContent(),
      /0 problems/,
    );

    await page.click("#problem-filters-reset");
    assert.equal(await page.locator("#problem-search").inputValue(), "");
    assert.equal(await page.locator("#problem-topic").inputValue(), "");
    assert.equal(
      await page.locator("#problem-filter-summary").textContent(),
      "",
    );
  },
);

lobbyTest(
  "switching search modes looks up published titles without changing recommendations",
  async (page) => {
    await lobby(page);
    await page.click("details.problem-picker summary");
    await setLevel(page, "Easy", true);
    const chosen = pageOf("two-sum");
    const originalRecommendation = (await snapshot(page)).card;
    const sourceCheckbox = page.locator("#show-sources");
    assert.equal(await sourceCheckbox.isChecked(), false);

    await page.fill("#problem-search", "Two Sum");
    assert.equal(
      await page.locator(`[data-problem="${chosen}"]`).isVisible(),
      false,
    );

    await page
      .locator('[name="problem-search-mode"][value="leetcode"]')
      .check();
    await page.waitForFunction(() =>
      document
        .querySelector("#problem-filter-summary")
        .textContent.includes("matching LeetCode title"),
    );
    assert.equal(
      await page.locator(`[data-problem="${chosen}"]`).isVisible(),
      true,
    );
    assert.equal(await sourceCheckbox.isChecked(), false);
    assert.equal((await snapshot(page)).card, originalRecommendation);

    await page
      .locator('[name="problem-search-mode"][value="scenario"]')
      .check();
    assert.equal(
      await page.locator(`[data-problem="${chosen}"]`).isVisible(),
      false,
    );
    await page.click("#problem-filters-reset");
    assert.equal(
      await page
        .locator('[name="problem-search-mode"][value="scenario"]')
        .isChecked(),
      true,
    );
    assert.equal(await page.locator("#problem-search").inputValue(), "");
  },
);

lobbyTest(
  "search field stays above Topic and leaves room for the focus ring",
  async (page) => {
    await lobby(page);
    await page.click("details.problem-picker summary");
    for (const width of [320, 375, 800]) {
      await page.setViewportSize({ width, height: 800 });
      await page.locator("#problem-search").focus();
      const boxes = await page.evaluate(() => {
        const rect = (selector) =>
          document.querySelector(selector).getBoundingClientRect();
        return {
          headingBottom: rect(".practice-filters legend").bottom,
          inputTop: rect("#problem-search").top,
          inputBottom: rect("#problem-search").bottom,
          topicTop: rect('label[for="problem-topic"]').top,
          scroll: document.documentElement.scrollWidth,
          viewport: document.documentElement.clientWidth,
        };
      });
      assert.ok(
        boxes.inputTop - boxes.headingBottom >= 5,
        `search focus ring overlaps heading at ${width}px`,
      );
      assert.ok(
        boxes.topicTop - boxes.inputBottom >= 5,
        `Topic sits beside or overlaps search at ${width}px`,
      );
      assert.equal(
        boxes.scroll,
        boxes.viewport,
        `horizontal overflow at ${width}px`,
      );
    }
  },
);

lobbyTest(
  "account refresh cannot expose a manual pick hidden by title search",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await lobby(page);
    await openTopicFilter(page);
    const chosen = MEDIUM[0];
    await page.click(`[data-problem="${chosen}"]`);
    await page.fill("#problem-search", "no-title-has-this-unique-phrase-404");
    assert.equal(await page.locator(".problem-card:visible").count(), 0);
    assert.equal((await snapshot(page)).card, chosen);

    await page.fill("#github-login", "candidate");
    await page.click("#login-link");
    await reaches(
      page,
      () =>
        document.querySelector("#account-status").textContent ===
          "Signed in as candidate" &&
        !document.querySelector("#start").disabled,
    );

    assert.equal((await snapshot(page)).card, chosen);
    assert.equal(
      await page.locator(".problem-card:visible").count(),
      0,
      "account refresh must not override active title search",
    );
    assert.match(
      await page.locator("#problem-filter-summary").textContent(),
      /0 problems matching scenario title/,
    );
  },
);

lobbyTest(
  "title search cannot invalidate a manual pick through difficulty or topic filters",
  async (page) => {
    const easyArray = Object.entries(TOPICS_BY_PAGE).find(
      ([pageName, topics]) =>
        topics.includes("Array") &&
        !topics.includes("Tree") &&
        read("web/index.html").includes(
          `data-problem="${pageName}" data-difficulty="Easy"`,
        ),
    )?.[0];
    assert.ok(easyArray, "the problem bank has no Easy Array example");

    await lobby(page);
    await awaitReady(page);
    await openTopicFilter(page);
    await setLevel(page, "Easy", true);
    await page.click(`[data-problem="${easyArray}"]`);
    assert.equal((await snapshot(page)).card, easyArray);

    await page.fill("#problem-search", "no_such_scenario_title_404");
    assert.equal(
      await page.locator(`[data-problem="${easyArray}"]`).isVisible(),
      false,
    );
    assert.equal((await snapshot(page)).card, easyArray);

    await setLevel(page, "Hard", true);
    assert.equal(
      (await snapshot(page)).card,
      easyArray,
      "a difficulty change discarded a pick hidden only by title search",
    );
    await page.selectOption("#problem-topic", "Array");
    assert.equal(
      (await snapshot(page)).card,
      easyArray,
      "a matching topic change discarded a pick hidden only by title search",
    );

    // A genuinely incompatible topic still has to discard the manual pick.
    assert.equal(TOPICS_BY_PAGE[easyArray].includes("Tree"), false);
    await page.selectOption("#problem-topic", "Tree");
    assert.notEqual((await snapshot(page)).card, easyArray);
  },
);

lobbyTest(
  "changing the topic while LeetCode titles load cannot keep an ineligible pick",
  async (page) => {
    let releaseMap;
    const heldMap = new Promise((resolve) => (releaseMap = resolve));
    await page.route("**/problem-pages.json", async (route) => {
      await heldMap;
      await route.continue();
    });

    try {
      const easyArray = Object.entries(TOPICS_BY_PAGE).find(
        ([pageName, topics]) =>
          topics.includes("Array") &&
          !topics.includes("Tree") &&
          read("web/index.html").includes(
            `data-problem="${pageName}" data-difficulty="Easy"`,
          ),
      )?.[0];
      assert.ok(easyArray, "the bank has no Easy Array-only example");

      await lobby(page);
      await awaitReady(page);
      await openTopicFilter(page);
      await setLevel(page, "Easy", true);
      await page.click(`[data-problem="${easyArray}"]`);
      const scenarioTitle = (
        await page
          .locator(`[data-problem="${easyArray}"] .problem-title`)
          .textContent()
      ).trim();
      await page.fill("#problem-search", scenarioTitle);
      assert.equal(
        await page.locator(`[data-problem="${easyArray}"]`).isVisible(),
        true,
      );

      await page
        .locator('[name="problem-search-mode"][value="leetcode"]')
        .check();
      await page.waitForFunction(
        () =>
          document.querySelector("#problem-filter-summary").textContent ===
          "Loading LeetCode titles...",
      );

      // The grid is hidden, and its card.hidden flags have not been refreshed.
      // A filter change must check topic eligibility, not those stale flags.
      await page.selectOption("#problem-topic", "Tree");
      assert.notEqual(
        (await snapshot(page)).card,
        easyArray,
        "an old Array selection survived a Tree filter during map loading",
      );

      releaseMap();
      await page.waitForFunction(() =>
        document
          .querySelector("#problem-filter-summary")
          .textContent.includes("matching LeetCode title"),
      );
      const state = await snapshot(page);
      assert.notEqual(state.card, easyArray);
      assert.ok(state.card, "the Tree filter found no replacement problem");
      assert.equal(TOPICS_BY_PAGE[state.card].includes("Tree"), true);
      await page.click("#start");
      await page.waitForURL(/\/interview/);
      assert.equal(new URL(page.url()).searchParams.get("problem"), state.card);
    } finally {
      releaseMap();
    }
  },
);

lobbyTest(
  "published-title search hides stale scenario cards while the map loads",
  async (page) => {
    let releaseMap;
    const heldMap = new Promise((resolve) => (releaseMap = resolve));
    await page.route("**/problem-pages.json", async (route) => {
      await heldMap;
      await route.continue();
    });
    try {
      await lobby(page);
      await page.click("details.problem-picker summary");
      const originalRecommendation = (await snapshot(page)).card;
      const scenarioTitle = (
        await page
          .locator(".problem-card:visible .problem-title")
          .first()
          .textContent()
      ).trim();
      await page.fill("#problem-search", scenarioTitle);
      assert.equal(await page.locator(".problem-card:visible").count(), 1);

      await page
        .locator('[name="problem-search-mode"][value="leetcode"]')
        .check();
      await page.waitForFunction(
        () =>
          document.querySelector("#problem-filter-summary").textContent ===
          "Loading LeetCode titles...",
      );
      assert.equal(
        await page.locator(".problem-card:visible").count(),
        0,
        "scenario matches remain visible while LeetCode titles load",
      );
      assert.equal((await snapshot(page)).card, originalRecommendation);

      releaseMap();
      await page.waitForFunction(() =>
        document
          .querySelector("#problem-filter-summary")
          .textContent.includes("matching LeetCode title"),
      );
      assert.equal(
        await page.locator(".problem-grid").evaluate((grid) => !grid.hidden),
        true,
        "successful title lookup did not restore the picker",
      );
      assert.equal((await snapshot(page)).card, originalRecommendation);
    } finally {
      releaseMap();
    }
  },
);

lobbyTest(
  "published-title search failure hides stale cards and leaves scenario search usable",
  async (page) => {
    await page.route("**/problem-pages.json", (route) =>
      route.fulfill({ status: 503, body: "unavailable" }),
    );
    await lobby(page);
    await page.click("details.problem-picker summary");
    const originalRecommendation = (await snapshot(page)).card;
    const scenarioTitle = (
      await page
        .locator(".problem-card:visible .problem-title")
        .first()
        .textContent()
    ).trim();
    await page.fill("#problem-search", scenarioTitle);
    assert.equal(await page.locator(".problem-card:visible").count(), 1);

    await page
      .locator('[name="problem-search-mode"][value="leetcode"]')
      .check();
    await page.waitForFunction(() =>
      document
        .querySelector("#problem-filter-summary")
        .textContent.includes("could not be loaded"),
    );
    assert.equal(
      await page.locator(".problem-card:visible").count(),
      0,
      "failed LeetCode lookup still shows scenario search results",
    );
    assert.equal((await snapshot(page)).card, originalRecommendation);

    await page
      .locator('[name="problem-search-mode"][value="scenario"]')
      .check();
    assert.equal(await page.locator(".problem-card:visible").count(), 1);
    assert.equal((await snapshot(page)).card, originalRecommendation);
  },
);

lobbyTest(
  "returning to LeetCode search during a pending lookup hides stale scenario cards",
  async (page) => {
    let releaseMap;
    const heldMap = new Promise((resolve) => (releaseMap = resolve));
    await page.route("**/problem-pages.json", async (route) => {
      await heldMap;
      await route.continue();
    });
    try {
      await lobby(page);
      await page.click("details.problem-picker summary");
      const scenarioTitle = (
        await page
          .locator(".problem-card:visible .problem-title")
          .first()
          .textContent()
      ).trim();
      await page.fill("#problem-search", scenarioTitle);
      assert.equal(await page.locator(".problem-card:visible").count(), 1);

      const leetcode = page.locator(
        '[name="problem-search-mode"][value="leetcode"]',
      );
      const scenario = page.locator(
        '[name="problem-search-mode"][value="scenario"]',
      );
      await leetcode.check();
      await page.waitForFunction(
        () =>
          document.querySelector("#problem-filter-summary").textContent ===
          "Loading LeetCode titles...",
      );
      assert.equal(await page.locator(".problem-card:visible").count(), 0);

      await scenario.check();
      assert.equal(await page.locator(".problem-card:visible").count(), 1);
      await leetcode.check();
      assert.equal(
        await page.locator(".problem-card:visible").count(),
        0,
        "a pending lookup left old scenario search results visible",
      );
      assert.equal(
        await page.locator(".problem-grid").isVisible(),
        false,
        "a pending title lookup must hide the entire problem grid",
      );
      releaseMap();
      await page.waitForFunction(() =>
        document
          .querySelector("#problem-filter-summary")
          .textContent.includes("matching LeetCode title"),
      );
    } finally {
      releaseMap();
    }
  },
);

lobbyTest(
  "a missing published-title map is reported as unavailable rather than zero matches",
  async (page) => {
    await page.route("**/problem-pages.json", (route) =>
      route.fulfill({ status: 404, body: "not found" }),
    );
    await lobby(page);
    await page.click("details.problem-picker summary");
    await page.fill("#problem-search", "Two Sum");
    await page
      .locator('[name="problem-search-mode"][value="leetcode"]')
      .check();
    await page.waitForFunction(() => {
      const status = document.querySelector(
        "#problem-filter-summary",
      ).textContent;
      return (
        status.includes("matching LeetCode title") ||
        status.includes("could not be loaded")
      );
    });
    assert.match(
      await page.locator("#problem-filter-summary").textContent(),
      /could not be loaded/,
      "a missing title map should not be presented as a valid empty result",
    );
    assert.equal(await page.locator(".problem-card:visible").count(), 0);
  },
);

lobbyTest(
  "a failed topic lookup is retried when the picker reopens",
  async (page) => {
    let requests = 0;
    await page.route("**/problem-topics.json", async (route) => {
      requests += 1;
      // The data loader already retries one dropped request. Fail both of its
      // bounded attempts so reopening the picker has to start a new load.
      if (requests <= 2) {
        await route.fulfill({ status: 500, body: "nope" });
        return;
      }
      await route.continue();
    });
    await lobby(page);

    await page.click("details.problem-picker summary");
    await page.waitForFunction(() =>
      document
        .querySelector("#problem-filter-summary")
        .textContent.includes("could not be loaded"),
    );
    assert.equal(await page.locator("#problem-topic").isDisabled(), true);

    await page.click("details.problem-picker summary");
    await openTopicFilter(page);
    assert.equal(requests, 3);
    assert.ok((await page.locator("#problem-topic option").count()) > 1);
    assert.equal(
      await page.locator("#problem-filter-summary").textContent(),
      "",
    );
  },
);

lobbyTest("a topic choice does not survive a lobby restore", async (page) => {
  await lobby(page);
  await openTopicFilter(page);
  await page.selectOption("#problem-topic", "Array");
  await page.fill("#problem-search", "Array");
  assert.match(
    await page.locator("#problem-filter-summary").textContent(),
    /tagged Array/,
  );

  await restore(page);
  await awaitReady(page);
  assert.equal(await page.locator("#problem-topic").inputValue(), "");
  assert.equal(await page.locator("#problem-search").inputValue(), "");
  assert.equal(await page.locator("#problem-filter-summary").textContent(), "");
});

lobbyTest(
  "a hidden manual pick does not survive the combined filters",
  async (page) => {
    const hardArray = Object.entries(TOPICS_BY_PAGE).find(
      ([pageName, topics]) =>
        topics.includes("Array") &&
        read("web/index.html").includes(
          `data-problem="${pageName}" data-difficulty="Hard"`,
        ),
    )[0];
    await lobby(page);
    await openTopicFilter(page);
    await page.evaluate((id) => {
      const card = document.querySelector(`[data-problem="${id}"]`);
      card.hidden = false;
      card.click();
    }, hardArray);

    await page.selectOption("#problem-topic", "Array");
    const state = await snapshot(page);
    assert.notEqual(state.card, hardArray);
    assert.equal((await cardInfo(page, hardArray)).hidden, true);
    assert.equal((await cardInfo(page, state.card)).hidden, false);
  },
);

lobbyTest(
  "a due review does not appear in zero title-search results after a topic change",
  async (page) => {
    const manual = pageOf("two-sum");
    const due = pageOf("valid-parentheses");
    assert.equal(TOPICS_BY_PAGE[manual].includes("Array"), true);
    assert.equal(TOPICS_BY_PAGE[manual].includes("Binary Tree"), false);
    assert.equal(TOPICS_BY_PAGE[due].includes("Binary Tree"), false);

    reports = [savedAttempt(due)];
    await lobby(page);
    await awaitReady(page);
    await openTopicFilter(page);
    await setLevel(page, "Easy", true);
    await page.click(`[data-problem="${manual}"]`);
    assert.equal((await snapshot(page)).card, manual);

    await page.fill("#problem-search", "zzzzzz-no-match");
    assert.equal(await page.locator(".problem-card:visible").count(), 0);
    await page.selectOption("#problem-topic", "Array");
    assert.equal(
      (await snapshot(page)).card,
      manual,
      "a matching topic change discarded the hand-picked problem",
    );

    // Exclude the manual pick: the due-review recommendation is allowed to
    // take priority, but its card cannot bypass the active title query.
    await page.selectOption("#problem-topic", "Binary Tree");
    assert.equal((await snapshot(page)).card, due);
    assert.match((await snapshot(page)).note, /Review due after/);
    assert.equal(
      await page.locator(".problem-card:visible").count(),
      0,
      "a due review escaped a title search with zero results",
    );
    assert.equal(
      await page.locator(`[data-problem="${due}"]`).isVisible(),
      false,
    );
    assert.match(
      await page.locator("#problem-filter-summary").textContent(),
      /0 problems tagged Binary Tree and matching scenario title/,
    );
  },
);

lobbyTest(
  "opening the picker preserves a due review outside the selected difficulty",
  async (page) => {
    const due = pageOf("valid-parentheses");
    reports = [savedAttempt(due)];
    await lobby(page);
    await awaitReady(page);
    await setLevel(page, "Hard", true);
    await setLevel(page, "Medium", false);
    assert.deepEqual((await snapshot(page)).levels, ["Hard"]);
    assert.equal((await snapshot(page)).card, due);
    assert.match((await snapshot(page)).note, /Review due after/);
    assert.equal((await cardInfo(page, due)).level, "Easy");
    assert.equal((await cardInfo(page, due)).hidden, false);

    // Finishing the topic-map load must not hide the active review exception.
    await openTopicFilter(page);
    assert.equal((await snapshot(page)).card, due);
    assert.equal((await cardInfo(page, due)).hidden, false);
  },
);

lobbyTest(
  "opening the picker preserves a history retry outside the selected difficulty",
  async (page) => {
    reports = [savedAttempt(EASY[0])];
    await lobby(page);
    await awaitReady(page);
    await setLevel(page, "Hard", true);
    await setLevel(page, "Medium", false);
    assert.deepEqual((await snapshot(page)).levels, ["Hard"]);
    await page.getByRole("button", { name: "Try again", exact: true }).click();
    assert.equal((await snapshot(page)).card, EASY[0]);
    assert.equal((await cardInfo(page, EASY[0])).hidden, false);

    await openTopicFilter(page);
    assert.equal((await snapshot(page)).card, EASY[0]);
    assert.equal((await cardInfo(page, EASY[0])).hidden, false);
  },
);

lobbyTest(
  "clearing title search restores an out-of-filter due-review card",
  async (page) => {
    const due = pageOf("valid-parentheses");
    assert.equal(TOPICS_BY_PAGE[due].includes("Array"), false);
    reports = [savedAttempt(due)];
    await lobby(page);
    await openTopicFilter(page);
    await page.selectOption("#problem-topic", "Array");
    assert.equal((await snapshot(page)).card, due);
    assert.equal((await cardInfo(page, due)).hidden, false);

    await page.fill("#problem-search", "no-title-matches-this-query-404");
    assert.equal((await cardInfo(page, due)).hidden, true);
    await page.fill("#problem-search", "");
    assert.equal((await snapshot(page)).card, due);
    assert.equal(
      (await cardInfo(page, due)).hidden,
      false,
      "clearing title search left the due-review card hidden",
    );
    // Clicking the visible review card also must not lose its exception.
    await page.locator(`[data-problem="${due}"]`).click();
    await page.fill("#problem-search", "no-title-matches-this-query-404");
    await page.fill("#problem-search", "");
    assert.equal((await cardInfo(page, due)).hidden, false);
  },
);

lobbyTest(
  "clearing title search restores a retried card outside difficulty",
  async (page) => {
    reports = [savedAttempt(EASY[0])];
    await lobby(page);
    await openTopicFilter(page);
    await page.getByRole("button", { name: "Try again", exact: true }).click();
    assert.equal((await snapshot(page)).card, EASY[0]);
    assert.equal((await cardInfo(page, EASY[0])).hidden, false);

    await page.fill("#problem-search", "no-title-matches-this-query-404");
    assert.equal((await cardInfo(page, EASY[0])).hidden, true);
    await page.fill("#problem-search", "");
    assert.equal((await snapshot(page)).card, EASY[0]);
    assert.equal(
      (await cardInfo(page, EASY[0])).hidden,
      false,
      "clearing title search left the retried card hidden",
    );
  },
);

lobbyTest(
  "a due review outside the selected topic remains the first priority",
  async (page) => {
    const due = pageOf("valid-parentheses");
    assert.equal(TOPICS_BY_PAGE[due].includes("Array"), false);
    reports = [savedAttempt(due)];
    await lobby(page);
    await openTopicFilter(page);
    await page.selectOption("#problem-topic", "Array");

    const state = await snapshot(page);
    assert.equal(state.card, due);
    assert.match(state.note, /Review due after/);
    assert.equal((await cardInfo(page, due)).hidden, false);
  },
);

lobbyTest(
  "a difficulty change keeps an out-of-filter due review visible",
  async (page) => {
    const due = pageOf("valid-parentheses");
    assert.equal(TOPICS_BY_PAGE[due].includes("Array"), false);
    reports = [savedAttempt(due)];
    await lobby(page);
    await openTopicFilter(page);
    await page.selectOption("#problem-topic", "Array");

    await setLevel(page, "Easy", true);
    const state = await snapshot(page);
    assert.equal(state.card, due);
    assert.match(state.note, /Review due after/);
    assert.equal((await cardInfo(page, due)).hidden, false);

    await page.click("#start");
    await page.waitForURL(/\/interview/);
    assert.equal(new URL(page.url()).searchParams.get("problem"), due);
  },
);

lobbyTest(
  "a topic change redraws a kept problem that the topic hides",
  async (page) => {
    await page.addInitScript(() => {
      Math.random = () => 0;
    });
    await lobby(page);
    const drawn = await snapshot(page);

    await setLevel(page, "Easy", true);
    assert.equal((await snapshot(page)).card, drawn.card);
    await openTopicFilter(page);
    const topic = Object.entries(TOPICS_BY_PAGE)
      .filter(([pageName]) => pageName !== drawn.card)
      .flatMap(([, topics]) => topics)
      .find(
        (candidate, index, topics) =>
          topics.indexOf(candidate) === index &&
          !TOPICS_BY_PAGE[drawn.card].includes(candidate) &&
          Object.entries(TOPICS_BY_PAGE).some(
            ([pageName, cardTopics]) =>
              cardTopics.includes(candidate) &&
              ["Easy", "Medium"].includes(
                read("web/index.html").match(
                  new RegExp(
                    `data-problem="${pageName}" data-difficulty="([^"]+)"`,
                  ),
                )?.[1],
              ),
          ),
      );
    assert.ok(topic, "the bank has no topic that excludes the drawn problem");

    await page.selectOption("#problem-topic", topic);
    const redrawn = await snapshot(page);
    assert.notEqual(redrawn.card, drawn.card);
    assert.equal(TOPICS_BY_PAGE[redrawn.card].includes(topic), true);
    assert.equal((await cardInfo(page, redrawn.card)).hidden, false);

    await page.click("#start");
    await page.waitForURL(/\/interview/);
    assert.equal(new URL(page.url()).searchParams.get("problem"), redrawn.card);
  },
);

lobbyTest(
  "a completed topic and level are described without overstating progress",
  async (page) => {
    const markup = read("web/index.html");
    const mediumArray = Object.entries(TOPICS_BY_PAGE)
      .filter(
        ([pageName, topics]) =>
          topics.includes("Array") &&
          markup.includes(
            `data-problem="${pageName}" data-difficulty="Medium"`,
          ),
      )
      .map(([pageName]) => pageName);
    reports = mediumArray.map(hired);
    await lobby(page);
    await setLevel(page, "Medium", true);
    await setLevel(page, "Easy", false);
    await setLevel(page, "Hard", false);
    await openTopicFilter(page);
    await page.selectOption("#problem-topic", "Array");

    assert.match(
      (await snapshot(page)).note,
      /You have passed every problem matching this topic and level\./,
    );
  },
);

lobbyTest(
  "recent assessed reports produce a practice snapshot",
  async (page) => {
    reports = [
      {
        problemId: HARD[0],
        createdAt: 40,
        payload: { report: { incomplete: true, decision: "HIRE" } },
      },
      {
        problemId: EASY[0],
        createdAt: 30,
        payload: { report: { decision: "NO_HIRE" } },
      },
      {
        problemId: EASY[1],
        createdAt: 20,
        payload: { report: { decision: "HIRE" } },
      },
      {
        problemId: MEDIUM[0],
        createdAt: 10,
        payload: { report: { decision: "NO_HIRE" } },
      },
    ];
    await lobby(page);

    assert.equal(
      await page.locator("#recent-performance-summary").textContent(),
      "Last 3 assessed interviews: 1 passed, 2 missed (33%). 3 of these interviews predate versioned scoring.",
    );
    assert.equal(await page.locator("#recent-performance").isHidden(), false);
  },
);

lobbyTest(
  "weak topics show counts, latest dates and sparse evidence before topic opt-in",
  async (page) => {
    reports = [
      missed(EASY[0], {
        createdAt: Date.UTC(2026, 9, 20) / 1000,
        topics: ["Hash Table", "Unknown topic"],
      }),
      missed(EASY[0], {
        createdAt: Date.UTC(2026, 9, 19) / 1000,
        topics: ["Hash Table"],
      }),
      hired(EASY[1], {
        createdAt: Date.UTC(2026, 9, 18) / 1000,
        topics: ["Array"],
      }),
    ];
    const requests = [];
    page.on("request", (request) =>
      requests.push(new URL(request.url()).pathname),
    );
    await lobby(page);
    const topics = page.locator("#recent-weak-topics");
    const date = await page.evaluate(() =>
      new Date(Date.UTC(2026, 9, 20)).toLocaleDateString(undefined, {
        dateStyle: "medium",
      }),
    );
    assert.match(await topics.textContent(), /Hash Table — missed 2 of 2;/);
    assert.ok((await topics.textContent()).includes(`latest ${date}.`));
    assert.equal(await topics.locator(".weak-topic").count(), 2);
    assert.equal(await topics.locator("button").count(), 0);
    assert.equal(requests.includes("/problem-topics.json"), false);
    for (const item of await topics.locator(".weak-topic").all())
      assert.match(
        await item.textContent(),
        /Reminder from few interviews, not a skill measurement/,
      );
    await openTopicFilter(page);
    await page.waitForFunction(() =>
      document.querySelector("#recent-weak-topics button"),
    );
    assert.equal(
      await topics
        .getByRole("button", {
          name: "Filter problems by Hash Table",
          exact: true,
        })
        .count(),
      1,
    );
    assert.equal(
      await topics.getByRole("button", { name: /Unknown topic/ }).count(),
      0,
    );
    assert.match(await topics.textContent(), /Unknown topic — missed 1 of 1;/);
  },
);

lobbyTest(
  "three topic attempts remove the sparse caveat and keyboard activation filters the grid",
  async (page) => {
    const hashTable = { topics: ["Hash Table"] };
    reports = [
      missed(EASY[0], { ...hashTable, createdAt: 30 }),
      missed(EASY[0], { ...hashTable, createdAt: 20 }),
      hired(EASY[0], { ...hashTable, createdAt: 10 }),
    ];
    await lobby(page);
    const topics = page.locator("#recent-weak-topics");
    assert.match(await topics.textContent(), /missed 2 of 3/);
    assert.doesNotMatch(await topics.textContent(), /few interviews/);
    await openTopicFilter(page);
    const { levels } = await snapshot(page);
    await page.click("details.problem-picker summary");
    const button = topics.getByRole("button", {
      name: "Filter problems by Hash Table",
      exact: true,
    });
    await page.evaluate(() => {
      window.topicChanges = 0;
      document
        .querySelector("#problem-topic")
        .addEventListener("change", () => {
          window.topicChanges += 1;
        });
    });
    for (const key of ["Enter", "Space"]) {
      const before = await snapshot(page);
      const alreadySelected =
        (await page.locator("#problem-topic").inputValue()) === "Hash Table";
      await button.focus();
      await page.keyboard.press(key);
      await page.waitForFunction(
        () => document.querySelector("#problem-topic").value === "Hash Table",
      );
      assert.ok(await isFocused(page.locator("#problem-topic")));
      assert.equal(await page.evaluate(() => window.topicChanges), 1);
      const after = await snapshot(page);
      if (alreadySelected) assert.equal(after.card, before.card);
      assert.deepEqual(after.levels, levels);
      assert.equal(
        await page.locator("details.problem-picker").getAttribute("open"),
        "",
      );
      const visible = await page
        .locator("[data-problem]:visible")
        .evaluateAll((nodes) => nodes.map((node) => node.dataset.problem));
      assert.ok(visible.length > 0);
      for (const id of visible)
        assert.ok(TOPICS_BY_PAGE[id].includes("Hash Table"));
    }
  },
);

lobbyTest(
  "refreshing history preserves weak topic focus or moves it to the picker when removed",
  async (page) => {
    reports = [
      missed(EASY[0], { createdAt: 20, topics: ["Hash Table"] }),
      missed(EASY[1], { createdAt: 10, topics: ["Stack"] }),
    ];
    await lobby(page);
    await openTopicFilter(page);
    const button = page.getByRole("button", {
      name: "Filter problems by Hash Table",
      exact: true,
    });
    await button.focus();
    reports.push({ ...reports[0], createdAt: 30 });
    await restore(page);
    await page.waitForFunction(
      () =>
        document.activeElement?.dataset.topic === "Hash Table" &&
        document
          .querySelector("#recent-weak-topics")
          .textContent.includes("missed 2 of 2"),
    );
    assert.ok(await isFocused(button));
    reports.push(
      hired(EASY[0], { createdAt: 40, topics: ["Hash Table"] }),
      hired(EASY[0], { createdAt: 50, topics: ["Hash Table"] }),
    );
    await restore(page);
    await page.waitForFunction(
      () =>
        !document
          .querySelector("#recent-weak-topics")
          .textContent.includes("Hash Table"),
    );
    assert.equal(await page.locator("#recent-performance").isHidden(), false);
    assert.ok(await isFocused(page.locator("details.problem-picker summary")));
    await page
      .getByRole("button", { name: "Filter problems by Stack", exact: true })
      .focus();
    reports = [];
    await restore(page);
    await page.waitForFunction(
      () => document.querySelector("#recent-performance").hidden,
    );
    assert.ok(await isFocused(page.locator("details.problem-picker summary")));
  },
);

lobbyTest(
  "weak topic controls appear only after a failed topic lookup is retried successfully",
  async (page) => {
    reports = [missed(EASY[0], { createdAt: 20, topics: ["Hash Table"] })];
    failing.add("/problem-topics.json");
    await lobby(page);
    const weakTopics = page.locator("#recent-weak-topics");
    assert.match(
      await weakTopics.textContent(),
      /Topic filters load when the specific-problem picker opens\.$/,
    );
    await page.click("details.problem-picker summary");
    await page.waitForFunction(
      () =>
        document.querySelector("#problem-filter-summary").textContent ===
        "Topic filters could not be loaded.",
    );
    assert.match(
      await weakTopics.textContent(),
      /close and reopen the specific-problem picker to retry\.$/,
    );
    assert.equal(await page.locator("#recent-weak-topics button").count(), 0);
    failing.delete("/problem-topics.json");
    await page.click("details.problem-picker summary");
    await openTopicFilter(page);
    await page.waitForFunction(() =>
      document.querySelector("#recent-weak-topics button"),
    );
    assert.equal(await page.locator("#recent-weak-topics button").count(), 1);
    assert.doesNotMatch(await weakTopics.textContent(), /Topic filters/);
  },
);

lobbyTest(
  "recent snapshot excludes unsupported contracts and labels only counted legacy reports",
  async (page) => {
    const saved = (createdAt, decision, interviewContract) => ({
      problemId: EASY[0],
      createdAt,
      payload: { report: { decision, interviewContract } },
    });
    reports = [
      saved(40, "HIRE", {
        ...ACTIVE_CONTRACT,
        bundleVersion: ACTIVE_CONTRACT.bundleVersion + 1,
      }),
      saved(30, "HIRE", { ...ACTIVE_CONTRACT, extra: 1 }),
      saved(20, "NO_HIRE", ACTIVE_CONTRACT),
    ];
    await lobby(page);
    assert.equal(
      await page.locator("#recent-performance-summary").textContent(),
      "Last 1 assessed interview: 0 passed, 1 missed (0%).",
    );
    reports.push(saved(10, "HIRE", undefined));
    await lobby(page);
    assert.equal(
      await page.locator("#recent-performance-summary").textContent(),
      "Last 2 assessed interviews: 1 passed, 1 missed (50%). 1 of these interviews predates versioned scoring.",
    );
  },
);

lobbyTest(
  "recent snapshot reports a streak and the legacy note together",
  async (page) => {
    const saved = (problemId, createdAt, decision, interviewContract) => ({
      problemId,
      createdAt,
      payload: { report: { decision, interviewContract } },
    });
    reports = [
      saved(EASY[0], 30, "HIRE", ACTIVE_CONTRACT),
      saved(EASY[1], 20, "HIRE", undefined),
      saved(MEDIUM[0], 10, "NO_HIRE", undefined),
    ];
    await lobby(page);
    assert.equal(
      await page.locator("#recent-performance-summary").textContent(),
      "Last 3 assessed interviews: 2 passed, 1 missed (67%). Current pass streak: 2. 2 of these interviews predate versioned scoring.",
    );
  },
);

lobbyTest(
  "a failed report load hides the recent practice snapshot",
  async (page) => {
    failing = new Set(["/api/reports"]);
    await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
    await reaches(
      page,
      () =>
        document.querySelector("#progress-summary").textContent ===
        "Could not load saved account progress.",
    );

    assert.equal(await page.locator("#recent-performance").isHidden(), true);
    assert.equal(
      await page.locator("#recent-performance-summary").textContent(),
      "",
    );
  },
);

lobbyTest(
  "a candidate who touches nothing gets the server's own default length",
  async (page) => {
    const state = await lobby(page);

    // Compared against the markup rather than written here. tests/web.rs pins the
    // preselected button to the server's DEFAULT_DURATION_MIN, so asserting the
    // rendered page agrees with it ties what a candidate actually gets to the
    // server constant, without either file restating the difficulty mapping the
    // other half owns.
    assert.equal(
      state.duration,
      markupDuration(),
      "the lobby renders a different length than the markup preselects",
    );
    assert.deepEqual(state.levels, ["Medium"]);
    assert.equal(state.startDisabled, false);
  },
);

lobbyTest(
  "the recommendation does not move once it is on screen",
  async (page) => {
    reports = [hired("two-sum")];
    // Every write to the line, not a sample of it every thirty milliseconds. A
    // sampler misses a value that appears and is replaced inside one gap, which
    // is the shape of the bug this test is here for: recommending at load and
    // again when the fetch landed used to show two problems on most runs.
    await page.addInitScript(() => {
      window.__recommendations = [];
      // Attached to the one node, not to the document: a subtree observer on the
      // root fires across the parse of all 150 cards and costs more than the poll
      // it replaced. This runs before the document exists, so it waits for the
      // node and records whatever is already there when it arrives.
      const attach = () => {
        const line = document.querySelector("#recommendation");
        if (!line) return requestAnimationFrame(attach);
        const record = () => {
          const text = line.textContent;
          if (text && window.__recommendations.at(-1) !== text)
            window.__recommendations.push(text);
        };
        record();
        new MutationObserver(record).observe(line, {
          childList: true,
          characterData: true,
          subtree: true,
        });
      };
      attach();
    });
    await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
    await awaitReady(page);

    const seen = await page.evaluate(() => window.__recommendations);
    assert.equal(
      seen.length,
      1,
      `the recommendation changed under the reader: ${seen.join(" -> ")}`,
    );
  },
);

lobbyTest(
  "the lobby carries an assessed drill into the next practice session",
  async (page) => {
    reports = [focusedAttempt(EASY[0])];
    const state = await lobby(page);

    assert.equal(
      state.focus,
      "Carry forward: Test boundaries. Drill: Build a test table. Success: Predict each output.",
    );
  },
);

lobbyTest(
  "a candidate explicitly chooses whether to share the practice focus",
  async (page) => {
    reports = [focusedAttempt(EASY[0])];
    await lobby(page);
    assert.equal(await page.locator("#practice-focus-share").isHidden(), false);

    await page.click("#practice-focus-share-input");
    await page.click("#start");
    await page.waitForURL(/\/interview/);

    // Handed over in session storage, never the address bar, where a crafted
    // link could put its own text into the interviewer's instructions.
    assert.equal(new URL(page.url()).searchParams.get("focus"), null);
    assert.equal(
      await page.evaluate(() =>
        sessionStorage.getItem("codetrial.sharedPracticeFocus"),
      ),
      "Test boundaries",
    );
  },
);

lobbyTest(
  "loading a resume keeps the JD requirements already checked",
  async (page) => {
    await lobby(page);
    await page.click("details.interview-context summary");
    const txt = (name, text) => ({
      name,
      mimeType: "text/plain",
      buffer: Buffer.from(text),
    });

    await page.setInputFiles(
      "#grounding-jd",
      txt("jd.txt", "Must know Rust\nMust know SQL"),
    );
    const jd = page.locator(
      '#grounding-choices input[data-group="requirements"]',
    );
    await jd.first().waitFor();
    await jd.nth(1).check();

    await page.setInputFiles(
      "#grounding-resume",
      txt("resume.txt", "Skills: Rust, Go\nBuilt a parser"),
    );
    await page
      .locator('#grounding-choices input[data-group="skills"]')
      .first()
      .waitFor();

    assert.deepEqual(
      await jd.evaluateAll((boxes) => boxes.map((box) => box.checked)),
      [false, true],
    );
  },
);

/// A PDF built here rather than committed: one page per entry of `pages`, each
/// line a text-showing operator in the standard Helvetica. What matters is
/// that its text is only reachable through pdf.js, so a pass means the
/// vendored parser loaded, ran its worker and found the lines.
function pdfOf(pages) {
  // Written out as latin1 below, which keeps only the low byte of anything
  // wider: a test that used other text would get a different PDF than it wrote.
  const wide = pages.flat().find((line) => /[^\u0000-\u00ff]/.test(line));
  if (wide) throw new Error(`pdfOf writes Latin-1 only: ${wide}`);
  const escape = (line) => line.replace(/[\\()]/g, (char) => `\\${char}`);
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    `<< /Type /Pages /Kids [${pages.map((_, index) => `${4 + index * 2} 0 R`).join(" ")}] /Count ${pages.length} >>`,
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
  ];
  for (const [index, lines] of pages.entries()) {
    const content = `BT /F1 12 Tf 16 TL 72 720 Td ${lines.map((line) => `(${escape(line)}) Tj T*`).join(" ")} ET`;
    objects.push(
      `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents ${5 + index * 2} 0 R >>`,
      `<< /Length ${content.length} >>\nstream\n${content}\nendstream`,
    );
  }
  let body = "%PDF-1.4\n";
  const offsets = objects.map((object, index) => {
    const offset = body.length;
    body += `${index + 1} 0 obj\n${object}\nendobj\n`;
    return offset;
  });
  const xref = body.length;
  body += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  body += offsets
    .map((offset) => `${String(offset).padStart(10, "0")} 00000 n \n`)
    .join("");
  body += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(body, "latin1");
}

lobbyTest(
  "a resume and a JD read out of PDFs with the vendored pdf.js",
  async (page) => {
    await lobby(page);
    await page.click("details.interview-context summary");
    const asPdf = (name, pages) => ({
      name,
      mimeType: "application/pdf",
      buffer: pdfOf(pages),
    });

    await page.setInputFiles(
      "#grounding-resume",
      asPdf("resume.pdf", [
        ["Skills: Rust, C, Python", "Led the (key-update) rollback fix"],
        ["Built a parser"],
      ]),
    );
    await page
      .locator('#grounding-choices input[data-group="anchors"]')
      .nth(1)
      .waitFor();
    await page.setInputFiles(
      "#grounding-jd",
      asPdf("jd.pdf", [["Must know Rust"]]),
    );
    await page
      .locator('#grounding-choices input[data-group="requirements"]')
      .first()
      .waitFor();

    // Both pages, lines kept apart, and the escaped parentheses read back as
    // the text the candidate wrote.
    assert.deepEqual(await groundingText(page), [
      "Must know Rust",
      "Rust",
      "C",
      "Python",
      "Led the (key-update) rollback fix",
      "Built a parser",
    ]);
    assert.match(
      await page.locator("#grounding-resume-status").textContent(),
      /^Parsed locally/,
    );
  },
);

lobbyTest(
  "a PDF the platform types as something else is still read",
  async (page) => {
    await lobby(page);
    await page.click("details.interview-context summary");
    await page.setInputFiles("#grounding-jd", {
      name: "jd.pdf",
      mimeType: "application/octet-stream",
      buffer: pdfOf([["Must know Rust"]]),
    });
    await page
      .locator('#grounding-choices input[data-group="requirements"]')
      .first()
      .waitFor();
    assert.deepEqual(await groundingText(page), ["Must know Rust"]);
  },
);

lobbyTest(
  "a PDF over the page limit is refused with its page count",
  async (page) => {
    await lobby(page);
    await page.click("details.interview-context summary");
    const pages = Array.from({ length: 11 }, (_, index) => [
      `Must know topic ${index}`,
    ]);
    await page.setInputFiles("#grounding-jd", {
      name: "long.pdf",
      mimeType: "application/pdf",
      buffer: pdfOf(pages),
    });
    await reaches(page, () =>
      /11 pages/.test(
        document.querySelector("#grounding-jd-status").textContent,
      ),
    );
    assert.match(
      await page.locator("#grounding-jd-status").textContent(),
      /11 pages/,
    );
    assert.deepEqual(await groundingText(page), []);
  },
);

lobbyTest(
  "a missing PDF reader is reported as the reader, not the file",
  async (page) => {
    failing.add("/vendor/pdfjs/pdf.min.mjs");
    await lobby(page);
    await page.click("details.interview-context summary");
    await page.setInputFiles("#grounding-jd", {
      name: "jd.pdf",
      mimeType: "application/pdf",
      buffer: pdfOf([["Must know Rust"]]),
    });
    await reaches(page, () =>
      /reader did not load/.test(
        document.querySelector("#grounding-jd-status").textContent,
      ),
    );
    assert.match(
      await page.locator("#grounding-jd-status").textContent(),
      /reader did not load/,
    );
  },
);

lobbyTest(
  "a PDF with no text layer says so instead of offering nothing",
  async (page) => {
    await lobby(page);
    await page.click("details.interview-context summary");
    await page.setInputFiles("#grounding-resume", {
      name: "scan.pdf",
      mimeType: "application/pdf",
      buffer: pdfOf([[]]),
    });
    await reaches(page, () =>
      /selectable text/.test(
        document.querySelector("#grounding-resume-status").textContent,
      ),
    );
    assert.match(
      await page.locator("#grounding-resume-status").textContent(),
      /selectable text/,
    );
    assert.deepEqual(await groundingText(page), []);
  },
);

async function holdSlowRead(page) {
  await page.evaluate(() => {
    const read = Blob.prototype.arrayBuffer;
    let open;
    const gate = new Promise((resolve) => {
      open = resolve;
    });
    let done = null;
    Blob.prototype.arrayBuffer = async function () {
      if (this.name === "slow.txt") {
        await gate;
        done = read.call(this);
        return done;
      }
      return read.call(this);
    };
    window.releaseSlowRead = async () => {
      open();
      for (let wait = 0; !done && wait < 1000; wait++)
        await new Promise((resolve) => setTimeout(resolve, 10));
      if (!done) throw new Error("the gated read never started");
      await done;
      await new Promise((resolve) => setTimeout(resolve, 0));
    };
  });
  return () => page.evaluate(() => window.releaseSlowRead());
}

const groundingTxt = (name, text) => ({
  name,
  mimeType: "text/plain",
  buffer: Buffer.from(text),
});
const groundingText = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll("#grounding-choices label")].map((row) =>
      row.textContent.trim(),
    ),
  );

lobbyTest(
  "a slower read of an earlier JD does not replace the one picked after it",
  async (page) => {
    await lobby(page);
    await page.click("details.interview-context summary");
    const release = await holdSlowRead(page);

    await page.setInputFiles(
      "#grounding-jd",
      groundingTxt("slow.txt", "Must know Rust"),
    );
    await page.setInputFiles(
      "#grounding-jd",
      groundingTxt("jd.txt", "Must know SQL"),
    );
    await reaches(
      page,
      () => document.querySelector("#grounding-choices label") !== null,
    );
    await release();

    assert.deepEqual(await groundingText(page), ["Must know SQL"]);
    assert.match(
      await page.locator("#grounding-jd-status").textContent(),
      /^Parsed locally/,
    );
  },
);

lobbyTest(
  "a resume still reading when grounding is cleared stays cleared",
  async (page) => {
    await lobby(page);
    await page.click("details.interview-context summary");
    const release = await holdSlowRead(page);

    await page.setInputFiles(
      "#grounding-resume",
      groundingTxt("slow.txt", "Skills: Rust, Go\nBuilt a parser"),
    );
    await page.click("#grounding-clear");
    await release();

    assert.deepEqual(await groundingText(page), []);
    assert.equal(
      await page.locator("#grounding-resume-status").textContent(),
      "",
    );
  },
);

lobbyTest(
  "a manually selected problem still receives the arriving practice focus",
  async (page) => {
    reports = [focusedAttempt(EASY[0])];
    const release = await heldLobby(page);
    await page.evaluate((problemId) => {
      document.querySelector(`[data-problem="${problemId}"]`).click();
    }, MEDIUM[0]);
    release();
    await reaches(
      page,
      () => !document.querySelector("#practice-focus").hidden,
    );

    const state = await snapshot(page);
    assert.equal(
      state.card,
      MEDIUM[0],
      "the history must not replace the candidate's selection",
    );
    assert.match(state.focus, /Carry forward: Test boundaries/);
  },
);

lobbyTest(
  "start cannot fire before there is a problem to start",
  async (page) => {
    // Held open, so this is the pre-history state however slow the machine is,
    // rather than a snapshot hoping to beat a timer.
    const release = await heldLobby(page);

    const early = await snapshot(page);
    assert.equal(
      early.card,
      null,
      "a problem was chosen before the reports arrived",
    );
    assert.equal(
      early.startDisabled,
      true,
      "start was live with no problem behind it",
    );

    release();
    await awaitReady(page);
    const settled = await snapshot(page);
    assert.equal(settled.startDisabled, false);
    assert.ok(settled.card, "nothing was recommended");
  },
);

lobbyTest(
  "the start button ships the problem and length that are on screen",
  async (page) => {
    const state = await lobby(page);
    await page.click("#start");
    await page.waitForURL(/\/interview/);

    const query = new URL(page.url()).searchParams;
    // The page name, which is what the address bar shows, and not the id.
    assert.equal(query.get("problem"), state.card);
    assert.notEqual(query.get("problem"), "two-sum");
    assert.equal(query.get("duration"), state.duration);
    assert.equal(query.get("focus"), null);
    assert.equal(
      await page.evaluate(() =>
        sessionStorage.getItem("codetrial.sharedPracticeFocus"),
      ),
      null,
      "a focus is shared only after an explicit choice",
    );
  },
);

lobbyTest(
  "published problem names stay hidden until the candidate asks, and the choice is kept",
  async (page) => {
    await lobby(page);
    const visibleSources = () =>
      page.evaluate(
        () =>
          [...document.querySelectorAll(".problem-source")].filter(
            (source) => !source.hidden,
          ).length,
      );
    assert.equal(
      await visibleSources(),
      0,
      "a published name is on screen by default",
    );
    const sourceCount = await page.evaluate(
      () => document.querySelectorAll(".problem-source").length,
    );
    assert.equal(
      sourceCount,
      await page.evaluate(
        () => document.querySelectorAll(".problem-card").length,
      ),
      "every picker card has one source slot",
    );
    const namedSourceCount = Object.values(
      JSON.parse(read("web/problem-pages.json")),
    ).filter((entry) => entry.source).length;

    await page.evaluate(() => {
      document.querySelector(".problem-picker").open = true;
    });
    await page.check("#show-sources");
    // The names arrive with the map, fetched on the first request for them.
    await page.waitForFunction(
      (count) =>
        [...document.querySelectorAll(".problem-source")].filter(
          (source) => !source.hidden,
        ).length === count,
      namedSourceCount,
    );
    assert.match(
      await page
        .locator(`[data-problem="${pageOf("two-sum")}"] .problem-source`)
        .textContent(),
      /LeetCode: Two Sum/,
    );
    // The recommendation still names the scenario only.
    assert.doesNotMatch(
      await page.locator("#recommendation").textContent(),
      /LeetCode:/,
    );

    await lobby(page);
    await page.waitForFunction(
      (count) =>
        [...document.querySelectorAll(".problem-source")].filter(
          (source) => !source.hidden,
        ).length === count,
      namedSourceCount,
    );
  },
);

lobbyTest(
  "the lobby never suggests a length past its own default",
  async (page) => {
    // Half of the recording-safety property, and the half a browser can actually
    // see. The other half is a `const` assertion in src/config.rs holding
    // DEFAULT_DURATION_MIN at or below DEFAULT_RECORDING_MAX_MINUTES, so a
    // suggestion inside the default is inside what a default deployment records. Naming the default length "the cap"
    // here would be wrong: the two are separate environment variables and equal
    // only by their defaults. The sixty minute button stays reachable either way;
    // it just has to be a click rather than something the lobby chooses.
    const ceiling = Number(markupDuration());
    await lobby(page);

    for (const level of ["Easy", "Medium", "Hard"]) {
      for (const other of ["Easy", "Medium", "Hard"]) {
        await setLevel(page, other, other === level);
      }
      const suggested = Number((await snapshot(page)).duration);
      assert.ok(
        suggested <= ceiling,
        `${level} suggests ${suggested} minutes, past the ${ceiling} the lobby defaults to`,
      );
    }
  },
);

lobbyTest(
  "the picker shows the levels that are checked and no others",
  async (page) => {
    await lobby(page);

    const medium = (await snapshot(page)).visibleCards;
    await setLevel(page, "Hard", true);
    const both = (await snapshot(page)).visibleCards;
    await setLevel(page, "Medium", false);
    const hard = (await snapshot(page)).visibleCards;

    assert.ok(hard > 0, "checking Hard hid every problem");
    assert.ok(medium > hard, "Medium is not a different set from Hard");
    assert.equal(
      both,
      medium + hard,
      "checking both levels is not the union of them",
    );

    // The check that matters: no card from an unchecked level is on screen.
    const shown = await page.evaluate(() =>
      [...document.querySelectorAll(".problem-card")]
        .filter((card) => !card.hidden)
        .map((card) => card.dataset.difficulty),
    );
    assert.deepEqual([...new Set(shown)], ["Hard"]);
  },
);

lobbyTest(
  "choosing a problem by hand keeps the filter and the length the candidate set",
  async (page) => {
    await lobby(page);

    await setLevel(page, "Hard", true);
    await page.click('[data-duration="60"]');
    const before = await snapshot(page);
    assert.deepEqual(before.levels, ["Medium", "Hard"]);
    assert.equal(before.duration, "60");

    await page.click("details.problem-picker summary");
    await page.click(`[data-problem="${pageOf("candy")}"]`);
    const after = await snapshot(page);

    assert.equal(after.card, pageOf("candy"));
    assert.equal(
      after.pressed,
      pageOf("candy"),
      "the selection is a border colour and nothing else",
    );
    // And every other card says it is not pressed, rather than saying nothing:
    // one pressed button among 149 plain ones does not read as a choice.
    assert.equal(
      await page.evaluate(
        () =>
          document.querySelectorAll(".problem-card:not([aria-pressed])").length,
      ),
      0,
      "cards nobody selected carry no pressed state at all",
    );
    assert.deepEqual(
      after.levels,
      ["Medium", "Hard"],
      "picking a card rewrote the difficulty filter",
    );
    assert.equal(
      after.duration,
      "60",
      "picking a card discarded the length the candidate chose",
    );
  },
);

lobbyTest(
  "Random problem restores automatic selection and can draw again",
  async (page) => {
    await lobby(page);
    await setLevel(page, "Hard", true);
    await page.click('[data-duration="60"]');
    await page.click("details.problem-picker summary");
    const eligible = await page
      .locator(".problem-card:not([hidden])")
      .evaluateAll((cards) => cards.map((card) => card.dataset.problem));
    await page.click(`[data-problem="${eligible[0]}"]`);
    await page.evaluate(() => {
      Math.random = () => 0;
    });
    await page.click("#random-problem");
    const first = await snapshot(page);
    const title = await page
      .locator(`[data-problem="${first.card}"] .problem-title`)
      .textContent();
    assert.equal(
      first.note,
      `Selected problem: ${title} (${(await cardInfo(page, first.card)).level}).`,
    );
    assert.equal(first.card, eligible[1]);
    assert.deepEqual(first.levels, ["Medium", "Hard"]);
    assert.equal(first.duration, "60");
    await restore(page);
    await awaitReady(page);
    assert.equal((await snapshot(page)).card, first.card);
    const button = await page.locator("#random-problem").boundingBox();
    const recommendation = await page.locator("#recommendation").boundingBox();
    assert.ok(button.x + button.width <= recommendation.x);
    assert.ok(
      recommendation.y >= button.y &&
        recommendation.y < button.y + button.height,
    );

    await page.click("#random-problem");
    const second = await snapshot(page);
    assert.equal(second.card, eligible[0]);
    assert.deepEqual(
      await page.locator("#random-problem").boundingBox(),
      button,
    );
    await page.click("#start");
    await page.waitForURL(/\/interview/);
    assert.equal(new URL(page.url()).searchParams.get("problem"), second.card);
  },
);

lobbyTest(
  "a drawn problem survives a difficulty change that still includes it",
  async (page) => {
    await page.addInitScript(() => {
      Math.random = () => 0;
    });
    await lobby(page);
    await page.click("#random-problem");
    const drawn = await snapshot(page);
    const level = (await cardInfo(page, drawn.card)).level;

    const others = ["Easy", "Medium", "Hard"].filter((name) => name !== level);
    for (const other of others) {
      await setLevel(page, other, true);
      assert.equal(
        (await snapshot(page)).card,
        drawn.card,
        `checking ${other} redrew a problem it did not hide`,
      );
    }
    // Returning from the media preflight re-runs the recommendation, and the
    // roll that drew this problem names a different one under the new filter.
    await restore(page);
    await awaitReady(page);
    assert.equal(
      (await snapshot(page)).card,
      drawn.card,
      "a kept problem moved when the page was restored",
    );
    await setLevel(page, others[0], false);
    assert.equal(
      (await snapshot(page)).card,
      drawn.card,
      `unchecking ${others[0]} redrew a problem it did not hide`,
    );

    await setLevel(page, level, false);
    const redrawn = await snapshot(page);
    assert.notEqual(redrawn.card, drawn.card, "a hidden problem stayed picked");
    const info = await cardInfo(page, redrawn.card);
    assert.equal(info.level, others[1]);
    assert.equal(info.hidden, false);
  },
);

// Widening the filter keeps the card on screen, but it was drawn from the
// reports in hand, so refreshed ones that pass it decide again. The usual order
// widens before leaving for the interview; the other widens inside the reload,
// while the reports in hand still belong to the page before the candidate left.
for (const [when, widenFirst] of [
  ["before the interview", true],
  ["while the history reloads", false],
]) {
  lobbyTest(
    `a drawn problem kept ${when} is redrawn from the history it comes back to`,
    async (page) => {
      const shown = (await lobby(page)).card;
      const widen = async () => {
        await setLevel(page, "Hard", true);
        assert.equal((await snapshot(page)).card, shown);
      };

      if (widenFirst) await widen();
      const release = holdHistory();
      reports = [hired(shown)];
      await restore(page);
      if (!widenFirst) await widen();

      release();
      await awaitReady(page);
      assert.notEqual(
        (await snapshot(page)).card,
        shown,
        "a problem the refreshed history passed stayed recommended",
      );
    },
  );
}

lobbyTest(
  "Random problem waits for history on load and browser restore",
  async (page) => {
    const random = page.locator("#random-problem");
    const release = await heldLobby(page);
    try {
      assert.equal(await random.isDisabled(), true);
      assert.equal(
        await random.evaluate((button) => getComputedStyle(button).cursor),
        "not-allowed",
      );
    } finally {
      release();
    }
    await awaitReady(page);
    assert.equal(await random.isEnabled(), true);
    assert.equal(
      await random.evaluate((button) => getComputedStyle(button).cursor),
      "pointer",
    );

    const releaseRestore = holdHistory();
    try {
      await restore(page);
      assert.equal(await random.isDisabled(), true);
      assert.equal(
        await random.evaluate((button) => getComputedStyle(button).cursor),
        "not-allowed",
      );
    } finally {
      releaseRestore();
    }
    await awaitReady(page);
    assert.equal(await random.isEnabled(), true);
    assert.equal(
      await random.evaluate((button) => getComputedStyle(button).cursor),
      "pointer",
    );
  },
);

lobbyTest(
  "a random draw hides an out-of-filter legacy retry and updates its suggested duration",
  async (page) => {
    reports = [savedAttempt(EASY[0])];
    reports[0].payload.date = new Date().toISOString();
    await lobby(page);
    await page.getByRole("button", { name: "Try again", exact: true }).click();
    const before = await snapshot(page);
    assert.equal(before.duration, "30");
    assert.equal((await cardInfo(page, EASY[0])).hidden, false);
    await page.click("#random-problem");
    const after = await snapshot(page);
    assert.equal((await cardInfo(page, EASY[0])).hidden, true);
    assert.equal((await cardInfo(page, after.card)).level, "Medium");
    assert.deepEqual(after.levels, before.levels);
    assert.equal(after.duration, "45");
  },
);

for (const first of [0, 1]) {
  lobbyTest(
    `overlapping history loads ignore the old response when load ${first} finishes first`,
    async (page) => {
      await page.addInitScript(() => {
        const fetch = window.fetch.bind(window);
        window.pendingHistory = [];
        window.fetch = (url, ...args) =>
          url === "/api/reports"
            ? new Promise((resolve) =>
                window.pendingHistory.push((reports) =>
                  resolve(new Response(JSON.stringify({ reports }))),
                ),
              )
            : fetch(url, ...args);
      });
      await page.goto(`${base}/`);
      await page.waitForFunction(() => window.pendingHistory.length === 1);
      await restore(page);
      await page.waitForFunction(() => window.pendingHistory.length === 2);
      let latestDone = false;
      for (const index of [first, 1 - first]) {
        await page.evaluate(
          async ([index, reports]) => {
            window.pendingHistory[index](reports);
            await new Promise(requestAnimationFrame);
          },
          [index, [savedAttempt(index === 0 ? EASY[0] : MEDIUM[0])]],
        );
        latestDone ||= index === 1;
        assert.equal(await page.isDisabled("#random-problem"), !latestDone);
        assert.equal(
          (await snapshot(page)).card,
          latestDone ? MEDIUM[0] : null,
        );
      }
      await page.click("details.problem-picker summary");
      await page.click(`[data-problem="${MEDIUM[1]}"]`);
      assert.equal((await snapshot(page)).card, MEDIUM[1]);
      await page.click("#random-problem");
      const drawn = await snapshot(page);
      assert.notEqual(drawn.card, MEDIUM[1]);
      assert.equal((await cardInfo(page, drawn.card)).level, "Medium");
      assert.doesNotMatch(drawn.note, /Review due/);
      await page.click("#random-problem");
      assert.notEqual((await snapshot(page)).card, drawn.card);
    },
  );
}

lobbyTest(
  "an explicit length survives everything that would otherwise suggest one",
  async (page) => {
    await lobby(page);

    await page.click('[data-duration="60"]');
    await setLevel(page, "Easy", true);
    await setLevel(page, "Medium", false);
    assert.equal(
      (await snapshot(page)).duration,
      "60",
      "a suggestion overrode an explicit choice",
    );
  },
);

lobbyTest(
  "the last difficulty cannot be unchecked into an empty lobby",
  async (page) => {
    const before = await lobby(page);

    await setLevel(page, "Medium", false);
    const state = await snapshot(page);
    assert.deepEqual(
      state.levels,
      ["Medium"],
      "the lobby was left filtering on nothing",
    );
    // The box goes back, so nothing about what is on offer moved and the
    // recommendation must not move either. Rerolling on the way through swapped
    // it for a different problem at the same level, which reads as the lobby
    // changing its mind on its own.
    assert.equal(
      state.card,
      before.card,
      "a rejected uncheck rerolled the recommendation",
    );
    assert.equal(state.note, before.note);
    assert.ok(state.card, "unchecking the last level left no problem to start");
    assert.ok(state.visibleCards > 0, "the picker was left with nothing in it");
    assert.equal(state.startDisabled, false);
    assert.equal(
      (await cardInfo(page, state.card)).hidden,
      false,
      "the surviving selection is a card nobody can see",
    );
  },
);

lobbyTest(
  "history saved on this device under published ids still counts as passed",
  async (page) => {
    // An earlier build saved the published id. The lobby renames it through the
    // page map, fetched because such an entry is there; unrenamed, the ids match
    // no card, so the streak has no level and the lobby stays put. Account
    // history arrives renamed by the server, so this is the device's own copy.
    session = { signedIn: false };
    await page.addInitScript(
      (entries) => {
        if (!localStorage.getItem("codetrial_history")) {
          localStorage.setItem("codetrial_history", JSON.stringify(entries));
        }
      },
      ["jump-game", "gas-station"].map((problemId, index) => ({
        problemId,
        date: `2026-01-0${index + 1}T00:00:00Z`,
        report: { decision: "HIRE" },
      })),
    );
    const state = await lobby(page);
    assert.match(state.note, /passed your last two Medium problems/);
    const saved = await page.evaluate(() =>
      JSON.parse(localStorage.getItem("codetrial_history")),
    );
    assert.deepEqual(
      saved.map((entry) => entry.problemId).sort(),
      MEDIUM.slice().sort(),
      "renamed in place",
    );
    assert.deepEqual(state.levels, ["Hard"]);
  },
);

lobbyTest(
  "short local history remains visible when full reports exceed the review budget",
  async (page) => {
    session = { signedIn: false };
    await page.addInitScript((problemId) => {
      const entries = Array.from({ length: 20 }, (_, index) => ({
        ...(index < 10 ? { id: `attempt-${index}` } : {}),
        problemId,
        problemTitle: "Two Sum",
        date: `2026-01-${String(index + 1).padStart(2, "0")}T00:00:00Z`,
        report: { decision: "HIRE", summary: "x".repeat(20_000) },
      }));
      localStorage.setItem("codetrial_history", JSON.stringify(entries));
    }, EASY[0]);

    await lobby(page);
    assert.equal(
      await page.getByRole("button", { name: "Open report" }).count(),
      20,
    );
  },
);

lobbyTest(
  "a lobby with no old history never fetches the published names",
  async (page) => {
    const fetched = [];
    page.on("request", (request) =>
      fetched.push(new URL(request.url()).pathname),
    );
    reports = [hired(EASY[0])];
    await lobby(page);
    assert.ok(
      !fetched.includes("/problem-pages.json"),
      "the map was fetched without a reason",
    );
    const index = await page.content();
    assert.doesNotMatch(index, /data-problem="two-sum"|LeetCode: Two Sum/);
  },
);

lobbyTest(
  "a problem already passed is not what gets recommended",
  async (page) => {
    // Passed one Hard and missed another, so the streak has no opinion and the
    // only thing under test is the exclusion.
    reports = [hired(HARD[0]), missed(HARD[1])];
    await lobby(page);
    await setLevel(page, "Hard", true);
    await setLevel(page, "Medium", false);

    const state = await snapshot(page);
    // Assert there is a real, visible Hard pick before asserting which one it is
    // not: "not the passed problem" is also true of no problem at all.
    assert.ok(state.card, "nothing was recommended");
    assert.equal(state.startDisabled, false);
    const picked = await cardInfo(page, state.card);
    assert.equal(picked.level, "Hard");
    assert.equal(
      picked.hidden,
      false,
      "the recommendation is a card the filter hides",
    );
    assert.notEqual(
      state.card,
      HARD[0],
      "a problem the candidate passed came back",
    );
  },
);

lobbyTest(
  "a completed problem returns with a due-review explanation",
  async (page) => {
    reports = [savedAttempt(EASY[0])];
    await lobby(page);
    await setLevel(page, "Easy", true);
    await setLevel(page, "Medium", false);

    const state = await snapshot(page);
    assert.equal(state.card, EASY[0]);
    assert.match(state.note, /Review due after 1 day/);
  },
);

lobbyTest(
  "a saved report reopens and collapses from the lobby",
  async (page) => {
    reports = [savedAttempt(EASY[0])];
    reports[0].payload.report.debrief = {
      approach: "Use a map to find the complement in linear time.",
    };
    await lobby(page);
    const history = page.locator("#attempt-history");
    const report = history.locator(".report-card");
    const toggle = history.getByRole("button", {
      name: /^(Open|Collapse) report$/,
    });
    assert.equal(await toggle.textContent(), "Open report");
    assert.equal(await report.isVisible(), false);

    await toggle.click();
    assert.equal(await toggle.textContent(), "Collapse report");
    assert.equal(await report.isVisible(), true);
    const finalCode = report
      .locator("details")
      .filter({ hasText: "Your final code" });
    assert.equal(await finalCode.evaluate((node) => node.open), false);
    await finalCode.locator("summary").click();
    assert.equal(await finalCode.evaluate((node) => node.open), true);
    const debrief = report
      .locator("details")
      .filter({ hasText: "What the interviewer held back" });
    assert.equal(await debrief.evaluate((node) => node.open), false);
    await debrief.locator("summary").click();
    assert.equal(await debrief.evaluate((node) => node.open), true);

    await toggle.click();
    assert.equal(await toggle.textContent(), "Open report");
    assert.equal(await report.isVisible(), false);

    await toggle.click();
    assert.equal(await toggle.textContent(), "Collapse report");
    assert.equal(await report.count(), 1);
    assert.equal(await report.isVisible(), true);
    assert.equal(await finalCode.evaluate((node) => node.open), true);
    assert.equal(await debrief.evaluate((node) => node.open), true);
    assert.equal(
      await history.locator(".report-actions, #download-report, #done").count(),
      0,
    );
  },
);

for (const storage of ["account", "device"]) {
  lobbyTest(
    `saved ${storage} attempt labels distinguish local times on the same day`,
    async (page) => {
      const entries = ["2026-01-01T16:15:00Z", "2026-01-02T10:30:00Z"].map(
        (date, index) => ({
          id: `attempt-${index}`,
          problemId: EASY[0],
          problemTitle: "Same problem",
          language: "python",
          date,
          report: { decision: "HIRE" },
        }),
      );
      if (storage === "account") {
        reports = entries.map((payload) => ({ id: payload.id, payload }));
      } else {
        session = { signedIn: false };
        await page.addInitScript((entries) => {
          localStorage.setItem("codetrial_history", JSON.stringify(entries));
        }, entries);
      }
      await lobby(page);
      const times = ["Jan 2, 2026, 6:30 PM", "Jan 2, 2026, 12:15 AM"];
      const rows = page.locator("#attempt-history > li");
      assert.deepEqual(
        await rows.locator(":scope > p").allTextContents(),
        times.map(
          (when) =>
            `${when} · Same problem · Difficulty not recorded · Python · HIRE`,
        ),
      );
      for (const [index, when] of times.entries()) {
        const row = rows.nth(index);
        for (const name of [
          `Download report (.md) for Same problem, ${when}`,
          `Delete the ${when} report for Same problem`,
        ]) {
          assert.equal(
            await row.getByRole("button", { name, exact: true }).count(),
            1,
          );
        }
      }
    },
    { timezoneId: "Asia/Taipei", locale: "en-US" },
  );
}

for (const storage of ["account", "device"]) {
  lobbyTest(
    `saved ${storage} reports download their own markdown with local timestamps`,
    async (page) => {
      const cases = [
        {
          report: { decision: "HIRE", codingScore: 81 },
          language: "python",
          verdict: "HIRE",
          heading: "Python",
        },
        {
          report: { decision: "NO_HIRE", codingScore: 42 },
          language: "javascript",
          verdict: "NO HIRE",
          heading: "JavaScript",
        },
        {
          report: {},
          language: "unknown-language",
          verdict: null,
          heading: "not recorded",
        },
        {
          report: { incomplete: true, decision: "HIRE" },
          verdict: null,
          heading: "not recorded",
        },
      ];
      const entries = cases.map((item, index) => ({
        problemId: EASY[0],
        problemTitle: "Saved report",
        date: `2026-01-01T${17 + index}:30:00Z`,
        language: item.language,
        report: { ...item.report, summary: `Stored summary ${index}` },
      }));
      if (storage === "account") {
        reports = entries.map((payload) => ({ payload }));
      } else {
        session = { signedIn: false };
        await page.addInitScript((entries) => {
          localStorage.setItem("codetrial_history", JSON.stringify(entries));
        }, entries);
      }
      await lobby(page);
      const history = page.locator("#attempt-history");
      assert.equal(
        await history.getByRole("button", { name: /^Download report/ }).count(),
        cases.length,
      );
      for (const [index, item] of cases.entries()) {
        const when = `1/2/2026, ${index + 1}:30:00 AM`;
        const button = history.getByRole("button", {
          name: `Download report (.md) for Saved report, Jan 2, 2026, ${index + 1}:30 AM`,
          exact: true,
        });
        const downloading = page.waitForEvent("download");
        await button.click();
        const download = await downloading;
        assert.equal(await download.failure(), null);
        assert.equal(
          download.suggestedFilename(),
          `interview-report-${EASY[0]}-2026-01-02-0${index + 1}3000.md`,
        );
        const chunks = [];
        for await (const chunk of await download.createReadStream())
          chunks.push(chunk);
        const markdown = Buffer.concat(chunks).toString("utf8");
        assert.ok(
          markdown.startsWith(`# Interview Report - Saved report\n_${when}_`),
        );
        assert.ok(markdown.includes(`Stored summary ${index}`));
        assert.ok(
          markdown.includes(
            `## Final code (${item.heading})\n(final code was not saved)`,
          ),
        );
        assert.match(
          markdown,
          /## Conversation transcript\n\(transcript was not saved\)/,
        );
        assert.doesNotMatch(
          markdown,
          /```|no speech captured|editor was empty/,
        );
        if (item.verdict) {
          assert.ok(markdown.includes(`## Verdict: ${item.verdict}\n`));
          assert.ok(
            markdown.includes(`| Coding | ${item.report.codingScore} / 100 |`),
          );
        } else {
          assert.match(markdown, /## No evaluation/);
          assert.doesNotMatch(
            markdown,
            /## Verdict:|Judged against|\/ 100|Hints used/,
          );
        }
      }
    },
    { timezoneId: "Asia/Taipei", locale: "en-US", acceptDownloads: true },
  );
}

lobbyTest(
  "try again selects a problem without escaping an active title search",
  async (page) => {
    reports = [savedAttempt(EASY[0])];
    await lobby(page);
    await openTopicFilter(page);
    await page.fill("#problem-search", "zzzzzz-no-match");
    assert.equal(await page.locator(".problem-card:visible").count(), 0);

    await page.getByRole("button", { name: "Try again", exact: true }).click();
    assert.equal((await snapshot(page)).card, EASY[0]);
    assert.equal(
      await page.locator(".problem-card:visible").count(),
      0,
      "try again made a card visible despite a zero-result title search",
    );
    assert.equal(
      await page.locator(`[data-problem="${EASY[0]}"]`).isVisible(),
      false,
    );
    assert.match(
      await page.locator("#problem-filter-summary").textContent(),
      /0 problems matching scenario title/,
    );
  },
);

lobbyTest("try again selects the problem", async (page) => {
  reports = [savedAttempt(EASY[0])];
  await lobby(page);
  await page.getByRole("button", { name: "Try again" }).click();
  assert.equal((await snapshot(page)).card, EASY[0]);
  assert.match(
    await page.locator("#recommendation").textContent(),
    /Selected problem:/,
  );
});

for (const filter of ["difficulty", "topic"]) {
  lobbyTest(
    `try again reveals the card and practice checkbox hidden by ${filter}`,
    async (page) => {
      reports = [savedAttempt(EASY[0])];
      reports[0].payload.date = new Date().toISOString();
      await lobby(page);
      await openTopicFilter(page);
      if (filter === "topic") {
        await setLevel(page, "Easy", true);
        await page.selectOption("#problem-topic", "Stack");
      }
      const card = page.locator(`[data-problem="${EASY[0]}"]`);
      const entry = page.locator(`#problem-entry-${EASY[0]}`);
      const checkbox = practiceInput(page, EASY[0]);
      const before = await snapshot(page);
      const topic = await page.locator("#problem-topic").inputValue();
      assert.equal(await entry.isHidden(), true);
      assert.equal(await card.isHidden(), true);
      assert.equal(await checkbox.isHidden(), true);
      await page
        .getByRole("button", { name: "Try again", exact: true })
        .click();
      const retried = await snapshot(page);
      assert.equal(retried.card, EASY[0]);
      assert.equal(retried.pressed, EASY[0]);
      assert.deepEqual(retried.levels, before.levels);
      assert.equal(await page.locator("#problem-topic").inputValue(), topic);
      assert.equal(await entry.isVisible(), true);
      assert.equal(await card.isVisible(), true);
      assert.equal(await checkbox.isVisible(), true);
      await checkbox.check();
      assert.equal((await snapshot(page)).card, EASY[0]);
      await restore(page);
      await awaitReady(page);
      assert.equal(await card.isVisible(), true);
      assert.equal(await checkbox.isVisible(), true);
      assert.equal(await checkbox.isChecked(), true);
      await page.click("#start");
      await page.waitForURL("**/interview?**");
      assert.equal(new URL(page.url()).searchParams.get("problem"), EASY[0]);
    },
  );
}

lobbyTest("try again restores the saved language", async (page) => {
  reports = [
    {
      payload: {
        ...savedAttempt(EASY[0]).payload,
        language: "javascript",
      },
    },
  ];
  await lobby(page);
  await page.getByRole("button", { name: "Try again" }).click();
  await page.click("#start");
  await page.waitForURL("**/interview?**");
  assert.equal(new URL(page.url()).searchParams.get("language"), "javascript");
});

for (const account of [false, true]) {
  lobbyTest(
    `Try again from Strict selects Practice and restores retry settings from ${account ? "account" : "device"} history`,
    async (page) => {
      const attempt = {
        ...savedAttempt(EASY[0]).payload,
        selectionMode: "strict",
        language: "javascript",
        durationMin: 60,
        interviewLoop: "coding_only",
        interviewMode: "coding",
      };
      if (account) reports = [{ payload: attempt }];
      else {
        session = {
          signedIn: false,
          loginRequired: true,
          guestModeAvailable: true,
        };
        await page.addInitScript((entry) => {
          localStorage.setItem("codetrial_history", JSON.stringify([entry]));
        }, attempt);
      }
      await lobby(page);
      await setLevel(page, "Hard", true);
      const levels = (await snapshot(page)).levels;
      await page.selectOption("#selection-mode", "strict");
      await page
        .getByRole("button", { name: "Try again", exact: true })
        .click();
      assert.equal(
        await page.locator("#selection-mode").inputValue(),
        "practice",
      );
      assert.equal(await page.locator(".problem-picker").isHidden(), false);
      assert.equal((await snapshot(page)).card, EASY[0]);
      assert.deepEqual((await snapshot(page)).levels, levels);
      await page.click("#start");
      await page.waitForURL("**/interview?**");
      const params = new URL(page.url()).searchParams;
      assert.equal(params.has("selection"), false);
      assert.equal(params.get("problem"), EASY[0]);
      assert.equal(params.get("language"), "javascript");
      assert.equal(params.get("duration"), "60");
      assert.equal(params.get("loop"), "coding_only");
      assert.equal(params.get("mode"), "coding");
    },
  );
}

for (const transition of ["change", "pageshow", "Start", "back to Practice"]) {
  lobbyTest(
    `Strict clears Try again language through ${transition}`,
    async (page) => {
      reports = [
        {
          payload: {
            ...savedAttempt(EASY[0]).payload,
            language: "javascript",
            durationMin: 60,
            interviewLoop: "coding_only",
            interviewMode: "coding",
          },
        },
      ];
      await lobby(page);
      await setLevel(page, "Hard", true);
      const levels = (await snapshot(page)).levels;
      await page
        .getByRole("button", { name: "Try again", exact: true })
        .click();
      if (transition === "change" || transition === "back to Practice") {
        await page.selectOption("#selection-mode", "strict");
      } else {
        await page.evaluate(() => {
          document.querySelector("#selection-mode").value = "strict";
        });
        if (transition === "pageshow") await normalPageshow(page);
      }
      if (transition === "back to Practice")
        await page.selectOption("#selection-mode", "practice");
      await page.click("#start");
      await page.waitForURL("**/interview?**");
      const params = new URL(page.url()).searchParams;
      assert.equal(params.has("language"), false);
      if (transition === "back to Practice") {
        assert.equal(params.has("selection"), false);
        assert.equal(params.get("problem"), EASY[0]);
      } else {
        assert.equal(params.get("selection"), "strict");
        assert.equal(params.has("problem"), false);
        assert.deepEqual(params.getAll("difficulty"), levels);
      }
      assert.equal(params.get("duration"), "60");
      assert.equal(params.get("loop"), "coding_only");
      assert.equal(params.get("mode"), "coding");
    },
  );
}

lobbyTest(
  "an unmappable history entry fetches the page map at most once",
  async (page) => {
    session = { signedIn: false };
    const requests = [];
    page.on("request", (request) =>
      requests.push(new URL(request.url()).pathname),
    );
    await page.addInitScript(() => {
      if (!localStorage.getItem("codetrial_history"))
        localStorage.setItem(
          "codetrial_history",
          JSON.stringify([
            {
              problemId: "retired-problem",
              date: "2026-01-01T00:00:00Z",
              report: { decision: "HIRE" },
            },
          ]),
        );
    });
    await lobby(page);
    await lobby(page);
    assert.equal(
      requests.filter((path) => path === "/problem-pages.json").length,
      1,
    );
  },
);

lobbyTest(
  "a due review below the suggested level is shown and recommended",
  async (page) => {
    reports = [savedAttempt(EASY[0]), hired(EASY[1])];
    const state = await lobby(page);

    assert.deepEqual(state.levels, ["Medium"]);
    assert.equal(state.card, EASY[0]);
    assert.match(state.note, /Review due after 1 day \(Easy\)/);
    assert.equal((await cardInfo(page, EASY[0])).hidden, false);
  },
);

lobbyTest(
  "two passes move the candidate up a level, and the lobby says why",
  async (page) => {
    reports = [hired(EASY[0]), hired(EASY[1])];
    const state = await lobby(page);

    assert.deepEqual(
      state.levels,
      ["Medium"],
      "passing twice at Easy did not move the candidate",
    );
    assert.match(state.note, /passed your last two Easy problems/);
    assert.equal(
      (await cardInfo(page, state.card)).level,
      "Medium",
      "the level moved but the problem did not follow it",
    );
  },
);

lobbyTest(
  "two misses move the candidate down a level, and the lobby says why",
  async (page) => {
    reports = [missed(MEDIUM[0]), missed(MEDIUM[1])];
    const state = await lobby(page);

    assert.deepEqual(state.levels, ["Easy"]);
    assert.match(state.note, /last two Medium problems did not land/);
    // Easy carries the shorter slot with it.
    assert.equal(state.duration, "30");
  },
);

lobbyTest(
  "a candidate who picks a level first is not overruled by their history",
  async (page) => {
    // Two passed Easy problems, which would otherwise check Medium and say so.
    reports = [hired(EASY[0]), hired(EASY[1])];
    const release = await heldLobby(page);

    // Checked while the reports are held, so the suggestion arrives strictly
    // after the candidate has already said what they want.
    await setLevel(page, "Hard", true);
    await setLevel(page, "Medium", false);
    release();
    await awaitReady(page);

    const state = await snapshot(page);
    assert.deepEqual(
      state.levels,
      ["Hard"],
      "the history overrode a level the candidate chose",
    );
    assert.equal(
      state.note.includes("passed your last two"),
      false,
      "the lobby explained a level the candidate had chosen for themselves",
    );
  },
);

lobbyTest(
  "a gated candidate signs in and reaches the interview",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    reports = [];
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));

    await lobby(page);
    assert.equal((await snapshot(page)).startText, "Use GitHub to start");

    await page.fill("#github-login", "candidate");
    await page.click("#start");
    // The regression this pins: the handler kept using the event's target across
    // an await, where the browser has already cleared it. It threw on the line
    // after the login succeeded, so the name was recorded and the interview never
    // started, leaving the button disabled on "Recording GitHub..." for good.
    await page.waitForURL(/\/interview/);
    assert.deepEqual(
      errors,
      [],
      "the start handler threw on the way to the interview",
    );
  },
);

lobbyTest(
  "a lobby restored from cache rechecks the session instead of replaying it",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    reports = [];
    await lobby(page);
    assert.equal((await snapshot(page)).startText, "Use GitHub to start");

    // The login lands while this page is not the one in front: on the way out to
    // the interview, or in another tab. The cached page knows none of it, so what
    // it holds about the session is now wrong in every part.
    session = { signedIn: true, user: { login: "candidate" } };
    await restore(page);
    await reaches(
      page,
      () =>
        document
          .querySelector("#account-status")
          .textContent.startsWith("Signed in") &&
        !document.querySelector("#start").disabled,
    );

    const restored = await snapshot(page);
    assert.equal(
      restored.startText,
      "Start interview",
      "a signed-in candidate was told to sign in",
    );
    assert.equal(
      restored.startDisabled,
      false,
      "the restored button never came back",
    );
    assert.match(restored.account, /Signed in as candidate/);
  },
);

lobbyTest(
  "a restored lobby re-reads the history, not just the session",
  async (page) => {
    reports = [hired(MEDIUM[0]), hired(MEDIUM[1])];
    const before = await lobby(page);
    assert.deepEqual(before.levels, ["Hard"]);
    assert.match(before.note, /passed your last two Medium problems/);

    // An interview finished in between, in the tab this page was cached behind.
    // The restore refetches the reports, so everything the old ones decided has
    // to be decided again: re-enabling the button while the checked level and the
    // sentence under it still describe the replaced history is the bug here.
    reports = [missed(MEDIUM[0]), missed(MEDIUM[1])];
    await restore(page);
    await reaches(
      page,
      () =>
        !document.querySelector("#start").disabled &&
        document
          .querySelector("#recommendation")
          .textContent.includes("did not land"),
    );

    const after = await snapshot(page);
    assert.deepEqual(
      after.levels,
      ["Easy"],
      "the restored lobby kept the level the old reports set",
    );
    assert.match(after.note, /last two Medium problems did not land/);
    assert.equal(
      after.duration,
      "30",
      "the level moved but the length it implies did not",
    );
  },
);

lobbyTest(
  "dropping the level of a hand-picked problem does not leave it startable",
  async (page) => {
    const release = await heldLobby(page);

    // All of this while the history is held open, where there is nothing to
    // choose a replacement with.
    await page.click("details.problem-picker summary");
    await page.click(`[data-problem="${pageOf("jump-game")}"]`);
    assert.equal((await snapshot(page)).card, pageOf("jump-game"));

    await setLevel(page, "Hard", true);
    await setLevel(page, "Medium", false);
    const dropped = await snapshot(page);
    // The card is hidden by the filter now, so a start button still carrying it
    // would begin an interview on a level the candidate had just cleared.
    assert.equal(
      dropped.card,
      null,
      "a problem the filter hides stayed selected",
    );
    assert.equal(
      dropped.startDisabled,
      true,
      "start still carried the filtered-out problem",
    );

    release();
    await awaitReady(page);
    const settled = await snapshot(page);
    assert.deepEqual(settled.levels, ["Hard"]);

    // Assert on what start actually ships, not on which card wears a class: the
    // class is cleared either way, so it cannot tell a cleared pick from a stale
    // one still sitting in the query string.
    await page.click("#start");
    await page.waitForURL(/\/interview/);
    const shipped = new URL(page.url()).searchParams.get("problem");
    assert.equal(shipped, settled.card);
    assert.notEqual(
      shipped,
      pageOf("jump-game"),
      "start shipped the problem the filter dropped",
    );
  },
);

lobbyTest(
  "widening the filter keeps a problem the candidate picked by hand",
  async (page) => {
    const release = await heldLobby(page);

    await page.click("details.problem-picker summary");
    await page.click(`[data-problem="${pageOf("jump-game")}"]`);

    // Adding a level does not hide their card, so it is not a reason to discard
    // a choice they made on purpose.
    await setLevel(page, "Hard", true);
    const widened = await snapshot(page);
    assert.equal(
      widened.card,
      pageOf("jump-game"),
      "adding a difficulty threw away an explicit pick",
    );
    assert.deepEqual(widened.levels, ["Medium", "Hard"]);
    assert.equal(widened.startDisabled, false);
    await normalPageshow(page);
    assert.equal(
      await page.isDisabled("#start"),
      false,
      "Practice must keep a manual pick startable during the initial history load",
    );

    // And the arriving history does not overrule it either.
    release();
    await awaitReady(page);
    assert.equal(
      (await snapshot(page)).card,
      pageOf("jump-game"),
      "the history overruled an explicit pick",
    );
  },
);

lobbyTest(
  "a restore in flight is not a history a difficulty change may read",
  async (page) => {
    reports = [savedAttempt(MEDIUM[0])];
    await lobby(page);

    // Restored from cache, so the reports in hand are the ones from before the
    // candidate left. Held open, the page is back in the same state a first load
    // starts in, and a difficulty change during it must wait the same way.
    const release = holdHistory();
    reports = [hired(HARD[0]), hired(HARD[1])];
    await restore(page);
    await setLevel(page, "Hard", true);
    await setLevel(page, "Medium", false);

    const midflight = await snapshot(page);
    assert.equal(
      midflight.card,
      null,
      "a problem was chosen from the history being replaced",
    );
    assert.equal(
      midflight.startDisabled,
      true,
      "start went live on a history already known stale",
    );
    assert.equal(await page.locator("#delete-reports").isDisabled(), true);

    release();
    await awaitReady(page);
    const settled = await snapshot(page);
    assert.ok(
      settled.card,
      "nothing was recommended once the refreshed reports arrived",
    );
    assert.ok(
      ![HARD[0], HARD[1]].includes(settled.card),
      "the refreshed history was not applied to the pick",
    );
  },
);

// One value, not the shape matrix: what only a browser can show is the DOM
// outcome, and "a stored history that is not a list reads as no history" in
// account.test.js already pins the shapes for free.
lobbyTest(
  "local history that is not a list does not take the lobby out",
  async (page) => {
    session = { signedIn: false };
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.addInitScript(() =>
      localStorage.setItem("codetrial_history", '{"a":1}'),
    );
    await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
    await awaitReady(page);

    const state = await snapshot(page);
    assert.equal(
      state.startDisabled,
      false,
      "the lobby was left with no way to start",
    );
    assert.ok(state.card, "nothing was recommended");
    assert.deepEqual(errors, [], `it threw: ${errors[0]}`);
    assert.equal(state.history, "", "it was counted as past reports");
  },
);

lobbyTest(
  "a server that cannot answer at all still leaves a usable lobby",
  async (page) => {
    // Both endpoints 500, which is what a half-deployed or failing server looks
    // like from here. Nothing is recommended until the history settles, so a
    // history that never arrives has to settle as "none" rather than as a wait
    // with no end and a button that never comes back.
    session = null;
    reports = null;
    failing = new Set(["/api/session", "/api/reports"]);
    await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
    await awaitReady(page);

    const state = await snapshot(page);
    assert.ok(
      state.card,
      "a lobby whose history never arrived recommended nothing at all",
    );
    assert.equal(
      state.startDisabled,
      false,
      "the lobby waited forever on a fetch that failed",
    );
  },
);

lobbyTest(
  "a signed-in candidate whose reports fail still gets a problem",
  async (page) => {
    // The half that is easy to miss: the session answers, so the account row is
    // right, and only the history is missing. Without a recommendation on this
    // path the candidate is signed in and cannot start.
    failing = new Set(["/api/reports"]);
    await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
    await awaitReady(page);

    const state = await snapshot(page);
    assert.match(state.account, /Signed in as candidate/);
    assert.ok(state.card, "a failed report fetch left nothing to start");
    assert.equal(state.startDisabled, false);
  },
);

lobbyTest(
  "changing the level before the reports land waits for them",
  async (page) => {
    // Both Hard problems in the bank that this test can reach are passed, so a
    // recommendation made from an empty history would offer one of them back.
    reports = [hired(HARD[0]), hired(HARD[1])];
    const release = await heldLobby(page);

    // Checked while the reports are still in flight.
    await setLevel(page, "Hard", true);
    await setLevel(page, "Medium", false);
    const early = await snapshot(page);
    assert.equal(
      early.card,
      null,
      "a problem was chosen from a history that had not arrived",
    );
    assert.equal(
      early.startDisabled,
      true,
      "start went live before the reports did",
    );

    release();
    await awaitReady(page);
    const settled = await snapshot(page);
    assert.deepEqual(
      settled.levels,
      ["Hard"],
      "the level the candidate chose did not survive",
    );
    assert.ok(settled.card, "nothing was recommended once the reports arrived");
    assert.ok(
      ![HARD[0], HARD[1]].includes(settled.card),
      "a problem the candidate already passed was recommended",
    );
  },
);

lobbyTest(
  "a start ships the length that was on screen when it was pressed",
  async (page) => {
    // The duration row stays live while the sign-in round trip is in flight, so
    // the handler has to have taken the length with it. Reading the global after
    // the await shipped whatever the candidate landed on in the meantime.
    session = { signedIn: false, loginRequired: true };
    await lobby(page);
    const pressed = (await snapshot(page)).duration;

    const finishLogin = (() => {
      let done;
      holdLogin = new Promise((resolve) => (done = resolve));
      return done;
    })();
    await page.fill("#github-login", "candidate");
    await page.click("#start");
    await page.click('[data-duration="60"]');
    finishLogin();

    await page.waitForURL(/\/interview/);
    assert.equal(
      new URL(page.url()).searchParams.get("duration"),
      pressed,
      "start shipped a different length than the one showing when it was pressed",
    );
  },
);

lobbyTest(
  "a start ships the grounding snippets that were checked when it was pressed",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await lobby(page);
    await page.click("details.interview-context summary");
    const txt = (name, text) => ({
      name,
      mimeType: "text/plain",
      buffer: Buffer.from(text),
    });

    await page.setInputFiles(
      "#grounding-jd",
      txt("first.txt", "Must know Rust"),
    );
    await page
      .locator('#grounding-choices input[data-group="requirements"]')
      .first()
      .check();
    await page.check("#grounding-consent");

    const finishLogin = (() => {
      let done;
      holdLogin = new Promise((resolve) => (done = resolve));
      return done;
    })();
    await page.fill("#github-login", "candidate");
    await page.click("#start");
    await page.click("#grounding-clear");
    await page.setInputFiles(
      "#grounding-jd",
      txt("second.txt", "Must know SQL"),
    );
    await reaches(page, () =>
      document
        .querySelector("#grounding-choices label")
        ?.textContent.includes("SQL"),
    );
    await page
      .locator('#grounding-choices input[data-group="requirements"]')
      .first()
      .check();
    await page.check("#grounding-consent");
    finishLogin();

    await page.waitForURL(/\/interview/);
    const stored = await page.evaluate(() =>
      JSON.parse(sessionStorage.getItem("codetrial.interview-grounding.v1")),
    );
    assert.deepEqual(stored?.requirements, ["Must know Rust"]);
  },
);

lobbyTest(
  "consent withdrawn during the sign-in wait stores no grounding",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await lobby(page);
    await page.click("details.interview-context summary");
    await page.setInputFiles("#grounding-jd", {
      name: "jd.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("Must know Rust"),
    });
    await page
      .locator('#grounding-choices input[data-group="requirements"]')
      .first()
      .check();
    await page.check("#grounding-consent");

    const finishLogin = (() => {
      let done;
      holdLogin = new Promise((resolve) => (done = resolve));
      return done;
    })();
    await page.fill("#github-login", "candidate");
    await page.click("#start");
    await page.uncheck("#grounding-consent");
    finishLogin();

    await page.waitForURL(/\/interview/);
    const stored = await page.evaluate(() =>
      sessionStorage.getItem("codetrial.interview-grounding.v1"),
    );
    assert.equal(stored, null);
  },
);

lobbyTest(
  "a start the grounding packet refuses does not sign in first",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await lobby(page);
    await page.click("details.interview-context summary");
    await page.setInputFiles("#grounding-jd", {
      name: "jd.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("Must know Rust"),
    });
    await page
      .locator('#grounding-choices input[data-group="requirements"]')
      .first()
      .check();

    await page.fill("#github-login", "candidate");
    await page.click("#start");
    await reaches(page, () =>
      document.querySelector("#grounding-error").textContent.includes("Agree"),
    );
    assert.equal(
      session.signedIn,
      false,
      "no /api/login for a start that was never going to go",
    );
    assert.equal(await page.isDisabled("#start"), false);
  },
);

lobbyTest(
  "a start already on its way out is not undone by a later choice",
  async (page) => {
    // The sign-in round trip is the one window where the start handler is
    // suspended with the page still live under it, and everything the candidate
    // can touch during it writes the two things that handler is holding: which
    // problem is chosen, and whether the button is armed.
    session = { signedIn: false, loginRequired: true };
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const release = await heldLobby(page);

    await page.click("details.problem-picker summary");
    await page.click(`[data-problem="${pageOf("jump-game")}"]`);
    await page.fill("#github-login", "candidate");

    // Held rather than slowed: everything below lands while /api/login is in
    // flight because the stub is waiting on this, not because 400ms was long
    // enough to win a race.
    const finishLogin = (() => {
      let done;
      holdLogin = new Promise((resolve) => (done = resolve));
      return done;
    })();
    await page.click("#start");

    // A normal pageshow may finish after Start was pressed. Its selection
    // sync must leave the pending start locked, without rechecking the account.
    await normalPageshow(page);
    assert.equal(await page.isDisabled("#start"), true);
    assert.equal(await page.isDisabled("#selection-mode"), true);

    // A second card while the button reads "Recording GitHub...": re-arming it
    // here buys the candidate a second navigation out of one start.
    await page.click(`[data-problem="${pageOf("gas-station")}"]`);
    assert.equal(
      (await snapshot(page)).startDisabled,
      true,
      "a card click re-armed a start button that was already leaving",
    );

    // And a filter that drops the pick clears it, so a handler that read the
    // problem after its await rather than before had nothing left to read an id
    // off: it threw, and the button stayed down on "Recording GitHub..." for good.
    await setLevel(page, "Hard", true);
    await setLevel(page, "Medium", false);

    // The surface too: the destination was built before the await, so a
    // button lit now would name a mode the interview is not opened in.
    await page.click('[data-mode="whiteboard"]');
    assert.equal(
      await page.getAttribute('[data-mode="coding"]', "aria-pressed"),
      "true",
      "a mode click took effect on a start that had already read its mode",
    );

    finishLogin();
    await page.waitForURL(/\/interview/);
    assert.equal(new URL(page.url()).searchParams.get("mode"), "coding");
    assert.deepEqual(
      errors,
      [],
      "the start handler threw after the login came back",
    );
    assert.equal(
      new URL(page.url()).searchParams.get("problem"),
      pageOf("jump-game"),
      "start shipped something other than the problem that was on screen when it was pressed",
    );
    release();
  },
);

lobbyTest(
  "a length this deployment cannot record is disabled, and says why",
  async (page) => {
    // The cap the server reports, which is `DEFAULT_RECORDING_MAX_MINUTES` on a
    // default deployment. Sixty is the length the row offers past it, and
    // `stale_after` reaps that recording ten minutes before the interview ends.
    session = {
      signedIn: true,
      user: { login: "candidate" },
      maxDurationMin: 45,
    };

    const state = await lobby(page);

    assert.deepEqual(
      state.durationsOff,
      ["60"],
      "a length past the recording cap was still on offer",
    );
    assert.match(
      state.durationNote,
      /at most 45 minutes/,
      "the disabled button gave no reason",
    );
    assert.equal(
      state.duration,
      "45",
      "the cap moved a length that was already under it",
    );
  },
);

lobbyTest(
  "the recording cap outranks the length the markup preselected",
  async (page) => {
    // Nothing else here overrules a length that is already set. This does,
    // because it is not a second opinion about what suits the candidate: it is
    // what this server is able to record.
    session = {
      signedIn: true,
      user: { login: "candidate" },
      maxDurationMin: 30,
    };

    const state = await lobby(page);

    assert.equal(
      state.duration,
      "30",
      "the lobby kept a length longer than the recording",
    );
    assert.deepEqual(state.durationsOff, ["45", "60"]);
  },
);

lobbyTest(
  "a server that can record every length on offer disables nothing",
  async (page) => {
    session = {
      signedIn: true,
      user: { login: "candidate" },
      maxDurationMin: 90,
    };

    const state = await lobby(page);

    assert.deepEqual(state.durationsOff, []);
    assert.equal(
      state.durationNote,
      "",
      "a lobby with nothing capped explained a cap anyway",
    );
    assert.equal(state.duration, markupDuration());
  },
);

lobbyTest(
  "saved reports require confirmation and clear the progress panel",
  async (page) => {
    reports = [savedAttempt(EASY[0]), savedAttempt(EASY[1])];
    await lobby(page);
    assert.match(
      await page.locator("#recommendation").textContent(),
      /passed your last two Easy problems/,
    );

    page.once("dialog", (dialog) => dialog.dismiss());
    await page.click("#delete-reports");
    assert.equal(deleteRequests, 0);
    assert.equal(await page.locator("#history").isHidden(), false);

    page.once("dialog", (dialog) => {
      assert.match(dialog.message(), /reports and progress/);
      assert.match(
        dialog.message(),
        /Recording files follow their separate retention policy/,
      );
      return dialog.accept();
    });
    await page.click("#delete-reports");
    await reaches(page, () => document.querySelector("#history").hidden);
    assert.equal(deleteRequests, 1);
    assert.deepEqual(reports, []);
    assert.equal(await page.locator("#history").isHidden(), true);
    assert.equal(await page.locator("#delete-reports").isHidden(), true);
    assert.doesNotMatch(
      await page.locator("#recommendation").textContent(),
      /passed your last two/,
    );
    assert.equal(
      await page.locator("#report-delete-status").textContent(),
      "Saved reports and progress were deleted.",
    );
    assert.equal(
      await page.evaluate(() => document.activeElement.id),
      "report-delete-status",
    );
  },
);

lobbyTest(
  "a filtered attempt list deletes the report the row shows",
  async (page) => {
    const attempt = (id, language, day) => ({
      id,
      problemId: EASY[0],
      payload: {
        id,
        problemId: EASY[0],
        problemTitle: `Attempt ${id}`,
        language,
        date: `2026-01-0${day}T00:00:00Z`,
        report: { decision: "HIRE" },
      },
    });
    reports = [
      attempt("a", "python", 1),
      attempt("b", "cpp", 2),
      attempt("c", "python", 3),
    ];
    await lobby(page);
    await page.selectOption("#progress-language", "python");
    const rows = () =>
      page.locator("#attempt-history > li > p").allTextContents();
    assert.equal((await rows()).length, 2);
    // Newest first, so the first row shown is the third report stored and the
    // first report stored is not on screen at all.
    const first = page
      .locator("#attempt-history > li")
      .first()
      .getByRole("button", { name: /^Delete the / });

    page.once("dialog", (dialog) => dialog.dismiss());
    await first.click();
    assert.deepEqual(deletedIds, []);

    page.once("dialog", (dialog) => {
      assert.match(dialog.message(), /report for Attempt c\?/);
      return dialog.accept();
    });
    await first.click();
    await reaches(
      page,
      () => document.querySelector("#report-delete-status").textContent !== "",
    );
    assert.deepEqual(deletedIds, ["c"]);
    assert.deepEqual(
      reports.map((report) => report.id),
      ["a", "b"],
    );
    assert.equal(
      await page.locator("#progress-language").inputValue(),
      "python",
      "the filter outlived the reload",
    );
    const left = await rows();
    assert.equal(left.length, 1);
    assert.match(left[0], /Attempt a/);
    assert.equal(
      await page.locator("#report-delete-status").textContent(),
      "The report was deleted.",
    );
    assert.equal(
      deleteRequests,
      0,
      "one report deleted was every report deleted",
    );
  },
);

lobbyTest(
  "two attempts on one day have Delete buttons that name different reports",
  async (page) => {
    const attempt = (id, time) => ({
      id,
      problemId: EASY[0],
      payload: {
        id,
        problemId: EASY[0],
        problemTitle: "Same problem",
        language: "python",
        date: `2026-01-01T${time}:00Z`,
        report: { decision: "HIRE" },
      },
    });
    reports = [attempt("morning", "09:00"), attempt("evening", "18:30")];
    await lobby(page);
    const names = await page
      .locator("#attempt-history [data-delete-report]")
      .evaluateAll((buttons) =>
        buttons.map((button) => button.getAttribute("aria-label")),
      );
    assert.equal(names.length, 2);
    assert.notEqual(names[0], names[1]);
  },
);

lobbyTest(
  "deleting a row leaves focus in the list and announces the result",
  async (page) => {
    const attempt = (id, day) => ({
      id,
      problemId: EASY[0],
      payload: {
        id,
        problemId: EASY[0],
        problemTitle: `Attempt ${id}`,
        language: "python",
        date: `2026-01-0${day}T00:00:00Z`,
        report: { decision: "HIRE" },
      },
    });
    reports = [attempt("a", 1), attempt("b", 2), attempt("c", 3)];
    await lobby(page);
    page.once("dialog", (dialog) => dialog.accept());
    // Newest first, so the second row shown is "b" and "a" takes its place.
    await page.locator("#attempt-history [data-delete-report]").nth(1).click();
    await reaches(
      page,
      () => document.querySelector("#report-delete-status").textContent !== "",
    );
    assert.deepEqual(deletedIds, ["b"]);
    assert.equal(
      await page.locator("#report-delete-status").getAttribute("role"),
      "status",
    );
    assert.match(
      await page.evaluate(() =>
        document.activeElement.getAttribute("aria-label"),
      ),
      /report for Attempt a$/,
    );
  },
);

lobbyTest(
  "a device row still deletes after the first session check failed",
  async (page) => {
    session = { signedIn: false };
    await page.addInitScript(
      (entry) => {
        if (!localStorage.getItem("codetrial_history")) {
          localStorage.setItem("codetrial_history", JSON.stringify([entry]));
        }
      },
      {
        id: "local",
        problemId: EASY[0],
        date: "2026-01-04T00:00:00Z",
        report: { decision: "HIRE" },
      },
    );
    failing.add("/api/session");
    await lobby(page);
    // One flaky call at load, answered on the recheck: the list drawn was this
    // device's, and that is the copy there is to delete.
    failing.delete("/api/session");
    page.once("dialog", (dialog) => dialog.accept());
    await page.locator("#attempt-history [data-delete-report]").click();
    await reaches(
      page,
      () => document.querySelector("#report-delete-status").textContent !== "",
    );
    assert.equal(
      await page.locator("#report-delete-status").textContent(),
      "The report was deleted.",
    );
    assert.deepEqual(
      await page.evaluate(() =>
        JSON.parse(localStorage.getItem("codetrial_history")),
      ),
      [],
    );
  },
);

lobbyTest(
  "a back/forward restore holds the row deletes down until the lobby reloads",
  async (page) => {
    reports = [
      {
        ...savedAttempt(EASY[0]),
        id: "kept",
        payload: { ...savedAttempt(EASY[0]).payload, id: "kept" },
      },
    ];
    await lobby(page);
    const remove = page.locator("#attempt-history [data-delete-report]");
    assert.equal(await remove.isDisabled(), false);
    const release = holdHistory();
    await restore(page);
    assert.equal(await remove.isDisabled(), true);
    release();
    await awaitReady(page);
    assert.equal(await remove.isDisabled(), false);
  },
);

lobbyTest(
  "an account delete this device could not follow shows the copy it kept",
  async (page) => {
    const entry = {
      id: "both",
      problemId: EASY[0],
      date: "2026-01-03T00:00:00Z",
      report: { decision: "HIRE" },
    };
    reports = [{ id: "both", problemId: EASY[0], payload: entry }];
    await page.addInitScript((saved) => {
      localStorage.setItem("codetrial_history", JSON.stringify([saved]));
      const setItem = Storage.prototype.setItem;
      Storage.prototype.setItem = function writeHistory(key, value) {
        if (key === "codetrial_history") throw new Error("blocked");
        return setItem.call(this, key, value);
      };
    }, entry);
    await lobby(page);
    page.once("dialog", (dialog) => dialog.accept());
    await page.locator("#attempt-history [data-delete-report]").click();
    await reaches(
      page,
      () => document.querySelector("#report-delete-status").textContent !== "",
    );
    assert.deepEqual(reports, []);
    assert.equal(
      await page.locator("#report-delete-status").textContent(),
      "The report was deleted from your account, but its copy on this device could not be deleted.",
    );
    assert.match(
      await page.locator("#progress-summary").textContent(),
      /saved on this device/,
    );
    assert.equal(await page.locator("#attempt-history > li").count(), 1);
  },
);

lobbyTest("a row without an id offers no delete", async (page) => {
  reports = [savedAttempt(EASY[0])];
  await lobby(page);
  assert.equal(
    await page
      .locator("#attempt-history")
      .getByRole("button", { name: /^Delete/ })
      .count(),
    0,
  );
});

lobbyTest(
  "a phase row lists three weaknesses and folds the rest where they can still be read",
  async (page) => {
    const ranked = [
      "Explain the algorithm before coding",
      "Justify time complexity",
      "Justify space complexity",
      "Name an alternative",
    ];
    reports = [
      focusedAttempt(EASY[0], {
        phase: "Algorithm",
        weaknesses: ranked,
        assessed: true,
      }),
    ];
    await lobby(page);
    await reaches(page, () =>
      document.querySelector("#progress-weaknesses > li"),
    );

    const row = await page.evaluate(() => {
      const item = document.querySelector("#progress-weaknesses > li");
      const list = document.querySelector("#progress-weaknesses");
      return {
        note: document
          .getElementById(list.getAttribute("aria-describedby"))
          ?.textContent.trim()
          .slice(0, 16),
        heading: item.querySelector(":scope > p").textContent,
        shown: [...item.querySelectorAll(":scope > ul > li")].map(
          (node) => node.textContent,
        ),
        summary: item.querySelector(":scope > details > summary")?.textContent,
        folded: [...item.querySelectorAll(":scope > details > ul > li")].map(
          (node) => node.textContent,
        ),
      };
    });
    assert.deepEqual(
      row,
      {
        note: "Each row counts ",
        heading: "REACTO · Algorithm · flagged in 1 of 1 assessed attempt",
        shown: ranked.slice(0, 3),
        summary: "1 more weakness",
        folded: ranked.slice(3),
      },
      "the report's highest-ranked weakness is shown, and the notice is not one more list item",
    );
  },
);

lobbyTest(
  "a failed report deletion retains the current progress",
  async (page) => {
    reports = [savedAttempt(EASY[0])];
    await lobby(page);
    failing.add("/api/reports");
    page.once("dialog", (dialog) => dialog.accept());

    await page.click("#delete-reports");
    await reaches(
      page,
      () => document.querySelector("#report-delete-status").textContent !== "",
    );

    assert.equal(deleteRequests, 1);
    assert.equal(await page.locator("#history").isHidden(), false);
    assert.match(
      await page.locator("#progress-summary").textContent(),
      /1 of 1 attempts shown/,
    );
    assert.equal(
      await page.locator("#report-delete-status").textContent(),
      "Could not delete saved reports. Your reports may not have been removed.",
    );
  },
);

lobbyTest(
  "a signed-in account can erase reports saved only on this device",
  async (page) => {
    await page.addInitScript(
      (entry) => {
        localStorage.setItem("codetrial_history", JSON.stringify([entry]));
      },
      {
        problemId: EASY[0],
        date: "2026-01-03T00:00:00Z",
        report: { decision: "HIRE" },
      },
    );
    await lobby(page);
    assert.equal(await page.locator("#history").isHidden(), true);
    assert.equal(await page.locator("#delete-reports").isVisible(), true);
    page.once("dialog", (dialog) => dialog.accept());

    await page.click("#delete-reports");
    await reaches(
      page,
      () => document.querySelector("#report-delete-status").textContent !== "",
    );

    assert.equal(
      await page.evaluate(() => localStorage.getItem("codetrial_history")),
      null,
    );
    assert.equal(await page.locator("#delete-reports").isHidden(), true);
  },
);

lobbyTest(
  "an unavailable account check retains visible local reports",
  async (page) => {
    await page.addInitScript(
      (entry) => {
        localStorage.setItem("codetrial_history", JSON.stringify([entry]));
      },
      {
        problemId: EASY[0],
        date: "2026-01-04T00:00:00Z",
        report: { decision: "HIRE" },
      },
    );
    failing.add("/api/session");
    await lobby(page);
    page.once("dialog", (dialog) => dialog.accept());

    await page.click("#delete-reports");
    await reaches(
      page,
      () => document.querySelector("#report-delete-status").textContent !== "",
    );

    assert.notEqual(
      await page.evaluate(() => localStorage.getItem("codetrial_history")),
      null,
    );
    assert.equal(
      await page.locator("#report-delete-status").textContent(),
      "Could not delete saved reports. Your reports may not have been removed.",
    );
  },
);

lobbyTest(
  "a partial clear stays visible when local storage cannot be read",
  async (page) => {
    reports = [savedAttempt(EASY[0])];
    await page.addInitScript(() => {
      const getItem = Storage.prototype.getItem;
      const removeItem = Storage.prototype.removeItem;
      Storage.prototype.getItem = function readHistory(key) {
        if (key === "codetrial_history") throw new Error("blocked");
        return getItem.call(this, key);
      };
      Storage.prototype.removeItem = function removeHistory(key) {
        if (key === "codetrial_history") throw new Error("blocked");
        return removeItem.call(this, key);
      };
    });
    await lobby(page);
    page.once("dialog", (dialog) => dialog.accept());

    await page.click("#delete-reports");
    await reaches(
      page,
      () => document.querySelector("#report-delete-status").textContent !== "",
    );

    assert.equal(await page.locator("#history").isHidden(), true);
    assert.equal(await page.locator("#report-delete-status").isVisible(), true);
    assert.equal(
      await page.locator("#report-delete-status").textContent(),
      "Account reports were deleted, but reports saved on this device could not be deleted.",
    );
  },
);

lobbyTest(
  "blocked local storage does not prevent account deletion",
  async (page) => {
    reports = [savedAttempt(EASY[0])];
    await page.addInitScript(() => {
      Object.defineProperty(window, "localStorage", {
        configurable: true,
        get() {
          throw new DOMException("blocked", "SecurityError");
        },
      });
    });
    await lobby(page);
    page.once("dialog", (dialog) => dialog.accept());

    await page.click("#delete-reports");
    await reaches(
      page,
      () => document.querySelector("#report-delete-status").textContent !== "",
    );

    assert.equal(deleteRequests, 1);
    assert.deepEqual(reports, []);
    assert.equal(
      await page.locator("#report-delete-status").textContent(),
      "Account reports were deleted, but reports saved on this device could not be deleted.",
    );
    assert.equal(
      await page.evaluate(() => document.activeElement.id),
      "report-delete-status",
    );
  },
);

lobbyTest(
  "a partial clear reloads local reports and states what remains",
  async (page) => {
    reports = [savedAttempt(EASY[0])];
    await page.addInitScript(
      (entry) => {
        localStorage.setItem("codetrial_history", JSON.stringify([entry]));
        const removeItem = Storage.prototype.removeItem;
        Storage.prototype.removeItem = function removeHistory(key) {
          if (key === "codetrial_history") throw new Error("blocked");
          return removeItem.call(this, key);
        };
      },
      {
        problemId: EASY[1],
        date: "2026-01-02T00:00:00Z",
        report: { decision: "HIRE" },
      },
    );
    await lobby(page);
    page.once("dialog", (dialog) => dialog.accept());

    await page.click("#delete-reports");
    await reaches(
      page,
      () => document.querySelector("#report-delete-status").textContent !== "",
    );

    assert.equal(deleteRequests, 1);
    assert.deepEqual(reports, []);
    assert.match(
      await page.locator("#progress-summary").textContent(),
      /saved on this device/,
    );
    assert.equal(
      await page.locator("#report-delete-status").textContent(),
      "Account reports were deleted, but reports saved on this device could not be deleted.",
    );
    assert.equal(
      await page.evaluate(() => document.activeElement.id),
      "report-delete-status",
    );
  },
);

lobbyTest(
  "a cap under every length keeps the row usable and explains the effective limit",
  async (page) => {
    // Keep the page usable even when the server limit fits no duration button.
    session = {
      signedIn: true,
      user: { login: "candidate" },
      maxDurationMin: 15,
    };

    const state = await lobby(page);

    assert.deepEqual(state.durationsOff, []);
    assert.equal(state.duration, null);
    assert.match(state.durationNote, /limited to 15 minutes/);
  },
);

test("a failed history load leaves no other account's attempts behind", () => {
  // reports and progressNormalized outlive the panel: recommendations and
  // every filter change read them again. Clearing only the DOM left the lobby
  // answering from whichever history it had last loaded successfully, which
  // after a sign-out is a different person's.
  const shown = functionBody(read("web/app.js"), "showProgressError");
  assert.match(shown, /reports = \[\]/);
  assert.match(shown, /progressNormalized = \[\]/);
});

lobbyTest(
  "Enter in the GitHub field submits the account choice",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await lobby(page);
    await page.fill("#github-login", "@candidate");
    const login = page.waitForRequest((request) =>
      request.url().endsWith("/api/login"),
    );
    await page.press("#github-login", "Enter");
    assert.deepEqual((await login).postDataJSON(), { login: "candidate" });
    await page.waitForFunction(
      () =>
        document.querySelector("#account-status").textContent ===
        "Signed in as candidate",
    );
  },
);

lobbyTest(
  "composition Enter does not submit a GitHub username",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    let release;
    holdLogin = new Promise((resolve) => (release = resolve));
    try {
      await lobby(page);
      await page.fill("#github-login", "composing");
      const login = page.waitForRequest((request) =>
        request.url().endsWith("/api/login"),
      );
      await page.evaluate(() =>
        document.querySelector("#github-login").dispatchEvent(
          new KeyboardEvent("keydown", {
            key: "Enter",
            isComposing: true,
            bubbles: true,
          }),
        ),
      );
      // Hold the response so an early submit fails on its payload rather than
      // hiding the field while the test is still entering the committed name.
      await page.fill("#github-login", "candidate");
      await page.press("#github-login", "Enter");
      assert.deepEqual((await login).postDataJSON(), { login: "candidate" });
      release();
      await page.waitForFunction(
        () =>
          document.querySelector("#account-status").textContent ===
          "Signed in as candidate",
      );
    } finally {
      release();
    }
  },
);

lobbyTest(
  "composition-end Enter does not submit a GitHub username",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    let release;
    holdLogin = new Promise((resolve) => (release = resolve));
    try {
      await lobby(page);
      await page.fill("#github-login", "composing");
      const login = page.waitForRequest((request) =>
        request.url().endsWith("/api/login"),
      );
      await page.evaluate(() => {
        const input = document.querySelector("#github-login");
        input.dispatchEvent(
          new CompositionEvent("compositionend", {
            data: "composing",
            bubbles: true,
          }),
        );
        input.dispatchEvent(
          new KeyboardEvent("keydown", {
            key: "Enter",
            keyCode: 229,
            isComposing: false,
            bubbles: true,
          }),
        );
      });
      await page.fill("#github-login", "candidate");
      await page.press("#github-login", "Enter");
      assert.deepEqual((await login).postDataJSON(), { login: "candidate" });
      release();
      await page.waitForFunction(
        () =>
          document.querySelector("#account-status").textContent ===
          "Signed in as candidate",
      );
    } finally {
      release();
    }
  },
);

lobbyTest(
  "Enter does not submit again while the GitHub choice is pending",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    let release;
    holdLogin = new Promise((resolve) => (release = resolve));
    const submitted = [];
    page.on("request", (request) => {
      if (request.url().endsWith("/api/login")) submitted.push(request);
    });
    try {
      await lobby(page);
      await page.fill("#github-login", "candidate");
      const login = page.waitForRequest((request) =>
        request.url().endsWith("/api/login"),
      );
      await page.press("#github-login", "Enter");
      await login;
      assert.equal(await page.locator("#login-link").isDisabled(), true);
      await page.press("#github-login", "Enter");
      await page.press("#github-login", "Enter");
      release();
      await page.waitForFunction(
        () =>
          document.querySelector("#account-status").textContent ===
          "Signed in as candidate",
      );
      assert.equal(submitted.length, 1);
    } finally {
      release();
    }
  },
);

lobbyTest(
  "a pending start owns the GitHub login until the interview opens",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    let release;
    holdLogin = new Promise((resolve) => (release = resolve));
    const submitted = [];
    page.on("request", (request) => {
      if (request.url().endsWith("/api/login")) submitted.push(request);
    });
    try {
      const selected = await lobby(page);
      await page.evaluate(() => {
        const fetch = window.fetch;
        window.loginRequests = 0;
        window.fetch = (...args) => {
          if (args[0] === "/api/login") window.loginRequests += 1;
          return fetch(...args);
        };
      });
      await page.fill("#github-login", "candidate");
      const login = page.waitForRequest((request) =>
        request.url().endsWith("/api/login"),
      );
      await page.click("#start");
      await login;
      await page.press("#github-login", "Enter");
      // Both keyboard and mouse use the same header login handler.
      await page.evaluate(() => document.querySelector("#login-link").click());
      assert.equal(
        await page.evaluate(() => window.loginRequests),
        1,
        "the header sent another login while Start was pending",
      );
      release();
      await page.waitForURL(/\/interview/);
      assert.equal(
        submitted.length,
        1,
        "the header sent another login while Start was pending",
      );
      assert.equal(
        new URL(page.url()).searchParams.get("problem"),
        selected.card,
      );
    } finally {
      release();
    }
  },
);

lobbyTest(
  "a pending GitHub login prevents Start from sending another login",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    let release;
    holdLogin = new Promise((resolve) => (release = resolve));
    const submitted = [];
    const interviews = [];
    page.on("request", (request) => {
      if (request.url().endsWith("/api/login")) submitted.push(request);
      if (new URL(request.url()).pathname === "/interview")
        interviews.push(request);
    });
    try {
      await lobby(page);
      await page.evaluate(() => {
        const fetch = window.fetch;
        window.loginRequests = 0;
        window.fetch = (...args) => {
          if (args[0] === "/api/login") window.loginRequests += 1;
          return fetch(...args);
        };
      });
      await page.fill("#github-login", "candidate");
      const login = page.waitForRequest((request) =>
        request.url().endsWith("/api/login"),
      );
      await page.press("#github-login", "Enter");
      await login;
      await page.evaluate(() => document.querySelector("#start").click());
      await page.click("details.problem-picker summary");
      await page.click(`[data-problem="${pageOf("gas-station")}"]`);
      await page.evaluate(() => document.querySelector("#start").click());
      assert.equal(
        await page.evaluate(() => window.loginRequests),
        1,
        "Start sent another login while the header login was pending",
      );
      release();
      await page.waitForFunction(
        () =>
          document.querySelector("#account-status").textContent ===
          "Signed in as candidate",
      );
      assert.equal(
        submitted.length,
        1,
        "Start sent another login while the header login was pending",
      );
      assert.equal(
        interviews.length,
        0,
        "Start raced the header login with an interview navigation",
      );
    } finally {
      release();
    }
  },
);

lobbyTest(
  "a successful GitHub login stays busy through the account and history refresh",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await lobby(page);
    await page.evaluate(() => {
      const fetch = window.fetch;
      window.loginRequests = 0;
      window.fetch = (...args) => {
        if (args[0] === "/api/login") window.loginRequests += 1;
        return fetch(...args);
      };
    });
    const submitted = [];
    page.on("request", (request) => {
      if (request.url().endsWith("/api/login")) submitted.push(request);
    });
    let releaseAccount;
    const holdAccount = new Promise((resolve) => (releaseAccount = resolve));
    const releaseReports = holdHistory();
    await page.route(`${base}/api/session`, async (route) => {
      await holdAccount;
      await route.continue();
    });
    const assertBusy = async () => {
      for (const selector of [
        "#start",
        "#random-problem",
        "#delete-reports",
        "#login-link",
      ]) {
        assert.equal(
          await page.locator(selector).isDisabled(),
          true,
          `${selector} was live during the account refresh`,
        );
      }
      await page.evaluate(() => {
        document.querySelector("#login-link").click();
        document.querySelector("#start").click();
      });
    };
    try {
      await page.fill("#github-login", "candidate");
      const account = page.waitForRequest((request) =>
        request.url().endsWith("/api/session"),
      );
      await page.press("#github-login", "Enter");
      await account;
      await assertBusy();
      await page.click("details.problem-picker summary");
      await page.click(`[data-problem="${pageOf("gas-station")}"]`);
      assert.equal(
        await page.locator("#start").isDisabled(),
        true,
        "a card click re-armed Start during the account refresh",
      );
      const history = page.waitForRequest((request) =>
        request.url().endsWith("/api/reports"),
      );
      releaseAccount();
      await history;
      await assertBusy();
      releaseReports();
      await awaitReady(page);
      assert.equal(await page.locator("#start").isDisabled(), false);
      assert.equal(await page.locator("#random-problem").isDisabled(), false);
      assert.equal(await page.locator("#login-link").isHidden(), true);
      const loginRequests = await page.evaluate(() => {
        document
          .querySelector("#github-login")
          .dispatchEvent(
            new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
          );
        document.querySelector("#login-link").click();
        return window.loginRequests;
      });
      assert.equal(
        loginRequests,
        1,
        "a queued input submitted the hidden login button again",
      );
      assert.equal(
        submitted.length,
        1,
        "another login was sent during the account refresh",
      );
      assert.equal(new URL(page.url()).pathname, "/");
    } finally {
      releaseAccount();
      releaseReports();
    }
  },
);

lobbyTest("a failed GitHub login can be retried with Enter", async (page) => {
  session = { signedIn: false, loginRequired: true };
  failing.add("/api/login");
  await lobby(page);
  await page.fill("#github-login", "candidate");
  const failure = page.waitForResponse(
    (response) =>
      response.url().endsWith("/api/login") && response.status() === 500,
  );
  await page.press("#github-login", "Enter");
  await failure;
  await reaches(
    page,
    () =>
      document.querySelector("#account-status").textContent ===
      "Could not record GitHub username.",
  );
  assert.equal(
    await page.locator("#login-link").isDisabled(),
    false,
    "a failed login left the button disabled",
  );
  failing.delete("/api/login");
  const retry = page.waitForRequest((request) =>
    request.url().endsWith("/api/login"),
  );
  await page.press("#github-login", "Enter");
  assert.deepEqual((await retry).postDataJSON(), { login: "candidate" });
  await page.waitForFunction(
    () =>
      document.querySelector("#account-status").textContent ===
      "Signed in as candidate",
  );
});

for (const trigger of ["Enter", "button"]) {
  lobbyTest(
    `${trigger} login refreshes the account without losing interview choices`,
    async (page) => {
      session = { signedIn: false, loginRequired: true };
      await page.addInitScript((entry) => {
        localStorage.setItem("codetrial_history", JSON.stringify([entry]));
      }, focusedAttempt(EASY[0]).payload);
      await lobby(page);
      await page.check("#practice-focus-share-input");
      await page.click("details.problem-picker summary");
      await setLevel(page, "Hard", true);
      await page.click(`[data-problem="${MEDIUM[1]}"]`);
      await page.click('[data-duration="60"]');
      await page.click('[data-loop="coding_only"]');
      await page.click("details.interview-context summary");
      await page.fill("#profile-role", "Backend engineer");
      await page.selectOption("#profile-seniority", "senior");
      await page.fill("#profile-company", "Example");
      await page.setInputFiles(
        "#grounding-jd",
        groundingTxt("jd.txt", "Must know Rust\nMust know SQL"),
      );
      await page
        .locator('#grounding-choices input[data-group="requirements"]')
        .nth(1)
        .check();
      await page.setInputFiles(
        "#grounding-resume",
        groundingTxt("resume.txt", "Skills: Rust, Go\nBuilt a parser"),
      );
      await page
        .locator('#grounding-choices input[data-group="skills"]')
        .first()
        .check();
      await page
        .locator('#grounding-choices input[data-group="anchors"]')
        .first()
        .check();
      await page.check("#grounding-consent");
      const choices = await page
        .locator("#grounding-choices input")
        .evaluateAll((inputs) => inputs.map((input) => input.checked));
      const selected = await snapshot(page);
      await page.evaluate(() => {
        window.loginPageMarker = "same document";
      });
      const updated = focusedAttempt(MEDIUM[0]);
      updated.payload.report.improvementPlan[0].weakness = "Explain invariants";
      reports = [updated];
      await page.fill("#github-login", "candidate");
      if (trigger === "Enter") await page.press("#github-login", "Enter");
      else await page.click("#login-link");
      await reaches(
        page,
        () =>
          !document.querySelector("#start").disabled &&
          document.querySelector("#account-status").textContent ===
            "Signed in as candidate",
      );
      assert.equal(
        await page.evaluate(() => window.loginPageMarker),
        "same document",
        "login replaced the document",
      );
      const signedIn = await snapshot(page);
      assert.equal(signedIn.account, "Signed in as candidate");
      assert.equal(signedIn.card, selected.card);
      assert.equal(signedIn.duration, selected.duration);
      assert.deepEqual(signedIn.levels, selected.levels);
      assert.match(signedIn.history, /saved to your account/);
      assert.match(signedIn.focus, /Explain invariants/);
      assert.equal(
        await page.locator("#practice-focus-share-input").isChecked(),
        false,
        "a new practice focus inherited consent to the old text",
      );
      assert.equal(await page.locator("#grounding-consent").isChecked(), true);
      assert.deepEqual(
        await page
          .locator("#grounding-choices input")
          .evaluateAll((inputs) => inputs.map((input) => input.checked)),
        choices,
      );
      assert.deepEqual(await groundingText(page), [
        "Must know Rust",
        "Must know SQL",
        "Rust",
        "Go",
        "Built a parser",
      ]);
      await page.click("#start");
      await page.waitForURL(/\/interview/);
      const query = new URL(page.url()).searchParams;
      assert.equal(query.get("problem"), MEDIUM[1]);
      assert.equal(query.get("duration"), "60");
      assert.equal(query.get("loop"), "coding_only");
      assert.equal(query.get("role"), "Backend engineer");
      assert.equal(query.get("seniority"), "senior");
      assert.equal(query.get("company"), "Example");
      const stored = await page.evaluate(() =>
        JSON.parse(sessionStorage.getItem("codetrial.interview-grounding.v1")),
      );
      assert.deepEqual(stored.requirements, ["Must know SQL"]);
      assert.deepEqual(stored.skills, ["Rust"]);
      assert.deepEqual(stored.anchors, ["Built a parser"]);
      assert.equal(
        await page.evaluate(() =>
          sessionStorage.getItem("codetrial.sharedPracticeFocus"),
        ),
        null,
      );
    },
  );
}

lobbyTest(
  "an account check can be retried without recording the GitHub username again",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await lobby(page);
    failing.add("/api/session");
    const submitted = [];
    page.on("request", (request) => {
      if (request.url().endsWith("/api/login")) submitted.push(request);
    });
    await page.fill("#github-login", "candidate");
    await page.press("#github-login", "Enter");
    await reaches(
      page,
      () =>
        document.querySelector("#login-link").textContent ===
        "Retry account check",
    );
    assert.match(
      await page.locator("#account-status").textContent(),
      /account could not be refreshed/,
    );
    assert.equal(await page.locator("#github-login").isHidden(), true);
    assert.equal(await page.locator("#login-link").isVisible(), true);
    assert.equal(await page.locator("#login-link").isDisabled(), false);
    assert.equal(await page.locator("#start").isDisabled(), true);
    await page.click("details.problem-picker summary");
    await page.click(`[data-problem="${MEDIUM[1]}"]`);
    assert.equal(
      await page.locator("#start").isDisabled(),
      true,
      "a card click bypassed the unfinished account check",
    );
    failing.delete("/api/session");
    reports = [savedAttempt(EASY[0])];
    const account = page.waitForRequest((request) =>
      request.url().endsWith("/api/session"),
    );
    await page.click("#login-link");
    await account;
    await awaitReady(page);
    assert.equal(
      await page.locator("#account-status").textContent(),
      "Signed in as candidate",
    );
    assert.equal(await page.locator("#start").isDisabled(), false);
    assert.match(
      await page.locator("#progress-summary").textContent(),
      /1 of 1 attempts shown/,
    );
    assert.equal(
      submitted.length,
      1,
      "retry recorded the same GitHub username again",
    );
    // A later failed account read uses the normal sign-in path again. Its
    // button must not retain the earlier GET-only retry label.
    failing.add("/api/session");
    await restore(page);
    await reaches(
      page,
      () =>
        document.querySelector("#account-status").textContent === "Signed out",
    );
    assert.equal(await page.locator("#login-link").textContent(), "Use GitHub");
  },
);

lobbyTest(
  "a login without a signed-in session returns to the GitHub choice",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await lobby(page);
    await page.route(
      `${base}/api/session`,
      (route) =>
        route.fulfill({
          json: { signedIn: false, loginRequired: true },
        }),
      { times: 1 },
    );
    await page.fill("#github-login", "candidate");
    await page.press("#github-login", "Enter");
    await reaches(page, () =>
      document
        .querySelector("#account-status")
        .textContent.includes("no signed-in session"),
    );
    assert.match(
      await page.locator("#account-status").textContent(),
      /no signed-in session/,
    );
    assert.equal(await page.locator("#github-login").isVisible(), true);
    assert.equal(await page.locator("#login-link").textContent(), "Use GitHub");
    assert.equal(await page.locator("#login-link").isDisabled(), false);
    assert.equal(
      await page.locator("#start").textContent(),
      "Use GitHub to start",
    );
    const retry = page.waitForRequest((request) =>
      request.url().endsWith("/api/login"),
    );
    await page.click("#login-link");
    await retry;
    await reaches(
      page,
      () =>
        document.querySelector("#account-status").textContent ===
        "Signed in as candidate",
    );
    assert.equal(
      await page.locator("#account-status").textContent(),
      "Signed in as candidate",
    );
  },
);

lobbyTest(
  "failed account history after login clears stale local progress and focus",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await page.addInitScript((entry) => {
      localStorage.setItem("codetrial_history", JSON.stringify([entry]));
    }, focusedAttempt(EASY[0]).payload);
    await lobby(page);
    assert.match(
      await page.locator("#progress-summary").textContent(),
      /1 of 1 attempts shown/,
    );
    await page.check("#practice-focus-share-input");
    failing.add("/api/reports");
    await page.fill("#github-login", "candidate");
    await page.press("#github-login", "Enter");
    await reaches(
      page,
      () =>
        !document.querySelector("#start").disabled &&
        document
          .querySelector("#history")
          .textContent.includes("Could not load saved account progress."),
    );
    assert.equal(
      await page.locator("#account-status").textContent(),
      "Signed in as candidate",
    );
    assert.equal(await page.locator("#start").isDisabled(), false);
    assert.match(
      await page.locator("#history").textContent(),
      /Could not load saved account progress/,
    );
    assert.equal(await page.locator("#attempt-history").textContent(), "");
    assert.equal(await page.locator("#practice-focus").isHidden(), true);
    assert.equal(
      await page.locator("#practice-focus-share-input").isChecked(),
      false,
    );
    await page.click("#random-problem");
    assert.equal(
      await page.locator("#practice-focus").isHidden(),
      true,
      "a redraw revived stale local history",
    );
  },
);

lobbyTest(
  "an account refresh clamps a manual duration to the server recording cap",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await lobby(page);
    await page.click('[data-duration="60"]');
    await page.route(
      `${base}/api/session`,
      (route) =>
        route.fulfill({
          json: {
            signedIn: true,
            user: { login: "candidate" },
            maxDurationMin: 30,
          },
        }),
      { times: 1 },
    );
    await page.fill("#github-login", "candidate");
    await page.press("#github-login", "Enter");
    await awaitReady(page);
    const state = await snapshot(page);
    assert.equal(state.duration, "30");
    assert.deepEqual(state.durationsOff, ["45", "60"]);
    assert.match(state.durationNote, /at most 30 minutes/);
  },
);

lobbyTest(
  "an older login refresh cannot re-arm controls while the restored history is pending",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await lobby(page);
    await page.evaluate(() => {
      const fetch = window.fetch.bind(window);
      window.pendingHistory = [];
      window.fetch = (url, ...args) =>
        url === "/api/reports"
          ? new Promise((resolve) =>
              window.pendingHistory.push((reports) =>
                resolve(new Response(JSON.stringify({ reports }))),
              ),
            )
          : fetch(url, ...args);
    });
    try {
      await page.fill("#github-login", "candidate");
      await page.press("#github-login", "Enter");
      await page.waitForFunction(() => window.pendingHistory.length === 1);
      await restore(page);
      await page.waitForFunction(() => window.pendingHistory.length === 2);
      await page.evaluate(
        async (reports) => {
          window.pendingHistory[0](reports);
          await new Promise(requestAnimationFrame);
        },
        [focusedAttempt(EASY[0])],
      );
      for (const selector of [
        "#start",
        "#login-link",
        "#random-problem",
        "#delete-reports",
      ]) {
        assert.equal(
          await page.locator(selector).isDisabled(),
          true,
          `${selector} was re-armed by the older refresh`,
        );
      }
      await page.evaluate(
        async (reports) => {
          window.pendingHistory[1](reports);
          await new Promise(requestAnimationFrame);
        },
        [savedAttempt(MEDIUM[0])],
      );
      assert.equal(await page.locator("#start").isDisabled(), false);
      assert.equal(await page.locator("#random-problem").isDisabled(), false);
      assert.equal(
        await page.locator("#account-status").textContent(),
        "Signed in as candidate",
      );
      assert.match(
        await page.locator("#progress-summary").textContent(),
        /1 of 1 attempts shown/,
      );
      assert.equal((await snapshot(page)).card, MEDIUM[0]);
      assert.equal(
        await page.locator("#practice-focus").isHidden(),
        true,
        "the older history replaced the current reports",
      );
    } finally {
      await page.evaluate(() =>
        window.pendingHistory.forEach((release) => release([])),
      );
    }
  },
);

/// The request stays pending until the application's own timeout signal aborts.
/// Keeping headers and body stalls separate catches a timeout that stops as
/// soon as fetch resolves, before response.json has consumed the response.
async function stallTimedRequest(
  page,
  { url, after = 0, body = false, status = 200 },
) {
  await page.addInitScript(
    ({ url, after, body, status }) => {
      const timers = new WeakMap();
      AbortSignal.timeout = (delay) => {
        const controller = new AbortController();
        timers.set(controller.signal, { controller, delay });
        return controller.signal;
      };
      const fetch = window.fetch.bind(window);
      let matched = 0;
      window.lobbyRequests = [];
      window.fetch = (path, options = {}) => {
        window.lobbyRequests.push({ path, method: options.method ?? "GET" });
        if (path !== url || matched++ !== after) return fetch(path, options);
        const signal = options.signal;
        const timer = timers.get(signal);
        window.stalledRequest = {
          hasSignal: signal instanceof AbortSignal,
          delay: timer?.delay,
          aborted: false,
          expire: () =>
            timer?.controller.abort(
              new DOMException("Request timed out", "TimeoutError"),
            ),
        };
        const onAbort = (reject) => {
          signal?.addEventListener(
            "abort",
            () => {
              window.stalledRequest.aborted = true;
              reject(signal.reason);
            },
            { once: true },
          );
        };
        if (body) {
          return Promise.resolve(
            new Response(
              new ReadableStream({
                start(controller) {
                  onAbort((error) => controller.error(error));
                },
              }),
              { status, headers: { "Content-Type": "application/json" } },
            ),
          );
        }
        return new Promise((resolve, reject) => {
          onAbort(reject);
        });
      };
    },
    { url, after, body, status },
  );
}

async function expireStalledRequest(page) {
  await page.waitForFunction(() => window.stalledRequest !== undefined);
  assert.equal(
    await page.evaluate(() => window.stalledRequest.hasSignal),
    true,
    "the pending fetch did not receive an abort signal",
  );
  assert.equal(
    await page.evaluate(() => window.stalledRequest.delay),
    10_000,
    "the request did not use the ten-second deadline",
  );
  await page.evaluate(() => window.stalledRequest.expire());
  assert.equal(
    await page.evaluate(() => window.stalledRequest.aborted),
    true,
    "the pending request did not observe the timeout",
  );
}

lobbyTest(
  "an initial account timeout falls back to usable local progress",
  async (page) => {
    await stallTimedRequest(page, { url: "/api/session" });
    await page.addInitScript((entry) => {
      localStorage.setItem("codetrial_history", JSON.stringify([entry]));
    }, savedAttempt(EASY[0]).payload);
    await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
    assert.equal(await page.locator("#start").isDisabled(), true);
    await expireStalledRequest(page);
    await awaitReady(page);
    assert.equal(await page.locator("#start").isDisabled(), false);
    assert.equal(
      await page.locator("#account-status").textContent(),
      "Signed out",
    );
    assert.match(
      await page.locator("#progress-summary").textContent(),
      /1 of 1 attempts shown/,
    );
    assert.match(
      await page.locator("#history").textContent(),
      /saved on this device/,
    );
  },
);

lobbyTest(
  "an account response body timeout offers a GET-only account retry",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await stallTimedRequest(page, {
      url: "/api/session",
      after: 1,
      body: true,
    });
    await lobby(page);
    await page.fill("#github-login", "candidate");
    await page.press("#github-login", "Enter");
    await expireStalledRequest(page);
    await reaches(
      page,
      () =>
        document.querySelector("#login-link").textContent ===
        "Retry account check",
    );
    assert.match(
      await page.locator("#account-status").textContent(),
      /account could not be refreshed/,
    );
    assert.equal(await page.locator("#start").isDisabled(), true);
    assert.equal(await page.locator("#github-login").isHidden(), true);
    assert.equal(await page.locator("#login-link").isDisabled(), false);
    await page.click("#login-link");
    await awaitReady(page);
    assert.equal(
      await page.locator("#account-status").textContent(),
      "Signed in as candidate",
    );
    assert.deepEqual(
      await page.evaluate(() =>
        window.lobbyRequests
          .filter(({ path }) => path === "/api/login")
          .map(({ method }) => method),
      ),
      ["POST"],
    );
  },
);

lobbyTest(
  "an account history body timeout clears stale progress and releases Start",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await stallTimedRequest(page, { url: "/api/reports", body: true });
    await page.addInitScript((entry) => {
      localStorage.setItem("codetrial_history", JSON.stringify([entry]));
    }, focusedAttempt(EASY[0]).payload);
    await lobby(page);
    await page.check("#practice-focus-share-input");
    await page.fill("#github-login", "candidate");
    await page.press("#github-login", "Enter");
    await expireStalledRequest(page);
    await awaitReady(page);
    assert.equal(
      await page.locator("#account-status").textContent(),
      "Signed in as candidate",
    );
    assert.equal(await page.locator("#start").isDisabled(), false);
    assert.equal(await page.locator("#random-problem").isDisabled(), false);
    assert.match(
      await page.locator("#history").textContent(),
      /Could not load saved account progress/,
    );
    assert.equal(await page.locator("#attempt-history").textContent(), "");
    assert.equal(await page.locator("#practice-focus").isHidden(), true);
    assert.equal(
      await page.locator("#practice-focus-share-input").isChecked(),
      false,
    );
  },
);

for (const trigger of ["header", "Start"]) {
  lobbyTest(
    `a ${trigger} login timeout checks the account before allowing another POST`,
    async (page) => {
      session = { signedIn: false, loginRequired: true };
      await stallTimedRequest(page, { url: "/api/login" });
      await lobby(page);
      await page.click("details.problem-picker summary");
      await page.click(`[data-problem="${MEDIUM[1]}"]`);
      await page.click('[data-duration="60"]');
      await page.fill("#github-login", "candidate");
      if (trigger === "header") await page.press("#github-login", "Enter");
      else await page.click("#start");
      await expireStalledRequest(page);
      await reaches(
        page,
        () =>
          document.querySelector("#login-link").textContent ===
          "Retry account check",
      );
      assert.equal(
        await page.locator("#account-status").textContent(),
        "Could not confirm the sign-in result. Retry the account check.",
      );
      assert.equal(await page.locator("#github-login").isHidden(), true);
      assert.equal(await page.locator("#login-link").isDisabled(), false);
      assert.equal(await page.locator("#start").isDisabled(), true);
      assert.equal(
        new URL(page.url()).pathname,
        "/",
        "a login timeout navigated into the interview",
      );
      const selected = await snapshot(page);
      assert.equal(selected.card, MEDIUM[1]);
      assert.equal(selected.duration, "60");
      await page.evaluate(() => document.querySelector("#start").click());
      if (trigger === "header") {
        // A malformed account response cannot establish that the POST failed.
        await page.route(
          `${base}/api/session`,
          (route) => route.fulfill({ json: {} }),
          { times: 1 },
        );
        await page.click("#login-link");
        await reaches(
          page,
          () => !document.querySelector("#login-link").disabled,
        );
        assert.equal(
          await page.locator("#login-link").textContent(),
          "Retry account check",
        );
        assert.equal(await page.locator("#start").isDisabled(), true);
        session = { signedIn: true, user: { login: "candidate" } };
      }
      await page.click("#login-link");
      await awaitReady(page);
      assert.deepEqual(
        await page.evaluate(() =>
          window.lobbyRequests
            .filter(({ path }) => path === "/api/login")
            .map(({ method }) => method),
        ),
        ["POST"],
      );
      if (trigger === "header") {
        assert.equal(
          await page.locator("#account-status").textContent(),
          "Signed in as candidate",
        );
        assert.equal(await page.locator("#login-link").isHidden(), true);
      } else {
        assert.match(
          await page.locator("#account-status").textContent(),
          /no signed-in session/i,
        );
        assert.equal(
          await page.locator("#login-link").textContent(),
          "Use GitHub",
        );
        assert.equal(await page.locator("#github-login").isVisible(), true);
        await page.click("#login-link");
        await awaitReady(page);
        assert.equal(
          await page.locator("#account-status").textContent(),
          "Signed in as candidate",
        );
        assert.equal(
          await page.evaluate(
            () =>
              window.lobbyRequests.filter(({ path }) => path === "/api/login")
                .length,
          ),
          2,
        );
      }
      assert.equal(
        new URL(page.url()).pathname,
        "/",
        "an account retry navigated without another Start",
      );
      const recovered = await snapshot(page);
      assert.equal(recovered.card, selected.card);
      assert.equal(recovered.duration, selected.duration);
      assert.equal(recovered.startDisabled, false);
    },
  );
}

lobbyTest(
  "a rejected login whose error body times out remains safe to resubmit",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await stallTimedRequest(page, {
      url: "/api/login",
      body: true,
      status: 500,
    });
    await lobby(page);
    await page.fill("#github-login", "candidate");
    await page.press("#github-login", "Enter");
    await expireStalledRequest(page);
    await reaches(page, () => !document.querySelector("#login-link").disabled);
    assert.equal(
      await page.locator("#account-status").textContent(),
      "Could not record GitHub username.",
    );
    assert.equal(await page.locator("#github-login").isVisible(), true);
    assert.equal(await page.locator("#login-link").textContent(), "Use GitHub");
    await page.press("#github-login", "Enter");
    await awaitReady(page);
    assert.equal(
      await page.locator("#account-status").textContent(),
      "Signed in as candidate",
    );
    assert.equal(
      await page.evaluate(
        () =>
          window.lobbyRequests.filter(({ path }) => path === "/api/login")
            .length,
      ),
      2,
    );
  },
);

lobbyTest(
  "a legacy page map timeout leaves local history usable",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await page.addInitScript(() => {
      localStorage.setItem(
        "codetrial_history",
        JSON.stringify([
          {
            problemId: "two-sum",
            date: "2026-01-01T00:00:00Z",
            report: { decision: "HIRE" },
          },
        ]),
      );
      const fetch = window.fetch.bind(window);
      window.fetch = (path, options) => {
        if (path !== "/problem-pages.json") return fetch(path, options);
        window.mapRequested = true;
        return new Promise(() => {});
      };
      const setTimeout = window.setTimeout.bind(window);
      window.mapDeadlines = [];
      window.setTimeout = (callback, delay, ...args) => {
        if (delay !== 10_000) return setTimeout(callback, delay, ...args);
        window.mapDeadlines.push({ delay, expire: () => callback(...args) });
        return 0;
      };
    });
    await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => window.mapRequested);
    assert.deepEqual(
      await page.evaluate(() => window.mapDeadlines.map(({ delay }) => delay)),
      [10_000],
    );
    assert.equal(await page.locator("#start").isDisabled(), true);
    await page.evaluate(() => window.mapDeadlines[0].expire());
    await awaitReady(page);
    assert.equal(await page.locator("#start").isDisabled(), false);
    assert.match(
      await page.locator("#history").textContent(),
      /saved on this device/,
    );
    assert.match(
      await page.locator("#progress-summary").textContent(),
      /1 of 1 attempts shown/,
    );
    assert.equal(
      await page.evaluate(
        () =>
          JSON.parse(localStorage.getItem("codetrial_history"))[0].problemId,
      ),
      "two-sum",
    );
  },
);

lobbyTest(
  "a late account check cannot erase an uncertain login result",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await lobby(page);
    await page.evaluate(() => {
      const fetch = window.fetch.bind(window);
      window.loginAttempts = 0;
      window.fetch = (path, options) => {
        if (path === "/api/login") {
          window.loginAttempts += 1;
          return new Promise((resolve, reject) => {
            window.rejectLogin = () => reject(new TypeError("Connection lost"));
          });
        }
        if (path === "/api/session") {
          return new Promise((resolve) => {
            window.releaseAccount = () =>
              resolve(
                new Response(
                  JSON.stringify({ signedIn: false, loginRequired: true }),
                ),
              );
          });
        }
        return fetch(path, options);
      };
    });
    try {
      await page.fill("#github-login", "candidate");
      await page.press("#github-login", "Enter");
      await page.waitForFunction(
        () => typeof window.rejectLogin === "function",
      );
      // A restored page checks the account while the earlier POST is unresolved.
      await restore(page);
      await page.waitForFunction(
        () => typeof window.releaseAccount === "function",
      );
      await page.evaluate(() => window.rejectLogin());
      await reaches(
        page,
        () =>
          document.querySelector("#login-link").textContent ===
          "Retry account check",
      );
      assert.equal(
        await page.locator("#login-link").textContent(),
        "Retry account check",
      );
      await page.evaluate(async () => {
        window.releaseAccount();
        await new Promise(requestAnimationFrame);
      });
      assert.equal(
        await page.locator("#account-status").textContent(),
        "Could not confirm the sign-in result. Retry the account check.",
      );
      assert.equal(
        await page.locator("#login-link").textContent(),
        "Retry account check",
      );
      assert.equal(await page.locator("#github-login").isHidden(), true);
      assert.equal(await page.locator("#start").isDisabled(), true);
      assert.equal(await page.evaluate(() => window.loginAttempts), 1);
    } finally {
      await page.evaluate(() => {
        window.rejectLogin?.();
        window.releaseAccount?.();
      });
    }
  },
);

lobbyTest(
  "a difficulty change from a fresh lobby retains review priority",
  async (page) => {
    reports = [savedAttempt(EASY[0]), savedAttempt(EASY[1])];
    const initial = await lobby(page);
    assert.deepEqual(initial.levels, ["Medium"]);
    assert.equal((await cardInfo(page, initial.card)).level, "Easy");
    assert.match(initial.note, /Review due/);
    await page.evaluate(() => {
      Math.random = () => 0;
    });
    await setLevel(page, "Hard", true);
    await setLevel(page, "Medium", false);
    const changed = await snapshot(page);
    assert.deepEqual(changed.levels, ["Hard"]);
    assert.ok(EASY.includes(changed.card));
    assert.match(changed.note, /Review due/);
    assert.equal((await cardInfo(page, changed.card)).hidden, false);
  },
);

lobbyTest(
  "a topic change after a random draw restores review priority",
  async (page) => {
    const due = pageOf("valid-parentheses");
    reports = [savedAttempt(due)];
    await lobby(page);
    await page.click("#random-problem");
    assert.doesNotMatch((await snapshot(page)).note, /Review due/);
    await openTopicFilter(page);
    await page.selectOption("#problem-topic", "Array");
    const changed = await snapshot(page);
    assert.equal(changed.card, due);
    assert.match(changed.note, /Review due/);
    assert.equal((await cardInfo(page, due)).hidden, false);
  },
);

lobbyTest(
  "an explicit random draw escapes overdue reviews at the selected level",
  async (page) => {
    reports = [savedAttempt(EASY[0]), savedAttempt(EASY[1])];
    const initial = await lobby(page);
    assert.match(initial.note, /Review due/);
    await page.evaluate(() => {
      Math.random = () => 0;
    });
    await page.click("#random-problem");
    const drawn = await snapshot(page);
    assert.equal((await cardInfo(page, drawn.card)).level, "Medium");
    assert.doesNotMatch(drawn.note, /Review due/);
  },
);

for (const account of [false, true]) {
  for (const interviewMode of ["coding", "whiteboard"]) {
    lobbyTest(
      `retry restores ${interviewMode} settings from ${account ? "account" : "device"} history`,
      async (page) => {
        const attempt = {
          ...savedAttempt(EASY[0]).payload,
          durationMin: 60,
          interviewLoop: "coding_only",
          interviewMode,
        };
        if (account) reports = [{ payload: attempt }];
        else {
          session = { signedIn: false };
          await page.addInitScript(
            (saved) =>
              localStorage.setItem(
                "codetrial_history",
                JSON.stringify([saved]),
              ),
            attempt,
          );
        }
        await lobby(page);
        await page.click('[data-duration="30"]');
        await page
          .getByRole("button", { name: "Try again", exact: true })
          .click();
        assert.equal((await snapshot(page)).duration, "60");
        assert.equal(
          await page
            .locator('[data-loop="coding_only"]')
            .getAttribute("aria-pressed"),
          "true",
        );
        assert.equal(
          await page
            .locator(`[data-mode="${interviewMode}"]`)
            .getAttribute("aria-pressed"),
          "true",
        );
        assert.equal(
          await page.locator("#mode-note").isVisible(),
          interviewMode === "whiteboard",
        );
        assert.equal(
          await page
            .locator(".editor-options-row")
            .evaluate((fieldset) => fieldset.disabled),
          interviewMode === "whiteboard",
        );
        await page.click("#start");
        await page.waitForURL("**/interview?**");
        const query = new URL(page.url()).searchParams;
        assert.equal(query.get("problem"), EASY[0]);
        assert.equal(query.get("duration"), "60");
        assert.equal(query.get("loop"), "coding_only");
        assert.equal(query.get("mode"), interviewMode);
      },
    );
  }
}

lobbyTest(
  "retry restores the saved loop over the current choice and explains a capped duration",
  async (page) => {
    session.maxDurationMin = 45;
    reports = [
      {
        payload: {
          ...savedAttempt(EASY[0]).payload,
          durationMin: 60,
          interviewLoop: "coding_behavioral",
          interviewMode: "coding",
        },
      },
    ];
    await lobby(page);
    await page.click('[data-loop="coding_only"]');
    await page.click('[data-mode="whiteboard"]');
    await page.getByRole("button", { name: "Try again", exact: true }).click();
    const state = await snapshot(page);
    assert.equal(state.duration, "45");
    assert.match(state.note, /60 minutes; adjusted to 45 minutes/);
    assert.equal(
      await page
        .locator('[data-loop="coding_behavioral"]')
        .getAttribute("aria-pressed"),
      "true",
    );
    assert.equal(
      await page.locator('[data-mode="coding"]').getAttribute("aria-pressed"),
      "true",
    );
  },
);

lobbyTest(
  "lobby settings survive reload and a later recording cap",
  async (page) => {
    await lobby(page);
    await page.click('[data-duration="60"]');
    await page.click('[data-loop="coding_only"]');
    await page.click('[data-mode="whiteboard"]');
    await lobby(page);
    assert.equal((await snapshot(page)).duration, "60");
    assert.equal(
      await page
        .locator('[data-loop="coding_only"]')
        .getAttribute("aria-pressed"),
      "true",
    );
    assert.equal(
      await page
        .locator('[data-mode="whiteboard"]')
        .getAttribute("aria-pressed"),
      "true",
    );
    session.maxDurationMin = 45;
    await lobby(page);
    const state = await snapshot(page);
    assert.equal(state.duration, "45");
    assert.match(state.durationNote, /at most 45 minutes/);
    assert.equal(
      await page
        .locator('[data-loop="coding_only"]')
        .getAttribute("aria-pressed"),
      "true",
    );
    assert.equal(
      await page
        .locator('[data-mode="whiteboard"]')
        .getAttribute("aria-pressed"),
      "true",
    );
  },
);

for (const stored of [
  '{"durationMin":-1,"interviewLoop":"invalid","interviewMode":"invalid"}',
  "not json",
]) {
  lobbyTest(
    `invalid preferences do not stop the lobby: ${stored}`,
    async (page) => {
      await page.addInitScript(
        (value) => localStorage.setItem("codetrial.interviewSettings", value),
        stored,
      );
      const state = await lobby(page);
      assert.equal(state.startDisabled, false);
      assert.equal(
        await page
          .locator('[data-loop="coding_behavioral"]')
          .getAttribute("aria-pressed"),
        "true",
      );
      assert.equal(
        await page.locator('[data-mode="coding"]').getAttribute("aria-pressed"),
        "true",
      );
    },
  );
}

lobbyTest(
  "blocked preference storage still allows settings to be changed and retried",
  async (page) => {
    reports = [
      {
        payload: {
          ...savedAttempt(EASY[0]).payload,
          durationMin: 60,
          interviewLoop: "coding_only",
          interviewMode: "whiteboard",
        },
      },
    ];
    await page.addInitScript(() => {
      const get = Storage.prototype.getItem;
      const set = Storage.prototype.setItem;
      Storage.prototype.getItem = function (key) {
        if (key === "codetrial.interviewSettings") throw new Error("blocked");
        return get.call(this, key);
      };
      Storage.prototype.setItem = function (key, value) {
        if (key === "codetrial.interviewSettings") throw new Error("blocked");
        return set.call(this, key, value);
      };
    });
    await lobby(page);
    await page.click('[data-duration="30"]');
    await page.getByRole("button", { name: "Try again", exact: true }).click();
    assert.equal((await snapshot(page)).duration, "60");
    assert.equal(
      await page
        .locator('[data-mode="whiteboard"]')
        .getAttribute("aria-pressed"),
      "true",
    );
  },
);

for (const selector of [
  '[data-loop="coding_only"]',
  '[data-mode="whiteboard"]',
]) {
  lobbyTest(
    `changing ${selector} does not persist a suggested duration`,
    async (page) => {
      await lobby(page);
      await page.evaluate(
        (id) => document.querySelector(`[data-problem="${id}"]`).click(),
        EASY[0],
      );
      assert.equal((await snapshot(page)).duration, "30");
      await page.click(selector);
      const settings = await page.evaluate(() =>
        JSON.parse(localStorage.getItem("codetrial.interviewSettings")),
      );
      assert.equal(Object.hasOwn(settings, "durationMin"), false);
      await lobby(page);
      await page.evaluate(
        (id) => document.querySelector(`[data-problem="${id}"]`).click(),
        MEDIUM[0],
      );
      assert.equal((await snapshot(page)).duration, "45");
      assert.equal(
        await page.locator(selector).getAttribute("aria-pressed"),
        "true",
      );
    },
  );
}

lobbyTest(
  "retrying a legacy attempt preserves an earlier explicit duration",
  async (page) => {
    reports = [
      {
        payload: {
          ...savedAttempt(MEDIUM[0]).payload,
          date: "2026-01-02T00:00:00Z",
          durationMin: 60,
        },
      },
      savedAttempt(EASY[0]),
    ];
    await lobby(page);
    const retries = page.getByRole("button", {
      name: "Try again",
      exact: true,
    });
    await retries.nth(0).click();
    assert.equal((await snapshot(page)).duration, "60");
    await retries.nth(1).click();
    assert.equal((await snapshot(page)).duration, "60");
    assert.equal(
      await page.evaluate(
        () =>
          JSON.parse(localStorage.getItem("codetrial.interviewSettings"))
            .durationMin,
      ),
      60,
    );
  },
);

lobbyTest(
  "retry, loop, and duration clicks cannot change settings while a start is pending",
  async (page) => {
    session = { signedIn: false, loginRequired: true };
    await page.addInitScript(
      (attempt) =>
        localStorage.setItem("codetrial_history", JSON.stringify([attempt])),
      {
        ...savedAttempt(EASY[0]).payload,
        durationMin: 60,
        interviewLoop: "coding_only",
        interviewMode: "whiteboard",
      },
    );
    let release;
    holdLogin = new Promise((resolve) => (release = resolve));
    try {
      await lobby(page);
      await page.fill("#github-login", "candidate");
      const pending = page.waitForRequest((request) =>
        request.url().endsWith("/api/login"),
      );
      const before = await snapshot(page);
      const settings = await page.evaluate(() =>
        localStorage.getItem("codetrial.interviewSettings"),
      );
      await page.click("#start");
      await pending;
      const startingState = await snapshot(page);
      await page.click('[data-loop="coding_only"]');
      await page.click('[data-duration="60"]');
      await page
        .getByRole("button", { name: "Try again", exact: true })
        .click();
      assert.deepEqual(await snapshot(page), startingState);
      assert.equal(
        await page.locator('[data-mode="coding"]').getAttribute("aria-pressed"),
        "true",
      );
      assert.equal(
        await page
          .locator('[data-loop="coding_behavioral"]')
          .getAttribute("aria-pressed"),
        "true",
      );
      assert.equal(
        await page.evaluate(() =>
          localStorage.getItem("codetrial.interviewSettings"),
        ),
        settings,
      );
      release();
      await page.waitForURL("**/interview?**");
      const query = new URL(page.url()).searchParams;
      assert.equal(query.get("problem"), before.card);
      assert.equal(query.get("mode"), "coding");
      assert.equal(query.get("loop"), "coding_behavioral");
      assert.equal(query.get("duration"), before.duration);
    } finally {
      release();
    }
  },
);

lobbyTest(
  "retry uses the server limit when no offered duration fits",
  async (page) => {
    session.maxDurationMin = 10;
    reports = [
      { payload: { ...savedAttempt(EASY[0]).payload, durationMin: 15 } },
    ];
    await lobby(page);
    await page.getByRole("button", { name: "Try again", exact: true }).click();
    const state = await snapshot(page);
    assert.equal(state.duration, null);
    assert.match(state.durationNote, /limited to 10 minutes/);
    assert.match(state.note, /15 minutes; adjusted to 10 minutes/);
    await page.click("#start");
    await page.waitForURL("**/interview?**");
    assert.equal(new URL(page.url()).searchParams.get("duration"), "10");
  },
);

for (const selector of [
  '[data-loop="coding_only"]',
  '[data-mode="whiteboard"]',
]) {
  lobbyTest(
    `changing ${selector} under a cap preserves the chosen duration`,
    async (page) => {
      await lobby(page);
      await page.click('[data-duration="60"]');
      session.maxDurationMin = 45;
      await restore(page);
      await awaitReady(page);
      assert.equal((await snapshot(page)).duration, "45");
      await page.click(selector);
      assert.equal(
        await page.evaluate(
          () =>
            JSON.parse(localStorage.getItem("codetrial.interviewSettings"))
              .durationMin,
        ),
        60,
      );
      await lobby(page);
      assert.equal((await snapshot(page)).duration, "45");
      await page.click(selector);
      session.maxDurationMin = 90;
      await restore(page);
      await awaitReady(page);
      assert.equal((await snapshot(page)).duration, "60");
      await lobby(page);
      assert.equal((await snapshot(page)).duration, "60");
    },
  );
}

lobbyTest(
  "a legacy retry leaves its duration suggested across reloads",
  async (page) => {
    reports = [savedAttempt(EASY[0])];
    await lobby(page);
    await page.getByRole("button", { name: "Try again", exact: true }).click();
    assert.equal((await snapshot(page)).duration, "30");
    const settings = await page.evaluate(() =>
      JSON.parse(localStorage.getItem("codetrial.interviewSettings")),
    );
    assert.equal(Object.hasOwn(settings, "durationMin"), false);
    await page.evaluate(
      (id) => document.querySelector(`[data-problem="${id}"]`).click(),
      MEDIUM[0],
    );
    assert.equal((await snapshot(page)).duration, "45");
    await lobby(page);
    await page.evaluate(
      (id) => document.querySelector(`[data-problem="${id}"]`).click(),
      MEDIUM[0],
    );
    assert.equal((await snapshot(page)).duration, "45");
  },
);

lobbyTest(
  "example drawing is opt-in and stays off in Whiteboard",
  async (page) => {
    for (const enabled of [false, true]) {
      await lobby(page);
      const toggle = page.getByRole("checkbox", {
        name: "Enable Example drawing",
        exact: true,
      });
      assert.equal(await toggle.isChecked(), false);
      if (enabled) await toggle.check();
      await page.click("#start");
      await page.waitForURL(/\/interview/);
      assert.equal(
        new URL(page.url()).searchParams.get("exampleDrawing"),
        enabled ? "1" : null,
      );
    }
    await lobby(page);
    const toggle = page.getByRole("checkbox", {
      name: "Enable Example drawing",
      exact: true,
    });
    await toggle.check();
    await page.click('[data-mode="whiteboard"]');
    assert.equal(await toggle.isDisabled(), true);
    await page.click("#start");
    await page.waitForURL(/\/interview/);
    assert.equal(new URL(page.url()).searchParams.get("exampleDrawing"), null);
  },
);

lobbyTest(
  "example drawing choice survives reload and can be turned off",
  async (page) => {
    await lobby(page);
    const toggle = page.getByRole("checkbox", {
      name: "Enable Example drawing",
      exact: true,
    });
    assert.equal(await toggle.isChecked(), false);
    await toggle.check();
    assert.equal(
      await page.evaluate(
        () =>
          JSON.parse(localStorage.getItem("codetrial.interviewSettings"))
            ?.exampleDrawing,
      ),
      true,
    );
    await page.reload();
    await awaitReady(page);
    assert.equal(await toggle.isChecked(), true);
    await page.click('[data-mode="whiteboard"]');
    await page.reload();
    await awaitReady(page);
    assert.equal(await toggle.isDisabled(), true);
    assert.equal(await toggle.isChecked(), true);
    await page.click('[data-mode="coding"]');
    await toggle.uncheck();
    await page.reload();
    await awaitReady(page);
    assert.equal(await toggle.isChecked(), false);
    assert.equal(await toggle.isDisabled(), false);
  },
);

for (const settings of [
  {},
  { exampleDrawing: "true" },
  { exampleDrawing: false },
]) {
  lobbyTest(
    `example drawing stays off without a stored true choice: ${JSON.stringify(settings)}`,
    async (page) => {
      await page.addInitScript(
        (value) =>
          localStorage.setItem(
            "codetrial.interviewSettings",
            JSON.stringify(value),
          ),
        settings,
      );
      await lobby(page);
      assert.equal(
        await page
          .getByRole("checkbox", {
            name: "Enable Example drawing",
            exact: true,
          })
          .isChecked(),
        false,
      );
    },
  );
}

for (const storage of ["account", "device"]) {
  lobbyTest(`saved ${storage} attempt labels show difficulty`, async (page) => {
    const difficulties = ["Easy", "Medium", "Hard", null, "Unknown"];
    const entries = difficulties.map((difficulty, index) => ({
      id: `difficulty-${index}`,
      problemId: EASY[0],
      problemTitle: `Difficulty example ${index}`,
      language: "python",
      date: `2026-01-01T0${index}:00:00Z`,
      ...(difficulty === null ? {} : { difficulty }),
      report: { decision: "HIRE" },
    }));

    if (storage === "account") {
      reports = entries.map((payload) => ({
        id: payload.id,
        payload,
      }));
    } else {
      session = { signedIn: false };
      await page.addInitScript((entries) => {
        localStorage.setItem("codetrial_history", JSON.stringify(entries));
      }, entries);
    }

    await lobby(page);

    const labels = await page
      .locator("#attempt-history > li > p")
      .allTextContents();

    assert.equal(labels.length, entries.length);
    for (const [index, difficulty] of difficulties.entries()) {
      const expected =
        difficulty === null || difficulty === "Unknown"
          ? "Difficulty not recorded"
          : difficulty;
      assert.ok(
        labels[entries.length - 1 - index].endsWith(
          ` · Difficulty example ${index} · ${expected} · Python · HIRE`,
        ),
      );
    }
  });
}

lobbyTest(
  "problem cards span desktop panels and return home on narrow screens",
  async (page) => {
    await page.setViewportSize({ width: 1440, height: 1000 });
    await lobby(page);
    const grid = page.locator(".problem-grid");
    const picker = page.locator("details.problem-picker");
    const summary = picker.locator("summary");
    assert.equal(await grid.isVisible(), false);
    await openTopicFilter(page);
    await grid.waitFor({ state: "visible" });
    assert.ok(
      await grid.evaluate(
        (node) =>
          node.previousElementSibling ===
            document.querySelector(".setup-panel") &&
          node.nextElementSibling === document.querySelector(".session-panel"),
      ),
    );
    await page.locator("#problem-filters-reset").focus();
    await page.keyboard.press("Tab");
    assert.ok(
      await grid.evaluate((node) => node.contains(document.activeElement)),
    );
    const framesAgree = await page.evaluate(async () => {
      const picker = document.querySelector("details.problem-picker");
      const grid = document.querySelector(".problem-grid");
      const outline = document.querySelector(".problem-picker-outline");
      const summary = picker.querySelector("summary");
      for (let index = 0; index < 12; index++) {
        summary.click();
        await new Promise(requestAnimationFrame);
        if (
          Boolean(grid.hidden) === picker.open ||
          outline.hasAttribute("hidden") === picker.open ||
          document
            .querySelector(".lobby")
            .classList.contains("problem-picker-connected") !== picker.open
        )
          return false;
      }
      return picker.open;
    });
    assert.ok(
      framesAgree,
      "cards and outline must agree with the native picker before paint",
    );
    await grid.evaluate((node) => {
      for (const [index, entry] of [
        ...node.querySelectorAll(".problem-entry"),
      ].entries())
        entry.hidden = index > 0;
    });
    await page.waitForFunction(() => {
      const path = document.querySelector(".problem-picker-outline path");
      const lobby = document.querySelector(".lobby").getBoundingClientRect();
      const panel = document
        .querySelector(".setup-panel")
        .getBoundingClientRect();
      const grid = document.querySelector(".problem-grid");
      const card = grid.querySelector(".problem-card").getBoundingClientRect();
      const expected =
        Math.max(
          panel.right,
          card.right + parseFloat(getComputedStyle(grid).paddingRight),
        ) - lobby.left;
      const bounds = path.getBBox();
      return Math.abs(bounds.x + bounds.width - expected) < 1;
    });
    await grid.evaluate((node) => {
      // Filtering hides both the entry and its card, so both come back.
      for (const item of node.querySelectorAll(".problem-entry, .problem-card"))
        item.hidden = false;
    });
    const bounds = await page.evaluate(() => {
      const setup = document
        .querySelector(".setup-grid")
        .getBoundingClientRect();
      const cards = document
        .querySelector(".problem-grid")
        .getBoundingClientRect();
      return {
        left: Math.abs(setup.left - cards.left),
        width: Math.abs(setup.width - cards.width),
        below: [...document.querySelectorAll(".setup-panel")].every(
          (panel) => cards.top >= panel.getBoundingClientRect().bottom + 24,
        ),
      };
    });
    assert.ok(bounds.left < 1 && bounds.width < 1 && bounds.below);
    assert.equal(
      await page.locator(".setup-panel:first-child #problem-topic").count(),
      1,
    );
    for (const selector of [".setup-panel", ".session-panel", ".start-panel"]) {
      const outline = await page
        .locator(".problem-picker-outline path")
        .getAttribute("d");
      await page
        .locator(selector)
        .first()
        .evaluate((node) => {
          node.style.minHeight = `${node.getBoundingClientRect().height + 600}px`;
        });
      await page.waitForFunction(
        (previous) =>
          document
            .querySelector(".problem-picker-outline path")
            .getAttribute("d") !== previous,
        outline,
      );
      assert.ok(
        await page.evaluate(() => {
          const cards = document
            .querySelector(".problem-grid")
            .getBoundingClientRect();
          return [...document.querySelectorAll(".setup-panel")].every(
            (panel) => cards.top >= panel.getBoundingClientRect().bottom + 24,
          );
        }),
      );
      await page
        .locator(selector)
        .first()
        .evaluate((node) => node.style.removeProperty("min-height"));
      await page.evaluate(
        () =>
          new Promise((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(resolve)),
          ),
      );
    }
    const first = grid.locator(".problem-card:visible").first();
    const id = await first.getAttribute("data-problem");
    await first.click();
    assert.equal(
      await grid.evaluate((node) => {
        // Rendered cards, not unhidden ones: filtering hides the enclosing
        // entry, and a card inside it measures as a zero box at the top.
        const cards = [...node.querySelectorAll(".problem-card")].filter(
          (card) => card.checkVisibility(),
        );
        const top = cards[0].getBoundingClientRect().top;
        return cards.filter(
          (card) => Math.abs(card.getBoundingClientRect().top - top) < 1,
        ).length;
      }),
      4,
    );
    assert.ok(
      await page.evaluate(() => {
        const topic = document
          .querySelector("#problem-topic")
          .getBoundingClientRect();
        const reset = document
          .querySelector("#problem-filters-reset")
          .getBoundingClientRect();
        return Math.abs(topic.top - reset.top) < 1;
      }),
    );
    const width = (await first.boundingBox()).width;
    await page.setViewportSize({ width: 1600, height: 1000 });
    assert.ok(Math.abs((await first.boundingBox()).width - width) < 1);
    for (const viewportWidth of [1101, 1100, 1000, 761]) {
      await page.setViewportSize({ width: viewportWidth, height: 1000 });
      await page.waitForFunction(() =>
        document.querySelector(".setup-grid > .problem-grid"),
      );
      const layout = await page.locator(".setup-grid").evaluate((node) => {
        const start = node
          .querySelector(".start-panel")
          .getBoundingClientRect();
        const cards = node
          .querySelector(".problem-grid")
          .getBoundingClientRect();
        return {
          columns: getComputedStyle(node).gridTemplateColumns.split(" ").length,
          startAboveCards: start.bottom <= cards.top,
          startBelowCards: start.top >= cards.bottom,
        };
      });
      assert.equal(layout.columns, viewportWidth > 1100 ? 3 : 2);
      assert.equal(layout.startAboveCards, viewportWidth > 1100);
      assert.equal(layout.startBelowCards, viewportWidth <= 1100);
      assert.ok(
        await page.evaluate(() => {
          const setup = document
            .querySelector(".setup-grid")
            .getBoundingClientRect();
          const cards = document
            .querySelector(".problem-grid")
            .getBoundingClientRect();
          const panels = [...document.querySelectorAll(".setup-panel")].slice(
            0,
            2,
          );
          return (
            Math.abs(setup.width - cards.width) < 1 &&
            Math.abs(setup.left - cards.left) < 1 &&
            panels.every(
              (panel) => cards.top >= panel.getBoundingClientRect().bottom + 24,
            ) &&
            document.documentElement.scrollWidth <= innerWidth
          );
        }),
      );
    }
    await first.focus();
    await page.setViewportSize({ width: 760, height: 1000 });
    await page.waitForFunction(() =>
      document.querySelector("details.problem-picker .problem-grid"),
    );
    assert.equal(
      await first.evaluate((node) => node === document.activeElement),
      true,
    );
    assert.equal(
      await page.locator(`[data-problem="${id}"]`).getAttribute("aria-pressed"),
      "true",
    );
    await summary.click();
    await grid.waitFor({ state: "hidden" });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.waitForFunction(() =>
      document.querySelector(".setup-grid > .problem-grid"),
    );
    assert.equal(await grid.isVisible(), false);
    await summary.click();
    await grid.waitFor({ state: "visible" });
    assert.equal(await first.getAttribute("aria-pressed"), "true");
    await page.setViewportSize({ width: 320, height: 800 });
    await page.waitForFunction(() =>
      document.querySelector("details.problem-picker .problem-grid"),
    );
    assert.ok(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    );
  },
  { viewport: { width: 1440, height: 1000 } },
);

lobbyTest(
  "picker preserves compact spacing and mobile field sizing",
  async (page) => {
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await lobby(page);
    for (const width of [639, 640, 760, 761, 1023, 1024, 1280]) {
      await page.setViewportSize({ width, height: 800 });
      assert.equal(
        await page
          .locator(".problem-picker")
          .evaluate((node) => getComputedStyle(node).marginTop),
        width >= 640 ? "16px" : "36px",
      );
    }
    await page.setViewportSize({ width: 1280, height: 851 });
    assert.equal(
      await page
        .locator(".problem-picker")
        .evaluate((node) => getComputedStyle(node).marginTop),
      "36px",
    );
    await openTopicFilter(page);
    await page.locator(".problem-grid").waitFor({ state: "visible" });
    await page.setViewportSize({ width: 375, height: 800 });
    await page.waitForFunction(() =>
      document.querySelector("details.problem-picker .problem-grid"),
    );
    assert.equal(
      await page
        .locator("#problem-topic")
        .evaluate((node) => getComputedStyle(node).fontSize),
      "16px",
    );
    assert.deepEqual(errors, []);
  },
);

lobbyTest(
  "picker outline stays aligned after lower content grows",
  async (page) => {
    await page.setViewportSize({ width: 1440, height: 1000 });
    await lobby(page);
    await openTopicFilter(page);
    const path = page.locator(".problem-picker-outline path");
    await path.waitFor({ state: "visible" });
    const before = await path.boundingBox();
    for (const selector of ["#recent-performance", "#history"]) {
      const previousHeight = await page
        .locator(".lobby")
        .evaluate((node) => node.getBoundingClientRect().height);
      await page.locator(selector).evaluate((node) => {
        node.hidden = false;
        node.style.minHeight = `${node.getBoundingClientRect().height + 600}px`;
      });
      await page.waitForFunction(
        (height) =>
          document.querySelector(".lobby").getBoundingClientRect().height >
          height + 500,
        previousHeight,
      );
      await page.evaluate(
        () =>
          new Promise((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(resolve)),
          ),
      );
      const after = await path.boundingBox();
      for (const property of ["x", "y", "width", "height"]) {
        assert.ok(
          Math.abs(after[property] - before[property]) < 1,
          `${selector} growth must not change outline ${property}`,
        );
      }
    }
  },
);

lobbyTest(
  "fragment navigation reveals a collapsed problem card",
  async (page) => {
    await page.setViewportSize({ width: 1440, height: 1000 });
    await lobby(page);
    // Remove the native until-found skip so the application must hide contents.
    await page.addStyleTag({
      content:
        ':where([hidden="until-found"]) { content-visibility: visible; }',
    });
    const picker = page.locator("details.problem-picker");
    const grid = page.locator(".problem-grid");
    assert.equal(await picker.evaluate((node) => node.open), false);
    assert.equal(await grid.getAttribute("hidden"), "until-found");
    assert.equal(
      await grid.evaluate((node) => getComputedStyle(node).contentVisibility),
      "hidden",
    );
    assert.equal(
      await page.evaluate(() => {
        const selected = document.querySelector("#recommendation").textContent;
        const title = [
          ...document.querySelectorAll(
            ".problem-card:not([hidden]) .problem-title",
          ),
        ].find((node) => !selected.includes(node.textContent)).textContent;
        const found = window.find(title);
        window.getSelection().removeAllRanges();
        return found;
      }),
      true,
    );
    await picker.locator("summary").focus();
    await page.keyboard.press("Tab");
    assert.equal(
      await page.evaluate(
        () => !!document.activeElement.closest(".session-panel"),
      ),
      true,
    );
    await page.evaluate(() => {
      const card = document.querySelector(".problem-card:not([hidden])");
      card.id = "find-problem-card";
      const link = document.createElement("a");
      link.href = "#find-problem-card";
      link.textContent = "Find collapsed problem";
      document.querySelector(".lobby").prepend(link);
    });
    await page.getByRole("link", { name: "Find collapsed problem" }).click();
    await page.waitForFunction(
      () => document.querySelector("details.problem-picker").open,
    );
    assert.equal(await grid.getAttribute("hidden"), null);
    assert.equal(await page.locator("#find-problem-card").isVisible(), true);
    await picker.locator("summary").click();
    await page.waitForFunction(
      () => document.querySelector(".problem-grid").hidden === "until-found",
    );
    await page.setViewportSize({ width: 375, height: 800 });
    await page.waitForFunction(() =>
      document.querySelector("details.problem-picker .problem-grid"),
    );
    assert.equal(await grid.getAttribute("hidden"), null);
  },
);

lobbyTest(
  "picker outline follows mode changes and a late topic error",
  async (page) => {
    let releaseTopics;
    const topicsHeld = new Promise((resolve) => {
      releaseTopics = resolve;
    });
    await page.route("**/problem-topics.json", async (route) => {
      await topicsHeld;
      await route.fulfill({ status: 500, body: "nope" });
    });
    try {
      await page.setViewportSize({ width: 1101, height: 1000 });
      await lobby(page);
      await page.locator("details.problem-picker summary").click();
      const aligned = () =>
        page.waitForFunction(() => {
          const panel = document
            .querySelector(".setup-panel")
            .getBoundingClientRect();
          const grid = document
            .querySelector(".problem-grid")
            .getBoundingClientRect();
          const outline = document
            .querySelector(".problem-picker-outline path")
            .getBoundingClientRect();
          return (
            Math.abs(outline.left - panel.left) < 1 &&
            Math.abs(outline.top - panel.top) < 1 &&
            Math.abs(outline.bottom - grid.bottom) < 1
          );
        });
      await aligned();
      await page.locator('[data-mode="whiteboard"]').click();
      assert.equal(await page.locator("#mode-note").isVisible(), true);
      await aligned();
      releaseTopics();
      await page.waitForFunction(() =>
        document
          .querySelector("#problem-filter-summary")
          .textContent.includes("could not be loaded"),
      );
      await aligned();
      await page.locator('[data-mode="coding"]').click();
      assert.equal(await page.locator("#mode-note").isVisible(), false);
      await aligned();
    } finally {
      releaseTopics();
    }
  },
);

/// Loads the lobby and waits until `/api/session` has been applied. The
/// status starts as "Checking account..." in the markup, so only the text the
/// answer writes says the notice has been decided; any non-empty text would
/// pass before the response arrived.
async function lobbyAfterSession(page, status) {
  const answered = page.waitForResponse(
    (response) => new URL(response.url()).pathname === "/api/session",
  );
  await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
  await answered;
  await page.waitForFunction(
    (expected) =>
      document.querySelector("#account-status").textContent === expected,
    status,
  );
}

const BILLED_TO_SERVER =
  "Gemini Live usage is billed to this server's Google AI project, on the whole context of the session.";

lobbyTest(
  "the billing notice names voice and video for a coding interview with candidate video",
  async (page) => {
    session = {
      signedIn: true,
      user: { login: "candidate" },
      candidateVideo: true,
    };
    await lobbyAfterSession(page, "Signed in as candidate");
    const note = page.locator("#cost-note");
    assert.equal(await note.isVisible(), true);
    assert.equal(
      await note.textContent(),
      `This interview sends your voice and video to Gemini Live. ${BILLED_TO_SERVER}`,
    );
    assert.equal(
      await page.getAttribute("#start", "aria-describedby"),
      "cost-note",
    );
  },
);

lobbyTest(
  "the billing notice names voice alone for a coding interview without candidate video",
  async (page) => {
    session = { signedIn: false, candidateVideo: false };
    await lobbyAfterSession(page, "Signed out");
    const note = page.locator("#cost-note");
    assert.equal(await note.isVisible(), true);
    assert.equal(
      await note.textContent(),
      `This interview sends your voice to Gemini Live. ${BILLED_TO_SERVER}`,
    );
  },
);

for (const candidateVideo of [true, false]) {
  lobbyTest(
    `the billing notice names board images at a whiteboard (candidate video ${candidateVideo})`,
    async (page) => {
      session = {
        signedIn: true,
        user: { login: "candidate" },
        candidateVideo,
      };
      await lobbyAfterSession(page, "Signed in as candidate");
      const note = page.locator("#cost-note");
      const coding = `This interview sends ${
        candidateVideo ? "your voice and video" : "your voice"
      } to Gemini Live. ${BILLED_TO_SERVER}`;
      assert.equal(await note.textContent(), coding);

      await page.locator('[data-mode="whiteboard"]').click();
      assert.equal(await note.isVisible(), true);
      assert.equal(
        await note.textContent(),
        `This interview sends your voice and board images to Gemini Live. ${BILLED_TO_SERVER}`,
      );

      await page.locator('[data-mode="coding"]').click();
      assert.equal(await note.textContent(), coding);
    },
  );
}

// A web-only server answers null, an older one leaves the field out, and a
// failed request means offline practice: none of them can say who is billed.
for (const [name, setup, status] of [
  [
    "is null",
    () => (session = { signedIn: false, candidateVideo: null }),
    "Signed out",
  ],
  [
    "is missing",
    () => (session = { signedIn: true, user: { login: "candidate" } }),
    "Signed in as candidate",
  ],
  ["request fails", () => failing.add("/api/session"), "Signed out"],
]) {
  lobbyTest(
    `the billing notice stays hidden when candidateVideo ${name}`,
    async (page) => {
      setup();
      await lobbyAfterSession(page, status);
      assert.equal(await page.locator("#cost-note").isVisible(), false);
      await page.locator('[data-mode="whiteboard"]').click();
      assert.equal(await page.locator("#cost-note").isVisible(), false);
    },
  );
}

lobbyTest(
  "a failed account refresh takes down the billing notice",
  async (page) => {
    session = {
      signedIn: true,
      user: { login: "candidate" },
      candidateVideo: true,
    };
    await lobbyAfterSession(page, "Signed in as candidate");
    assert.equal(await page.locator("#cost-note").isVisible(), true);

    failing.add("/api/session");
    const answered = page.waitForResponse(
      (response) => new URL(response.url()).pathname === "/api/session",
    );
    await restore(page);
    await answered;
    await page.waitForFunction(
      () =>
        document.querySelector("#account-status").textContent === "Signed out",
    );
    assert.equal(await page.locator("#cost-note").isVisible(), false);
  },
);

for (const candidateVideo of [true, false]) {
  lobbyTest(
    `the billing notice names the coding drawing while example drawing is on (candidate video ${candidateVideo})`,
    async (page) => {
      session = {
        signedIn: true,
        user: { login: "candidate" },
        candidateVideo,
      };
      await lobbyAfterSession(page, "Signed in as candidate");
      const note = page.locator("#cost-note");
      const voice = candidateVideo ? "your voice and video" : "your voice";
      const plain = `This interview sends ${voice} to Gemini Live. ${BILLED_TO_SERVER}`;
      const drawing = `This interview sends ${voice}, and your drawing whenever Jim reads it, to Gemini Live. ${BILLED_TO_SERVER}`;
      assert.equal(await note.textContent(), plain);

      await page.locator("#enable-example-drawing").check();
      assert.equal(await note.textContent(), drawing);

      // The whiteboard's own wording wins while the checkbox is disabled, and
      // the drawing comes back with coding because the box stayed checked.
      await page.locator('[data-mode="whiteboard"]').click();
      assert.equal(
        await note.textContent(),
        `This interview sends your voice and board images to Gemini Live. ${BILLED_TO_SERVER}`,
      );
      await page.locator('[data-mode="coding"]').click();
      assert.equal(await note.textContent(), drawing);

      await page.locator("#enable-example-drawing").uncheck();
      assert.equal(await note.textContent(), plain);
    },
  );
}

lobbyTest(
  "a restored example drawing setting is named in the billing notice",
  async (page) => {
    session = {
      signedIn: true,
      user: { login: "candidate" },
      candidateVideo: false,
    };
    await page.addInitScript(() =>
      localStorage.setItem(
        "codetrial.interviewSettings",
        JSON.stringify({ exampleDrawing: true }),
      ),
    );
    await lobbyAfterSession(page, "Signed in as candidate");
    assert.equal(await page.isChecked("#enable-example-drawing"), true);
    assert.equal(
      await page.locator("#cost-note").textContent(),
      `This interview sends your voice, and your drawing whenever Jim reads it, to Gemini Live. ${BILLED_TO_SERVER}`,
    );
  },
);
