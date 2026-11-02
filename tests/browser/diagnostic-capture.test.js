import assert from "node:assert/strict";
import { test } from "node:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { PassThrough } from "node:stream";
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { until, withBackstop } from "./wait.js";
const require = createRequire(import.meta.url);
const {
  DiagnosticCapture,
  captureDiagnosticStream,
  drainDiagnosticStreams,
} = require("../../scripts/floor-capture.cjs");
const floorLine =
  "codetrial floor room=synthetic session=1 socket=1 seq=1 kind=overlap outcome=continued\n";
const latencyLine =
  'codetrial latency {"room":"synthetic","stage":"tool_response","tool_names":["log_hint"],"sent":true}\n';
const gateLine =
  'codetrial replay_gate {"event":"gate_boundary","version":1,"stage":"interrupted","room":"synthetic","session":1,"socket":1}\n';
const clockLine =
  "interview clock: pid=1 session=1 start_unix_ms=10000 elapsed_ms=0 sample=0 room=synthetic\n";
function fixture(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "diagnostic-capture-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const floor = path.join(dir, "floor.log");
  const latency = path.join(dir, "latency.log");
  fs.writeFileSync(floor, "", { mode: 0o600 });
  fs.writeFileSync(latency, "", { mode: 0o600 });
  return { dir, floor, latency };
}
const read = (file) => fs.readFileSync(file, "utf8");

test("floor and full latency/clock/replacement lines stay separate, without unrelated output", (t) => {
  const f = fixture(t);
  const floor = new DiagnosticCapture(f.floor);
  const latency = new DiagnosticCapture(f.latency, "latency");
  const replacements =
    "replacement start: session=1 room=synthetic\nreplacement end: session=1 room=synthetic\n";
  const text =
    floorLine +
    "private unrelated output\n".repeat(2000) +
    clockLine +
    latencyLine +
    gateLine +
    replacements;
  for (let i = 0; i < text.length; i += 7) {
    floor.push(Buffer.from(text.slice(i, i + 7)));
    latency.push(Buffer.from(text.slice(i, i + 7)));
  }
  floor.close();
  latency.close();
  assert.equal(read(f.floor), floorLine);
  assert.equal(
    read(f.latency),
    clockLine + latencyLine + gateLine + replacements,
  );
  assert.equal(fs.statSync(f.latency).mode & 0o777, 0o600);
});

test("the byte boundary includes the newline, and oversized diagnostics leave no content", (t) => {
  const f = fixture(t);
  const capture = new DiagnosticCapture(f.floor);
  const exact =
    "codetrial floor " +
    "x".repeat(4096 - "codetrial floor ".length - 1) +
    "\n";
  capture.push(exact);
  capture.push(exact.slice(0, -1) + "x\n");
  capture.push("private".repeat(5000) + "\n");
  capture.close();
  assert.equal(
    read(f.floor),
    exact + "codetrial capture_incomplete family=floor reason=oversized-line\n",
  );
});

test("overflow detection finds diagnostic prefixes split after a long non-diagnostic prefix", (t) => {
  const f = fixture(t);
  const capture = new DiagnosticCapture(f.latency, "latency");
  capture.push("private".repeat(2000) + "codetrial lat");
  capture.push("ency secret-not-retained\n");
  capture.close();
  assert.equal(
    read(f.latency),
    "codetrial capture_incomplete family=latency reason=oversized-line\n",
  );
});

test("EOF and invalid UTF-8 cannot silently discard a diagnostic", (t) => {
  const f = fixture(t);
  const capture = new DiagnosticCapture(f.latency, "latency");
  // A two-byte fixture splits the decoder in the middle of a code point.
  const valid = Buffer.from('codetrial latency {"fixture":"λ"}\n');
  const split = valid.indexOf(Buffer.from("λ")) + 1;
  capture.push(valid.subarray(0, split));
  capture.push(valid.subarray(split));
  capture.push(
    Buffer.concat([
      Buffer.from("codetrial latency "),
      Buffer.from([0xff]),
      Buffer.from("\n"),
    ]),
  );
  capture.push("log-level " + clockLine);
  capture.push(
    Buffer.concat([Buffer.from("codetrial latency "), Buffer.from([0xe2])]),
  );
  capture.close();
  capture.close();
  assert.equal(
    read(f.latency),
    valid.toString() +
      "codetrial capture_incomplete family=latency reason=invalid-utf8\n" +
      "codetrial capture_incomplete family=latency reason=unframed-line\n" +
      "codetrial capture_incomplete family=latency reason=unterminated-line\n",
  );
  assert.throws(() => capture.push(latencyLine), /closed/);
});

test("oversized unterminated diagnostics are marked once and unrelated EOF is excluded", (t) => {
  const f = fixture(t);
  const capture = new DiagnosticCapture(f.floor);
  capture.push("codetrial floor " + "x".repeat(5000));
  capture.close();
  capture.close();
  assert.equal(
    read(f.floor),
    "codetrial capture_incomplete family=floor reason=oversized-line\n",
  );
  const other = new DiagnosticCapture(f.latency, "latency");
  other.push("unrelated private partial");
  other.close();
  assert.equal(read(f.latency), "");
});

test("two pipe buffers preserve their own split lines in one append file", async (t) => {
  const f = fixture(t);
  const first = new PassThrough();
  const second = new PassThrough();
  const captures = [first, second].map((s) =>
    captureDiagnosticStream(s, { floor: f.floor, latency: f.latency }),
  );
  first.write("codetrial fl");
  second.write("codetrial lat");
  first.end("oor " + floorLine.slice("codetrial floor ".length));
  second.end("ency " + latencyLine.slice("codetrial latency ".length));
  await drainDiagnosticStreams(captures);
  assert.equal(read(f.floor), floorLine);
  assert.equal(read(f.latency), latencyLine);
});

test("stream errors and write failures make drainage fail", async (t) => {
  const f = fixture(t);
  const stream = new PassThrough();
  const capture = captureDiagnosticStream(stream, {
    floor: f.floor,
    latency: f.latency,
  });
  stream.destroy(new Error("synthetic stream failure"));
  await assert.rejects(drainDiagnosticStreams([capture]), /capture failed/);
  assert.match(read(f.floor), /reason=stream-error/);
  assert.equal(read(f.floor).includes("synthetic stream failure"), false);
  const bad = new PassThrough();
  const failed = captureDiagnosticStream(bad, {
    floor: path.join(f.dir, "missing", "floor.log"),
  });
  bad.end(floorLine);
  await assert.rejects(drainDiagnosticStreams([failed]), /capture failed/);
});

test("a short write cannot be reported as a complete retained line", async (t) => {
  const f = fixture(t);
  const original = fs.writeSync;
  let writes = 0;
  t.mock.method(fs, "writeSync", (fd, bytes) =>
    original(fd, ++writes === 1 ? bytes.subarray(0, bytes.length - 1) : bytes),
  );
  const stream = new PassThrough();
  const capture = captureDiagnosticStream(stream, { floor: f.floor });
  stream.end(floorLine);
  await assert.rejects(drainDiagnosticStreams([capture]), /capture failed/);
  assert.equal(
    read(f.floor),
    floorLine +
      "codetrial capture_incomplete family=floor reason=write-failed\n",
  );
});

test("an unclosed pipe leaves a marker and fails a bounded drain", async (t) => {
  const f = fixture(t);
  const stream = new PassThrough();
  t.after(() => stream.destroy());
  const capture = captureDiagnosticStream(stream, {
    floor: f.floor,
    latency: f.latency,
  });
  await assert.rejects(drainDiagnosticStreams([capture], 1), /drain deadline/);
  assert.match(read(f.floor), /reason=drain-timeout/);
  assert.match(read(f.latency), /reason=drain-timeout/);
  stream.end();
  await capture.closed;
});

test("the real child pipes drain final diagnostics after targeted shutdown", async (t) => {
  const f = fixture(t);
  const script = `process.on('SIGTERM',()=>{process.stderr.write(${JSON.stringify(latencyLine + gateLine)});process.stdout.write(${JSON.stringify(floorLine)});process.exitCode=0;clearInterval(timer);});const timer=setInterval(()=>{},1000);process.stderr.write(${JSON.stringify(clockLine)});`;
  const child = spawn(process.execPath, ["-e", script]);
  t.after(() => child.kill("SIGKILL"));
  const captures = [child.stdout, child.stderr].map((s) =>
    captureDiagnosticStream(s, { floor: f.floor, latency: f.latency }),
  );
  await until(
    () => read(f.latency).includes("sample=0"),
    "the real child to start diagnostics",
  );
  child.kill("SIGTERM");
  await withBackstop(
    drainDiagnosticStreams(captures),
    "the child pipes to drain",
  );
  assert.equal(read(f.floor), floorLine);
  assert.equal(read(f.latency), clockLine + latencyLine + gateLine);
});

test("server follower keeps both files through teardown and marks trailing diagnostics", async (t) => {
  const f = fixture(t);
  const source = path.join(f.dir, "server.log");
  const done = path.join(f.dir, "done");
  fs.writeFileSync(source, floorLine + clockLine);
  const child = spawn(process.execPath, [
    "scripts/floor-capture.cjs",
    source,
    f.floor,
    done,
    f.latency,
  ]);
  t.after(() => child.kill("SIGKILL"));
  const closed = new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("exit", (code) =>
      code === 0 ? resolve() : reject(new Error(`capture exit ${code}`)),
    );
  });
  await until(
    () => read(f.latency).includes("sample=0"),
    "follower clock retention",
  );
  fs.appendFileSync(source, latencyLine + gateLine + "codetrial floor partial");
  fs.writeFileSync(done, "");
  await withBackstop(closed, "the follower final drain");
  assert.equal(read(f.latency), clockLine + latencyLine + gateLine);
  assert.equal(
    read(f.floor),
    floorLine +
      "codetrial capture_incomplete family=floor reason=unterminated-line\n",
  );
});

test("multibyte byte limits reject character-count substitution across UTF-8 splits", (t) => {
  const f = fixture(t);
  const capture = new DiagnosticCapture(f.latency, "latency");
  // Two-byte code points make a character count weaker than the byte cap.
  const prefix = "codetrial latency ";
  const count = Math.floor((4096 - prefix.length - 1) / 2);
  const exact = Buffer.from(
    prefix +
      "λ".repeat(count) +
      "x".repeat((4096 - prefix.length - 1) % 2) +
      "\n",
  );
  assert.equal(exact.length, 4096);
  const split = prefix.length + 1;
  capture.push(exact.subarray(0, split));
  capture.push(exact.subarray(split));
  capture.push(Buffer.concat([exact.subarray(0, -1), Buffer.from("x\n")]));
  capture.close();
  assert.equal(
    read(f.latency),
    exact.toString() +
      "codetrial capture_incomplete family=latency reason=oversized-line\n",
  );
});

test("an EOF fragment of a known prefix leaves an incomplete marker", (t) => {
  const f = fixture(t);
  for (const fragment of [
    "codetrial latency",
    "codetrial lat",
    "interview clock:",
  ]) {
    fs.writeFileSync(f.latency, "");
    const capture = new DiagnosticCapture(f.latency, "latency");
    capture.push(fragment);
    capture.close();
    assert.equal(
      read(f.latency),
      "codetrial capture_incomplete family=latency reason=unterminated-line\n",
    );
  }
});

async function follower(t, f, initial) {
  const source = path.join(f.dir, "server.log");
  const done = path.join(f.dir, "done");
  fs.writeFileSync(source, initial);
  const child = spawn(process.execPath, [
    "scripts/floor-capture.cjs",
    source,
    f.floor,
    done,
    f.latency,
  ]);
  t.after(() => child.kill("SIGKILL"));
  const exited = new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("exit", resolve);
  });
  await until(
    () => read(f.floor).includes("seq=1"),
    "follower initial floor retention",
  );
  return { source, done, child, exited };
}

test("source shrink and rotation cannot splice private new-source text into an old diagnostic", async (t) => {
  for (const rotate of [false, true]) {
    const f = fixture(t);
    const initial = Buffer.concat([
      Buffer.from(floorLine + "codetrial latency " + "x".repeat(200)),
      Buffer.from([0xe2]),
    ]);
    const run = await follower(t, f, initial);
    if (rotate) fs.renameSync(run.source, run.source + ".old");
    fs.writeFileSync(run.source, "UNRELATED_PRIVATE_TEXT\n" + clockLine);
    fs.writeFileSync(run.done, "");
    assert.equal(
      await withBackstop(run.exited, "follower changed source drainage"),
      0,
    );
    assert.equal(
      read(f.latency),
      "codetrial capture_incomplete family=latency reason=source-replaced-or-truncated\n" +
        clockLine,
    );
    assert.equal(read(f.latency).includes("UNRELATED_PRIVATE_TEXT"), false);
    assert.equal(
      read(f.floor).includes("reason=source-replaced-or-truncated"),
      true,
    );
  }
});

test("a source lost after retention is marked incomplete during final drainage", async (t) => {
  const f = fixture(t);
  const run = await follower(t, f, floorLine);
  fs.unlinkSync(run.source);
  fs.writeFileSync(run.done, "");
  assert.equal(
    await withBackstop(run.exited, "follower missing source drainage"),
    0,
  );
  assert.match(read(f.floor), /reason=source-missing/);
  assert.match(read(f.latency), /reason=source-missing/);
});

test("the follower cannot report a completed drain when its completion marker never arrives", async (t) => {
  const f = fixture(t);
  const run = await follower(t, f, floorLine);
  run.child.kill("SIGTERM");
  assert.equal(
    await withBackstop(run.exited, "follower abandoned drainage", 15000),
    1,
  );
  assert.match(read(f.floor), /reason=drain-timeout/);
  assert.match(read(f.latency), /reason=drain-timeout/);
});

test("a failed discontinuity marker still clears old text and decoder state", (t) => {
  const f = fixture(t);
  const capture = new DiagnosticCapture(f.latency, "latency");
  capture.push(
    Buffer.concat([Buffer.from("codetrial latency old"), Buffer.from([0xe2])]),
  );
  const original = fs.writeSync;
  const mock = t.mock.method(fs, "writeSync", () => {
    throw new Error("synthetic full disk");
  });
  assert.throws(
    () => capture.discontinuity("source-replaced-or-truncated"),
    /full disk/,
  );
  mock.mock.restore();
  assert.equal(fs.writeSync, original);
  capture.push("UNRELATED_PRIVATE_TEXT\n" + clockLine);
  capture.close();
  assert.equal(read(f.latency).trimStart(), clockLine);
  assert.equal(read(f.latency).includes("PRIVATE"), false);
});

test("an oversized incomplete UTF-8 diagnostic at EOF still leaves a marker", (t) => {
  const f = fixture(t);
  const capture = new DiagnosticCapture(f.latency, "latency");
  capture.push(
    Buffer.concat([
      Buffer.from("codetrial latency " + "x".repeat(5000)),
      Buffer.from([0xe2]),
    ]),
  );
  capture.close();
  assert.equal(
    read(f.latency),
    "codetrial capture_incomplete family=latency reason=oversized-line\n",
  );
});
