import { test } from "node:test";
import assert from "node:assert/strict";

import { atScrollEnd, scrolledByCandidate } from "../../web/scroll-follow.js";

const box = (scrollTop, scrollHeight, clientHeight) => ({
  scrollTop,
  scrollHeight,
  clientHeight,
});

test("a box at its end, or within the zoom slack of it, is at the end", () => {
  assert.equal(atScrollEnd(box(300, 500, 200)), true);
  assert.equal(atScrollEnd(box(298.4, 500, 200)), true);
  assert.equal(atScrollEnd(box(298, 500, 200)), true, "exactly the slack");
  assert.equal(atScrollEnd(box(297, 500, 200)), false);
  assert.equal(atScrollEnd(box(0, 100, 200)), true, "nothing to scroll");
});

test("only a move the clamp cannot explain is the candidate's", () => {
  // Resting where it was last seen, or within the slack of it.
  assert.equal(scrolledByCandidate(box(120, 500, 200), 120), false);
  assert.equal(scrolledByCandidate(box(121.5, 500, 200), 120), false);
  assert.equal(scrolledByCandidate(box(122, 500, 200), 120), false);
  assert.equal(scrolledByCandidate(box(123, 500, 200), 120), true);
  // Content shorter than the box leaves no range to scroll.
  assert.equal(scrolledByCandidate(box(0, 100, 200), 50), false);
  // The range shrank under a box that sat lower: the browser clamped it.
  assert.equal(scrolledByCandidate(box(100, 300, 200), 250), false);
  // A hidden box has no range and sits at the top.
  assert.equal(scrolledByCandidate(box(0, 0, 0), 250), false);
  // Moved up, or down, from where it was last seen.
  assert.equal(scrolledByCandidate(box(40, 500, 200), 120), true);
  assert.equal(scrolledByCandidate(box(300, 500, 200), 120), true);
});
