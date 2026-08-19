// Run with: node --test tests/browser/*.test.js
//
// The scripts reach into the page by id and by data attribute. If markup and
// script drift apart the page dies at load with a null dereference, and nothing
// short of opening a browser noticed. These checks read both sides as text, so
// they cost nothing and run in the default gate.

import { test } from "node:test";
import assert from "node:assert/strict";
import { captures as matchAll, functionBody, root } from "./source.js";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const web = join(root, "web");
const read = (name) => readFileSync(join(web, name), "utf8");
const scripts = readdirSync(web).filter((name) => name.endsWith(".js"));
const pages = readdirSync(web).filter((name) => name.endsWith(".html"));

/// Ids the browser can find: written in markup, or produced by a script's own
/// template literals (the report card builds its buttons that way).
function availableIds() {
  const ids = new Set();
  for (const name of [...pages, ...scripts]) {
    for (const id of matchAll(read(name), /id="([^"]+)"/g)) {
      ids.add(id);
    }
  }
  return ids;
}

function availableAttributes() {
  const attributes = new Set();
  for (const name of [...pages, ...scripts]) {
    for (const attribute of matchAll(read(name), /(data-[a-z-]+)=/g)) {
      attributes.add(attribute);
    }
    // app.js sets card.dataset.problem, which renders as data-problem.
    for (const property of matchAll(read(name), /dataset\.([A-Za-z]+)\s*=/g)) {
      attributes.add(`data-${property.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}`);
    }
  }
  return attributes;
}

test("every id a script queries exists in the markup", () => {
  const ids = availableIds();
  const missing = [];

  for (const name of scripts) {
    for (const selector of matchAll(read(name), /querySelector\("#([^"]+)"\)/g)) {
      if (!ids.has(selector)) missing.push(`${name} -> #${selector}`);
    }
  }

  assert.deepEqual(missing, [], "these selectors resolve to nothing at runtime");
});

test("every data attribute a script queries is produced somewhere", () => {
  const attributes = availableAttributes();
  const missing = [];

  for (const name of scripts) {
    for (const selector of matchAll(read(name), /querySelectorAll\("\[([^\]"]+)\]"\)/g)) {
      if (!attributes.has(selector)) missing.push(`${name} -> [${selector}]`);
    }
  }

  assert.deepEqual(missing, [], "these attribute selectors match nothing");
});

test("ids are unique within each page", () => {
  for (const name of pages) {
    const ids = matchAll(read(name), /id="([^"]+)"/g);
    const duplicates = ids.filter((id, index) => ids.indexOf(id) !== index);
    assert.deepEqual(duplicates, [], `${name} repeats an id, so querySelector picks one at random`);
  }
});

test("every script and stylesheet a page loads exists on disk", () => {
  const missing = [];
  for (const name of pages) {
    const page = read(name);
    const refs = [
      ...matchAll(page, /<script[^>]+src="\/([^"]+)"/g),
      ...matchAll(page, /<link[^>]+href="\/([^"]+)"/g),
    ];
    for (const ref of refs) {
      if (ref === "runtime-config.js") continue;
      try {
        readFileSync(join(web, ref));
      } catch {
        missing.push(`${name} -> /${ref}`);
      }
    }
  }
  assert.deepEqual(missing, [], "the page requests assets that are not served");
});

test("every module import resolves to a file that exists", () => {
  const missing = [];
  for (const name of scripts) {
    for (const specifier of matchAll(read(name), /from\s+"\.\/([^"]+)"/g)) {
      try {
        readFileSync(join(web, specifier));
      } catch {
        missing.push(`${name} -> ./${specifier}`);
      }
    }
  }
  assert.deepEqual(missing, [], "a module import points at a missing file");
});

test("problem module exports the lookup used by the interview page", async () => {
  const { getProblem } = await import(join(web, "problems.js"));

  assert.equal(getProblem("two-sum").id, "two-sum");
  assert.equal(getProblem("missing").id, "two-sum");
});

test("the interview page keeps the structure the script drives", () => {
  const page = read("interview.html");
  // These are the hooks the interview flow needs; losing one breaks the
  // session in a way unit tests on pure functions cannot see.
  for (const id of ["editor", "run-tests", "timer", "transcript-panel", "problem-panel", "camera-integrity-video"]) {
    assert.match(page, new RegExp(`id="${id}"`), `interview.html must keep #${id}`);
  }
  assert.match(page, /<script[^>]+type="module"[^>]+src="\/interview\.js"/, "interview.js must load as a module");
  assert.match(page, /<script[^>]+src="\/runtime-config\.js"/, "runtime config must load");
  assert.ok(
    page.indexOf('src="/runtime-config.js"') >= 0
      && page.indexOf('src="/interview.js"') >= 0
      && page.indexOf('src="/runtime-config.js"') < page.indexOf('src="/interview.js"'),
    "runtime config must load before interview.js imports runners",
  );
  assert.match(page, /data-language="python"/, "the language tabs drive setLanguage");
  assert.match(page, /C, C\+\+ and Java runs are sent to Compiler Explorer/, "compiled language runs need third-party disclosure");
  const script = read("interview.js");
  assert.match(script, /CODETRIAL_COMPILER_EXPLORER_ENABLED !== false/);
  assert.match(script, /C, C\+\+ and Java test runs are disabled by this server/);
  assert.match(page, /id="audio-step-camera"/, "preflight must expose camera readiness");
  // Screen sharing was removed rather than left behind a disabled flag, so no
  // part of it may creep back as unreachable code.
  assert.doesNotMatch(script, /[Ss]creen[SH]|SCREEN_/, "the screen-share path is deleted, not paused");
  assert.doesNotMatch(page, /screen-share-request|audio-step-screen|screen-health-video/, "no orphaned screen markup");
  assert.match(script, /new Worker\("\/integrity-worker\.js", \{ type: "module" \}\)/, "integrity analysis must stay off the UI thread");
  assert.match(read("integrity-worker.js"), /new Worker\("\/face-worker\.js"\)/, "MediaPipe face detection needs a classic worker");
  assert.doesNotMatch(script, /publishIntegrityEvent\(\{[^}]*url/s, "Blob URLs must not become integrity payloads");
  // A camera another application has taken stays `live` and delivers stale
  // frames. Sampling those charges them as an absent candidate and ends the
  // interview 15s later, which is what opening the camera in Google Meet did.
  assert.match(
    script,
    /track\.readyState !== "live" \|\| track\.muted/,
    "a muted camera is a gap in sampling, not evidence the candidate left",
  );
});

test("output confirmation is required but not blocked by tone timing", () => {
  const script = read("interview.js");
  const heardHandler = script.slice(
    script.indexOf("const confirmOutput ="),
    script.indexOf("nodes.audioJoin.addEventListener"),
  );

  assert.doesNotMatch(script, /outputRequired: false/);
  assert.match(heardHandler, /outputConfirmed = true/);
  assert.match(heardHandler, /addEventListener\("pointerdown", confirmOutput\)/);
  assert.match(heardHandler, /addEventListener\("click", confirmOutput\)/);
  // The tone used to be the only user gesture, and it was what let the browser
  // play the interviewer at all. Confirming without it must not skip that.
  assert.match(heardHandler, /unblockOutput\(\)/);
  assert.doesNotMatch(heardHandler, /if \(!tonePlayed\)/);
  assert.doesNotMatch(script, /nodes\.audioJoin\.focus\(\);\s*finish\(\)/);
});

test("runtime config can withdraw compiled language test runs", async () => {
  const previous = globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL;
  globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = "";
  try {
    const { runBrowserTests } = await import(`../../web/runners.js?compiled-runs-disabled=${Date.now()}`);
    for (const language of ["c", "cpp", "java"]) {
      const summary = await runBrowserTests("two-sum", "", language);
      assert.equal(summary.language, language);
      assert.equal(summary.passed, 0);
      assert.match(summary.setupError, /tests are not wired up yet/);
    }
  } finally {
    if (previous === undefined) {
      delete globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL;
    } else {
      globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = previous;
    }
  }
});

// Every runner has to be interruptible. Candidate code that never returns must
// cost one test run, not the interview: on the main thread it would freeze the
// timer, the transcript and the interviewer's audio until the tab is reloaded.
// Showing the report tells the candidate the interview is over. The devices
// have to actually be released at that point, on every path. Only the
// agent-sent path used to do it, so an offline or forced report left the
// microphone and camera live and the integrity heartbeat still sampling while
// the candidate read their result.
test("every report path releases the camera and microphone", () => {
  const script = read("interview.js");
  const body = functionBody(script, "renderReport");

  assert.match(body, /stopLocalMedia\(\);/, "renderReport must release the devices");
  assert.match(body, /stopAvatar\(\);/, "and stop the render loop");

  // Both report paths must funnel through it rather than each remembering to
  // clean up for themselves, which is how one of them forgot.
  for (const caller of ["receiveReport", "showReport"]) {
    assert.match(functionBody(script, caller), /renderReport\(\)/, `${caller} must render through renderReport`);
  }
});

// A mid-interview network drop used to be entirely invisible: `state.connected`
// stayed true, the UI said nothing, and `publish` silently discarded every code
// update, test result and integrity event for the rest of the session.
test("a dropped connection is visible and recovers its state", () => {
  const script = read("interview.js");
  const connect = functionBody(script, "connectLiveKit");

  for (const event of ["Reconnecting", "Reconnected", "Disconnected"]) {
    assert.match(connect, new RegExp(`RoomEvent\\.${event}`), `RoomEvent.${event} must be handled`);
  }
  // Reconnecting is not a dead session, so the candidate is told to keep going.
  assert.match(connect, /Keep working; your code is safe/);
  // Nothing published during the gap arrived, so the buffer is resent rather
  // than left to drift until the next keystroke.
  assert.match(connect, /Reconnected[\s\S]*?publish\(topics\.code/);
});

// Offline practice mode looks identical whether the server has no LiveKit
// credentials, is at capacity, or the microphone failed to publish. The server
// writes candidate-facing text for exactly this; dropping it on the floor is
// what makes a refusal indistinguishable from the product working.
test("a refused interview says why instead of silently practising", () => {
  const script = read("interview.js");
  const connect = functionBody(script, "connect");

  assert.match(connect, /setBanner\("connection", `\$\{error\?\.message/);
  assert.match(connect, /keep practising here in the meantime/);
  // The reason has to reach the candidate, not only the console.
  const bannerAt = connect.indexOf('setBanner("connection"');
  const practiceAt = connect.indexOf("Offline practice mode is ready");
  assert.ok(bannerAt !== -1 && practiceAt !== -1 && bannerAt < practiceAt);
});

// One banner element with several owners, ranked, so a new owner is a row here
// rather than another boolean lock and another branch.
test("the presence banner ranks its owners instead of racing them", () => {
  const script = read("interview.js");
  assert.match(script, /const BANNER_RANK = \{[^}]*session[^}]*connection[^}]*interviewer[^}]*face[^}]*\}/);
  // Every owner keeps a slot, so a warning raised behind a higher-ranked one is
  // deferred rather than discarded.
  assert.match(functionBody(script, "setBanner"), /banners\[source\] = text \|\| "";/);
  // The flag-and-deferred-field mechanism this replaced is gone entirely.
  assert.doesNotMatch(script, /interviewerBanner|faceBanner|paintPresenceBanner/);
});

test("no test runner executes candidate code on the main thread", () => {
  const script = read("runners.js");

  assert.match(script, /function runPython/);
  assert.doesNotMatch(script, /pyodide\.runPython\(pythonDriver\)/, "Python must not run inline");
  for (const runner of ["runPython", "runWorker"]) {
    const start = script.indexOf(`function ${runner}(`);
    // Up to the next top-level declaration, so the assertions cannot drift into
    // a neighbouring function and pass for the wrong reason.
    const end = script.indexOf("\nfunction ", start + 1);
    const body = script.slice(start, end);

    assert.match(body, /setTimeout\(/, `${runner} must arm a timeout`);
    assert.match(body, /terminate\(\)/, `${runner} must be able to stop a runaway loop`);
  }
  // Measured: a cold Compiler Explorer compile takes about six seconds, so a
  // remote run sharing the local execution budget fails every first attempt.
  const budget = (name) => Number(script.match(new RegExp(`const ${name} = ([0-9_]+)`))[1].replace(/_/g, ""));
  assert.ok(
    budget("compilerExplorerTimeoutMs") > budget("testTimeoutMs"),
    "a remote compile needs a larger budget than local execution",
  );
  // The interpreter has to come from this origin. A CDN URL here would put the
  // bytes that execute candidate code back outside the SHA256SUMS pin.
  assert.match(script, /vendor\/pyodide\//);
  assert.doesNotMatch(script, /cdn\.jsdelivr\.net/);
});

test("runner progress statuses stay wired to each execution path", () => {
  const script = read("runners.js");

  assert.match(script, /onStatus = null/);
  assert.match(script, /reportStatus\??\.\("booting"\)|reportStatus\("booting"\)/);
  assert.match(script, /reportStatus\??\.\("compiling"\)|reportStatus\("compiling"\)/);
  assert.match(script, /reportStatus\??\.\("running"\)|reportStatus\("running"\)/);
});
