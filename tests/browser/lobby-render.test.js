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
// still proves is the import, the signed-out account state, and the branch the
// progress panel takes when it cannot read anything.
import { test } from "node:test";
import assert from "node:assert/strict";
import { read } from "./source.js";
import { installDocument } from "./dom.js";

const markup = read("web/index.html");
const dom = installDocument(markup);

// Imported once, at the top, because ESM caches by URL and the page does its
// work while it loads. Every test below reads the same rendered document.
await import("/app.js");

/// What the page may say to a signed-out visitor whose device has no history.
///
/// Both of those are the stub's doing and neither is contrived: `installDocument`
/// refuses the fetch the page starts at import, so the session lookup fails, and
/// plain `node` has no `localStorage`, so the saved-progress read fails too.
/// That is the worst case a real visitor can also reach, on a first visit with
/// the network down, and it is the one state this file can produce without a
/// browser.
const SIGNED_OUT = {
  "account-status": "Signed out",
  start: "Start interview",
  "progress-summary": "Could not load progress saved on this device.",
};

test("the lobby page loads outside a browser and says what it was given", () => {
  for (const [id, expected] of Object.entries(SIGNED_OUT)) {
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
    if (node && node.spoken().length && !(id in SIGNED_OUT)) spoke.push(id);
  }
  assert.deepEqual(spoke, [], "a panel put words on the lobby before it had any");
});
