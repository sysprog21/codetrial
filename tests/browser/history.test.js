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
    "replay-window-note",
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
  // Every transcript event becomes a line, which is what the schema means by a
  // transcript accumulating. Asserted as the two ends rather than as the control
  // flow between them: an earlier version pinned `if (... !== "transcript")
  // continue;`, which fails an identical rewrite as `if (... === "transcript")
  // { ... }` and holds no property either shape does not.
  assert.match(render, /for \(const event of rows\)/);
  assert.match(render, /nodes\.transcript\.append\(line\)/);
  // A body that is not a list is an empty replay rather than a throw, and the
  // guard has to be here: `findLast` runs before anything downstream is
  // reached, so a guard only inside `replayTimeline` is one the page never gets
  // to. What the guard then does is driven in tests/browser/lib.test.js.
  assert.match(render, /const rows = replayRows\(events\)/);

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

test("a replay with no recorded mode shows no dangling separator", () => {
  // select() sets the status text before render() runs, so the status node is
  // always truthy by the time the mode is appended. Every recording made since
  // the mode was removed has an empty mode, and appending it unconditionally
  // left the status reading "Recording ready · " with nothing after the dot.
  // The append has to sit inside the guard, not merely somewhere near it.
  const render = functionBody(code, "render");
  assert.match(render, /if \(mode\) \{\s*nodes\.status\.textContent =/);
});

test("a rate-limited replay batch is kept, not dropped", () => {
  // The batch is spliced off the queue before the request, so a status nobody
  // handles loses that stretch of the interview for good. Rate limiting bounds
  // how often a browser may ask; it is not a decision about which evidence
  // survives, which is what silently discarding the batch made it.
  const feed = withoutComments(read("web/replay-feed.js"));
  const send = functionBody(feed, "sendQueuedBatch");
  assert.match(send, /=== 429/, "429 has to be handled at all");
  assert.match(send, /429[\s\S]*replayQueue\.unshift\(\.\.\.batch\)/, "and handled by keeping it");

  // Keeping it is only half the answer. Putting the batch back leaves the
  // queue at the size that makes recordReplay flush on sight, so a limit
  // answered without a wait becomes a request per event for the rest of the
  // window: every event asks again, and every one earns another refusal.
  assert.match(send, /429[\s\S]*retryAfter =/, "and by waiting out the window");
  assert.match(
    functionBody(feed, "recordReplay"),
    /REPLAY_MAX_BATCH && Date\.now\(\) >= retryAfter/,
    "the fill trigger has to respect the wait, or nothing does",
  );
});

/// Every string literal in a slice of source, as the values they parse to
/// rather than as the escapes they were written with.
///
/// All three quotings. `eslint.config.mjs` has no `quotes` rule and nothing else
/// pins one, so a check that read only double quotes would be telling the next
/// person that a single-quoted or backticked label is allowed; both were written
/// and shown to pass an earlier version of this. Single-quoted and backticked
/// text is requoted before parsing, because JSON accepts neither.
///
/// `JSON.parse` rather than the raw match, so an escape in the file and the
/// character in the allowlist are the same string here. Comment lines go first:
/// a `///` paragraph explaining the copy is not part of the copy.
function literals(source) {
  const bare = source.replace(/^\s*\/\/.*$/gm, "");
  const requote = (text) => `"${text.replace(/\\(['`])/g, "$1").replace(/"/g, '\\"')}"`;
  return [
    ...[...bare.matchAll(/"(?:[^"\\\n]|\\.)*"/g)].map((match) => match[0]),
    ...[...bare.matchAll(/'(?:[^'\\\n]|\\.)*'/g)].map((match) => requote(match[0].slice(1, -1))),
    ...[...bare.matchAll(/`(?:[^`\\]|\\.)*`/g)].map((match) =>
      requote(match[0].slice(1, -1).replace(/\$\{[^}]*\}/g, "").replace(/\n/g, " ")),
    ),
  ].map((text) => JSON.parse(text));
}

test("the response window panel says what the number is worth, in words a test can hold", () => {
  // Integrity features produce evidence for human review, and a duration
  // offered as evidence owes its provenance. "No severity language anywhere" is
  // a negative no test can observe, so this is the positive form: the panel may
  // use these strings and no others, and the next person to add a label is told
  // by a failure rather than by nobody.

  // Half of it is the paragraph, which is static because it needs no state.
  const note = page.slice(page.indexOf('id="replay-window-note"'), page.indexOf("</p>", page.indexOf('id="replay-window-note"')));
  const prose = note.slice(note.indexOf(">") + 1).replace(/\s+/g, " ").trim();
  assert.equal(
    prose,
      "A response window is the time between the interviewer finishing a turn and the interviewer " +
      "speaking again. It is measured from the clock of the browser that recorded the interview, which " +
      "can jump forward as well as back, and it includes the time CodeTrial itself took to prepare the " +
      "reply, which is not the same on every turn. The interviewer speaks again on its own after about " +
      "twenty-five seconds of silence, and neither talking nor typing counts as silence, so a window " +
      "runs long only while the candidate is working: a long window means they were busy, and a run of " +
      "short windows holding no transcript is what silence looks like. A window opens whenever the " +
      "interviewer stops speaking, which is often not the end of a question: it also follows the " +
      "interviewer's own prompts and reactions, a candidate interrupting, a moment when the interviewer " +
      "published no state at all, the interviewer dropping out and coming back, and the candidate " +
      "pausing the interview, which is the one of those a window says on itself. Events reach the " +
      "server in batches and a batch can be lost, so a window can be missing, doubled, shown with no " +
      "candidate transcript, wrongly marked as paused, or added after the interview ended. A window " +
      "shows no duration when the recording ended inside it, or when that clock ran backwards. Each " +
      "turn carries the window its stream started in, and a recording made before that says so on " +
      "every window it cannot place, rather than reporting a question nobody answered. A long " +
      "window is a place in the recording to go and watch, not a finding about anybody.",
  );
  // Each thing that has to reach the reader, named so a rewrite that drops one
  // fails rather than reading fine.
  assert.match(prose, /time between the interviewer finishing a turn/, "what it measures");
  assert.match(
    prose,
    /a recording made before that says so on every window it cannot place/,
    "that an older recording withholds the attribution rather than denying it",
  );
  assert.match(prose, /speaks again on its own after about twenty-five seconds/, "what bounds it");
  assert.match(prose, /a long window means they were busy/, "so a long one is not the shape to look for");
  assert.match(prose, /run of short windows holding no transcript/, "and this one is");
  assert.match(prose, /clock of the browser that recorded the interview/, "whose clock measured it");
  assert.match(prose, /jump forward as well as back/, "in both directions, so a long reading is not a measurement either");
  assert.match(prose, /published no state at all/, "a window can open without a question having ended");
  assert.match(prose, /follows the interviewer's own prompts and reactions/, "and most often does");
  assert.match(prose, /not a finding about anybody/, "and it is not an accusation");
  assert.match(prose, /includes the time CodeTrial itself took/, "whose latency is inside the number");
  assert.match(prose, /a candidate interrupting/, "a window is not always a question ending");
  // Every consequence of a lost batch, named rather than summarised. A merged
  // window reads as a long one, a missing window is a question the panel never
  // lists, a lost transcript row makes an answered question look unanswered, and
  // losing the interviewer's first row empties the panel entirely.
  assert.match(prose, /missing, doubled, shown with no candidate transcript, wrongly marked as paused, or added after the interview ended/);
  assert.match(prose, /the candidate pausing the interview/, "the one cause a window can name");

  // And nothing else on the page speaks. Pinning the paragraph by its id guards
  // the paragraph, and pinning one `<section>` guards one section: a sibling
  // with no id was written and passed, and so was a paragraph under the Report
  // heading, which renders on the same page a reviewer is reading. What this
  // page may say is therefore the whole of what it says, comments and markup
  // removed.
  // From the top of the document, not from `<body>`. The `<title>` is what a
  // browser writes on the tab, which is in front of the reader like anything
  // else, and slicing it off was a surface this list did not cover.
  const spoken = page
    .slice(page.indexOf("<head>"))
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/<[^>]*>/g, "\u0001")
    .split("\u0001")
    .map((text) => text.replace(/\s+/g, " ").trim())
    .filter(Boolean);
  // Attributes as well as text. A browser speaks `aria-label` and shows `title`
  // on hover, and the tag-stripping below erases both before the allowlist sees
  // them, so `<h3 title="likely assisted">Moments</h3>` renders a sentence the
  // list would not hold. Both quote styles: HTML takes either, and an earlier
  // version of this took one.
  assert.deepEqual(
    [...page.matchAll(/\s(?:title|aria-label|alt|placeholder)\s*=\s*("[^"]*"|'[^']*')/g)].map(
      (match) => match[1].slice(1, -1),
    ),
    [],
    "this page says nothing through an attribute",
  );

  // And no stylesheet of its own. `tests/browser/replay-render.test.js` reads
  // `web/styles.css` and would not see a second one written here. The scan below
  // would: it starts at `<head>` and strips tags, so the rules land in it as
  // text. This fails first and with a better name, which is why it is here and
  // not left to that.
  assert.doesNotMatch(page, /<style/i, "this page has no stylesheet of its own");

  assert.deepEqual(
    spoken,
    [
      "CodeTrial - Recordings",
      "CODETRIAL",
      "Your recordings",
      "Interviews this account recorded. A recording is kept for 24 hours and then deleted, along with everything on this page.",
      "Older",
      "No recordings yet.",
      "Choose a recording",
      "What was said",
      "Moments",
      prose,
      "Code",
      "Report",
    ],
    "every word this page says without being told one by the server",
  );

  // The stylesheet is not read here. `tests/browser/replay-render.test.js` scans
  // it for every property that draws text, which is strictly more than the
  // `content:` scan that stood here, so one file owns `web/styles.css` rather
  // than two disagreeing about how thoroughly it was checked.

  // The other half is the per-window label, which is the one place a severity
  // word could be added by hand.
  const WINDOW_COPY = [
    "response window",
    " s",
    " · ",
    "duration not recorded",
    "no candidate transcript recorded",
    "transcript not matched to a window",
    "interview paused during this window",
  ];
  const words = script.slice(script.indexOf("const WINDOW_WORDS = {"));
  assert.deepEqual(
    new Set(literals(words.slice(0, words.indexOf("\n};")))),
    new Set(WINDOW_COPY),
    "the words table says these and no others",
  );

  // Every string this page can say, in one set. Deleting this when the render
  // check landed was a straight loss: that check drives `render`, and
  // `MEDIA_WORDS`, `GONE_WORDS` and every "could not load" sentence are written
  // by `select`, `loadEvents` and `loadReport`, which it does not drive. The two
  // are complements and were treated as alternatives.
  assert.deepEqual(
    new Set(literals(script)),
    new Set([
      // Wiring: selectors, module paths, tags, classes, dataset and event names.
      "", "/lib.js", "/render.js", "/api/reports", "button", "div", "li", "click", "current",
      "[data-moment]", "replay-item", "replay-line",
      "replay-moment", "replay-window", "#replay-code", "#replay-empty", "#replay-list",
      "#replay-media", "#replay-moment-label", "#replay-more", "#replay-report",
      "#replay-status", "#replay-tests", "#replay-timeline", "#replay-title",
      "#replay-transcript", "#replay-window-note",
      // Replay event kinds and the speaker a missing one reads as.
      "stage", "transcript", "editor", "tests", "candidate",
      // What the page says about the media, per state.
      "The video was shared with your verified email address. It is deleted 24 hours after the interview.",
      "This interview is still being recorded.", "The recording is being finished.",
      "The recording is being delivered to your email address.",
      "There is no video for this interview.", "The video has been deleted.",
      "This recording is past its 24 hours and has been deleted.",
      "This recording has been deleted.", "This recording is not available on this account.",
      " The replay below stops before the end of the interview.",
      // What it says when a read failed, which is not the same as nothing
      // recorded.
      "Could not load the replay for this interview.", "Could not load the report.",
      "Could not read your recordings.", "No report was saved for this interview.",
      "Sign in to see your recordings.", "No test run before this point.",
      "Recording", "Deleted", "Not available", "Code",
      // And the window panel, spread from the set pinned above rather than
      // retyped: the narrower assertion catches a word relocated out of
      // `WINDOW_WORDS`, and this one catches a word added anywhere, so they are
      // different checks over one list.
      ...WINDOW_COPY,
      // Template literals, with their interpolations stripped: the routes this
      // page calls and the separators it joins values with. The events request
      // is here in the shape it is sent, which is the other half of the done
      // condition for this feature.
      "/api/recordings", "/api/recordings/", "/api/recordings//events?avatar=history",
      "?before=", " ·  · ", " : ", "Code · ", "/ passing",
    ]),
    "a string this page can say that is not in this list is one nobody chose",
  );

  // What the panel then does with those words is asserted by driving it, not
  // here, and the reason is the whole history of this test. Reading source for
  // labels was
  // defeated twelve times: by a single-quoted literal, a template literal, a
  // second table beside the first, a helper, a computed lookup, a spliced
  // element, `button.title`, `button[KEY]`, a local named something else, a
  // conditional field on the span, a third file, and a stylesheet. Each fix
  // named a surface, and the page renders through five of them.
  //
  // `tests/browser/replay-render.test.js` drives the render with sentinels in
  // every value the server supplies and reads what comes out, which has no such
  // edge. What is left here is the copy itself, which is worth naming where a
  // person reading the test can see it.
});

test("the replay page asks for the avatar history and renders it beside the moments", () => {
  // The done condition is the request, not what the page then draws with it.
  // What it draws is asserted in `tests/browser/replay-render.test.js`, which
  // does import `web/replay.js`; what it asks for is text, and text is what this
  // file reads.
  const events = withoutComments(functionBody(script, "loadEvents"));
  assert.match(
    events,
    /\/events\?avatar=history/,
    "the snapshot keeps one avatar row, and a window needs the history",
  );

  // Their own array, not `moments`. `data-moment` indexes that one and
  // `showMoment` rebuilds state by scanning its prefix, so a third kind mixed
  // in would shift every index and be scanned over as neither snapshot.
  const render = withoutComments(functionBody(script, "render"));
  assert.match(
    render,
    /const \{ moments, windows, timeline \} = replayTimeline\(rows\)/,
    "the page draws the timeline; web/lib.js decides it",
  );
  // What that timeline is, and that a window is not a moment, is asserted by
  // driving `replayTimeline` in tests/browser/lib.test.js. Source text was the
  // wrong tool for it and was the tool this used: an assembly that appended
  // every window after every moment matched the regexes that stood here.
  assert.match(
    withoutComments(functionBody(script, "windowItem")),
    /dataset\.window = String\(index\)/,
    "a window is addressed as a window",
  );

  // Only a moment controls the code and test panels, so a response window
  // cannot take their selection mark.
  const current = withoutComments(functionBody(script, "markCurrent"));
  assert.match(current, /\[data-moment\]/);
  assert.doesNotMatch(current, /data-window/);

  // The paragraph carries `hidden` in the page, so both of these have to be
  // here: without the second the caveats are never shown at all, and
  // without the first they stand over the next recording's empty timeline. The
  // whole show-and-hide was unguarded, and deleting both lines left the suite
  // green while the note stopped existing for a reader.
  assert.match(
    withoutComments(functionBody(script, "clearDetail")),
    /nodes\.windowNote\.hidden = true;/,
    "a new selection hides the paragraph until there is something to explain",
  );
  assert.match(
    render,
    /nodes\.windowNote\.hidden = windows\.length === 0;/,
    "and a replay with windows shows it",
  );
  assert.ok(page.includes('id="replay-window-note" class="muted small" hidden'));
});
