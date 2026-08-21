// Run with: node --test tests/browser/history.test.js
//
// The replay page reads an account's own recordings back. Nothing here can run
// it, so these read the page and its script as text: what ids exist, what the
// script does with the two answers that mean the media is gone, and the one
// rule the page must never break, which is that it has no link to a video.
//
// That rule is not really enforced here. It is enforced by the routes, which
// return no object path, no Drive file id and no permission id at all; this is
// the tripwire that says so out loud, so a future edit that starts rendering
// one fails a test rather than shipping.

import { test } from "node:test";
import assert from "node:assert/strict";
import { functionBody, read } from "./source.js";

const page = read("web/replay.html");
const script = read("web/replay.js");
const withoutComments = (source) => source.replace(/^\s*\/\/.*$/gm, "").replace(/^\s*\/\/\/.*$/gm, "");
const code = withoutComments(script);

test("replay page dom ids", () => {
  const required = [
    "replay-list",
    "replay-more",
    "replay-empty",
    "replay-title",
    "replay-status",
    "replay-media",
    "replay-transcript",
    "replay-timeline",
    "replay-moment-label",
    "replay-code",
    "replay-tests",
    "replay-report",
  ];
  for (const id of required) {
    assert.ok(page.includes(`id="${id}"`), `the page needs #${id}`);
    assert.ok(
      code.includes(`querySelector("#${id}")`),
      `#${id} is in the page and nothing reads it`,
    );
  }
  assert.ok(page.includes('src="/replay.js"'), "the page loads its own script");
  assert.ok(
    !/href="https?:\/\//.test(page),
    "no page-level link off this origin; the media lives somewhere this page is not told about",
  );
});

test("replay missing drive file", () => {
  // "No video" is a state, not an error. Every state the server can report has
  // a sentence, and none of them is a link.
  const words = code.slice(code.indexOf("const MEDIA_WORDS"), code.indexOf("let cursor"));
  for (const state of ["ready", "recording", "transferring", "failed", "deleted"]) {
    assert.ok(words.includes(`${state}:`), `the page has to say something about ${state}`);
  }
  assert.ok(
    /There is no video for this interview\./.test(words),
    "a recording that failed says so plainly",
  );
  assert.ok(
    !/https?:\/\/(?!localhost)/.test(code),
    "the page holds no URL to media, because the server never gives it one",
  );
  assert.ok(
    !/drive\.google|storage\.googleapis|gcsObject|driveFileId|drivePermissionId/.test(script),
    "and it does not know the words for one either",
  );

  const select = withoutComments(functionBody(script, "select"));
  assert.ok(
    select.includes('MEDIA_WORDS[recording.state] ||'),
    "the sentence comes from the state the server reported",
  );
  assert.ok(
    select.includes("recording.quotaExceeded"),
    "a replay that stopped growing says so, or its tail looks like an interview that ended early",
  );
});

test("replay expired", () => {
  const gone = code.slice(code.indexOf("const GONE_WORDS"), code.indexOf("export function momentTime"));
  assert.ok(gone.includes("replay_expired:"), "past its retention deadline");
  assert.ok(gone.includes("recording_deleted:"), "and taken away are different things");

  const select = withoutComments(functionBody(script, "select"));
  assert.ok(
    select.includes("response.status === 410"),
    "a recording that is gone is not an error to apologise for",
  );
  assert.ok(
    select.includes("GONE_WORDS[body.code]"),
    "and the page says which of the two it was",
  );

  const events = withoutComments(functionBody(script, "loadEvents"));
  assert.ok(
    events.includes("if (response.status === 410)"),
    "the replay can expire between the two reads, and the page has to survive that",
  );
});

test("replay renders moments, not media", () => {
  const render = withoutComments(functionBody(script, "render"));
  assert.ok(
    render.includes('event.kind === "transcript"'),
    "the transcript accumulates, which is what the schema says",
  );
  assert.ok(
    render.includes('event.kind === "editor" || event.kind === "tests"'),
    "and the snapshots are the moments",
  );

  const moment = withoutComments(functionBody(script, "showMoment"));
  assert.ok(
    moment.includes("moments.slice(0, index + 1)"),
    "the state at a moment is the last of each kind up to it, not the chosen event",
  );
  assert.ok(
    moment.includes("nodes.code.textContent = code;"),
    "the candidate's own code is written as text",
  );
  assert.ok(
    !/nodes\.code\.innerHTML/.test(code),
    "and never as markup, whatever the value is called by then",
  );

  // A click while a fetch is in flight is the normal case on a list, not an
  // edge one: without the guard the older answer paints over the newer and the
  // page shows one recording's state beside another's transcript.
  for (const name of ["select", "loadEvents", "loadReport"]) {
    const body = withoutComments(functionBody(script, name));
    // At least twice: once for the request and once for reading its body.
    // Parsing is another await, and a click during it is another chance for
    // this answer to land on somebody else's recording.
    const guards = (body.match(/selected !== recordingId/g) || []).length;
    assert.ok(guards >= 2, `${name} guards ${guards} of its awaits`);
    // And nothing written between reading a body and checking again.
    for (const after of body.split("json()").slice(1)) {
      const wrote = after.indexOf("nodes.");
      const guarded = after.indexOf("selected !== recordingId");
      assert.ok(
        guarded !== -1 && (wrote === -1 || guarded < wrote),
        `${name} writes to the page before rechecking after a parse`,
      );
    }
  }

  // A read that failed is not an interview that recorded nothing, and the page
  // has to be able to tell a person which one this is.
  assert.ok(code.includes("Could not load the replay for this interview."));
  assert.ok(code.includes("Could not load the report."));
  assert.ok(code.includes("No report was saved for this interview."));
});
