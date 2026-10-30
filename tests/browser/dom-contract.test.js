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
  assertIncludesCompact,
  captures as matchAll,
  failFetchWith,
  firstPartyScripts,
  functionBody,
  initialisedModules,
  interviewSource,
  root,
} from "./source.js";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const web = join(root, "web");
const read = (name) => readFileSync(join(web, name), "utf8");
// What import() takes. A bare absolute path only works where it starts with a
// slash: on Windows the loader reads the drive letter as a URL scheme.
const webUrl = (name) => pathToFileURL(join(web, name)).href;
// The page names these tests use, read from the generated map rather than
// written here, so renaming a scenario does not break a test about loading.
const pageMap = JSON.parse(read("problem-pages.json"));
const twoSum = pageMap["two-sum"];
const fallback = Object.values(pageMap).find((entry) => entry.default).page;

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
      attributes.add(
        `data-${property.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}`,
      );
    }
  }
  return attributes;
}

test("every id a script queries exists in the markup", () => {
  const ids = availableIds();
  const missing = [];

  for (const name of scripts) {
    for (const selector of matchAll(
      read(name),
      /querySelector\("#([^"]+)"\)/g,
    )) {
      if (!ids.has(selector)) missing.push(`${name} -> #${selector}`);
    }
  }

  assert.deepEqual(
    missing,
    [],
    "these selectors resolve to nothing at runtime",
  );
});

test("every data attribute a script queries is produced somewhere", () => {
  const attributes = availableAttributes();
  const missing = [];

  for (const name of scripts) {
    for (const selector of matchAll(
      read(name),
      /querySelectorAll\("\[([^\]"]+)\]"\)/g,
    )) {
      if (!attributes.has(selector)) missing.push(`${name} -> [${selector}]`);
    }
  }

  assert.deepEqual(missing, [], "these attribute selectors match nothing");
});

test("ids are unique within each page", () => {
  for (const name of pages) {
    const ids = matchAll(read(name), /id="([^"]+)"/g);
    const duplicates = ids.filter((id, index) => ids.indexOf(id) !== index);
    assert.deepEqual(
      duplicates,
      [],
      `${name} repeats an id, so querySelector picks one at random`,
    );
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

test("problem loader resolves legacy ids and falls back to the default", async () => {
  // Stands in for the server: serves web/ and 404s anything not on disk, which
  // is what a request for a problem outside the bank gets.
  const served = [];
  const restore = serveWebFromDisk(served);
  try {
    const { loadProblem, loadJudge } = await import(webUrl("problem-data.js"));

    const scenario = await loadProblem(twoSum.page);
    assert.equal(scenario.page, twoSum.page);
    assert.equal(
      "id" in scenario,
      false,
      "a scenario load carries no published id",
    );
    assert.equal(scenario.requestedPage, undefined);
    // An old link carries the published id and still opens its scenario.
    assert.equal((await loadProblem("two-sum")).title, twoSum.title);
    // A name the bank does not have opens the default, and says so.
    const missing = await loadProblem("missing");
    assert.equal(missing.page, fallback);
    assert.equal(missing.requestedPage, "missing");
    assert.equal((await loadProblem("__proto__")).requestedPage, "__proto__");
    assert.ok((await loadJudge(twoSum.page)).cases.length > 0);
    assert.equal(await loadJudge("missing"), null);
  } finally {
    restore();
  }

  // The point of the split: one problem asked for is one problem fetched, not
  // a module carrying the answers to the other 149.
  assert.deepEqual(served, [
    `/problems/${twoSum.page}.json`,
    "/problems/two-sum.json",
    "/problem-pages.json",
    `/problems/${twoSum.page}.json`,
    "/problems/missing.json",
    `/problems/${fallback}.json`,
    "/problems/__proto__.json",
    `/problems/${fallback}.json`,
    `/judges/${twoSum.page}.json`,
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
    const { loadProblem, loadJudge } = await import(
      `${webUrl("problem-data.js")}?unreachable`
    );

    await assert.rejects(() => loadProblem("two-sum"), /could not be reached/);
    await assert.rejects(() => loadJudge("two-sum"), /could not be reached/);
    // One retry each, not none and not a storm.
    assert.equal(attempts, 4);
  } finally {
    restore();
  }

  restore = failFetchWith(async () => ({
    ok: false,
    status: 503,
    json: async () => null,
  }));
  try {
    const { loadJudge } = await import(
      `${webUrl("problem-data.js")}?unavailable`
    );
    await assert.rejects(() => loadJudge("two-sum"), /returned 503/);
  } finally {
    restore();
  }
});

test("a page map that could not be fetched is asked for again", async () => {
  const served = [];
  const fromDisk = serveWebFromDisk(served);
  const fromDiskFetch = globalThis.fetch;
  let down = true;
  globalThis.fetch = async (url) => {
    if (down) throw new TypeError("Failed to fetch");
    return fromDiskFetch(url);
  };
  try {
    const { loadPageMap, loadCategoryMap } = await import(
      `${webUrl("problem-data.js")}?map-retry`
    );
    await assert.rejects(() => loadPageMap(), /could not be reached/);
    await assert.rejects(() => loadCategoryMap(), /could not be reached/);
    down = false;
    assert.equal((await loadPageMap())["two-sum"].page, twoSum.page);
    assert.equal((await loadCategoryMap())[twoSum.page], "Hashmap");
    await loadPageMap();
    await loadCategoryMap();
    assert.deepEqual(
      served,
      ["/problem-pages.json", "/problem-categories.json"],
      "fetched once it arrived, not per call",
    );
  } finally {
    fromDisk();
  }
});

test("a judge that cannot be fetched is not a problem without tests", async () => {
  const restore = failFetchWith(async () => {
    throw new TypeError("Failed to fetch");
  });
  try {
    const { runBrowserTests } = await import(
      `${webUrl("runners.js")}?judge-unreachable`
    );
    const summary = await runBrowserTests("two-sum", "x = 1", "python");

    assert.match(summary.setupError, /could not be loaded/);
    assert.doesNotMatch(summary.setupError, /No test cases are defined/);
    assert.equal(summary.total, 0);
    // Not the candidate's error: the agent may take a hand trace for Test.
    assert.equal(summary.runnerUnavailable, true);
    assert.equal(summary.code, "x = 1");
  } finally {
    restore();
  }
});

test("the interview page keeps the structure the script drives", () => {
  const page = read("interview.html");
  // These are the hooks the interview flow needs; losing one breaks the
  // session in a way unit tests on pure functions cannot see.
  for (const id of [
    "editor",
    "run-tests",
    "timer",
    "transcript-panel",
    "problem-panel",
    "camera-integrity-video",
    "self-view",
    "screen-recording",
  ]) {
    assert.match(
      page,
      new RegExp(`id="${id}"`),
      `interview.html must keep #${id}`,
    );
  }
  assert.match(
    page,
    /<script[^>]+type="module"[^>]+src="\/interview\.js"/,
    "interview.js must load as a module",
  );
  assert.match(
    page,
    /<script[^>]+src="\/runtime-config\.js"/,
    "runtime config must load",
  );
  assert.ok(
    page.indexOf('src="/runtime-config.js"') >= 0 &&
      page.indexOf('src="/interview.js"') >= 0 &&
      page.indexOf('src="/runtime-config.js"') <
        page.indexOf('src="/interview.js"'),
    "runtime config must load before interview.js imports runners",
  );
  assert.match(
    page,
    /data-language="python"/,
    "the language selector drives setLanguage",
  );
  assert.match(
    page,
    /C, C\+\+ and Java runs are sent to Compiler Explorer/,
    "compiled language runs need third-party disclosure",
  );
  const script = interviewSource();
  // The disclosure reads the same "compiled runs are on" the tabs do.
  assert.match(
    script,
    /compileDisclosure\.textContent = !editorOptions\.execution[\s\S]*?: compiledTestsEnabled\(\)/,
  );
  assert.match(
    read("compiler-explorer.js"),
    /CODETRIAL_COMPILER_EXPLORER_ENABLED !== false/,
  );
  assert.match(
    script,
    /C, C\+\+ and Java test runs are disabled by this server/,
  );
  assert.match(
    page,
    /id="audio-step-camera"/,
    "preflight must expose camera readiness",
  );
  // Screen sharing was removed rather than left behind a disabled flag, so no
  // part of it may creep back as unreachable code.
  assert.doesNotMatch(
    script,
    /[Ss]creen[SH]|SCREEN_/,
    "the screen-share path is deleted, not paused",
  );
  assert.doesNotMatch(
    page,
    /screen-share-request|audio-step-screen|screen-health-video/,
    "no orphaned screen markup",
  );
  assert.match(
    script,
    /new Worker\("\/integrity-worker\.js", \{ type: "module" \}\)/,
    "integrity analysis must stay off the UI thread",
  );
  assert.match(
    read("integrity-worker.js"),
    /new Worker\("\/face-worker\.js"\)/,
    "MediaPipe face detection needs a classic worker",
  );
  assert.doesNotMatch(
    script,
    /publishIntegrityEvent\(\{[^}]*url/s,
    "Blob URLs must not become integrity payloads",
  );
  // A camera another application has taken stays `live` and delivers stale
  // frames. Sampling those charges them as an absent candidate and ends the
  // interview 15s later, which is what opening the camera in Google Meet did.
  assert.match(
    script,
    /track\.readyState !== "live" \|\| track\.muted/,
    "a muted camera is a gap in sampling, not evidence the candidate left",
  );
});

test("a late judge response does not replace the active editor buffer", () => {
  const script = read("interview.js");
  // Test is credited from the results message alone, so a send that fails
  // has to reach the candidate rather than an unhandled rejection.
  assert.match(
    functionBody(script, "runTests"),
    /publish\(topics\.tests, testPayload\(summary\)\)\?\.catch\([^]*?not sent to the interviewer/,
  );
  const applyLanguages = functionBody(script, "applyLanguages");
  const setLanguage = functionBody(script, "setLanguage");
  const updateRunAvailability = functionBody(script, "updateRunAvailability");
  assert.doesNotMatch(applyLanguages, /setLanguage\(/);
  assert.match(
    applyLanguages,
    /updateRunAvailability\(\)/,
    "the unsupported active tab cannot run",
  );
  assert.match(
    setLanguage,
    /updateRunAvailability\(\)/,
    "a supported tab restores the runner",
  );
  assert.match(
    updateRunAvailability,
    /!languages\.includes\(state\.language\)/,
  );
});

test("rapid language selections publish only the final tab", () => {
  const script = read("interview.js");
  const setLanguage = functionBody(script, "setLanguage");
  const flushEditor = functionBody(script, "flushPendingEditorPublish");
  assert.match(setLanguage, /flushPendingEditorPublish\(\)/);
  assert.doesNotMatch(setLanguage, /flushPendingCodePublish\(\)/);
  // "An edit is queued" is the absence of a queued switch, which is the whole
  // reason a switch is held as a payload rather than a flag.
  assert.match(
    flushEditor,
    /if \(!codePublishTimer \|\| pendingLanguagePublish\) return/,
  );

  // Where it flushes, not just that it does. publishCode reads the live editor
  // and state.language, so a flush moved below either assignment would publish
  // the tab being switched to and drop the edit nobody has reported yet, which
  // is the regression this test exists for.
  const flushAt = setLanguage.indexOf("flushPendingEditorPublish()");
  const languageAt = setLanguage.indexOf("state.language = language;");
  const bufferAt = setLanguage.indexOf("nodes.editor.value = next.value;");
  assert.ok(flushAt >= 0, "setLanguage flushes a pending editor publish");
  assert.ok(
    languageAt > flushAt,
    "the flush runs before the language is switched",
  );
  assert.ok(
    bufferAt > flushAt,
    "the flush runs before the editor buffer is swapped",
  );
});

test("typing into a freshly picked tab sends the switch before the keystroke", () => {
  const script = read("interview.js");
  const setLanguage = functionBody(script, "setLanguage");
  const flushLanguage = functionBody(script, "flushPendingLanguagePublish");

  // The switch is held as the buffer it was made with, not as a flag. By the
  // time a keystroke can ask for it the editor already holds what was typed, so
  // a flag would have republished the typing under the switch and changed
  // nothing. The publish has to spend that captured copy, not read the editor.
  assert.match(
    setLanguage,
    /pendingLanguagePublish = \{ code: nodes\.editor\.value/,
  );
  assert.match(
    flushLanguage,
    /publishCode\(.*pending\.code, pending\.language\)/,
  );

  // Where it flushes, not just that it does. The handler overwrites
  // codeByLanguage with the live editor on its first line, so a flush below
  // that reads the buffer the keystroke produced rather than the one the tab
  // arrived with.
  const handler = script.slice(
    script.indexOf('nodes.editor.addEventListener("input"'),
  );
  const flushAt = handler.indexOf("flushPendingLanguagePublish()");
  const captureAt = handler.indexOf(
    "state.codeByLanguage[state.language] = nodes.editor.value;",
  );
  assert.ok(
    flushAt >= 0,
    "the input handler flushes a pending language publish",
  );
  assert.ok(
    captureAt > flushAt,
    "the flush runs before the keystroke is captured",
  );

  // Clicking through tabs without typing still coalesces, which is what keeps
  // the interviewer from confirming three languages out loud.
  assert.doesNotMatch(setLanguage, /flushPendingLanguagePublish\(\)/);

  // A cancelled timer is nulled, not just cleared. `flushPendingEditorPublish`
  // reads the handle to mean "an edit is queued", so an id left behind by the
  // end of the interview would publish a buffer after it.
  const endInterview = functionBody(script, "endInterview");
  assert.match(
    endInterview,
    /clearTimeout\(codePublishTimer\);\s*\n\s*codePublishTimer = null;/,
  );
});

test("a queued switch leaves as a switch whoever asks for the flush", () => {
  const script = read("interview.js");
  const flush = functionBody(script, "flushPendingCodePublish");
  const edit = functionBody(script, "flushPendingEditorPublish");

  // Running the tests flushes whatever is queued. The switch goes first, down
  // its own path, which sends no `at` and writes nothing to the editor replay;
  // the edit path runs after it and refuses anything with a switch behind it.
  const switchFirst = flush.indexOf("flushPendingLanguagePublish()");
  const editAfter = flush.indexOf("flushPendingEditorPublish()");
  assert.ok(
    switchFirst >= 0 && editAfter > switchFirst,
    "the switch is flushed before the edit",
  );
  assert.match(
    edit,
    /if \(!codePublishTimer \|\| pendingLanguagePublish\) return;/,
  );
  assert.ok(
    edit.indexOf("pendingLanguagePublish") < edit.indexOf("recordReplay("),
    "the edit path refuses a switch before it records anything",
  );
  assert.match(functionBody(script, "runTests"), /flushPendingCodePublish\(\)/);
});

test("the gutter is rebuilt only when the line count moves", () => {
  const paint = functionBody(read("interview.js"), "paintEditor");

  // One read of the buffer, not two: the highlight pass and the line count both
  // work from it.
  assert.equal((paint.match(/currentCode\(\)/g) || []).length, 1);

  // And the numbers down the side are only rewritten when they would differ.
  // Typing inside a line is most of what a candidate does and none of it moves
  // them, so the common keystroke costs one highlight and nothing else.
  assert.match(paint, /if \(lines !== paintedLineCount\)/);
  assert.match(paint, /paintedLineCount = lines/);
});

test("the module state the first paint reads exists before init paints", () => {
  // A module's top-level code runs in order, and `init()` is called near the
  // top of it: everything before its first `await` runs while the rest of the
  // file is still unevaluated. That prefix calls `setLanguage`, which paints
  // the editor, so any `let` or `const` the paint reads has to be declared
  // above the call. One declared beside `paintEditor` instead was still in
  // its temporal dead zone on the first paint, and since `init` is async the
  // throw became a rejected promise nothing reported: `bindEvents` never ran,
  // and the preflight sat on a disabled button until the browser test timed
  // out. No source-text assertion on `paintEditor` itself could see that; its
  // body was correct and only its neighbour's position was wrong.
  const script = read("interview.js");
  const lines = script.split("\n");
  const initCall = lines.findIndex((line) => line === "init();");
  assert.ok(initCall > 0, "init() is called at the top level of the module");

  const init = functionBody(script, "init");
  const prefix = init.slice(0, init.indexOf("await"));
  assert.match(
    prefix,
    /initializeLanguages\(/,
    "init initializes languages before its first await",
  );
  const initialize = functionBody(script, "initializeLanguages");
  assert.match(initialize, /^function initializeLanguages\(/);
  const initialLanguages = initialize.slice(
    0,
    initialize.indexOf("void judgePromise.then"),
  );
  assert.doesNotMatch(initialLanguages, /\bawait\b|\.then\(/);
  assert.match(
    initialLanguages,
    /setLanguage\(/,
    "initialization paints synchronously",
  );

  const topLevel = new Map();
  lines.forEach((line, index) => {
    const match = /^(?:let|const) ([A-Za-z_$][\w$]*)/.exec(line);
    if (match) topLevel.set(match[1], index);
  });
  const reads = [
    ...functionBody(script, "paintEditor").matchAll(/\b[A-Za-z_$][\w$]*\b/g),
  ]
    .map(([name]) => name)
    .filter((name) => topLevel.has(name));
  assert.ok(
    reads.includes("paintedLineCount"),
    "the check sees what paintEditor reads",
  );
  for (const name of new Set(reads)) {
    assert.ok(
      topLevel.get(name) < initCall,
      `${name} is declared on line ${topLevel.get(name) + 1}, after init() on line ${initCall + 1}`,
    );
  }
});

test("output confirmation is required but not blocked by tone timing", () => {
  const script = interviewSource();
  const heardHandler = script.slice(
    script.indexOf("const confirmOutput ="),
    script.indexOf("nodes.audioJoin.addEventListener"),
  );

  assert.doesNotMatch(script, /outputRequired: false/);
  assert.match(heardHandler, /outputConfirmed = true/);
  assert.match(
    heardHandler,
    /addEventListener\("pointerdown", confirmOutput\)/,
  );
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
  assert.match(
    read("interview.js"),
    /onLost: \(\) => \{\s*stopCameraPreview\(\);\s*faceCheck\.reset\(\);?\s*\}/,
    "losing a camera must clear the preview and reset the face check",
  );

  // The preflight notices an ended camera per frame and the pool notices one on
  // a retry pass. Neither may reset the check by hand: `dropTrack` fires
  // `onLost` for the first and `requestDevice` for the second, and a caller
  // pairing the two by hand is what left the reset written at two sites.
  const scriptWithoutOnLost = read("interview.js").replace(
    /onLost: \(\) => \{\s*stopCameraPreview\(\);\s*faceCheck\.reset\(\);?\s*\}/,
    "",
  );

  assert.doesNotMatch(
    scriptWithoutOnLost,
    /faceCheck\.reset\(\);/,
    "the face check reset belongs to onLost, not to a call site",
  );

  // start() awaits three times, and the camera can be replaced across any of
  // them, so every resumption has to re-check that this run still owns the
  // check rather than testing it once at the top. Counted rather than asserted
  // behaviorally: a test can prove one resumption bails, not that a fourth
  // await added later remembered to.
  const stale = [...read("face-check.js").matchAll(/\bstale\(\)/g)];
  assert.equal(
    stale.length,
    3,
    "a stale detector must not publish a verdict for the replacement camera",
  );
});

test("an unrecorded preflight can continue without a camera", () => {
  const page = read("interview.html");
  const script = interviewSource();

  assert.match(
    page,
    /id="camera-skip"/,
    "the optional path needs a reachable control",
  );
  assert.match(
    script,
    /nodes\.cameraSkip\.hidden = recordingEnabled;/,
    "recorded interviews must not offer the bypass a second time",
  );
  assert.match(
    script,
    /pool\.disable\("video"\)/,
    "declining the camera must stop only its retry path",
  );
  assert.match(
    script,
    /pool\.enable\("video"\)/,
    "undoing the skip must ask the pool for the camera again",
  );
  assert.match(
    script,
    /type: "CAMERA_NOT_USED"/,
    "the session must name the condition in its evidence trail",
  );
  assert.match(
    script,
    /CAMERA_SKIP_REASONS\[deviceErrorKind\(error\)\]/,
    "the skip reason must come from the classifier that chose the advice",
  );
  assert.match(
    script,
    /else if \(!preflight\.cameraSkipped\)/,
    "a skipped camera must not start the face-presence worker",
  );
});

// The busy-camera advice tells the candidate to tick a control by its label.
// A renamed label would leave the advice pointing at nothing while every other
// test still passed, so the quoted text is read out of the advice and looked
// up in the page.
test("the busy-camera advice quotes a label the page still has", () => {
  const advice = read("audio-check.js");
  const quoted = advice.match(/then tick "([^"]+)"/);
  assert.ok(quoted, "the busy-camera advice must quote the control it names");
  assertIncludesCompact(
    read("interview.html"),
    `${quoted[1]}</label>`,
    "the advice quotes a label interview.html does not have",
  );
});

// The camera's liveness is judged on every preflight frame. The microphone's
// is not: a level meter over a device that went away reports silence rather
// than an error, so nothing asked the pool to look again and the media gate,
// which has no bypass, stayed shut for the rest of the preflight.
test("a preflight microphone that goes away is asked for again", () => {
  const script = interviewSource();
  // The drop itself moved into `preflightReadiness` in `web/audio-check.js`,
  // where `tests/browser/mic-meter.test.js` drives it against a pool holding a
  // dead track. What is left here is the wiring only this file can show.
  // Anchored to the audio hook and lazily matched, so a rename of the video
  // hook cannot silently widen the slice this is read out of. What `forget`
  // has to do is not asserted here: it lives in `web/mic-meter.js` now, and
  // `tests/browser/mic-meter.test.js` drives it rather than reading it.
  assert.match(
    script,
    /pool\.configure\("audio",[\s\S]*?onLost: \(\) => \{\s*meter\.forget\(\);/,
    "dropping a microphone must invalidate its meter and readiness state",
  );
});

test("runtime config can withdraw compiled language test runs", async () => {
  const previous = globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL;
  globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = "";
  const restore = serveWebFromDisk();
  try {
    const { runBrowserTests } = await import(
      `../../web/runners.js?compiled-runs-disabled=${Date.now()}`
    );
    for (const language of ["c", "cpp", "java"]) {
      const summary = await runBrowserTests(twoSum.page, "", language);
      assert.equal(summary.language, language);
      assert.equal(summary.passed, 0);
      assert.match(summary.setupError, /disabled by this server/);
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

  assert.match(
    body,
    /stopLocalMedia\(\);/,
    "renderReport must release the devices",
  );
  assert.match(body, /stopAvatar\(\);/, "and stop the render loop");

  // Both report paths must funnel through it rather than each remembering to
  // clean up for themselves, which is how one of them forgot.
  for (const caller of ["receiveReport", "showReport"]) {
    assert.match(
      functionBody(script, caller),
      /renderReport\(\)/,
      `${caller} must render through renderReport`,
    );
  }
});

// A mid-interview network drop used to be entirely invisible: `state.connected`
// stayed true, the UI said nothing, and `publish` silently discarded every code
// update, test result and integrity event for the rest of the session.
test("a dropped connection is visible and recovers its state", () => {
  const script = interviewSource();
  const connect = functionBody(script, "connectLiveKit");

  for (const event of ["Reconnecting", "Reconnected", "Disconnected"]) {
    assert.match(
      connect,
      new RegExp(`RoomEvent\\.${event}`),
      `RoomEvent.${event} must be handled`,
    );
  }
  // Reconnecting is not a dead session, so the candidate is told to keep going.
  assert.match(connect, /providerUiState\("reconnecting"\)/);
  // Nothing published during the gap arrived, so the buffer is resent rather
  // than left to drift until the next keystroke.
  assert.match(
    connect,
    /Reconnected[\s\S]*?reconnectCodingInterview\([\s\S]*?publishCode,/,
  );
  // The board too, or one that settled during the gap waits for a stroke a
  // candidate who has stopped drawing never makes. It goes ahead of the queue
  // the gap held, because an `end_interview` in that queue freezes the report.
  assert.match(
    connect,
    /Reconnected[\s\S]*?republishBoard\(\);\s*void boardUploadsSettled\(\)\.then\(flushPendingPublishes\)/,
  );
  assert.match(functionBody(script, "republishBoard"), /queueBoardPublish\(\)/);
  // And the checkpoints a failed upload held go ahead of every later board,
  // whether or not a reconnect ever comes, since nothing drawn since can
  // stand in for them.
  assert.match(
    functionBody(script, "chainBoardPublish"),
    /retryHeldCheckpoints\(\);[\s\S]*?sendBoard\(/,
  );
});

// The board is locked when the editor is: the behavioral round retires the
// coding surface.
test("the board takes no edits once the behavioral round retires it", () => {
  const script = interviewSource();
  const locked = functionBody(script, "boardLocked");
  assert.match(locked, /frameworkRound === "behavioral"/);
  // Nor before the room is joined, where a stroke would miss the replay.
  assert.match(locked, /!board\.ready/);
  assert.match(
    functionBody(script, "bindBoardPointer"),
    /"pointerdown"[\s\S]*?if \(boardLocked\(\)\) return;/,
  );
  const paint = functionBody(script, "paintBoard");
  for (const button of ["boardUndo", "boardRedo", "boardClear"]) {
    assert.match(
      paint,
      new RegExp(`nodes\\.${button}\\.disabled = locked \\|\\|`),
      `${button} stays usable on a locked board`,
    );
  }
  // The round moving repaints the toolbar, after the state moved.
  assert.match(
    script,
    /frameworkRound = "behavioral";\s*if \(whiteboard\) paintBoard\(\);/,
  );
});

// The agent's reason and wait estimate reach the page only if the whole
// control message is handed over; the library tests cover the wording.
test("an interviewer reconnect banner is worded from the agent's notice", () => {
  assert.match(
    functionBody(interviewSource(), "receiveControl"),
    /setBanner\(\s*"reconnect",\s*message\.reconnecting\s*\?\s*interviewerReconnectMessage\(message\)\s*:\s*"",?\s*\)/,
  );
});

// One banner element with several owners, ranked, so a new owner is a row here
// rather than another boolean lock and another branch.
test("the presence banner ranks its owners instead of racing them", () => {
  const script = interviewSource();
  assert.match(
    script,
    /const BANNER_RANK = \{[^}]*session[^}]*connection[^}]*interviewer[^}]*face[^}]*\}/,
  );
  // Every owner keeps a slot, so a warning raised behind a higher-ranked one is
  // deferred rather than discarded.
  assert.match(read("lib.js"), /slots\[source\] = text \|\| "";/);
  assert.match(
    functionBody(script, "setBanner"),
    /bannerBoard\.set\(source, text\);/,
  );
  // The flag-and-deferred-field mechanism this replaced is gone entirely.
  assert.doesNotMatch(
    script,
    /interviewerBanner|faceBanner|paintPresenceBanner/,
  );
});

// The avatar going quiet is news, not a fault: it is the lowest owner, so a
// face or connection warning is never hidden behind it, and it is drawn yellow.
test("the avatar notice is the lowest banner owner and drawn as a notice", () => {
  const script = interviewSource();
  const rank = script.match(/const BANNER_RANK = \{([^}]*)\}/)[1];
  const ranks = Object.fromEntries(
    [...rank.matchAll(/(\w+):\s*(\d+)/g)].map(([, owner, value]) => [
      owner,
      Number(value),
    ]),
  );
  assert.equal(ranks.avatar, Math.min(...Object.values(ranks)));
  assert.equal(
    Object.values(ranks).filter((value) => value === ranks.avatar).length,
    1,
    "no other owner ties with the avatar",
  );
  // The ordering, the one-shot clock and the unchanged-text guard run in
  // lib.test.js; here the page only has to be wired to them.
  assert.match(
    script,
    /createBannerBoard\(\{\s*ranks: BANNER_RANK,\s*notice: "avatar",/,
  );
  assert.match(script, /dataset\.tone = top === "avatar" \? "notice" : ""/);
  assert.match(
    script,
    /if \(nodes\.presenceBanner\.textContent !== text\)\s*nodes\.presenceBanner\.textContent = text;/,
  );
  assert.match(
    script,
    /initAvatarStage\(\{[^}]*onRetired: \(message\) => setBanner\("avatar", message\)/,
  );
  assert.match(
    read("styles.css"),
    /\.presence-banner\[data-tone="notice"\]\s*\{[^}]*--notice-tint/,
  );
});

test("no test runner executes candidate code on the main thread", () => {
  const script = read("runners.js");

  assert.match(script, /function runPython/);
  assert.doesNotMatch(
    script,
    /pyodide\.runPython\(pythonDriver\)/,
    "Python must not run inline",
  );
  for (const runner of ["runPython", "runWorker"]) {
    const start = script.indexOf(`function ${runner}(`);
    // Up to the next top-level declaration, so the assertions cannot drift into
    // a neighbouring function and pass for the wrong reason.
    const end = script.indexOf("\nfunction ", start + 1);
    const body = script.slice(start, end);

    assert.match(body, /setTimeout\(/, `${runner} must arm a timeout`);
    assert.match(
      body,
      /terminate\(\)/,
      `${runner} must be able to stop a runaway loop`,
    );
  }
  // Measured: a cold Compiler Explorer compile takes about six seconds, so a
  // remote run sharing the local execution budget fails every first attempt.
  const budget = (name) =>
    Number(
      script
        .match(new RegExp(`const ${name} = ([0-9_]+)`))[1]
        .replace(/_/g, ""),
    );
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
  const listed = INTERVIEW_SOURCES.filter(
    (name) => name !== "web/interview.js",
  ).sort();
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
      // `export ... from` too. No module re-exports today, which is exactly
      // why it is listed: it was the one specifier shape the narrower check
      // deleted below could see and this one could not, and leaving the hole
      // open would have made that deletion a real loss the day somebody adds
      // a barrel file.
      ...matchAll(source, /(?:^|[^\w])export\s+[^;]*?from\s+"(\.[^"]+)"/g),
    ];
    for (const specifier of specifiers) {
      if (!existsSync(resolve(dir, specifier)))
        missing.push(`web/${name} -> ${specifier}`);
    }
  }
  assert.deepEqual(missing, [], "these specifiers name nothing on disk");
});

test("runner progress statuses stay wired to each execution path", () => {
  const script = read("runners.js");

  assert.match(script, /onStatus = null/);
  assert.match(
    script,
    /reportStatus\??\.\("booting"\)|reportStatus\("booting"\)/,
  );
  assert.match(
    script,
    /reportStatus\??\.\("compiling"\)|reportStatus\("compiling"\)/,
  );
  assert.match(
    script,
    /reportStatus\??\.\("running"\)|reportStatus\("running"\)/,
  );
});

/// A report that never arrives must not leave "leave the room" as the only way
/// out.
///
/// Both buttons exist already; what was wrong is when each is offered.
/// `#force-report` builds the summary from what this page is already holding,
/// and it was hidden for the whole life of a session that had a room --  which
/// is every real session -- so the candidate whose report was lost was shown
/// one option, and it was the one that navigates away and discards the
/// interview. Issue 31 was reported by someone who took it.
test("a report that never lands still offers the offline summary", () => {
  const ending = functionBody(interviewSource(), "endInterview");

  // Inspect the timeout callback rather than a mention in its comment.
  const deadline = ending.search(/\},\s*REPORT_ESCAPE_WAIT_MS\s*\)/);
  assert.ok(deadline !== -1, "the escape timeout left endInterview");
  const callback = ending.lastIndexOf("setTimeout(() => {", deadline);
  assert.ok(callback !== -1 && callback < deadline);
  const escape = ending.slice(callback, deadline);
  assert.ok(escape.includes("nodes.leaveRoom.hidden = false"));
  assert.ok(
    escape.includes("nodes.forceReport.hidden = false"),
    "past the escape deadline the offline summary is offered beside leaving, not instead of it",
  );
});

/// A spinner cannot say whether anything is still happening; a number can.
///
/// The clock is stopped where the interview actually finishes, not at each exit
/// that remembered to: `renderReport` is where every report path lands, agent
/// sent and offline alike, and `leaveRoom` is the one exit that renders none.
/// Hanging it off `showReport` missed `receiveReport`, which is the path every
/// successful interview takes, and left the interval running for the life of
/// the tab.
test("the ending overlay counts the wait it is asking the candidate to sit through", () => {
  const script = interviewSource();
  const ending = functionBody(script, "endInterview");

  assert.match(ending, /startEndingClock\(\)/);
  assert.match(
    functionBody(script, "startEndingClock"),
    /nodes\.endingElapsed\.textContent/,
  );
  for (const exit of ["renderReport", "leaveRoom"]) {
    assert.match(
      functionBody(script, exit),
      /stopEndingClock\(\)/,
      `${exit} leaves the ending clock running`,
    );
  }
});

/// The thresholds `tickTimer` publishes are levels, sent once each.
test("the countdown's thresholds are levels, not crossings", () => {
  const tick = functionBody(interviewSource(), "tickTimer");

  // A crossing latched separately from the send would be one tick wide, so a
  // missed tick loses the event; a level plus the flag cannot be missed.
  for (const spoken of ["round_transition", "timeWarningPayload"]) {
    const at = tick.indexOf(spoken);
    assert.ok(at !== -1, `${spoken} left tickTimer`);
    assert.doesNotMatch(
      tick.slice(0, at),
      /previousRemaining/,
      `${spoken} is edge-triggered again, so a missed tick loses it`,
    );
  }
});

/// The interviewer can end the session itself, and that route does not pass
/// through `endInterview`.
///
/// `replayTimeline` breaks its window scan on the `ended` lifecycle frame, so a
/// replay missing one shows a trailing unanswered question window that is
/// really the goodbye. `endInterview` writes it for the two routes the page
/// drives; `receiveReport` has to write it for the one it does not.
test("an interview the interviewer ended still records that it ended", () => {
  const receive = functionBody(interviewSource(), "receiveReport");

  assertIncludesCompact(receive, 'recordReplay("lifecycle", { state: "ended"');
  assert.match(
    receive,
    /state\.phase === "live"/,
    "a browser-driven end already wrote the frame, and two would be worse than none",
  );
});

/// The page has two ways to reach a report it did not ask for, and they are not
/// the same event.
///
/// The interviewer can close a finished session, and the agent's own deadline
/// publishes a report too when a suspended tab never fired its own `time_up`.
/// Both arrive in phase "live", so the reason has to be derived rather than
/// assumed, or the replay records a decision Jim never made.
test("a report that arrives unasked says which clock ended the interview", () => {
  const receive = functionBody(interviewSource(), "receiveReport");

  assert.match(
    receive,
    /state\.endsAt/,
    "the reason is hardcoded, so the agent's deadline is logged as the interviewer's decision",
  );
  assert.match(receive, /"time_up"/);
  assert.match(receive, /"interviewer_ended"/);
});

/// The offline summary is now offered while a room is still up, which it was
/// not before: `#force-report` used to be hidden for the whole life of one.
///
/// That makes `showReport` the one report path that can leave the agent in an
/// interview nobody is attending. `receiveReport` disconnects because the agent
/// has already published and left; here nothing has, and a candidate reading a
/// local summary would still be paying for a Gemini session behind it.
test("taking the offline summary releases the room", () => {
  const show = functionBody(interviewSource(), "showReport");

  assert.match(
    show,
    /state\.room\?\.disconnect/,
    "the offline report leaves the agent connected",
  );
  assert.match(show, /state\.room = null/);
  assert.match(show, /state\.connected = false/);
});

/// "Skip report and exit": the way out of the report wait that discards the
/// report instead of waiting for it, showing it, or saving it.
///
/// The button lives on the ending overlay, so it is offered for the whole wait
/// rather than after the escape timer, and nothing about leaving may build the
/// offline summary or touch report history.
test("the ending overlay offers Skip report and exit for the whole wait", () => {
  const page = read("interview.html");
  const overlay = page.slice(
    page.indexOf('id="ending-overlay"'),
    page.indexOf('id="jim-stage"'),
  );
  assert.match(
    overlay,
    /<button[^>]*id="skip-report"[\s\S]*?Skip report and exit/,
    "the skip button is missing from the ending overlay",
  );

  const ending = functionBody(interviewSource(), "endInterview");
  assert.match(
    ending,
    /nodes\.skipReport\.hidden = !reportComing/,
    "the skip is offered as soon as an agent is writing a report",
  );
  const escape = ending.slice(ending.search(/REPORT_ESCAPE_WAIT_MS/) - 600);
  assert.ok(
    !escape.includes("nodes.skipReport"),
    "the skip must not wait for the escape timer",
  );
});

test("confirming Skip changes the overlay and keeps focus off the page body", () => {
  const skip = functionBody(interviewSource(), "skipReport");
  // Focus moves before the focused button is disabled, or it drops to <body>.
  assert.ok(
    skip.indexOf("nodes.endingTitle.focus()") <
      skip.indexOf("nodes.skipReport.disabled = true"),
    "focus must leave the Skip button before it is disabled",
  );
  assert.match(
    skip,
    /nodes\.endingTitle\.textContent = "Skipping report\.\.\."/,
  );
  assert.match(skip, /nodes\.endingDetail\.textContent =/);
  assert.match(skip, /querySelector\("\.spinner"\)\.hidden = true/);
});

test("skipping the report tells the agent, ignores any report, and saves nothing", () => {
  const script = interviewSource();
  const skip = functionBody(script, "skipReport");

  assert.match(skip, /window\.confirm\(SKIP_REPORT_CONFIRM\)/);
  assert.match(
    script,
    /End this interview without waiting for a report\? Any report in progress will be discarded and will not be saved\./,
  );
  assert.match(skip, /state\.reportSkipped = true/);
  assert.match(skip, /publish\(topics\.control, skipReportPayload\(\)\)/);
  assert.match(skip, /leaveRoom\(\)/);
  // The end must have gone out first, or there is no report to cancel.
  assert.ok(skip.indexOf("endSent") < skip.indexOf("skipReportPayload"));
  for (const built of ["showReport", "saveHistory", "saveReportHistory"]) {
    assert.ok(!skip.includes(built), `skipping must not call ${built}`);
  }

  // A report already on its way, and the offline summary, are both refused.
  assert.match(
    script,
    /if \(!acceptsReport\(topic, participant\)\) return;\s*(?:\/\/[^\n]*\n\s*)*if \(state\.reportSkipped\) return;/,
  );
  assert.match(
    functionBody(script, "showReport"),
    /if \(state\.reportSkipped\) return;/,
  );
});

test("the skip button is taken away once a report or recovery owns the ways out", () => {
  const script = interviewSource();
  assert.match(
    script,
    /nodes\.leaveRoom\.hidden = true;\s*nodes\.skipReport\.hidden = true;/,
    "a report that lands must hide the skip",
  );
  assert.match(
    functionBody(script, "reportRenderFailed"),
    /nodes\.skipReport\.hidden = true/,
  );
  assert.match(
    script,
    /nodes\.skipReport\.hidden = true;\s*nodes\.leaveRoom\.hidden = false;\s*nodes\.leaveRoom\.textContent = "Save incomplete report and leave"/,
  );
});

test("late report and every history save entry reject a confirmed skip", () => {
  const script = interviewSource();
  assert.match(
    functionBody(script, "receiveReport"),
    /if \(state\.reportSkipped\) return;/,
    "a report delivered after Skip must not be processed",
  );
  assert.match(
    functionBody(script, "saveHistory"),
    /if \(state\.reportSkipped\) return;/,
    "recovery or offline saves must not bypass Skip",
  );
});

test("the skip packet is the control message the agent listens for", async () => {
  const { skipReportPayload } = await import(webUrl("lib.js"));
  assert.deepEqual(skipReportPayload(), { type: "skip_report" });
});

test("skip waits for agent acknowledgement before disconnecting", async () => {
  const script = interviewSource();
  const { skipReportPayload } = await import(webUrl("lib.js"));
  const bodies = ["skipReport", "leaveRoom", "receiveControl"]
    .map((name) => `${functionBody(script, name)}\n}`)
    .join("\n");

  function setup() {
    const events = [];
    const timers = new Map();
    let timerId = 0;
    const room = {
      disconnect: async () => events.push("disconnect"),
    };
    const scope = {
      state: {
        phase: "ending",
        reportReceiving: false,
        reportSkipped: false,
        room,
      },
      nodes: {
        skipReport: {},
        forceReport: {},
        leaveRoom: {},
        endingTitle: { focus() {} },
        endingDetail: {},
        ending: { querySelector: () => ({}) },
      },
      window: {
        confirm: () => true,
        location: { href: "/interview" },
      },
      globalThis: {
        setTimeout(callback, delay) {
          const id = ++timerId;
          timers.set(id, { callback, delay });
          return id;
        },
        clearTimeout(id) {
          timers.delete(id);
        },
      },
      topics: { control: "control" },
      skipReportPayload,
      endSent: Promise.resolve(),
      publish: async () => events.push("publish"),
      stopEndingEscape() {},
      stopEndingClock() {},
      stopAvatar() {},
      stopLocalMedia() {},
      beforeUnloadGuard: { dispose() {} },
      reportRecovery: {
        stop: () => events.push("stop-recovery"),
        finish: () => events.push("finish-recovery"),
      },
    };

    const page = new Function(
      "scope",
      `with (scope) {
        let resolveSkipReportAck = null;
        const SKIP_REPORT_SEND_WAIT_MS = 3000;
        const SKIP_REPORT_CONFIRM = "Skip report?";
        ${bodies}
        return { skipReport, receiveControl };
      }`,
    )(new Proxy(scope, { has: (target, key) => key in target }));

    const settle = () => new Promise((resolve) => setImmediate(resolve));

    return {
      ...page,
      scope,
      events,
      timers,
      settle,
      acknowledge() {
        page.receiveControl(
          new TextEncoder().encode(JSON.stringify({ type: "skip_report_ack" })),
        );
      },
      expire() {
        for (const [id, timer] of [...timers]) {
          if (timer.delay === 3000) {
            timers.delete(id);
            timer.callback();
          }
        }
      },
    };
  }

  // A resolved publish promise does not prove the agent received the packet.
  {
    const page = setup();
    const leaving = page.skipReport();
    await page.settle();
    assert.ok(page.events.includes("publish"));
    assert.ok(!page.events.includes("disconnect"));
    assert.equal(page.scope.window.location.href, "/interview");

    page.acknowledge();
    await leaving;
    assert.equal(page.events.filter((e) => e === "disconnect").length, 1);
    assert.equal(page.scope.window.location.href, "/");
  }

  // Missing acknowledgements must not trap the candidate indefinitely.
  {
    const page = setup();
    const leaving = page.skipReport();
    await page.settle();
    assert.ok(!page.events.includes("disconnect"));
    page.expire();
    await leaving;
    assert.ok(page.events.includes("disconnect"));
    assert.equal(page.scope.window.location.href, "/");
  }

  // Recovery arriving during the ACK wait must not finalize a skipped report.
  {
    const page = setup();
    const leaving = page.skipReport();
    await page.settle();
    page.scope.state.phase = "report_recovery";
    page.acknowledge();
    await leaving;
    assert.ok(page.events.includes("stop-recovery"));
    assert.ok(!page.events.includes("finish-recovery"));
    assert.ok(page.events.includes("disconnect"));
    assert.equal(page.scope.window.location.href, "/");
  }
});
