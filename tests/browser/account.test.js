// Run with: node --test tests/browser/*.test.js

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import {
  clearReportHistory,
  deleteReport,
  historyKey,
  readDeviceHistory,
  readLocalHistory,
  readReviewHistory,
  renameLocalHistory,
  reportIsPersisted,
  reviewHistoryKey,
  saveReportHistory,
} from "../../web/history.js";
import {
  assertIncludesCompact,
  functionBody,
  memoryStorage,
  root,
} from "./source.js";

const web = join(root, "web");
const read = (name) => readFileSync(join(web, name), "utf8");

test("lobby exposes signed in and signed out account hooks", () => {
  const page = read("index.html");
  const script = read("app.js");

  for (const id of [
    "account-status",
    "github-login",
    "login-link",
    "logout",
    "history-header",
    "history",
    "delete-reports",
    "report-delete-status",
  ]) {
    assert.match(page, new RegExp(`id="${id}"`), `index.html must keep #${id}`);
  }
  assert.match(script, /fetch\("\/api\/login", \{/);
  assert.ok(
    script.includes('replace(/^@+/, "")'),
    "GitHub username entry accepts a leading @ handle",
  );
  assert.doesNotMatch(script, /window\.location\.href = "\/api\/login"/);
  assert.match(script, /fetchJson\("\/api\/session"\)/);
  assert.match(script, /fetchJson\("\/api\/reports"\)/);
  assert.match(script, /fetch\("\/api\/logout", \{ method: "POST" \}\)/);
  assert.match(script, /readDeviceHistory\(\)/);
  assert.match(script, /clearReportHistory\(\{ account: accountHistory \}\)/);
  assert.match(
    script,
    /setStartGate\(true\)/,
    "GitHub username must gate interview start when required",
  );
});

test("interview history routes through the shared persistence helper", () => {
  const script = read("interview.js");
  const saveHistory = functionBody(script, "saveHistory");

  // Named, not the whole import list: interview.js also takes the id generator
  // from this module, and pinning the exact list made adding that a test
  // failure about wiring the test does not care about.
  assert.match(
    script,
    /import \{[^}]*\bsaveReportHistory\b[^}]*\} from "\.\/history\.js"/,
  );
  assert.match(saveHistory, /await saveReportHistory\(entry\)/);
  assert.match(saveHistory, /reportIsPersisted\(result\)/);
  assert.match(saveHistory, /state\.reportPersisted = true/);
  for (const name of ["receiveReport", "showReport"]) {
    const body = functionBody(script, name);
    assert.match(body, /renderReport\(\)/);
    // An agent report also waits on its receipt before Done may navigate.
    const awaited = body.search(/await (saving|Promise\.all\(\[saving\b)/);
    assert.ok(awaited !== -1, `${name} awaits the history save`);
    assert.match(
      body,
      /renderReportSaveStatus\(await saving\)|await Promise\.all\(\[saving, flushed\]\)[\s\S]*renderReportSaveStatus\(/,
    );
    assert.ok(body.indexOf("renderReport()") < awaited);
  }
  assert.match(functionBody(script, "renderReport"), /saveResult: null/);
  assert.match(
    functionBody(script, "renderReportSaveStatus"),
    /querySelector\("#done"\)\.disabled = false/,
  );
  assert.match(
    functionBody(script, "renderReport"),
    /nodes\.ending\.hidden = true/,
  );
});

test("only a report copy available to Past attempts counts as persisted", () => {
  assert.equal(reportIsPersisted({ local: "saved", account: "failed" }), true);
  assert.equal(reportIsPersisted({ local: "failed", account: "saved" }), true);
  assert.equal(
    reportIsPersisted({ local: "failed", account: "skipped" }),
    false,
  );
  assert.equal(
    reportIsPersisted({ local: "failed", account: "failed" }),
    false,
  );
  assert.equal(reportIsPersisted(null), false);
});

test("a completed test run keeps pause and round locks", () => {
  const interview = read("interview.js");

  // Both places that hand the runner back use the same guard. A finished test
  // run was the one that got it right; resuming from a pause was the one that
  // did not, and gave the editor and the runner back during the behavioral
  // round that had just retired them.
  assert.match(
    functionBody(interview, "runTests"),
    /updateRunAvailability\(\)/,
  );
  const applyPause = functionBody(interview, "applyPause");
  assert.match(
    applyPause,
    /nodes\.editor\.disabled = paused \|\| codingClosed\(\)/,
  );
  assert.match(applyPause, /updateRunAvailability\(\)/);
  // Asserted as two facts rather than one spelling: the guard is `codingClosed`
  // itself now, not a DOM attribute that happens to track it.
  const updateRunAvailability = functionBody(
    interview,
    "updateRunAvailability",
  );
  assert.match(updateRunAvailability, /codingClosed\(\)/);
  assert.match(
    updateRunAvailability,
    /!languages\.includes\(state\.language\)/,
  );
  assert.match(
    functionBody(interview, "codingClosed"),
    /frameworkRound === "behavioral" \|\| state\.phase !== "live"/,
  );
});

test("the lobby offers an editor or a whiteboard and carries the choice into the room", () => {
  const lobby = read("index.html");
  const app = read("app.js");
  const page = read("interview.html");
  const interview = read("interview.js");

  // The mode the lobby offers is which surface the interview is held on, and
  // that is the only thing it may be. The practice/scored split this replaced
  // was a choice on screen about how hard the interview counted, and a stray
  // button is exactly how it would come back, so the values are asserted
  // rather than the attribute's absence.
  assert.deepEqual(
    [...lobby.matchAll(/data-mode="([^"]*)"/g)].map((match) => match[1]),
    ["coding", "whiteboard"],
  );
  // Both ends of the link, since the payload line below holds whatever `mode`
  // is: dropping either one leaves every whiteboard interview running as a
  // coding interview with this test still green.
  assert.match(app, /destination\.searchParams\.set\("mode", mode\)/);
  assert.match(interview, /modeIsWhiteboard\(params\.get\("mode"\)\)/);
  assert.doesNotMatch(app + lobby + interview, /"(practice|scored)"/);
  // Pause stayed; the two coaching controls went with the mode that gated them.
  assert.match(page, /id="pause"/);
  assert.doesNotMatch(interview, /retryPractice/);
  // Pause moves no deadline any more; see the note in applyPause.
  assert.doesNotMatch(interview + read("lib.js"), /resumeDeadline/);
  assert.doesNotMatch(interview, /practiceGuide/);
  // One framework on screen at a time, ticked from what the interviewer banks,
  // and an offer that leaves on its own while the candidate is waiting on Jim.
  assert.match(page, /id="framework-progress"/);
  assert.match(page, /id="framework-hint"/);
  // Centred over the page, and inert on the way past: it can appear while the
  // candidate is typing, so it must not take focus or swallow a click.
  const css = read("styles.css");
  // Centred by the same rule the ending overlay uses, differing only where it
  // means to. Asserted as composition rather than as a second copy of the
  // positioning, because a duplicate block is what painted a border around the
  // whole viewport: only the properties the later rule named were overridden.
  assert.match(page, /class="overlay framework-hint"/);
  assert.match(css, /\.overlay \{[^}]*position: fixed;/s);
  assert.equal(
    css.match(/^\.framework-hint \{/gm).length,
    1,
    "a second rule block overrides only part of the first",
  );
  // It must paint nothing: the page behind it is what the candidate is being
  // asked about, and the card is bounded so it cannot fill the screen.
  assert.match(
    css,
    /\.framework-hint \{[^}]*background: none;[^}]*pointer-events: none;/s,
  );
  assert.match(css, /\.framework-hint-card \{[^}]*max-height: 70vh;/s);
  assert.match(css, /\.framework-hint-card \{[^}]*max-width: min\(/s);
  assert.doesNotMatch(interview, /showModal\(\)/);
  assert.match(interview, /frameworkHintBody\.innerHTML/);
  assert.match(interview, /<caption>\$\{escapeHtml\(framework\.name\)\}/);
  assert.doesNotMatch(interview, /frameworkBriefing/);
  assert.doesNotMatch(interview, /Two shapes fit this interview/);
  assert.doesNotMatch(page, /REACTO: Repeat/);
  assertIncludesCompact(
    interview,
    'message.type === "framework_state" && Array.isArray(message.phases)',
  );
  assert.match(interview, /frameworkRound = "behavioral"/);
  assert.match(
    interview,
    /globalThis\.setTimeout\(\(\) => \{\s*nodes\.frameworkHint\.hidden = true;/,
  );
  assertIncludesCompact(
    interview,
    "JSON.stringify({ problemId: problem.page, durationMin, interviewId, interviewLoop, interviewMode: mode, interviewProfile, ...(interviewGrounding",
  );
  assertIncludesCompact(
    interview,
    "durationMin, interviewLoop, interviewMode: mode, report, };",
  );
});

test("interview loop is explicit, budgeted, gated, and carried into artifacts", () => {
  const lobby = read("index.html");
  const app = read("app.js");
  const page = read("interview.html");
  const interview = read("interview.js");
  assert.match(lobby, /data-loop="coding_only"/);
  assert.match(
    lobby,
    /data-loop="coding_behavioral"[^>]*class="[^"]*selected|class="[^"]*selected"[^>]*data-loop="coding_behavioral"/,
  );
  assert.match(app, /let interviewLoop = "coding_behavioral"/);
  assert.match(app, /destination\.searchParams\.set\("loop", interviewLoop\)/);
  assert.match(page, /id="round-plan-summary"/);
  assertIncludesCompact(
    interview,
    'behavioralMinutes = interviewLoop === "coding_behavioral" ? Math.min(8, durationMin) : 0',
  );
  assertIncludesCompact(
    interview,
    'type: "round_transition", round: "behavioral"',
  );
  assertIncludesCompact(
    interview,
    'recordReplay("lifecycle", { state: "round_reserve_started"',
  );
  assert.match(interview, /message\.type === "round_state"/);
  assert.match(interview, /nodes\.editor\.disabled = true/);
  assert.match(interview, /state: "rounds_final"/);
});

test("optional interview profile is accessible, bounded, and omitted when blank", () => {
  const lobby = read("index.html");
  const app = read("app.js");
  const interview = read("interview.js");
  assert.match(lobby, /<summary>Optional interview context<\/summary>/);
  assert.match(
    lobby,
    /<fieldset>[\s\S]*<legend>Tailor the behavioral question<\/legend>/,
  );
  for (const id of [
    "profile-role",
    "profile-seniority",
    "profile-company",
    "practice-focus-share",
    "practice-focus-share-input",
  ]) {
    assert.match(lobby, new RegExp(`id="${id}"`));
  }
  assert.match(lobby, /id="profile-role"[^>]*maxlength="80"/);
  assert.match(lobby, /id="profile-company"[^>]*maxlength="80"/);
  for (const value of [
    "intern",
    "junior",
    "mid",
    "senior",
    "staff",
    "manager",
  ]) {
    assert.match(lobby, new RegExp(`<option value="${value}">`));
  }
  assert.match(
    app,
    /if \(profile\.role\) destination\.searchParams\.set\("role", profile\.role\)/,
  );
  assertIncludesCompact(
    app,
    'if (profile.seniority) destination.searchParams.set("seniority", profile.seniority)',
  );
  assertIncludesCompact(
    app,
    'if (profile.targetCompany) destination.searchParams.set("company", profile.targetCompany)',
  );
  assert.match(interview, /const interviewProfile = \{/);
  // The focus travels in session storage, never the address bar.
  assert.match(
    app,
    /storeSharedFocus\(sessionStorage, focus\?\.weakness \?\? null\)/,
  );
  assert.doesNotMatch(app, /searchParams\.set\("focus"/);
  assert.match(interview, /practiceFocus: consumeSharedFocus\(tabStorage\)/);
});

test("document grounding is explicit, clearable, ephemeral, and absent from saved artifacts", () => {
  const lobby = read("index.html");
  const app = read("app.js");
  const interview = read("interview.js");
  for (const id of [
    "grounding-jd",
    "grounding-resume",
    "grounding-choices",
    "grounding-consent",
    "grounding-clear",
    "grounding-error",
  ]) {
    assert.match(lobby, new RegExp(`id="${id}"`));
  }
  assert.match(
    lobby,
    /Send\s+only\s+my\s+selected\s+snippets\s+to\s+the\s+AI\s+interviewer\./,
  );
  assert.match(
    app,
    /nodes\.groundingClear\.addEventListener\("click", clearGrounding\)/,
  );
  assert.match(app, /storeGroundingPacket\(sessionStorage, packet\)/);
  assert.match(interview, /consumeGroundingPacket\(tabStorage\)/);
  const savedArtifacts = [
    read("history.js"),
    read("replay-feed.js"),
    functionBody(interview, "saveHistory"),
  ];
  for (const source of savedArtifacts) {
    assert.doesNotMatch(source, /interviewGrounding/);
    assert.doesNotMatch(source, /practiceFocus/);
  }
});

test("report history writes local storage before account sync", async () => {
  const storage = memoryStorage();
  const entry = {
    id: "r1",
    problemId: "two-sum",
    report: { decision: "HIRE" },
  };
  let releaseSession;
  const session = new Promise((resolve) => {
    releaseSession = () => resolve(response({ signedIn: true }));
  });
  const posts = [];
  const saved = saveReportHistory(entry, {
    storage,
    fetcher: async (url, options) => {
      if (url === "/api/session") return session;
      posts.push({ url, options });
      return response({ id: "r1" });
    },
  });

  assert.equal(JSON.parse(storage.getItem(historyKey))[0].id, "r1");
  assert.deepEqual(posts, []);

  releaseSession();
  assert.deepEqual(await saved, { local: "saved", account: "saved" });
  assert.equal(posts.length, 1);
  assert.equal(posts[0].url, "/api/reports");
  assert.equal(posts[0].options.method, "POST");
});

test("a report saved again under its id replaces the earlier copy", async () => {
  const storage = memoryStorage();
  const fetcher = async () => response({ signedIn: false });
  await saveReportHistory(
    { id: "older", problemId: "two-sum", report: { decision: "HIRE" } },
    { storage, fetcher },
  );
  for (const summary of ["provisional failure", "regenerated"]) {
    await saveReportHistory(
      {
        id: "interview-report",
        problemId: "two-sum",
        report: { incomplete: summary !== "regenerated", summary },
      },
      { storage, fetcher },
    );
  }
  for (const rows of [readLocalHistory(storage), readReviewHistory(storage)]) {
    assert.deepEqual(
      rows.map((row) => row.id),
      ["interview-report", "older"],
    );
    assert.equal(rows[0].report.summary, "regenerated");
  }
});

test("account saves of one report id land in the order they were made", async () => {
  const posted = [];
  let releaseFirst;
  const firstSession = new Promise((resolve) => {
    releaseFirst = () => resolve(response({ signedIn: true }));
  });
  let sessions = 0;
  const fetcher = async (url, options) => {
    if (url === "/api/session")
      return ++sessions === 1 ? firstSession : response({ signedIn: true });
    posted.push(JSON.parse(options.body).report.summary);
    return response({ id: "same" });
  };
  const entry = (summary) => ({
    id: "same",
    problemId: "two-sum",
    report: { incomplete: true, summary },
  });
  const provisional = saveReportHistory(entry("provisional"), {
    storage: memoryStorage(),
    fetcher,
  });
  const final = saveReportHistory(entry("final"), {
    storage: memoryStorage(),
    fetcher,
  });
  // The second save's own session check would answer at once; it still waits.
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(posted, []);
  releaseFirst();
  assert.deepEqual(await provisional, { local: "saved", account: "saved" });
  assert.deepEqual(await final, { local: "saved", account: "saved" });
  assert.deepEqual(posted, ["provisional", "final"]);
});

test("review inputs survive the full-report cap", async () => {
  const storage = memoryStorage();
  for (let index = 0; index < 21; index += 1) {
    await saveReportHistory(
      {
        problemId: `problem-${index}`,
        date: `2026-01-${String(index + 1).padStart(2, "0")}T00:00:00Z`,
        report: {
          decision: index === 0 ? "NO_HIRE" : "HIRE",
          incomplete: false,
          improvementPlan: [],
        },
      },
      { storage, fetcher: async () => response({ signedIn: false }) },
    );
  }
  assert.equal(readLocalHistory(storage).length, 20);
  const reviews = readReviewHistory(storage);
  assert.equal(reviews.length, 21);
  assert.equal(reviews.at(-1).problemId, "problem-0");
  const { id, ...oldest } = reviews.at(-1);
  assert.equal(typeof id, "string");
  assert.deepEqual(oldest, {
    problemId: "problem-0",
    date: "2026-01-01T00:00:00Z",
    report: { decision: "NO_HIRE", incomplete: false, improvementPlan: [] },
  });
});

test("a rebuilt review history keeps the local-storage budget", () => {
  const storage = memoryStorage();
  storage.setItem(
    historyKey,
    JSON.stringify(
      Array.from({ length: 20 }, (_, index) => ({
        problemId: `problem-${index}`,
        report: { summary: "x".repeat(20_000) },
      })),
    ),
  );
  const reviews = readReviewHistory(storage);
  assert.ok(
    reviews.length < 20,
    "oversized full reports cannot bypass the review budget",
  );
  assert.ok(
    new TextEncoder().encode(storage.getItem(reviewHistoryKey)).length <=
      164 * 1024,
  );
});

test("a rebuilt review history remains readable when storage is read-only", () => {
  const entries = [{ id: "readable", problemId: "two-sum" }];
  const storage = {
    getItem: (key) => (key === historyKey ? JSON.stringify(entries) : null),
    setItem: () => {
      throw new Error("quota");
    },
  };
  assert.deepEqual(readReviewHistory(storage), entries);
});

test("device history is empty when local storage access is forbidden", () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    get: () => {
      throw new Error("forbidden");
    },
  });
  try {
    assert.deepEqual(readDeviceHistory(), []);
  } finally {
    if (previous) Object.defineProperty(globalThis, "localStorage", previous);
    else delete globalThis.localStorage;
  }
});

test("the device history keeps what the review budget dropped, once", () => {
  const storage = memoryStorage();
  const entries = Array.from({ length: 20 }, (_, index) => ({
    id: `attempt-${index}`,
    problemId: `problem-${index}`,
    report: { summary: "x".repeat(20_000) },
  }));
  storage.setItem(historyKey, JSON.stringify(entries));
  assert.ok(
    readReviewHistory(storage).length < 20,
    "the budget has to drop some of these",
  );
  assert.deepEqual(readDeviceHistory(storage), entries);
});

test("legacy attempts receive distinct stable ids across both stores", () => {
  const storage = memoryStorage();
  const date = "2026-01-01T00:00:00Z";
  const history = [
    { problemId: "renamed-a", date, report: { decision: "HIRE" } },
  ];
  const reviews = [
    { problemId: "published-a", date, report: { decision: "HIRE" } },
    { problemId: "problem-b", date, report: { decision: "HIRE" } },
  ];
  storage.setItem(historyKey, JSON.stringify(history));
  storage.setItem(reviewHistoryKey, JSON.stringify(reviews));

  const merged = readDeviceHistory(storage);
  const savedHistory = readLocalHistory(storage);
  const savedReviews = readReviewHistory(storage);
  assert.equal(merged.length, 2);
  assert.notEqual(
    merged[0].id,
    merged[1].id,
    "same-date attempts share no identity",
  );
  assert.equal(
    savedHistory[0].id,
    savedReviews[0].id,
    "a partially renamed copy keeps one identity",
  );
  assert.notEqual(savedReviews[0].id, savedReviews[1].id);
  assert.deepEqual(
    readDeviceHistory(storage),
    merged,
    "the assigned ids survive another read",
  );
});

test("legacy id migration leaves a full review store intact", () => {
  const budget = 164 * 1024;
  const reviews = Array.from({ length: 30 }, (_, index) => ({
    problemId: `problem-${index}`,
    report: { summary: "" },
  }));
  const encoder = new TextEncoder();
  const serialized = () => JSON.stringify(reviews);
  reviews[0].report.summary = "x".repeat(
    budget - encoder.encode(serialized()).length - 1,
  );

  const memory = memoryStorage();
  memory.setItem(historyKey, "[]");
  const stored = serialized();
  memory.setItem(reviewHistoryKey, stored);
  let writes = 0;
  const storage = {
    getItem: (key) => memory.getItem(key),
    setItem: (key, value) => {
      if (key === reviewHistoryKey) writes += 1;
      memory.setItem(key, value);
    },
  };

  const migrated = readDeviceHistory(storage);
  assert.equal(migrated.length, reviews.length);
  assert.ok(migrated.every((entry) => typeof entry.id === "string"));
  assert.equal(writes, 0, "migration does not rewrite an over-budget store");
  assert.equal(memory.getItem(reviewHistoryKey), stored);
});

test("device history survives an origin without crypto.randomUUID", () => {
  // `crypto.randomUUID` exists only in a secure context, so on an http:// origin
  // it is undefined. Assigning ids to pre-id rows must not depend on it: the
  // throw used to reach readDeviceHistory's catch, which answered with no
  // history, and the next save then rebuilt the review store from nothing.
  const storage = memoryStorage();
  const rows = [
    {
      problemId: "two-sum",
      date: "2026-01-01T00:00:00Z",
      report: { decision: "HIRE" },
    },
  ];
  storage.setItem(historyKey, JSON.stringify(rows));
  storage.setItem(reviewHistoryKey, JSON.stringify(rows));

  const original = Object.getOwnPropertyDescriptor(globalThis, "crypto");
  Object.defineProperty(globalThis, "crypto", {
    configurable: true,
    value: {},
  });
  try {
    const merged = readDeviceHistory(storage);
    assert.equal(merged.length, 1, "the attempt is still reachable");
    assert.equal(
      typeof merged[0].id,
      "string",
      "and it was given an id anyway",
    );
  } finally {
    if (original) Object.defineProperty(globalThis, "crypto", original);
    else delete globalThis.crypto;
  }
});

test("a report the review store refuses is still saved on this device", async () => {
  const memory = memoryStorage();
  const storage = {
    getItem: (key) => memory.getItem(key),
    setItem: (key, value) => {
      if (key === reviewHistoryKey) throw new Error("quota");
      memory.setItem(key, value);
    },
  };
  const saved = await saveReportHistory(
    { id: "only-local", problemId: "two-sum", report: { decision: "HIRE" } },
    {
      storage,
      fetcher: async () => response({ signedIn: false }),
    },
  );
  assert.equal(saved.local, "saved");
  assert.deepEqual(
    readLocalHistory(memory).map((entry) => entry.id),
    ["only-local"],
  );
});

test("an oversized newest review does not erase older review history", async () => {
  const storage = memoryStorage();
  storage.setItem(
    reviewHistoryKey,
    JSON.stringify([{ problemId: "kept", report: { decision: "HIRE" } }]),
  );
  await saveReportHistory(
    {
      problemId: "too-large",
      report: { summary: "x".repeat(200_000) },
    },
    { storage, fetcher: async () => response({ signedIn: false }) },
  );
  assert.deepEqual(
    readReviewHistory(storage).map((entry) => entry.problemId),
    ["kept"],
  );
});

test("report history keeps anonymous and failed account saves local", async () => {
  const anonymous = memoryStorage();
  assert.deepEqual(
    await saveReportHistory(
      { id: "anon", problemId: "two-sum" },
      {
        storage: anonymous,
        fetcher: async () => response({ signedIn: false }),
      },
    ),
    { local: "saved", account: "skipped" },
  );
  assert.equal(JSON.parse(anonymous.getItem(historyKey))[0].id, "anon");

  const failed = memoryStorage();
  const calls = [];
  assert.deepEqual(
    await saveReportHistory(
      { id: "fail", problemId: "two-sum" },
      {
        storage: failed,
        fetcher: async (url) => {
          calls.push(url);
          return url === "/api/session"
            ? response({ signedIn: true })
            : response({}, false);
        },
      },
    ),
    { local: "saved", account: "failed" },
  );
  assert.deepEqual(calls, ["/api/session", "/api/reports"]);
  assert.equal(JSON.parse(failed.getItem(historyKey))[0].id, "fail");
});

test("report history exposes every local and account failure", async () => {
  const brokenStorage = {
    getItem: () => null,
    setItem: () => {
      throw new Error("quota");
    },
  };
  assert.deepEqual(
    await saveReportHistory(
      { id: "local-fail" },
      {
        storage: brokenStorage,
        fetcher: async () => response({ signedIn: false }),
      },
    ),
    { local: "failed", account: "skipped" },
  );

  const accountFailure = async (fetcher) =>
    saveReportHistory(
      { id: "account-fail" },
      {
        storage: memoryStorage(),
        fetcher,
      },
    );
  assert.deepEqual(
    await accountFailure(async () => {
      throw new Error("offline");
    }),
    { local: "saved", account: "failed" },
  );
  assert.deepEqual(await accountFailure(async () => response(null)), {
    local: "saved",
    account: "failed",
  });
  assert.deepEqual(
    await accountFailure(async () => ({
      ok: true,
      json: async () => {
        throw new Error("bad json");
      },
    })),
    { local: "saved", account: "failed" },
  );

  for (const session of [
    response({}, false),
    response({ loginRequired: true }),
  ]) {
    let calls = 0;
    assert.deepEqual(
      await accountFailure(async () => {
        calls += 1;
        return session;
      }),
      { local: "saved", account: "failed" },
    );
    assert.equal(
      calls,
      1,
      "a failed or malformed session must not POST a report",
    );
  }
});

test("report history rechecks signed-out and signed-in storage before clearing", async () => {
  const signedOut = memoryStorage();
  signedOut.setItem(historyKey, "local");
  const signedOutCalls = [];
  assert.equal(
    await clearReportHistory({
      storage: signedOut,
      fetcher: async (url) => {
        signedOutCalls.push(url);
        return response({ signedIn: false });
      },
    }),
    "cleared",
  );
  assert.equal(signedOut.getItem(historyKey), null);
  assert.deepEqual(signedOutCalls, ["/api/session"]);

  const signedIn = memoryStorage();
  signedIn.setItem(historyKey, "local");
  const calls = [];
  const clear = () =>
    clearReportHistory({
      storage: signedIn,
      fetcher: async (url, options) => {
        calls.push({ url, options });
        return url === "/api/session"
          ? response({ signedIn: true })
          : response({ deleted: 1 });
      },
    });
  assert.equal(await clear(), "cleared");
  signedIn.setItem(historyKey, "local-again");
  assert.equal(await clear(), "cleared");
  assert.equal(signedIn.getItem(historyKey), null);
  assert.deepEqual(
    calls.map(({ url }) => url),
    ["/api/session", "/api/reports", "/api/session", "/api/reports"],
  );
  assert.equal(calls[1].options.method, "DELETE");
});

test("report history retains local data on account failure and reports a partial clear", async () => {
  const retained = memoryStorage();
  retained.setItem(historyKey, "keep");
  for (const fetcher of [
    async () => {
      throw new Error("offline");
    },
    async () => response({}, false),
  ]) {
    assert.equal(
      await clearReportHistory({ storage: retained, fetcher }),
      "failed",
    );
    assert.equal(retained.getItem(historyKey), "keep");
  }

  const brokenStorage = {
    removeItem: () => {
      throw new Error("blocked");
    },
  };
  assert.equal(
    await clearReportHistory({
      storage: brokenStorage,
      fetcher: async (url) =>
        url === "/api/session"
          ? response({ signedIn: true })
          : response({ deleted: 1 }),
    }),
    "account-cleared-local-failed",
  );
  assert.equal(
    await clearReportHistory({
      storage: brokenStorage,
      fetcher: async () => response({}),
    }),
    "failed",
  );
});

test("a cross-tab sign-out keeps the account copy rather than claiming a clear", async () => {
  const storage = memoryStorage();
  storage.setItem(historyKey, "keep");
  const calls = [];
  // The lobby loaded account history, then the session went away in another
  // tab. This browser can no longer authenticate the delete, so the account
  // copy is still there and saying it was erased would be a lie.
  assert.equal(
    await clearReportHistory({
      account: true,
      storage,
      fetcher: async (url) => {
        calls.push(url);
        return response({ signedIn: false });
      },
    }),
    "failed",
  );
  assert.equal(storage.getItem(historyKey), "keep");
  assert.deepEqual(calls, ["/api/session"]);
});

test("deleting one report removes every copy of it and nothing else", async () => {
  const storage = memoryStorage();
  // The review store holds the same attempt under its own order, plus an older
  // one the short history has rolled past, so a position means a different
  // report in each store.
  // Dated apart, because an id-less row is paired with its review copy by what
  // else it holds, and rows holding nothing else would all pair.
  const row = (id, day) => ({
    ...(id ? { id } : {}),
    problemId: "p",
    date: `2026-01-0${day}`,
  });
  storage.setItem(
    historyKey,
    JSON.stringify([row("c", 4), row("b", 3), row(null, 2)]),
  );
  storage.setItem(
    reviewHistoryKey,
    JSON.stringify([row("b", 3), row("c", 4), row("a", 1)]),
  );
  const calls = [];
  assert.equal(
    await deleteReport("b", {
      storage,
      fetcher: async (url) => {
        calls.push(url);
        return response({ signedIn: false });
      },
    }),
    "deleted",
  );
  assert.deepEqual(
    calls,
    ["/api/session"],
    "a signed-out browser has no account copy to ask about",
  );
  assert.deepEqual(JSON.parse(storage.getItem(historyKey)), [
    row("c", 4),
    row(null, 2),
  ]);
  assert.deepEqual(
    readReviewHistory(storage).map((entry) => entry.id),
    ["c", "a"],
  );
  assert.deepEqual(
    readDeviceHistory(storage).map((entry) => entry.date),
    ["2026-01-04", "2026-01-02", "2026-01-01"],
  );
});

test("deleting one report asks the account for that id alone", async () => {
  const storage = memoryStorage();
  storage.setItem(
    historyKey,
    JSON.stringify([{ id: "one/off try" }, { id: "kept" }]),
  );
  const calls = [];
  assert.equal(
    await deleteReport("one/off try", {
      account: true,
      storage,
      fetcher: async (url, options) => {
        calls.push({
          url,
          method: options?.method,
          deadline: options?.signal instanceof AbortSignal,
        });
        return url === "/api/session"
          ? response({ signedIn: true })
          : response({ deleted: 1 });
      },
    }),
    "deleted",
  );
  assert.deepEqual(calls, [
    { url: "/api/session", method: undefined, deadline: true },
    { url: "/api/reports/one%2Foff%20try", method: "DELETE", deadline: true },
  ]);
  assert.deepEqual(JSON.parse(storage.getItem(historyKey)), [{ id: "kept" }]);
});

test("a report the account kept stays on this device too", async () => {
  const kept = JSON.stringify([{ id: "keep" }]);
  for (const fetcher of [
    async () => {
      throw new Error("offline");
    },
    async (url) =>
      url === "/api/session"
        ? response({ signedIn: true })
        : response({}, false),
    // Signed out in another tab: the account copy cannot be reached, so
    // deleting the local one would claim a deletion that did not happen.
    async () => response({ signedIn: false }),
  ]) {
    const storage = memoryStorage();
    storage.setItem(historyKey, kept);
    assert.equal(
      await deleteReport("keep", { account: true, storage, fetcher }),
      "failed",
    );
    assert.equal(storage.getItem(historyKey), kept);
  }
  assert.equal(
    await deleteReport("", {
      fetcher: async () => assert.fail("no request for no id"),
    }),
    "failed",
  );

  const brokenStorage = {
    getItem: () => JSON.stringify([{ id: "keep" }]),
    setItem: () => {
      throw new Error("blocked");
    },
  };
  const signedIn = async (url) =>
    url === "/api/session"
      ? response({ signedIn: true })
      : response({ deleted: 1 });
  assert.equal(
    await deleteReport("keep", { storage: brokenStorage, fetcher: signedIn }),
    "account-deleted-local-failed",
  );
  assert.equal(
    await deleteReport("keep", {
      storage: brokenStorage,
      fetcher: async () => response({ signedIn: false }),
    }),
    "failed",
  );
});

test("deleting an id no store holds says so instead of claiming a removal", async () => {
  // A pre-id row whose assigned id never reached storage: the rows are still
  // there, so answering "deleted" named a removal the next read undoes.
  const legacy = JSON.stringify([{ problemId: "p", date: "2026-01-01" }]);
  const signedOut = memoryStorage();
  signedOut.setItem(historyKey, legacy);
  assert.equal(
    await deleteReport("assigned", {
      storage: signedOut,
      fetcher: async () => response({ signedIn: false }),
    }),
    "missing",
  );
  assert.equal(signedOut.getItem(historyKey), legacy);

  const signedIn = (deleted) => async (url) =>
    url === "/api/session"
      ? response({ signedIn: true })
      : response({ deleted });
  const storage = memoryStorage();
  storage.setItem(historyKey, legacy);
  assert.equal(
    await deleteReport("assigned", {
      account: true,
      storage,
      fetcher: signedIn(0),
    }),
    "missing",
  );
  // The account held it though this device did not, which is still a removal.
  assert.equal(
    await deleteReport("assigned", {
      account: true,
      storage,
      fetcher: signedIn(1),
    }),
    "deleted",
  );
  // A count of 0 removed nothing, so a local write that then fails is a plain
  // failure rather than an account deletion.
  const brokenStorage = {
    getItem: () => JSON.stringify([{ id: "keep" }]),
    setItem: () => {
      throw new Error("blocked");
    },
  };
  assert.equal(
    await deleteReport("keep", {
      storage: brokenStorage,
      fetcher: signedIn(0),
    }),
    "failed",
  );
});

test("every history request carries a deadline", async () => {
  const deadlines = [];
  const fetcher = async (url, options) => {
    deadlines.push(options?.signal instanceof AbortSignal);
    return url === "/api/session"
      ? response({ signedIn: true })
      : response({ deleted: 1 });
  };
  const storage = memoryStorage();
  await saveReportHistory({ id: "deadline" }, { fetcher, storage });
  await clearReportHistory({ fetcher, storage });
  // Session probe and report POST for the save, then the same pair for the
  // clear. A request without one hangs whatever the candidate is waiting on.
  assert.deepEqual(deadlines, [true, true, true, true]);
});

function response(body, ok = true) {
  return {
    ok,
    json: async () => body,
  };
}

// `event.currentTarget` is null once dispatch ends, so an async handler that
// keeps using it writes to null on every line after its first `await`. That
// shipped: a candidate on a sign-in-gated lobby recorded their name, the start
// handler threw, and the button stayed disabled on "Recording GitHub..." with
// no interview. The rule is cheaper to check than the symptom.
test("no handler in these files reaches for the event's current target", () => {
  for (const name of ["app.js", "interview.js", "history.js"]) {
    assert.doesNotMatch(
      read(name),
      /currentTarget/,
      `${name} uses event.currentTarget, which is null after an await`,
    );
  }
});

// The key lives in the origin's local storage, so what it holds is input: an
// older build, another tab, or anyone with devtools open can leave any JSON
// under it, and every reader downstream treats what comes back as a list.
// Old history carries published ids, and translating them costs the lobby a
// page-map fetch on each visit. Renamed in place once, the next visit has page
// names and fetches nothing.
test("history saved under published ids is renamed to page names once", () => {
  const storage = memoryStorage();
  const pages = { "two-sum": { page: "some-scenario" } };
  storage.setItem(
    historyKey,
    JSON.stringify([
      { problemId: "two-sum", date: "2026-01-01" },
      { problemId: "already-a-page" },
      { problemId: "__proto__" },
      "not an entry",
    ]),
  );
  const renamed = renameLocalHistory(pages, storage);
  assert.deepEqual(renamed[0], {
    problemId: "some-scenario",
    date: "2026-01-01",
  });
  assert.deepEqual(renamed.slice(1), [
    { problemId: "already-a-page" },
    { problemId: "__proto__" },
    "not an entry",
  ]);
  assert.deepEqual(
    readLocalHistory(storage),
    renamed,
    "the rename is written back",
  );

  const unchanged = memoryStorage();
  unchanged.setItem(historyKey, '[{"problemId":"already-a-page"}]');
  let writes = 0;
  const counting = {
    ...unchanged,
    getItem: (key) => unchanged.getItem(key),
    setItem: (...args) => {
      writes += 1;
      unchanged.setItem(...args);
    },
  };
  renameLocalHistory(pages, counting);
  assert.equal(writes, 0, "nothing to rename is nothing written");
});

// The review list is built from the history the first time it is read, so an
// interview saved before any lobby visit copies the published ids into it. The
// rename has to reach that copy too, or those reviews match no card.
test("a review list built before the rename is renamed with the history", async () => {
  const storage = memoryStorage();
  const pages = { "two-sum": { page: "some-scenario" } };
  storage.setItem(
    historyKey,
    JSON.stringify([{ problemId: "two-sum", date: "2026-01-01" }]),
  );
  const offline = async () => ({
    ok: false,
    status: 401,
    json: async () => ({}),
  });
  await saveReportHistory(
    { problemId: "some-scenario", date: "2026-02-01", report: {} },
    { fetcher: offline, storage },
  );
  assert.deepEqual(
    readReviewHistory(storage).map((entry) => entry.problemId),
    ["some-scenario", "two-sum"],
  );

  storage.setItem(
    reviewHistoryKey,
    JSON.stringify([...readReviewHistory(storage), { problemId: "retired" }]),
  );
  renameLocalHistory(pages, storage, true);
  assert.deepEqual(
    readReviewHistory(storage).map((entry) => [
      entry.problemId,
      entry.pageMapChecked ?? false,
    ]),
    [
      ["some-scenario", true],
      ["some-scenario", false],
      ["retired", true],
    ],
    "published ids renamed, and ids the map does not key are marked so the lobby stops asking",
  );
});

test("a stored history that is not a list reads as no history", () => {
  const stored = (value) => {
    const storage = memoryStorage();
    storage.setItem(historyKey, value);
    return readLocalHistory(storage);
  };

  assert.deepEqual(stored("[]"), []);
  assert.deepEqual(stored('[{"problemId":"two-sum"}]'), [
    { problemId: "two-sum" },
  ]);
  // Each of these parses, so only the shape check turns them away, and each
  // failed differently before it: an object was counted as "undefined past
  // reports", "five" as four of them, and a missing key parses to null.
  for (const value of ['{"a":1}', '"five"', "null"]) {
    assert.deepEqual(stored(value), [], `${value} was not read as no history`);
  }
  // And unparseable JSON keeps the answer it already had.
  assert.deepEqual(stored("{not json"), []);
});
