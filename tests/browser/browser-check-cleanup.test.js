import assert from "node:assert/strict";
import { test } from "node:test";
import { PassThrough } from "node:stream";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";
import { withBackstop } from "./wait.js";
const require = createRequire(import.meta.url);
const {
  cleanupBrowserCheck,
} = require("../../scripts/browser-check-cleanup.cjs");
const { captureDiagnosticStream } = require("../../scripts/floor-capture.cjs");
const { BudgetDenied } = require("../../scripts/run-budget-guard.cjs");

const finish = (options) =>
  withBackstop(cleanupBrowserCheck(options), "browser check cleanup");

function failedStream(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "check-cleanup-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const stream = new PassThrough();
  const capture = captureDiagnosticStream(stream, {
    floor: path.join(dir, "floor.log"),
  });
  stream.destroy(new Error("synthetic pipe failure"));
  return capture;
}

test("a pipe drain failure remains the cause while later resources are still closed", async (t) => {
  const calls = [];
  const capture = failedStream(t);
  const cleanup = finish({
    stopAgent: async () => {
      calls.push("agent");
    },
    streams: [capture],
    compilerExplorerMock: {
      server: {
        close(callback) {
          calls.push("mock");
          callback(new Error("later mock error"));
        },
      },
    },
    browser: {
      async close() {
        calls.push("browser");
        throw new Error("later browser error");
      },
    },
    operationFailed: false,
  });
  await assert.rejects(cleanup, /agent diagnostic capture failed/);
  assert.deepEqual(calls, ["agent", "mock", "browser"]);
});

test("a failed stop cannot skip waiting for pipe drainage and later cleanup", async (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "check-cleanup-drain-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const stream = new PassThrough();
  t.after(() => stream.destroy());
  const capture = captureDiagnosticStream(stream, {
    floor: path.join(dir, "floor.log"),
  });
  const calls = [];
  const first = new Error("stop failed");
  const cleanup = finish({
    stopAgent: async () => {
      calls.push("agent");
      throw first;
    },
    streams: [capture],
    compilerExplorerMock: {
      server: {
        close(callback) {
          calls.push("mock");
          callback();
        },
      },
    },
    browser: {
      async close() {
        calls.push("browser");
      },
    },
    operationFailed: false,
  });
  let settled = false;
  cleanup.then(
    () => {
      settled = true;
    },
    () => {
      settled = true;
    },
  );
  const rejected = assert.rejects(cleanup, (error) => error === first);
  // Flush queued promise work; the pipe is deliberately still open.
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(settled, false);
  assert.deepEqual(calls, ["agent"]);
  stream.end();
  await rejected;
  assert.deepEqual(calls, ["agent", "mock", "browser"]);
});

test("the real budget error survives multiple cleanup errors without printing private errors", async (t) => {
  const printed = [];
  t.mock.method(console, "error", (message) => printed.push(message));
  const primary = new BudgetDenied("synthetic budget denial");
  const calls = [];
  const run = async () => {
    try {
      throw primary;
    } finally {
      await finish({
        stopAgent: async () => {
          calls.push("agent");
          throw new Error("private stop detail");
        },
        streams: [failedStream(t)],
        compilerExplorerMock: {
          server: {
            close(callback) {
              calls.push("mock");
              callback(new Error("private mock detail"));
            },
          },
        },
        browser: {
          async close() {
            calls.push("browser");
            throw new Error("private browser detail");
          },
        },
        operationFailed: true,
      });
    }
  };
  await assert.rejects(
    run(),
    (error) => error === primary && error instanceof BudgetDenied,
  );
  assert.deepEqual(calls, ["agent", "mock", "browser"]);
  assert.deepEqual(printed, [
    "browser check cleanup also failed after the run failed",
  ]);
});

test("successful cleanup waits for the mock close callback before closing the browser", async () => {
  const calls = [];
  let released;
  let ready;
  const waiting = new Promise((resolve) => {
    ready = resolve;
  });
  const cleanup = finish({
    stopAgent: async () => {
      calls.push("agent");
    },
    streams: [],
    compilerExplorerMock: {
      server: {
        close(callback) {
          calls.push("mock");
          released = callback;
          ready();
        },
      },
    },
    browser: {
      async close() {
        calls.push("browser");
      },
    },
    operationFailed: false,
  });
  await withBackstop(waiting, "the mock close request");
  assert.deepEqual(calls, ["agent", "mock"]);
  released();
  await cleanup;
  assert.deepEqual(calls, ["agent", "mock", "browser"]);
});
