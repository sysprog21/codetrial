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
