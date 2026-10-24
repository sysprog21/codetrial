import { test } from "node:test";
import assert from "node:assert/strict";
import { FakeStream, FakeTrack, settle } from "./fake-media.js";

import { createDevicePool, openMicrophone } from "../../web/devices.js";

function poolWith(getUserMedia, options = {}) {
  const changes = { count: 0 };
  const pool = createDevicePool({
    mediaDevices: { getUserMedia },
    isFinished: options.isFinished ?? (() => false),
    onChange: () => {
      changes.count += 1;
    },
    retryMs: options.retryMs ?? 20,
    // For a test that drives the retry by hand. Left undefined, the pool's
    // own defaults take over, so every other test keeps the real timer.
    schedule: options.schedule,
    unschedule: options.unschedule,
  });
  pool.configure("audio", {
    accept: (track) => Boolean(track),
    onTrack: () => {},
  });
  pool.configure("video", {
    accept: (track) => Boolean(track),
    onTrack: () => {},
  });
  return { pool, changes };
}

test("a granted track of each kind lands in one stream", async () => {
  const { pool } = poolWith(
    async (constraints) =>
      new FakeStream([new FakeTrack(constraints.audio ? "audio" : "video")]),
  );

  pool.start();
  await settle();

  assert.equal(pool.stream.getTracks().length, 2);
  assert.ok(pool.trackOf("audio"));
  assert.ok(pool.trackOf("video"));
  assert.equal(pool.errorOf("audio"), null);
});

test("selecting a microphone replaces only audio and preserves processing constraints", async () => {
  const asked = [];
  const { pool } = poolWith(async (constraints) => {
    asked.push(constraints);
    return new FakeStream([
      new FakeTrack(constraints.audio ? "audio" : "video"),
    ]);
  });
  let lost = 0;
  pool.configure("audio", {
    onLost: () => {
      lost += 1;
    },
  });
  pool.start();
  await settle();
  const original = pool.trackOf("audio");
  const camera = pool.trackOf("video");
  const stream = pool.stream;
  pool.selectMicrophone("usb");
  assert.equal(original.stopped, true);
  assert.equal(pool.trackOf("audio"), null);
  assert.equal(lost, 1);
  await settle();
  assert.deepEqual(asked.at(-1), {
    audio: {
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
      deviceId: { exact: "usb" },
    },
  });
  assert.equal(pool.stream, stream);
  assert.equal(pool.trackOf("video"), camera);
  assert.equal(camera.stopped, false);
  assert.notEqual(pool.trackOf("audio"), original);
  const count = asked.length;
  pool.selectMicrophone("usb");
  assert.equal(asked.length, count);
  pool.selectMicrophone("");
  await settle();
  assert.equal(asked.at(-1).audio.deviceId, undefined);
  assert.equal(pool.stream.getTracks().length, 2);
});

test("rapid microphone choices stop the old grant and request only the latest device", async () => {
  const pending = [];
  const asked = [];
  const { pool } = poolWith((constraints) => {
    if (constraints.video)
      return Promise.resolve(new FakeStream([new FakeTrack("video")]));
    asked.push(constraints.audio.deviceId?.exact || "");
    return new Promise((resolve) => pending.push(resolve));
  });
  pool.start();
  pool.selectMicrophone("usb");
  pool.selectMicrophone("headset");
  assert.deepEqual(asked, [""]);
  const abandoned = new FakeTrack("audio");
  pending.shift()(new FakeStream([abandoned]));
  await settle();
  assert.equal(abandoned.stopped, true);
  assert.equal(pool.trackOf("audio"), null);
  assert.deepEqual(asked, ["", "headset"]);
  const latest = new FakeTrack("audio");
  pending.shift()(new FakeStream([latest]));
  await settle();
  assert.equal(pool.trackOf("audio"), latest);
  assert.equal(latest.stopped, false);
});

test("an obsolete microphone rejection does not poison the latest choice", async () => {
  let rejectOld;
  let grantNew;
  const { pool } = poolWith((constraints) => {
    if (constraints.video)
      return Promise.resolve(new FakeStream([new FakeTrack("video")]));
    if (!constraints.audio.deviceId)
      return new Promise((resolve, reject) => {
        rejectOld = reject;
      });
    return new Promise((resolve) => {
      grantNew = resolve;
    });
  });
  pool.start();
  pool.selectMicrophone("usb");
  rejectOld(new Error("Old device refused"));
  await settle();
  assert.equal(pool.errorOf("audio"), null);
  const selected = new FakeTrack("audio");
  grantNew(new FakeStream([selected]));
  await settle();
  assert.equal(pool.trackOf("audio"), selected);
});

test("a failed selected microphone retries that device and can return to default", async () => {
  const asked = [];
  const retries = [];
  const { pool } = poolWith(
    async (constraints) => {
      if (constraints.audio?.deviceId) {
        asked.push(constraints.audio.deviceId.exact);
        throw new Error("Selected device unavailable");
      }
      return new FakeStream([
        new FakeTrack(constraints.audio ? "audio" : "video"),
      ]);
    },
    {
      schedule: (callback) => {
        retries.push(callback);
        return retries.length;
      },
      unschedule: () => {},
    },
  );
  pool.start();
  await settle();
  pool.selectMicrophone("missing");
  await settle();
  assert.equal(pool.trackOf("audio"), null);
  assert.match(pool.errorOf("audio"), /Selected device unavailable/);
  retries.shift()();
  await settle();
  assert.deepEqual(asked, ["missing", "missing"]);
  pool.selectMicrophone("");
  await settle();
  assert.ok(pool.trackOf("audio"));
  assert.equal(pool.errorOf("audio"), null);
});

test("leaving during a microphone change stops a late grant without requesting again", async () => {
  let finished = false;
  let grant;
  let asked = 0;
  const { pool } = poolWith(
    (constraints) => {
      if (constraints.video)
        return Promise.resolve(new FakeStream([new FakeTrack("video")]));
      asked += 1;
      return new Promise((resolve) => {
        grant = resolve;
      });
    },
    { isFinished: () => finished },
  );
  pool.start();
  pool.selectMicrophone("usb");
  finished = true;
  const late = new FakeTrack("audio");
  grant(new FakeStream([late]));
  await settle();
  assert.equal(late.stopped, true);
  assert.equal(pool.trackOf("audio"), null);
  assert.equal(asked, 1);
  pool.selectMicrophone("headset");
  assert.equal(asked, 1);
});

// The bug this replaces: each device scheduled its own retry and both called
// back into the same start, so denying both prompts doubled the number of
// in-flight requests every two seconds until the tab died.
test("two denied devices share one retry, so requests do not multiply", async () => {
  let asked = 0;
  const pending = [];
  const { pool } = poolWith(
    async () => {
      asked += 1;
      throw new Error("NotAllowedError");
    },
    {
      schedule: (fn) => {
        pending.push(fn);
        return pending.length;
      },
      unschedule: () => {},
    },
  );

  pool.start();
  await settle();

  assert.equal(asked, 2, "one prompt per device on the first pass");
  assert.equal(pending.length, 1, "two denials scheduled more than one retry");
  assert.match(pool.errorOf("audio"), /NotAllowedError/);

  pending.pop()();
  await settle();
  assert.equal(asked, 4, "the retry asked once per device");
});

test("disabling the camera leaves microphone retry enabled", async () => {
  const asked = [];
  let retry = null;
  const { pool } = poolWith(
    async (constraints) => {
      asked.push(constraints.audio ? "audio" : "video");
      throw new Error("denied");
    },
    {
      schedule: (callback) => {
        retry = callback;
        return 1;
      },
      unschedule: () => {},
    },
  );

  pool.start();
  await settle();
  pool.disable("video");
  retry();
  await settle();

  assert.deepEqual(asked, ["audio", "video", "audio"]);
});

test("disabling a held camera releases it from the interview stream", async () => {
  const { pool } = poolWith(
    async (constraints) =>
      new FakeStream([new FakeTrack(constraints.audio ? "audio" : "video")]),
  );

  pool.start();
  await settle();
  const camera = pool.trackOf("video");
  pool.disable("video");

  assert.ok(camera.stopped);
  assert.equal(pool.trackOf("video"), null);
});

test("a re-enabled camera is requested again", async () => {
  const asked = [];
  const { pool } = poolWith(async (constraints) => {
    const kind = constraints.audio ? "audio" : "video";
    asked.push(kind);
    return new FakeStream([new FakeTrack(kind)]);
  });

  pool.start();
  await settle();
  pool.disable("video");
  pool.enable("video");
  await settle();

  assert.deepEqual(asked, ["audio", "video", "video"]);
  assert.ok(pool.trackOf("video"));
});

test("undoing a skip after a failed attempt asks again with no stale error", async () => {
  let videoRequests = 0;
  let grantVideo;
  const { pool } = poolWith(
    (constraints) => {
      if (!constraints.video)
        return Promise.resolve(new FakeStream([new FakeTrack("audio")]));
      videoRequests += 1;
      if (videoRequests === 1)
        return Promise.reject(
          Object.assign(new Error("Permission denied"), {
            name: "NotAllowedError",
          }),
        );
      return new Promise((resolve) => {
        grantVideo = resolve;
      });
    },
    { schedule: () => 1, unschedule: () => {} },
  );

  pool.start();
  await settle();
  assert.match(pool.errorOf("video"), /NotAllowedError/);

  pool.disable("video");
  pool.enable("video");
  assert.equal(videoRequests, 2, "undoing the skip asks the camera again");
  assert.equal(pool.errorOf("video"), null, "the old error is gone");

  grantVideo(new FakeStream([new FakeTrack("video")]));
  await settle();
  assert.ok(pool.trackOf("video"));
  assert.equal(pool.errorOf("video"), null);
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
      : new FakeStream([new FakeTrack("audio")]),
  );

  pool.start();
  await settle();

  assert.ok(spare.stopped, "the second video track was left running");
  assert.equal(
    pool.stream.getTracks().filter((t) => t.kind === "video").length,
    1,
  );
});

// A grant that resolves after the preflight has no owner: the stream the
// candidate joins with is already handed over, so a late one would turn a
// device back on behind their back.
test("a track granted after the preflight is stopped, not adopted", async () => {
  let finished = false;
  const late = new FakeTrack("audio");
  let release;
  const { pool } = poolWith(
    () =>
      new Promise((resolve) => {
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
    return new FakeStream([
      new FakeTrack(constraints.audio ? "audio" : "video"),
    ]);
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
    return new FakeStream([
      new FakeTrack(constraints.audio ? "audio" : "video"),
    ]);
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

// The pool drops the dead track itself, so whatever the caller was running
// over it -- the preflight's face check, its level meter -- has to be told in
// the same step. Otherwise the replacement arrives and nothing notices it is a
// different device, and a face check already bound to the old one refuses to
// start over the new one.
test("the caller is told when the pool drops a device that ended", async () => {
  const lost = [];
  const { pool } = poolWith(
    async (constraints) =>
      new FakeStream([new FakeTrack(constraints.audio ? "audio" : "video")]),
  );
  // Merged onto the hooks `poolWith` already set, because a caller that names
  // one of the three must not blank the other two.
  pool.configure("video", { onLost: () => lost.push("video") });

  pool.start();
  await settle();
  assert.deepEqual(lost, [], "nothing was lost while both devices were live");

  pool.trackOf("video").readyState = "ended";
  pool.start();
  await settle();

  assert.deepEqual(
    lost,
    ["video"],
    "the camera was replaced behind the caller's back",
  );
  assert.equal(pool.trackOf("video").readyState, "live");
});

// The preflight reads its signals on every animation frame, and that read drops
// an ended track. So `onLost` runs inside a read that the hook itself restarts:
// the audio hook calls refresh(), refresh() reads again, and the read would drop
// the same track a second time if it were still there. Removing before notifying
// is what makes that bottom out after one pass instead of recursing until the
// stack gives out and the preflight page dies.
//
// Pinned here rather than in the caller because the ordering is the pool's to
// keep, and swapping the two lines reads just as naturally as leaving them.
test("a dropped track is gone from the stream before the caller is told", async () => {
  const { pool } = poolWith(
    async (constraints) =>
      new FakeStream([new FakeTrack(constraints.audio ? "audio" : "video")]),
    { retryMs: 10_000 },
  );
  pool.start();
  await settle();

  let seen = "not called";
  pool.configure("audio", {
    onLost: () => {
      seen = pool.trackOf("audio");
    },
  });
  pool.dropTrack(pool.trackOf("audio"));

  assert.equal(
    seen,
    null,
    "onLost must not see the track it is being told about",
  );
  assert.equal(pool.trackOf("audio"), null);
  pool.cancelRetry();
});

// Same rule for the pool's own drop of an ended track, the other route in.
test("the pool's own drop of an ended track also removes before notifying", async () => {
  let finished = false;
  const { pool } = poolWith(
    async (constraints) =>
      new FakeStream([new FakeTrack(constraints.audio ? "audio" : "video")]),
    { isFinished: () => finished, retryMs: 10 },
  );
  pool.start();
  await settle();

  pool.trackOf("audio").readyState = "ended";
  let seen = "not called";
  pool.configure("audio", {
    onLost: () => {
      seen = pool.trackOf("audio");
    },
  });
  pool.retry();
  await new Promise((resolve) => setTimeout(resolve, 30));

  assert.equal(
    seen,
    null,
    "onLost must not see the ended track still in the stream",
  );
  finished = true;
  pool.cancelRetry();
});

// The switch the candidate asked for names its device exactly: opening some
// other input and calling it theirs would be worse than saying it failed.
test("a mid-interview microphone is asked for by exact device", async () => {
  const asked = [];
  const track = new FakeTrack("audio");
  const opened = await openMicrophone(
    {
      getUserMedia: async (constraints) => {
        asked.push(constraints);
        return new FakeStream([track]);
      },
    },
    "headset",
  );

  assert.equal(opened, track);
  assert.deepEqual(asked, [
    {
      audio: {
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
        deviceId: { exact: "headset" },
      },
    },
  ]);
});

test("the default microphone is asked for with no device id", async () => {
  const asked = [];
  await openMicrophone(
    {
      getUserMedia: async (constraints) => {
        asked.push(constraints);
        return new FakeStream([new FakeTrack("audio")]);
      },
    },
    "",
  );

  assert.equal("deviceId" in asked[0].audio, false);
});

test("a grant with no live input is refused and leaves nothing open", async () => {
  const spare = new FakeTrack("video");
  const dead = new FakeTrack("audio");
  dead.readyState = "ended";

  await assert.rejects(
    openMicrophone(
      { getUserMedia: async () => new FakeStream([dead, spare]) },
      "headset",
    ),
    /no active audio track/,
  );
  assert.ok(spare.stopped, "a track nothing will read was left running");
  assert.ok(dead.stopped);
});
