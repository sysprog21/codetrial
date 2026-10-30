// Shared helpers for the browser tests: reading source, and launching one.
//
// These tests read `web/*.js` and assert on text, because nothing here can
// execute `web/interview.js`. That makes them tripwires rather than proofs, and
// a tripwire that fails OPEN is worse than none: every copy of the
// function-body slicer used to terminate on `"\nfunction "`, so a body whose
// next sibling was an `async function` ran on to the end of the file and the
// assertions against it passed against unrelated code. `web/interview.js`
// declares fifteen `async function`s, so that was one edit away from happening.

import assert from "node:assert/strict";
import { createServer } from "node:http";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, extname, join, sep } from "node:path";

export const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

/// Asserts that `snippet` occurs in `source` once both have their whitespace
/// removed and a comma before a closing bracket dropped, so an assertion on
/// formatted source reads the same whether or not the formatter wrapped a call
/// and gave it a trailing comma. Whitespace inside a string literal goes too,
/// so a snippet whose meaning rests on a space in a string needs its own check.
/// The failure names the snippet, which a bare boolean in assert.ok would not.
export function assertIncludesCompact(source, snippet, message) {
  const compact = (text) =>
    text.replace(/\s+/g, "").replace(/,(?=[)\]}])/g, "");
  assert.ok(
    compact(source).includes(compact(snippet)),
    `${message ?? "source text is missing"}: ${snippet}`,
  );
}

export function read(name) {
  return readFileSync(join(root, name), "utf8");
}

/// `web/interview.js` and the modules it hands its bindings to.
///
/// One list, in one place, because every assertion that reads it is a
/// whole-path one. "setSinkId targets Jim and nothing else" and "the avatar
/// never reads the candidate's microphone" are properties of the interview
/// path, not of a filename, and a per-test subset of that path quietly stops
/// covering whatever moves outside it. Five copies of this list would mean a
/// seventh module is covered by none of them until all five are edited, which
/// is the fail-open this file exists to warn about.
///
/// `interview_path_is_read_whole` in dom-contract.test.js keeps it honest: it
/// derives the same set from the source and fails when the two disagree.
export const INTERVIEW_SOURCES = [
  "web/interview.js",
  "web/audio-output.js",
  "web/captions.js",
  "web/integrity.js",
  "web/recording-state.js",
  "web/replay-feed.js",
  "web/avatar/stage.js",
];

/// The interview path as one text.
///
/// The whole-file checks that read this are the kind a split defeats:
/// `doesNotMatch` over one file says nothing about the sibling the code moved
/// into, and an allowlist counted per file stops counting the call sites that
/// left. Read together, the property the assertion was written for survives the
/// move, because it was a property of the path and not of a filename.
export function interviewSource() {
  return INTERVIEW_SOURCES.map(read).join("\n");
}

/// Loads named functions out of web/interview.js against `scope`, which stands
/// in for the module around them. Names not in `scope` fall through to the
/// real globals, which is how `JSON` and `TextDecoder` reach them. With
/// `stubMissing` every other name outside `scope`, globals such as `JSON`
/// included, resolves to a stub that takes any call, property or spread, so a
/// test names only the bindings whose effect it asserts.
export function loadInterview(names, scope, { stubMissing = false } = {}) {
  const source = read("web/interview.js");
  const bodies = names.map((name) => `${functionBody(source, name)}\n}`);
  const exports = `{ ${names.join(", ")} }`;
  const stub = new Proxy(function () {}, {
    get: (_, key) => (key === Symbol.iterator ? function* () {} : stub),
    apply: () => stub,
  });
  // Values rather than bindings: stubbed, `x === undefined` would quietly take
  // the wrong branch.
  const values = new Set(["undefined", "NaN", "Infinity"]);
  const has = (target, key) =>
    key in target || (stubMissing && !values.has(key));
  const get = (target, key) =>
    key in target || !stubMissing || key === Symbol.unscopables
      ? target[key]
      : stub;
  return new Function(
    "scope",
    `with (scope) { ${bodies.join("\n")}\n return ${exports}; }`,
  )(new Proxy(scope, { has, get }));
}

/// The modules `interview.js` initialises, read back out of the source rather
/// than listed a second time. A cluster split out of the page script announces
/// itself with an `init*` export and an `init*(...)` call, so this finds the
/// ones a hand-written list would have missed.
export function initialisedModules() {
  const page = read("web/interview.js");
  const called = new Set(captures(page, /^(init[A-Z]\w*)\(/gm));
  return firstPartyScripts()
    .map((name) => `web/${name}`)
    .filter((path) => {
      const exported = captures(
        read(path),
        /^export function (init[A-Z]\w*)\(/gm,
      );
      return exported.some((name) => called.has(name));
    });
}

/// Every first-party browser script, `web/vendor/` excluded by path. Recursive,
/// so a bridge added under `web/avatar/` or `web/recording/` is covered too.
export function firstPartyScripts() {
  return readdirSync(join(root, "web"), { recursive: true })
    .map((name) => String(name).replace(/\\/g, "/"))
    .filter((name) => name.endsWith(".js") && !name.startsWith("vendor/"));
}

/// The agent runtime as one text: `src/livekit.rs` and the modules under
/// `src/livekit/`.
///
/// The same reason [`interviewSource`] exists, in the other language. The
/// agent-state pin counted `set_agent_state` call sites in the root file, and
/// when the Gemini event half moved into `src/livekit/events.rs` it took the
/// only `AGENT_STATE_SPEAKING` publish with it: one state published where two
/// are declared, from a move that changed no behaviour. Read together, the
/// property survives the move, because it was a property of the runtime and
/// not of a filename.
///
/// Globbed rather than listed, so the next split needs no edit here to stay
/// honest. Recursive for the same reason [`firstPartyScripts`] is: a split that
/// puts the next module a directory down is still a split, and a one-level read
/// would drop it from the source these pins are computed over -- which is the
/// fail-open this function exists to close, arriving one directory later.
export function livekitSource() {
  return [
    read("src/livekit.rs"),
    ...readdirSync(join(root, "src/livekit"), { recursive: true })
      .map((name) => String(name).replace(/\\/g, "/"))
      .filter((name) => name.endsWith(".rs"))
      .map((name) => read(join("src/livekit", name))),
  ].join("\n");
}

/// One function body, terminated by the closing brace at column zero rather
/// than by whatever declaration happens to come next. Throws on a name it
/// cannot find, so a renamed function fails the test that pins it instead of
/// silently asserting against an empty string.
export function functionBody(source, name) {
  const start = source.search(
    new RegExp(`^(?:export )?(?:async )?function ${name}\\(`, "m"),
  );
  if (start === -1) throw new Error(`no function named ${name}`);
  const end = source.indexOf("\n}\n", start);
  return source.slice(start, end === -1 ? source.length : end);
}

/// Installs a `globalThis.fetch` stub and returns its undo. Shared rather than
/// re-written per file: two copies had already appeared, and a restore that one
/// of them forgets leaks a stub into every test that runs after it.
export function failFetchWith(handler) {
  const previous = globalThis.fetch;
  globalThis.fetch = handler;
  return () => {
    globalThis.fetch = previous;
  };
}

/// Replaces a global for the length of one test, restored even when an
/// assertion fails. Shared for the fetch stub's reason: the save-and-restore
/// had been written out by hand in half a dozen files, and a restore one of
/// them forgets leaks a fake into every test that runs after it.
export function stubGlobal(t, name, value) {
  const previous = Object.getOwnPropertyDescriptor(globalThis, name);
  Object.defineProperty(globalThis, name, { configurable: true, value });
  t.after(() => {
    if (previous) Object.defineProperty(globalThis, name, previous);
    else delete globalThis[name];
  });
}

/// A `localStorage` stand-in backed by a Map. Shared for the same reason as the
/// fetch stub above: two copies had appeared and they had drifted apart. One of
/// them read a stored empty string back as `null` and did not stringify what it
/// was given, so a test could pass against a stub that behaves as the real
/// storage does not.
/// `getItem` answers `null` only for a key that is absent, and `setItem`
/// stringifies, because that is what the browser does.
export function memoryStorage() {
  const data = new Map();
  return {
    getItem: (key) => data.get(key) ?? null,
    removeItem: (key) => data.delete(key),
    setItem: (key, value) => data.set(key, String(value)),
  };
}

/// A LiveKit room that connects to nothing and records what the page
/// publishes. Shared by browser flows that must prove an editor action does or
/// does not send a code_update packet; one fake keeps those publication tests
/// on the same room contract. Readiness is opt-in for flows that need a started
/// interview rather than only an available publication transport.
export function fakeLiveKit({ startupReady = false } = {}) {
  return `
window.LivekitClient = {
  RoomEvent: new Proxy({}, { get: (_, name) => String(name) }),
  Track: { Source: { Microphone: "microphone", Camera: "camera" } },
  Room: class {
    constructor() {
      const agent = ${startupReady ? JSON.stringify({ identity: "agent-fake-room", kind: "AGENT", attributes: {} }) : "null"};
      this.remoteParticipants = new Map(agent ? [[agent.identity, agent]] : []);
      this.name = "fake-room";
      this.localParticipant = {
        identity: "candidate",
        publishTrack: async () => {},
        publishData: async (bytes, options) => {
          (window.__published ||= []).push({
            topic: options.topic,
            payload: JSON.parse(new TextDecoder().decode(bytes)),
          });
          if (agent && options.topic === "integrity" && !this.ready) {
            this.ready = true;
            window.__handlers.DataReceived(
              new TextEncoder().encode(JSON.stringify({
                type: "interviewer_state", reconnecting: false, startup: "ready",
              })),
              agent, undefined, "control",
            );
          }
        },
        streamBytes: async () => ({ write: async () => {}, close: async () => {} }),
      };
    }
    on(name, handler) {
      (window.__handlers ||= {})[name] = handler;
      return this;
    }
    registerTextStreamHandler() {}
    async connect() {}
    async disconnect() {}
  },
};`;
}

export const FAKE_LIVEKIT = fakeLiveKit();

// Speech as the fake microphone, not Chromium's built-in beep: under a local
// sound server the beep is too short for the microphone check that enables
// Join, so a test passes on a runner without one and fails on a desktop.
export const FAKE_SPEECH_CAPTURE = `--use-file-for-fake-audio-capture=${join(root, "web", "audio", "test-voice.wav")}`;

/// The browser, or null when there is no browser to be had. Shared so that
/// every file launching one agrees on what a missing Playwright or an
/// undownloaded Chromium means. `make check` installs neither, so a null here
/// reads as "unverified", not "fine".
///
/// Which is why CI gets a throw instead. A skipped suite reports its tests, no
/// failures, and exit 0, so if the workflow's Chromium install ever stops
/// working the browser tests go on passing while running nothing and a green
/// build is the only evidence. `.github/workflows/check.yml` installs it before
/// the gate, so there is no legitimate skip there.
export async function launchChromium(options = {}) {
  try {
    const { chromium } = await import("playwright");
    return await chromium.launch(options);
  } catch (error) {
    // `cause` rather than the message alone: when CI does break, the frame that
    // names what went wrong is the launcher's, not this one's.
    if (process.env.CI) {
      throw new Error(
        "CI has no usable Chromium, so the browser suite would test nothing",
        {
          cause: error,
        },
      );
    }
    return null;
  }
}

/// What `/runtime-config.js` answers with when a caller passes none of its
/// own. Stands in for `runtime_config_handler` in `src/web/assets.rs`, which
/// `tests/web.rs` pins against the real server; the interview page fetches
/// this before `bindEvents` runs, so a caller that loads it with no value at
/// all is a fetch racing a 404 rather than a stub.
export const DEFAULT_RUNTIME_CONFIG = `globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = false;
globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = "";
globalThis.CODETRIAL_RECORDING_ENABLED = false;
globalThis.CODETRIAL_CONSENT_VERSION = "";
globalThis.CODETRIAL_REPLAY_VERSION = 1;
`;

const STATIC_CONTENT_TYPES = {
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".html": "text/html",
  ".css": "text/css",
  ".wasm": "application/wasm",
  ".wav": "audio/wav",
};

/// A static file server over `web/`, the way the browser sees it once
/// `tests/browser/dom.js`'s stub is out of reach for a page. Shared rather
/// than re-written per file: this same handful of lines had already been
/// copied into two browser test files -- lobby.test.js for its own static
/// fallback, candidate-cases.test.js whole -- before a third arrived needing
/// it too, and the `/runtime-config.js` stub in particular is a copy of
/// `runtime_config_handler` that nothing keeps in step with the original
/// when it changes.
///
/// `handle`, when given, runs first and may answer the request itself --
/// returning a truthy value once it has called something on `response` -- for
/// routes static serving cannot, such as an API a page under test fetches.
/// Anything it declines falls through to `web/` on disk. `runtimeConfig`
/// answers `/runtime-config.js`; omit it for a page that never fetches that
/// file, such as the lobby.
export async function startStaticServer({ handle, runtimeConfig } = {}) {
  const web = join(root, "web");
  const server = createServer(async (request, response) => {
    const url = new URL(request.url, "http://browser-test.invalid");
    if (handle && (await handle(request, response, url))) return;
    if (runtimeConfig !== undefined && url.pathname === "/runtime-config.js") {
      response.setHeader("content-type", "text/javascript");
      response.end(runtimeConfig);
      return;
    }
    const file = join(web, url.pathname === "/" ? "index.html" : url.pathname);
    if (
      !file.startsWith(web + sep) ||
      !existsSync(file) ||
      statSync(file).isDirectory()
    ) {
      response.statusCode = 404;
      response.end("not found");
      return;
    }
    response.setHeader(
      "content-type",
      STATIC_CONTENT_TYPES[extname(file)] ?? "application/json",
    );
    response.end(readFileSync(file));
  });
  await new Promise((listening) => server.listen(0, "127.0.0.1", listening));
  return { server, base: `http://127.0.0.1:${server.address().port}` };
}

/// The interview's setup check closed the way a passing check closes it, for a
/// test about what lies behind it. It is a modal dialog over an inert
/// workspace, so this waits for the workspace to come back rather than for a
/// fixed time: until then nothing behind it can take focus.
export async function closeSetupCheck(page) {
  await page.evaluate(() => {
    document.querySelector("#audio-check").hidden = true;
  });
  await page.waitForFunction(
    () => !document.querySelector(".editor-panel").inert,
  );
}

/// Stops a page's installed fake clock about where it stands. Playwright pauses
/// only at a named time, and the clock keeps moving between reading the time
/// and pausing at it: a loaded machine has spent more than 100ms there, past
/// the target, and `pauseAt` refuses to go back. The refused call has already
/// stopped the clock, so a target read again lands ahead of it.
export async function pauseClock(page) {
  for (let attempt = 1; ; attempt++) {
    const at = await page.evaluate(() => Date.now() + 100);
    try {
      return await page.clock.pauseAt(at);
    } catch (error) {
      if (attempt === 5 || !/fast-forward to the past/.test(error.message))
        throw error;
    }
  }
}

/// Waits for the page to reach a state and says whether it did, so the
/// assertion after it fails in one line naming what was wrong instead of as a
/// timeout naming a predicate. The wait removes the race; it is not the check.
/// No shorter cap than Playwright's default: on a loaded runner the wait alone
/// can take seconds before it first reads the page. Only a timeout means "not
/// reached"; a predicate that throws or a page that closed is reported as is.
export function reaches(page, predicate, arg) {
  return page.waitForFunction(predicate, arg).then(
    () => true,
    (error) => {
      if (error.name === "TimeoutError") return false;
      throw error;
    },
  );
}

export function captures(source, pattern) {
  return [...source.matchAll(pattern)].map((match) => match[1]);
}
