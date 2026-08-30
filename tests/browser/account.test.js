// Run with: node --test tests/browser/*.test.js

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { historyKey, readLocalHistory, saveReportHistory } from "../../web/history.js";

const web = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "web");
const read = (name) => readFileSync(join(web, name), "utf8");

test("lobby exposes signed in and signed out account hooks", () => {
  const page = read("index.html");
  const script = read("app.js");

  for (const id of ["account-status", "github-login", "login-link", "logout", "history"]) {
    assert.match(page, new RegExp(`id="${id}"`), `index.html must keep #${id}`);
  }
  assert.match(script, /fetch\("\/api\/login", \{/);
  assert.ok(script.includes('replace(/^@+/, "")'), "GitHub username entry accepts a leading @ handle");
  assert.doesNotMatch(script, /window\.location\.href = "\/api\/login"/);
  assert.match(script, /fetchJson\("\/api\/session"\)/);
  assert.match(script, /fetchJson\("\/api\/reports"\)/);
  assert.match(script, /fetch\("\/api\/logout", \{ method: "POST" \}\)/);
  assert.match(script, /readLocalHistory\(\)/);
  assert.match(script, /setStartGate\(true\)/, "GitHub username must gate interview start when required");
});

test("interview history routes through the shared persistence helper", () => {
  const script = read("interview.js");
  const saveHistory = script.slice(script.indexOf("function saveHistory"));

  assert.match(script, /import \{ saveReportHistory \} from "\.\/history\.js"/);
  assert.match(saveHistory, /return saveReportHistory\(entry\)/);
});

test("practice and scored modes stay explicit from lobby through artifacts", () => {
  const lobby = read("index.html");
  const app = read("app.js");
  const page = read("interview.html");
  const interview = read("interview.js");

  assert.match(lobby, /data-mode="practice"/);
  assert.match(lobby, /data-mode="scored"[^>]*class="[^"]*selected|class="[^"]*selected"[^>]*data-mode="scored"/);
  assert.match(app, /let mode = "scored"/);
  assert.match(app, /destination\.searchParams\.set\("mode", mode\)/);
  for (const id of ["practice-guide", "pause", "retry"]) assert.match(page, new RegExp(`id="${id}"`));
  assert.match(interview, /params\.get\("mode"\) === "practice" \? "practice" : "scored"/);
  assert.match(interview, /JSON\.stringify\(\{ problemId: problem\.id, durationMin, interviewId, mode, interviewProfile \}\)/);
  assert.match(interview, /mode, report: state\.report/);
  assert.match(interview, /searchParams\.set\("retry", Date\.now\(\)\.toString\(\)\)/);
});

test("optional interview profile is accessible, bounded, and omitted when blank", () => {
  const lobby = read("index.html");
  const app = read("app.js");
  const interview = read("interview.js");
  assert.match(lobby, /<summary>Optional interview context<\/summary>/);
  assert.match(lobby, /<fieldset>[\s\S]*<legend>Tailor the behavioral question<\/legend>/);
  for (const id of ["profile-role", "profile-seniority", "profile-company"]) {
    assert.match(lobby, new RegExp(`id="${id}"`));
  }
  assert.match(lobby, /id="profile-role"[^>]*maxlength="80"/);
  assert.match(lobby, /id="profile-company"[^>]*maxlength="80"/);
  for (const value of ["intern", "junior", "mid", "senior", "staff", "manager"]) {
    assert.match(lobby, new RegExp(`<option value="${value}">`));
  }
  assert.match(app, /if \(profile\.role\) destination\.searchParams\.set\("role", profile\.role\)/);
  assert.match(app, /if \(profile\.seniority\) destination\.searchParams\.set\("seniority", profile\.seniority\)/);
  assert.match(app, /if \(profile\.targetCompany\) destination\.searchParams\.set\("company", profile\.targetCompany\)/);
  assert.match(interview, /const interviewProfile = \{/);
});

test("report history writes local storage before account sync", async () => {
  const storage = memoryStorage();
  const entry = { id: "r1", problemId: "two-sum", report: { decision: "HIRE" } };
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
  assert.equal(await saved, true);
  assert.equal(posts.length, 1);
  assert.equal(posts[0].url, "/api/reports");
  assert.equal(posts[0].options.method, "POST");
});

test("report history keeps anonymous and failed account saves local", async () => {
  const anonymous = memoryStorage();
  assert.equal(
    await saveReportHistory({ id: "anon", problemId: "two-sum" }, {
      storage: anonymous,
      fetcher: async () => response({ signedIn: false }),
    }),
    false,
  );
  assert.equal(JSON.parse(anonymous.getItem(historyKey))[0].id, "anon");

  const failed = memoryStorage();
  const calls = [];
  assert.equal(
    await saveReportHistory({ id: "fail", problemId: "two-sum" }, {
      storage: failed,
      fetcher: async (url) => {
        calls.push(url);
        return url === "/api/session" ? response({ signedIn: true }) : response({}, false);
      },
    }),
    false,
  );
  assert.deepEqual(calls, ["/api/session", "/api/reports"]);
  assert.equal(JSON.parse(failed.getItem(historyKey))[0].id, "fail");
});

function memoryStorage() {
  const data = new Map();
  return {
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => data.set(key, String(value)),
  };
}

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
test("a stored history that is not a list reads as no history", () => {
  const stored = (value) => {
    const storage = memoryStorage();
    storage.setItem(historyKey, value);
    return readLocalHistory(storage);
  };

  assert.deepEqual(stored("[]"), []);
  assert.deepEqual(stored('[{"problemId":"two-sum"}]'), [{ problemId: "two-sum" }]);
  // Each of these parses, so only the shape check turns them away, and each
  // failed differently before it: an object was counted as "undefined past
  // reports", "five" as four of them, and a missing key parses to null.
  for (const value of ['{"a":1}', '"five"', "null"]) {
    assert.deepEqual(stored(value), [], `${value} was not read as no history`);
  }
  // And unparseable JSON keeps the answer it already had.
  assert.deepEqual(stored("{not json"), []);
});
