import { test } from "node:test";
import assert from "node:assert/strict";
import { runInNewContext } from "node:vm";
import { functionBody, read } from "./source.js";

function fixture({ enabled = true } = {}) {
  const page = read("web/interview.js");

  const monitors = [];
  let cancellations = 0;

  const context = {
    audioTiming: enabled
      ? {
          cancelPending: () => {
            cancellations++;
          },
          audioActivity: () => {},
        }
      : null,

    startAudioActivityMonitor(track, onActivity, options) {
      const monitor = {
        track,
        onActivity,
        options,
        stopped: false,
      };

      monitors.push(monitor);

      return () => {
        monitor.stopped = true;
      };
    },

    console: {
      warn() {},
    },
  };

  const code = `
    let audioTimingTrack = null;
    let stopAudioTimingMonitor = null;
    let audioTimingReady = false;

    ${functionBody(page, "releaseAudioTiming")}
    }

    ${functionBody(page, "attachAudioTiming")}
    }

    globalThis.timingApi = {
      attachAudioTiming,
      releaseAudioTiming,
      currentTrack: () => audioTimingTrack,
      ready: () => audioTimingReady,
    };
  `;

  runInNewContext(code, context);

  return {
    api: context.timingApi,
    monitors,
    cancellations: () => cancellations,
  };
}

function remoteTrack(id) {
  return {
    id,
    mediaStreamTrack: {
      kind: "audio",
      readyState: "live",
    },
  };
}

test("replacing a remote track stops the old monitor", () => {
  const f = fixture();

  const trackA = remoteTrack("A");
  const trackB = remoteTrack("B");

  f.api.attachAudioTiming(trackA);

  assert.equal(f.monitors.length, 1);
  assert.equal(f.monitors[0].stopped, false);

  f.api.attachAudioTiming(trackB);

  assert.equal(f.monitors.length, 2);
  assert.equal(f.monitors[0].stopped, true);
  assert.equal(f.monitors[1].stopped, false);

  assert.equal(f.api.currentTrack(), trackB);

  f.api.releaseAudioTiming();
});

test("late unsubscribe from an old track preserves the new monitor", () => {
  const f = fixture();

  const trackA = remoteTrack("A");
  const trackB = remoteTrack("B");

  f.api.attachAudioTiming(trackA);
  f.api.attachAudioTiming(trackB);

  // The old unsubscribe arrives after the replacement is active.
  f.api.releaseAudioTiming(trackA);

  assert.equal(f.monitors.length, 2);
  assert.equal(f.monitors[1].stopped, false);
  assert.equal(f.api.currentTrack(), trackB);

  // Releasing the current track must still work.
  f.api.releaseAudioTiming(trackB);

  assert.equal(f.monitors[1].stopped, true);
  assert.equal(f.api.currentTrack(), null);
});

test("reconnect can recreate a monitor for the same remote track", () => {
  const f = fixture();

  const track = remoteTrack("A");

  f.api.attachAudioTiming(track);

  assert.equal(f.monitors.length, 1);

  // Simulate the Reconnecting handler.
  f.api.releaseAudioTiming();

  assert.equal(f.monitors[0].stopped, true);
  assert.equal(f.api.currentTrack(), null);

  // Simulate Reconnected with an existing live track.
  f.api.attachAudioTiming(track);

  assert.equal(f.monitors.length, 2);
  assert.equal(f.monitors[1].stopped, false);
  assert.equal(f.api.currentTrack(), track);

  f.api.releaseAudioTiming();
});

test("disabled audio timing creates no monitor", () => {
  const f = fixture({ enabled: false });

  const track = remoteTrack("A");

  f.api.attachAudioTiming(track);

  assert.equal(f.monitors.length, 0);
  assert.equal(f.api.currentTrack(), null);

  f.api.releaseAudioTiming();

  assert.equal(f.monitors.length, 0);
});
