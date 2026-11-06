import { test } from "node:test";
import assert from "node:assert/strict";

import owner from "../../scripts/swap-audio-owner.cjs";
import * as fixtures from "./soak-judge-fixtures.js";

const { judgeSettledSwapAudio, startSwapMarker } = owner;

/// A page that is only `evaluate`: it runs the function it is given against a
/// stand-in global, so what the marker and the settle ask of the page is
/// observable without a browser.
function fakePage({ failOn = null } = {}) {
  const calls = [];
  const page = {
    calls,
    async evaluate(fn, arg) {
      const marks = [];
      const stand = {
        __markJimSwap: (key) => {
          if (key === failOn) throw new Error(`page refused ${key}`);
          marks.push(key);
        },
        __settleJimSwapAudio: async (waitMs) => {
          calls.push(["settle", waitMs]);
          return page.rearms ?? [];
        },
      };
      const real = globalThis.__markJimSwap;
      const realSettle = globalThis.__settleJimSwapAudio;
      Object.assign(globalThis, stand);
      try {
        const out = await fn(arg);
        marks.forEach((key) => calls.push(["mark", key]));
        return out;
      } finally {
        globalThis.__markJimSwap = real;
        globalThis.__settleJimSwapAudio = realSettle;
      }
    },
  };
  return page;
}

/// A clock the test steps by hand instead of waiting 250 ms for.
function manualClock() {
  const waiting = [];
  const delays = [];
  return {
    delays,
    wait: (ms) => {
      delays.push(ms);
      return new Promise((resolve) => waiting.push(resolve));
    },
    async tick() {
      const due = waiting.splice(0);
      due.forEach((resolve) => resolve());
      // Let the loop run its body before the test looks.
      await new Promise((resolve) => setImmediate(resolve));
    },
  };
}

// Swap keys are the words "swap:KEY" in the text; the parser under test is the
// soak judge's, covered by its own tests.
const keysOf = (text) =>
  [...text.matchAll(/swap:(\S+)/g)].map((match) => match[1]);

/// Ends the loop the way production does: its pending wait runs out, which the
/// test's clock does by hand.
async function ending(clock, promise) {
  await clock.tick();
  return promise;
}

function marker(overrides = {}) {
  const clock = manualClock();
  const agentOutput = [];
  let pending = "";
  const page = overrides.page ?? fakePage();
  const handle = startSwapMarker({
    page,
    roomName: "room",
    agentOutput,
    newLines: () => {
      const text = pending;
      pending = "";
      return text;
    },
    keysOf,
    wait: clock.wait,
    ...overrides.options,
  });
  return {
    clock,
    page,
    handle,
    agentOutput,
    log: (text) => {
      pending += text;
    },
  };
}

test("each installed swap is marked once, in the order it was read", async () => {
  const { clock, page, handle, log, agentOutput } = marker();
  agentOutput.push("swap:7:1:1\n");
  log("swap:7:1:2\nswap:7:1:1\n");
  await clock.tick();
  agentOutput.push("swap:7:1:3\n");
  await clock.tick();
  await clock.tick();
  await ending(clock, handle.stop());
  assert.deepEqual(page.calls, [
    ["mark", "7:1:1"],
    ["mark", "7:1:2"],
    ["mark", "7:1:3"],
  ]);
});

test("the loop polls on its own 250 ms clock", async () => {
  const { clock, handle } = marker();
  await clock.tick();
  await ending(clock, handle.stop());
  assert.ok(clock.delays.length >= 1);
  assert.ok(
    clock.delays.every((ms) => ms === 250),
    clock.delays.join(),
  );
});

test("with the real parser, an installed swap line in the output is marked", async () => {
  const clock = manualClock();
  const page = fakePage();
  const handle = startSwapMarker({
    page,
    roomName: fixtures.ROOM,
    agentOutput: [fixtures.swapped(1, 3, "answered", 2)],
    newLines: () => "",
    wait: clock.wait,
  });
  await clock.tick();
  await ending(clock, handle.stop());
  assert.deepEqual(page.calls, [["mark", "7:1:1"]]);
});

test("stop marks what the loop had not yet read before it returns", async () => {
  const { clock, page, handle, log } = marker();
  log("swap:7:1:9\n");
  await ending(clock, handle.stop());
  assert.deepEqual(page.calls, [["mark", "7:1:9"]]);
});

test("a page that fails to mark stops the loop and is rethrown by stop", async () => {
  const page = fakePage({ failOn: "7:1:2" });
  const { clock, handle, log } = marker({ page });
  log("swap:7:1:1\nswap:7:1:2\nswap:7:1:3\n");
  await clock.tick();
  // The loop gave up at the failure and reads nothing more on its own.
  log("swap:7:1:4\n");
  await clock.tick();
  await clock.tick();
  const rejected = assert.rejects(handle.stop(), /page refused 7:1:2/);
  await clock.tick();
  await rejected;
  // Nothing after the failure was marked, and the loop did not carry on.
  assert.deepEqual(page.calls, [["mark", "7:1:1"]]);
});

test("halt after a failed mark neither rethrows nor marks again", async () => {
  const page = fakePage({ failOn: "7:1:1" });
  const { clock, handle, log } = marker({ page });
  log("swap:7:1:1\n");
  await clock.tick();
  await ending(clock, handle.halt());
  assert.deepEqual(page.calls, []);
  // A halted loop reads nothing more, however long the log grows.
  log("swap:7:1:2\n");
  await clock.tick();
  assert.deepEqual(page.calls, []);
});

test("halt waits for the loop to finish, and a wake after it marks nothing", async () => {
  const { clock, page, handle, log } = marker();
  log("swap:7:1:5\n");
  let finished = false;
  const halting = handle.halt().then(() => {
    finished = true;
  });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(finished, false, "halt returned before the loop did");
  await clock.tick();
  await halting;
  assert.equal(finished, true);
  assert.deepEqual(page.calls, []);
});

test("the settle hands the page its grace, judges its samples and reports each episode", async () => {
  const page = fakePage();
  // Episode 1 has its fresh sample; episode 2 has none.
  page.rearms = [
    {
      swap: "7:1:1",
      seq: 1,
      status: "ok",
      reason: null,
      edgesAtMark: 4,
      windowEdge: 5,
      counterPairs: 3,
      bytesReceived: 900,
      packetsReceived: 9,
      peakRms: 0.2,
    },
  ];
  const lines = [];
  const answered = (episode) => ({
    pid: 7,
    session: 1,
    episode,
    status: "pass",
    class: "answered",
  });
  const audio = await judgeSettledSwapAudio(
    page,
    { episodes: [answered(1), answered(2)] },
    "soak",
    { graceMs: 1234, log: (line) => lines.push(line) },
  );
  assert.deepEqual(page.calls, [["settle", 1234]]);
  assert.deepEqual(
    audio.episodes.map((e) => [e.episode, e.status]),
    [
      [1, "verified"],
      [2, "missing_sample"],
    ],
  );
  assert.equal(lines.length, 2);
  // A row with no reasons carries no trailing space.
  assert.equal(lines[0], "soak episode 1 audio: verified");
  assert.match(
    lines[1],
    /^soak episode 2 audio: missing_sample no post-swap audio sample/,
  );
});

test("a page that fails to settle propagates, so the caller can still stop the probe", async () => {
  const page = {
    evaluate: async () => {
      throw new Error("page closed");
    },
  };
  await assert.rejects(
    judgeSettledSwapAudio(page, { episodes: [] }, "soak", {
      graceMs: 1,
      log: () => {},
    }),
    /page closed/,
  );
});
