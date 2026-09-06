// Run with: node --test tests/browser/*.test.js

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { historyKey, readLocalHistory, saveReportHistory } from "../../web/history.js";
import { functionBody, memoryStorage, root } from "./source.js";

const web = join(root, "web");
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

test("a completed test run keeps pause and round locks", () => {
  const interview = read("interview.js");

  // Both places that hand the runner back use the same guard. A finished test
  // run was the one that got it right; resuming from a pause was the one that
  // did not, and gave the editor and the runner back during the behavioral
  // round that had just retired them.
  assert.match(functionBody(interview, "runTests"), /updateRunAvailability\(\)/);
  const applyPause = functionBody(interview, "applyPause");
  assert.match(applyPause, /nodes\.editor\.disabled = paused \|\| codingClosed\(\)/);
  assert.match(applyPause, /updateRunAvailability\(\)/);
  // Asserted as two facts rather than one spelling: the guard is `codingClosed`
  // itself now, not a DOM attribute that happens to track it.
  const updateRunAvailability = functionBody(interview, "updateRunAvailability");
  assert.match(updateRunAvailability, /codingClosed\(\)/);
  assert.match(updateRunAvailability, /!languages\.includes\(state\.language\)/);
  assert.match(
    functionBody(interview, "codingClosed"),
    /frameworkRound === "behavioral" \|\| state\.phase !== "live"/,
  );
});

test("the lobby offers one interview and carries no mode to the room", () => {
  const lobby = read("index.html");
  const app = read("app.js");
  const page = read("interview.html");
  const interview = read("interview.js");

  // One interview, so the lobby offers no mode to pick and nothing carries one
  // to the room. Asserted as absence because the confusion this removed was a
  // choice on screen, and a stray button is exactly how it would come back.
  assert.doesNotMatch(lobby, /data-mode=/);
  assert.doesNotMatch(app, /searchParams\.set\("mode"/);
  assert.doesNotMatch(interview, /params\.get\("mode"\)/);
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
  assert.equal(css.match(/^\.framework-hint \{/gm).length, 1, "a second rule block overrides only part of the first");
  // It must paint nothing: the page behind it is what the candidate is being
  // asked about, and the card is bounded so it cannot fill the screen.
  assert.match(css, /\.framework-hint \{[^}]*background: none;[^}]*pointer-events: none;/s);
  assert.match(css, /\.framework-hint-card \{[^}]*max-height: 70vh;/s);
  assert.match(css, /\.framework-hint-card \{[^}]*max-width: min\(/s);
  assert.doesNotMatch(interview, /showModal\(\)/);
  assert.match(interview, /frameworkHintBody\.innerHTML/);
  assert.match(interview, /<caption>\$\{escapeHtml\(framework\.name\)\}/);
  assert.doesNotMatch(interview, /frameworkBriefing/);
  assert.doesNotMatch(interview, /Two shapes fit this interview/);
  assert.doesNotMatch(page, /REACTO: Repeat/);
  assert.match(interview, /message\.type === "framework_state" && Array\.isArray\(message\.phases\)/);
  assert.match(interview, /frameworkRound = "behavioral"/);
  assert.match(interview, /globalThis\.setTimeout\(\(\) => \{\s*nodes\.frameworkHint\.hidden = true;/);
  assert.match(interview, /JSON\.stringify\(\{ problemId: problem\.id, durationMin, interviewId, interviewLoop, interviewProfile, \.\.\.\(interviewGrounding/);
  assert.match(interview, /interviewLoop, report: state\.report/);
});

test("interview loop is explicit, budgeted, gated, and carried into artifacts", () => {
  const lobby = read("index.html");
  const app = read("app.js");
  const page = read("interview.html");
  const interview = read("interview.js");
  assert.match(lobby, /data-loop="coding_only"/);
  assert.match(lobby, /data-loop="coding_behavioral"[^>]*class="[^"]*selected|class="[^"]*selected"[^>]*data-loop="coding_behavioral"/);
  assert.match(app, /let interviewLoop = "coding_behavioral"/);
  assert.match(app, /destination\.searchParams\.set\("loop", interviewLoop\)/);
  assert.match(page, /id="round-plan-summary"/);
  assert.match(interview, /behavioralMinutes = interviewLoop === "coding_behavioral" \? Math\.min\(8, durationMin\) : 0/);
  assert.match(interview, /type: "round_transition", round: "behavioral"/);
  assert.match(interview, /recordReplay\("lifecycle", \{ state: "round_reserve_started"/);
  assert.match(interview, /message\.type === "round_state"/);
  assert.match(interview, /nodes\.editor\.disabled = true/);
  assert.match(interview, /state: "rounds_final"/);
  assert.match(interview, /interviewLoop, report: state\.report/);
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

test("document grounding is explicit, clearable, ephemeral, and absent from saved artifacts", () => {
  const lobby = read("index.html");
  const app = read("app.js");
  const interview = read("interview.js");
  for (const id of ["grounding-jd", "grounding-resume", "grounding-choices", "grounding-consent", "grounding-clear", "grounding-error"]) {
    assert.match(lobby, new RegExp(`id="${id}"`));
  }
  assert.match(lobby, /Send only my selected snippets to the AI interviewer\./);
  assert.match(app, /nodes\.groundingClear\.addEventListener\("click", clearGrounding\)/);
  assert.match(app, /storeGroundingPacket\(sessionStorage, packet\)/);
  assert.match(interview, /consumeGroundingPacket\(sessionStorage\)/);
  const savedArtifacts = [read("history.js"), read("replay-feed.js"), functionBody(interview, "saveHistory")];
  for (const source of savedArtifacts) assert.doesNotMatch(source, /interviewGrounding/);
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
