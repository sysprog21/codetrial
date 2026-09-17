// Run with: node --test tests/browser/replay-feed.test.js
//
// The replay feed's answer to a 429, run rather than read: a fixed one-second
// retry passes every text check and still asks the server again sixty times
// inside the window it named.

import { test } from "node:test";
import assert from "node:assert/strict";

const replayFeed = await import("../../web/replay-feed.js");

function fakeClock() {
  const real = { now: Date.now, setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout };
  let now = 0;
  let nextId = 1;
  const pending = new Map();
  Date.now = () => now;
  globalThis.setTimeout = (fn, ms) => {
    pending.set(nextId, { at: now + ms, fn });
    return nextId++;
  };
  globalThis.clearTimeout = (id) => pending.delete(id);
  return {
    tick(ms) {
      now += ms;
      for (const [id, timer] of [...pending]) {
        if (timer.at > now) continue;
        pending.delete(id);
        timer.fn();
      }
    },
    restore() {
      Date.now = real.now;
      globalThis.setTimeout = real.setTimeout;
      globalThis.clearTimeout = real.clearTimeout;
    },
  };
}

test("replay-feed waits out Retry-After", async () => {
  const clock = fakeClock();
  const posts = [];
  const statuses = [429, 204];
  globalThis.fetch = async (_url, init) => {
    posts.push(JSON.parse(init.body).events.length);
    const status = statuses.shift();
    return { status, headers: { get: (name) => (name === "Retry-After" ? "30" : null) } };
  };
  try {
    replayFeed.initReplay({
      state: { interviewId: "i1" },
      nodes: {},
      recordingEnabled: true,
      consentVersion: "v1",
      replayVersion: 1,
    });
    replayFeed.recordReplay("lifecycle", { state: "started" });
    await replayFeed.flushReplay();
    assert.deepEqual(posts, [1], "the first batch is refused");

    replayFeed.recordReplay("lifecycle", { state: "still_here" });
    await replayFeed.flushReplay();
    assert.deepEqual(posts, [1], "a flush inside the window sends nothing");

    for (let second = 1; second < 30; second += 1) {
      clock.tick(1000);
      await new Promise(setImmediate);
    }
    assert.deepEqual(posts, [1], "nor does any timer before the window has passed");

    clock.tick(1000);
    await new Promise(setImmediate);
    assert.deepEqual(posts, [1, 2], "then the kept batch goes, with what queued behind it");
  } finally {
    replayFeed.closeReplay();
    clock.restore();
    delete globalThis.fetch;
  }
});

test("replay-feed keeps a batch alive past the page unless it is over the keepalive budget", async () => {
  const feed = await import("../../web/replay-feed.js?keepalive");
  const posts = [];
  globalThis.fetch = async (_url, init) => {
    posts.push({ events: JSON.parse(init.body).events.length, keepalive: init.keepalive });
    return { status: 204, headers: { get: () => null } };
  };
  try {
    feed.initReplay({
      state: { interviewId: "i1" },
      nodes: {},
      recordingEnabled: true,
      consentVersion: "v1",
      replayVersion: 1,
    });
    feed.recordReplay("lifecycle", { state: "ended", reason: "time_up" });
    await feed.flushReplay();
    feed.recordReplay("code", { text: "é".repeat(40_000) });
    await feed.flushReplay();
    assert.deepEqual(posts, [
      { events: 1, keepalive: true },
      { events: 1, keepalive: false },
    ]);
  } finally {
    feed.closeReplay();
    delete globalThis.fetch;
  }
});
