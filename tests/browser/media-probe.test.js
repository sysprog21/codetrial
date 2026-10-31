import { test } from "node:test";
import assert from "node:assert/strict";
import {
  createMediaSampler,
  assertJimAudio,
  installMediaProbe,
  summarizeJimAudio,
  mediaProbeSource,
} from "../../scripts/media-probe.cjs";

function fixture({
  counts = [0, 10, 20],
  rms = 0.1,
  stats = true,
  running = true,
  contextError = false,
  timeoutMs = 40,
  speakingWaitMs = timeoutMs,
  minimumWindowMs,
  onDelay = () => {},
} = {}) {
  let clock = 0;
  let index = 0;
  let current = true;
  let speaking = true;
  let cleaned = 0;
  let closed = 0;
  const context = {
    state: running ? "running" : "suspended",
    resume: async () => {},
    close: async () => {
      closed++;
    },
    createMediaStreamSource: () => ({
      connect: () => {},
      disconnect: () => {
        cleaned++;
      },
    }),
    createAnalyser: () => ({
      fftSize: 0,
      getFloatTimeDomainData: (values) => values.fill(rms),
      disconnect: () => {
        cleaned++;
      },
    }),
  };
  const track = {
    mediaStreamTrack: {},
    getRTCStatsReport: async () => {
      if (!stats) return;
      const value = counts[Math.min(index++, counts.length - 1)];
      return new Map([
        [
          "inbound",
          {
            type: "inbound-rtp",
            kind: "audio",
            id: "inbound",
            bytesReceived: value,
            packetsReceived: value,
          },
        ],
      ]);
    },
  };
  const sampler = createMediaSampler({
    now: () => clock,
    delay: async (ms) => {
      clock += ms;
      onDelay();
    },
    makeContext: () => {
      if (contextError) throw new Error("unavailable");
      return context;
    },
    makeStream: (input) => {
      assert.equal(input, track.mediaStreamTrack);
      return {};
    },
    timeoutMs,
    speakingWaitMs,
    minimumWindowMs,
    intervalMs: 10,
  });
  return {
    track,
    sampler,
    context,
    run: () =>
      sampler(
        track,
        7,
        () => current,
        () => speaking,
      ),
    changeTrack: () => {
      current = false;
    },
    quiet: () => {
      speaking = false;
    },
    speak: () => {
      speaking = true;
    },
    cleaned: () => cleaned,
    closed: () => closed,
  };
}

test("track receiver counters and decoded signal establish distinct evidence", async () => {
  const f = fixture();
  const result = await f.run();
  assert.equal(result.status, "ok");
  assert.equal(result.generation, 7);
  assert.equal(result.bytesReceived, 10);
  assert.equal(result.packetsReceived, 10);
  assert.ok(result.peakRms > 0.01);
  assert.equal(result.durationMs, 10);
  assert.equal(f.cleaned(), 2);
  assert.equal(f.closed(), 1);
  assert.doesNotThrow(() => assertJimAudio({ generations: [result] }));
});
for (const [options, status] of [
  [{ counts: [9, 9, 9] }, "transport-silent"],
  [{ rms: 0 }, "decoded-silent"],
  [{ stats: false }, "unavailable"],
  [{ running: false }, "unavailable"],
  [{ contextError: true }, "unavailable"],
]) {
  test(`controlled ${JSON.stringify(options)} reports ${status}`, async () => {
    const f = fixture(options);
    const result = await f.run();
    assert.equal(result.status, status);
    assert.throws(
      () => assertJimAudio({ generations: [result] }),
      new RegExp(status),
    );
    if (!options.contextError) assert.equal(f.closed(), 1);
  });
}

test("counter resets do not create a positive delta", async () => {
  const f = fixture({ counts: [100, 1, 1, 1] });
  const result = await f.run();
  assert.equal(result.status, "transport-silent");
  assert.equal(result.bytesReceived, 0);
});

test("a replaced track cannot contribute late receiver observations", async () => {
  const f = fixture();
  f.track.getRTCStatsReport = async () => {
    f.changeTrack();
    return new Map([
      [
        "live",
        {
          type: "inbound-rtp",
          kind: "audio",
          id: "live",
          bytesReceived: 100,
          packetsReceived: 10,
        },
      ],
    ]);
  };
  const result = await f.run();
  assert.equal(result.status, "superseded");
  assert.equal(result.statsSamples, 0);
  assert.equal(f.closed(), 1);
});

test("a replacement after one counter pair supersedes a quiet generation instead of failing it", async () => {
  let delays = 0;
  const f = fixture({
    rms: 0,
    timeoutMs: 100,
    onDelay: () => {
      if (++delays === 2) f.changeTrack();
    },
  });
  const result = await f.run();
  assert.equal(result.counterPairs, 1);
  assert.equal(result.status, "superseded");
  assert.equal(result.reason, "track-ended-before-signal");
  assert.equal(f.closed(), 1);
});

test("a slow resume neither delays receiver samples nor ends decoded sampling", async () => {
  let delays = 0;
  let running;
  const f = fixture({
    counts: [0, 10, 20, 30, 40, 50],
    timeoutMs: 100,
    onDelay: () => {
      if (++delays === 3) running();
    },
  });
  f.context.state = "suspended";
  f.context.resume = () =>
    new Promise((resolve) => {
      running = () => {
        f.context.state = "running";
        resolve();
      };
    });
  const result = await f.run();
  assert.equal(result.status, "ok");
  assert.ok(result.statsSamples >= 4);
  assert.equal(f.closed(), 1);
});

test("a generation that sampled a long window keeps its failure when its track ends", async () => {
  let delays = 0;
  const f = fixture({
    counts: [9],
    timeoutMs: 1000,
    minimumWindowMs: 100,
    onDelay: () => {
      if (++delays === 20) f.changeTrack();
    },
  });
  const result = await f.run();
  assert.equal(result.status, "transport-silent");
});

test("no Speaking interval produces no signal claim or audio context", async () => {
  const f = fixture();
  f.quiet();
  const result = await f.run();
  assert.equal(result.status, "unavailable");
  assert.equal(result.samples, 0);
  assert.equal(result.reason, "no-speaking-samples");
  assert.equal(result.peakRms, null);
  assert.equal(f.closed(), 0);
});

test("receiver timeout is bounded and releases its context", async () => {
  const f = fixture({ timeoutMs: 10 });
  f.track.getRTCStatsReport = () => new Promise(() => {});
  const result = await f.run();
  assert.equal(result.status, "unavailable");
  assert.equal(f.closed(), 1);
});

test("a good old generation cannot hide a silent resubscription", () => {
  assert.throws(
    () =>
      assertJimAudio({
        generations: [
          { status: "ok" },
          { generation: 2, status: "transport-silent", reason: "flat" },
        ],
      }),
    /generation 2/,
  );
  assert.throws(
    () => assertJimAudio({ generations: [] }),
    /no-attached-agent-track/,
  );
});

test("receiver identity resets and ambiguous streams cannot fabricate packet deltas", async () => {
  const f = fixture();
  let calls = 0;
  f.track.getRTCStatsReport = async () =>
    new Map([
      [
        "inbound",
        {
          type: "inbound-rtp",
          kind: "audio",
          id: `stream-${calls++}`,
          bytesReceived: calls * 100,
          packetsReceived: calls,
        },
      ],
    ]);
  assert.equal((await f.run()).status, "unavailable");
  const g = fixture();
  g.track.getRTCStatsReport = async () =>
    new Map(
      [1, 2].map((id) => [
        id,
        {
          type: "inbound-rtp",
          kind: "audio",
          id,
          bytesReceived: 100,
          packetsReceived: 10,
        },
      ]),
    );
  assert.equal((await g.run()).status, "unavailable");
});

test("setup, analyser failure and refused cleanup remain bounded unavailable observations", async () => {
  const f = fixture();
  f.context.createMediaStreamSource = () => {
    throw new Error("setup");
  };
  assert.equal((await f.run()).status, "unavailable");
  assert.equal(f.closed(), 1);
  const g = fixture();
  g.context.createAnalyser = () => ({
    disconnect: () => {},
    getFloatTimeDomainData: () => {
      throw new Error("read");
    },
  });
  assert.equal((await g.run()).status, "unavailable");
  assert.equal(g.closed(), 1);
  const h = fixture();
  h.context.close = async () => {
    throw new Error("already closed");
  };
  assert.equal((await h.run()).status, "ok");
});

test("public Room subscription hooks follow only Jim's attached track and keep generation outcomes", async (t) => {
  const { EventEmitter } = await import("node:events");
  const names = [
    "LivekitClient",
    "__createMediaSampler",
    "__finishJimAudioProbe",
    "__summarizeJimAudio",
    "document",
  ];
  const previous = names.map((name) => [
    name,
    Object.getOwnPropertyDescriptor(globalThis, name),
  ]);
  t.after(() => {
    for (const [name, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
  });
  const events = {
    TrackSubscribed: "sub",
    TrackUnsubscribed: "unsub",
    Disconnected: "disconnect",
  };
  globalThis.LivekitClient = { Room: EventEmitter, RoomEvent: events };
  let attached;
  globalThis.document = {
    querySelector: (selector) =>
      selector === "#jim-audio"
        ? { srcObject: { getAudioTracks: () => [attached] } }
        : { textContent: "Speaking" },
  };
  const seen = [];
  globalThis.__createMediaSampler =
    () => async (track, generation, isCurrent, isSpeaking) => {
      seen.push({ track, generation, isCurrent, isSpeaking });
      return {
        generation,
        status: "ok",
        bytesReceived: 10,
        packetsReceived: 1,
        peakRms: 0.1,
      };
    };
  globalThis.__summarizeJimAudio = summarizeJimAudio;
  installMediaProbe();
  const room = new globalThis.LivekitClient.Room();
  const first = { kind: "audio", mediaStreamTrack: {} };
  const unrelated = { kind: "audio", mediaStreamTrack: {} };
  const replacement = { kind: "audio", mediaStreamTrack: {} };
  attached = first.mediaStreamTrack;
  room.emit("sub", unrelated);
  room.emit("sub", first);
  await new Promise((resolve) => queueMicrotask(resolve));
  assert.equal(seen.length, 1);
  assert.equal(seen[0].isCurrent(), true);
  assert.equal(seen[0].isSpeaking(), true);
  attached = replacement.mediaStreamTrack;
  room.emit("sub", replacement);
  await new Promise((resolve) => queueMicrotask(resolve));
  room.emit("unsub", first);
  assert.equal(seen[0].isCurrent(), false);
  assert.equal(seen[1].isCurrent(), true);
  room.emit("disconnect");
  assert.equal(seen[1].isCurrent(), false);
  const result = await globalThis.__finishJimAudioProbe();
  assert.equal(result.generations.length, 2);
  assert.equal(result.bytesReceived, 20);
  assert.equal(globalThis.LivekitClient.Room, EventEmitter);
  installMediaProbe();
  const noisyRoom = new globalThis.LivekitClient.Room();
  for (let index = 0; index < 65; index++) {
    const track = { kind: "audio", mediaStreamTrack: {} };
    attached = track.mediaStreamTrack;
    noisyRoom.emit("sub", track);
    await new Promise((resolve) => queueMicrotask(resolve));
  }
  const overflow = await globalThis.__finishJimAudioProbe();
  assert.equal(overflow.generations.length, 65);
  assert.equal(overflow.status, "unavailable");
  assert.equal(
    overflow.generations.at(-1).reason,
    "subscription-observation-limit",
  );
  assert.equal(overflow.generations.at(-1).bytesReceived, null);
  assert.equal(overflow.generations.at(-1).packetsReceived, null);
});

test("Chromium measures real received WebRTC packets and decoded tone without provider credentials", async (t) => {
  const { launchChromium, root } = await import("./source.js");
  const { join } = await import("node:path");
  const { readFileSync } = await import("node:fs");
  const browser = await launchChromium({
    args: ["--autoplay-policy=no-user-gesture-required"],
  });
  if (!browser) return t.skip("playwright chromium unavailable");
  t.after(() => browser.close());
  const page = await browser.newPage();
  await page.route("http://localhost:31999/**", async (route) => {
    const script = route.request().url().endsWith("/vendor/livekit-client.js");
    await route.fulfill({
      status: 200,
      headers: {
        "content-type": script ? "text/javascript" : "text/html",
        "content-security-policy":
          "default-src 'self'; script-src 'self' blob: 'wasm-unsafe-eval'",
      },
      body: script
        ? readFileSync(join(root, "web/vendor/livekit-client.js"), "utf8")
        : '<html><body><script src="/vendor/livekit-client.js"></script></body></html>',
    });
  });
  await page.goto("http://localhost:31999/");
  await page.evaluate(mediaProbeSource());
  const sdkProbe = await page.evaluate(async () => {
    const room = new globalThis.LivekitClient.Room();
    const result = await globalThis.__finishJimAudioProbe();
    await room.disconnect();
    return result;
  });
  assert.equal(sdkProbe.status, "unavailable");
  assert.equal(sdkProbe.generations.length, 0);
  const result = await page.evaluate(async () => {
    const sender = new RTCPeerConnection();
    const receiver = new RTCPeerConnection();
    const context = new AudioContext();
    const oscillator = context.createOscillator();
    const destination = context.createMediaStreamDestination();
    oscillator.connect(destination);
    oscillator.start();
    sender.onicecandidate = (event) => {
      if (event.candidate) void receiver.addIceCandidate(event.candidate);
    };
    receiver.onicecandidate = (event) => {
      if (event.candidate) void sender.addIceCandidate(event.candidate);
    };
    // Bounded, so a loopback that never negotiates fails here and still
    // reaches the cleanup below instead of hanging the suite.
    const received = new Promise((resolve, reject) => {
      receiver.ontrack = resolve;
      setTimeout(() => reject(new Error("loopback track timeout")), 10000);
    });
    sender.addTrack(destination.stream.getAudioTracks()[0], destination.stream);
    try {
      await sender.setLocalDescription(await sender.createOffer());
      await receiver.setRemoteDescription(sender.localDescription);
      await receiver.setLocalDescription(await receiver.createAnswer());
      await sender.setRemoteDescription(receiver.localDescription);
      const { track, receiver: inbound } = await received;
      const audio = document.createElement("audio");
      audio.srcObject = new MediaStream([track]);
      document.body.append(audio);
      await audio.play();
      return await globalThis.__createMediaSampler()(
        {
          mediaStreamTrack: track,
          getRTCStatsReport: () => inbound.getStats(),
        },
        1,
        () => true,
        () => true,
      );
    } finally {
      oscillator.stop();
      destination.stream.getTracks().forEach((track) => track.stop());
      sender.close();
      receiver.close();
      await context.close();
    }
  });
  assert.equal(result.status, "ok", JSON.stringify(result));
  assert.ok(result.bytesReceived > 0);
  assert.ok(result.packetsReceived > 0);
  assert.ok(result.peakRms > 0.001);
});

test("non-Speaking gaps cannot bridge counter deltas", async () => {
  let ticks = 0;
  const f = fixture({
    counts: [0, 100, 100, 100],
    onDelay: () => {
      ticks++;
      if (ticks === 1) f.quiet();
      if (ticks === 2) f.speak();
    },
  });
  const result = await f.run();
  assert.equal(result.status, "transport-silent");
  assert.equal(result.bytesReceived, 0);
});

test("Speaking lost during a receiver await discards the sample and its baseline", async () => {
  let calls = 0;
  const f = fixture({
    onDelay: () => {
      if (calls === 2) f.speak();
    },
  });
  f.track.getRTCStatsReport = async () => {
    calls++;
    if (calls === 2) f.quiet();
    return new Map([
      [
        "inbound",
        {
          type: "inbound-rtp",
          kind: "audio",
          id: "inbound",
          bytesReceived: calls === 1 ? 0 : 100,
          packetsReceived: calls === 1 ? 0 : 10,
        },
      ],
    ]);
  };
  const result = await f.run();
  assert.equal(result.status, "transport-silent");
  assert.equal(result.bytesReceived, 0);
});

test("cancel discards a pending stats observation and closes the context", async () => {
  const f = fixture();
  let stopped = false;
  f.track.getRTCStatsReport = async () => {
    stopped = true;
    return new Map([
      [
        "inbound",
        {
          type: "inbound-rtp",
          kind: "audio",
          id: "inbound",
          bytesReceived: 100,
          packetsReceived: 10,
        },
      ],
    ]);
  };
  const result = await f.sampler(
    f.track,
    1,
    () => true,
    () => true,
    () => stopped,
  );
  assert.equal(result.statsSamples, 0);
  assert.equal(result.status, "unavailable");
  assert.equal(result.reason, "sampling-cancelled");
  assert.equal(f.closed(), 1);
});

test("waiting for Speaking does not spend the sampling window", async () => {
  let ticks = 0;
  const f = fixture({
    timeoutMs: 20,
    speakingWaitMs: 60,
    onDelay: () => {
      if (++ticks === 3) f.speak();
    },
  });
  f.quiet();
  const result = await f.run();
  assert.equal(result.status, "ok");
  assert.equal(result.durationMs, 40);
});

test("unknown aggregate observations remain null and superseded empty generations require a healthy successor", () => {
  const absent = summarizeJimAudio([
    {
      generation: 1,
      status: "unavailable",
      reason: "stats",
      bytesReceived: null,
      packetsReceived: null,
      peakRms: null,
    },
  ]);
  assert.equal(absent.bytesReceived, null);
  assert.equal(absent.packetsReceived, null);
  assert.equal(absent.peakRms, null);
  assert.equal(absent.reason, "stats");
  const superseded = {
    generation: 1,
    status: "superseded",
    reason: "replaced",
    bytesReceived: null,
    packetsReceived: null,
    peakRms: null,
  };
  const healthy = {
    generation: 2,
    status: "ok",
    bytesReceived: 10,
    packetsReceived: 1,
    peakRms: 0.1,
  };
  assert.equal(summarizeJimAudio([superseded, healthy]).status, "ok");
  assert.throws(
    () => assertJimAudio({ generations: [superseded] }),
    /superseded/,
  );
});

// Re-armed sampling after a socket swap. The sample taken before the swap is
// older than the socket under test, so a swap is sampled again from its own
// counter baseline, on the first Speaking transition after it was marked, and
// within a finite number of samples.

function runWith(f, options) {
  return f.sampler(
    f.track,
    9,
    () => true,
    () => true,
    () => false,
    options,
  );
}

test("a re-armed window stays shut until a Speaking transition after the mark", async () => {
  let ticks = 0;
  let edges = 3;
  const f = fixture({
    counts: [0, 10, 20, 30],
    timeoutMs: 1000,
    speakingWaitMs: 1000,
    onDelay: () => {
      if (++ticks === 5) edges = 4;
    },
  });
  let statsCalls = 0;
  const stats = f.track.getRTCStatsReport;
  f.track.getRTCStatsReport = (...args) => {
    statsCalls++;
    return stats(...args);
  };
  const result = await runWith(f, {
    afterEdge: () => edges > 3,
    edgeNow: () => edges,
    maxSamples: 50,
  });
  assert.equal(result.status, "ok");
  assert.equal(result.windowEdge, 4);
  assert.ok(ticks >= 5, "waited out the audio that was already playing");
  assert.equal(statsCalls, 2, "no receiver sample was taken before the edge");
  assert.equal(result.bytesReceived, 10, "the baseline starts at the window");
});

test("a re-armed window that never sees a transition is unavailable and takes no sample", async () => {
  const f = fixture({ timeoutMs: 100, speakingWaitMs: 60 });
  const result = await runWith(f, {
    afterEdge: () => false,
    edgeNow: () => 99,
    maxSamples: 50,
  });
  assert.equal(result.status, "unavailable");
  assert.equal(result.reason, "no-speaking-samples");
  assert.equal(result.samples, 0);
  assert.equal(result.windowEdge, null);
  assert.equal(
    f.closed(),
    0,
    "no audio context for a window that never opened",
  );
});

test("cumulative counters from before the swap are not counted by a re-armed baseline", async () => {
  const f = fixture({ counts: [900000, 900010, 900020] });
  const result = await runWith(f, {
    afterEdge: () => true,
    edgeNow: () => 7,
    maxSamples: 50,
  });
  assert.equal(result.status, "ok");
  assert.equal(result.bytesReceived, 10);
  assert.equal(result.packetsReceived, 10);
  assert.equal(result.windowEdge, 7);
});

test("the sample bound ends a re-armed window that never proves anything", async () => {
  const f = fixture({
    counts: [9],
    timeoutMs: 1_000_000,
    speakingWaitMs: 1_000_000,
  });
  const result = await runWith(f, {
    afterEdge: () => true,
    edgeNow: () => 1,
    maxSamples: 3,
  });
  assert.equal(result.samples, 3);
  assert.equal(result.boundHit, true);
  assert.equal(result.status, "transport-silent");
  const unbounded = await fixture({ counts: [0, 10, 20] }).run();
  assert.equal(unbounded.boundHit, false);
  assert.equal(unbounded.windowEdge, null);
});

async function swapHarness(t) {
  const { EventEmitter } = await import("node:events");
  const names = [
    "LivekitClient",
    "__createMediaSampler",
    "__finishJimAudioProbe",
    "__markJimSwap",
    "__settleJimSwapAudio",
    "__summarizeJimAudio",
    "document",
  ];
  const previous = names.map((name) => [
    name,
    Object.getOwnPropertyDescriptor(globalThis, name),
  ]);
  t.after(() => {
    for (const [name, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
  });
  const events = { TrackSubscribed: "sub", TrackUnsubscribed: "unsub" };
  globalThis.LivekitClient = { Room: EventEmitter, RoomEvent: events };
  // `hold` makes a wait yield to the event loop, so a swap that never sees a
  // transition is still waiting when the test settles it.
  const h = {
    state: "Speaking",
    tick: 0,
    clock: 0,
    calls: 0,
    hold: false,
    rms: 0.1,
    hooks: new Map(),
  };
  const mediaStreamTrack = {};
  let attached = mediaStreamTrack;
  globalThis.document = {
    querySelector: (selector) =>
      selector === "#jim-audio"
        ? { srcObject: { getAudioTracks: () => [attached] } }
        : selector === "#audio-playback-notice"
          ? { hidden: true }
          : { textContent: h.state },
  };
  const makeTrack = (mediaStreamTrack) => ({
    kind: "audio",
    mediaStreamTrack,
    getRTCStatsReport: async () => {
      h.calls++;
      const value = 5_000_000 + h.calls * 100;
      return new Map([
        [
          "inbound",
          {
            type: "inbound-rtp",
            kind: "audio",
            id: "inbound",
            bytesReceived: value,
            packetsReceived: value,
          },
        ],
      ]);
    },
  });
  const track = makeTrack(mediaStreamTrack);
  globalThis.__createMediaSampler = (options) =>
    createMediaSampler({
      ...options,
      now: () => h.clock,
      delay: async (ms) => {
        h.clock += ms;
        h.hooks.get(++h.tick)?.();
        if (h.hold) await new Promise((resolve) => setTimeout(resolve, 1));
      },
      makeContext: () => ({
        state: "running",
        resume: async () => {},
        close: async () => {},
        createMediaStreamSource: () => ({
          connect: () => {},
          disconnect: () => {},
        }),
        createAnalyser: () => ({
          fftSize: 0,
          getFloatTimeDomainData: (values) => values.fill(h.rms),
          disconnect: () => {},
        }),
      }),
      makeStream: () => ({}),
      intervalMs: 10,
    });
  globalThis.__summarizeJimAudio = summarizeJimAudio;
  installMediaProbe();
  const room = new globalThis.LivekitClient.Room();
  const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
  room.emit("sub", track);
  await new Promise((resolve) => queueMicrotask(resolve));
  await flush();
  h.room = room;
  h.track = track;
  h.flush = flush;
  // A second subscription, attached to the page as Jim's track.
  h.resubscribe = async () => {
    const next = makeTrack({});
    attached = next.mediaStreamTrack;
    room.emit("sub", next);
    await new Promise((resolve) => queueMicrotask(resolve));
    return next;
  };
  return h;
}

test("a swap is sampled again on the first Speaking transition after it was marked", async (t) => {
  const h = await swapHarness(t);
  // The first subscription already proved decoded audio before any swap.
  const callsBefore = h.calls;
  assert.ok(callsBefore >= 2);
  // Speaking continues across the mark: that is the old socket's audio.
  assert.deepEqual(globalThis.__markJimSwap("7:1:2"), { armed: true });
  const tickAtMark = h.tick;
  let callsWhileGated = null;
  h.hooks.set(tickAtMark + 4, () => {
    callsWhileGated = h.calls;
    h.state = "Thinking";
  });
  h.hooks.set(tickAtMark + 7, () => {
    h.state = "Speaking";
  });
  await h.flush();
  const rearms = await globalThis.__settleJimSwapAudio(0);
  assert.equal(rearms.length, 1);
  const [one] = rearms;
  assert.equal(one.swap, "7:1:2");
  assert.equal(one.status, "ok");
  assert.ok(one.windowEdge > one.edgesAtMark);
  assert.equal(
    callsWhileGated,
    callsBefore,
    "no receiver sample was taken while the old audio played on",
  );
  assert.ok(one.bytesReceived > 0 && one.bytesReceived < 5_000_000);
  const summary = await globalThis.__finishJimAudioProbe();
  assert.equal(summary.status, "ok");
  assert.equal(summary.rearms.length, 1);
  assert.equal(summary.generations.length, 1);
});

test("a later swap supersedes an earlier one that has not been sampled", async (t) => {
  const h = await swapHarness(t);
  h.hold = true;
  globalThis.__markJimSwap("7:1:2");
  globalThis.__markJimSwap("7:1:3");
  const rearms = await globalThis.__settleJimSwapAudio(0);
  const [first, second] = rearms;
  assert.equal(first.status, "superseded");
  assert.equal(first.reason, "superseded-by-later-swap");
  assert.equal(second.swap, "7:1:3");
  assert.ok(second.seq > first.seq);
  assert.equal(second.reason, "sampling-cancelled");
});

test("a swap still waiting for Speaking is cancelled at settle and reported unavailable", async (t) => {
  const h = await swapHarness(t);
  h.hold = true;
  globalThis.__markJimSwap("7:1:2");
  const rearms = await globalThis.__settleJimSwapAudio(0);
  assert.equal(rearms.length, 1);
  assert.equal(rearms[0].status, "unavailable");
  assert.equal(rearms[0].reason, "sampling-cancelled");
  assert.equal(rearms[0].windowEdge, null);
  // The probe takes no further marks once settled.
  assert.deepEqual(globalThis.__markJimSwap("7:1:3"), { armed: false });
  await h.flush();
});

test("a swap marked with no attached track is recorded, not dropped", async (t) => {
  const h = await swapHarness(t);
  h.room.emit("unsub", h.track);
  assert.deepEqual(globalThis.__markJimSwap("7:1:2"), { armed: false });
  const rearms = await globalThis.__settleJimSwapAudio(0);
  assert.equal(rearms[0].status, "unavailable");
  assert.equal(rearms[0].reason, "no-attached-agent-track");
  assert.throws(() => assertJimAudio({ generations: [] }), /no-attached/);
});

test("marks past the observation limit are recorded as unavailable", async (t) => {
  const h = await swapHarness(t);
  h.hold = true;
  for (let swap = 0; swap < 66; swap++) globalThis.__markJimSwap(`7:1:${swap}`);
  const rearms = await globalThis.__settleJimSwapAudio(0);
  assert.equal(rearms.length, 66);
  assert.equal(rearms.at(-1).reason, "swap-observation-limit");
  await h.flush();
});

test("the summary carries re-armed samples but the track verdict does not read them", () => {
  const silent = {
    swap: "7:1:2",
    status: "decoded-silent",
    reason: "packets-arrived-without-decoded-signal",
  };
  const summary = summarizeJimAudio(
    [{ generation: 1, status: "ok", bytesReceived: 5, packetsReceived: 1 }],
    false,
    [silent],
  );
  assert.deepEqual(summary.rearms, [silent]);
  assert.equal(summary.status, "ok");
  assert.deepEqual(summarizeJimAudio([]).rearms, []);
});

test("a later swap does not cut short a sample that has already opened its window", async (t) => {
  const h = await swapHarness(t);
  h.hold = true;
  // Undecoded, so the first swap's sample stays open until its time bound.
  h.rms = 0;
  globalThis.__markJimSwap("7:1:2");
  const tickAtMark = h.tick;
  h.hooks.set(tickAtMark + 3, () => {
    h.state = "Thinking";
  });
  h.hooks.set(tickAtMark + 6, () => {
    h.state = "Speaking";
  });
  h.hooks.set(tickAtMark + 20, () => {
    // An idle swap, marked while the first one is mid-window.
    globalThis.__markJimSwap("7:1:3");
  });
  const rearms = await globalThis.__settleJimSwapAudio(3000);
  const [first, second] = rearms;
  assert.equal(first.swap, "7:1:2");
  assert.notEqual(first.reason, "superseded-by-later-swap");
  assert.notEqual(first.status, "superseded");
  assert.ok(first.windowEdge > first.edgesAtMark, "its window had opened");
  assert.ok(first.samples > 5, "it kept sampling past the later mark");
  assert.equal(second.swap, "7:1:3");
  assert.equal(second.windowEdge, null);
});

test("a swap whose track was resubscribed is sampled again on the new track from the same mark", async (t) => {
  const h = await swapHarness(t);
  h.hold = true;
  globalThis.__markJimSwap("7:1:2");
  h.room.emit("unsub", h.track);
  const next = await h.resubscribe();
  assert.ok(next);
  const tickNow = h.tick;
  h.hooks.set(tickNow + 5, () => {
    h.state = "Thinking";
  });
  h.hooks.set(tickNow + 8, () => {
    h.state = "Speaking";
  });
  const rearms = await globalThis.__settleJimSwapAudio(5000);
  assert.equal(rearms.length, 1);
  const [one] = rearms;
  assert.equal(one.swap, "7:1:2");
  assert.equal(one.status, "ok");
  assert.ok(one.windowEdge > one.edgesAtMark);
});

test("a swap marked while no track was attached is sampled once one arrives", async (t) => {
  const h = await swapHarness(t);
  h.hold = true;
  h.room.emit("unsub", h.track);
  assert.deepEqual(globalThis.__markJimSwap("7:1:2"), { armed: false });
  await h.resubscribe();
  const tickNow = h.tick;
  h.hooks.set(tickNow + 5, () => {
    h.state = "Thinking";
  });
  h.hooks.set(tickNow + 8, () => {
    h.state = "Speaking";
  });
  const [one] = await globalThis.__settleJimSwapAudio(5000);
  assert.equal(one.status, "ok");
  assert.equal(one.swap, "7:1:2");
});

test("a track that never returns leaves the swap unsampled and failing", async (t) => {
  const h = await swapHarness(t);
  h.hold = true;
  globalThis.__markJimSwap("7:1:2");
  h.room.emit("unsub", h.track);
  const [one] = await globalThis.__settleJimSwapAudio(200);
  assert.notEqual(one.status, "ok");
  assert.equal(one.windowEdge, null);
});
