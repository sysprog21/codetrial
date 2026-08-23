import { test } from "node:test";
import assert from "node:assert/strict";

import { createDevicePool } from "../../web/devices.js";

class FakeTrack {
  constructor(kind) {
    this.kind = kind;
    this.stopped = false;
    this.readyState = "live";
  }
  stop() {
    this.stopped = true;
  }
}

class FakeStream {
  constructor(tracks = []) {
    this.tracks = tracks;
  }
  getTracks() {
    return this.tracks;
  }
  addTrack(track) {
    this.tracks.push(track);
  }
  removeTrack(track) {
    this.tracks = this.tracks.filter((each) => each !== track);
  }
}

// The pool builds its own stream with `new MediaStream()`, which node does not
// have. Nothing else in these tests touches a global.
globalThis.MediaStream = FakeStream;

function poolWith(getUserMedia, options = {}) {
  const changes = { count: 0 };
  const pool = createDevicePool({
    mediaDevices: { getUserMedia },
    isFinished: options.isFinished ?? (() => false),
    onChange: () => {
      changes.count += 1;
    },
    retryMs: options.retryMs ?? 20,
  });
  pool.configure("audio", { accept: (track) => Boolean(track), onTrack: () => {} });
  pool.configure("video", { accept: (track) => Boolean(track), onTrack: () => {} });
  return { pool, changes };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

test("a granted track of each kind lands in one stream", async () => {
  const { pool } = poolWith(async (constraints) =>
    new FakeStream([new FakeTrack(constraints.audio ? "audio" : "video")]));

  pool.start();
  await settle();

  assert.equal(pool.stream.getTracks().length, 2);
  assert.ok(pool.trackOf("audio"));
  assert.ok(pool.trackOf("video"));
  assert.equal(pool.errorOf("audio"), null);
});

// The bug this replaces: each device scheduled its own retry and both called
// back into the same start, so denying both prompts doubled the number of
// in-flight requests every two seconds until the tab died.
test("two denied devices share one retry, so requests do not multiply", async () => {
  let asked = 0;
  const pending = [];
  const pool = createDevicePool({
    mediaDevices: {
      getUserMedia: async () => {
        asked += 1;
        throw new Error("NotAllowedError");
      },
    },
    isFinished: () => false,
    onChange: () => {},
    schedule: (fn) => {
      pending.push(fn);
      return pending.length;
    },
    unschedule: () => {},
  });
  pool.configure("audio", { accept: (track) => Boolean(track), onTrack: () => {} });
  pool.configure("video", { accept: (track) => Boolean(track), onTrack: () => {} });

  pool.start();
  await settle();

  assert.equal(asked, 2, "one prompt per device on the first pass");
  assert.equal(pending.length, 1, "two denials scheduled more than one retry");
  assert.match(pool.errorOf("audio"), /NotAllowedError/);

  pending.pop()();
  await settle();
  assert.equal(asked, 4, "the retry asked once per device");
});

test("a request already on screen is not opened a second time", async () => {
  let asked = 0;
  const { pool } = poolWith(() => {
    asked += 1;
    return new Promise(() => {});
  });

  pool.start();
  pool.start();
  pool.start();

  assert.equal(asked, 2, "one still-pending prompt per device");
});

test("a track the caller refuses is stopped rather than held", async () => {
  const refused = new FakeTrack("video");
  const pool = createDevicePool({
    mediaDevices: { getUserMedia: async () => new FakeStream([refused]) },
    isFinished: () => false,
    onChange: () => {},
    retryMs: 10_000,
  });
  pool.configure("audio", { accept: () => false, onTrack: () => {} });
  pool.configure("video", { accept: () => false, onTrack: () => {} });

  pool.start();
  await settle();
  pool.cancelRetry();

  assert.ok(refused.stopped);
  assert.equal(pool.trackOf("video"), null);
  assert.equal(pool.errorOf("video"), "no active video track");
});

// A device that hands back more than was asked for leaves the spares running
// otherwise, which is a camera light on for a track nothing reads.
test("tracks the pool did not claim are stopped", async () => {
  const spare = new FakeTrack("video");
  const { pool } = poolWith(async (constraints) =>
    constraints.video
      ? new FakeStream([new FakeTrack("video"), spare])
      : new FakeStream([new FakeTrack("audio")]));

  pool.start();
  await settle();

  assert.ok(spare.stopped, "the second video track was left running");
  assert.equal(pool.stream.getTracks().filter((t) => t.kind === "video").length, 1);
});

// A grant that resolves after the preflight has no owner: the stream the
// candidate joins with is already handed over, so a late one would turn a
// device back on behind their back.
test("a track granted after the preflight is stopped, not adopted", async () => {
  let finished = false;
  const late = new FakeTrack("audio");
  let release;
  const { pool } = poolWith(
    () => new Promise((resolve) => {
      release = () => resolve(new FakeStream([late]));
    }),
    { isFinished: () => finished, retryMs: 10_000 },
  );

  pool.start();
  finished = true;
  release();
  await settle();
  pool.cancelRetry();

  assert.ok(late.stopped);
  assert.equal(pool.trackOf("audio"), null);
});

test("a browser without getUserMedia says so on both devices", () => {
  const pool = createDevicePool({
    mediaDevices: {},
    isFinished: () => false,
    onChange: () => {},
  });

  assert.equal(pool.supported, false);
  pool.start();
  assert.equal(pool.errorOf("audio"), "getUserMedia is not supported");
  assert.equal(pool.errorOf("video"), "getUserMedia is not supported");
});

// An ended track never revives, and while it sits in the stream the retry sees
// a track of that kind and asks for nothing.
test("dropping an ended track lets the retry ask for a replacement", async () => {
  let asked = 0;
  const { pool } = poolWith(async (constraints) => {
    asked += 1;
    return new FakeStream([new FakeTrack(constraints.audio ? "audio" : "video")]);
  });

  pool.start();
  await settle();
  assert.equal(asked, 2);

  pool.dropTrack(pool.trackOf("video"));
  pool.start();
  await settle();

  assert.equal(asked, 3, "only the dropped kind was asked for again");
});

// A microphone that goes away mid-preflight leaves an ended track sitting in
// the stream. Until the pool dropped it, that track looked like a device the
// pool already held, so the retry asked for nothing and the gate the candidate
// cannot bypass never reopened.
test("an ended track is replaced instead of counted as a device the pool holds", async () => {
  let asked = 0;
  const { pool } = poolWith(async (constraints) => {
    asked += 1;
    return new FakeStream([new FakeTrack(constraints.audio ? "audio" : "video")]);
  });

  pool.start();
  await settle();
  assert.equal(asked, 2);

  const mic = pool.trackOf("audio");
  mic.readyState = "ended";
  pool.start();
  await settle();

  assert.equal(asked, 3, "the ended microphone was never asked for again");
  assert.equal(pool.trackOf("audio").readyState, "live");
  assert.equal(
    pool.stream.getTracks().filter((each) => each.kind === "audio").length,
    1,
    "the dead track was left in the stream the candidate joins with",
  );
});
