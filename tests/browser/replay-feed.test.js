// Run with: node --test tests/browser/replay-feed.test.js
//
// The replay feed's answer to a 429, run rather than read: a fixed one-second
// retry passes every text check and still asks the server again sixty times
// inside the window it named.

import { test } from "node:test";
import assert from "node:assert/strict";

const replayFeed = await import("../../web/replay-feed.js");

test("every stage restatement carries the committed avatar selection", async (t) => {
  const feed = await import("../../web/replay-feed.js?avatar-selection");
  const originalFetch = globalThis.fetch;
  const posts = [];
  globalThis.fetch = async (_url, init) => {
    posts.push(...JSON.parse(init.body).events);
    return { status: 204, headers: { get: () => null } };
  };
  t.after(() => {
    feed.closeReplay();
    globalThis.fetch = originalFetch;
  });
  feed.initReplay({
    state: {
      interviewId: "i-avatar",
      interviewerAvatar: "jim-business-male-04",
    },
    nodes: {
      title: { textContent: "A problem" },
      meta: { textContent: "Python" },
    },
    recordingEnabled: true,
    consentVersion: "v1",
    replayVersion: 1,
  });
  feed.recordStage();
  await feed.flushReplay();
  feed.recordStageTick(300);
  await feed.flushReplay();
  assert.equal(posts.length, 2);
  assert.deepEqual(
    posts.map(({ payload }) => payload.avatarId),
    ["jim-business-male-04", "jim-business-male-04"],
  );
  assert.equal(posts[1].payload.remainingSeconds, 300);
});

function fakeClock() {
  const real = {
    now: Date.now,
    setTimeout: globalThis.setTimeout,
    clearTimeout: globalThis.clearTimeout,
  };
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
    return {
      status,
      headers: { get: (name) => (name === "Retry-After" ? "30" : null) },
    };
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
    assert.deepEqual(
      posts,
      [1],
      "nor does any timer before the window has passed",
    );

    clock.tick(1000);
    await new Promise(setImmediate);
    assert.deepEqual(
      posts,
      [1, 2],
      "then the kept batch goes, with what queued behind it",
    );
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
    posts.push({
      events: JSON.parse(init.body).events.length,
      keepalive: init.keepalive,
    });
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

test("replay-feed compacts superseded restates so the interview's end fits one keepalive post", async () => {
  const feed = await import("../../web/replay-feed.js?compaction");
  const posts = [];
  globalThis.fetch = async (_url, init) => {
    posts.push({
      keepalive: init.keepalive,
      bytes: new TextEncoder().encode(init.body).length,
      events: JSON.parse(init.body).events,
    });
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
    // The queue an interview ends on: three editor snapshots of a large buffer,
    // two stage restatements, and the evidence between them.
    feed.recordReplay("editor", { text: "a".repeat(30_000) });
    feed.recordReplay("transcript", { text: "how would you start?" });
    feed.recordReplay("stage", { title: "superseded" });
    feed.recordReplay("editor", { text: "b".repeat(30_000) });
    feed.recordReplay("tests", { passed: 3, failed: 1 });
    feed.recordReplay("stage", { title: "newest" });
    feed.recordReplay("editor", { text: "c".repeat(30_000) });
    feed.recordReplay("lifecycle", { state: "ended", reason: "time_up" });
    feed.recordReplay("lifecycle", { state: "rounds_final", rounds: 2 });
    await feed.flushReplay();

    assert.equal(
      posts.length,
      1,
      "the end of the interview goes out in one post",
    );
    const [post] = posts;
    assert.equal(
      post.keepalive,
      true,
      "and it is the post that survives the tab closing",
    );
    assert.ok(
      post.bytes <= 64 * 1024,
      `the body is ${post.bytes} bytes, over the keepalive budget`,
    );
    assert.deepEqual(
      post.events.map((event) => event.kind),
      ["transcript", "tests", "stage", "editor", "lifecycle", "lifecycle"],
      "only the superseded editor and stage frames are gone, and the order is the candidate's",
    );
    assert.equal(
      post.events.at(3).payload.text,
      "c".repeat(30_000),
      "the editor frame kept is the newest",
    );
    assert.equal(
      post.events.at(2).payload.title,
      "newest",
      "and so is the stage frame",
    );
    assert.deepEqual(
      post.events
        .filter((event) => event.kind === "lifecycle")
        .map((event) => event.payload.state),
      ["ended", "rounds_final"],
      "the lifecycle events are present and in order",
    );
  } finally {
    feed.closeReplay();
    delete globalThis.fetch;
  }
});

test("replay-feed splits an oversized queue rather than compacting past the restates", async () => {
  const feed = await import("../../web/replay-feed.js?splitting");
  const posts = [];
  globalThis.fetch = async (_url, init) => {
    posts.push({
      keepalive: init.keepalive,
      bytes: new TextEncoder().encode(init.body).length,
      events: JSON.parse(init.body).events,
    });
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
    // Four transcript lines of twenty kilobytes each: nothing here is
    // superseded by anything, so the only answer is more posts.
    for (let line = 0; line < 4; line += 1) {
      feed.recordReplay("transcript", { text: `${line}`.repeat(20_000) });
    }
    await feed.flushReplay();
    await feed.flushReplay();

    assert.ok(
      posts.length > 1,
      "a queue over the budget is more than one post",
    );
    for (const post of posts) {
      assert.equal(
        post.keepalive,
        true,
        "and every one of them can survive the page",
      );
      assert.ok(
        post.bytes <= 64 * 1024,
        `a body of ${post.bytes} bytes is over the keepalive budget`,
      );
    }
    assert.deepEqual(
      posts
        .flatMap((post) => post.events)
        .map((event) => event.payload.text.at(0)),
      ["0", "1", "2", "3"],
      "every line is delivered, in order",
    );
  } finally {
    feed.closeReplay();
    delete globalThis.fetch;
  }
});

test("replay-feed sends an event larger than the budget on its own rather than stalling", async () => {
  const feed = await import("../../web/replay-feed.js?oversize");
  const posts = [];
  globalThis.fetch = async (_url, init) => {
    posts.push({
      keepalive: init.keepalive,
      kinds: JSON.parse(init.body).events.map((event) => event.kind),
    });
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
    // Eighty kilobytes in one event: over the keepalive budget on its own, and
    // under the server's own body limit, so it is the batch or there is none.
    feed.recordReplay("editor", { text: "é".repeat(40_000) });
    feed.recordReplay("transcript", { text: "that is my final answer" });
    feed.recordReplay("lifecycle", { state: "ended", reason: "time_up" });
    await feed.flushReplay();
    await feed.flushReplay();

    assert.deepEqual(
      posts,
      [
        { keepalive: false, kinds: ["editor"] },
        { keepalive: true, kinds: ["transcript", "lifecycle"] },
      ],
      "the oversized event goes alone and unflagged, and what queued behind it follows in order",
    );
  } finally {
    feed.closeReplay();
    delete globalThis.fetch;
  }
});

test("replay-feed keeps a batch of exactly the keepalive budget in one post", async () => {
  // The separators are one fewer than the events. Counted per event instead,
  // a body of exactly the budget reads as one byte over it and is split, and
  // the second post is the one that waits on the first and dies with the page.
  const clock = fakeClock();
  const feed = await import("../../web/replay-feed.js?boundary");
  const posts = [];
  globalThis.fetch = async (_url, init) => {
    posts.push({
      bytes: new TextEncoder().encode(init.body).length,
      keepalive: init.keepalive,
    });
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
    const encoded = (value) =>
      new TextEncoder().encode(JSON.stringify(value)).length;
    // The envelope and the event, as the queue writes them: the fake clock
    // holds `at` at 0, so the only length left to choose is the text's.
    const envelope = encoded({ events: [] });
    const event = (text) => ({
      v: 1,
      kind: "transcript",
      at: 0,
      payload: { text },
    });
    const budget = 64 * 1024;
    const text = "x".repeat(
      budget - envelope - 1 - encoded(event("")) - encoded(event("")),
    );

    feed.recordReplay("transcript", { text: "" });
    feed.recordReplay("transcript", { text });
    await feed.flushReplay();

    assert.deepEqual(
      posts,
      [{ bytes: budget, keepalive: true }],
      "one post, of exactly the budget, and one the page closing cannot cancel",
    );
  } finally {
    feed.closeReplay();
    clock.restore();
    delete globalThis.fetch;
  }
});
