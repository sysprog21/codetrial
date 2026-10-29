import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { watchLauncher } = require("../../scripts/run-budget-guard.cjs");

const pings = (answers) => {
  const queue = [...answers];
  return { launcherAlive: async () => (queue.length ? queue.shift() : true) };
};
const settle = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test("one missed ping is load, not loss", async () => {
  let lost = 0;
  const timer = watchLauncher(pings([false, true, false, true]), {
    onLost: async () => {
      lost += 1;
    },
    everyMs: 5,
  });
  await settle(80);
  clearInterval(timer);
  assert.equal(lost, 0);
});

test("two missed pings in a row report the loss exactly once", async () => {
  let lost = 0;
  const timer = watchLauncher(pings([true, false, false, false, false]), {
    onLost: async () => {
      lost += 1;
    },
    everyMs: 5,
  });
  await settle(120);
  clearInterval(timer);
  assert.equal(lost, 1);
});

test("a slow loss handler is not entered twice by later ticks", async () => {
  let lost = 0;
  const timer = watchLauncher(
    pings([false, false, false, false, false, false]),
    {
      onLost: async () => {
        lost += 1;
        await settle(60);
      },
      everyMs: 5,
    },
  );
  await settle(120);
  clearInterval(timer);
  assert.equal(lost, 1);
});
