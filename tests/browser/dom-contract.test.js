// Run with: node --test tests/browser/*.test.js
//
// The scripts reach into the page by id and by data attribute. If markup and
// script drift apart the page dies at load with a null dereference, and nothing
// short of opening a browser noticed. These checks read both sides as text, so
// they cost nothing and run in the default gate.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  INTERVIEW_SOURCES,
  captures as matchAll,
  failFetchWith,
  functionBody,
  initialisedModules,
  interviewSource,
  root,
  firstPartyScripts,
} from "./source.js";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

const web = join(root, "web");
const read = (name) => readFileSync(join(web, name), "utf8");

/// Stands in for the server: serves web/ off disk and 404s what is not there.
/// Returns the undo, because a `globalThis.fetch` left installed makes the next
/// test in the file depend on this one having run.
function serveWebFromDisk(served = []) {
  const previous = globalThis.fetch;
  globalThis.fetch = async (url) => {
    served.push(url);
    const path = join(web, new URL(url, "http://localhost/").pathname);
    try {
      const body = readFileSync(path, "utf8");
      return { ok: true, status: 200, json: async () => JSON.parse(body) };
    } catch {
      return { ok: false, status: 404, json: async () => null };
    }
  };
  return () => {
    globalThis.fetch = previous;
  };
}

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

test("problem loader fetches one problem and falls back to the default", async () => {
  // Stands in for the server: serves web/ and 404s anything not on disk, which
  // is what a request for a problem outside the bank gets.
  const served = [];
  const restore = serveWebFromDisk(served);
  try {
    const { loadProblem, loadJudge } = await import(join(web, "problem-data.js"));

    assert.equal((await loadProblem("two-sum")).id, "two-sum");
    assert.equal((await loadProblem("missing")).id, "two-sum");
    assert.ok((await loadJudge("two-sum")).cases.length > 0);
    assert.equal(await loadJudge("missing"), null);
  } finally {
    restore();
  }

  // The point of the split: one problem asked for is one problem fetched, not
  // a module carrying the answers to the other 149.
  assert.deepEqual(served, [
    "/problems/two-sum.json",
    "/problems/missing.json",
    "/problems/two-sum.json",
    "/judges/two-sum.json",
    "/judges/missing.json",
  ]);
});

test("an unreachable bank is reported as unreachable, not as an empty one", async () => {
  // A 404 says the bank does not have it. Everything else says we could not
  // find out, and the two must not arrive at the same answer: reporting a
  // dropped connection as "no test cases are defined" tells a candidate to stop
  // trying.
  let attempts = 0;
  let restore = failFetchWith(async () => {
    attempts += 1;
    throw new TypeError("Failed to fetch");
  });
  try {
    const { loadProblem, loadJudge } = await import(`${join(web, "problem-data.js")}?unreachable`);

    await assert.rejects(() => loadProblem("two-sum"), /could not be reached/);
    await assert.rejects(() => loadJudge("two-sum"), /could not be reached/);
    // One retry each, not none and not a storm.
    assert.equal(attempts, 4);
  } finally {
    restore();
  }

  restore = failFetchWith(async () => ({ ok: false, status: 503, json: async () => null }));
  try {
    const { loadJudge } = await import(`${join(web, "problem-data.js")}?unavailable`);
    await assert.rejects(() => loadJudge("two-sum"), /returned 503/);
  } finally {
    restore();
  }
});

test("a judge that cannot be fetched is not a problem without tests", async () => {
  const restore = failFetchWith(async () => {
    throw new TypeError("Failed to fetch");
  });
  try {
    const { runBrowserTests } = await import(`${join(web, "runners.js")}?judge-unreachable`);
    const summary = await runBrowserTests("two-sum", "", "python");

    assert.match(summary.setupError, /could not be loaded/);
    assert.doesNotMatch(summary.setupError, /No test cases are defined/);
    assert.equal(summary.total, 0);
  } finally {
    restore();
  }
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
  const script = interviewSource();
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
  const script = interviewSource();
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

test("a replacement preflight camera gets a fresh face check", () => {
  // Dropping the dead track, forgetting its face check, and asking for a
  // replacement are one step: any two of the three leave the panel judging a
  // camera that is gone. Both routes to that step now end in `onLost`, so this
  // asserts the hook rather than a sequence written out at the call site. This
  // is the wiring `face-check.test.js` cannot see, because it is about who
  // calls the reset rather than what the reset does.
  assert.match(read("interview.js"), /onLost: \(\) => faceCheck\.reset\(\)/,
    "losing a camera must reset the face check");

  // The preflight notices an ended camera per frame and the pool notices one on
  // a retry pass. Neither may reset the check by hand: `dropTrack` fires
  // `onLost` for the first and `requestDevice` for the second, and a caller
  // pairing the two by hand is what left the reset written at two sites.
  assert.doesNotMatch(read("interview.js"), /faceCheck\.reset\(\);/,
    "the face check reset belongs to onLost, not to a call site");

  // start() awaits three times, and the camera can be replaced across any of
  // them, so every resumption has to re-check that this run still owns the
  // check rather than testing it once at the top. Counted rather than asserted
  // behaviorally: a test can prove one resumption bails, not that a fourth
  // await added later remembered to.
  const stale = [...read("face-check.js").matchAll(/\bstale\(\)/g)];
  assert.equal(stale.length, 3,
    "a stale detector must not publish a verdict for the replacement camera");
});

// The camera's liveness is judged on every preflight frame. The microphone's
// is not: a level meter over a device that went away reports silence rather
// than an error, so nothing asked the pool to look again and the media gate,
// which has no bypass, stayed shut for the rest of the preflight.
test("a preflight microphone that goes away is asked for again", () => {
  const script = interviewSource();
  // One loop covers both kinds, so the microphone is no longer the case that
  // can be forgotten: naming a kind here is what asks the pool to look again.
  assert.match(script,
    /for \(const kind of \["audio", "video"\]\)[\s\S]*?pool\.dropTrack\(track\);\s*pool\.retry\(\);/,
    "a dead device of either kind must reopen the request the pool makes");
  // Anchored to the audio hook and lazily matched, so a rename of the video
  // hook cannot silently widen the slice this is read out of.
  assert.match(script,
    /pool\.configure\("audio",[\s\S]*?onLost: \(\) => \{\s*meterGeneration \+= 1;\s*micPeak = 0;\s*recentPeaks\.length = 0;/,
    "dropping a microphone must invalidate its meter and readiness state");
});

test("runtime config can withdraw compiled language test runs", async () => {
  const previous = globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL;
  globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = "";
  const restore = serveWebFromDisk();
  try {
    const { runBrowserTests } = await import(`../../web/runners.js?compiled-runs-disabled=${Date.now()}`);
    for (const language of ["c", "cpp", "java"]) {
      const summary = await runBrowserTests("two-sum", "", language);
      assert.equal(summary.language, language);
      assert.equal(summary.passed, 0);
      assert.match(summary.setupError, /tests are not wired up yet/);
    }
  } finally {
    restore();
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
  const script = interviewSource();
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
  const script = interviewSource();
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
  const script = interviewSource();
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
  const script = interviewSource();
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


/// The whole-path assertions in this file and four others read
/// `INTERVIEW_SOURCES`. A cluster split out of `interview.js` and not added to
/// it is not a failing test: it is five tests that quietly stop covering the
/// code that moved, which is the fail-open `source.js` was written to warn
/// about. So the list is checked against the source rather than trusted.
test("the interview path is read whole", () => {
  const derived = initialisedModules().sort();
  const listed = INTERVIEW_SOURCES.filter((name) => name !== "web/interview.js").sort();
  assert.deepEqual(
    derived,
    listed,
    "every module interview.js hands its bindings to must be in INTERVIEW_SOURCES, and nothing else",
  );
  assert.ok(
    INTERVIEW_SOURCES.includes("web/interview.js"),
    "the page script itself is part of the path",
  );
});


/// Every relative import in every first-party script resolves to a file.
///
/// A specifier is relative to the file holding it, so moving code between
/// directories silently breaks one: the avatar stage carried
/// `import("./avatar/vrm.js")` out of `web/interview.js` into `web/avatar/`,
/// where it named `web/avatar/avatar/vrm.js`. Nothing caught it, because a
/// source-text assertion matches the string and the dynamic import only runs
/// after the model HEAD succeeds, which needs a published model.
test("relative imports resolve to files that exist", () => {
  const missing = [];
  for (const name of firstPartyScripts()) {
    const source = read(name);
    const dir = dirname(join(root, "web", name));
    const specifiers = [
      ...matchAll(source, /(?:^|[^\w])import\s+[^;]*?from\s+"(\.[^"]+)"/g),
      ...matchAll(source, /import\("(\.[^"]+)"\)/g),
    ];
    for (const specifier of specifiers) {
      if (!existsSync(resolve(dir, specifier))) missing.push(`web/${name} -> ${specifier}`);
    }
  }
  assert.deepEqual(missing, [], "these specifiers name nothing on disk");
});

test("runner progress statuses stay wired to each execution path", () => {
  const script = read("runners.js");

  assert.match(script, /onStatus = null/);
  assert.match(script, /reportStatus\??\.\("booting"\)|reportStatus\("booting"\)/);
  assert.match(script, /reportStatus\??\.\("compiling"\)|reportStatus\("compiling"\)/);
  assert.match(script, /reportStatus\??\.\("running"\)|reportStatus\("running"\)/);
});
