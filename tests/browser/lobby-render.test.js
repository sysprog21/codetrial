// The lobby page, driven rather than read.
//
// `web/app.js` is 676 lines, exports nothing, and reads its nodes at module
// scope, so until this file the only things that ran it were a browser and
// `./scripts/browser-check.sh`, which is optional and skipped on a checkout
// with no Chromium. Everything else about it was a check on its source text.
//
// What this adds is small and is the half that source reading cannot do: the
// page is imported, so a module-scope throw fails a `node --test` run, and what
// it wrote into the document is read back, so the copy a signed-out visitor
// actually sees is pinned to strings somebody chose.
//
// It is here as much to hold `tests/browser/dom.js` to being general as to hold
// the lobby. That stub was written for the replay page and claimed to be
// page-agnostic; a second caller is what turns the claim into a fact, and it
// found two document-level gaps when it arrived.
//
// The boundary is in `dom.js` and is not repeated here beyond what this file
// depends on: `document.querySelectorAll` answers ids and nothing else, so the
// problem cards, the difficulty radios, the duration buttons and the loop
// buttons are all absent, and this drives the page that is left. What that page
// still proves is the import, the unidentified account state, and the branch the
// progress panel takes when it cannot read anything.
import { test } from "node:test";
import assert from "node:assert/strict";
import { read } from "./source.js";
import { installDocument } from "./dom.js";

const markup = read("web/index.html");
const dom = installDocument(markup);

// Site data blocked, said here rather than left to the runtime: from Node 25
// `sessionStorage` is a working store by default and `localStorage` is one
// once `--localstorage-file` names a file (an empty object without it), while
// plain `node` before 25 has neither. What the page renders would depend on
// how Node runs it, so both throw on access, as a browser with site data
// blocked does.
for (const name of ["localStorage", "sessionStorage"]) {
  Object.defineProperty(globalThis, name, {
    configurable: true,
    get() {
      throw new DOMException("blocked", "SecurityError");
    },
  });
}

// Imported once, at the top, because ESM caches by URL and the page does its
// work while it loads. Every test below reads the same rendered document.
await import("/app.js");

/// What the page may say to a visitor it cannot identify, on a device with no
/// history.
///
/// Neither is contrived: `installDocument` refuses the fetch the page starts at
/// import, so the session lookup fails, and the storage block above decides
/// the rest. The blocked `sessionStorage` is what turns that failed lookup into
/// the account retry: `account-status`, `login-link`, `start` and
/// `progress-summary` all follow from it. The blocked `localStorage` is what
/// makes the practice list unreadable, which is the only reason
/// `practice-list-status` ends by saying browser storage is unavailable. No
/// expectation here needs either store to work. That is the worst case a real
/// visitor can also reach, with site data blocked and the network down, and it
/// is the one state this file can produce without a browser. Start is held
/// there, because a server that requires sign-in would send the visitor
/// straight back, so the page offers the account retry.
const UNIDENTIFIED = {
  "account-status": "Could not check your account. Retry the account check.",
  "login-link": "Retry account check",
  "progress-problem": "all",
  start: "Start interview",
  "progress-summary":
    "Could not load account progress. Retry the account check.",
  "practice-list-status":
    "0 problems in your list. Open the problem picker below to check problems. Kept for this visit only; browser storage is unavailable.",
};
test("the lobby page loads outside a browser and says what it was given", () => {
  for (const [id, expected] of Object.entries(UNIDENTIFIED)) {
    assert.deepEqual(dom.node(id).spoken(), [expected], `#${id}`);
  }
});

test("no other panel of the lobby speaks before anything has loaded", () => {
  // An allowlist over the whole page rather than a check on three nodes. A
  // panel that starts populated is a panel showing a visitor something that is
  // not about them yet, and the failure this catches is a default that looks
  // like data: the progress panels and the grounding statuses are the ones with
  // somewhere to put a sentence nobody asked for.
  const spoke = [];
  for (const [, id] of markup.matchAll(/id="([^"]+)"/g)) {
    const node = dom.node(id);
    if (node && node.spoken().length && !(id in UNIDENTIFIED)) spoke.push(id);
  }
  assert.deepEqual(
    spoke,
    [],
    "a panel put words on the lobby before it had any",
  );
});
