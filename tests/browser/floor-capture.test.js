import assert from "node:assert/strict";
import { test } from "node:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { until, withBackstop } from "./wait.js";
const require = createRequire(import.meta.url);
const {
  DiagnosticCapture,
  assertInterruptedOverlap,
  readFloorRecords,
} = require("../../scripts/floor-capture.cjs");
const line =
  "codetrial floor room=synthetic session=1 socket=2 seq=1 kind=overlap outcome=interrupted at=0:01 gap_ms=unknown overlap_ms=600 playout_ahead_ms=200 suspect=unknown\n";
function fixture(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "floor-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const target = path.join(dir, "floor.log");
  fs.writeFileSync(target, "", { mode: 0o600 });
  return { dir, target };
}

const persisted = (target) =>
  until(
    () => fs.readFileSync(target, "utf8").includes("seq=1"),
    "the follower to persist its first line",
  );

test("split lines survive rolling-buffer overflow and exclude other output", (t) => {
  const { target } = fixture(t);
  const capture = new DiagnosticCapture(target);
  capture.push(Buffer.from(line.slice(0, 15)));
  capture.push(Buffer.from(line.slice(15)));
  capture.push("x".repeat(20000) + "\nprivate unrelated output\n");
  capture.push(line.replace("seq=1", "seq=2"));
  assert.equal(
    fs.readFileSync(target, "utf8"),
    line + line.replace("seq=1", "seq=2"),
  );
  assertInterruptedOverlap(target, "synthetic");
  assert.deepEqual(
    readFloorRecords(target).map(({ socket, seq }) => [socket, seq]),
    [
      ["2", "1"],
      ["2", "2"],
    ],
  );
  assert.throws(
    () => assertInterruptedOverlap(target, "other"),
    /no measured interrupted/,
  );
});

test("missing, unresolved and unknown-duration overlaps fail the smoke", (t) => {
  const { target } = fixture(t);
  for (const text of [
    "",
    line.trimEnd(),
    line.replace("overlap_ms=600", "overlap_ms=unknown"),
    line.replace("outcome=interrupted", "outcome=unresolved"),
  ]) {
    fs.writeFileSync(target, text);
    assert.throws(
      () => assertInterruptedOverlap(target, "synthetic"),
      /no measured interrupted/,
    );
  }
});

test("server follower preserves early and teardown records on interruption", async (t) => {
  const { dir, target } = fixture(t);
  const source = path.join(dir, "server.log");
  fs.writeFileSync(source, line + "unrelated\n".repeat(20000));
  const child = spawn(process.execPath, [
    "scripts/floor-capture.cjs",
    source,
    target,
  ]);
  t.after(() => child.kill());
  const exited = new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("exit", (code) =>
      code === 0 ? resolve() : reject(new Error(`capture exit ${code}`)),
    );
  });
  await persisted(target);
  fs.appendFileSync(source, line.replace("seq=1", "seq=2"));
  child.kill("SIGTERM");
  await exited;
  assert.equal(
    fs.readFileSync(target, "utf8"),
    line + line.replace("seq=1", "seq=2"),
  );
});

test("completion marker before follower startup drains without a signal race", async (t) => {
  const { dir, target } = fixture(t);
  const source = path.join(dir, "server.log");
  const done = path.join(dir, "done");
  fs.writeFileSync(source, line);
  fs.writeFileSync(done, "");
  const child = spawn(process.execPath, [
    "scripts/floor-capture.cjs",
    source,
    target,
    done,
  ]);
  t.after(() => child.kill());
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error("follower startup/shutdown hung"));
    }, 5000);
    child.once("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    child.once("exit", (code) => {
      clearTimeout(timeout);
      code === 0 ? resolve() : reject(new Error(`capture exit ${code}`));
    });
  });
  assert.equal(fs.readFileSync(target, "utf8"), line);
});

test("group interruption waits for server teardown marker and final records", async (t) => {
  const { dir, target } = fixture(t);
  const source = path.join(dir, "server.log");
  const done = path.join(dir, "done");
  fs.writeFileSync(source, line);
  const child = spawn(process.execPath, [
    "scripts/floor-capture.cjs",
    source,
    target,
    done,
  ]);
  t.after(() => child.kill("SIGKILL"));
  const closed = new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("exit", (code) =>
      code === 0 ? resolve() : reject(new Error(`capture exit ${code}`)),
    );
  });
  await persisted(target);
  child.kill("SIGINT");
  // Absence proof: the follower must not exit on SIGINT, so outwait its shutdown.
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.equal(child.exitCode, null, "must keep following while server exits");
  fs.appendFileSync(source, line.replace("seq=1", "seq=2"));
  fs.writeFileSync(done, "");
  await closed;
  assert.equal(
    fs.readFileSync(target, "utf8"),
    line + line.replace("seq=1", "seq=2"),
  );
});

test("orphaned follower exits when its shell parent disappears", async (t) => {
  const { dir, target } = fixture(t);
  const source = path.join(dir, "server.log");
  const done = path.join(dir, "done");
  const pidFile = path.join(dir, "pid");
  fs.writeFileSync(source, line);
  const script = `const fs = require('fs'); const {spawn} = require('child_process'); const child = spawn(process.execPath, ['scripts/floor-capture.cjs', ${JSON.stringify(source)}, ${JSON.stringify(target)}, ${JSON.stringify(done)}], {stdio:'inherit'}); fs.writeFileSync(${JSON.stringify(pidFile)}, String(child.pid)); setInterval(()=>{},1000);`;
  const parent = spawn(process.execPath, ["-e", script]);
  t.after(() => {
    parent.kill("SIGKILL");
    if (fs.existsSync(pidFile)) {
      try {
        process.kill(Number(fs.readFileSync(pidFile, "utf8")), "SIGKILL");
      } catch {}
    }
  });
  await persisted(target);
  const closed = new Promise((resolve, reject) => {
    parent.once("close", resolve);
    parent.once("error", reject);
  });
  parent.kill("SIGKILL");
  await withBackstop(closed, "the orphaned follower to release its pipes");
  assert.equal(
    fs.readFileSync(target, "utf8"),
    line + "codetrial capture_incomplete family=floor reason=parent-lost\n",
  );
});
