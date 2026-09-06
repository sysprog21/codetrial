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
import { createServer } from "node:http";
import { readFileSync, existsSync, statSync } from "node:fs";
import { join } from "node:path";

import { functionBody, launchChromium, read, root } from "./source.js";

const web = join(root, "web");

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
/// The same gate for `/api/login`, so a test about what the candidate can touch
/// while a start is mid-flight holds that request open rather than guessing a
/// latency long enough to win the race.
let holdLogin = null;
let deleteRequests = 0;

const type = (file) =>
  file.endsWith(".js") ? "text/javascript" : file.endsWith(".css") ? "text/css" : "text/html";

before(async () => {
  browser = await launchChromium();
  if (!browser) return;

  server = createServer((request, response) => {
    const url = new URL(request.url, "http://lobby.invalid");
    // Answered at once. A blanket delay here was about five of this file's
    // eleven seconds, paid twice per page load, and the two tests that need to
    // act while a request is in flight hold that request open instead, which is
    // a state rather than a race they hope to win.
    const json = (body) => {
      response.setHeader("content-type", "application/json");
      response.end(JSON.stringify(body));
    };

    if (request.method === "DELETE" && url.pathname === "/api/reports") {
      deleteRequests += 1;
    }

    if (failing.has(url.pathname)) {
      response.statusCode = 500;
      return response.end("nope");
    }
    if (url.pathname === "/api/session") return json(session);
    if (url.pathname === "/api/reports") {
      if (request.method === "DELETE") {
        const deleted = reports.length;
        reports = [];
        return json({ deleted });
      }
      return holdReports ? holdReports.then(() => json({ reports })) : json({ reports });
    }
    if (url.pathname === "/api/login") {
      session = { signedIn: true, user: { login: "candidate" } };
      return holdLogin ? holdLogin.then(() => json({ ok: true })) : json({ ok: true });
    }
    // Only somewhere for the start button to land, so the query string it built
    // can be read back off the URL.
    if (url.pathname === "/interview") {
      response.setHeader("content-type", "text/html");
      return response.end("<h1>interview</h1>");
    }

    const file = join(web, url.pathname === "/" ? "index.html" : url.pathname);
    // `startsWith` before touching the path: the URL comes from the page, but a
    // stub that will serve `/../../etc/passwd` is one somebody copies.
    if (!file.startsWith(web) || !existsSync(file) || statSync(file).isDirectory()) {
      response.statusCode = 404;
      return response.end("not found");
    }
    response.setHeader("content-type", type(file));
    response.end(readFileSync(file));
  });
  await new Promise((listening) => server.listen(0, "127.0.0.1", listening));
  base = `http://127.0.0.1:${server.address().port}`;
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
  holdLogin = null;
  deleteRequests = 0;
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
      card: one(".problem-card.selected")?.dataset.problem ?? null,
      pressed: one('.problem-card[aria-pressed="true"]')?.dataset.problem ?? null,
      duration: one(".duration-button.selected")?.dataset.duration ?? null,
      durationsOff: [...document.querySelectorAll(".duration-button")]
        .filter((button) => button.disabled)
        .map((button) => button.dataset.duration),
      durationNote: one("#duration-note").hidden ? "" : one("#duration-note").textContent,
      levels: [...document.querySelectorAll('[name="difficulty"]:checked')].map((i) => i.value),
      visibleCards: [...document.querySelectorAll(".problem-card")].filter((c) => !c.hidden).length,
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
      const input = document.querySelector(`[name="difficulty"][value="${level}"]`);
      if (input.checked === on) return;
      input.checked = on;
      input.dispatchEvent(new Event("change", { bubbles: true }));
    },
    [level, on],
  );

/// Wait for the page to reach a state, then let the assertions report on it.
///
/// Swallowing the timeout is deliberate: a bare `waitForFunction` that never
/// comes true fails as a thirty second timeout naming a predicate, and the
/// assertion right after it fails in one line naming what was wrong. The wait
/// is here to remove the race, not to be the check. Ten seconds because the
/// work behind these is two stubbed fetches, so anything near it is a hang.
async function settles(page, predicate) {
  await page.waitForFunction(predicate, null, { timeout: 10_000 }).catch(() => {});
}

/// A page whose first recommendation has landed.
async function lobby(page) {
  await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => document.querySelector("#recommendation").textContent !== "");
  return snapshot(page);
}

/// The length index.html preselects, which tests/web.rs pins to the server's
/// DEFAULT_DURATION_MIN. Read from the markup rather than from app.js's
/// `let duration`, which the module-scope setup overwrites before anything
/// reads it: a chain running through a value nothing observes is not a chain.
const markupDuration = () =>
  read("web/index.html").match(/duration-button selected"[^>]*data-duration="(\d+)"/)[1];

const hired = (problemId) => ({ problemId, payload: { report: { decision: "HIRE" } } });
const missed = (problemId) => ({ problemId, payload: { report: { decision: "NO_HIRE" } } });
const savedAttempt = (problemId) => ({
  problemId,
  payload: {
    problemId,
    date: "2026-01-01T00:00:00Z",
    report: { decision: "HIRE" },
  },
});

/// Ids from the real bank, so these break if the bank stops carrying them
/// rather than testing against problems that do not exist.
const EASY = ["two-sum", "valid-parentheses"];
const MEDIUM = ["jump-game", "gas-station"];
const HARD = ["candy", "trapping-rain-water"];

/// A test with a page of its own, closed even when an assertion throws. The
/// bare `await page.close()` each of these used to end on was skipped by any
/// failure, leaving a Chromium page and its 150-card DOM alive for the rest of
/// the run.
function lobbyTest(name, body, options) {
  test(name, async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const page = await browser.newPage(options);
    try {
      await body(page, t);
    } finally {
      await page.close();
    }
  });
}

/// A lobby whose two fetches are still in flight, and the release that lets
/// them land. Everything between is the pre-history state.
async function heldLobby(page) {
  const release = holdHistory();
  await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
  await settles(
    page,
    () => document.querySelector("#account-status").textContent !== "Checking account...",
  );
  return release;
}

/// The lobby has chosen: `settle` is what enables the button, so this is the
/// one condition that means every derived thing on the page is in place.
const awaitReady = (page) => settles(page, () => !document.querySelector("#start").disabled);

const restore = (page) =>
  page.evaluate(() =>
    window.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true })),
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
    const closed = await page.evaluate(() => document.body.scrollHeight);
    assert.ok(closed <= height, `the lobby is ${closed}px tall, past one ${height}px screen`);

    // And the wall is still reachable, just not in the way.
    await page.click("details.problem-picker summary");
    const open = await page.evaluate(() => document.body.scrollHeight);
    assert.ok(open > closed, "opening the picker shows nothing");
  },
  { viewport: { width: 1280, height: 800 } },
);

lobbyTest("a candidate who touches nothing gets the server's own default length", async (page) => {
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
});

lobbyTest("the recommendation does not move once it is on screen", async (page) => {
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
        if (text && window.__recommendations.at(-1) !== text) window.__recommendations.push(text);
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
  assert.equal(seen.length, 1, `the recommendation changed under the reader: ${seen.join(" -> ")}`);
});

lobbyTest("start cannot fire before there is a problem to start", async (page) => {
  // Held open, so this is the pre-history state however slow the machine is,
  // rather than a snapshot hoping to beat a timer.
  const release = await heldLobby(page);

  const early = await snapshot(page);
  assert.equal(early.card, null, "a problem was chosen before the reports arrived");
  assert.equal(early.startDisabled, true, "start was live with no problem behind it");

  release();
  await awaitReady(page);
  const settled = await snapshot(page);
  assert.equal(settled.startDisabled, false);
  assert.ok(settled.card, "nothing was recommended");
});

lobbyTest("the start button ships the problem and length that are on screen", async (page) => {
  const state = await lobby(page);
  await page.click("#start");
  await page.waitForURL(/\/interview/);

  const query = new URL(page.url()).searchParams;
  assert.equal(query.get("problem"), state.card);
  assert.equal(query.get("duration"), state.duration);
});

lobbyTest("the lobby never suggests a length past its own default", async (page) => {
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
});

lobbyTest("the picker shows the levels that are checked and no others", async (page) => {
  await lobby(page);

  const medium = (await snapshot(page)).visibleCards;
  await setLevel(page, "Hard", true);
  const both = (await snapshot(page)).visibleCards;
  await setLevel(page, "Medium", false);
  const hard = (await snapshot(page)).visibleCards;

  assert.ok(hard > 0, "checking Hard hid every problem");
  assert.ok(medium > hard, "Medium is not a different set from Hard");
  assert.equal(both, medium + hard, "checking both levels is not the union of them");

  // The check that matters: no card from an unchecked level is on screen.
  const shown = await page.evaluate(() =>
    [...document.querySelectorAll(".problem-card")]
      .filter((card) => !card.hidden)
      .map((card) => card.dataset.difficulty),
  );
  assert.deepEqual([...new Set(shown)], ["Hard"]);
});

lobbyTest("choosing a problem by hand keeps the filter and the length the candidate set", async (page) => {
  await lobby(page);

  await setLevel(page, "Hard", true);
  await page.click('[data-duration="60"]');
  const before = await snapshot(page);
  assert.deepEqual(before.levels, ["Medium", "Hard"]);
  assert.equal(before.duration, "60");

  await page.click("details.problem-picker summary");
  await page.click('[data-problem="candy"]');
  const after = await snapshot(page);

  assert.equal(after.card, "candy");
  assert.equal(after.pressed, "candy", "the selection is a border colour and nothing else");
  // And every other card says it is not pressed, rather than saying nothing:
  // one pressed button among 149 plain ones does not read as a choice.
  assert.equal(
    await page.evaluate(
      () => document.querySelectorAll('.problem-card:not([aria-pressed])').length,
    ),
    0,
    "cards nobody selected carry no pressed state at all",
  );
  assert.deepEqual(after.levels, ["Medium", "Hard"], "picking a card rewrote the difficulty filter");
  assert.equal(after.duration, "60", "picking a card discarded the length the candidate chose");
});

lobbyTest("an explicit length survives everything that would otherwise suggest one", async (page) => {
  await lobby(page);

  await page.click('[data-duration="60"]');
  await setLevel(page, "Easy", true);
  await setLevel(page, "Medium", false);
  assert.equal((await snapshot(page)).duration, "60", "a suggestion overrode an explicit choice");
});

lobbyTest("the last difficulty cannot be unchecked into an empty lobby", async (page) => {
  const before = await lobby(page);

  await setLevel(page, "Medium", false);
  const state = await snapshot(page);
  assert.deepEqual(state.levels, ["Medium"], "the lobby was left filtering on nothing");
  // The box goes back, so nothing about what is on offer moved and the
  // recommendation must not move either. Rerolling on the way through swapped
  // it for a different problem at the same level, which reads as the lobby
  // changing its mind on its own.
  assert.equal(state.card, before.card, "a rejected uncheck rerolled the recommendation");
  assert.equal(state.note, before.note);
  assert.ok(state.card, "unchecking the last level left no problem to start");
  assert.ok(state.visibleCards > 0, "the picker was left with nothing in it");
  assert.equal(state.startDisabled, false);
  assert.equal(
    (await cardInfo(page, state.card)).hidden,
    false,
    "the surviving selection is a card nobody can see",
  );
});

lobbyTest("a problem already passed is not what gets recommended", async (page) => {
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
  assert.equal(picked.hidden, false, "the recommendation is a card the filter hides");
  assert.notEqual(state.card, HARD[0], "a problem the candidate passed came back");
});

lobbyTest("two passes move the candidate up a level, and the lobby says why", async (page) => {
  reports = [hired(EASY[0]), hired(EASY[1])];
  const state = await lobby(page);

  assert.deepEqual(state.levels, ["Medium"], "passing twice at Easy did not move the candidate");
  assert.match(state.note, /passed your last two Easy problems/);
  assert.equal(
    (await cardInfo(page, state.card)).level,
    "Medium",
    "the level moved but the problem did not follow it",
  );
});

lobbyTest("two misses move the candidate down a level, and the lobby says why", async (page) => {
  reports = [missed(MEDIUM[0]), missed(MEDIUM[1])];
  const state = await lobby(page);

  assert.deepEqual(state.levels, ["Easy"]);
  assert.match(state.note, /last two Medium problems did not land/);
  // Easy carries the shorter slot with it.
  assert.equal(state.duration, "30");
});

lobbyTest("a candidate who picks a level first is not overruled by their history", async (page) => {
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
  assert.deepEqual(state.levels, ["Hard"], "the history overrode a level the candidate chose");
  assert.equal(
    state.note.includes("passed your last two"),
    false,
    "the lobby explained a level the candidate had chosen for themselves",
  );
});

lobbyTest("a gated candidate signs in and reaches the interview", async (page) => {
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
  assert.deepEqual(errors, [], "the start handler threw on the way to the interview");
});

lobbyTest("a lobby restored from cache rechecks the session instead of replaying it", async (page) => {
  session = { signedIn: false, loginRequired: true };
  reports = [];
  await lobby(page);
  assert.equal((await snapshot(page)).startText, "Use GitHub to start");

  // The login lands while this page is not the one in front: on the way out to
  // the interview, or in another tab. The cached page knows none of it, so what
  // it holds about the session is now wrong in every part.
  session = { signedIn: true, user: { login: "candidate" } };
  await restore(page);
  await settles(
    page,
    () =>
      document.querySelector("#account-status").textContent.startsWith("Signed in") &&
      !document.querySelector("#start").disabled,
  );

  const restored = await snapshot(page);
  assert.equal(restored.startText, "Start interview", "a signed-in candidate was told to sign in");
  assert.equal(restored.startDisabled, false, "the restored button never came back");
  assert.match(restored.account, /Signed in as candidate/);
});

lobbyTest("a restored lobby re-reads the history, not just the session", async (page) => {
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
  await settles(
    page,
    () =>
      !document.querySelector("#start").disabled &&
      document.querySelector("#recommendation").textContent.includes("did not land"),
  );

  const after = await snapshot(page);
  assert.deepEqual(after.levels, ["Easy"], "the restored lobby kept the level the old reports set");
  assert.match(after.note, /last two Medium problems did not land/);
  assert.equal(after.duration, "30", "the level moved but the length it implies did not");
});

lobbyTest("dropping the level of a hand-picked problem does not leave it startable", async (page) => {
  const release = await heldLobby(page);

  // All of this while the history is held open, where there is nothing to
  // choose a replacement with.
  await page.click("details.problem-picker summary");
  await page.click('[data-problem="jump-game"]');
  assert.equal((await snapshot(page)).card, "jump-game");

  await setLevel(page, "Hard", true);
  await setLevel(page, "Medium", false);
  const dropped = await snapshot(page);
  // The card is hidden by the filter now, so a start button still carrying it
  // would begin an interview on a level the candidate had just cleared.
  assert.equal(dropped.card, null, "a problem the filter hides stayed selected");
  assert.equal(dropped.startDisabled, true, "start still carried the filtered-out problem");

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
  assert.notEqual(shipped, "jump-game", "start shipped the problem the filter dropped");
});

lobbyTest("widening the filter keeps a problem the candidate picked by hand", async (page) => {
  const release = await heldLobby(page);

  await page.click("details.problem-picker summary");
  await page.click('[data-problem="jump-game"]');

  // Adding a level does not hide their card, so it is not a reason to discard
  // a choice they made on purpose.
  await setLevel(page, "Hard", true);
  const widened = await snapshot(page);
  assert.equal(widened.card, "jump-game", "adding a difficulty threw away an explicit pick");
  assert.deepEqual(widened.levels, ["Medium", "Hard"]);
  assert.equal(widened.startDisabled, false);

  // And the arriving history does not overrule it either.
  release();
  await awaitReady(page);
  assert.equal((await snapshot(page)).card, "jump-game", "the history overruled an explicit pick");
});

lobbyTest("a restore in flight is not a history a difficulty change may read", async (page) => {
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
  assert.equal(midflight.card, null, "a problem was chosen from the history being replaced");
  assert.equal(midflight.startDisabled, true, "start went live on a history already known stale");
  assert.equal(await page.locator("#delete-reports").isDisabled(), true);

  release();
  await awaitReady(page);
  const settled = await snapshot(page);
  assert.ok(settled.card, "nothing was recommended once the refreshed reports arrived");
  assert.ok(
    ![HARD[0], HARD[1]].includes(settled.card),
    "the refreshed history was not applied to the pick",
  );
});

// One value, not the shape matrix: what only a browser can show is the DOM
// outcome, and "a stored history that is not a list reads as no history" in
// account.test.js already pins the shapes for free.
lobbyTest("local history that is not a list does not take the lobby out", async (page) => {
  session = { signedIn: false };
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => localStorage.setItem("codetrial_history", '{"a":1}'));
  await page.goto(`${base}/`, { waitUntil: "domcontentloaded" });
  await awaitReady(page);

  const state = await snapshot(page);
  assert.equal(state.startDisabled, false, "the lobby was left with no way to start");
  assert.ok(state.card, "nothing was recommended");
  assert.deepEqual(errors, [], `it threw: ${errors[0]}`);
  assert.equal(state.history, "", "it was counted as past reports");
});

lobbyTest("a server that cannot answer at all still leaves a usable lobby", async (page) => {
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
  assert.ok(state.card, "a lobby whose history never arrived recommended nothing at all");
  assert.equal(state.startDisabled, false, "the lobby waited forever on a fetch that failed");
});

lobbyTest("a signed-in candidate whose reports fail still gets a problem", async (page) => {
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
});

lobbyTest("changing the level before the reports land waits for them", async (page) => {
  // Both Hard problems in the bank that this test can reach are passed, so a
  // recommendation made from an empty history would offer one of them back.
  reports = [hired(HARD[0]), hired(HARD[1])];
  const release = await heldLobby(page);

  // Checked while the reports are still in flight.
  await setLevel(page, "Hard", true);
  await setLevel(page, "Medium", false);
  const early = await snapshot(page);
  assert.equal(early.card, null, "a problem was chosen from a history that had not arrived");
  assert.equal(early.startDisabled, true, "start went live before the reports did");

  release();
  await awaitReady(page);
  const settled = await snapshot(page);
  assert.deepEqual(settled.levels, ["Hard"], "the level the candidate chose did not survive");
  assert.ok(settled.card, "nothing was recommended once the reports arrived");
  assert.ok(
    ![HARD[0], HARD[1]].includes(settled.card),
    "a problem the candidate already passed was recommended",
  );
});

lobbyTest("a start ships the length that was on screen when it was pressed", async (page) => {
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
});

lobbyTest("a start already on its way out is not undone by a later choice", async (page) => {
  // The sign-in round trip is the one window where the start handler is
  // suspended with the page still live under it, and everything the candidate
  // can touch during it writes the two things that handler is holding: which
  // problem is chosen, and whether the button is armed.
  session = { signedIn: false, loginRequired: true };
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const release = await heldLobby(page);

  await page.click("details.problem-picker summary");
  await page.click('[data-problem="jump-game"]');
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

  // A second card while the button reads "Recording GitHub...": re-arming it
  // here buys the candidate a second navigation out of one start.
  await page.click('[data-problem="gas-station"]');
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

  finishLogin();
  await page.waitForURL(/\/interview/);
  assert.deepEqual(errors, [], "the start handler threw after the login came back");
  assert.equal(
    new URL(page.url()).searchParams.get("problem"),
    "jump-game",
    "start shipped something other than the problem that was on screen when it was pressed",
  );
  release();
});

lobbyTest("a length this deployment cannot record is disabled, and says why", async (page) => {
  // The cap the server reports, which is `DEFAULT_RECORDING_MAX_MINUTES` on a
  // default deployment. Sixty is the length the row offers past it, and
  // `stale_after` reaps that recording ten minutes before the interview ends.
  session = { signedIn: true, user: { login: "candidate" }, maxDurationMin: 45 };

  const state = await lobby(page);

  assert.deepEqual(state.durationsOff, ["60"], "a length past the recording cap was still on offer");
  assert.match(state.durationNote, /at most 45 minutes/, "the disabled button gave no reason");
  assert.equal(state.duration, "45", "the cap moved a length that was already under it");
});

lobbyTest("the recording cap outranks the length the markup preselected", async (page) => {
  // Nothing else here overrules a length that is already set. This does,
  // because it is not a second opinion about what suits the candidate: it is
  // what this server is able to record.
  session = { signedIn: true, user: { login: "candidate" }, maxDurationMin: 30 };

  const state = await lobby(page);

  assert.equal(state.duration, "30", "the lobby kept a length longer than the recording");
  assert.deepEqual(state.durationsOff, ["45", "60"]);
});

lobbyTest("a server that can record every length on offer disables nothing", async (page) => {
  session = { signedIn: true, user: { login: "candidate" }, maxDurationMin: 90 };

  const state = await lobby(page);

  assert.deepEqual(state.durationsOff, []);
  assert.equal(state.durationNote, "", "a lobby with nothing capped explained a cap anyway");
  assert.equal(state.duration, markupDuration());
});

lobbyTest("saved reports require confirmation and clear the progress panel", async (page) => {
  reports = [savedAttempt(EASY[0]), savedAttempt(EASY[1])];
  await lobby(page);
  assert.match(await page.locator("#recommendation").textContent(), /passed your last two Easy problems/);

  page.once("dialog", (dialog) => dialog.dismiss());
  await page.click("#delete-reports");
  assert.equal(deleteRequests, 0);
  assert.equal(await page.locator("#history").isHidden(), false);

  page.once("dialog", (dialog) => {
    assert.match(dialog.message(), /reports and progress/);
    assert.match(dialog.message(), /Recording files follow their separate retention policy/);
    return dialog.accept();
  });
  await page.click("#delete-reports");
  await settles(page, () => document.querySelector("#history").hidden);
  assert.equal(deleteRequests, 1);
  assert.deepEqual(reports, []);
  assert.equal(await page.locator("#history").isHidden(), true);
  assert.equal(await page.locator("#delete-reports").isHidden(), true);
  assert.doesNotMatch(await page.locator("#recommendation").textContent(), /passed your last two/);
  assert.equal(await page.locator("#report-delete-status").textContent(), "Saved reports and progress were deleted.");
  assert.equal(await page.evaluate(() => document.activeElement.id), "report-delete-status");
});

lobbyTest("a failed report deletion retains the current progress", async (page) => {
  reports = [savedAttempt(EASY[0])];
  await lobby(page);
  failing.add("/api/reports");
  page.once("dialog", (dialog) => dialog.accept());

  await page.click("#delete-reports");
  await settles(page, () => document.querySelector("#report-delete-status").textContent !== "");

  assert.equal(deleteRequests, 1);
  assert.equal(await page.locator("#history").isHidden(), false);
  assert.match(await page.locator("#progress-summary").textContent(), /1 of 1 attempts shown/);
  assert.equal(
    await page.locator("#report-delete-status").textContent(),
    "Could not delete saved reports. Your reports may not have been removed.",
  );
});

lobbyTest("a signed-in account can erase reports saved only on this device", async (page) => {
  await page.addInitScript((entry) => {
    localStorage.setItem("codetrial_history", JSON.stringify([entry]));
  }, {
    problemId: EASY[0],
    date: "2026-01-03T00:00:00Z",
    report: { decision: "HIRE" },
  });
  await lobby(page);
  assert.equal(await page.locator("#history").isHidden(), true);
  assert.equal(await page.locator("#delete-reports").isVisible(), true);
  page.once("dialog", (dialog) => dialog.accept());

  await page.click("#delete-reports");
  await settles(page, () => document.querySelector("#report-delete-status").textContent !== "");

  assert.equal(await page.evaluate(() => localStorage.getItem("codetrial_history")), null);
  assert.equal(await page.locator("#delete-reports").isHidden(), true);
});

lobbyTest("an unavailable account check retains visible local reports", async (page) => {
  await page.addInitScript((entry) => {
    localStorage.setItem("codetrial_history", JSON.stringify([entry]));
  }, {
    problemId: EASY[0],
    date: "2026-01-04T00:00:00Z",
    report: { decision: "HIRE" },
  });
  failing.add("/api/session");
  await lobby(page);
  page.once("dialog", (dialog) => dialog.accept());

  await page.click("#delete-reports");
  await settles(page, () => document.querySelector("#report-delete-status").textContent !== "");

  assert.notEqual(await page.evaluate(() => localStorage.getItem("codetrial_history")), null);
  assert.equal(
    await page.locator("#report-delete-status").textContent(),
    "Could not delete saved reports. Your reports may not have been removed.",
  );
});

lobbyTest("a partial clear stays visible when local storage cannot be read", async (page) => {
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
  await settles(page, () => document.querySelector("#report-delete-status").textContent !== "");

  assert.equal(await page.locator("#history").isHidden(), true);
  assert.equal(await page.locator("#report-delete-status").isVisible(), true);
  assert.equal(
    await page.locator("#report-delete-status").textContent(),
    "Account reports were deleted, but reports saved on this device could not be deleted.",
  );
});

lobbyTest("blocked local storage does not prevent account deletion", async (page) => {
  reports = [savedAttempt(EASY[0])];
  await page.addInitScript(() => {
    Object.defineProperty(window, "localStorage", {
      configurable: true,
      get() { throw new DOMException("blocked", "SecurityError"); },
    });
  });
  await lobby(page);
  page.once("dialog", (dialog) => dialog.accept());

  await page.click("#delete-reports");
  await settles(page, () => document.querySelector("#report-delete-status").textContent !== "");

  assert.equal(deleteRequests, 1);
  assert.deepEqual(reports, []);
  assert.equal(
    await page.locator("#report-delete-status").textContent(),
    "Account reports were deleted, but reports saved on this device could not be deleted.",
  );
  assert.equal(await page.evaluate(() => document.activeElement.id), "report-delete-status");
});

lobbyTest("a partial clear reloads local reports and states what remains", async (page) => {
  reports = [savedAttempt(EASY[0])];
  await page.addInitScript((entry) => {
    localStorage.setItem("codetrial_history", JSON.stringify([entry]));
    const removeItem = Storage.prototype.removeItem;
    Storage.prototype.removeItem = function removeHistory(key) {
      if (key === "codetrial_history") throw new Error("blocked");
      return removeItem.call(this, key);
    };
  }, {
    problemId: EASY[1],
    date: "2026-01-02T00:00:00Z",
    report: { decision: "HIRE" },
  });
  await lobby(page);
  page.once("dialog", (dialog) => dialog.accept());

  await page.click("#delete-reports");
  await settles(page, () => document.querySelector("#report-delete-status").textContent !== "");

  assert.equal(deleteRequests, 1);
  assert.deepEqual(reports, []);
  assert.match(await page.locator("#progress-summary").textContent(), /saved on this device/);
  assert.equal(
    await page.locator("#report-delete-status").textContent(),
    "Account reports were deleted, but reports saved on this device could not be deleted.",
  );
  assert.equal(await page.evaluate(() => document.activeElement.id), "report-delete-status");
});

lobbyTest("a cap under every length on offer leaves the row alone", async (page) => {
  // A deployment that records less than the shortest interview it offers is
  // misconfigured, and `recording_config` refuses that pairing at startup. If
  // one reaches the browser anyway, a row with every button dead is a lobby
  // nobody can start; the server's own floor decides instead.
  session = { signedIn: true, user: { login: "candidate" }, maxDurationMin: 15 };

  const state = await lobby(page);

  assert.deepEqual(state.durationsOff, []);
  assert.equal(state.durationNote, "");
});

test("a failed history load leaves no other account's attempts behind", () => {
  // reports and progressEntries outlive the panel: recommendations and every
  // filter change read them again. Clearing only the DOM left the lobby
  // answering from whichever history it had last loaded successfully, which
  // after a sign-out is a different person's.
  const shown = functionBody(read("web/app.js"), "showProgressError");
  assert.match(shown, /reports = \[\]/);
  assert.match(shown, /progressEntries = \[\]/);
});
