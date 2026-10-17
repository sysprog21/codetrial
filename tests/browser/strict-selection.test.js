import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import {
  STRICT_ASSIGNMENT_KEY,
  previousAssignment,
  rememberAssignment,
  resetAssignments,
} from "../../web/selection.js";
import {
  DEFAULT_RUNTIME_CONFIG,
  launchChromium,
  memoryStorage,
  startStaticServer,
} from "./source.js";

test("selection receipts stay tab-local and storage failure is not a new sequence", () => {
  const first = memoryStorage();
  const second = memoryStorage();
  assert.equal(previousAssignment(first), null);
  rememberAssignment(first, "signed-first");
  assert.equal(previousAssignment(first), "signed-first");
  assert.equal(previousAssignment(second), null);
  rememberAssignment(first, "signed-second");
  assert.equal(previousAssignment(first), "signed-second");
  resetAssignments(first);
  assert.equal(previousAssignment(first), null);
  assert.throws(() => previousAssignment(null), /session storage/);
  assert.throws(
    () =>
      previousAssignment({
        getItem: () => "previous",
        setItem: () => {
          throw new Error("quota");
        },
      }),
    /session storage/,
  );
  assert.throws(() => rememberAssignment(second, ""), /invalid/);
  assert.throws(
    () =>
      rememberAssignment(
        {
          setItem: () => {},
          getItem: () => null,
        },
        "receipt",
      ),
    /session storage/,
  );
});

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
after(async () => {
  await browser?.close();
  await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
});

// Real MediaStream tracks with deterministic microphone samples exercise the
// existing readiness gate without depending on headless audio scheduling.
// The room transport is fake; reload still discards the whole page state.
function mediaAndRoom() {
  const NativeAudioContext = window.AudioContext;
  window.AudioContext = class extends NativeAudioContext {
    createAnalyser() {
      const analyser = super.createAnalyser();
      analyser.getByteTimeDomainData = (samples) => {
        for (let index = 0; index < samples.length; index++)
          samples[index] = index % 2 ? 200 : 60;
      };
      return analyser;
    }
  };
  navigator.mediaDevices.getUserMedia = async (constraints) => {
    if (constraints.video) throw new DOMException("No camera", "NotFoundError");
    return new NativeAudioContext().createMediaStreamDestination().stream;
  };
  window.roomJoins = [];
  window.roomEvents = {};
  window.LivekitClient = {
    RoomEvent: new Proxy({}, { get: (_, key) => key }),
    Room: class {
      remoteParticipants = new Map();
      localParticipant = {
        publishTrack: async () => {},
        publishData: async () => {},
      };
      on(event, callback) {
        (window.roomEvents[event] ??= []).push(callback);
        return this;
      }
      async connect(url, token) {
        window.roomJoins.push({ url, token });
      }
      async disconnect() {}
    },
  };
}

async function interview(t, query, handler, context = browser) {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return null;
  }
  if (context === browser) {
    context = await browser.newContext();
    t.after(() => context.close());
  }
  const page = await context.newPage();
  t.after(() => page.close());
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  t.after(() => assert.deepEqual(errors, []));
  page.on("dialog", (dialog) => dialog.accept());
  await page.route("**/vendor/livekit-client.js", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body: "",
    }),
  );
  await page.route("**/api/session", (route) =>
    route.fulfill({
      json: { signedIn: true, loginRequired: true },
    }),
  );
  await page.route("**/api/token", handler);
  await page.addInitScript(mediaAndRoom);
  await page.goto(`${base}/interview.html?${query}`);
  return page;
}

async function preflight(page) {
  await page.locator("#audio-heard").click();
  await page.locator("#camera-skip").click();
  try {
    await page.locator("#audio-check-join").click({ timeout: 10000 });
  } catch (error) {
    throw new Error(await page.locator("#audio-check-status").textContent(), {
      cause: error,
    });
  }
}

function grant(page, receipt, room = receipt) {
  return {
    problemPage: page,
    previousAssignment: receipt,
    roomName: room,
    serverUrl: "wss://example.test",
    token: room,
    durationMin: 30,
  };
}

test("Strict refresh keeps difficulty, ignores URL problem and joins a fresh assignment", async (t) => {
  const calls = [];
  const pages = ["chargeback-pair-match", "kiosk-panel-mirror-audit"];
  const page = await interview(
    t,
    "selection=strict&difficulty=Easy&problem=terrain-runoff-pooling",
    (route) => {
      calls.push(route.request().postDataJSON());
      return route.fulfill({
        json: grant(pages[calls.length - 1], `receipt-${calls.length}`),
      });
    },
  );
  if (!page) return;
  let warnings = 0;
  page.on("dialog", () => warnings++);
  assert.equal(calls.length, 0);
  assert.equal(await page.locator("#end").isDisabled(), true);
  assert.equal(await page.locator("#pause").isDisabled(), true);
  assert.equal(
    await page.locator("#problem-title").textContent(),
    "Strict Interview",
  );
  await page.locator("#interviewer-voice").selectOption("Puck");
  await page.locator("#interviewer-accent").selectOption("British");
  await preflight(page);
  await page.waitForFunction(() => window.roomJoins.length === 1);
  await page.locator("#editor").fill("previous attempt code");
  const firstTitle = await page.locator("#problem-title").textContent();
  assert.equal(await page.locator("#end").isEnabled(), true);
  assert.equal(await page.locator("#pause").isEnabled(), true);
  assert.equal(calls[0].selectionMode, "strict");
  assert.equal(calls[0].interviewerVoice, "Puck");
  assert.equal(calls[0].interviewerAccent, "British");
  assert.deepEqual(calls[0].difficulties, ["Easy"]);
  assert.equal(calls[0].codeExecution, undefined);
  assert.equal(calls[0].problemId, undefined);
  assert.equal(calls[0].previousAssignment, undefined);
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    "receipt-1",
  );
  // A transport reconnect reuses this attempt and does not reallocate.
  await page.evaluate(() => {
    for (const event of ["Reconnecting", "Reconnected"])
      for (const callback of window.roomEvents[event] ?? []) callback();
  });
  assert.equal(calls.length, 1);
  await page.reload();
  assert.equal(
    warnings,
    1,
    "the existing unsaved-interview warning still runs",
  );
  assert.equal(await page.locator("#audio-check").isVisible(), true);
  assert.equal(calls.length, 1);
  assert.equal(await page.locator("#editor").inputValue(), "");
  await page.locator("#interviewer-voice").selectOption("Random");
  await page.locator("#interviewer-accent").selectOption("Random");
  await preflight(page);
  await page.waitForFunction(() => window.roomJoins.length === 1);
  assert.notEqual(
    await page.locator("#problem-title").textContent(),
    firstTitle,
  );
  assert.equal(calls[1].interviewerVoice, "Random");
  assert.equal(calls[1].interviewerAccent, "Random");
  assert.deepEqual(calls[1].difficulties, ["Easy"]);
  assert.deepEqual(new URL(page.url()).searchParams.getAll("difficulty"), [
    "Easy",
  ]);
  assert.equal(calls[1].previousAssignment, "receipt-1");
  assert.equal(calls[1].problemId, undefined);
  assert.equal(
    await page.evaluate(() => window.roomJoins[0].token),
    "receipt-2",
  );
  assert.notEqual(
    await page.locator("#editor").inputValue(),
    "previous attempt code",
  );
});

test("Practice reopens its URL problem and keeps the existing offline fallback", async (t) => {
  const calls = [];
  const page = await interview(t, "problem=chargeback-pair-match", (route) => {
    calls.push(route.request().postDataJSON());
    return route.fulfill({ status: 503, json: { error: "No provider" } });
  });
  if (!page) return;
  for (let attempt = 0; attempt < 2; attempt++) {
    if (attempt) await page.reload();
    await preflight(page);
    await page.waitForFunction(() =>
      document
        .querySelector("#transcript-panel")
        .textContent.includes("Offline mode is ready"),
    );
    assert.equal(calls[attempt].problemId, "chargeback-pair-match");
    assert.equal(calls[attempt].selectionMode, undefined);
    assert.equal(calls[attempt].difficulties, undefined);
    assert.equal(await page.locator("#editor").isEnabled(), true);
  }
});

test("a refused Strict assignment never starts offline practice or spends the previous receipt", async (t) => {
  const page = await interview(
    t,
    "selection=strict&difficulty=Easy",
    (route) => {
      const request = route.request().postDataJSON();
      assert.deepEqual(request.difficulties, ["Easy"]);
      assert.equal(request.previousAssignment, "previous");
      return route.fulfill({
        status: 409,
        json: {
          code: "strict_no_alternative",
          error:
            "No different problem is available at the selected difficulties.",
        },
      });
    },
  );
  if (!page) return;
  await page.evaluate(
    (key) => sessionStorage.setItem(key, "previous"),
    STRICT_ASSIGNMENT_KEY,
  );
  await preflight(page);
  await page.waitForFunction(
    () =>
      document.querySelector("#audio-check-join").textContent ===
      "Retry media preflight",
  );
  assert.equal(await page.locator("#audio-check").isVisible(), true);
  assert.match(
    await page.locator("#audio-check-status").textContent(),
    /No different problem.*selected difficulties/,
  );
  assert.deepEqual(new URL(page.url()).searchParams.getAll("difficulty"), [
    "Easy",
  ]);
  assert.equal(await page.locator("#editor").isDisabled(), true);
  assert.equal(await page.evaluate(() => window.roomJoins.length), 0);
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    "previous",
  );
  assert.doesNotMatch(
    await page.locator("#transcript-panel").textContent(),
    /Offline mode/,
  );
});

test("a missing Strict problem retains its receipt without loading the fallback into an interview", async (t) => {
  const page = await interview(t, "selection=strict", (route) =>
    route.fulfill({ json: grant("missing-problem", "new-receipt") }),
  );
  if (!page) return;
  await preflight(page);
  await page.waitForFunction(
    () =>
      document.querySelector("#audio-check-join").textContent ===
      "Retry media preflight",
  );
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    "new-receipt",
  );
  assert.equal(await page.evaluate(() => window.roomJoins.length), 0);
  assert.equal(
    await page.locator("#problem-title").textContent(),
    "Strict Interview",
  );
});

test("blocked storage refuses Strict before requesting an attempt", async (t) => {
  let calls = 0;
  const page = await interview(t, "selection=strict", (route) => {
    calls++;
    return route.fulfill({ json: grant("chargeback-pair-match", "receipt") });
  });
  if (!page) return;
  await page.evaluate(() => {
    Storage.prototype.setItem = () => {
      throw new Error("blocked");
    };
  });
  await preflight(page);
  await page.waitForFunction(
    () =>
      document.querySelector("#audio-check-join").textContent ===
      "Retry media preflight",
  );
  assert.equal(calls, 0);
  assert.match(
    await page.locator("#audio-check-status").textContent(),
    /session storage/,
  );
});

test("two Strict tabs keep separate previous assignments in the same browser context", async (t) => {
  const firstCalls = [];
  const first = await interview(t, "selection=strict", (route) => {
    firstCalls.push(route.request().postDataJSON());
    return route.fulfill({ json: grant("chargeback-pair-match", "first-tab") });
  });
  if (!first) return;
  await preflight(first);
  await first.waitForFunction(() => window.roomJoins.length === 1);
  const secondCalls = [];
  const second = await interview(
    t,
    "selection=strict&problem=two-sum",
    (route) => {
      secondCalls.push(route.request().postDataJSON());
      return route.fulfill({
        json: grant("kiosk-panel-mirror-audit", "second-tab"),
      });
    },
    first.context(),
  );
  await preflight(second);
  await second.waitForFunction(() => window.roomJoins.length === 1);
  assert.equal(firstCalls[0].previousAssignment, undefined);
  assert.equal(secondCalls[0].previousAssignment, undefined);
  assert.equal(
    await first.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    "first-tab",
  );
  assert.equal(
    await second.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    "second-tab",
  );
});

test("a receipt write failing after allocation cannot reveal or join the assigned problem", async (t) => {
  let calls = 0;
  const page = await interview(t, "selection=strict", async (route) => {
    calls++;
    await page.evaluate(() => {
      Storage.prototype.setItem = () => {
        throw new Error("quota");
      };
    });
    await route.fulfill({ json: grant("chargeback-pair-match", "new") });
  });
  if (!page) return;
  await preflight(page);
  await page.waitForFunction(
    () =>
      document.querySelector("#audio-check-join").textContent ===
      "Retry media preflight",
  );
  assert.equal(calls, 1);
  assert.equal(
    await page.locator("#problem-title").textContent(),
    "Strict Interview",
  );
  assert.equal(await page.evaluate(() => window.roomJoins.length), 0);
});

test("a failed preflight starts no Strict attempt", async (t) => {
  let calls = 0;
  const page = await interview(t, "selection=strict", (route) => {
    calls++;
    return route.fulfill({ status: 500 });
  });
  if (!page) return;
  // The output confirmation is deliberately absent, even after microphone
  // sampling has succeeded and the optional camera has been skipped.
  await page.locator("#camera-skip").click();
  assert.equal(await page.locator("#audio-check-join").isDisabled(), true);
  assert.equal(calls, 0);
});

test("the lobby defaults to Practice and starts Strict without a selected problem URL", async (t) => {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.route("**/api/session", (route) =>
    route.fulfill({
      json: { signedIn: true, user: { login: "candidate" } },
    }),
  );
  await page.route("**/api/reports", (route) =>
    route.fulfill({ json: { reports: [] } }),
  );
  await page.route("**/interview?**", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: "<p>New interview</p>",
    }),
  );
  await page.goto(base);
  for (const option of [
    "highlight",
    "execution",
    "autoIndent",
    "autoClose",
    "monospace",
  ])
    await page.locator(`[name="editor-option"][value="${option}"]`).uncheck();
  assert.equal(await page.locator("#selection-mode").inputValue(), "practice");
  assert.equal(await page.locator(".random-picker").isVisible(), true);
  assert.equal(await page.locator(".problem-picker").isVisible(), true);
  assert.equal(await page.locator(".difficulty-row").isVisible(), true);
  await page.locator("#selection-mode").selectOption("strict");
  assert.equal(await page.locator(".random-picker").isVisible(), false);
  assert.equal(await page.locator(".problem-picker").isVisible(), false);
  assert.equal(await page.locator(".difficulty-row").isVisible(), true);
  await page.locator('[name="difficulty"][value="Easy"]').check();
  await page.locator('[name="difficulty"][value="Hard"]').check();
  await page.locator('[name="difficulty"][value="Medium"]').uncheck();
  await page.locator("#selection-mode").selectOption("practice");
  assert.equal(await page.locator(".random-picker").isVisible(), true);
  assert.equal(await page.locator(".problem-picker").isVisible(), true);
  await page.locator("#selection-mode").selectOption("strict");
  await page.evaluate(
    (key) => sessionStorage.setItem(key, "previous"),
    STRICT_ASSIGNMENT_KEY,
  );
  await page.locator("#start").click();
  await page.waitForURL("**/interview?**");
  const url = new URL(page.url());
  assert.equal(url.searchParams.get("selection"), "strict");
  assert.deepEqual(url.searchParams.getAll("difficulty"), ["Easy", "Hard"]);
  assert.equal(url.searchParams.has("problem"), false);
  for (const parameter of [
    "editorHighlight",
    "editorExecution",
    "editorAutoIndent",
    "editorAutoClose",
    "editorMonospace",
  ])
    assert.equal(url.searchParams.get(parameter), "0", parameter);
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    null,
  );
});

test("restoring a Strict page from history returns to preflight instead of reviving the attempt", async (t) => {
  let calls = 0;
  const page = await interview(t, "selection=strict", (route) => {
    calls++;
    return route.fulfill({
      json: grant("chargeback-pair-match", "history-receipt"),
    });
  });
  if (!page) return;
  await preflight(page);
  await page.locator("#editor").fill("previous attempt code");
  const reloaded = page.waitForEvent("domcontentloaded");
  // Chromium's automation disables BFCache. Exercise its persisted-pageshow
  // signal, as the lobby restore tests do, without weakening the unload path.
  await page.evaluate(() =>
    window.dispatchEvent(
      new PageTransitionEvent("pageshow", { persisted: true }),
    ),
  );
  await reloaded;
  await page.waitForFunction(
    () =>
      document.querySelector("#problem-title").textContent ===
      "Strict Interview",
  );
  assert.equal(await page.locator("#audio-check").isVisible(), true);
  assert.equal(await page.locator("#editor").inputValue(), "");
  assert.equal(calls, 1);
  assert.equal(
    await page.evaluate(
      (key) => sessionStorage.getItem(key),
      STRICT_ASSIGNMENT_KEY,
    ),
    "history-receipt",
  );
});

test("Strict assignment, reconnect and refresh retain disabled editor assistance and execution", async (t) => {
  const calls = [];
  const pages = ["chargeback-pair-match", "kiosk-panel-mirror-audit"];
  const query = new URLSearchParams({
    selection: "strict",
    problem: "two-sum",
    editorHighlight: "0",
    editorExecution: "0",
    editorAutoIndent: "0",
    editorAutoClose: "0",
    editorMonospace: "0",
  });
  const page = await interview(t, query, (route) => {
    calls.push(route.request().postDataJSON());
    return route.fulfill({
      json: grant(pages[calls.length - 1], `options-${calls.length}`),
    });
  });
  if (!page) return;
  const parsers = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.startsWith("/vendor/tree-sitter"))
      parsers.push(request.url());
  });
  assert.equal(calls.length, 0);
  for (let attempt = 0; attempt < 2; attempt++) {
    if (attempt) await page.reload();
    assert.equal(await page.locator("#audio-check").isVisible(), true);
    assert.equal(calls.length, attempt);
    await preflight(page);
    await page.locator("#editor:not([disabled])").waitFor();
    const request = calls[attempt];
    assert.equal(request.selectionMode, "strict");
    assert.equal(
      request.difficulties,
      undefined,
      "old Strict URLs keep an unrestricted draw",
    );
    assert.equal(request.problemId, undefined);
    assert.equal(request.codeExecution, false);
    assert.equal(request.previousAssignment, attempt ? "options-1" : undefined);
    assert.equal(await page.locator("#editor-highlight").isVisible(), false);
    assert.equal(
      await page
        .locator(".editor-stack")
        .evaluate((element) =>
          element.style.getPropertyValue("--editor-font-family"),
        ),
      "inherit",
    );
    await page.locator("#editor").fill("");
    await page.keyboard.type("(");
    assert.equal(await page.locator("#editor").inputValue(), "(");
    await page.locator("#editor").fill("    if ready:");
    await page.keyboard.press("End");
    await page.keyboard.press("Enter");
    assert.equal(await page.locator("#editor").inputValue(), "    if ready:\n");

    await page.locator('[data-language="javascript"]').click();
    await page.locator("#editor").fill("const answer = 1;");
    const code = await page.locator("#editor").inputValue();
    for (const chord of ["Control+Enter", "Meta+Enter"])
      await page.keyboard.press(chord);
    assert.equal(await page.locator("#editor").inputValue(), code);
    await page.evaluate(() => {
      for (const event of ["Reconnecting", "Reconnected"])
        for (const callback of window.roomEvents[event] ?? []) callback();
    });
    assert.equal(await page.locator("#run-tests").isVisible(), false);
    assert.equal(await page.locator("#run-tests").isDisabled(), true);
    assert.equal(await page.locator(".test-drawer").isVisible(), false);
    assert.equal(await page.locator("#editor").inputValue(), code);
    assert.equal(calls.length, attempt + 1);
  }
  assert.deepEqual(parsers, []);
});

for (const [levels, assigned] of [
  [["Easy"], "chargeback-pair-match"],
  [["Medium"], "carousel-slot-shift"],
  [["Hard"], "terrain-runoff-pooling"],
  [["Easy", "Hard"], "terrain-runoff-pooling"],
]) {
  test(`Strict forwards ${levels.join("/")} to the server and loads its assignment`, async (t) => {
    const calls = [];
    const query = new URLSearchParams({
      selection: "strict",
      problem: "template-delimiter-audit",
    });
    for (const level of levels) query.append("difficulty", level);
    const page = await interview(t, query, (route) => {
      calls.push(route.request().postDataJSON());
      return route.fulfill({ json: grant(assigned, "difficulty-receipt") });
    });
    if (!page) return;
    await preflight(page);
    await page.waitForFunction(() => window.roomJoins.length === 1);
    assert.equal(calls.length, 1);
    assert.deepEqual(calls[0].difficulties, levels);
    assert.equal(calls[0].problemId, undefined);
    const problem = await page.evaluate(
      async (pageName) => (await fetch(`/problems/${pageName}.json`)).json(),
      assigned,
    );
    assert.equal(
      await page.locator("#problem-title").textContent(),
      problem.title,
    );
    assert.ok(levels.includes(problem.difficulty));
  });
}

test("an invalid Strict URL difficulty reaches server validation without broadening the selection", async (t) => {
  const calls = [];
  const page = await interview(
    t,
    "selection=strict&difficulty=Easy&difficulty=Impossible&problem=two-sum",
    (route) => {
      calls.push(route.request().postDataJSON());
      return route.fulfill({
        status: 400,
        json: {
          code: "strict_difficulty_invalid",
          error:
            "Choose one or more distinct difficulties: Easy, Medium or Hard.",
        },
      });
    },
  );
  if (!page) return;
  await preflight(page);
  await page.waitForFunction(
    () =>
      document.querySelector("#audio-check-join").textContent ===
      "Retry media preflight",
  );
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].difficulties, ["Easy", "Impossible"]);
  assert.equal(calls[0].problemId, undefined);
  assert.equal(await page.evaluate(() => window.roomJoins.length), 0);
  assert.match(
    await page.locator("#audio-check-status").textContent(),
    /distinct difficulties/,
  );
  assert.doesNotMatch(
    await page.locator("#transcript-panel").textContent(),
    /Offline mode/,
  );
});

test("Practice still filters by difficulty and opens a specifically chosen problem after switching modes", async (t) => {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.route("**/api/session", (route) =>
    route.fulfill({
      json: { signedIn: true, user: { login: "candidate" } },
    }),
  );
  await page.route("**/api/reports", (route) =>
    route.fulfill({ json: { reports: [] } }),
  );
  await page.route("**/interview?**", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: "<p>New interview</p>",
    }),
  );
  await page.goto(base);
  await page.locator("#start:not([disabled])").waitFor();
  await page.locator("#selection-mode").selectOption("strict");
  await page.locator("#selection-mode").selectOption("practice");
  await page.locator('[name="difficulty"][value="Easy"]').check();
  await page.locator('[name="difficulty"][value="Medium"]').uncheck();
  await page.locator(".problem-picker summary").click();
  const chosen = "chargeback-pair-match";
  assert.equal(
    await page.locator('[data-problem="carousel-slot-shift"]').isVisible(),
    false,
  );
  await page.locator(`[data-problem="${chosen}"]`).click();
  await page.locator("#start").click();
  await page.waitForURL("**/interview?**");
  const params = new URL(page.url()).searchParams;
  assert.equal(params.get("problem"), chosen);
  assert.equal(params.has("selection"), false);
  assert.equal(
    params.has("difficulty"),
    false,
    "Practice URL shape is unchanged",
  );
});
