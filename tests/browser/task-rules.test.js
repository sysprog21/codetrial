import test from "node:test";
import assert from "node:assert/strict";
import { lend, st } from "../../web/task-state.js";
import { closeFullscreenAway, watchRules } from "../../web/task-rules.js";

// task-rules.js reaches the page through `host`, so a node test imports it and
// lends it fakes. The DOM is a map of plain objects and the document a plain
// event target whose fullscreen element the test sets.
const elements = {};
const el = (id) =>
  (elements[id] ??= {
    hidden: true,
    textContent: "",
    onclick: null,
    disabled: false,
    contains: () => false,
    focus() {
      document.activeElement = this;
    },
  });
const listeners = {};
globalThis.document = {
  getElementById: el,
  addEventListener: (type, listener) => (listeners[type] = listener),
  fullscreenElement: {},
  hidden: false,
  activeElement: null,
  exitFullscreen: async () => {
    exits += 1;
    document.fullscreenElement = null;
  },
};
// Node 21 and later have a navigator; an older one gets an empty stand-in, as
// a browser without Keyboard Lock would offer.
globalThis.navigator ??= {};
let ended;
let exits = 0;
lend({
  endAttempt:
    () =>
    (...args) =>
      ended.push(args),
});
watchRules();

function setUp(fullscreenGraceSeconds = 5) {
  ended = [];
  closeFullscreenAway();
  Object.assign(st, {
    attempt: "running",
    state: { phase: "work" },
    task: { rules: { fullscreenGraceSeconds }, fullscreenReturns: 3 },
    fullscreenReturnsUsed: 0,
    startViolation: null,
  });
  document.fullscreenElement = {};
}

const leave = () => {
  document.fullscreenElement = null;
  listeners.fullscreenchange();
};
const back = () => {
  document.fullscreenElement = {};
  listeners.fullscreenchange();
};

test("leaving fullscreen opens the grace period and returning closes it", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval"] });
  setUp();
  leave();
  assert.equal(elements["fullscreen-return"].hidden, false);
  assert.match(elements["fullscreen-return-text"].textContent, /within 5/);
  back();
  assert.equal(elements["fullscreen-return"].hidden, true);
  assert.equal(st.fullscreenReturnsUsed, 1);
  t.mock.timers.tick(10000);
  assert.deepEqual(ended, [], "the closed grace period ends nothing");
});

test("a grace period that runs out ends the attempt as invalid", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval"] });
  setUp();
  let now = 1000;
  t.mock.method(performance, "now", () => now);
  leave();
  now += 4999;
  t.mock.timers.tick(4999);
  assert.deepEqual(ended, []);
  now += 1;
  t.mock.timers.tick(1);
  assert.deepEqual(
    ended,
    [["invalid", "fullscreen", 5000]],
    "the record says how long the learner was out of fullscreen",
  );
  assert.equal(elements["fullscreen-return"].hidden, true);
});

test("no grace is left after the last return, or in a set without one", () => {
  setUp();
  st.fullscreenReturnsUsed = 3;
  leave();
  assert.deepEqual(ended, [["invalid", "fullscreen", 0]]);
  setUp(0);
  leave();
  assert.deepEqual(ended, [["invalid", "fullscreen", 0]]);
});

test("leaving while Start is acknowledged is kept as the start violation", () => {
  setUp();
  st.attempt = "starting";
  leave();
  assert.deepEqual(ended, [["invalid", "fullscreen", 0]]);
  assert.equal(st.startViolation, "fullscreen");
  assert.equal(elements["fullscreen-return"].hidden, true, "no grace yet");
});

test("hiding the page ends the attempt at once", () => {
  setUp();
  document.hidden = true;
  listeners.visibilitychange();
  document.hidden = false;
  assert.deepEqual(ended, [["invalid", "visibility", 0]]);
});

test("the countdown is rewritten only when its number changes", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval"] });
  setUp();
  let now = 1000;
  t.mock.method(performance, "now", () => now);
  leave();
  const text = elements["fullscreen-return-text"];
  text.textContent = "unchanged";
  now += 250;
  t.mock.timers.tick(250);
  assert.equal(text.textContent, "unchanged", "same second, no rewrite");
  now += 750;
  t.mock.timers.tick(750);
  assert.match(text.textContent, /within 4/);
});

test("returning moves focus from the hidden button to the editor", () => {
  setUp();
  const panel = el("fullscreen-return");
  const button = el("fullscreen-return-button");
  panel.contains = (node) => node === button;
  leave();
  document.activeElement = button;
  back();
  assert.equal(document.activeElement, el("code"));
  leave();
  document.activeElement = button;
  el("code").disabled = true;
  closeFullscreenAway();
  assert.equal(document.activeElement, button, "a disabled editor is left be");
  el("code").disabled = false;
});

test("a return granted after the grace ran out leaves fullscreen again", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval"] });
  setUp();
  let grant;
  el("workspace").requestFullscreen = () =>
    new Promise((resolve) => (grant = resolve));
  leave();
  el("fullscreen-return-button").onclick();
  t.mock.timers.tick(5000);
  assert.equal(ended.length, 1, "the grace ran out first");
  // What endAttempt in task.js leaves behind, which the fake does not.
  st.attempt = "ending";
  exits = 0;
  document.fullscreenElement = {};
  grant();
  listeners.fullscreenchange();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(exits, 1, "the finished page does not stay in fullscreen");
  assert.equal(ended.length, 1, "and nothing ends twice");
  delete el("workspace").requestFullscreen;
});

test("a return granted in time keeps fullscreen", async () => {
  setUp();
  let grant;
  el("workspace").requestFullscreen = () =>
    new Promise((resolve) => (grant = resolve));
  leave();
  el("fullscreen-return-button").onclick();
  exits = 0;
  document.fullscreenElement = {};
  grant();
  listeners.fullscreenchange();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(exits, 0);
  assert.equal(st.fullscreenReturnsUsed, 1);
  delete el("workspace").requestFullscreen;
});
