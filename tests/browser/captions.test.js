import { test } from "node:test";
import assert from "node:assert/strict";
import { functionBody, read } from "./source.js";
import { MAX_UNCLEAR_LINES } from "../../web/lib.js";

let captionCopies = 0;

async function setupCaptions(t, { scrollable = false, subpixel = 0 } = {}) {
  // Caption state is module-level, so each case imports a fresh copy.
  const captions = await import(
    `../../web/captions.js?copy=${++captionCopies}`
  );
  const nodes = { captionsText: { textContent: "" } };
  nodes.captionsBar = scrollable
    ? layoutBar(nodes, subpixel)
    : { hidden: true };
  captions.initCaptions({ nodes });
  t.mock.timers.enable({ apis: ["setTimeout"] });
  return { captions, nodes };
}

function finishJim(t, nodes, text) {
  const expected = `[Jim]: ${text}`;
  for (let ticks = 0; ticks < text.length; ticks++) {
    if (nodes.captionsText.textContent === expected) break;
    t.mock.timers.tick(300);
  }
  assert.equal(nodes.captionsText.textContent, expected);
}

function finishDismissal(t, nodes, text) {
  for (
    let ticks = 0;
    ticks < text.length && !nodes.captionsBar.hidden;
    ticks++
  ) {
    t.mock.timers.tick(300);
  }
  assert.equal(
    nodes.captionsBar.hidden,
    true,
    "a pending edit must dismiss Jim after the final reveal",
  );
}

// Execute page logic against test dependencies without its browser startup.
function loadInterviewFunction(name, scope) {
  const body = `${functionBody(read("web/interview.js"), name)}\n}`;
  return new Function("scope", `with (scope) { ${body}\nreturn ${name}; }`)(
    scope,
  );
}

test("the listening placeholder hides after twelve seconds", async (t) => {
  const { captions, nodes } = await setupCaptions(t);
  nodes.captionsText.textContent = "Listening...";
  captions.showCaptions();

  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(11999);
  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(1);
  assert.equal(nodes.captionsBar.hidden, true);
});

test("candidate captions keep their window and renew the idle timeout", async (t) => {
  const { captions, nodes } = await setupCaptions(t);
  const text =
    `${"I am checking the input constraints ".repeat(6)}` +
    "before choosing a data structure. I will use a hash map.";

  captions.updateCaptions("you", text, "you-1");
  assert.equal(nodes.captionsText.textContent, "[You]: I will use a hash map.");
  assert.equal(nodes.captionsBar.hidden, false);

  t.mock.timers.tick(11000);
  assert.equal(nodes.captionsBar.hidden, false);

  captions.updateCaptions("you", "I will scan the array once.", "you-1");
  assert.equal(
    nodes.captionsText.textContent,
    "[You]: I will scan the array once.",
  );

  // The previous update's deadline must not hide the renewed caption.
  t.mock.timers.tick(1000);
  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(10999);
  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(1);
  assert.equal(nodes.captionsBar.hidden, true);
});

test("interim interviewer captions pace cumulative updates", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1");
  assert.equal(nodes.captionsText.textContent, "[Jim]: Use a ha");
  assert.equal(nodes.captionsBar.hidden, false);

  t.mock.timers.tick(300);
  assert.equal(nodes.captionsText.textContent, "[Jim]: Use a hash map.");

  captions.updateCaptions("interviewer", "Use a", "jim-1");
  assert.equal(nodes.captionsText.textContent, "[Jim]: Use a hash map.");

  captions.updateCaptions("interviewer", "Try sorting.", "jim-2");
  assert.equal(nodes.captionsText.textContent, "[Jim]: Try sort");
  t.mock.timers.tick(300);
  assert.equal(nodes.captionsText.textContent, "[Jim]: Try sorting.");
});

test("a finished interviewer turn is shown in full", async (t) => {
  const { captions, nodes } = await setupCaptions(t);
  const text =
    "Start by checking how often each value appears in the input. " +
    "Store counts in a hash map so repeated values are handled consistently. " +
    "Then scan the array once and compare each value with its required complement.";

  assert.ok(text.length > 160);

  captions.updateCaptions("interviewer", text, "jim-1");
  const beforeFinal = nodes.captionsText.textContent;
  captions.updateCaptions("interviewer", text, "jim-1", { final: true });

  assert.equal(nodes.captionsText.textContent, beforeFinal);

  t.mock.timers.tick(300);
  assert.equal(nodes.captionsText.textContent, `[Jim]: ${text.slice(0, 16)}`);

  // Past 160 characters the reveal must keep the opening, not slide a window.
  for (let ticks = 0; ticks < text.length; ticks++) {
    assert.ok(
      nodes.captionsText.textContent.startsWith(`[Jim]: ${text.slice(0, 16)}`),
      `the opening was cut: ${nodes.captionsText.textContent}`,
    );
    if (nodes.captionsText.textContent === `[Jim]: ${text}`) break;
    t.mock.timers.tick(300);
  }
  finishJim(t, nodes, text);

  // Later ticks must neither trim nor hide the retained turn.
  t.mock.timers.tick(300);
  assert.equal(nodes.captionsText.textContent, `[Jim]: ${text}`);
  t.mock.timers.tick(60000);
  assert.equal(nodes.captionsBar.hidden, false);
});

// A bar laid out the way the browser lays out #captions-bar: content one pixel
// per character, a viewport of `clientHeight`, and no scroll position while
// hidden. As in the browser, every change of scrollTop queues one scroll
// event for the next frame, whoever caused it: a write, or the clamp when the
// text gets shorter. `scrollTo` is the candidate, whose event runs at once
// unless queued.
// `subpixel` is how far past, or when negative short of, the whole-pixel
// bottom a zoomed browser lets an overflowing bar scroll, while scrollHeight
// and clientHeight stay whole.
function layoutBar(nodes, subpixel) {
  let top = 0;
  let hidden = true;
  let clientHeight = 100;
  let pending = false;
  let words = "";
  const listeners = [];
  const height = () => (hidden ? 0 : Math.max(clientHeight, words.length));
  const bottom = () =>
    height() > clientHeight ? height() - clientHeight + subpixel : 0;
  const frame = () => {
    if (!pending) return;
    pending = false;
    for (const listener of listeners) listener();
  };
  const move = (value) => {
    const before = top;
    top = hidden ? 0 : Math.min(Math.max(0, value), bottom());
    if (top !== before && !hidden && !pending) {
      pending = true;
      setTimeout(frame, 16);
    }
  };
  nodes.captionsText = {
    get textContent() {
      return words;
    },
    set textContent(value) {
      words = value;
      move(top);
    },
  };
  return {
    get clientHeight() {
      return clientHeight;
    },
    set clientHeight(value) {
      clientHeight = value;
      move(top);
    },
    get scrollHeight() {
      return height();
    },
    get scrollTop() {
      return top;
    },
    set scrollTop(value) {
      move(value);
    },
    get hidden() {
      return hidden;
    },
    set hidden(value) {
      hidden = value;
      if (hidden) {
        top = 0;
        pending = false;
      }
    },
    addEventListener(type, listener) {
      if (type === "scroll") listeners.push(listener);
    },
    // A candidate scroll's event can also wait for the next frame, as it does
    // when the scroll lands just before one of Jim's ticks.
    scrollTo(value, { queued = false } = {}) {
      move(value);
      if (!queued) frame();
    },
  };
}

function atEnd(bar) {
  return bar.scrollTop + bar.clientHeight >= bar.scrollHeight;
}

const LONG_JIM_TURN =
  "Count each value before you pair it with its complement. ".repeat(6);

test("a long interviewer reveal follows its newest words", async (t) => {
  const { captions, nodes } = await setupCaptions(t, { scrollable: true });

  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1", {
    final: true,
  });
  finishJim(t, nodes, LONG_JIM_TURN);

  assert.ok(nodes.captionsBar.scrollTop > 0, "the turn must overflow");
  assert.ok(atEnd(nodes.captionsBar), "the reveal must reach the end");
});

test("a candidate rereading Jim's opening is not pulled down", async (t) => {
  const { captions, nodes } = await setupCaptions(t, { scrollable: true });

  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1", {
    final: true,
  });
  for (let ticks = 0; ticks < 20; ticks++) t.mock.timers.tick(300);
  assert.ok(nodes.captionsBar.scrollTop > 0, "the reveal must overflow first");

  nodes.captionsBar.scrollTo(0);
  finishJim(t, nodes, LONG_JIM_TURN);
  assert.equal(nodes.captionsBar.scrollTop, 0);
});

test("a candidate scrolling back to the end resumes the follow", async (t) => {
  const { captions, nodes } = await setupCaptions(t, { scrollable: true });
  const bar = nodes.captionsBar;

  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1", {
    final: true,
  });
  for (let ticks = 0; ticks < 20; ticks++) t.mock.timers.tick(300);
  bar.scrollTo(0);
  t.mock.timers.tick(300);
  bar.scrollTo(bar.scrollHeight);

  finishJim(t, nodes, LONG_JIM_TURN);
  assert.ok(atEnd(bar), "the reveal must follow again from the end");
});

test("scrolling up and back down between ticks resumes the follow", async (t) => {
  const { captions, nodes } = await setupCaptions(t, { scrollable: true });
  const bar = nodes.captionsBar;
  const opening = LONG_JIM_TURN.slice(0, 200);

  // Caught up and waiting for more, so no tick runs between the two scrolls.
  captions.updateCaptions("interviewer", opening, "jim-1");
  finishJim(t, nodes, opening);
  t.mock.timers.tick(16);
  const bottom = bar.scrollTop;
  bar.scrollTo(0);
  bar.scrollTo(bar.scrollHeight);
  assert.equal(bar.scrollTop, bottom, "back exactly where the tick left it");

  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1", {
    final: true,
  });
  finishJim(t, nodes, LONG_JIM_TURN);
  assert.ok(atEnd(bar), "the growth after scrolling back must be followed");
});

test("a taller bar does not restart a follow the candidate stopped", async (t) => {
  const { captions, nodes } = await setupCaptions(t, { scrollable: true });
  const bar = nodes.captionsBar;
  const opening = LONG_JIM_TURN.slice(0, 200);

  captions.updateCaptions("interviewer", opening, "jim-1");
  finishJim(t, nodes, opening);
  t.mock.timers.tick(16);
  bar.scrollTo(bar.scrollTop - 10);

  // A resize, or the compact card's cap lifting, clamps the bar to its new
  // bottom without the candidate scrolling.
  bar.clientHeight = 130;
  t.mock.timers.tick(16);
  assert.ok(atEnd(bar), "the clamp must land the bar on its new bottom");
  const clamped = bar.scrollTop;

  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1", {
    final: true,
  });
  finishJim(t, nodes, LONG_JIM_TURN);
  assert.equal(bar.scrollTop, clamped, "the reveal must not pull them down");
});

test("a taller zoomed bar does not restart a follow the candidate stopped", async (t) => {
  const { captions, nodes } = await setupCaptions(t, {
    scrollable: true,
    subpixel: 0.375,
  });
  const bar = nodes.captionsBar;
  const opening = LONG_JIM_TURN.slice(0, 200);

  captions.updateCaptions("interviewer", opening, "jim-1");
  finishJim(t, nodes, opening);
  t.mock.timers.tick(16);
  bar.scrollTo(bar.scrollTop - 10);

  bar.clientHeight = 130;
  t.mock.timers.tick(16);
  const clamped = bar.scrollTop;
  assert.notEqual(clamped, Math.trunc(clamped), "the clamp must be fractional");

  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1", {
    final: true,
  });
  finishJim(t, nodes, LONG_JIM_TURN);
  assert.equal(bar.scrollTop, clamped, "the reveal must not pull them down");
});

test("a zoomed bar that stops short of its end still resumes the follow", async (t) => {
  const { captions, nodes } = await setupCaptions(t, {
    scrollable: true,
    subpixel: -0.375,
  });
  const bar = nodes.captionsBar;
  const opening = LONG_JIM_TURN.slice(0, 200);
  const short = () => bar.scrollHeight - bar.clientHeight - bar.scrollTop;

  captions.updateCaptions("interviewer", opening, "jim-1");
  finishJim(t, nodes, opening);
  t.mock.timers.tick(16);
  bar.scrollTo(0);
  bar.scrollTo(bar.scrollHeight);
  assert.ok(short() > 0, "the bar must stop short of its whole-pixel end");

  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1", {
    final: true,
  });
  finishJim(t, nodes, LONG_JIM_TURN);
  assert.ok(short() < 1, "the growth after scrolling back must be followed");
});

test("a scroll of the last turn still queued does not stop the next one", async (t) => {
  const { captions, nodes } = await setupCaptions(t, { scrollable: true });
  const bar = nodes.captionsBar;
  const next = "Then scan the array and check the boundary cases. ".repeat(6);

  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1", {
    final: true,
  });
  finishJim(t, nodes, LONG_JIM_TURN);
  t.mock.timers.tick(16);
  // The next turn arrives before the frame that would dispatch this scroll.
  bar.scrollTo(bar.scrollTop / 2, { queued: true });

  captions.updateCaptions("interviewer", next, "jim-2", { final: true });
  finishJim(t, nodes, next);
  assert.ok(atEnd(bar), "the next turn must follow from its first tick");
});

test("a caption bar that shrinks mid reveal keeps following", async (t) => {
  const { captions, nodes } = await setupCaptions(t, { scrollable: true });
  const bar = nodes.captionsBar;

  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1", {
    final: true,
  });
  for (let ticks = 0; ticks < 20; ticks++) t.mock.timers.tick(300);
  assert.ok(bar.scrollTop > 0, "the reveal must overflow first");

  // The ending wait cuts the bar to 4.5rem without the candidate scrolling,
  // before the event the last follow queued has run.
  bar.clientHeight = 30;
  assert.ok(!atEnd(bar), "the shrink must leave the bar short of its end");
  t.mock.timers.tick(16);

  finishJim(t, nodes, LONG_JIM_TURN);
  assert.ok(atEnd(bar), "the newest words must stay in view");
});

test("a live turn that grows after its bar idled out still follows", async (t) => {
  const { captions, nodes } = await setupCaptions(t, { scrollable: true });
  const bar = nodes.captionsBar;
  const opening = LONG_JIM_TURN.slice(0, 200);

  captions.updateCaptions("interviewer", opening, "jim-1");
  finishJim(t, nodes, opening);
  t.mock.timers.tick(12000);
  assert.equal(bar.hidden, true, "a live turn must idle out");

  // The tick that reopens the bar already reveals past what it can hold.
  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1", {
    final: true,
  });
  assert.equal(bar.hidden, false);
  assert.ok(atEnd(bar), "the tick that reopens the bar must follow");
  finishJim(t, nodes, LONG_JIM_TURN);
  assert.ok(atEnd(bar), "the reveal must follow once the bar is shown");
});

test("a candidate who stopped the follow keeps it stopped past an idle bar", async (t) => {
  const { captions, nodes } = await setupCaptions(t, { scrollable: true });
  const bar = nodes.captionsBar;
  const opening = LONG_JIM_TURN.slice(0, 200);

  captions.updateCaptions("interviewer", opening, "jim-1");
  finishJim(t, nodes, opening);
  bar.scrollTo(0);
  t.mock.timers.tick(12000);
  assert.equal(bar.hidden, true, "a live turn must idle out");

  // Hiding drops the position, which the candidate did not move.
  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1", {
    final: true,
  });
  finishJim(t, nodes, LONG_JIM_TURN);
  assert.equal(bar.scrollTop, 0, "the reopened turn must not follow");
});

test("a new Jim turn follows after the last one was left mid scroll", async (t) => {
  const { captions, nodes } = await setupCaptions(t, { scrollable: true });
  const bar = nodes.captionsBar;
  const next = "Then scan the array and check the boundary cases. ".repeat(6);

  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1", {
    final: true,
  });
  finishJim(t, nodes, LONG_JIM_TURN);
  bar.scrollTo(bar.scrollTop / 2);

  captions.updateCaptions("interviewer", next, "jim-2", { final: true });
  finishJim(t, nodes, next);
  assert.ok(atEnd(bar), "the next turn must follow its own reveal");
});

test("a candidate caption does not restart a follow the candidate stopped", async (t) => {
  const { captions, nodes } = await setupCaptions(t, { scrollable: true });
  const bar = nodes.captionsBar;

  // The candidate is speaking when Jim's live turn starts, so a longer copy of
  // their segment pauses Jim instead of retiring him.
  captions.updateCaptions("you", "I will", "you-1");
  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1");
  for (let ticks = 0; ticks < 30; ticks++) t.mock.timers.tick(300);
  bar.scrollTo(bar.scrollTop / 2);
  assert.ok(bar.scrollTop > 0, "the candidate must stop midway");

  // Their short caption clamps the bar to its top, which is also its end.
  captions.updateCaptions("you", "I will count each value first.", "you-1");
  t.mock.timers.tick(16);

  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1");
  t.mock.timers.tick(300);
  assert.ok(!atEnd(bar), "the resumed reveal must not jump to the end");
});

test("a candidate scroll queued behind Jim's tick still stops the follow", async (t) => {
  const { captions, nodes } = await setupCaptions(t, { scrollable: true });
  const bar = nodes.captionsBar;

  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1", {
    final: true,
  });
  for (let ticks = 0; ticks < 20; ticks++) t.mock.timers.tick(300);
  // Let the last follow's own event run, then scroll a moment before the next
  // tick, so the tick runs while the candidate's event is still queued.
  t.mock.timers.tick(16);
  t.mock.timers.tick(283);
  bar.scrollTo(0, { queued: true });

  finishJim(t, nodes, LONG_JIM_TURN);
  assert.equal(bar.scrollTop, 0, "the tick must not write over the scroll");
});

test("scrolling the candidate's own caption leaves Jim's follow alone", async (t) => {
  const { captions, nodes } = await setupCaptions(t, { scrollable: true });
  const bar = nodes.captionsBar;
  const reply = "I will count each value first and then pair them. ".repeat(2);

  captions.updateCaptions("you", "I will", "you-1");
  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1");
  for (let ticks = 0; ticks < 30; ticks++) t.mock.timers.tick(300);

  // In the ending wait's short bar the candidate's caption overflows too, and
  // they scroll it, not Jim's turn.
  bar.clientHeight = 30;
  captions.updateCaptions("you", reply, "you-1");
  t.mock.timers.tick(16);
  bar.scrollTo(0);

  captions.updateCaptions("interviewer", LONG_JIM_TURN, "jim-1");
  finishJim(t, nodes, LONG_JIM_TURN);
  assert.ok(atEnd(bar), "Jim's resumed reveal must still follow");
});

test("a finished interviewer turn outlives idle timers", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.showCaptions();
  t.mock.timers.tick(11000);

  captions.updateCaptions("interviewer", "Think.", "jim-1", {
    final: true,
  });

  // The placeholder's old deadline must not hide the completed turn.
  t.mock.timers.tick(1000);
  assert.equal(nodes.captionsBar.hidden, false);

  t.mock.timers.tick(60000);
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(nodes.captionsText.textContent, "[Jim]: Think.");
});

test("candidate speech replaces Jim without late revival", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });
  finishJim(t, nodes, "Use a hash map.");
  captions.updateCaptions("you", "I will count the values.", "you-1");

  assert.equal(
    nodes.captionsText.textContent,
    "[You]: I will count the values.",
  );

  captions.dismissCaptions();
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(
    nodes.captionsText.textContent,
    "[You]: I will count the values.",
  );

  t.mock.timers.tick(1000);

  for (const final of [false, true]) {
    captions.updateCaptions(
      "interviewer",
      "Use a hash map and count each value.",
      "jim-1",
      { final },
    );
    assert.equal(
      nodes.captionsText.textContent,
      "[You]: I will count the values.",
    );
    assert.equal(nodes.captionsBar.hidden, false);
  }

  t.mock.timers.tick(10999);
  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(1);
  assert.equal(nodes.captionsBar.hidden, true);
});

for (const final of [false, true]) {
  test(`a code edit during ${final ? "a final" : "an interim"} Jim reveal is remembered`, async (t) => {
    const { captions, nodes } = await setupCaptions(t);
    const text = "Use a hash map and count each value.";

    captions.updateCaptions("interviewer", text, "jim-1", { final });
    const beforeEdit = nodes.captionsText.textContent;

    captions.dismissCaptions();
    assert.equal(nodes.captionsBar.hidden, false);
    assert.equal(nodes.captionsText.textContent, beforeEdit);

    t.mock.timers.tick(300);
    assert.equal(nodes.captionsBar.hidden, false);
    assert.notEqual(nodes.captionsText.textContent, beforeEdit);

    captions.updateCaptions("interviewer", text, "jim-1", {
      final: true,
    });
    assert.equal(nodes.captionsBar.hidden, false);

    finishDismissal(t, nodes, text);

    for (const lateFinal of [false, true]) {
      captions.updateCaptions(
        "interviewer",
        `${text} Then scan the array.`,
        "jim-1",
        { final: lateFinal },
      );
      assert.equal(nodes.captionsBar.hidden, true);
    }

    t.mock.timers.tick(60000);
    assert.equal(nodes.captionsBar.hidden, true);
  });
}

test("a dismissed completed Jim turn stays hidden", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });
  finishJim(t, nodes, "Use a hash map.");
  captions.dismissCaptions();
  assert.equal(nodes.captionsBar.hidden, true);

  t.mock.timers.tick(300);
  assert.equal(nodes.captionsBar.hidden, true);

  for (const lateFinal of [false, true]) {
    captions.updateCaptions(
      "interviewer",
      "Use a hash map and count each value.",
      "jim-1",
      { final: lateFinal },
    );
    assert.equal(nodes.captionsBar.hidden, true);
  }

  captions.updateCaptions("interviewer", "Try sorting.", "jim-2", {
    final: true,
  });
  finishJim(t, nodes, "Try sorting.");
  assert.equal(nodes.captionsText.textContent, "[Jim]: Try sorting.");
  assert.equal(nodes.captionsBar.hidden, false);

  t.mock.timers.tick(60000);
  assert.equal(nodes.captionsBar.hidden, false);
});

test("a new Jim turn replaces the previous turn despite late updates", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });
  captions.updateCaptions("interviewer", "Try sorting.", "jim-2");
  assert.equal(nodes.captionsText.textContent, "[Jim]: Try sort");

  t.mock.timers.tick(300);
  captions.updateCaptions("interviewer", "Try sorting.", "jim-2", {
    final: true,
  });

  captions.updateCaptions(
    "interviewer",
    "Use a hash map and count each value.",
    "jim-1",
    { final: true },
  );
  assert.equal(nodes.captionsText.textContent, "[Jim]: Try sorting.");

  t.mock.timers.tick(60000);
  assert.equal(nodes.captionsBar.hidden, false);
});

for (const { name, final, broken } of [
  {
    name: "a complete final stream retains the caption",
    final: true,
    broken: false,
  },
  {
    name: "an interrupted final stream keeps partial captions temporary",
    final: true,
    broken: true,
  },
  {
    name: "an interim stream keeps captions temporary",
    final: false,
    broken: false,
  },
]) {
  test(name, async (t) => {
    const { captions, nodes } = await setupCaptions(t);
    const updates = [];
    const consume = loadInterviewFunction("consumeTranscript", {
      isCurrentAgent: () => true,
      responseWindowIndex: () => 0,
      updateTranscriptSegment: () => {},
      recordReplay: () => {},
      updateCaptions(speaker, text, id, options = {}) {
        updates.push(options.final === true);
        captions.updateCaptions(speaker, text, id, options);
      },
    });

    async function* chunks() {
      yield "Think";
      if (broken) throw new Error("stream interrupted");
      yield ".";
    }

    const reader = chunks();
    reader.info = {
      attributes: {
        "lk.segment_id": "jim-1",
        "lk.transcription_final": String(final),
      },
    };

    await consume({ localParticipant: { identity: "candidate" } }, reader, {
      identity: "jim",
    });

    assert.deepEqual(updates, broken ? [false, false] : [false, false, final]);
    assert.equal(
      nodes.captionsText.textContent,
      broken ? "[Jim]: Think" : "[Jim]: Think.",
    );

    t.mock.timers.tick(12000);
    assert.equal(nodes.captionsBar.hidden, broken || !final);
  });
}

test("only changed editor text dismisses Jim", async (t) => {
  const { captions, nodes } = await setupCaptions(t);
  const events = [];
  let input;
  nodes.editor = {
    value: "",
    addEventListener(type, handler) {
      assert.equal(type, "input");
      input = handler;
    },
  };
  const state = {
    language: "javascript",
    codeByLanguage: { javascript: "" },
  };

  const binding =
    functionBody(read("web/interview.js"), "workspaceEdited") + "\n}";
  input = new Function("scope", `with (scope) { return (${binding}); }`)({
    nodes,
    state,
    dismissCaptions: captions.dismissCaptions,
    // History is covered separately; this fixture isolates input side effects.
    replayingEditorHistory: true,
    drawingTurn: null,
    flushPendingLanguagePublish: () => events.push("language"),
    scheduleEditorPaint: () => events.push("paint"),
    flushPendingEditorPublish: () => events.push("publish"),
    codePublishTimer: null,
    CODE_PUBLISH_DEBOUNCE_MS: 300,
    setTimeout,
    clearTimeout,
  });

  captions.updateCaptions("interviewer", "Think.", "jim-1", {
    final: true,
  });
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(typeof input, "function");

  input();
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(state.codeByLanguage.javascript, "");
  assert.deepEqual(events, ["language", "paint"]);
  t.mock.timers.tick(300);
  assert.deepEqual(events, ["language", "paint", "publish"]);
  events.length = 0;

  nodes.editor.value = "const answer = 42;";
  input();
  assert.equal(nodes.captionsBar.hidden, true);
  assert.equal(state.codeByLanguage.javascript, "const answer = 42;");
  assert.deepEqual(events, ["language", "paint"]);

  t.mock.timers.tick(299);
  assert.deepEqual(events, ["language", "paint"]);
  t.mock.timers.tick(1);
  assert.deepEqual(events, ["language", "paint", "publish"]);

  captions.updateCaptions("interviewer", "Think about the input.", "jim-1", {
    final: true,
  });
  assert.equal(nodes.captionsBar.hidden, true);
  captions.updateCaptions("interviewer", "Try another approach.", "jim-2", {
    final: true,
  });
  finishJim(t, nodes, "Try another approach.");
  input();
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(nodes.captionsText.textContent, "[Jim]: Try another approach.");
  t.mock.timers.tick(12000);
  assert.equal(nodes.captionsBar.hidden, false);
});

test("a candidate final replay does not dismiss Jim", async (t) => {
  const { captions, nodes } = await setupCaptions(t);
  const jimText = "Use a hash map and count each value.";

  captions.updateCaptions("you", "I will", "you-1");
  captions.updateCaptions("interviewer", jimText, "jim-1");
  const interimJimText = nodes.captionsText.textContent;

  const consume = loadInterviewFunction("consumeTranscript", {
    isCurrentAgent: () => true,
    responseWindowIndex: () => 0,
    updateTranscriptSegment: () => {},
    recordReplay: () => {},
    updateCaptions: captions.updateCaptions,
  });

  async function* chunks() {
    yield "I will count";
    yield " the values.";
  }

  const reader = chunks();
  reader.info = {
    attributes: {
      "lk.segment_id": "you-1",
      "lk.transcription_final": "true",
    },
  };

  await consume({ localParticipant: { identity: "candidate" } }, reader, {
    identity: "candidate",
  });

  assert.equal(nodes.captionsText.textContent, interimJimText);

  captions.updateCaptions("interviewer", jimText, "jim-1", {
    final: true,
  });
  finishJim(t, nodes, jimText);
  assert.equal(nodes.captionsText.textContent, `[Jim]: ${jimText}`);

  for (const final of [false, true]) {
    captions.updateCaptions(
      "you",
      "I will count the values and check their complements.",
      "you-1",
      { final },
    );
    assert.equal(nodes.captionsText.textContent, `[Jim]: ${jimText}`);
  }

  t.mock.timers.tick(12000);
  assert.equal(nodes.captionsBar.hidden, false);

  captions.updateCaptions("you", "I will compare complements.", "you-2");
  assert.equal(
    nodes.captionsText.textContent,
    "[You]: I will compare complements.",
  );
  t.mock.timers.tick(11999);
  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(1);
  assert.equal(nodes.captionsBar.hidden, true);
});

test("candidate speech pauses a live Jim turn without retiring it", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions("you", "I will", "you-1");
  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1");
  captions.updateCaptions("you", "I will count the values.", "you-1");

  assert.equal(
    nodes.captionsText.textContent,
    "[You]: I will count the values.",
  );

  t.mock.timers.tick(300);
  assert.equal(
    nodes.captionsText.textContent,
    "[You]: I will count the values.",
  );

  const text = "Use a hash map and count each value.";
  captions.updateCaptions("interviewer", text, "jim-1", {
    final: true,
  });
  finishJim(t, nodes, text);

  t.mock.timers.tick(12000);
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(nodes.captionsText.textContent, `[Jim]: ${text}`);
});

test("a final-only Jim turn still reveals gradually", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });

  assert.equal(nodes.captionsText.textContent, "[Jim]: Use a ha");
  assert.equal(nodes.captionsBar.hidden, false);

  t.mock.timers.tick(300);
  assert.equal(nodes.captionsText.textContent, "[Jim]: Use a hash map.");

  t.mock.timers.tick(12000);
  assert.equal(nodes.captionsBar.hidden, false);
});

for (const candidateId of ["you-1", null]) {
  test(`candidate speech retires a revealing final Jim turn (${candidateId ?? "no ID"})`, async (t) => {
    const { captions, nodes } = await setupCaptions(t);
    const text = "Use a hash map and count each value.";
    const candidateText = "I will count the values.";

    captions.updateCaptions("interviewer", text, "jim-1", {
      final: true,
    });
    assert.notEqual(nodes.captionsText.textContent, `[Jim]: ${text}`);

    captions.updateCaptions("you", candidateText, candidateId);
    assert.equal(nodes.captionsText.textContent, `[You]: ${candidateText}`);

    t.mock.timers.tick(300);
    assert.equal(nodes.captionsText.textContent, `[You]: ${candidateText}`);

    for (const final of [false, true]) {
      captions.updateCaptions(
        "interviewer",
        `${text} Then scan the array.`,
        "jim-1",
        { final },
      );
      assert.equal(nodes.captionsText.textContent, `[You]: ${candidateText}`);
      assert.equal(nodes.captionsBar.hidden, false);
    }

    t.mock.timers.tick(11699);
    assert.equal(nodes.captionsBar.hidden, false);
    t.mock.timers.tick(1);
    assert.equal(nodes.captionsBar.hidden, true);
  });
}

test("a new candidate segment retires a live Jim turn", async (t) => {
  const { captions, nodes } = await setupCaptions(t);
  const jimText = "Use a hash map and count each value.";
  const candidateText = "Now I will compare complements.";

  captions.updateCaptions("you", "I will count the values.", "you-1");
  captions.updateCaptions("interviewer", jimText, "jim-1");
  captions.updateCaptions("you", candidateText, "you-2");

  assert.equal(nodes.captionsText.textContent, `[You]: ${candidateText}`);

  t.mock.timers.tick(300);
  assert.equal(nodes.captionsText.textContent, `[You]: ${candidateText}`);

  for (const final of [false, true]) {
    captions.updateCaptions(
      "interviewer",
      `${jimText} Then scan the array.`,
      "jim-1",
      { final },
    );
    assert.equal(nodes.captionsText.textContent, `[You]: ${candidateText}`);
    assert.equal(nodes.captionsBar.hidden, false);
  }

  t.mock.timers.tick(11699);
  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(1);
  assert.equal(nodes.captionsBar.hidden, true);
});

test("a pending code edit does not dismiss the next Jim turn", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions(
    "interviewer",
    "Use a hash map and count each value.",
    "jim-1",
  );
  captions.dismissCaptions();

  captions.updateCaptions("interviewer", "Try sorting.", "jim-2", {
    final: true,
  });
  finishJim(t, nodes, "Try sorting.");

  t.mock.timers.tick(13000);
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(nodes.captionsText.textContent, "[Jim]: Try sorting.");

  for (const final of [false, true]) {
    captions.updateCaptions(
      "interviewer",
      "Use a hash map and count each value. Then scan the array.",
      "jim-1",
      { final },
    );
    assert.equal(nodes.captionsBar.hidden, false);
    assert.equal(nodes.captionsText.textContent, "[Jim]: Try sorting.");
  }
});

test("a pending code edit preserves the candidate idle deadline", async (t) => {
  const { captions, nodes } = await setupCaptions(t);
  const jimText = "Use a hash map and count each value.";
  const candidateText = "I will count the values.";

  captions.updateCaptions("you", "I will", "you-1");
  captions.updateCaptions("interviewer", jimText, "jim-1");
  captions.updateCaptions("you", candidateText, "you-1");

  t.mock.timers.tick(11000);
  captions.dismissCaptions();
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(nodes.captionsText.textContent, `[You]: ${candidateText}`);

  t.mock.timers.tick(999);
  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(1);
  assert.equal(nodes.captionsBar.hidden, true);

  captions.updateCaptions("interviewer", jimText, "jim-1", {
    final: true,
  });
  assert.equal(nodes.captionsBar.hidden, false);

  finishDismissal(t, nodes, jimText);
  t.mock.timers.tick(60000);
  assert.equal(nodes.captionsBar.hidden, true);
});

function fakeUnclearButton() {
  return {
    hidden: true,
    disabled: false,
    title: "",
    attributes: {},
    click: null,
    setAttribute(name, value) {
      this.attributes[name] = value;
    },
    addEventListener(type, handler) {
      if (type === "click") this.click = handler;
    },
  };
}

async function setupUnclear(t) {
  const captions = await import(
    `../../web/captions.js?copy=${++captionCopies}`
  );
  const button = fakeUnclearButton();
  const nodes = {
    captionsText: { textContent: "" },
    captionsBar: { hidden: true },
    captionUnclear: button,
  };
  captions.initCaptions({ nodes });
  t.mock.timers.enable({ apis: ["setTimeout"] });
  return { captions, button, nodes };
}

test("the Unclear button marks and unmarks the current Jim turn", async (t) => {
  const { captions, button } = await setupUnclear(t);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });
  assert.equal(button.hidden, false);

  button.click();
  assert.equal(button.attributes["aria-pressed"], "true");
  assert.deepEqual(captions.unclearLineIds(), ["jim-1"]);

  button.click();
  assert.equal(button.attributes["aria-pressed"], "false");
  assert.deepEqual(captions.unclearLineIds(), []);
});

test("candidate speech hides the Unclear button and a new Jim turn starts unmarked", async (t) => {
  const { captions, button } = await setupUnclear(t);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });
  button.click();
  captions.updateCaptions("you", "Okay.", "you-1");
  assert.equal(button.hidden, true);

  captions.updateCaptions("interviewer", "Try sorting.", "jim-2");
  assert.equal(button.hidden, false);
  assert.equal(button.attributes["aria-pressed"], "false");
  assert.deepEqual(captions.unclearLineIds(), ["jim-1"]);
});

test("the Unclear button stops marking at the cap", async (t) => {
  const { captions, button } = await setupUnclear(t);

  for (let turn = 1; turn <= MAX_UNCLEAR_LINES; turn++) {
    captions.updateCaptions("interviewer", `Line ${turn}.`, `jim-${turn}`, {
      final: true,
    });
    button.click();
  }
  assert.equal(captions.unclearLineIds().length, MAX_UNCLEAR_LINES);

  captions.updateCaptions("interviewer", "One more line.", "jim-extra", {
    final: true,
  });
  assert.equal(button.attributes["aria-disabled"], "true");
  button.click();
  assert.equal(captions.unclearLineIds().length, MAX_UNCLEAR_LINES);
  assert.ok(!captions.unclearLineIds().includes("jim-extra"));
});

function focusOn(button, nodes, editor = {}) {
  const document = { activeElement: button };
  button.ownerDocument = document;
  const target = (name, options = {}) => ({
    isConnected: true,
    disabled: false,
    ...options,
    focus() {
      document.activeElement = nodes[name];
    },
  });
  nodes.editor = target("editor", editor);
  nodes.board = target("board");
  return document;
}

test("candidate speech moves focus off the Unclear button it hides", async (t) => {
  const { captions, button, nodes } = await setupUnclear(t);
  const document = focusOn(button, nodes);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });
  button.click();
  assert.equal(document.activeElement, button);

  captions.updateCaptions("you", "Okay.", "you-1");
  assert.equal(button.hidden, true);
  assert.equal(document.activeElement, nodes.editor);
});

test("the cap leaves focus on the Unclear button it marks unavailable", async (t) => {
  const { captions, button, nodes } = await setupUnclear(t);
  const document = focusOn(button, nodes);

  for (let turn = 1; turn <= MAX_UNCLEAR_LINES; turn++) {
    captions.updateCaptions("interviewer", `Line ${turn}.`, `jim-${turn}`, {
      final: true,
    });
    button.click();
  }
  assert.equal(document.activeElement, button);

  captions.updateCaptions("interviewer", "One more line.", "jim-extra", {
    final: true,
  });
  assert.equal(button.attributes["aria-disabled"], "true");
  assert.equal(document.activeElement, button);
});

test("the idle timeout moves focus off the Unclear button it hides", async (t) => {
  const { captions, button, nodes } = await setupUnclear(t);
  const document = focusOn(button, nodes);

  // A Jim turn that never turns final keeps the idle timeout armed.
  captions.updateCaptions("interviewer", "Go on.", "jim-1");
  t.mock.timers.tick(12000);
  assert.equal(nodes.captionsBar.hidden, true);
  assert.equal(document.activeElement, nodes.editor);
});

test("a code edit moves focus off the Unclear button it hides", async (t) => {
  const { captions, button, nodes } = await setupUnclear(t);
  const document = focusOn(button, nodes);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });
  t.mock.timers.tick(1000);
  captions.dismissCaptions();
  assert.equal(nodes.captionsBar.hidden, true);
  assert.equal(document.activeElement, nodes.editor);
});

for (const [name, editor] of [
  ["disabled", { disabled: true }],
  ["removed", { isConnected: false }],
]) {
  test(`focus skips a ${name} editor for the board`, async (t) => {
    const { captions, button, nodes } = await setupUnclear(t);
    const document = focusOn(button, nodes, editor);

    captions.updateCaptions("interviewer", "Go on.", "jim-1", { final: true });
    captions.updateCaptions("you", "Okay.", "you-1");
    assert.equal(document.activeElement, nodes.board);
  });
}

test("a mouse click on Unclear hands focus back to the editor", async (t) => {
  const { captions, button, nodes } = await setupUnclear(t);
  const document = focusOn(button, nodes);

  captions.updateCaptions("interviewer", "Go on.", "jim-1", { final: true });
  button.click({ detail: 1 });
  assert.equal(button.attributes["aria-pressed"], "true");
  assert.equal(document.activeElement, nodes.editor);
});

test("Space or Enter on Unclear keeps focus on the button", async (t) => {
  const { captions, button, nodes } = await setupUnclear(t);
  const document = focusOn(button, nodes);

  captions.updateCaptions("interviewer", "Go on.", "jim-1", { final: true });
  button.click({ detail: 0 });
  assert.equal(button.attributes["aria-pressed"], "true");
  assert.equal(document.activeElement, button);
});
