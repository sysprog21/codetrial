// Shared source-reading helpers for the browser tests.
//
// These tests read `web/*.js` and assert on text, because nothing here can
// execute `web/interview.js`. That makes them tripwires rather than proofs, and
// a tripwire that fails OPEN is worse than none: every copy of the
// function-body slicer used to terminate on `"\nfunction "`, so a body whose
// next sibling was an `async function` ran on to the end of the file and the
// assertions against it passed against unrelated code. `web/interview.js`
// declares fifteen `async function`s, so that was one edit away from happening.

import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

export const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

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
      const exported = captures(read(path), /^export function (init[A-Z]\w*)\(/gm);
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

/// One function body, terminated by the closing brace at column zero rather
/// than by whatever declaration happens to come next. Throws on a name it
/// cannot find, so a renamed function fails the test that pins it instead of
/// silently asserting against an empty string.
export function functionBody(source, name) {
  const start = source.search(new RegExp(`^(?:export )?(?:async )?function ${name}\\(`, "m"));
  if (start === -1) throw new Error(`no function named ${name}`);
  const end = source.indexOf("\n}\n", start);
  return source.slice(start, end === -1 ? source.length : end);
}

export function captures(source, pattern) {
  return [...source.matchAll(pattern)].map((match) => match[1]);
}
