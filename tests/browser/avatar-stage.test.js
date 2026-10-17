import { test } from "node:test";
import assert from "node:assert/strict";
import {
  AVATAR_FRAME_MS,
  createAvatar,
  createPlayoutMonitor,
  createRenderBudget,
} from "../../web/avatar/avatar.js";
import { stubGlobal } from "./source.js";

test("render budget tolerates healthy rendering and isolated stalls", () => {
  const budget = createRenderBudget();
  let at = 0;
  for (let i = 0; i < 500; i++) {
    at += i === 20 ? 700 : AVATAR_FRAME_MS;
    assert.equal(budget.sample(at, i === 20 ? 700 : 2), false);
  }
});

test("one long stall early in a window does not retire the avatar", () => {
  // A mean over five frames let one 2 s draw outweigh four healthy ones.
  const budget = createRenderBudget();
  const samples = [0, 16, 32, 48, 64].map((at) => [at, 2]);
  samples[4][1] = 2000;
  samples.push([2064, 2]);
  for (let at = 2097; at < 6000; at += AVATAR_FRAME_MS) samples.push([at, 2]);
  assert.deepEqual(
    samples.filter(([at, cost]) => budget.sample(at, cost)),
    [],
    "one stall is not sustained pressure",
  );
});

test("render budget detects slow cadence and expensive draws separately", () => {
  for (const [gap, cost] of [
    [50, 2],
    [AVATAR_FRAME_MS, 12],
  ]) {
    const budget = createRenderBudget();
    let tripped = false;
    // Two windows: one overloaded window alone never decides.
    for (let at = 0; at <= 4600; at += gap) {
      if (budget.sample(at, cost)) {
        tripped = true;
        break;
      }
    }
    assert.equal(tripped, true, `${gap} ms cadence, ${cost} ms draw`);
  }
});

test("one overloaded window at start-up does not retire the avatar", () => {
  // The trace measured on a GPU under load: eleven start-up draws of about
  // 18 ms spread over the first two seconds while the page joined the room,
  // then 3 ms draws at the cap for the rest of the interview.
  const budget = createRenderBudget();
  let at = 0;
  let tripped = false;
  for (let i = 0; i < 11; i++) {
    at += 200;
    tripped ||= budget.sample(at, 18);
  }
  for (let i = 0; i < 400 && !tripped; i++) {
    at += AVATAR_FRAME_MS;
    tripped = budget.sample(at, 3);
  }
  assert.equal(tripped, false);
});

test("a frozen page is retired within ten seconds however slow it is", () => {
  // Ten frames per window made the wait grow with the freeze: two windows took
  // 21 s at 1 fps and 42 s at 0.5 fps.
  for (const fps of [3.3, 1, 0.5]) {
    const budget = createRenderBudget();
    let at = 0;
    let trippedAt = null;
    while (trippedAt === null && at < 60000) {
      at += 1000 / fps;
      if (budget.sample(at, 2)) trippedAt = at;
    }
    assert.ok(
      trippedAt !== null && trippedAt <= 10000,
      `${fps} fps: ${trippedAt}`,
    );
  }
});

test("a page frozen in bursts is retired, not only one frozen evenly", () => {
  // Counting slow frames saw a quarter of these slow and never retired them,
  // though each pattern spends most of its time stalled.
  for (const [quick, stallMs] of [
    [3, 1500],
    [4, 1000],
    [2, 700],
    [6, 500],
  ]) {
    const budget = createRenderBudget();
    let at = 0;
    let trippedAt = null;
    while (trippedAt === null && at < 60000) {
      for (let i = 0; i < quick && trippedAt === null; i++) {
        at += AVATAR_FRAME_MS;
        if (budget.sample(at, 3)) trippedAt = at;
      }
      at += stallMs;
      if (trippedAt === null && budget.sample(at, 3)) trippedAt = at;
    }
    assert.ok(
      trippedAt !== null && trippedAt <= 10000,
      `${quick} quick frames and a ${stallMs} ms stall: ${trippedAt}`,
    );
  }
});

test("a pause breaks a run of overloaded windows", () => {
  const budget = createRenderBudget();
  let at = 0;
  for (; at < 2200; at += 50) assert.equal(budget.sample(at, 12), false);
  budget.reset();
  at += 60000;
  const end = at + 2200;
  for (; at < end; at += 50) assert.equal(budget.sample(at, 12), false);
});

test("render budget holds at the 30 fps cap with occasional missed vsyncs", () => {
  // At 60 Hz the cap draws every 33 ms and a missed vsync is 50 ms. A third of
  // frames missing is load the candidate can live with; most of them is not.
  for (const [missEvery, trips] of [
    [3, false],
    [1, true],
  ]) {
    const budget = createRenderBudget();
    let at = 0;
    let tripped = false;
    for (let i = 0; i < 300 && !tripped; i++) {
      at += i % missEvery === 0 ? 50 : 1000 / 30;
      tripped = budget.sample(at, 2);
    }
    assert.equal(tripped, trips, `a missed vsync every ${missEvery} frames`);
  }
});

test("render budget resets across pauses and ignores first-frame setup", () => {
  const budget = createRenderBudget();
  for (let at = 0; at < 1900; at += 50)
    assert.equal(budget.sample(at, 12), false);
  budget.reset();
  assert.equal(budget.sample(100000, 500), false);
  for (let at = 100016; at < 104000; at += 16)
    assert.equal(budget.sample(at, 2), false);
});

// Cumulative stats as the receiver reports them: delay in seconds summed over
// every emitted sample, at 48 kHz.
function stats(windows) {
  let delay = 0;
  let emitted = 0;
  return windows.map((ms) => {
    if (ms !== null) {
      emitted += 96000;
      delay += (96000 * ms) / 1000;
    }
    return { jitterBufferDelay: delay, jitterBufferEmittedCount: emitted };
  });
}

test("playout monitor trips on sustained late audio, not on one burst", () => {
  // The first sample only sets the baseline.
  const cases = [
    [[80, 90, 85, 100, 95], false],
    [[80, 900, 90, 700, 85], false],
    [[80, 450, 600], true],
    // The issue's 38-57 fps case: fast frames, audio still ~400 ms late.
    [[90, 410, 410], true],
  ];
  for (const [windows, trips] of cases) {
    const monitor = createPlayoutMonitor();
    const verdicts = stats(windows).map((entry) => monitor.sample(entry));
    assert.equal(verdicts.some(Boolean), trips, windows.join(","));
  }
});

test("playout monitor waits through silence and restarts on a new receiver", () => {
  const monitor = createPlayoutMonitor();
  const [base, late, silent, later] = stats([80, 500, null, 500]);
  assert.equal(monitor.sample(base), false);
  assert.equal(monitor.sample(late), false);
  // Nothing emitted is no evidence: the streak neither grows nor resets.
  assert.equal(monitor.sample(silent), false);
  assert.equal(monitor.sample(later), true);

  const fresh = createPlayoutMonitor();
  const [a, b] = stats([80, 500]);
  assert.equal(fresh.sample(a), false);
  assert.equal(fresh.sample(b), false);
  // Counters that went backwards are a new receiver, not a negative delay.
  assert.equal(
    fresh.sample({ jitterBufferDelay: 1, jitterBufferEmittedCount: 1 }),
    false,
  );
  assert.equal(fresh.sample(null), false);
  assert.equal(fresh.sample({ jitterBufferDelay: 1 }), false);
});

// A browser just large enough for stage.js: animation frames, intervals and
// the clock are all driven by hand, so nothing here waits on real time.
function installBrowser(t) {
  const listeners = new Map();
  const frames = new Map();
  const intervals = new Map();
  let handle = 0;
  const env = {
    now: 0,
    display: "block",
    listeners,
    frames,
    intervals,
    document: {
      hidden: false,
      activeElement: null,
      addEventListener: (name, fn) => listeners.set(`document:${name}`, fn),
    },
  };
  const install = (name, value) => stubGlobal(t, name, value);
  install("document", env.document);
  install("window", {
    getComputedStyle: () => ({ display: env.display }),
    // The stage's width observer, driven by the test through `setWidth`.
    ResizeObserver: class {
      constructor(callback) {
        env.observeWidth = (width) => callback([{ contentRect: { width } }]);
      }
      observe() {}
    },
    addEventListener: (name, fn) => listeners.set(`window:${name}`, fn),
    AudioContext: class {
      constructor() {
        env.contexts = (env.contexts ?? 0) + 1;
        this.state = "running";
      }
      createMediaStreamSource() {
        return {
          connect() {},
          disconnect() {
            env.disconnected = (env.disconnected ?? 0) + 1;
          },
        };
      }
      createAnalyser() {
        // Silence by default; a test sets `env.sample` to hear Jim speak.
        return {
          getByteTimeDomainData: (samples) => samples.fill(env.sample ?? 128),
        };
      }
      close() {
        env.closed = (env.closed ?? 0) + 1;
        return Promise.resolve();
      }
    },
  });
  install("performance", { now: () => env.now });
  install(
    "MediaStream",
    class {
      constructor(tracks) {
        this.tracks = tracks;
      }
    },
  );
  install("requestAnimationFrame", (fn) => {
    frames.set(++handle, fn);
    return handle;
  });
  install("cancelAnimationFrame", (id) => frames.delete(id));
  install("setInterval", (fn) => {
    intervals.set(++handle, fn);
    return handle;
  });
  install("clearInterval", (id) => intervals.delete(id));
  return env;
}

let stageCopies = 0;

async function mountStage(
  env,
  { load = "ready", drawCost = () => 0, width = 168 } = {},
) {
  // A fresh module per scenario: the stage's state is module-level by design.
  const stage = await import(`../../web/avatar/stage.js?copy=${++stageCopies}`);
  const counts = { draws: 0, disposed: 0, loads: 0, focused: 0 };
  let click;
  let finishLoading;
  let signal;
  const nodes = {
    jimStage: { inert: false },
    jimAvatar: { dataset: {}, clientWidth: width },
    jimAvatarNote: {
      textContent: "",
      focus: () => {
        counts.focused++;
        env.document.activeElement = nodes.jimAvatarNote;
      },
    },
    jimAvatarStatus: { textContent: "" },
    hideAvatar: {
      hidden: true,
      addEventListener: (_, fn) => {
        click = fn;
      },
    },
  };
  stage.initAvatarStage({
    nodes,
    onShowAvatar: (track, participant) =>
      stage.attachAvatarAnalyser(track, participant),
    loadModel: async (_mount, abort) => {
      counts.loads++;
      signal = abort;
      if (load === "unavailable") throw new Error("model unavailable");
      const model = {
        apply: (pose) => {
          counts.draws++;
          counts.pose = pose;
          env.now += drawCost();
        },
        dispose: () => counts.disposed++,
      };
      if (load === "pending")
        return new Promise((resolve) => {
          finishLoading = () => resolve(model);
        });
      return model;
    },
  });
  // As the page does: the preflight covers the stage from the first paint.
  stage.setStageCovered(true);
  const settle = () => new Promise((resolve) => setImmediate(resolve));
  const tick = (gap = 1000 / 60) => {
    env.now += gap;
    const pending = [...env.frames.values()];
    env.frames.clear();
    for (const callback of pending) callback(env.now);
  };
  const poll = async () => {
    for (const fn of env.intervals.values()) await fn();
    await settle();
  };
  return {
    stage,
    nodes,
    counts,
    tick,
    poll,
    settle,
    click: () => click(),
    finishLoading: () => finishLoading(),
    signal: () => signal,
    // The page's order: the stage starts under the preflight overlay, which
    // closes and hands the loop back through setStageCovered.
    async start() {
      stage.startAvatar();
      await settle();
    },
    closePreflight() {
      stage.setStageCovered(false);
    },
    /// The mount crossing the CSS breakpoint, as its observer reports it.
    setWidth(width) {
      env.observeWidth(width);
    },
    /// The tab hiding or coming back.
    setHidden(hidden) {
      env.document.hidden = hidden;
      env.listeners.get("document:visibilitychange")();
    },
  };
}

test("stage waits for the preflight before drawing or reachable controls", async (t) => {
  const env = installBrowser(t);
  const s = await mountStage(env);
  await s.start();
  assert.equal(s.nodes.jimAvatar.dataset.avatarState, "ready");
  // Nothing is drawn and nothing is scheduled behind the overlay, and the
  // stage is inert, so its Hide control cannot be reached there unseen.
  assert.equal(s.nodes.jimStage.inert, true);
  assert.equal(env.frames.size, 0);
  assert.equal(s.counts.draws, 0);
  assert.equal(env.intervals.size, 0, "no stats are read behind it either");
  s.closePreflight();
  assert.equal(s.nodes.jimStage.inert, false);
  assert.equal(s.nodes.hideAvatar.hidden, false);
  assert.equal(s.counts.draws, 1);
  assert.equal(env.frames.size, 1);
  s.stage.stopAvatar();
});

for (const hz of [60, 75, 100, 120, 144]) {
  test(`stage draws 30 frames a second at ${hz} Hz`, async (t) => {
    // A minimum gap between draws drew every third vsync at 100 Hz, 33 fps.
    const env = installBrowser(t);
    const s = await mountStage(env);
    await s.start();
    s.closePreflight();
    const before = s.counts.draws;
    for (let i = 0; i < hz * 4; i++) s.tick(1000 / hz);
    const fps = (s.counts.draws - before) / 4;
    assert.ok(fps >= 29 && fps <= 30.5, `${hz} Hz drew ${fps} fps`);
    s.stage.stopAvatar();
  });
}

test("a narrow stage stops the loop and widening brings it back", async (t) => {
  const env = installBrowser(t);
  const s = await mountStage(env);
  await s.start();
  s.closePreflight();
  s.setWidth(0);
  s.tick(AVATAR_FRAME_MS);
  assert.equal(env.frames.size, 0, "a hidden stage schedules nothing");
  assert.equal(env.intervals.size, 0, "and reads no stats");
  const drawn = s.counts.draws;
  s.setWidth(0);
  assert.equal(env.frames.size, 0, "still narrow, still paused");
  s.setWidth(168);
  assert.equal(s.counts.draws, drawn + 1);
  assert.equal(env.frames.size, 1);
  // A long pause is not charged to the budget as one slow frame.
  env.now += 60000;
  for (let i = 0; i < 200; i++) s.tick(AVATAR_FRAME_MS);
  assert.equal(s.counts.disposed, 0);
  s.stage.stopAvatar();
});

test("a stage that starts narrow builds the avatar once it widens", async (t) => {
  const env = installBrowser(t);
  env.display = "none";
  const s = await mountStage(env, { width: 0 });
  await s.start();
  assert.equal(s.counts.loads, 0, "nothing is built below the breakpoint");
  env.display = "block";
  s.setWidth(168);
  await s.settle();
  assert.equal(s.counts.loads, 1);
  assert.equal(s.nodes.jimAvatar.dataset.avatarState, "ready");
  s.stage.stopAvatar();
});

test("widening after the interview ends builds nothing", async (t) => {
  const env = installBrowser(t);
  env.display = "none";
  const s = await mountStage(env, { width: 0 });
  await s.start();
  s.stage.stopAvatar();
  env.display = "block";
  s.setWidth(168);
  await s.settle();
  assert.equal(s.counts.loads, 0);
});

/// The one entry of a receiver's stats report the playout watch reads.
function inboundAudio(entry) {
  return new Map([["a", { type: "inbound-rtp", kind: "audio", ...entry }]]);
}

/// Jim's track, answering each stats read with the next of `reports`.
function statsTrack(reports) {
  return {
    mediaStreamTrack: {},
    getRTCStatsReport: async () => inboundAudio(reports.shift()),
  };
}

/// Three 1 s stretches of fast 60 Hz frames, each followed by a stats read:
/// never enough to trip the render budget, so any retirement is the audio's.
async function drawAndPoll(s) {
  for (let round = 0; round < 3; round++) {
    for (let i = 0; i < 60; i++) s.tick(1000 / 60);
    await s.poll();
  }
}

/// Draws expensive frames until the render budget retires the avatar.
function exhaustBudget(s, setCost) {
  setCost(12);
  for (let i = 0; i < 200 && s.counts.disposed === 0; i++)
    s.tick(AVATAR_FRAME_MS);
}

test("stage retires the avatar when Jim's audio is late", async (t) => {
  const env = installBrowser(t);
  const warnings = [];
  t.mock.method(console, "warn", (line) => warnings.push(String(line)));
  const s = await mountStage(env);
  const reports = stats([90, 410, 410]);
  s.stage.attachAvatarAnalyser(statsTrack(reports), {
    identity: "interviewer-jim",
  });
  await s.start();
  s.closePreflight();
  assert.equal(env.intervals.size, 1);
  await drawAndPoll(s);
  assert.equal(s.nodes.jimAvatar.dataset.avatarState, "degraded");
  assert.equal(s.counts.disposed, 1);
  assert.equal(env.intervals.size, 0, "the stats poll stops with it");
  assert.equal(env.frames.size, 0);
  assert.equal(env.closed, 1);
  assert.match(s.nodes.jimAvatarStatus.textContent, /keep the interview/);
  // The number that decided is logged, so a real session can be calibrated.
  assert.ok(
    warnings.some((line) =>
      /^codetrial avatar_playout_delayed 410ms$/.test(line),
    ),
    warnings.join("\n"),
  );
});

test("late audio retires the avatar even with no analyser for lip sync", async (t) => {
  // A browser that throws on AudioContext loses lip sync. The playout watch
  // follows Jim's track, not the analyser, so it must still see him late.
  const env = installBrowser(t);
  globalThis.window.AudioContext = class {
    constructor() {
      throw new Error("no audio context here");
    }
  };
  const s = await mountStage(env);
  s.stage.attachAvatarAnalyser(statsTrack(stats([90, 410, 410])), {
    identity: "interviewer-jim",
  });
  await s.start();
  s.closePreflight();
  await drawAndPoll(s);
  assert.equal(s.nodes.jimAvatar.dataset.avatarState, "degraded");
});

test("a width change while drawing keeps an overloaded streak", async (t) => {
  // The observer calls back on every width change, running loop or not. A
  // budget reset there cleared the first of two overloaded windows, so a page
  // being resized while overloaded was never retired.
  const env = installBrowser(t);
  let cost = 0;
  const s = await mountStage(env, { drawCost: () => cost });
  await s.start();
  s.closePreflight();
  cost = 12;
  for (let i = 0; i < 70; i++) s.tick(AVATAR_FRAME_MS);
  assert.equal(s.counts.disposed, 0, "one overloaded window alone");
  s.setWidth(168);
  for (let i = 0; i < 70 && s.counts.disposed === 0; i++)
    s.tick(AVATAR_FRAME_MS);
  assert.equal(s.nodes.jimAvatar.dataset.avatarState, "degraded");
});

test("late audio while nothing is drawn is not the avatar's doing", async (t) => {
  const env = installBrowser(t);
  const s = await mountStage(env);
  const reports = stats([90, 900, 900, 900, 900]);
  s.stage.attachAvatarAnalyser(statsTrack(reports), {
    identity: "interviewer-jim",
  });
  await s.start();
  s.closePreflight();
  s.setWidth(0);
  s.tick(AVATAR_FRAME_MS);
  for (let round = 0; round < 5; round++) await s.poll();
  assert.equal(s.nodes.jimAvatar.dataset.avatarState, "ready");
  // The stats timer runs only while frames are drawn.
  assert.equal(reports.length, 5, "a paused stage does not ask");
  s.stage.stopAvatar();
});

/// A track whose stats answers are handed out by the test, one at a time.
function deferredStatsTrack() {
  const pending = [];
  return {
    pending,
    track: {
      mediaStreamTrack: {},
      getRTCStatsReport: () =>
        new Promise((resolve) => {
          pending.push((entry) => resolve(inboundAudio(entry)));
        }),
    },
  };
}

for (const interruption of ["replaced", "paused", "hidden"]) {
  test(`a stats answer that outlives a ${interruption} track is discarded`, async (t) => {
    // Baseline, one late window, and a third answer held in flight while the
    // track is replaced, the stage goes narrow, or the tab hides. Read, that
    // answer either completed the pair outright or became a baseline whose
    // counters from before the interruption charged all of it to the next
    // window. Either way two more late windows after it retired the avatar;
    // dropped, those two are a fresh baseline and a single late window.
    const env = installBrowser(t);
    const s = await mountStage(env);
    const jim = { identity: "interviewer-jim" };
    let jimTrack = deferredStatsTrack();
    s.stage.attachAvatarAnalyser(jimTrack.track, jim);
    await s.start();
    s.closePreflight();
    const reports = stats([90, 500, 500, 500, 500]);
    const window = async () => {
      s.tick(AVATAR_FRAME_MS);
      const polled = s.poll();
      jimTrack.pending.shift()(reports.shift());
      await polled;
    };
    await window();
    await window();

    s.tick(AVATAR_FRAME_MS);
    const polled = s.poll();
    const stale = jimTrack.pending.shift();
    if (interruption === "replaced") {
      jimTrack = deferredStatsTrack();
      s.stage.attachAvatarAnalyser(jimTrack.track, jim);
    } else if (interruption === "paused") {
      s.setWidth(0);
      s.tick(AVATAR_FRAME_MS);
    } else {
      s.setHidden(true);
    }
    stale(reports.shift());
    await polled;

    // Back to drawing.
    if (interruption === "paused") s.setWidth(168);
    else if (interruption === "hidden") {
      s.setHidden(false);
    }
    await window();
    await window();
    assert.equal(s.nodes.jimAvatar.dataset.avatarState, "ready");
    s.stage.stopAvatar();
  });
}

test("a poll that starts while hidden takes no baseline", async (t) => {
  // Draw, hide, and let the stats poll fire while hidden. Read then, its answer
  // is a baseline the first window after the tab returns measures the whole
  // hidden stretch from, and one more late window retires the avatar.
  const env = installBrowser(t);
  const s = await mountStage(env);
  s.stage.attachAvatarAnalyser(statsTrack(stats([90, 500, 500, 500])), {
    identity: "interviewer-jim",
  });
  await s.start();
  s.closePreflight();
  s.tick(AVATAR_FRAME_MS);
  await s.poll();
  s.tick(AVATAR_FRAME_MS);
  s.setHidden(true);
  await s.poll();
  s.setHidden(false);
  for (let i = 0; i < 2; i++) {
    s.tick(AVATAR_FRAME_MS);
    await s.poll();
  }
  assert.equal(s.nodes.jimAvatar.dataset.avatarState, "ready");
  s.stage.stopAvatar();
});

for (const trigger of ["budget", "budget-while-focused"]) {
  test(`stage retires the renderer for good after ${trigger}`, async (t) => {
    const env = installBrowser(t);
    let cost = 0;
    const s = await mountStage(env, { drawCost: () => cost });
    const participant = { identity: "interviewer-jim" };
    s.stage.attachAvatarAnalyser({ mediaStreamTrack: {} }, participant);
    assert.equal(env.contexts, 1);
    await s.start();
    s.closePreflight();
    const automatic = trigger.startsWith("budget");
    const focused = trigger.endsWith("while-focused");
    if (focused) env.document.activeElement = s.nodes.hideAvatar;
    if (automatic) exhaustBudget(s, (next) => (cost = next));
    else s.click();
    assert.equal(s.counts.disposed, 1);
    assert.equal(env.frames.size, 0);
    assert.equal(s.nodes.hideAvatar.hidden, true);
    assert.equal(
      s.nodes.jimAvatar.dataset.avatarState,
      automatic ? "degraded" : "stopped",
    );
    // Said once: by focus for a manual hide, or when the hidden button held
    // focus, and by the status region otherwise.
    const byFocus = !automatic || focused;
    assert.equal(s.counts.focused, byFocus ? 1 : 0);
    assert.equal(s.nodes.jimAvatarStatus.textContent === "", byFocus);
    assert.equal(env.closed, 1);
    assert.equal(env.disconnected, 1);

    const retired = s.counts.draws;
    s.stage.attachAvatarAnalyser({ mediaStreamTrack: {} }, participant);
    assert.equal(env.contexts, 1, "no analyser is rebuilt");
    // Every door the page has back into the loop stays shut.
    s.stage.startAvatar();
    s.stage.setStageCovered(false);
    env.listeners.get("document:visibilitychange")();
    s.tick();
    assert.equal(s.counts.draws, retired);
    assert.equal(s.counts.loads, 1);
    s.stage.stopAvatar();
    assert.equal(s.counts.disposed, 1);
    assert.equal(
      s.nodes.jimAvatar.dataset.avatarState,
      automatic ? "degraded" : "stopped",
      "a later teardown keeps the reason",
    );
  });
}

for (const focused of [false, true]) {
  test(`candidate Hide can show the avatar again${focused ? " with focus" : ""}`, async (t) => {
    const env = installBrowser(t);
    const s = await mountStage(env);
    const participant = { identity: "interviewer-jim" };
    s.stage.attachAvatarAnalyser({ mediaStreamTrack: {} }, participant);
    await s.start();
    s.closePreflight();
    if (focused) env.document.activeElement = s.nodes.hideAvatar;

    s.click();
    assert.equal(s.nodes.hideAvatar.hidden, false);
    assert.equal(s.nodes.hideAvatar.textContent, "Show avatar");
    assert.equal(s.nodes.jimAvatar.dataset.avatarState, "stopped");
    assert.equal(s.counts.disposed, 1);
    assert.equal(env.frames.size, 0);
    // Focus is left where the candidate put it, so the same key undoes the
    // Hide, and the status region is what says the avatar went away.
    assert.equal(s.counts.focused, 0, "focus is not moved to the note");
    assert.equal(
      env.document.activeElement,
      focused ? s.nodes.hideAvatar : null,
    );
    assert.match(s.nodes.jimAvatarStatus.textContent, /turned off/);

    s.click();
    assert.equal(s.nodes.jimAvatarStatus.textContent, "");
    assert.equal(s.nodes.jimAvatarNote.textContent, "Jim is here by voice.");
    for (let attempt = 0; attempt < 5; attempt++) {
      if (s.nodes.jimAvatar.dataset.avatarState === "ready") break;
      await s.settle();
    }
    assert.equal(s.nodes.hideAvatar.hidden, false);
    assert.equal(s.nodes.hideAvatar.textContent, "Hide avatar");
    assert.equal(s.nodes.jimAvatar.dataset.avatarState, "ready");
    assert.equal(s.counts.loads, 2);
    assert.equal(s.counts.disposed, 1);
    assert.equal(env.contexts, 2, "Show rebuilds the released analyser");
    assert.equal(env.disconnected, 1, "Hide released the first analyser");
    s.stage.stopAvatar();
    assert.equal(env.disconnected, 2, "leaving releases the rebuilt analyser");
  });
}

test("Show hands the rebuilt avatar Jim's latest state", async (t) => {
  const env = installBrowser(t);
  const s = await mountStage(env);
  const participant = { identity: "interviewer-jim" };
  s.stage.attachAvatarAnalyser({ mediaStreamTrack: {} }, participant);
  await s.start();
  s.closePreflight();
  // Loud enough that an open mouth would show: a zero mouth below is the
  // mute, not silence.
  env.sample = 255;
  s.click();
  // Reported while hidden, with no model to tell: the page sends each state
  // once, on the change, so nothing would repeat it after Show.
  s.stage.setAvatarExpression("thinking");
  s.stage.setAvatarSpeaking(false);
  s.click();
  await s.settle();
  assert.equal(s.nodes.jimAvatar.dataset.avatarState, "ready");
  s.tick(AVATAR_FRAME_MS);
  assert.equal(s.counts.pose.agentState, "thinking");
  assert.equal(s.counts.pose.mouth, 0, "the mute survives the rebuild");
  s.stage.setAvatarSpeaking(true);
  s.tick(AVATAR_FRAME_MS);
  assert.ok(s.counts.pose.mouth > 0, "and the samples were loud");
  s.stage.stopAvatar();
});

test("a stage that widens late builds with the state reported while narrow", async (t) => {
  const env = installBrowser(t);
  env.display = "none";
  const s = await mountStage(env, { width: 0 });
  await s.start();
  s.closePreflight();
  s.stage.setAvatarExpression("speaking");
  env.display = "block";
  s.setWidth(168);
  await s.settle();
  assert.equal(s.counts.loads, 1);
  s.tick(AVATAR_FRAME_MS);
  assert.equal(s.counts.pose.agentState, "speaking");
  s.stage.stopAvatar();
});

test("final stop removes the manual Show action", async (t) => {
  const env = installBrowser(t);
  const s = await mountStage(env);
  await s.start();
  s.closePreflight();
  s.click();
  assert.equal(s.nodes.hideAvatar.hidden, false);
  assert.equal(s.nodes.hideAvatar.textContent, "Show avatar");
  s.stage.stopAvatar();
  assert.equal(s.nodes.hideAvatar.hidden, true);
  s.click();
  assert.equal(s.counts.loads, 1);
});

test("Hide during loading aborts the load and nothing is built", async (t) => {
  const env = installBrowser(t);
  const s = await mountStage(env, { load: "pending" });
  await s.start();
  s.closePreflight();
  assert.equal(s.nodes.hideAvatar.hidden, false, "offered while loading");
  assert.equal(s.signal().aborted, false);
  s.click();
  assert.equal(s.signal().aborted, true, "the loader is told to stop");
  assert.equal(s.counts.focused, 0);
  assert.match(s.nodes.jimAvatarStatus.textContent, /turned off/);
  // A loader that finished anyway is still disposed and never drawn.
  s.finishLoading();
  await s.settle();
  assert.equal(s.counts.disposed, 1);
  assert.equal(s.counts.draws, 0);
  assert.equal(env.frames.size, 0);
  assert.equal(env.intervals.size, 0);
  assert.equal(s.nodes.jimAvatar.dataset.avatarState, "stopped");
  assert.match(s.nodes.jimAvatarNote.textContent, /turned off/);
  s.stage.startAvatar();
  assert.equal(s.counts.loads, 1);
  s.click();
  await s.settle();
  assert.equal(s.counts.loads, 2);
  // Settled and stopped, or the load deadline's timer holds the test process
  // open for its whole minute.
  s.finishLoading();
  await s.settle();
  s.stage.stopAvatar();
});

test("an unavailable model hides the control and polls nothing", async (t) => {
  const env = installBrowser(t);
  const s = await mountStage(env, { load: "unavailable" });
  await s.start();
  s.closePreflight();
  assert.equal(s.nodes.jimAvatar.dataset.avatarState, "unavailable");
  assert.equal(s.nodes.hideAvatar.hidden, true);
  assert.equal(env.intervals.size, 0);
  assert.match(s.nodes.jimAvatarNote.textContent, /unavailable/);
  s.stage.stopAvatar();
});

test("the stage is initialized once", async (t) => {
  const env = installBrowser(t);
  const s = await mountStage(env);
  assert.throws(() => s.stage.initAvatarStage({ nodes: s.nodes }), /once/);
});

test("a load aborted by Hide or leaving is not logged as unavailable", async (t) => {
  // That console line tells a broken model from an absent one. An AbortError
  // from ending the avatar on purpose is neither, so it stays out of it; a
  // load that genuinely times out still says so.
  const warnings = [];
  t.mock.method(console, "warn", (...args) => warnings.push(args.join(" ")));
  const aborted = createAvatar({
    mount: { dataset: {} },
    // As the real loader does through throwIfAborted: `createAvatar` calls it
    // a microtask late, so a destroy() right after creation has already
    // aborted the signal by then, and a listener alone would never hear it.
    loadModel: (signal) =>
      new Promise((_resolve, reject) => {
        const abort = () => reject(new DOMException("aborted", "AbortError"));
        if (signal.aborted) abort();
        else signal.addEventListener("abort", abort);
      }),
    // A backstop, so a regression costs 50 ms here and not the 60 s default.
    timeoutMs: 50,
  });
  aborted.destroy();
  assert.equal(await aborted.ready, false);
  assert.equal(aborted.state(), "stopped");
  assert.deepEqual(warnings, []);

  const late = createAvatar({
    mount: { dataset: {} },
    loadModel: () => new Promise(() => {}),
    timeoutMs: 1,
  });
  assert.equal(await late.ready, false);
  assert.equal(warnings.length, 1);
  assert.match(warnings[0], /codetrial avatar_unavailable/);
});

test("a load that loses the timeout race is told to stop", async () => {
  // The recording page shares createAvatar, so the abort lives there and not
  // in the stage: a download that outlives the panel's patience is cancelled,
  // not merely disposed on arrival.
  let signal;
  const avatar = createAvatar({
    mount: { dataset: {} },
    loadModel: (abort) => {
      signal = abort;
      return new Promise(() => {});
    },
    timeoutMs: 1,
  });
  assert.equal(await avatar.ready, false);
  assert.equal(signal.aborted, true);
  assert.equal(avatar.state(), "unavailable");
});
